//! Test doubles for the USB boundary: a [`Transport`] that plays back scripted exchanges, and
//! the fixtures captured from the printer.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use crate::transport::{Transport, TransportError};

/// One request the script expects and how the printer answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    request: String,
    response: Vec<u8>,
    split: Vec<usize>,
    read_error: Option<TransportError>,
    write_error: Option<TransportError>,
    repeat: bool,
}

impl Exchange {
    /// Expects a request with this method and path, such as `"GET /eSCL/ScannerStatus"`, and
    /// answers it with `response` in a single read.
    #[must_use]
    pub fn new(request: &str, response: Vec<u8>) -> Self {
        Self {
            request: request.to_owned(),
            response,
            split: Vec::new(),
            read_error: None,
            write_error: None,
            repeat: false,
        }
    }

    /// Answers every matching request, zero or more times, until a request matches the next
    /// exchange: for polling whose number of rounds depends on time.
    #[must_use]
    pub fn repeat(request: &str, response: Vec<u8>) -> Self {
        Self {
            repeat: true,
            ..Self::new(request, response)
        }
    }

    /// Expects this request and fails writing it with `error`.
    #[must_use]
    pub fn write_fails(request: &str, error: TransportError) -> Self {
        Self {
            write_error: Some(error),
            ..Self::new(request, Vec::new())
        }
    }

    /// Delivers the answer in reads of the given sizes; whatever they leave over comes in one
    /// final read. A 0 is an empty read, which the [`Transport`] contract rules out but a
    /// misbehaving transport could still return.
    #[must_use]
    pub fn split(mut self, sizes: Vec<usize>) -> Self {
        self.split = sizes;
        self
    }

    /// Once the answer has been read, the next read fails with `error` (instead of timing out).
    #[must_use]
    pub fn then_fail(mut self, error: TransportError) -> Self {
        self.read_error = Some(error);
        self
    }

    fn reads(&self) -> Vec<Result<Vec<u8>, TransportError>> {
        let mut rest = self.response.as_slice();
        let mut reads = Vec::new();
        for &size in &self.split {
            let (piece, tail) = rest.split_at(size.min(rest.len()));
            reads.push(Ok(piece.to_vec()));
            rest = tail;
        }
        if !rest.is_empty() {
            reads.push(Ok(rest.to_vec()));
        }
        reads.extend(self.read_error.clone().map(Err));
        reads
    }
}

/// A [`Transport`] that checks each request against a script and answers from it.
///
/// `read` never waits: it returns [`TransportError::Timeout`] as soon as nothing is queued.
/// `drain` discards everything queued; `abandon` only counts the calls (see
/// [`ScriptedTransport::abandoned`]). A request that does not match the script fails with
/// [`TransportError::Io`] and is also recorded, so [`ScriptedTransport::finish`] reports it even
/// when the caller ignored the error.
#[derive(Default)]
pub struct ScriptedTransport {
    script: VecDeque<Exchange>,
    pending: VecDeque<Result<Vec<u8>, TransportError>>,
    writes: Vec<Vec<u8>>,
    mismatches: Vec<String>,
    abandoned: usize,
    on_write: Option<Box<dyn FnMut(usize) + Send>>,
}

impl fmt::Debug for ScriptedTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptedTransport")
            .field("script", &self.script)
            .field("pending", &self.pending)
            .field("writes", &self.writes.len())
            .field("mismatches", &self.mismatches)
            .field("abandoned", &self.abandoned)
            .finish_non_exhaustive()
    }
}

impl ScriptedTransport {
    /// A transport that expects exactly these exchanges, in this order.
    pub fn new(exchanges: impl IntoIterator<Item = Exchange>) -> Self {
        Self {
            script: exchanges.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Calls `hook` on every write with the number of writes so far, this one included; for
    /// example to set a cancel flag after the Nth request.
    #[must_use]
    pub fn on_write(mut self, hook: impl FnMut(usize) + Send + 'static) -> Self {
        self.on_write = Some(Box::new(hook));
        self
    }

    /// Every request written so far, including ones that did not match the script.
    #[must_use]
    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }

    /// How many exchanges ended without reading their whole answer.
    #[must_use]
    pub fn abandoned(&self) -> usize {
        self.abandoned
    }

    /// Checks that every request matched the script and every exchange that is not a
    /// [`Exchange::repeat`] was requested.
    ///
    /// # Errors
    ///
    /// The mismatched requests and the exchanges never requested, in one message.
    pub fn finish(&self) -> Result<(), String> {
        let unrequested = self.script.iter().filter(|exchange| !exchange.repeat);
        let problems: Vec<String> = self
            .mismatches
            .iter()
            .cloned()
            .chain(unrequested.map(|exchange| {
                format!(
                    "scripted transport: {:?} was never requested",
                    exchange.request
                )
            }))
            .collect();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Takes the exchange that answers `request`: a repeat stays in the script until the request
    /// matches the exchange after it.
    fn answer(&mut self, request: &str) -> Result<Exchange, String> {
        let next_matches = self
            .script
            .get(1)
            .is_some_and(|next| next.request == request);
        if next_matches && self.script.front().is_some_and(|front| front.repeat) {
            self.script.pop_front();
        }
        let Some(expected) = self.script.pop_front() else {
            return Err(format!(
                "scripted transport: unexpected request {request:?} after the end of the script"
            ));
        };
        let result = if expected.request == request {
            Ok(expected.clone())
        } else {
            Err(format!(
                "scripted transport: expected {:?}, got {request:?}",
                expected.request
            ))
        };
        if result.is_err() || expected.repeat {
            self.script.push_front(expected);
        }
        result
    }
}

/// The method and path of a request: its first line without the HTTP version.
fn method_and_path(data: &[u8]) -> String {
    let end = data
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .unwrap_or(data.len());
    let line = String::from_utf8_lossy(&data[..end]);
    let mut parts = line.splitn(3, ' ');
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    format!("{method} {path}")
}

impl Transport for ScriptedTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.writes.push(data.to_vec());
        if let Some(hook) = &mut self.on_write {
            hook(self.writes.len());
        }
        let exchange = self.answer(&method_and_path(data)).map_err(|mismatch| {
            self.mismatches.push(mismatch.clone());
            TransportError::Io(mismatch)
        })?;
        if let Some(error) = exchange.write_error {
            return Err(error);
        }
        self.pending.extend(exchange.reads());
        Ok(())
    }

    fn read(&mut self, _deadline: Instant) -> Result<Vec<u8>, TransportError> {
        self.pending
            .pop_front()
            .unwrap_or(Err(TransportError::Timeout))
    }

    fn drain(&mut self) -> Result<usize, TransportError> {
        let count = self.pending.iter().flatten().map(Vec::len).sum();
        self.pending.clear();
        Ok(count)
    }

    fn abandon(&mut self) {
        self.abandoned += 1;
    }
}

/// Reads a file from `scan-core/tests/fixtures/`.
///
/// # Errors
///
/// The I/O error, naming the fixture's path, when the file cannot be read.
pub fn fixture(name: &str) -> io::Result<Vec<u8>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    std::fs::read(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("reading fixture {}: {error}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn read(transport: &mut ScriptedTransport) -> Result<Vec<u8>, TransportError> {
        transport.read(Instant::now())
    }

    fn request(method_and_path: &str) -> Vec<u8> {
        format!("{method_and_path} HTTP/1.1\r\nHost: localhost\r\n\r\n").into_bytes()
    }

    #[test]
    fn answers_in_the_scripted_split_and_records_writes() {
        let mut transport = ScriptedTransport::new([
            Exchange::new("GET /a", b"abcdef".to_vec()).split(vec![2, 0, 3]),
            Exchange::new("GET /b", b"xy".to_vec()),
        ]);
        transport.write(&request("GET /a")).unwrap();
        let reads: Vec<Vec<u8>> = (0..4).map(|_| read(&mut transport).unwrap()).collect();
        assert_eq!(
            reads,
            [b"ab".to_vec(), Vec::new(), b"cde".to_vec(), b"f".to_vec()]
        );
        assert_eq!(read(&mut transport), Err(TransportError::Timeout));
        transport.write(&request("GET /b")).unwrap();
        assert_eq!(read(&mut transport).unwrap(), b"xy");
        assert_eq!(transport.writes(), [request("GET /a"), request("GET /b")]);
        assert_eq!(transport.finish(), Ok(()));
    }

    #[test]
    fn split_sizes_past_the_end_give_empty_reads() {
        let exchange = Exchange::new("GET /a", b"abc".to_vec()).split(vec![5, 1]);
        let mut transport = ScriptedTransport::new([exchange]);
        transport.write(&request("GET /a")).unwrap();
        assert_eq!(read(&mut transport), Ok(b"abc".to_vec()));
        assert_eq!(read(&mut transport), Ok(Vec::new()));
        assert_eq!(read(&mut transport), Err(TransportError::Timeout));
    }

    #[test]
    fn matches_method_and_path_exactly() {
        let status = Exchange::new("GET /eSCL/ScannerStatus", b"ok".to_vec());
        let mut transport = ScriptedTransport::new([status]);
        let error = transport
            .write(&request("GET /eSCL/ScannerStatusX"))
            .unwrap_err();
        let expected = "scripted transport: expected \"GET /eSCL/ScannerStatus\", \
                        got \"GET /eSCL/ScannerStatusX\"";
        assert_eq!(error, TransportError::Io(expected.to_owned()));
        assert!(
            transport
                .write(&request("POST /eSCL/ScannerStatus"))
                .is_err()
        );
        transport
            .write(&request("GET /eSCL/ScannerStatus"))
            .unwrap();
        assert_eq!(read(&mut transport), Ok(b"ok".to_vec()));
        let report = transport.finish().unwrap_err();
        assert!(
            report.contains("ScannerStatusX") && report.contains("POST"),
            "{report}"
        );
    }

    #[test]
    fn finish_reports_requests_after_the_end_of_the_script() {
        let mut transport = ScriptedTransport::new([]);
        let error = transport.write(&request("DELETE /a")).unwrap_err();
        assert!(
            matches!(&error, TransportError::Io(message) if message.contains("end of the script"))
        );
        let report = transport.finish().unwrap_err();
        assert!(
            report.contains("\"DELETE /a\" after the end of the script"),
            "{report}"
        );
    }

    #[test]
    fn finish_reports_exchanges_never_requested() {
        let mut transport = ScriptedTransport::new([
            Exchange::new("GET /a", Vec::new()),
            Exchange::new("DELETE /b", Vec::new()),
        ]);
        transport.write(&request("GET /a")).unwrap();
        let report = transport.finish().unwrap_err();
        assert!(
            report.contains("\"DELETE /b\" was never requested"),
            "{report}"
        );
    }

    #[test]
    fn a_read_fails_once_the_answer_is_exhausted() {
        let exchange = Exchange::new("GET /a", b"ab".to_vec())
            .split(vec![1])
            .then_fail(TransportError::Disconnected);
        let mut transport = ScriptedTransport::new([exchange]);
        transport.write(&request("GET /a")).unwrap();
        assert_eq!(read(&mut transport), Ok(b"a".to_vec()));
        assert_eq!(read(&mut transport), Ok(b"b".to_vec()));
        assert_eq!(read(&mut transport), Err(TransportError::Disconnected));
        assert_eq!(read(&mut transport), Err(TransportError::Timeout));
    }

    #[test]
    fn a_write_can_fail() {
        let mut transport = ScriptedTransport::new([
            Exchange::write_fails("DELETE /a", TransportError::Disconnected),
            Exchange::new("GET /b", b"b".to_vec()),
        ]);
        let result = transport.write(&request("DELETE /a"));
        assert_eq!(result, Err(TransportError::Disconnected));
        assert_eq!(read(&mut transport), Err(TransportError::Timeout));
        transport.write(&request("GET /b")).unwrap();
        assert_eq!(read(&mut transport), Ok(b"b".to_vec()));
        assert_eq!(transport.writes().len(), 2);
        assert_eq!(transport.finish(), Ok(()));
    }

    #[test]
    fn a_repeated_exchange_answers_until_the_next_one_matches() {
        let mut transport = ScriptedTransport::new([
            Exchange::repeat("GET /next", b"busy".to_vec()),
            Exchange::new("DELETE /job", b"gone".to_vec()),
            Exchange::repeat("GET /status", b"idle".to_vec()),
        ]);
        for _ in 0..3 {
            transport.write(&request("GET /next")).unwrap();
            assert_eq!(read(&mut transport), Ok(b"busy".to_vec()));
        }
        transport.write(&request("DELETE /job")).unwrap();
        assert_eq!(read(&mut transport), Ok(b"gone".to_vec()));
        assert!(transport.write(&request("GET /next")).is_err());
        assert_eq!(
            transport
                .finish()
                .map_err(|report| report.contains("GET /next")),
            Err(true)
        );
    }

    #[test]
    fn a_repeated_exchange_may_match_no_request() {
        let mut transport = ScriptedTransport::new([
            Exchange::repeat("GET /status", b"busy".to_vec()),
            Exchange::new("POST /jobs", b"made".to_vec()),
            Exchange::repeat("GET /next", b"busy".to_vec()),
        ]);
        transport.write(&request("POST /jobs")).unwrap();
        assert_eq!(read(&mut transport), Ok(b"made".to_vec()));
        assert_eq!(transport.finish(), Ok(()));
    }

    #[test]
    fn calls_the_write_hook_with_the_write_count() {
        let seen = Arc::new(AtomicUsize::new(0));
        let hook_seen = Arc::clone(&seen);
        let mut transport = ScriptedTransport::new([
            Exchange::new("GET /a", Vec::new()),
            Exchange::new("GET /b", Vec::new()),
        ])
        .on_write(move |count| hook_seen.store(count, Ordering::SeqCst));
        transport.write(&request("GET /a")).unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        assert!(transport.write(&request("GET /x")).is_err());
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn drain_discards_and_counts_everything_queued() {
        let exchange = Exchange::new("GET /a", b"abcdef".to_vec())
            .split(vec![2])
            .then_fail(TransportError::Disconnected);
        let mut transport = ScriptedTransport::new([exchange]);
        assert_eq!(transport.drain(), Ok(0));
        transport.write(&request("GET /a")).unwrap();
        assert_eq!(transport.drain(), Ok(6));
        assert_eq!(read(&mut transport), Err(TransportError::Timeout));
    }

    #[test]
    fn counts_abandoned_exchanges() {
        let mut transport = ScriptedTransport::new([]);
        assert_eq!(transport.abandoned(), 0);
        transport.abandon();
        transport.abandon();
        assert_eq!(transport.abandoned(), 2);
    }

    #[test]
    fn debug_output_shows_the_remaining_script() {
        let transport =
            ScriptedTransport::new([Exchange::new("GET /a", Vec::new())]).on_write(|_| {});
        let debug = format!("{transport:?}");
        assert!(
            debug.starts_with("ScriptedTransport {") && debug.contains("GET /a"),
            "{debug}"
        );
    }

    #[test]
    fn loads_fixtures_and_names_missing_ones() {
        let fixture_404 = fixture("m130a-nextdocument-404.http").unwrap();
        assert!(fixture_404.starts_with(b"HTTP/1.1 404"));
        let error = fixture("no-such-fixture").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("tests/fixtures/no-such-fixture"));
    }
}
