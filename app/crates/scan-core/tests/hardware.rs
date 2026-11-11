//! Tests against the M130a attached by USB, with a sheet on the glass: `just app-hw-test`.
//!
//! They write what they saw to stderr directly, so it shows even when the tests pass. They never
//! save the scanned image.

use std::io::{self, Write};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use std::error::Error;
use std::thread;
use std::time::Duration;

use scan_core::escl::{job_path, parse_capabilities, scan_settings_xml};
use scan_core::http::{Request, encode_request, exchange};
use scan_core::transport::Transport;
use scan_core::{ColorMode, ScanSettings, Scanner, ScannerState, UsbTransport, printer_connected};

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
    let expected = fixture_capabilities().expect("the capabilities fixture should parse");
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

fn fixture_capabilities() -> Result<scan_core::Capabilities, Box<dyn Error>> {
    let xml = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/m130a-scanner-capabilities.xml"
    ))?;
    Ok(parse_capabilities(&xml)?)
}

const SILENCE: Duration = Duration::from_secs(10);

/// Starts a 150 dpi grey job, polls it until its page starts to arrive, reads only the first
/// piece of that answer and releases the interface with the rest of the page unread, as a
/// program killed mid-scan does.
fn abandon_a_page_partway() -> Result<(), Box<dyn Error>> {
    let mut transport = UsbTransport::open()?;
    let started = Instant::now();
    let next = format!("{}/NextDocument", start_job(&mut transport)?);
    let first = first_piece_of_page(&mut transport, &next)?;
    report(&format!(
        "abandoned {next} after its first {} bytes, {:.1?} after the scan request",
        first.len(),
        started.elapsed()
    ));
    Ok(())
}

/// Posts a 150 dpi grey job; returns its path.
fn start_job(transport: &mut UsbTransport) -> Result<String, Box<dyn Error>> {
    let settings = ScanSettings {
        color: ColorMode::Grey,
        dpi: 150,
    };
    let xml = scan_settings_xml(&settings, &fixture_capabilities()?)?;
    let post = Request {
        method: "POST",
        path: "/eSCL/ScanJobs",
        content_type: Some("text/xml"),
        body: xml.as_bytes(),
    };
    let created = exchange(transport, &post, SILENCE)?;
    let location = created
        .header("Location")
        .ok_or("the job has no Location")?;
    Ok(job_path(location)?)
}

/// Polls `next` until the answer is a 200; returns the first bytes of that answer, unread past.
fn first_piece_of_page(
    transport: &mut UsbTransport,
    next: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let poll = encode_request(&Request {
        method: "GET",
        path: next,
        content_type: None,
        body: b"",
    });
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(120) {
        transport.drain()?;
        transport.write(&poll)?;
        let first = transport.read(Instant::now() + SILENCE)?;
        if first.starts_with(b"HTTP/1.1 200") {
            return Ok(first);
        }
        if !first.starts_with(b"HTTP/1.1 503") {
            let answer = String::from_utf8_lossy(&first).into_owned();
            return Err(format!("unexpected answer: {answer:?}").into());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("the page never started to arrive".into())
}

#[test]
#[ignore = "needs the M130a attached by USB, with a sheet on the glass"]
fn recovers_from_a_page_abandoned_partway() {
    abandon_a_page_partway().expect("a page should start and be abandoned");
    let started = Instant::now();
    let mut scanner = Scanner::open().expect("the scanner should open after an abandoned page");
    report(&format!("reopened in {:.1?}", started.elapsed()));
    let status = scanner.status().expect("the status should parse");
    report(&format!("status: {status:?}"));
    let settings = ScanSettings {
        color: ColorMode::Grey,
        dpi: 75,
    };
    let started = Instant::now();
    let page = scanner
        .scan_page(&settings, &AtomicBool::new(false))
        .expect("the next page should scan");
    let info = page.info();
    report(&format!(
        "then scanned {} bytes in {:.1?}: {}x{} px",
        page.jpeg().len(),
        started.elapsed(),
        info.width,
        info.height
    ));
    assert_eq!(info.components, 1);
    assert!(info.width.abs_diff(620) <= 10, "width {}", info.width);
}
