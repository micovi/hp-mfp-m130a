use std::fs;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lopdf::Document;
use scan_core::Timing;
use scan_core::testing::{Exchange, ScriptedTransport, fixture};
use scan_core::transport::TransportError;

use super::*;

const STATUS: &str = "GET /eSCL/ScannerStatus";
const CAPABILITIES: &str = "GET /eSCL/ScannerCapabilities";
const POST: &str = "POST /eSCL/ScanJobs";
const NEXT: &str = "GET /eSCL/ScanJobs/1002/NextDocument";
const DELETE: &str = "DELETE /eSCL/ScanJobs/1002";

/// A directory of its own under the system's temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("m130a-scan-{}-{test}", process::id()));
        let _absent = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.0);
    }
}

fn answer(status_line: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn ok_with(name: &str) -> Vec<u8> {
    answer("200 OK", &fixture(name).unwrap())
}

fn idle() -> Exchange {
    Exchange::new(STATUS, ok_with("m130a-scanner-status-idle.xml"))
}

/// The exchanges of one page scanned without a hitch.
fn one_page() -> Vec<Exchange> {
    vec![
        idle(),
        Exchange::new(POST, fixture("m130a-scanjobs-201.http").unwrap()),
        Exchange::new(NEXT, fixture("m130a-nextdocument-503.http").unwrap()),
        Exchange::new(
            NEXT,
            fixture("m130a-nextdocument-200-chunked.http").unwrap(),
        ),
        Exchange::new(DELETE, answer("200 OK", b"")),
    ]
}

fn scanner(exchanges: Vec<Exchange>) -> Scanner<ScriptedTransport> {
    let capabilities = Exchange::new(CAPABILITIES, ok_with("m130a-scanner-capabilities.xml"));
    let script = std::iter::once(capabilities).chain(exchanges);
    Scanner::with_transport(ScriptedTransport::new(script), fast()).unwrap()
}

/// Polls without sleeping; `request` is not zero because scripted reads never wait.
fn fast() -> Timing {
    Timing {
        poll: Duration::ZERO,
        request: Duration::from_secs(1),
        idle_wait: Duration::ZERO,
        page: Duration::from_secs(60),
    }
}

fn console(answers: &str) -> Console<&[u8], Vec<u8>, Vec<u8>> {
    Console {
        input: answers.as_bytes(),
        out: Vec::new(),
        err: Vec::new(),
    }
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

fn grey_150(out: PathBuf) -> ScanArgs {
    ScanArgs {
        out,
        grey: true,
        dpi: 150,
        force: false,
    }
}

fn page_count(path: &Path) -> usize {
    Document::load(path).unwrap().get_pages().len()
}

fn assert_script_done(scanner: &Scanner<ScriptedTransport>) {
    assert_eq!(scanner.transport().finish(), Ok(()));
}

#[test]
fn scans_one_page_when_the_answer_is_no() {
    let dir = TempDir::new("one-page");
    let args = grey_150(dir.file("scan.pdf"));
    let mut scanner = scanner(one_page());
    let mut console = console("n\n");
    scan(&mut scanner, &args, &Interrupt::default(), &mut console).unwrap();
    assert_script_done(&scanner);
    assert_eq!(page_count(&args.out), 1);
    let summary = format!("Saved 1 page to {}\n", args.out.display());
    assert_eq!(text(&console.out), summary);
    let progress = "Scanning page 1 (grey, 150 dpi; Ctrl-C cancels)…\n\
                    Page 1: 620 × 877 pixels, 105 × 149 mm\n\
                    Another page? [y/N] ";
    assert_eq!(text(&console.err), progress);
}

#[test]
fn scans_another_page_after_yes() {
    let dir = TempDir::new("two-pages");
    let args = grey_150(dir.file("scan.pdf"));
    let mut scanner = scanner([one_page(), one_page()].concat());
    let mut console = console("y\nn\n");
    scan(&mut scanner, &args, &Interrupt::default(), &mut console).unwrap();
    assert_script_done(&scanner);
    assert_eq!(page_count(&args.out), 2);
    assert_eq!(
        text(&console.out),
        format!("Saved 2 pages to {}\n", args.out.display())
    );
    assert_eq!(
        text(&console.err).matches("Another page? [y/N] ").count(),
        2
    );
    assert!(text(&console.err).contains("Scanning page 2 "));
}

#[test]
fn end_of_input_at_the_prompt_means_no() {
    let dir = TempDir::new("eof");
    let args = grey_150(dir.file("scan.pdf"));
    let mut scanner = scanner(one_page());
    let mut console = console("");
    scan(&mut scanner, &args, &Interrupt::default(), &mut console).unwrap();
    assert_script_done(&scanner);
    assert_eq!(page_count(&args.out), 1);
    assert!(text(&console.err).ends_with("Another page? [y/N] \n"));
}

#[test]
fn only_y_or_yes_asks_for_another_page() {
    for (answer, another) in [
        ("y\n", true),
        (" YES \r\n", true),
        ("Y", true),
        ("\n", false),
        ("n\n", false),
        ("yep\n", false),
        ("no\n", false),
    ] {
        assert_eq!(
            another_page(&mut console(answer)).unwrap(),
            another,
            "{answer:?}"
        );
    }
}

#[test]
fn an_error_on_the_second_page_writes_nothing() {
    let dir = TempDir::new("second-page-fails");
    let args = grey_150(dir.file("scan.pdf"));
    let unplugged = Exchange::write_fails(STATUS, TransportError::Disconnected);
    let mut scanner = scanner([one_page(), vec![unplugged]].concat());
    let mut console = console("y\n");
    let error = scan(&mut scanner, &args, &Interrupt::default(), &mut console).unwrap_err();
    assert_script_done(&scanner);
    assert!(
        matches!(
            error,
            CliError::Scan {
                error: ScanError::Disconnected,
                scanned: 1
            }
        ),
        "{error:?}"
    );
    assert!(!args.out.exists());
    assert_eq!(text(&console.out), "");
    assert_eq!(
        error.report(),
        "Error: The printer was disconnected; reconnect its USB cable and try again. \
         No PDF was written; the page already scanned was discarded."
    );
}

/// Scans with `answers` while the script plays `exchanges`, pressing Ctrl-C as request number
/// `press_at` (counting the capabilities request as 1) is written; asserts that the press found
/// a scan running, that the script was played out and that nothing was written.
fn scan_with_ctrl_c(exchanges: Vec<Exchange>, press_at: usize, answers: &str) -> CliError {
    let dir = TempDir::new(&format!("ctrl-c-{press_at}"));
    let args = grey_150(dir.file("scan.pdf"));
    let interrupt = Arc::new(Interrupt::default());
    let pressed = Arc::new(AtomicBool::new(false));
    let (hook_interrupt, hook_pressed) = (Arc::clone(&interrupt), Arc::clone(&pressed));
    let capabilities = Exchange::new(CAPABILITIES, ok_with("m130a-scanner-capabilities.xml"));
    let transport = ScriptedTransport::new(std::iter::once(capabilities).chain(exchanges))
        .on_write(move |count| {
            if count == press_at {
                hook_pressed.store(hook_interrupt.press(), Ordering::SeqCst);
            }
        });
    let mut scanner = Scanner::with_transport(transport, fast()).unwrap();
    let error = scan(&mut scanner, &args, &interrupt, &mut console(answers)).unwrap_err();
    assert!(
        pressed.load(Ordering::SeqCst),
        "Ctrl-C should find the scan running"
    );
    assert_script_done(&scanner);
    assert!(!args.out.exists());
    error
}

#[test]
fn ctrl_c_while_waiting_for_the_page_cancels_the_scan() {
    let exchanges = vec![
        idle(),
        Exchange::new(POST, fixture("m130a-scanjobs-201.http").unwrap()),
        Exchange::new(NEXT, fixture("m130a-nextdocument-503.http").unwrap()),
        Exchange::new(DELETE, answer("200 OK", b"")),
    ];
    let error = scan_with_ctrl_c(exchanges, 4, "");
    assert_eq!(error.report(), "Scan cancelled. No PDF was written.");
}

#[test]
fn ctrl_c_while_the_page_arrives_still_cancels_the_scan() {
    let exchanges = [one_page(), one_page()].concat();
    let second_page_arrives = 10;
    let error = scan_with_ctrl_c(exchanges, second_page_arrives, "y\n");
    assert_eq!(
        error.report(),
        "Scan cancelled. No PDF was written; the page already scanned was discarded."
    );
}

#[test]
fn ctrl_c_with_nothing_running_asks_to_end_the_program() {
    let interrupt = Interrupt::default();
    assert!(!interrupt.press());
    let cancelled = interrupt.busy(|cancel| cancel.load(Ordering::SeqCst));
    assert!(
        !cancelled,
        "a Ctrl-C between scans must not cancel the next one"
    );
    assert!(!interrupt.press(), "work that ended no longer takes Ctrl-C");
}

#[test]
fn refuses_an_existing_file_before_opening_the_scanner() {
    let dir = TempDir::new("exists");
    let args = grey_150(dir.file("scan.pdf"));
    fs::write(&args.out, b"keep me").unwrap();
    let mut opened = false;
    let open = || {
        opened = true;
        Err::<Scanner<ScriptedTransport>, _>(ScanError::NotConnected)
    };
    let command = Command::Scan(args.clone());
    let error = run(&command, open, &Interrupt::default(), &mut console("")).unwrap_err();
    assert!(!opened, "the scanner must not be opened");
    assert_eq!(fs::read(&args.out).unwrap(), b"keep me");
    let expected = format!(
        "Error: {} already exists; pass --force to replace it, or choose another --out.",
        args.out.display()
    );
    assert_eq!(error.report(), expected);
}

#[test]
fn force_replaces_an_existing_file() {
    let dir = TempDir::new("force");
    let args = ScanArgs {
        force: true,
        ..grey_150(dir.file("scan.pdf"))
    };
    fs::write(&args.out, b"old").unwrap();
    let command = Command::Scan(args.clone());
    let open = || Ok(scanner(one_page()));
    run(&command, open, &Interrupt::default(), &mut console("n\n")).unwrap();
    assert_eq!(page_count(&args.out), 1);
}

#[test]
fn does_not_replace_a_file_that_appeared_during_the_scan() {
    let dir = TempDir::new("appeared");
    let args = grey_150(dir.file("scan.pdf"));
    let mut scanner = scanner(one_page());
    let out = args.out.clone();
    let mut console = Console {
        input: AppearsOnRead(&out),
        out: Vec::new(),
        err: Vec::new(),
    };
    let error = scan(&mut scanner, &args, &Interrupt::default(), &mut console).unwrap_err();
    assert!(matches!(error, CliError::Exists(_)), "{error:?}");
    assert_eq!(fs::read(&args.out).unwrap(), b"someone else's");
}

/// Answers "n", having created the file at its path first.
struct AppearsOnRead<'a>(&'a Path);

impl std::io::Read for AppearsOnRead<'_> {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        unreachable!("read through BufRead")
    }
}

impl BufRead for AppearsOnRead<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if !self.0.exists() {
            fs::write(self.0, b"someone else's")?;
        }
        Ok(b"n\n")
    }

    fn consume(&mut self, _amount: usize) {}
}

#[test]
fn reports_a_pdf_that_cannot_be_written() {
    let dir = TempDir::new("unwritable");
    let args = grey_150(dir.file("missing-folder/scan.pdf"));
    let mut scanner = scanner(one_page());
    let error = scan(
        &mut scanner,
        &args,
        &Interrupt::default(),
        &mut console("n\n"),
    )
    .unwrap_err();
    assert!(
        matches!(&error, CliError::Save { path, .. } if *path == args.out),
        "{error:?}"
    );
    let report = error.report();
    assert!(
        report.starts_with(&format!("Error: Could not write {} (", args.out.display()))
            && report.ends_with("; check that its folder exists and is writable, then scan again."),
        "{report}"
    );
}

#[test]
fn reports_a_scanner_that_cannot_be_opened() {
    let dir = TempDir::new("not-connected");
    let command = Command::Scan(grey_150(dir.file("scan.pdf")));
    let open = || Err::<Scanner<ScriptedTransport>, _>(ScanError::NotConnected);
    let error = run(&command, open, &Interrupt::default(), &mut console("")).unwrap_err();
    assert_eq!(
        error.report(),
        "Error: The printer is not connected; connect it by USB and turn it on."
    );
}

#[test]
fn reports_settings_the_scanner_does_not_offer() {
    let dir = TempDir::new("unsupported");
    let args = ScanArgs {
        dpi: 200,
        ..grey_150(dir.file("scan.pdf"))
    };
    let mut scanner = scanner(Vec::new());
    let error = scan(&mut scanner, &args, &Interrupt::default(), &mut console("")).unwrap_err();
    assert_script_done(&scanner);
    assert_eq!(
        error.report(),
        "Error: The scanner cannot scan at 200 dpi; choose one of 75, 150, 300, 600 dpi. \
         No PDF was written."
    );
}

#[test]
fn status_prints_model_state_resolutions_and_colour_modes() {
    let command = Command::Status;
    let mut console = console("");
    let open = || Ok(scanner(vec![idle()]));
    run(&command, open, &Interrupt::default(), &mut console).unwrap();
    let expected = "Model: HP LaserJet MFP M130a\n\
                    State: Idle\n\
                    Resolutions: 75, 150, 200, 300, 600, 1200 dpi\n\
                    Colour modes: colour, grey\n";
    assert_eq!(text(&console.out), expected);
    assert_eq!(text(&console.err), "");
}

#[test]
fn status_names_every_scanner_state() {
    let processing = Exchange::new(STATUS, ok_with("m130a-scanner-status-processing.xml"));
    let mut scanner = scanner(vec![processing]);
    let mut out = Vec::new();
    status(&mut scanner, &mut out).unwrap();
    assert!(text(&out).contains("State: Processing\n"));
    let states = [
        (ScannerState::Testing, "Testing"),
        (ScannerState::Stopped, "Stopped"),
        (ScannerState::Down, "Down"),
        (ScannerState::Other("Warming".to_owned()), "Warming"),
    ];
    for (state, name) in states {
        assert_eq!(state_name(&state), name);
    }
}

#[test]
fn reports_a_status_that_cannot_be_read() {
    let unplugged = Exchange::write_fails(STATUS, TransportError::Disconnected);
    let mut scanner = scanner(vec![unplugged]);
    let error = status(&mut scanner, &mut Vec::new()).unwrap_err();
    assert_eq!(
        error.report(),
        "Error: The printer was disconnected; reconnect its USB cable and try again."
    );
}

#[test]
fn the_page_size_is_rounded_to_whole_millimetres() {
    let jpeg = fixture("synthetic-grey-75dpi.jpg").unwrap();
    let a4 = Page::new(jpeg.clone(), 75).unwrap();
    assert_eq!(page_size(&a4), "620 × 877 pixels, 210 × 297 mm");
    let half = Page::new(jpeg, 150).unwrap();
    assert_eq!(page_size(&half), "620 × 877 pixels, 105 × 149 mm");
}
