//! Test doubles for the USB boundary: a [`Transport`] that plays back scripted exchanges, and
//! the fixtures captured from the printer.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use crate::transport::{Transport, TransportError};

/// One request the script expects and the bytes the printer answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    /// What the written request must start with, such as `"GET /eSCL/ScannerStatus"`.
    pub request_line_prefix: String,
    /// The raw answer, possibly several responses or trailing bytes.
    pub response: Vec<u8>,
    /// Sizes of the successive reads that return the answer; whatever they leave over comes in
    /// one final read. A 0 is an empty read, which the [`Transport`] contract rules out but a
    /// misbehaving transport could still return.
    pub split: Vec<usize>,
}

impl Exchange {
    /// An exchange whose answer arrives in a single read.
    #[must_use]
    pub fn new(request_line_prefix: &str, response: Vec<u8>) -> Self {
        Self {
            request_line_prefix: request_line_prefix.to_owned(),
            response,
            split: Vec::new(),
        }
    }

    /// Delivers the answer in reads of the given sizes.
    #[must_use]
    pub fn split(mut self, sizes: Vec<usize>) -> Self {
        self.split = sizes;
        self
    }

    fn pieces(&self) -> Vec<Vec<u8>> {
        let mut rest = self.response.as_slice();
        let mut pieces = Vec::new();
        for &size in &self.split {
            let (piece, tail) = rest.split_at(size.min(rest.len()));
            pieces.push(piece.to_vec());
            rest = tail;
        }
        if !rest.is_empty() {
            pieces.push(rest.to_vec());
        }
        pieces
    }
}

/// A [`Transport`] that checks each request against a script and answers from it.
///
/// `read` never waits: it returns [`TransportError::Timeout`] as soon as nothing is queued.
#[derive(Debug, Default)]
pub struct ScriptedTransport {
    script: VecDeque<Exchange>,
    pending: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl ScriptedTransport {
    /// A transport that expects exactly these exchanges, in this order.
    pub fn new(exchanges: impl IntoIterator<Item = Exchange>) -> Self {
        Self {
            script: exchanges.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Every request written so far, including ones that did not match the script.
    #[must_use]
    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }
}

fn request_line(data: &[u8]) -> String {
    let end = data
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

impl Transport for ScriptedTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.writes.push(data.to_vec());
        let line = request_line(data);
        let Some(expected) = self.script.pop_front() else {
            return Err(TransportError::Io(format!(
                "scripted transport: unexpected request {line:?} after the end of the script"
            )));
        };
        if !data.starts_with(expected.request_line_prefix.as_bytes()) {
            return Err(TransportError::Io(format!(
                "scripted transport: expected a request starting with {:?}, got {line:?}",
                expected.request_line_prefix
            )));
        }
        self.pending.extend(expected.pieces());
        Ok(())
    }

    fn read(&mut self, _deadline: Instant) -> Result<Vec<u8>, TransportError> {
        self.pending.pop_front().ok_or(TransportError::Timeout)
    }

    fn drain(&mut self) -> Result<usize, TransportError> {
        let count = self.pending.iter().map(Vec::len).sum();
        self.pending.clear();
        Ok(count)
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
    use super::*;

    #[test]
    fn answers_in_the_scripted_split_and_records_writes() {
        let mut transport = ScriptedTransport::new([
            Exchange::new("GET /a", b"abcdef".to_vec()).split(vec![2, 0, 3]),
            Exchange::new("GET /b", b"xy".to_vec()),
        ]);
        transport.write(b"GET /a HTTP/1.1\r\n\r\n").unwrap();
        let reads: Vec<Vec<u8>> = (0..4)
            .map(|_| transport.read(Instant::now()).unwrap())
            .collect();
        assert_eq!(
            reads,
            [b"ab".to_vec(), Vec::new(), b"cde".to_vec(), b"f".to_vec()]
        );
        assert_eq!(transport.read(Instant::now()), Err(TransportError::Timeout));
        transport.write(b"GET /b HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(transport.read(Instant::now()).unwrap(), b"xy");
        assert_eq!(transport.writes().len(), 2);
    }

    #[test]
    fn split_sizes_past_the_end_give_empty_reads() {
        let exchange = Exchange::new("GET /", b"abc".to_vec()).split(vec![5, 1]);
        assert_eq!(exchange.pieces(), [b"abc".to_vec(), Vec::new()]);
    }

    #[test]
    fn rejects_a_request_the_script_does_not_expect() {
        let mut transport = ScriptedTransport::new([Exchange::new("GET /a", b"x".to_vec())]);
        let error = transport.write(b"POST /a HTTP/1.1\r\n\r\n").unwrap_err();
        assert_eq!(
            error,
            TransportError::Io(
                "scripted transport: expected a request starting with \"GET /a\", \
                 got \"POST /a HTTP/1.1\""
                    .to_owned()
            )
        );
        let error = transport.write(b"GET /a HTTP/1.1\r\n\r\n").unwrap_err();
        assert!(
            matches!(error, TransportError::Io(message) if message.contains("end of the script"))
        );
        assert_eq!(transport.writes().len(), 2);
    }

    #[test]
    fn drain_discards_and_counts_queued_bytes() {
        let mut transport =
            ScriptedTransport::new([Exchange::new("GET /", b"abcdef".to_vec()).split(vec![2])]);
        assert_eq!(transport.drain(), Ok(0));
        transport.write(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(transport.drain(), Ok(6));
        assert_eq!(transport.read(Instant::now()), Err(TransportError::Timeout));
    }

    #[test]
    fn loads_fixtures_and_names_missing_ones() {
        assert!(
            fixture("m130a-nextdocument-404.http")
                .unwrap()
                .starts_with(b"HTTP/1.1 404")
        );
        let error = fixture("no-such-fixture").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("tests/fixtures/no-such-fixture"));
    }
}
