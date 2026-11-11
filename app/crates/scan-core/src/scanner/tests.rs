use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::escl::{ColorMode, EsclError, ScanSettings, ScannerState};
use crate::testing::{Exchange, ScriptedTransport, fixture};
use crate::transport::TransportError;

const STATUS: &str = "GET /eSCL/ScannerStatus";
const CAPABILITIES: &str = "GET /eSCL/ScannerCapabilities";
const POST: &str = "POST /eSCL/ScanJobs";
const NEXT: &str = "GET /eSCL/ScanJobs/1002/NextDocument";
const DELETE: &str = "DELETE /eSCL/ScanJobs/1002";
const GREY_75: ScanSettings = ScanSettings {
    color: ColorMode::Grey,
    dpi: 75,
};

/// Polls without sleeping, waits no time for Idle and gives the page a minute; `request` is
/// not zero because a silence of zero always times out (scripted reads never wait).
fn fast() -> Timing {
    Timing {
        poll: Duration::ZERO,
        request: Duration::from_secs(1),
        idle_wait: Duration::ZERO,
        page: Duration::from_secs(60),
    }
}

/// An answer with a `Content-Length` body.
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

fn capabilities() -> Exchange {
    Exchange::new(CAPABILITIES, ok_with("m130a-scanner-capabilities.xml"))
}

fn status(state: &str) -> Exchange {
    Exchange::new(
        STATUS,
        ok_with(&format!("m130a-scanner-status-{state}.xml")),
    )
}

/// A `ScannerStatus` document in `state` listing `jobs` as (URI, job state).
fn status_with(state: &str, jobs: &[(&str, &str)]) -> Exchange {
    let mut infos = String::new();
    for (uri, job_state) in jobs {
        write!(
            infos,
            "<scan:JobInfo><pwg:JobUri>{uri}</pwg:JobUri>\
             <pwg:ImagesCompleted>0</pwg:ImagesCompleted>\
             <pwg:JobState>{job_state}</pwg:JobState></scan:JobInfo>"
        )
        .unwrap();
    }
    let xml = format!(
        r#"<scan:ScannerStatus xmlns:scan="s" xmlns:pwg="p"><pwg:Version>2.5</pwg:Version>
           <pwg:State>{state}</pwg:State><scan:Jobs>{infos}</scan:Jobs></scan:ScannerStatus>"#
    );
    Exchange::new(STATUS, answer("200 OK", xml.as_bytes()))
}

fn created() -> Exchange {
    Exchange::new(POST, fixture("m130a-scanjobs-201.http").unwrap())
}

fn busy() -> Exchange {
    Exchange::new(NEXT, fixture("m130a-nextdocument-503.http").unwrap())
}

fn page_arrives() -> Exchange {
    let response = fixture("m130a-nextdocument-200-chunked.http").unwrap();
    Exchange::new(NEXT, response).split(vec![512, 2048, 0, 100])
}

fn gone() -> Exchange {
    Exchange::new(NEXT, fixture("m130a-nextdocument-404.http").unwrap())
}

fn deleted() -> Exchange {
    Exchange::new(DELETE, answer("200 OK", b""))
}

fn synthetic_page() -> Vec<u8> {
    fixture("synthetic-grey-75dpi.jpg").unwrap()
}

/// A scanner whose script starts with the capabilities its construction asks for.
fn scanner_with(timing: Timing, exchanges: Vec<Exchange>) -> Scanner<ScriptedTransport> {
    let script = std::iter::once(capabilities()).chain(exchanges);
    Scanner::with_transport(ScriptedTransport::new(script), timing).unwrap()
}

fn scan(scanner: &mut Scanner<ScriptedTransport>) -> Result<Page, ScanError> {
    scanner.scan_page(&GREY_75, &AtomicBool::new(false))
}

fn assert_script_done(scanner: &Scanner<ScriptedTransport>) {
    assert_eq!(scanner.transport().finish(), Ok(()));
}

fn requests(scanner: &Scanner<ScriptedTransport>) -> Vec<String> {
    scanner
        .transport()
        .writes()
        .iter()
        .map(|write| {
            let text = String::from_utf8_lossy(write);
            let line = text.lines().next().unwrap_or_default();
            line.trim_end_matches(" HTTP/1.1").to_owned()
        })
        .collect()
}

fn timeout_for(error: &ScanError) -> Option<&str> {
    match error {
        ScanError::Timeout { waiting_for } => Some(waiting_for),
        _ => None,
    }
}

#[test]
fn the_observed_sequence_yields_the_page_and_deletes_the_job() {
    let delete_answers = [
        answer("200 OK", b""),
        fixture("m130a-nextdocument-404.http").unwrap(),
        fixture("m130a-nextdocument-503.http").unwrap(),
        Vec::new(),
    ];
    for delete_answer in delete_answers {
        let mut scanner = scanner_with(
            fast(),
            vec![
                status("idle"),
                created(),
                busy(),
                busy(),
                busy(),
                busy(),
                page_arrives(),
                Exchange::new(DELETE, delete_answer),
            ],
        );
        let page = scan(&mut scanner).unwrap();
        assert_eq!(page.jpeg(), synthetic_page());
        assert_eq!(page.dpi(), 75);
        assert_script_done(&scanner);
    }
}

#[test]
fn the_scan_request_is_the_one_the_printer_accepted() {
    let mut scanner = scanner_with(
        fast(),
        vec![status("idle"), created(), page_arrives(), deleted()],
    );
    scan(&mut scanner).unwrap();
    let golden = fixture("m130a-scanjobs-request.http").unwrap();
    assert_eq!(scanner.transport().writes()[2], golden);
    assert_script_done(&scanner);
}

#[test]
fn a_delete_that_cannot_be_sent_does_not_fail_the_scan() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            created(),
            page_arrives(),
            Exchange::write_fails(DELETE, TransportError::Disconnected),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap().jpeg(), synthetic_page());
    assert_script_done(&scanner);
}

#[test]
fn follows_an_absolute_job_location() {
    let location = "HTTP/1.1 201 Created\r\nLocation: http://localhost:60000/eSCL/ScanJobs/7/\r\n\
                    Content-Length: 0\r\n\r\n";
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::new(POST, location.as_bytes().to_vec()),
            Exchange::new(
                "GET /eSCL/ScanJobs/7/NextDocument",
                fixture("m130a-nextdocument-200-chunked.http").unwrap(),
            ),
            Exchange::new("DELETE /eSCL/ScanJobs/7", answer("200 OK", b"")),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap().jpeg(), synthetic_page());
    assert_script_done(&scanner);
}

#[test]
fn waits_for_the_scanner_to_become_idle() {
    let timing = Timing {
        idle_wait: Duration::from_secs(10),
        poll: Duration::from_millis(20),
        ..fast()
    };
    let mut scanner = scanner_with(
        timing,
        vec![
            status("processing"),
            status("processing"),
            status("idle"),
            created(),
            page_arrives(),
            deleted(),
        ],
    );
    let started = Instant::now();
    scan(&mut scanner).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert_script_done(&scanner);
}

#[test]
fn a_scanner_that_never_becomes_idle_is_busy() {
    let mut scanner = scanner_with(fast(), vec![status("processing")]);
    assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::ScannerBusy);
    assert_script_done(&scanner);
}

#[test]
fn keeps_asking_whether_the_scanner_is_idle_until_the_wait_is_over() {
    let timing = Timing {
        idle_wait: Duration::from_millis(100),
        poll: Duration::from_millis(10),
        ..fast()
    };
    let processing = ok_with("m130a-scanner-status-processing.xml");
    let mut scanner = scanner_with(timing, vec![Exchange::repeat(STATUS, processing)]);
    let started = Instant::now();
    assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::ScannerBusy);
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    let asked = requests(&scanner).len() - 1;
    assert!((1..=11).contains(&asked), "{asked} status requests");
    assert_script_done(&scanner);
}

#[test]
fn a_rejected_job_reports_the_status_and_needs_no_delete() {
    let cases = [
        (
            "409 Conflict",
            b"Scanner busy\r\n".as_slice(),
            409,
            "Scanner busy",
        ),
        ("400 Bad Request", b"".as_slice(), 400, ""),
        ("200 OK", b"".as_slice(), 200, ""),
    ];
    for (status_line, body, code, detail) in cases {
        let mut scanner = scanner_with(
            fast(),
            vec![
                status("idle"),
                Exchange::new(POST, answer(status_line, body)),
            ],
        );
        let expected = ScanError::JobRejected {
            status: code,
            detail: detail.to_owned(),
        };
        assert_eq!(scan(&mut scanner).unwrap_err(), expected);
        assert_script_done(&scanner);
    }
}

#[test]
fn a_created_job_without_a_usable_location_is_a_protocol_error() {
    let no_location = answer("201 Created", b"");
    let bad_location =
        b"HTTP/1.1 201 Created\r\nLocation: /eSCL/Other/1\r\nContent-Length: 0\r\n\r\n";
    for response in [no_location, bad_location.to_vec()] {
        let mut scanner = scanner_with(
            fast(),
            vec![
                status("idle"),
                Exchange::new(POST, response),
                status_with("Processing", &[("/eSCL/ScanJobs/1003", "Pending")]),
                Exchange::new("DELETE /eSCL/ScanJobs/1003", answer("200 OK", b"")),
            ],
        );
        let error = scan(&mut scanner).unwrap_err();
        assert!(matches!(error, ScanError::Protocol(_)), "{error:?}");
        assert_script_done(&scanner);
    }
}

#[test]
fn a_page_late_past_its_deadline_times_out_and_deletes_the_job() {
    let timing = Timing {
        page: Duration::ZERO,
        ..fast()
    };
    let mut scanner = scanner_with(timing, vec![status("idle"), created(), busy(), deleted()]);
    let error = scan(&mut scanner).unwrap_err();
    assert_eq!(timeout_for(&error), Some("the page"));
    assert_script_done(&scanner);
}

#[test]
fn polls_for_the_page_until_its_deadline() {
    let timing = Timing {
        page: Duration::from_millis(100),
        poll: Duration::from_millis(10),
        ..fast()
    };
    let not_yet = fixture("m130a-nextdocument-503.http").unwrap();
    let mut scanner = scanner_with(
        timing,
        vec![
            status("idle"),
            created(),
            Exchange::repeat(NEXT, not_yet),
            deleted(),
        ],
    );
    let started = Instant::now();
    let error = scan(&mut scanner).unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(timeout_for(&error), Some("the page"));
    assert!(elapsed >= Duration::from_millis(100), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    let polls = requests(&scanner)
        .iter()
        .filter(|r| r.ends_with("/NextDocument"))
        .count();
    assert!((1..=11).contains(&polls), "{polls} polls");
    assert_script_done(&scanner);
}

#[test]
fn waits_between_polls_for_the_page() {
    let timing = Timing {
        poll: Duration::from_millis(20),
        ..fast()
    };
    let mut scanner = scanner_with(
        timing,
        vec![
            status("idle"),
            created(),
            busy(),
            busy(),
            page_arrives(),
            deleted(),
        ],
    );
    let started = Instant::now();
    scan(&mut scanner).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert_script_done(&scanner);
}

#[test]
fn cancelling_while_polling_deletes_the_job() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let script = [
        capabilities(),
        status("idle"),
        created(),
        busy(),
        busy(),
        deleted(),
    ];
    let transport = ScriptedTransport::new(script).on_write(move |writes| {
        if writes == 5 {
            flag.store(true, Ordering::SeqCst);
        }
    });
    let mut scanner = Scanner::with_transport(transport, fast()).unwrap();
    let result = scanner.scan_page(&GREY_75, &cancel);
    assert_eq!(result.unwrap_err(), ScanError::Cancelled);
    assert_script_done(&scanner);
}

#[test]
fn cancelling_while_waiting_for_idle_creates_no_job() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let timing = Timing {
        idle_wait: Duration::from_secs(10),
        ..fast()
    };
    let transport = ScriptedTransport::new([capabilities(), status("processing")])
        .on_write(move |writes| flag.store(writes == 2, Ordering::SeqCst));
    let mut scanner = Scanner::with_transport(transport, timing).unwrap();
    let result = scanner.scan_page(&GREY_75, &cancel);
    assert_eq!(result.unwrap_err(), ScanError::Cancelled);
    assert_script_done(&scanner);
}

#[test]
fn a_scan_cancelled_before_it_starts_sends_nothing() {
    let mut scanner = scanner_with(fast(), Vec::new());
    let result = scanner.scan_page(&GREY_75, &AtomicBool::new(true));
    assert_eq!(result.unwrap_err(), ScanError::Cancelled);
    assert_eq!(requests(&scanner), [CAPABILITIES]);
    assert_script_done(&scanner);
}

#[test]
fn a_job_that_ends_before_any_page_has_no_page() {
    let mut scanner = scanner_with(
        fast(),
        vec![status("idle"), created(), busy(), gone(), deleted()],
    );
    assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::NoPage);
    assert_script_done(&scanner);
}

#[test]
fn an_unexpected_answer_for_the_page_is_a_protocol_error() {
    let answers = [
        (answer("500 Internal Server Error", b""), "HTTP 500"),
        (answer("200 OK", b"not a jpeg"), "not a JPEG"),
    ];
    for (response, cause) in answers {
        let mut scanner = scanner_with(
            fast(),
            vec![
                status("idle"),
                created(),
                Exchange::new(NEXT, response),
                deleted(),
            ],
        );
        let error = scan(&mut scanner).unwrap_err();
        assert!(
            matches!(&error, ScanError::Protocol(detail) if detail.contains(cause)),
            "{error:?}"
        );
        assert_script_done(&scanner);
    }
}

#[test]
fn a_page_that_never_answers_times_out_and_deletes_the_job() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            created(),
            Exchange::new(NEXT, Vec::new()),
            deleted(),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert_eq!(timeout_for(&error), Some("the page"));
    assert_script_done(&scanner);
}

#[test]
fn unplugging_the_printer_mid_page_is_a_disconnection() {
    let response = fixture("m130a-nextdocument-200-chunked.http").unwrap();
    let mid_page = Exchange::new(NEXT, response[..3000].to_vec())
        .split(vec![1000])
        .then_fail(TransportError::Disconnected);
    let before_answer = Exchange::new(NEXT, Vec::new()).then_fail(TransportError::Disconnected);
    for exchange in [mid_page, before_answer] {
        let mut scanner = scanner_with(
            fast(),
            vec![
                status("idle"),
                created(),
                exchange,
                Exchange::write_fails(DELETE, TransportError::Disconnected),
            ],
        );
        assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::Disconnected);
        assert_script_done(&scanner);
    }
}

#[test]
fn unplugging_the_printer_before_the_job_exists_is_a_disconnection() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::write_fails(POST, TransportError::Disconnected),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::Disconnected);
    assert_script_done(&scanner);
}

#[test]
fn a_silent_scan_request_times_out() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::new(POST, Vec::new()),
            status("idle"),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert_eq!(timeout_for(&error), Some("its answer to the scan request"));
    assert_script_done(&scanner);
}

#[test]
fn a_job_the_silent_scan_request_created_is_deleted() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::new(POST, Vec::new()),
            status_with("Processing", &[("/eSCL/ScanJobs/1003", "Processing")]),
            Exchange::new("DELETE /eSCL/ScanJobs/1003", answer("200 OK", b"")),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert_eq!(timeout_for(&error), Some("its answer to the scan request"));
    assert_script_done(&scanner);
}

#[test]
fn only_unfinished_jobs_this_scan_created_are_deleted() {
    let before = [
        ("/eSCL/ScanJobs/1001", "Pending"),
        ("/eSCL/ScanJobs/1002", "Completed"),
    ];
    let after = [
        ("/eSCL/ScanJobs/1001", "Pending"),
        ("/eSCL/ScanJobs/1002", "Completed"),
        ("/eSCL/ScanJobs/1003", "Completed"),
        ("/eSCL/ScanJobs/1004", "Pending"),
        ("/eSCL/ScanJobs/1005", "Processing"),
        ("/eSCL/ScanJobs/10 06", "Pending"),
    ];
    let response = b"HTTP/1.1 500 Oops\r\nTransfer-Encoding: gzip\r\n\r\n".to_vec();
    let mut scanner = scanner_with(
        fast(),
        vec![
            status_with("Idle", &before),
            Exchange::new(POST, response),
            status_with("Processing", &after),
            Exchange::new("DELETE /eSCL/ScanJobs/1004", Vec::new()),
            Exchange::new("DELETE /eSCL/ScanJobs/1005", Vec::new()),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert!(matches!(error, ScanError::Protocol(_)), "{error:?}");
    assert_script_done(&scanner);
}

#[test]
fn a_failed_status_after_a_failed_scan_request_keeps_the_first_error() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::new(POST, Vec::new()),
            Exchange::new(STATUS, answer("500 Oops", b"")),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert_eq!(timeout_for(&error), Some("its answer to the scan request"));
    assert_script_done(&scanner);
}

#[test]
fn a_scanner_busy_with_the_scan_request_is_asked_again() {
    let not_now = fixture("m130a-nextdocument-503.http").unwrap();
    let timing = Timing {
        idle_wait: Duration::from_secs(10),
        ..fast()
    };
    let mut scanner = scanner_with(
        timing,
        vec![
            status("idle"),
            Exchange::new(POST, not_now),
            status("idle"),
            created(),
            page_arrives(),
            deleted(),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap().jpeg(), synthetic_page());
    assert_script_done(&scanner);
}

#[test]
fn a_scanner_that_stays_busy_with_the_scan_request_is_busy() {
    let not_now = fixture("m130a-nextdocument-503.http").unwrap();
    let mut scanner = scanner_with(fast(), vec![status("idle"), Exchange::new(POST, not_now)]);
    assert_eq!(scan(&mut scanner).unwrap_err(), ScanError::ScannerBusy);
    assert_script_done(&scanner);
}

#[test]
fn a_stopped_or_down_scanner_needs_attention_at_once() {
    for state in ["Stopped", "Down"] {
        let timing = Timing {
            idle_wait: Duration::from_secs(10),
            poll: Duration::from_secs(10),
            ..fast()
        };
        let mut scanner = scanner_with(timing, vec![status_with(state, &[])]);
        let started = Instant::now();
        let error = scan(&mut scanner).unwrap_err();
        assert_eq!(error, ScanError::ScannerNeedsAttention, "{state}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_script_done(&scanner);
    }
}

#[test]
fn a_testing_scanner_is_waited_for() {
    let timing = Timing {
        idle_wait: Duration::from_secs(10),
        ..fast()
    };
    let mut scanner = scanner_with(
        timing,
        vec![
            status_with("Testing", &[]),
            status_with("Warming", &[]),
            status("idle"),
            created(),
            page_arrives(),
            deleted(),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap().jpeg(), synthetic_page());
    assert_script_done(&scanner);
}

#[test]
fn endless_waits_do_not_overflow_the_clock() {
    let timing = Timing {
        idle_wait: Duration::MAX,
        page: Duration::MAX,
        ..fast()
    };
    let mut scanner = scanner_with(
        timing,
        vec![
            status("processing"),
            status("idle"),
            created(),
            busy(),
            page_arrives(),
            deleted(),
        ],
    );
    assert_eq!(scan(&mut scanner).unwrap().jpeg(), synthetic_page());
    assert_script_done(&scanner);
    let colour_600 = ScanSettings {
        color: ColorMode::Colour,
        dpi: 600,
    };
    assert_eq!(timing.page_deadline(&colour_600), Duration::MAX);
}

#[test]
fn cancelling_during_a_long_poll_stops_at_once() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let timing = Timing {
        poll: Duration::from_secs(10),
        ..fast()
    };
    let script = [capabilities(), status("idle"), created(), busy(), deleted()];
    let transport = ScriptedTransport::new(script).on_write(move |writes| {
        if writes == 4 {
            let flag = Arc::clone(&flag);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                flag.store(true, Ordering::SeqCst);
            });
        }
    });
    let mut scanner = Scanner::with_transport(transport, timing).unwrap();
    let started = Instant::now();
    let result = scanner.scan_page(&GREY_75, &cancel);
    assert_eq!(result.unwrap_err(), ScanError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_script_done(&scanner);
}

#[test]
fn cancelling_during_a_long_wait_for_idle_stops_at_once() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let timing = Timing {
        poll: Duration::from_secs(10),
        idle_wait: Duration::from_secs(30),
        ..fast()
    };
    let transport =
        ScriptedTransport::new([capabilities(), status("processing")]).on_write(move |writes| {
            if writes == 2 {
                let flag = Arc::clone(&flag);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(20));
                    flag.store(true, Ordering::SeqCst);
                });
            }
        });
    let mut scanner = Scanner::with_transport(transport, timing).unwrap();
    let started = Instant::now();
    let result = scanner.scan_page(&GREY_75, &cancel);
    assert_eq!(result.unwrap_err(), ScanError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_script_done(&scanner);
}

#[test]
fn settings_the_scanner_does_not_offer_are_unsupported_and_send_nothing() {
    let mut scanner = scanner_with(fast(), Vec::new());
    let settings = ScanSettings {
        color: ColorMode::Colour,
        dpi: 200,
    };
    let error = scanner
        .scan_page(&settings, &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        matches!(
            error,
            ScanError::Unsupported(EsclError::UnsupportedResolution { dpi: 200, .. })
        ),
        "{error:?}"
    );
    assert_eq!(requests(&scanner), [CAPABILITIES]);
    assert_script_done(&scanner);
}

#[test]
fn reads_the_capabilities_when_constructed() {
    let scanner = scanner_with(fast(), Vec::new());
    let caps = scanner.capabilities();
    assert_eq!(caps.make_and_model, "HP LaserJet MFP M130a");
    assert!(caps.resolutions.contains(&300));
    assert_script_done(&scanner);
}

#[test]
fn construction_fails_when_the_capabilities_cannot_be_read() {
    let silent = || Exchange::new(CAPABILITIES, Vec::new());
    let cases = [
        vec![silent(), silent()],
        vec![Exchange::new(
            CAPABILITIES,
            answer("500 Internal Server Error", b""),
        )],
        vec![Exchange::new(CAPABILITIES, answer("200 OK", b"<oops"))],
        vec![Exchange::write_fails(
            CAPABILITIES,
            TransportError::Disconnected,
        )],
    ];
    let expected = [
        |error: &ScanError| timeout_for(error) == Some("the scanner's capabilities"),
        |error: &ScanError| matches!(error, ScanError::Protocol(d) if d.contains("HTTP 500")),
        |error: &ScanError| matches!(error, ScanError::Protocol(d) if d.contains("XML")),
        |error: &ScanError| *error == ScanError::Disconnected,
    ];
    for (exchanges, check) in cases.into_iter().zip(expected) {
        let result = Scanner::with_transport(ScriptedTransport::new(exchanges), fast());
        let error = result.unwrap_err();
        assert!(check(&error), "{error:?}");
    }
}

#[test]
fn reports_the_scanner_status() {
    let mut scanner = scanner_with(fast(), vec![status("processing"), status("idle")]);
    assert_eq!(scanner.status().unwrap().state, ScannerState::Processing);
    let idle = scanner.status().unwrap();
    assert_eq!(idle.state, ScannerState::Idle);
    assert_script_done(&scanner);
}

#[test]
fn a_status_that_cannot_be_read_is_an_error() {
    let silent = || Exchange::new(STATUS, Vec::new());
    let cases = [
        (vec![silent(), silent()], "timeout"),
        (
            vec![Exchange::new(STATUS, answer("404 Not Found", b""))],
            "HTTP 404",
        ),
        (
            vec![Exchange::new(STATUS, answer("200 OK", b"<a/>"))],
            "ScannerStatus",
        ),
    ];
    for (exchanges, cause) in cases {
        let mut scanner = scanner_with(fast(), exchanges);
        let error = scanner.status().unwrap_err();
        let fits = match &error {
            ScanError::Timeout { waiting_for } => {
                cause == "timeout" && waiting_for == "the scanner's status"
            }
            ScanError::Protocol(detail) => detail.contains(cause),
            _ => false,
        };
        assert!(fits, "{error:?}");
        assert_script_done(&scanner);
    }
}

#[test]
fn default_timing_follows_the_design() {
    let timing = Timing::default();
    assert_eq!(timing.poll, Duration::from_secs(1));
    assert_eq!(timing.request, Duration::from_secs(10));
    assert_eq!(timing.idle_wait, Duration::from_secs(30));
    assert_eq!(timing.page, Duration::from_secs(60));
}

#[test]
fn the_page_deadline_grows_with_resolution_and_colour() {
    let table = [
        (ColorMode::Grey, 75, 60),
        (ColorMode::Colour, 75, 90),
        (ColorMode::Grey, 150, 60),
        (ColorMode::Grey, 200, 80),
        (ColorMode::Colour, 150, 180),
        (ColorMode::Grey, 300, 120),
        (ColorMode::Colour, 300, 240),
        (ColorMode::Grey, 600, 240),
        (ColorMode::Colour, 600, 240),
        (ColorMode::Colour, u16::MAX, 240),
    ];
    let timing = Timing::default();
    for (color, dpi, seconds) in table {
        let deadline = timing.page_deadline(&ScanSettings { color, dpi });
        assert_eq!(deadline.as_secs(), seconds, "{color} {dpi} dpi");
    }
}

#[test]
fn the_page_deadline_scales_with_the_base() {
    let timing = Timing {
        page: Duration::from_millis(10),
        ..fast()
    };
    let colour_150 = ScanSettings {
        color: ColorMode::Colour,
        dpi: 150,
    };
    assert_eq!(timing.page_deadline(&colour_150), Duration::from_millis(30));
    assert_eq!(timing.page_deadline(&GREY_75), Duration::from_millis(10));
}

/// The start of a JPEG: what the printer sends while it is still sending a page that an
/// earlier exchange abandoned.
fn stale_page() -> Vec<u8> {
    synthetic_page()[..64].to_vec()
}

#[test]
fn capabilities_are_asked_again_after_an_answer_that_is_not_http() {
    for garbage in [
        stale_page(),
        b"<scan:Jobs>left over</scan:Jobs>\r\n\r\n".to_vec(),
        vec![b'x'; 9000],
    ] {
        let script = [
            Exchange::new(CAPABILITIES, garbage).split(vec![16]),
            capabilities(),
        ];
        let scanner = Scanner::with_transport(ScriptedTransport::new(script), fast()).unwrap();
        assert_eq!(requests(&scanner), [CAPABILITIES, CAPABILITIES]);
        assert_eq!(scanner.transport().abandoned(), 1);
        assert_script_done(&scanner);
    }
}

#[test]
fn the_status_is_asked_again_after_a_silence_or_a_cut_off_answer() {
    for first in [Vec::new(), b"HTTP/1.1 200 OK\r\nContent-Le".to_vec()] {
        let mut scanner = scanner_with(fast(), vec![Exchange::new(STATUS, first), status("idle")]);
        assert_eq!(scanner.status().unwrap().state, ScannerState::Idle);
        assert_eq!(scanner.transport().abandoned(), 1);
        assert_script_done(&scanner);
    }
}

#[test]
fn a_document_request_is_asked_again_only_once() {
    let script = [
        Exchange::new(CAPABILITIES, stale_page()),
        Exchange::new(CAPABILITIES, stale_page()),
    ];
    let error = Scanner::with_transport(ScriptedTransport::new(script), fast()).unwrap_err();
    assert!(
        matches!(&error, ScanError::Protocol(detail) if detail.contains("NUL")),
        "{error:?}"
    );
}

#[test]
fn answers_that_are_http_or_a_lost_printer_are_not_asked_again() {
    let cases = [
        Exchange::new(STATUS, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()),
        Exchange::new(STATUS, chunked_with_bad_size()),
        Exchange::new(STATUS, b"HTTP/1.1 200".to_vec()).then_fail(TransportError::Disconnected),
        Exchange::write_fails(STATUS, TransportError::Io("stalled".to_owned())),
    ];
    for exchange in cases {
        let mut scanner = scanner_with(fast(), vec![exchange]);
        assert!(scanner.status().is_err());
        assert_script_done(&scanner);
    }
}

fn chunked_with_bad_size() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n".to_vec()
}

#[test]
fn the_page_request_is_never_asked_again() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            created(),
            Exchange::new(NEXT, stale_page()),
            deleted(),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert!(matches!(error, ScanError::Protocol(_)), "{error:?}");
    assert_eq!(scanner.transport().abandoned(), 1);
    assert_script_done(&scanner);
}

#[test]
fn the_scan_request_is_never_asked_again() {
    let mut scanner = scanner_with(
        fast(),
        vec![
            status("idle"),
            Exchange::new(POST, stale_page()),
            status("idle"),
        ],
    );
    let error = scan(&mut scanner).unwrap_err();
    assert!(matches!(error, ScanError::Protocol(_)), "{error:?}");
    assert_script_done(&scanner);
}
