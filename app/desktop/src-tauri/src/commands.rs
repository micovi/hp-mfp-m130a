//! The commands the UI invokes, and the state they share.
//!
//! Every command fails with one sentence for the user: what happened, then what to do. A
//! cancelled scan fails too (the UI started the cancel, so it does not show that sentence).

use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use scan_core::{ColorMode, Page, ScanError, ScanSettings, Scanner, write_pdf};
use tauri::{AppHandle, Runtime, State};

use crate::pages::{PageId, PageMeta, PageStore};

/// How long quitting waits for a cancelled scan to delete its job. Quitting mid-scan would leave
/// the page's data in the printer's USB pipe, where it breaks the next scan.
pub const STOP_WAIT: Duration = Duration::from_secs(1);

/// The cancel flag of the running scan; `None` while no scan runs. Each scan gets a new flag,
/// so a cancel reaches only the scan that was running when it was asked for.
type RunningScan = Mutex<Option<Arc<AtomicBool>>>;

/// The scanned pages and the scan in progress, shared by every command.
#[derive(Debug, Default)]
pub struct Desktop {
    pages: Mutex<PageStore>,
    scan: RunningScan,
}

/// Marks a scan as running until dropped, so a second one is refused.
struct ScanSlot<'a> {
    running: &'a RunningScan,
    cancel: Arc<AtomicBool>,
}

impl<'a> ScanSlot<'a> {
    fn claim(running: &'a RunningScan) -> Option<Self> {
        let mut scan = lock_scan(running);
        if scan.is_some() {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *scan = Some(Arc::clone(&cancel));
        Some(Self { running, cancel })
    }
}

impl Drop for ScanSlot<'_> {
    fn drop(&mut self) {
        *lock_scan(self.running) = None;
    }
}

/// The running scan's flag. A panic elsewhere cannot leave the `Option` half-written, so a
/// poisoned lock still holds a valid value.
fn lock_scan(running: &RunningScan) -> MutexGuard<'_, Option<Arc<AtomicBool>>> {
    running.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Desktop {
    /// No pages and no scan.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `scan` on a blocking thread and adds the page it returns.
    ///
    /// # Errors
    ///
    /// The sentence for the scan's error (including [`ScanError::Cancelled`]), or for a scan
    /// already running.
    pub async fn scan<F>(&self, settings: ScanSettings, scan: F) -> Result<PageMeta, String>
    where
        F: FnOnce(ScanSettings, &AtomicBool) -> Result<Page, ScanError> + Send + 'static,
    {
        let Some(slot) = ScanSlot::claim(&self.scan) else {
            return Err(sentence(
                "a scan is already running; wait for it to finish or cancel it",
            ));
        };
        let cancel = Arc::clone(&slot.cancel);
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
        if let Some(cancel) = &*lock_scan(&self.scan) {
            cancel.store(true, Ordering::Release);
        }
    }

    /// Whether a scan is running.
    #[must_use]
    pub fn is_scanning(&self) -> bool {
        lock_scan(&self.scan).is_some()
    }

    /// Cancels the running scan, if any, and waits up to `wait` for it to end; returns whether
    /// no scan is running any more.
    pub fn stop_scan(&self, wait: Duration) -> bool {
        self.cancel();
        let deadline = Instant::now() + wait;
        while self.is_scanning() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
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

    /// Writes the pages, in order, as one PDF at `path`, and returns how many it wrote.
    ///
    /// The page list is locked only to take the pages; the PDF is built and written on a
    /// blocking thread, into a temporary file beside `path` that then replaces it, so a failed
    /// save never leaves half a PDF. The document stays dirty if it changed meanwhile.
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
        let (pages, revision) = {
            let store = self.pages()?;
            (store.in_order(), store.revision())
        };
        let count = pages.len();
        let shown = path.clone();
        tauri::async_runtime::spawn_blocking(move || write_pdf_file(&pages, &path))
            .await
            .map_err(|error| save_error(&shown, error))??;
        self.pages()?.mark_saved(revision);
        u32::try_from(count)
            .map_err(|_| sentence(format!("{count} pages are too many; save fewer pages")))
    }

    /// Whether closing now would lose pages.
    ///
    /// # Errors
    ///
    /// The sentence for a page list broken by an internal error.
    pub fn has_unsaved_pages(&self) -> Result<bool, String> {
        Ok(self.pages()?.is_dirty())
    }

    /// The page with this id; `None` for an unknown id.
    ///
    /// # Errors
    ///
    /// The sentence for a page list broken by an internal error.
    pub fn page(&self, id: PageId) -> Result<Option<Arc<Page>>, String> {
        Ok(self.pages()?.get(id))
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

/// Writes the PDF of `pages` to `path` through a temporary file in the same folder.
fn write_pdf_file(pages: &[Arc<Page>], path: &Path) -> Result<(), String> {
    let pdf = write_pdf(pages).map_err(sentence)?;
    let Some(name) = path.file_name() else {
        return Err(sentence(format!(
            "cannot save to {}, which names no file; choose the file again",
            path.display()
        )));
    };
    let partial = path.with_file_name(format!(".{}.partial", name.to_string_lossy()));
    let written = write_synced(&partial, &pdf).and_then(|()| fs::rename(&partial, path));
    written.map_err(|error| {
        // The write's error is the one to report; the cleanup only tidies up after it, and a
        // leftover hidden `.partial` file is harmless.
        if partial.exists() {
            fs::remove_file(&partial).ok();
        }
        save_error(path, error)
    })
}

fn write_synced(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// Opens the printer's scanner for this one page and releases the USB interface afterwards.
fn scan_usb(settings: ScanSettings, cancel: &AtomicBool) -> Result<Page, ScanError> {
    Scanner::open()?.scan_page(&settings, cancel)
}

/// Stops the running scan, waiting up to [`STOP_WAIT`] so the printer is left clean, then quits.
pub fn quit_app<R: Runtime>(app: &AppHandle<R>, desktop: &Desktop) {
    desktop.stop_scan(STOP_WAIT);
    app.exit(0);
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

/// Quits without asking about unsaved pages; the UI calls it once the user confirmed. A running
/// scan is cancelled first.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes command arguments by value"
)]
pub fn quit(app: AppHandle, desktop: State<'_, Desktop>) {
    quit_app(&app, &desktop);
}

#[cfg(test)]
mod tests;
