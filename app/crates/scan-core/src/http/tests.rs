use std::time::{Duration, Instant};

use super::*;
use crate::testing::{Exchange, ScriptedTransport, fixture};

const GET: Request = Request {
    method: "GET",
    path: "/x",
    content_type: None,
    body: b"",
};
const TIMEOUT: Duration = Duration::from_secs(10);
const NO_ANSWER: Result<Response, HttpError> = Err(HttpError::Transport(TransportError::Timeout));
const INCOMPLETE: Result<Response, HttpError> = Err(HttpError::Incomplete(TransportError::Timeout));
/// The limits the plan sets, spelled out so the tests pin them rather than follow the constants.
const EIGHT_KIB: usize = 8192;
const MIB_128: usize = 134_217_728;

/// Answers one GET with `response` delivered in reads of `step` bytes.
fn respond_in_steps(response: &[u8], step: usize) -> Result<Response, HttpError> {
    let reads = vec![step; response.len().div_ceil(step)];
    let exchange = Exchange::new("GET /x", response.to_vec()).split(reads);
    exchange_with(ScriptedTransport::new([exchange]))
}

fn respond(response: &[u8]) -> Result<Response, HttpError> {
    exchange_with(ScriptedTransport::new([Exchange::new(
        "GET /x",
        response.to_vec(),
    )]))
}

fn exchange_with(mut transport: ScriptedTransport) -> Result<Response, HttpError> {
    exchange(&mut transport, &GET, TIMEOUT)
}

fn body_of(response: &[u8]) -> Vec<u8> {
    respond(response).unwrap().body
}

const CHUNKED: &str = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";

fn chunked(body: &str) -> Vec<u8> {
    format!("{CHUNKED}{body}").into_bytes()
}

fn split_request(raw: &[u8]) -> (String, String, Vec<(String, String)>, &[u8]) {
    let end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let head = std::str::from_utf8(&raw[..end]).unwrap();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap().split(' ');
    let method = request_line.next().unwrap().to_owned();
    let path = request_line.next().unwrap().to_owned();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(": ").unwrap();
            (name.to_owned(), value.to_owned())
        })
        .collect();
    (method, path, headers, &raw[end + 4..])
}

#[test]
fn encodes_the_captured_scan_job_request_byte_for_byte() {
    let raw = fixture("m130a-scanjobs-request.http").unwrap();
    let (method, path, headers, body) = split_request(&raw);
    let content_type = headers
        .iter()
        .find(|(name, _)| name == "Content-Type")
        .map(|(_, v)| v);
    let request = Request {
        method: &method,
        path: &path,
        content_type: content_type.map(String::as_str),
        body,
    };
    assert_eq!(encode_request(&request), raw);
}

#[test]
fn encodes_requests_without_a_body() {
    assert_eq!(
        encode_request(&Request {
            path: "/eSCL/ScannerStatus",
            ..GET
        }),
        b"GET /eSCL/ScannerStatus HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n"
    );
    assert_eq!(
        encode_request(&Request {
            method: "DELETE",
            path: "/eSCL/ScanJobs/1",
            ..GET
        }),
        b"DELETE /eSCL/ScanJobs/1 HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n"
    );
    assert_eq!(
        encode_request(&Request {
            method: "POST",
            content_type: Some("text/xml"),
            ..GET
        }),
        b"POST /x HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\
          Content-Type: text/xml\r\nContent-Length: 0\r\n\r\n"
    );
    assert_eq!(
        encode_request(&Request {
            method: "PUT",
            body: b"ab",
            ..GET
        }),
        b"PUT /x HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\
          Content-Length: 2\r\n\r\nab"
    );
}

fn header_list(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|&(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn fixture_responses() -> Vec<(&'static str, Response)> {
    let gsoap = |status, location: Option<&str>, connection| {
        let mut headers = vec![("Cache-Control", "no-cache, no-store, must-revalidate")];
        headers.extend(location.map(|location| ("Location", location)));
        headers.extend([("Server", "gSOAP/2.7"), ("Content-Length", "0")]);
        headers.push(("Connection", connection));
        Response {
            status,
            headers: header_list(&headers),
            body: Vec::new(),
        }
    };
    let page = Response {
        status: 200,
        headers: header_list(&[
            ("X-Content-Type-Options", "nosniff"),
            ("Cache-Control", "no-cache, no-store, must-revalidate"),
            ("Content-Type", "image/jpeg"),
            ("Transfer-Encoding", "chunked"),
        ]),
        body: fixture("synthetic-grey-75dpi.jpg").unwrap(),
    };
    vec![
        (
            "m130a-scanjobs-201.http",
            gsoap(201, Some("/eSCL/ScanJobs/1002"), "keep-alive"),
        ),
        ("m130a-nextdocument-503.http", gsoap(503, None, "close")),
        ("m130a-nextdocument-404.http", gsoap(404, None, "close")),
        ("m130a-nextdocument-200-chunked.http", page),
    ]
}

#[test]
fn parses_every_captured_response_in_every_split() {
    for (name, expected) in fixture_responses() {
        let raw = fixture(name).unwrap();
        for step in 1..=raw.len() {
            let response = respond_in_steps(&raw, step);
            assert_eq!(
                response.as_ref(),
                Ok(&expected),
                "{name} in reads of {step} bytes"
            );
        }
    }
}

#[test]
fn looks_headers_up_case_insensitively() {
    let response = respond(&fixture("m130a-scanjobs-201.http").unwrap()).unwrap();
    assert_eq!(response.header("location"), Some("/eSCL/ScanJobs/1002"));
    assert_eq!(response.header("CONTENT-length"), Some("0"));
    assert_eq!(response.header("Content-Type"), None);
    let twice = respond(b"HTTP/1.1 204 No Content\r\nX-A: 1\r\nx-a: 2\r\n\r\n").unwrap();
    assert_eq!(twice.header("X-A"), Some("1"));
}

#[test]
fn chunked_with_extensions_and_trailers_in_any_split() {
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n\
         4;name=x\r\nWiki\r\n5 \t\r\npedia\r\n0\r\nExpires: never\r\nX-Long: {}\r\n\r\n",
        "a".repeat(200)
    );
    for step in 1..=response.len() {
        let body = respond_in_steps(response.as_bytes(), step).unwrap().body;
        assert_eq!(body, b"Wikipedia", "reads of {step} bytes");
    }
}

#[test]
fn stops_at_the_end_of_the_response() {
    assert_eq!(
        body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhiHTTP/1.1"),
        b"hi"
    );
    assert_eq!(body_of(&chunked("2\r\nhi\r\n0\r\n\r\nHTTP/1.1 503")), b"hi");
}

#[test]
fn skips_interim_responses_but_not_101() {
    let stream = b"HTTP/1.1 100 Continue\r\nX-Interim: 1\r\n\r\n\
                   HTTP/1.1 102 Processing\r\nTransfer-Encoding: chunked\r\n\r\n\
                   HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    for step in 1..=stream.len() {
        let response = respond_in_steps(stream, step).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.headers, header_list(&[("Content-Length", "2")]));
        assert_eq!(response.body, b"ok");
    }
    let switching = b"HTTP/1.1 101 Switching Protocols\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n";
    assert_eq!(respond(switching), Err(HttpError::SwitchingProtocols));
}

#[test]
fn a_content_length_from_an_interim_response_does_not_leak() {
    let stream = b"HTTP/1.1 100 Continue\r\nContent-Length: 99\r\n\r\nHTTP/1.1 200 OK\r\n\r\n";
    assert_eq!(respond(stream), Err(HttpError::NoFraming));
}

#[test]
fn responses_without_a_body() {
    for response in [
        &b"HTTP/1.1 204 No Content\r\nContent-Length: 5\r\n\r\nhello"[..],
        b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n",
        b"HTTP/1.1 200\r\nContent-Length: 0 \r\n\r\n",
    ] {
        assert_eq!(
            body_of(response),
            b"",
            "{}",
            String::from_utf8_lossy(response)
        );
    }
}

#[test]
fn framing_edges() {
    assert_eq!(
        body_of(b"HTTP/1.1 200\r\ncontent-length: 2\r\n\r\nhi"),
        b"hi"
    );
    assert_eq!(
        body_of(b"HTTP/1.0 200 OK\r\nContent-Length:\t2 \r\n\r\nhi"),
        b"hi"
    );
    assert_eq!(body_of(&chunked("0\r\n\r\n")), b"");
    assert_eq!(
        body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 05\r\nContent-Length: 5\r\n\r\nhello"),
        b"hello"
    );
    let te_overrides_length = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\
                                Transfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\n\r\n";
    assert_eq!(body_of(te_overrides_length), b"hi");
    let last_of_three = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, deflate, chunked \r\n\r\n\
                          0\r\n\r\n";
    assert_eq!(body_of(last_of_three), b"");
    let two_fields = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\
                       Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    assert_eq!(body_of(two_fields), b"");
    assert_eq!(
        body_of(&chunked(
            "00000000000000000000000000000000000002\r\nhi\r\n0\r\n\r\n"
        )),
        b"hi"
    );
}

#[test]
fn rejects_malformed_status_lines() {
    for stream in [
        &b"SOAP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n"[..],
        b"HTTP/1.1 2x0 OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 20 OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1x200 OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.a 200 OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nno colon\r\n\r\n",
    ] {
        let result = respond(stream);
        assert!(
            matches!(result, Err(HttpError::BadHead(_))),
            "{}: {result:?}",
            String::from_utf8_lossy(stream)
        );
    }
    let low = respond(b"HTTP/1.1 099 Low\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(
        low,
        Err(HttpError::BadHead("status code 99 below 100".to_owned()))
    );
}

#[test]
fn a_head_ends_only_at_crlf_crlf() {
    // httparse alone would end this head at "a\n\r\n" and read "Z:" as the two-byte body.
    let lone_lf = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX: a\n\r\nZ: b\r\n\r\nhi";
    let not_crlf = Err(HttpError::BadHead("head not ended by CRLF CRLF".to_owned()));
    assert_eq!(respond(lone_lf), not_crlf);
    assert_eq!(
        respond(b"HTTP/1.1 200 OK\r\nContent-Length: 2\n\nhi"),
        not_crlf
    );
}

#[test]
fn rejects_a_nul_in_the_head_but_not_in_the_body() {
    assert_eq!(
        respond(b"HTTP/1.1 200 OK\0\r\nContent-Length: 0\r\n\r\n"),
        Err(HttpError::NulInHead)
    );
    assert_eq!(
        respond(b"HTTP/1.1 200 OK\r\nX-A: a\0b\r\nContent-Length: 0\r\n\r\n"),
        Err(HttpError::NulInHead)
    );
    assert_eq!(
        body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na\0b"),
        b"a\0b"
    );
    let malformed_then_nul_body = respond(b"HTTP/1.1 200 OK\r\nno colon\r\n\r\n\0");
    assert!(matches!(
        malformed_then_nul_body,
        Err(HttpError::BadHead(_))
    ));
}

fn head_of_length(len: usize) -> Vec<u8> {
    let start = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Filler: ";
    let filler = "a".repeat(len - start.len() - 4);
    format!("{start}{filler}\r\n\r\n").into_bytes()
}

#[test]
fn limits_the_head_to_8_kib() {
    let at_limit = head_of_length(EIGHT_KIB);
    for step in [1, 1000, EIGHT_KIB] {
        assert_eq!(respond_in_steps(&at_limit, step).map(|r| r.status), Ok(200));
    }
    let over = head_of_length(EIGHT_KIB + 1);
    for step in [1, 1000, EIGHT_KIB + 1] {
        assert_eq!(respond_in_steps(&over, step), Err(HttpError::HeadTooLarge));
    }
    let fields = "X-Field: 1\r\n".repeat(65);
    let many = format!("HTTP/1.1 200 OK\r\n{fields}Content-Length: 0\r\n\r\n");
    assert!(matches!(
        respond(many.as_bytes()),
        Err(HttpError::BadHead(_))
    ));
}

#[test]
fn rejects_bad_content_lengths() {
    for value in ["12a", "", "-1", "+1", "1 2", "0x10"] {
        let stream = format!("HTTP/1.1 200 OK\r\nContent-Length: {value}\r\n\r\n");
        assert_eq!(
            respond(stream.as_bytes()),
            Err(HttpError::BadContentLength),
            "{value:?}"
        );
    }
    let conflicting = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 2\r\n\r\nhello";
    assert_eq!(
        respond(conflicting),
        Err(HttpError::ConflictingContentLength)
    );
}

#[test]
fn requires_a_final_chunked_coding() {
    for codings in [
        "x-notchunked",
        "chunked, gzip",
        "chunk",
        "gzip",
        "chunked;q=1",
    ] {
        let stream = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: {codings}\r\n\r\nhello"
        );
        let result = respond(stream.as_bytes());
        assert_eq!(
            result,
            Err(HttpError::TransferEncodingNotChunked),
            "{codings:?}"
        );
    }
}

#[test]
fn rejects_a_response_without_framing() {
    assert_eq!(
        respond(b"HTTP/1.1 200 OK\r\n\r\nhello"),
        Err(HttpError::NoFraming)
    );
    assert_eq!(
        respond(b"HTTP/1.1 503 Busy\r\n\r\n"),
        Err(HttpError::NoFraming)
    );
}

#[test]
fn rejects_bad_chunk_sizes() {
    // 64 bytes before the CR is one too many.
    let long = format!("{}2", "0".repeat(63));
    for size_line in [
        "zz", ";ext", "+2", "-1", " 2", "0x2", "2 x", "2\r", "\n2", &long,
    ] {
        let stream = chunked(&format!("{size_line}\r\nhi\r\n0\r\n\r\n"));
        assert_eq!(
            respond(&stream),
            Err(HttpError::BadChunkSize),
            "{size_line:?}"
        );
    }
    let bare_lf = chunked("12\nhi\r\n0\r\n\r\n");
    assert_eq!(respond(&bare_lf), Err(HttpError::BadChunkSize));
    let longest = format!("{}2", "0".repeat(62));
    assert_eq!(
        body_of(&chunked(&format!("{longest}\r\nhi\r\n0\r\n\r\n"))),
        b"hi"
    );
}

#[test]
fn only_crlf_ends_a_chunk_size_line() {
    // The line runs on to the CRLF after "hi", so "hi" is not chunk data and the CRLF that
    // should follow the chunk's two bytes ("0\r") is missing.
    for size_line in ["2;a\n", "2;a\rb\n", "2;a\r\rb\n"] {
        let stream = chunked(&format!("{size_line}hi\r\n0\r\n\r\n"));
        assert_eq!(
            respond(&stream),
            Err(HttpError::ChunkOverrun),
            "{size_line:?}"
        );
    }
}

#[test]
fn rejects_bytes_after_chunk_data_other_than_crlf() {
    for after in ["XHTTP/1.1", "\n0\r\n\r\n", "\r0\r\n\r\n", "\r\r\n0\r\n\r\n"] {
        let stream = chunked(&format!("2\r\nhi{after}"));
        assert_eq!(respond(&stream), Err(HttpError::ChunkOverrun), "{after:?}");
    }
}

#[test]
fn a_chunk_needs_both_bytes_of_its_crlf() {
    let stream = chunked("2\r\nhi\r");
    assert_eq!(respond(&stream), INCOMPLETE);
}

#[test]
fn limits_the_body_to_128_mib() {
    let at_limit = format!("HTTP/1.1 200 OK\r\nContent-Length: {MIB_128}\r\n\r\n");
    let incomplete = INCOMPLETE;
    assert_eq!(respond(at_limit.as_bytes()), incomplete);
    let over = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MIB_128 + 1);
    assert_eq!(respond(over.as_bytes()), Err(HttpError::BodyTooLarge));
    let huge = "HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\n";
    assert_eq!(respond(huge.as_bytes()), Err(HttpError::BodyTooLarge));
    assert_eq!(respond(&chunked(&format!("{MIB_128:x}\r\n"))), incomplete);
    let one_over = chunked(&format!("{:x}\r\n", MIB_128 + 1));
    assert_eq!(respond(&one_over), Err(HttpError::BodyTooLarge));
    let in_total = chunked(&format!("1\r\nx\r\n{MIB_128:x}\r\n"));
    assert_eq!(respond(&in_total), Err(HttpError::BodyTooLarge));
    let overflowing = chunked(&format!("{}\r\n", "f".repeat(40)));
    assert_eq!(respond(&overflowing), Err(HttpError::BodyTooLarge));
}

#[test]
fn tells_no_answer_from_an_answer_that_stopped() {
    assert_eq!(respond(b""), NO_ANSWER);
    let partial = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello";
    assert_eq!(respond(partial), INCOMPLETE);
    assert_eq!(respond(&chunked("2\r\nhi\r\n")), INCOMPLETE);
    assert_eq!(respond(b"HTTP/1.1 100 Continue\r\n\r\n"), INCOMPLETE);
    let unfinished_trailer = chunked("0\r\nExpires: never\r\n");
    assert_eq!(respond(&unfinished_trailer), INCOMPLETE);
}

#[test]
fn empty_reads_are_not_data() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
    let reads = vec![0, 5, 0, 0, 30, 0, 3, 0];
    let exchange = Exchange::new("GET /x", response.to_vec()).split(reads);
    assert_eq!(
        exchange_with(ScriptedTransport::new([exchange]))
            .unwrap()
            .body,
        b"hi"
    );
}

/// A transport whose reads come from a closure. A read long past its deadline fails, so an
/// exchange that ignores its limits ends instead of spinning forever.
struct Reads<F>(F);

impl<F: FnMut(Instant) -> Result<Vec<u8>, TransportError>> Transport for Reads<F> {
    fn write(&mut self, _data: &[u8]) -> Result<(), TransportError> {
        Ok(())
    }

    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>, TransportError> {
        if Instant::now() > deadline + Duration::from_millis(500) {
            return Err(TransportError::Io(
                "still reading long after the deadline".to_owned(),
            ));
        }
        (self.0)(deadline)
    }

    fn drain(&mut self) -> Result<usize, TransportError> {
        Ok(0)
    }
}

const SILENCE: Duration = Duration::from_millis(40);

/// Answers the first read with `first`, and every later one by calling `then`.
fn first_then(
    first: &[u8],
    mut then: impl FnMut() -> Result<Vec<u8>, TransportError>,
) -> impl FnMut(Instant) -> Result<Vec<u8>, TransportError> {
    let mut first = Some(first.to_vec());
    move |_| first.take().map_or_else(&mut then, Ok)
}

#[test]
fn silence_before_the_first_byte_is_no_answer() {
    let started = Instant::now();
    let latest = started + SILENCE + Duration::from_millis(20);
    let mut transport = Reads(|deadline: Instant| {
        assert!(deadline <= latest, "read deadline past the silence limit");
        Ok(Vec::new())
    });
    assert_eq!(exchange(&mut transport, &GET, SILENCE), NO_ANSWER);
    assert!(started.elapsed() >= SILENCE);
}

#[test]
fn silence_mid_body_is_incomplete() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel";
    let started = Instant::now();
    let mut transport = Reads(first_then(head, || Ok(Vec::new())));
    assert_eq!(exchange(&mut transport, &GET, SILENCE), INCOMPLETE);
    assert!(started.elapsed() >= SILENCE);
}

#[test]
fn each_read_with_data_restarts_the_silence_limit() {
    let mut pieces = [
        "HTTP/1.1 200 OK\r\n",
        "Content-Length: 4\r\n",
        "\r\n",
        "a",
        "b",
        "cd",
    ]
    .map(str::as_bytes)
    .into_iter();
    let mut transport = Reads(|_| {
        std::thread::sleep(SILENCE / 2);
        Ok(pieces.next().unwrap_or_default().to_vec())
    });
    let started = Instant::now();
    let response = exchange(&mut transport, &GET, SILENCE).unwrap();
    assert_eq!(response.body, b"abcd");
    assert!(started.elapsed() > SILENCE * 2);
}

#[test]
fn a_trickling_answer_stops_at_the_overall_limit() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n";
    let mut transport = Reads(first_then(head, || {
        std::thread::sleep(Duration::from_millis(1));
        Ok(b"a".to_vec())
    }));
    let total = SILENCE * 3;
    let started = Instant::now();
    let result = exchange_with_limits(&mut transport, &GET, SILENCE, total);
    assert_eq!(result, INCOMPLETE);
    assert!(started.elapsed() >= total);
    assert!(started.elapsed() < total * 5);
}

/// Answers in pieces with an empty read between them, recording each read's deadline.
fn answer_recording_deadlines(
    deadlines: &mut Vec<Instant>,
) -> Reads<impl FnMut(Instant) -> Result<Vec<u8>, TransportError> + '_> {
    let mut pieces = ["HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n", "", "ok"]
        .map(str::as_bytes)
        .into_iter();
    Reads(move |deadline| {
        deadlines.push(deadline);
        Ok(pieces.next().unwrap_or_default().to_vec())
    })
}

#[test]
fn an_endless_silence_limit_still_waits_at_most_300_s() {
    let mut deadlines = Vec::new();
    let started = Instant::now();
    let response = exchange(
        &mut answer_recording_deadlines(&mut deadlines),
        &GET,
        Duration::MAX,
    );
    assert_eq!(response.unwrap().body, b"ok");
    assert_eq!(deadlines.len(), 3);
    for deadline in deadlines {
        let wait = deadline - started;
        assert!(
            wait > Duration::from_secs(299) && wait <= Duration::from_secs(301),
            "{wait:?}"
        );
    }
}

#[test]
fn endless_limits_do_not_overflow_the_clock() {
    let mut deadlines = Vec::new();
    let started = Instant::now();
    let response = exchange_with_limits(
        &mut answer_recording_deadlines(&mut deadlines),
        &GET,
        Duration::MAX,
        Duration::MAX,
    );
    assert_eq!(response.unwrap().body, b"ok");
    assert_eq!(deadlines.len(), 3);
    for deadline in deadlines {
        assert!(
            deadline - started >= Duration::from_secs(86_399),
            "{deadline:?}"
        );
    }
}

#[test]
fn passes_a_disconnect_through() {
    let mut gone = Reads(|_| Err(TransportError::Disconnected));
    let result = exchange(&mut gone, &GET, TIMEOUT);
    assert_eq!(
        result,
        Err(HttpError::Transport(TransportError::Disconnected))
    );
    let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n";
    let mut gone_midway = Reads(first_then(head, || Err(TransportError::Disconnected)));
    let result = exchange(&mut gone_midway, &GET, TIMEOUT);
    assert_eq!(
        result,
        Err(HttpError::Incomplete(TransportError::Disconnected))
    );
}

#[test]
fn drains_leftovers_before_writing() {
    let mut first = b"HTTP/1.1 204 No Content\r\n\r\n".to_vec();
    first.extend_from_slice(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
    let second = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec();
    let mut transport = ScriptedTransport::new([
        Exchange::new("GET /x", first).split(vec![27]),
        Exchange::new("POST /y", second),
    ]);
    assert_eq!(
        exchange(&mut transport, &GET, TIMEOUT).map(|r| r.status),
        Ok(204)
    );
    let post = Request {
        method: "POST",
        path: "/y",
        content_type: Some("text/xml"),
        body: b"<a/>",
    };
    let response = exchange(&mut transport, &post, TIMEOUT).unwrap();
    assert_eq!(
        (response.status, response.body.as_slice()),
        (200, &b"ok"[..])
    );
    assert_eq!(
        transport.writes(),
        [encode_request(&GET), encode_request(&post)]
    );
}

#[test]
fn passes_transport_errors_through() {
    let mut transport = ScriptedTransport::new([Exchange::new("DELETE /x", Vec::new())]);
    let result = exchange(&mut transport, &GET, TIMEOUT);
    assert!(
        matches!(result, Err(HttpError::Transport(TransportError::Io(_)))),
        "{result:?}"
    );
}
