//! What `m130a-scan status` and `m130a-scan scan` do, over any [`Transport`] and terminal.

use std::fs::OpenOptions;
use std::io::{BufRead, ErrorKind, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use scan_core::transport::Transport;
use scan_core::{Page, ScanError, ScanSettings, Scanner, ScannerState, write_pdf};

use crate::cli::{Command, ScanArgs};
use crate::error::CliError;

/// Where the CLI reads answers and writes output: prompts and progress go to `err`, results
/// to `out`.
#[derive(Debug)]
pub struct Console<R, O, E> {
    /// Answers to prompts.
    pub input: R,
    /// Results: the status, the saved PDF.
    pub out: O,
    /// Prompts and progress.
    pub err: E,
}

/// What Ctrl-C does: while work is running it sets that work's cancel flag; otherwise the
/// program should end, as it would without a handler.
#[derive(Debug, Default)]
pub struct Interrupt {
    busy: AtomicBool,
    cancel: AtomicBool,
}

impl Interrupt {
    /// Handles one Ctrl-C. Returns `true` when work was running and its cancel flag is now
    /// set, `false` when nothing was running and the caller should end the program.
    pub fn press(&self) -> bool {
        if self.busy.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Runs `work` with a cleared cancel flag that Ctrl-C sets instead of ending the program.
    fn busy<R>(&self, work: impl FnOnce(&AtomicBool) -> R) -> R {
        self.cancel.store(false, Ordering::SeqCst);
        self.busy.store(true, Ordering::SeqCst);
        let result = work(&self.cancel);
        self.busy.store(false, Ordering::SeqCst);
        result
    }
}

/// Runs `command`, opening the scanner with `open` only once the command's arguments allow it.
///
/// # Errors
///
/// [`CliError`] for whatever stopped the command.
pub fn run<T: Transport>(
    command: &Command,
    open: impl FnOnce() -> Result<Scanner<T>, ScanError>,
    interrupt: &Interrupt,
    console: &mut Console<impl BufRead, impl Write, impl Write>,
) -> Result<(), CliError> {
    match command {
        Command::Status => {
            let mut scanner = open().map_err(CliError::Scanner)?;
            status(&mut scanner, &mut console.out)
        }
        Command::Scan(args) => {
            if !args.force && args.out.exists() {
                return Err(CliError::Exists(args.out.clone()));
            }
            let mut scanner = open().map_err(CliError::Scanner)?;
            scan(&mut scanner, args, interrupt, console)
        }
    }
}

/// Prints the scanner's model, state, resolutions and colour modes to `out`.
///
/// # Errors
///
/// [`CliError::Scanner`] when the status cannot be read, [`CliError::Terminal`] when `out`
/// fails.
pub fn status<T: Transport>(
    scanner: &mut Scanner<T>,
    out: &mut impl Write,
) -> Result<(), CliError> {
    let state = scanner.status().map_err(CliError::Scanner)?.state;
    let capabilities = scanner.capabilities();
    let resolutions: Vec<String> = capabilities
        .resolutions
        .iter()
        .map(u16::to_string)
        .collect();
    let modes: Vec<String> = capabilities
        .color_modes
        .iter()
        .map(ToString::to_string)
        .collect();
    writeln!(out, "Model: {}", capabilities.make_and_model)?;
    writeln!(out, "State: {}", state_name(&state))?;
    writeln!(out, "Resolutions: {} dpi", resolutions.join(", "))?;
    writeln!(out, "Colour modes: {}", modes.join(", "))?;
    Ok(())
}

fn state_name(state: &ScannerState) -> &str {
    match state {
        ScannerState::Idle => "Idle",
        ScannerState::Processing => "Processing",
        ScannerState::Testing => "Testing",
        ScannerState::Stopped => "Stopped",
        ScannerState::Down => "Down",
        ScannerState::Other(name) => name,
    }
}

/// Scans pages until the user answers anything but "y", writes them as one PDF and prints its
/// path and page count to `console.out`.
///
/// # Errors
///
/// [`CliError::Scan`] when a page fails or is cancelled; nothing is written then.
/// [`CliError::Exists`] when `args.out` appeared meanwhile and `args.force` is off,
/// [`CliError::Save`] or [`CliError::Pdf`] when the PDF cannot be written,
/// [`CliError::Terminal`] when the console fails.
pub fn scan<T: Transport>(
    scanner: &mut Scanner<T>,
    args: &ScanArgs,
    interrupt: &Interrupt,
    console: &mut Console<impl BufRead, impl Write, impl Write>,
) -> Result<(), CliError> {
    let pages = scan_pages(scanner, args.settings(), interrupt, console)?;
    interrupt.busy(|_| save(&pages, &args.out, args.force))?;
    let plural = if pages.len() == 1 { "" } else { "s" };
    writeln!(
        console.out,
        "Saved {} page{plural} to {}",
        pages.len(),
        args.out.display()
    )?;
    Ok(())
}

fn scan_pages<T: Transport>(
    scanner: &mut Scanner<T>,
    settings: ScanSettings,
    interrupt: &Interrupt,
    console: &mut Console<impl BufRead, impl Write, impl Write>,
) -> Result<Vec<Page>, CliError> {
    let mut pages = Vec::new();
    loop {
        let number = pages.len() + 1;
        writeln!(
            console.err,
            "Scanning page {number} ({}, {} dpi; Ctrl-C cancels)…",
            settings.color, settings.dpi
        )?;
        let page = interrupt
            .busy(|cancel| scan_page(scanner, settings, cancel))
            .map_err(|error| CliError::Scan {
                error,
                scanned: pages.len(),
            })?;
        writeln!(console.err, "Page {number}: {}", page_size(&page))?;
        pages.push(page);
        if !another_page(console)? {
            return Ok(pages);
        }
    }
}

/// Scans one page, treating a Ctrl-C that came while the page was arriving as a cancel too:
/// `scan_page` cannot interrupt that exchange and returns the page, but the user asked to stop.
fn scan_page<T: Transport>(
    scanner: &mut Scanner<T>,
    settings: ScanSettings,
    cancel: &AtomicBool,
) -> Result<Page, ScanError> {
    let page = scanner.scan_page(&settings, cancel)?;
    if cancel.load(Ordering::SeqCst) {
        Err(ScanError::Cancelled)
    } else {
        Ok(page)
    }
}

/// Such as "1240 × 1754 pixels, 210 × 297 mm".
fn page_size(page: &Page) -> String {
    let info = page.info();
    let dpi = u64::from(page.dpi());
    let millimetres = |pixels: u32| (u64::from(pixels) * 254 + dpi * 5) / (dpi * 10);
    format!(
        "{} × {} pixels, {} × {} mm",
        info.width,
        info.height,
        millimetres(info.width),
        millimetres(info.height)
    )
}

/// Asks whether another page follows; only "y" or "yes" is yes, and end of input is no.
fn another_page(
    console: &mut Console<impl BufRead, impl Write, impl Write>,
) -> Result<bool, CliError> {
    write!(console.err, "Another page? [y/N] ")?;
    console.err.flush()?;
    let mut answer = String::new();
    if console.input.read_line(&mut answer)? == 0 {
        writeln!(console.err)?;
        return Ok(false);
    }
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Writes the PDF; without `force`, only as a new file, so a file that appeared during the scan
/// is not replaced either.
fn save(pages: &[Page], path: &Path, force: bool) -> Result<(), CliError> {
    let pdf = write_pdf(pages).map_err(CliError::Pdf)?;
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let saving = |source| CliError::Save {
        path: path.to_owned(),
        source,
    };
    let mut file = options.open(path).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            CliError::Exists(path.to_owned())
        } else {
            saving(error)
        }
    })?;
    file.write_all(&pdf).map_err(saving)
}

#[cfg(test)]
mod tests;
