//! Tests against the M130a attached by USB, with a sheet on the glass: `just app-hw-test`.
//!
//! They write what they saw to stderr directly, so it shows even when the tests pass. They never
//! save the scanned image.

use std::io::{self, Write};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use scan_core::escl::parse_capabilities;
use scan_core::{ColorMode, ScanSettings, Scanner, ScannerState, printer_connected};

const NEEDS_PRINTER: &str = "needs the M130a attached by USB";

fn report(line: &str) {
    let _ignored = writeln!(io::stderr(), "    {line}");
}

#[test]
#[ignore = "needs the M130a attached by USB"]
fn opens_and_reads_the_same_capabilities_as_the_fixture() {
    assert!(printer_connected(), "{NEEDS_PRINTER}");
    let scanner = Scanner::open().expect("the scanner should open");
    report(&format!("opened: {scanner:?}"));
    let fixture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/m130a-scanner-capabilities.xml"
    ))
    .expect("the capabilities fixture should be readable");
    let expected = parse_capabilities(&fixture).expect("the fixture should parse");
    assert_eq!(scanner.capabilities(), &expected);
}

#[test]
#[ignore = "needs the M130a attached by USB"]
fn reads_the_status() {
    let mut scanner = Scanner::open().expect("the scanner should open");
    let status = scanner.status().expect("the status should parse");
    report(&format!("status: {status:?}"));
    assert!(
        matches!(status.state, ScannerState::Idle | ScannerState::Processing),
        "{status:?}"
    );
}

#[test]
#[ignore = "needs the M130a attached by USB, with a sheet on the glass"]
fn scans_a_grey_75_dpi_page() {
    let mut scanner = Scanner::open().expect("the scanner should open");
    let settings = ScanSettings {
        color: ColorMode::Grey,
        dpi: 75,
    };
    let started = Instant::now();
    let page = scanner
        .scan_page(&settings, &AtomicBool::new(false))
        .expect("the page should scan");
    let elapsed = started.elapsed();
    let info = page.info();
    report(&format!(
        "scanned {} bytes in {elapsed:.1?}: {}x{} px, {} component(s), progressive {}",
        page.jpeg().len(),
        info.width,
        info.height,
        info.components,
        info.progressive
    ));
    assert_eq!(page.dpi(), 75);
    assert_eq!(info.components, 1);
    assert!(info.width.abs_diff(620) <= 10, "width {}", info.width);
    assert!(info.height.abs_diff(877) <= 10, "height {}", info.height);
}
