//! HTTP/1.1 over a [`Transport`]: request encoding and response framing.
//!
//! IPP-over-USB carries plain HTTP/1.1 on a pair of bulk endpoints with no message boundaries,
//! so a response ends only where its `Content-Length` or chunked encoding says it does. The
//! framing rules follow the firmware's `http_framer.c`, except where httparse is more lenient
//! in the head: it skips empty lines before the status line and accepts a lone LF between header
//! fields; over 64 header fields is a [`HttpError::BadHead`].

use std::time::{Duration, Instant};

use crate::transport::{Transport, TransportError};

/// One HTTP request to the printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    /// `GET`, `POST`, `DELETE`, …
    pub method: &'a str,
    /// The request target, such as `/eSCL/ScannerStatus`.
    pub path: &'a str,
    /// The `Content-Type` header, when there is one.
    pub content_type: Option<&'a str>,
    /// The request body; empty for none.
    pub body: &'a [u8],
}

/// One complete HTTP response, after any interim (1xx) responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code, such as 200.
    pub status: u16,
    /// Header fields in the order received, names as sent.
    pub headers: Vec<(String, String)>,
    /// The body, reassembled when it was chunked.
    pub body: Vec<u8>,
}

impl Response {
    /// The value of the first header named `name`, compared case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Why an HTTP exchange failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The transport failed, or nothing arrived in time.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// Part of the response arrived, then the transport failed or went silent.
    #[error("the printer stopped partway through its answer: {0}")]
    Incomplete(TransportError),
    /// The response head contains a NUL byte.
    #[error("the printer sent a malformed HTTP response: NUL byte in the head")]
    NulInHead,
    /// The response head is longer than 8 KiB.
    #[error("the printer sent a malformed HTTP response: head over 8 KiB")]
    HeadTooLarge,
    /// The status line or a header field does not parse.
    #[error("the printer sent a malformed HTTP response: {0}")]
    BadHead(String),
    /// A 101 response: the printer wants another protocol on this connection.
    #[error("the printer answered 101 Switching Protocols, which this client does not support")]
    SwitchingProtocols,
    /// `Content-Length` is not a decimal number.
    #[error("the printer sent a malformed HTTP response: bad Content-Length")]
    BadContentLength,
    /// Two `Content-Length` fields with different values.
    #[error("the printer sent a malformed HTTP response: conflicting Content-Length")]
    ConflictingContentLength,
    /// `Transfer-Encoding` whose last coding is not `chunked`: the body would run to connection
    /// close, which USB framing cannot represent.
    #[error("the printer sent a malformed HTTP response: Transfer-Encoding without final chunked")]
    TransferEncodingNotChunked,
    /// Neither `Content-Length` nor chunked encoding on a response that has a body.
    #[error("the printer sent an HTTP response with neither Content-Length nor chunked encoding")]
    NoFraming,
    /// A chunk-size line that is not hex digits optionally followed by an extension.
    #[error("the printer sent a malformed HTTP response: bad chunk size")]
    BadChunkSize,
    /// Bytes other than CRLF right after a chunk's data.
    #[error("the printer sent a malformed HTTP response: chunk data longer than its size")]
    ChunkOverrun,
    /// The body would be over 128 MiB.
    #[error("the printer's HTTP response body is over 128 MiB")]
    BodyTooLarge,
}

/// Encodes `request` the way `probe/httpipp.py` (and ipp-usb) send requests to USB printers.
#[must_use]
pub fn encode_request(request: &Request) -> Vec<u8> {
    let mut lines = vec![
        format!("{} {} HTTP/1.1", request.method, request.path),
        "Host: localhost".to_owned(),
        "Connection: keep-alive".to_owned(),
    ];
    if let Some(content_type) = request.content_type {
        lines.push(format!("Content-Type: {content_type}"));
    }
    if !request.body.is_empty() || request.method == "POST" {
        lines.push(format!("Content-Length: {}", request.body.len()));
    }
    let mut encoded = format!("{}\r\n\r\n", lines.join("\r\n")).into_bytes();
    encoded.extend_from_slice(request.body);
    encoded
}

/// The longest a response may take from the request, however steadily its bytes arrive.
const RESPONSE_MAX_DURATION: Duration = Duration::from_secs(300);

/// Sends `request` and reads one response.
///
/// Bytes left over from an earlier exchange are drained first. Silence is measured by the clock:
/// the response's first bytes must arrive within `silence` of sending, and each later read with
/// data must follow the previous one within `silence`; empty reads never extend it. The whole
/// response must also arrive within 300 s of sending.
///
/// # Errors
///
/// [`HttpError::Transport`] when the transport fails or nothing arrives in time;
/// [`HttpError::Incomplete`] when part of the response arrived before that; any other
/// [`HttpError`] when the response breaks HTTP/1.1 framing.
pub fn exchange<T: Transport>(
    transport: &mut T,
    request: &Request,
    silence: Duration,
) -> Result<Response, HttpError> {
    exchange_with_limits(transport, request, silence, RESPONSE_MAX_DURATION)
}

/// [`exchange`] with the overall limit as a parameter, so tests need not wait 300 s.
fn exchange_with_limits<T: Transport>(
    transport: &mut T,
    request: &Request,
    silence: Duration,
    total: Duration,
) -> Result<Response, HttpError> {
    transport.drain()?;
    transport.write(&encode_request(request))?;
    let sent = Instant::now();
    let mut quiet_until = sent + silence;
    let mut received = false;
    let mut framer = Framer::default();
    loop {
        let deadline = quiet_until.min(sent + total);
        if Instant::now() >= deadline {
            return Err(stopped(received, TransportError::Timeout));
        }
        let data = transport
            .read(deadline)
            .map_err(|error| stopped(received, error))?;
        if !data.is_empty() {
            received = true;
            quiet_until = Instant::now() + silence;
        }
        if let Some(response) = framer.feed(&data)? {
            return Ok(response);
        }
    }
}

/// A transport failure, as an incomplete response once some of it has arrived.
fn stopped(received: bool, error: TransportError) -> HttpError {
    if received {
        HttpError::Incomplete(error)
    } else {
        HttpError::Transport(error)
    }
}

const HEAD_MAX: usize = 8 * 1024;
const HEADERS_MAX: usize = 64;
/// Chunk-size lines of this many bytes or more, not counting the CR, are rejected; longer
/// trailer lines are only stored up to it.
const LINE_MAX: usize = 64;
const BODY_MAX: usize = 128 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Head,
    Body {
        remaining: usize,
    },
    ChunkSize,
    ChunkData {
        remaining: usize,
    },
    ChunkEnd {
        seen_cr: bool,
    },
    Trailer,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Interim,
    Empty,
    Length(usize),
    Chunked,
}

/// One CRLF-terminated line; a lone CR or LF stays part of the line.
#[derive(Debug, Default)]
struct Line {
    bytes: Vec<u8>,
    overflow: bool,
    after_cr: bool,
}

impl Line {
    /// Adds one byte; true once the line's CRLF is complete.
    fn push(&mut self, byte: u8) -> bool {
        if byte == b'\n' && self.after_cr {
            if !self.overflow {
                self.bytes.pop();
            }
            return true;
        }
        self.after_cr = byte == b'\r';
        if self.bytes.len() < LINE_MAX {
            self.bytes.push(byte);
        } else {
            self.overflow = true;
        }
        false
    }

    /// Feeds bytes until the line completes; returns the bytes used and the finished line.
    fn feed(&mut self, data: &[u8]) -> (usize, Option<Line>) {
        for (index, &byte) in data.iter().enumerate() {
            if self.push(byte) {
                return (index + 1, Some(std::mem::take(self)));
            }
        }
        (data.len(), None)
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Response framing as a state machine fed with whatever bytes each read returns.
#[derive(Debug, Default)]
struct Framer {
    state: State,
    head: Vec<u8>,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    line: Line,
}

impl Framer {
    /// Consumes `data`; returns the response once it is complete. Bytes after the end of the
    /// response are ignored: the next exchange drains them anyway.
    fn feed(&mut self, mut data: &[u8]) -> Result<Option<Response>, HttpError> {
        while !data.is_empty() && self.state != State::Done {
            let used = self.step(data)?;
            data = &data[used..];
        }
        if self.state != State::Done {
            return Ok(None);
        }
        Ok(Some(Response {
            status: self.status,
            headers: std::mem::take(&mut self.headers),
            body: std::mem::take(&mut self.body),
        }))
    }

    /// Feeds `data`, never empty, to the current state; returns how many bytes it used.
    fn step(&mut self, data: &[u8]) -> Result<usize, HttpError> {
        match self.state {
            State::Head => self.feed_head(data),
            State::Body { remaining } => Ok(self.feed_body(data, remaining)),
            State::ChunkSize => self.feed_chunk_size(data),
            State::ChunkData { remaining } => Ok(self.feed_chunk_data(data, remaining)),
            State::ChunkEnd { seen_cr } => self.feed_chunk_end(data, seen_cr),
            State::Trailer => Ok(self.feed_trailer(data)),
            State::Done => Ok(0),
        }
    }

    fn feed_head(&mut self, data: &[u8]) -> Result<usize, HttpError> {
        let buffered = self.head.len();
        let take = data.len().min(HEAD_MAX - buffered);
        self.head.extend_from_slice(&data[..take]);
        let Some((len, head)) = parse_head(&self.head)? else {
            if self.head.len() == HEAD_MAX {
                return Err(HttpError::HeadTooLarge);
            }
            return Ok(take);
        };
        self.head.clear();
        self.state = match framing(&head)? {
            Framing::Interim => State::Head,
            Framing::Empty | Framing::Length(0) => State::Done,
            Framing::Length(remaining) => State::Body { remaining },
            Framing::Chunked => State::ChunkSize,
        };
        self.status = head.status;
        self.headers = head.headers;
        Ok(len - buffered)
    }

    /// Moves up to `remaining` body bytes from `data`; returns how many and how many are left.
    fn take_body(&mut self, data: &[u8], remaining: usize) -> (usize, usize) {
        let take = data.len().min(remaining);
        self.body.extend_from_slice(&data[..take]);
        (take, remaining - take)
    }

    fn feed_body(&mut self, data: &[u8], remaining: usize) -> usize {
        let (used, remaining) = self.take_body(data, remaining);
        self.state = if remaining == 0 {
            State::Done
        } else {
            State::Body { remaining }
        };
        used
    }

    fn feed_chunk_data(&mut self, data: &[u8], remaining: usize) -> usize {
        let (used, remaining) = self.take_body(data, remaining);
        self.state = if remaining == 0 {
            State::ChunkEnd { seen_cr: false }
        } else {
            State::ChunkData { remaining }
        };
        used
    }

    fn feed_chunk_size(&mut self, data: &[u8]) -> Result<usize, HttpError> {
        let (used, line) = self.line.feed(data);
        if let Some(line) = line {
            let size = chunk_size(&line)?;
            if size > BODY_MAX - self.body.len() {
                return Err(HttpError::BodyTooLarge);
            }
            self.state = if size == 0 {
                State::Trailer
            } else {
                State::ChunkData { remaining: size }
            };
        }
        Ok(used)
    }

    /// Only the CRLF closing a chunk's data may follow it; any other byte fails at once.
    fn feed_chunk_end(&mut self, data: &[u8], seen_cr: bool) -> Result<usize, HttpError> {
        self.state = match (seen_cr, data.first()) {
            (false, Some(b'\r')) => State::ChunkEnd { seen_cr: true },
            (true, Some(b'\n')) => State::ChunkSize,
            _ => return Err(HttpError::ChunkOverrun),
        };
        Ok(1)
    }

    fn feed_trailer(&mut self, data: &[u8]) -> usize {
        let (used, line) = self.line.feed(data);
        if line.is_some_and(|line| line.is_empty()) {
            self.state = State::Done;
        }
        used
    }
}

struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

/// Parses a complete head at the start of `buffer`; returns its length, or `None` while the
/// head is incomplete.
fn parse_head(buffer: &[u8]) -> Result<Option<(usize, Head)>, HttpError> {
    let mut fields = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut response = httparse::Response::new(&mut fields);
    let len = match response.parse(buffer) {
        Ok(httparse::Status::Complete(len)) => len,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(_) if head_has_nul(buffer) => return Err(HttpError::NulInHead),
        Err(error) => return Err(HttpError::BadHead(error.to_string())),
    };
    // httparse also ends a head at a lone LF followed by a line end; the body would then start
    // inside the header fields.
    if !buffer[..len].ends_with(b"\r\n\r\n") {
        return Err(HttpError::BadHead("head not ended by CRLF CRLF".to_owned()));
    }
    let headers = response
        .headers
        .iter()
        .map(|field| {
            let value = String::from_utf8_lossy(field.value).into_owned();
            (field.name.to_owned(), value)
        })
        .collect();
    let status = response.code.unwrap_or_default();
    if status < 100 {
        return Err(HttpError::BadHead(format!(
            "status code {status} below 100"
        )));
    }
    Ok(Some((len, Head { status, headers })))
}

/// httparse rejects a NUL like any other invalid byte; this names it. Only bytes before the end
/// of the head count: a body may contain NULs.
fn head_has_nul(buffer: &[u8]) -> bool {
    let end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(buffer.len());
    buffer[..end].contains(&0)
}

fn framing(head: &Head) -> Result<Framing, HttpError> {
    if head.status == 101 {
        return Err(HttpError::SwitchingProtocols);
    }
    let (content_length, chunked) = body_fields(&head.headers)?;
    // RFC 9112 6.3: Transfer-Encoding overrides Content-Length.
    match (head.status, chunked, content_length) {
        (_, Some(false), _) => Err(HttpError::TransferEncodingNotChunked),
        (100..200, _, _) => Ok(Framing::Interim),
        (204 | 304, _, _) => Ok(Framing::Empty),
        (_, Some(true), _) => Ok(Framing::Chunked),
        (_, None, Some(length)) => Ok(Framing::Length(length)),
        (_, None, None) => Err(HttpError::NoFraming),
    }
}

/// The `Content-Length` value, and whether the last `Transfer-Encoding` coding is `chunked`
/// (`None` without `Transfer-Encoding`).
fn body_fields(headers: &[(String, String)]) -> Result<(Option<usize>, Option<bool>), HttpError> {
    let mut content_length = None;
    let mut chunked = None;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("Content-Length") {
            let length = parse_content_length(value)?;
            if content_length.is_some_and(|earlier| earlier != length) {
                return Err(HttpError::ConflictingContentLength);
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("Transfer-Encoding") {
            chunked = Some(last_coding_is_chunked(value));
        }
    }
    Ok((content_length, chunked))
}

fn parse_content_length(value: &str) -> Result<usize, HttpError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HttpError::BadContentLength);
    }
    let length = parse_number(value.as_bytes(), 10).ok_or(HttpError::BodyTooLarge)?;
    if length > BODY_MAX {
        return Err(HttpError::BodyTooLarge);
    }
    Ok(length)
}

/// Codings are listed in the order applied, so only the last one decides the framing. A
/// substring match such as `x-notchunked` must not count. httparse already trimmed the value's
/// end, so only the blanks after the last comma remain.
fn last_coding_is_chunked(value: &str) -> bool {
    let last = value.rsplit(',').next().unwrap_or_default();
    trim_start_blanks(last.as_bytes()).eq_ignore_ascii_case(b"chunked")
}

/// A chunk-size line: `1*HEXDIG`, optional spaces or tabs, then the end or a `;` extension.
fn chunk_size(line: &Line) -> Result<usize, HttpError> {
    let digits = line
        .bytes
        .iter()
        .take_while(|byte| byte.is_ascii_hexdigit())
        .count();
    let rest = trim_start_blanks(&line.bytes[digits..]);
    let well_formed = digits > 0 && rest.first().is_none_or(|&byte| byte == b';');
    if line.overflow || !well_formed {
        return Err(HttpError::BadChunkSize);
    }
    parse_number(&line.bytes[..digits], 16).ok_or(HttpError::BodyTooLarge)
}

fn trim_start_blanks(bytes: &[u8]) -> &[u8] {
    let blanks = bytes
        .iter()
        .take_while(|&&byte| byte == b' ' || byte == b'\t')
        .count();
    &bytes[blanks..]
}

/// Digits already checked for `radix`; `None` on overflow.
fn parse_number(digits: &[u8], radix: u32) -> Option<usize> {
    digits.iter().try_fold(0_usize, |value, &digit| {
        let digit = char::from(digit).to_digit(radix)?;
        value
            .checked_mul(radix as usize)?
            .checked_add(digit as usize)
    })
}

#[cfg(test)]
mod tests;
