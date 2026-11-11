use super::*;
use crate::escl::ColorMode;

fn timeout(waiting_for: &str) -> ScanError {
    ScanError::Timeout {
        waiting_for: waiting_for.to_owned(),
    }
}

#[test]
fn every_error_is_one_sentence_saying_what_happened_and_what_to_do() {
    let unsupported = EsclError::UnsupportedColorMode {
        color: ColorMode::Colour,
        supported: vec![ColorMode::Grey],
    };
    let cases = [
        (
            ScanError::NotConnected,
            "the printer is not connected; connect it by USB and turn it on",
        ),
        (
            ScanError::InterfaceBusy,
            "the printer's USB interface is in use by another app; quit that app and try again",
        ),
        (
            ScanError::ScannerBusy,
            "the scanner is busy; try again in a moment",
        ),
        (
            ScanError::ScannerNeedsAttention,
            "the scanner needs attention; check the printer's display, then try again",
        ),
        (
            ScanError::JobRejected {
                status: 409,
                detail: "Conflict".to_owned(),
            },
            "the scanner refused the scan (HTTP 409: Conflict); \
             check the printer's display, then try again",
        ),
        (
            ScanError::JobRejected {
                status: 400,
                detail: String::new(),
            },
            "the scanner refused the scan (HTTP 400); check the printer's display, then try again",
        ),
        (
            timeout("the page"),
            "the printer took too long to send the page; \
             check that it is on and not showing an error, then try again",
        ),
        (
            ScanError::Disconnected,
            "the printer was disconnected; reconnect its USB cable and try again",
        ),
        (
            ScanError::Cancelled,
            "the scan was cancelled; start a new scan when ready",
        ),
        (
            ScanError::NoPage,
            "the scanner finished without sending a page; wait a moment, then scan again",
        ),
        (
            ScanError::Unsupported(unsupported),
            "the scanner cannot scan in colour; choose grey",
        ),
        (
            ScanError::Protocol("HTTP 500 for the page".to_owned()),
            "communication with the printer failed (HTTP 500 for the page); \
             turn the printer off and on, then try again",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected, "{error:?}");
    }
}

#[test]
fn a_lost_or_cut_short_answer_from_an_unplugged_printer_is_a_disconnection() {
    for error in [
        HttpError::Transport(TransportError::Disconnected),
        HttpError::Incomplete(TransportError::Disconnected),
    ] {
        let scan_error = ScanError::from_http(error, "the page");
        assert!(
            matches!(scan_error, ScanError::Disconnected),
            "{scan_error:?}"
        );
    }
}

#[test]
fn silence_names_what_was_awaited() {
    for error in [
        HttpError::Transport(TransportError::Timeout),
        HttpError::Incomplete(TransportError::Timeout),
    ] {
        let scan_error = ScanError::from_http(error, "the scanner's status");
        assert!(
            matches!(&scan_error, ScanError::Timeout { waiting_for }
                if waiting_for == "the scanner's status"),
            "{scan_error:?}"
        );
    }
}

#[test]
fn other_failures_are_protocol_errors_that_keep_their_cause() {
    let cases = [
        (
            HttpError::Transport(TransportError::Io("pipe error".to_owned())),
            "USB I/O failed: pipe error",
        ),
        (
            HttpError::Incomplete(TransportError::Io("pipe error".to_owned())),
            "the printer stopped partway through its answer: USB I/O failed: pipe error",
        ),
        (
            HttpError::NoFraming,
            "the printer sent an HTTP response with neither Content-Length nor chunked encoding",
        ),
    ];
    for (error, cause) in cases {
        let scan_error = ScanError::from_http(error, "the page");
        assert!(
            matches!(&scan_error, ScanError::Protocol(detail) if detail == cause),
            "{scan_error:?}"
        );
    }
}

#[test]
fn a_rejection_shows_the_first_line_of_the_body() {
    let rejected = ScanError::rejected(409, b"  Scanner busy  \r\nmore detail\r\n");
    assert!(
        matches!(&rejected, ScanError::JobRejected { status: 409, detail }
            if detail == "Scanner busy"),
        "{rejected:?}"
    );
    let empty = ScanError::rejected(400, b"");
    assert!(
        matches!(&empty, ScanError::JobRejected { status: 400, detail } if detail.is_empty()),
        "{empty:?}"
    );
}

#[test]
fn a_rejection_detail_is_cut_to_a_readable_length() {
    let body = format!("{}é{}", "x".repeat(DETAIL_MAX - 1), "y".repeat(50));
    let rejected = ScanError::rejected(400, body.as_bytes());
    let shortened = format!("{}é…", "x".repeat(DETAIL_MAX - 1));
    assert!(
        matches!(&rejected, ScanError::JobRejected { detail, .. } if *detail == shortened),
        "{rejected:?}"
    );
    let exact = "z".repeat(DETAIL_MAX);
    let rejected = ScanError::rejected(400, exact.as_bytes());
    assert!(
        matches!(&rejected, ScanError::JobRejected { detail, .. } if *detail == exact),
        "{rejected:?}"
    );
}

#[test]
fn a_rejection_body_that_is_not_utf8_is_still_shown() {
    let rejected = ScanError::rejected(400, b"bad \xff byte");
    assert!(
        matches!(&rejected, ScanError::JobRejected { detail, .. } if detail == "bad \u{fffd} byte"),
        "{rejected:?}"
    );
}

#[test]
fn a_rejection_skips_markup_and_blank_lines() {
    let html = b"<html>\r\n  <body>\r\n\r\n  Scanner jammed  \r\n</body>\r\n";
    let rejected = ScanError::rejected(400, html);
    assert!(
        matches!(&rejected, ScanError::JobRejected { detail, .. } if detail == "Scanner jammed"),
        "{rejected:?}"
    );
    let xml = b"<?xml version=\"1.0\"?><Error>no</Error>\n";
    let rejected = ScanError::rejected(409, xml);
    assert!(
        matches!(&rejected, ScanError::JobRejected { detail, .. } if detail.is_empty()),
        "{rejected:?}"
    );
}
