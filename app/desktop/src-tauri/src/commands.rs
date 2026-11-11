//! The commands the UI invokes, and the state they share.
//!
//! Every command fails with one sentence for the user: what happened, then what to do. A
//! cancelled scan fails too (the UI started the cancel, so it does not show that sentence).

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use scan_core::{ColorMode, Page, ScanError, ScanSettings, Scanner, write_pdf};
use tauri::{AppHandle, State};

use crate::pages::{PageId, PageMeta, PageStore};

/// The scanned pages and the scan in progress, shared by every command.
#[derive(Debug, Default)]
pub struct Desktop {
    pages: Mutex<PageStore>,
    cancel: Arc<AtomicBool>,
    scanning: AtomicBool,
}

/// Marks a scan as running until dropped, so a second one is refused.
struct ScanSlot<'a>(&'a AtomicBool);

impl<'a> ScanSlot<'a> {
    fn claim(scanning: &'a AtomicBool) -> Option<Self> {
        scanning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(Self(scanning))
    }
}

impl Drop for ScanSlot<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Desktop {
    /// No pages and no scan.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `scan` on a blocking thread and adds the page it returns.
    ///
    /// Clears the cancel flag first, so a [`Desktop::cancel`] left over from an earlier scan
    /// does not stop this one.
    ///
    /// # Errors
    ///
    /// The sentence for the scan's error (including [`ScanError::Cancelled`]), or for a scan
    /// already running.
    pub async fn scan<F>(&self, settings: ScanSettings, scan: F) -> Result<PageMeta, String>
    where
        F: FnOnce(ScanSettings, &AtomicBool) -> Result<Page, ScanError> + Send + 'static,
    {
        let Some(_slot) = ScanSlot::claim(&self.scanning) else {
            return Err(sentence(
                "a scan is already running; wait for it to finish or cancel it",
            ));
        };
        self.cancel.store(false, Ordering::Release);
        let cancel = Arc::clone(&self.cancel);
        let scanned = tauri::async_runtime::spawn_blocking(move || scan(settings, &cancel))
            .await
            .map_err(|error| {
                sentence(format!(
                    "the scan stopped unexpectedly ({error}); try again"
                ))
            })?;
        let page = scanned.map_err(sentence)?;
        Ok(self.pages()?.add(page))
    }

    /// Asks the running scan, if any, to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Removes a page.
    ///
    /// # Errors
    ///
    /// The sentence for an unknown page.
    pub fn delete(&self, id: PageId) -> Result<(), String> {
        self.pages()?.delete(id).map_err(sentence)
    }

    /// Puts the pages in the order of `ids`.
    ///
    /// # Errors
    ///
    /// The sentence for `ids` that are not a permutation of the pages.
    pub fn reorder(&self, ids: &[PageId]) -> Result<(), String> {
        self.pages()?.reorder(ids).map_err(sentence)
    }

    /// Writes the pages, in order, as one PDF at `path`, and returns how many it wrote. The
    /// document stays dirty if it changed while the file was written.
    ///
    /// # Errors
    ///
    /// The sentence for a relative path, no pages, or the OS error with the path.
    pub async fn save(&self, path: PathBuf) -> Result<u32, String> {
        if !path.is_absolute() {
            return Err(sentence(format!(
                "cannot save to {}, which is not a full path; choose the file again",
                path.display()
            )));
        }
        let (pdf, count, revision) = {
            let pages = self.pages()?;
            let pdf = write_pdf(&pages.in_order()).map_err(sentence)?;
            (pdf, pages.len(), pages.revision())
        };
        let written = path.clone();
        tauri::async_runtime::spawn_blocking(move || std::fs::write(&written, pdf))
            .await
            .map_err(|error| save_error(&path, error))?
            .map_err(|error| save_error(&path, error))?;
        self.pages()?.mark_saved(revision);
        u32::try_from(count).map_err(|_| sentence(format!("{count} pages are too many to count")))
    }

    /// Whether closing now would lose pages.
    ///
    /// # Errors
    ///
    /// The sentence for a page list broken by an internal error.
    pub fn has_unsaved_pages(&self) -> Result<bool, String> {
        Ok(self.pages()?.is_dirty())
    }

    /// The JPEG of a page: `None` for an unknown id.
    ///
    /// # Errors
    ///
    /// The sentence for a page list broken by an internal error.
    pub fn page_jpeg(&self, id: PageId) -> Result<Option<Vec<u8>>, String> {
        Ok(self.pages()?.get(id).map(|page| page.jpeg().to_vec()))
    }

    fn pages(&self) -> Result<MutexGuard<'_, PageStore>, String> {
        self.pages.lock().map_err(|_| {
            sentence("the page list is unavailable after an internal error; restart the app")
        })
    }
}

/// A message in scan-core's style (lowercase, no final period) as a sentence for the UI.
pub fn sentence(message: impl Display) -> String {
    let message = message.to_string();
    let mut chars = message.chars();
    let mut text: String = chars
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    text.push_str(chars.as_str());
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

fn save_error(path: &Path, error: impl Display) -> String {
    sentence(format!(
        "could not save the PDF to {}: {error}; choose another location and try again",
        path.display()
    ))
}

/// Opens the printer's scanner for this one page and releases the USB interface afterwards.
fn scan_usb(settings: ScanSettings, cancel: &AtomicBool) -> Result<Page, ScanError> {
    Scanner::open()?.scan_page(&settings, cancel)
}

/// Whether the printer is plugged in by USB; claims nothing.
///
/// # Errors
///
/// The sentence for a check that could not run.
#[tauri::command]
pub async fn printer_connected() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(scan_core::printer_connected)
        .await
        .map_err(|error| sentence(format!("the printer check failed ({error}); try again")))
}

/// Scans one page: `color` true for colour, false for grey.
///
/// # Errors
///
/// The scan's sentence (also when cancelled), or one for a scan already running.
#[tauri::command]
pub async fn scan_page(
    desktop: State<'_, Desktop>,
    color: bool,
    dpi: u16,
) -> Result<PageMeta, String> {
    let color = if color {
        ColorMode::Colour
    } else {
        ColorMode::Grey
    };
    desktop.scan(ScanSettings { color, dpi }, scan_usb).await
}

/// Asks the running scan, if any, to stop; its `scan_page` then fails with the cancel sentence.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn cancel_scan(desktop: State<'_, Desktop>) {
    desktop.cancel();
}

/// Removes a page.
///
/// # Errors
///
/// The sentence for an unknown page.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn delete_page(desktop: State<'_, Desktop>, id: PageId) -> Result<(), String> {
    desktop.delete(id)
}

/// Puts the pages in the order of `ids`, which must list each page once.
///
/// # Errors
///
/// The sentence for ids that are not a permutation of the pages.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn reorder_pages(desktop: State<'_, Desktop>, ids: Vec<PageId>) -> Result<(), String> {
    desktop.reorder(&ids)
}

/// Saves the pages as one PDF at `path` and returns how many it wrote.
///
/// # Errors
///
/// The sentence for no pages or a failed write.
#[tauri::command]
pub async fn save_pdf(desktop: State<'_, Desktop>, path: PathBuf) -> Result<u32, String> {
    desktop.save(path).await
}

/// Whether there are pages changed since the last save.
///
/// # Errors
///
/// The sentence for a page list broken by an internal error.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn has_unsaved_pages(desktop: State<'_, Desktop>) -> Result<bool, String> {
    desktop.has_unsaved_pages()
}

/// Quits without asking; the UI calls it once the user confirmed losing unsaved pages.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests;
