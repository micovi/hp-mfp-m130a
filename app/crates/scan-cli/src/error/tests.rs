use scan_core::EsclError;

use super::*;

#[test]
fn sentence_capitalises_and_ends_with_a_period() {
    assert_eq!(sentence(&"the scanner is busy"), "The scanner is busy.");
    assert_eq!(sentence(&"ünicode first"), "Ünicode first.");
    assert_eq!(sentence(&""), "");
}

#[test]
fn a_failed_page_says_how_many_pages_were_discarded() {
    let report = |scanned| {
        CliError::Scan {
            error: ScanError::ScannerBusy,
            scanned,
        }
        .report()
    };
    let busy = "Error: The scanner is busy; try again in a moment.";
    assert_eq!(report(0), format!("{busy} No PDF was written."));
    assert_eq!(
        report(1),
        format!("{busy} No PDF was written; the page already scanned was discarded.")
    );
    assert_eq!(
        report(3),
        format!("{busy} No PDF was written; the 3 pages already scanned were discarded.")
    );
}

#[test]
fn a_cancelled_scan_is_not_an_error() {
    let cancelled = CliError::Scan {
        error: ScanError::Cancelled,
        scanned: 2,
    };
    assert_eq!(
        cancelled.report(),
        "Scan cancelled. No PDF was written; the 2 pages already scanned were discarded."
    );
}

#[test]
fn other_errors_are_one_sentence() {
    let unsupported = ScanError::Unsupported(EsclError::UnsupportedResolution {
        dpi: 600,
        supported: vec![150, 300],
    });
    assert_eq!(
        CliError::Scanner(unsupported).report(),
        "Error: The scanner cannot scan at 600 dpi; choose one of 150, 300 dpi."
    );
    assert_eq!(
        CliError::Pdf(PdfError::NoPages).report(),
        "Error: There are no pages to save."
    );
    let terminal = CliError::from(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"));
    assert_eq!(
        terminal.report(),
        "Error: The terminal failed (pipe closed)."
    );
}
