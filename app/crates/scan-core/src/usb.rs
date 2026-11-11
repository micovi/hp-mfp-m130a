//! The production [`Transport`]: HTTP bytes over the bulk endpoint pair of the printer's
//! IPP-over-USB interface, through nusb.
//!
//! HP's IPP interfaces answer every bulk IN read at once with a zero-length packet while they
//! have nothing to send (about 11k a second), so an empty completion means "no data yet", never
//! "the device is quiet": silence is measured by the clock, against the caller's deadline.

use std::thread;
use std::time::{Duration, Instant};

use nusb::descriptors::{ConfigurationDescriptor, InterfaceDescriptor, TransferType};
use nusb::transfer::{Buffer, Bulk, Direction, In, Out, TransferError};
use nusb::{Device, DeviceInfo, Endpoint, ErrorKind, MaybeFuture};

use crate::error::ScanError;
use crate::scanner::{Scanner, Timing};
use crate::transport::{Transport, TransportError};

/// HP's USB vendor ID.
const HP_VENDOR_ID: u16 = 0x03F0;
/// Class, subclass and protocol of HP's vendor-specific IPP-over-USB interfaces.
const IPP_OVER_USB: (u8, u8, u8) = (0xFF, 0x09, 0x01);
/// Packets per bulk IN transfer: 64 KiB at high speed, 8 KiB at full speed.
const IN_PACKETS_PER_TRANSFER: usize = 128;
/// The pause after a zero-length completion, so polling an idle endpoint costs little CPU.
const EMPTY_READ_PAUSE: Duration = Duration::from_millis(1);
/// How long `drain` waits for each read before deciding nothing is left.
const DRAIN_READ_TIMEOUT: Duration = Duration::from_millis(200);
/// How long one bulk OUT transfer may take before the write counts as failed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The printer's IPP-over-USB interface, claimed: requests go out on one bulk endpoint and
/// answers come back on the other.
#[derive(Debug)]
pub struct UsbTransport {
    bulk_out: Endpoint<Bulk, Out>,
    bulk_in: Endpoint<Bulk, In>,
    /// Whether the next drain must wait for quiet: true after an abandoned exchange, and on a
    /// newly claimed interface, where an earlier program may have left an answer half-read.
    dirty: bool,
}

impl UsbTransport {
    /// Claims the first free IPP-over-USB interface of an attached HP printer.
    ///
    /// Every HP device with an `ff/09/01` interface is a candidate; its interfaces of that kind
    /// (3 and 4 on the M130a) are tried in order until one can be claimed.
    ///
    /// # Errors
    ///
    /// [`ScanError::NotConnected`] when no such printer is attached (or it went away while
    /// opening); [`ScanError::InterfaceBusy`] when another program holds the interfaces;
    /// [`ScanError::Protocol`] describing every failure otherwise.
    pub fn open() -> Result<Self, ScanError> {
        open_first(Ok)
    }
}

impl Transport for UsbTransport {
    fn write(&mut self, data: &[u8]) -> Result<(), TransportError> {
        write_message(&mut self.bulk_out, data)
    }

    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>, TransportError> {
        read_next(&mut self.bulk_in, deadline)
    }

    fn drain(&mut self) -> Result<usize, TransportError> {
        let discarded = drain(&mut self.bulk_in, self.dirty, &DRAIN_LIMITS)?;
        self.dirty = false;
        Ok(discarded)
    }

    fn abandon(&mut self) {
        self.dirty = true;
    }
}

impl Scanner<UsbTransport> {
    /// The scanner of the printer attached by USB, with the default [`Timing`].
    ///
    /// Each IPP-over-USB interface (3, then 4 on the M130a) is claimed in turn, drained of
    /// anything an earlier program left, and asked for the capabilities; the first that answers
    /// is used, and the others are released. One interface can be stuck while the other works.
    ///
    /// # Errors
    ///
    /// The errors of [`UsbTransport::open`] when no interface can be claimed; otherwise the
    /// error of [`Scanner::with_transport`] on the last interface tried.
    pub fn open() -> Result<Self, ScanError> {
        open_first(|transport| Self::with_transport(transport, Timing::default()))
    }
}

/// Whether an HP printer with an IPP-over-USB interface is attached; claims nothing.
#[must_use]
pub fn printer_connected() -> bool {
    nusb::list_devices()
        .wait()
        .is_ok_and(|mut devices| devices.any(|info| is_ipp_printer(&info)))
}

fn is_ipp_printer(info: &DeviceInfo) -> bool {
    info.vendor_id() == HP_VENDOR_ID
        && info
            .interfaces()
            .any(|interface| class_triple(interface) == IPP_OVER_USB)
}

fn class_triple(interface: &nusb::InterfaceInfo) -> (u8, u8, u8) {
    (
        interface.class(),
        interface.subclass(),
        interface.protocol(),
    )
}

/// An IPP-over-USB interface and its bulk endpoint pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IppInterface {
    number: u8,
    bulk_out: u8,
    bulk_in: u8,
}

/// The `ff/09/01` interfaces (alternate setting 0) that have a bulk OUT and a bulk IN endpoint,
/// by interface number.
fn ipp_interfaces(config: &ConfigurationDescriptor<'_>) -> Vec<IppInterface> {
    let mut found: Vec<IppInterface> = config
        .interface_alt_settings()
        .filter(|setting| {
            setting.alternate_setting() == 0
                && (setting.class(), setting.subclass(), setting.protocol()) == IPP_OVER_USB
        })
        .filter_map(|setting| {
            Some(IppInterface {
                number: setting.interface_number(),
                bulk_out: bulk_endpoint(&setting, Direction::Out)?,
                bulk_in: bulk_endpoint(&setting, Direction::In)?,
            })
        })
        .collect();
    found.sort_by_key(|interface| interface.number);
    found
}

fn bulk_endpoint(setting: &InterfaceDescriptor<'_>, direction: Direction) -> Option<u8> {
    setting
        .endpoints()
        .find(|endpoint| {
            endpoint.transfer_type() == TransferType::Bulk && endpoint.direction() == direction
        })
        .map(|endpoint| endpoint.address())
}

/// Why opening or claiming failed, for choosing the [`ScanError`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenFailure {
    kind: ErrorKind,
    detail: String,
}

impl OpenFailure {
    fn new(kind: ErrorKind, what: &str, error: &impl std::fmt::Display) -> Self {
        Self {
            kind,
            detail: format!("{what}: {error}"),
        }
    }
}

/// Claims the IPP interfaces of attached HP printers in order and returns what `start` makes of
/// the first one it accepts.
fn open_first<S>(start: impl FnMut(UsbTransport) -> Result<S, ScanError>) -> Result<S, ScanError> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|error| ScanError::Protocol(format!("listing USB devices failed: {error}")))?;
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for info in devices.filter(is_ipp_printer) {
        match open_device(&info) {
            Ok((device, interfaces)) => candidates.extend(
                interfaces
                    .into_iter()
                    .map(|interface| (device.clone(), interface)),
            ),
            Err(failure) => failures.push(failure),
        }
    }
    first_started(
        candidates,
        failures,
        |(device, interface)| claim(&device, interface),
        start,
    )
}

/// Opens the device and lists its IPP interfaces.
fn open_device(info: &DeviceInfo) -> Result<(Device, Vec<IppInterface>), OpenFailure> {
    let device = info
        .open()
        .wait()
        .map_err(|error| OpenFailure::new(error.kind(), "opening the printer", &error))?;
    let interfaces = {
        let config = device.active_configuration().map_err(|error| {
            OpenFailure::new(
                ErrorKind::Other,
                "reading the printer's configuration",
                &error,
            )
        })?;
        ipp_interfaces(&config)
    };
    Ok((device, interfaces))
}

/// Claims each candidate in turn and starts on it; returns the first start that succeeds.
/// A transport that fails to start is dropped, releasing its interface, before the next claim.
///
/// `failures` are those met before, such as devices that would not open. When nothing could be
/// claimed the error comes from all the failures; otherwise it is the last start's error.
fn first_started<C, T, S>(
    candidates: Vec<C>,
    mut failures: Vec<OpenFailure>,
    mut claim: impl FnMut(C) -> Result<T, OpenFailure>,
    mut start: impl FnMut(T) -> Result<S, ScanError>,
) -> Result<S, ScanError> {
    let mut last_error = None;
    for candidate in candidates {
        match claim(candidate) {
            Ok(transport) => match start(transport) {
                Ok(started) => return Ok(started),
                Err(error) => last_error = Some(error),
            },
            Err(failure) => failures.push(failure),
        }
    }
    Err(last_error.unwrap_or_else(|| open_error(&failures)))
}

fn claim(device: &Device, ipp: IppInterface) -> Result<UsbTransport, OpenFailure> {
    let number = ipp.number;
    let interface = device
        .claim_interface(number)
        .wait()
        .map_err(|error| OpenFailure::new(error.kind(), &format!("claiming IF{number}"), &error))?;
    let open_failed = |error: nusb::Error| {
        OpenFailure::new(
            error.kind(),
            &format!("opening the endpoints of IF{number}"),
            &error,
        )
    };
    Ok(UsbTransport {
        bulk_out: interface.endpoint(ipp.bulk_out).map_err(open_failed)?,
        bulk_in: interface.endpoint(ipp.bulk_in).map_err(open_failed)?,
        dirty: true,
    })
}

/// The error for an open that claimed nothing: busy wins, because quitting the other program
/// fixes it; a device that vanished is not connected; anything else is described.
fn open_error(failures: &[OpenFailure]) -> ScanError {
    if failures
        .iter()
        .any(|failure| failure.kind == ErrorKind::Busy)
    {
        return ScanError::InterfaceBusy;
    }
    if failures
        .iter()
        .all(|failure| failure.kind == ErrorKind::Disconnected)
    {
        return ScanError::NotConnected;
    }
    let details: Vec<&str> = failures
        .iter()
        .map(|failure| failure.detail.as_str())
        .collect();
    ScanError::Protocol(details.join("; "))
}

/// What one bulk IN transfer brought back; it can carry data even when it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Received {
    data: Vec<u8>,
    status: Result<(), TransferError>,
}

/// A bulk IN endpoint, one transfer at a time.
trait BulkIn {
    /// Runs one transfer, cancelling it after `timeout` (status [`TransferError::Cancelled`]).
    fn receive(&mut self, timeout: Duration) -> Received;
}

/// A bulk OUT endpoint, one transfer at a time.
trait BulkOut {
    fn max_packet_size(&self) -> usize;

    /// Sends `data` (a zero-length packet when empty), failing after `timeout`.
    fn send(&mut self, data: &[u8], timeout: Duration) -> Result<(), TransferError>;
}

impl BulkIn for Endpoint<Bulk, In> {
    fn receive(&mut self, timeout: Duration) -> Received {
        let size = Endpoint::max_packet_size(self) * IN_PACKETS_PER_TRANSFER;
        let completion = self.transfer_blocking(Buffer::new(size), timeout);
        Received {
            data: completion.buffer.into_vec(),
            status: completion.status,
        }
    }
}

impl BulkOut for Endpoint<Bulk, Out> {
    fn max_packet_size(&self) -> usize {
        Endpoint::max_packet_size(self)
    }

    fn send(&mut self, data: &[u8], timeout: Duration) -> Result<(), TransferError> {
        self.transfer_blocking(Buffer::from(data), timeout).status
    }
}

/// Returns the first non-empty data before `deadline`, resubmitting after empty completions.
fn read_next(input: &mut impl BulkIn, deadline: Instant) -> Result<Vec<u8>, TransportError> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(TransportError::Timeout);
        }
        let received = input.receive(left);
        if !received.data.is_empty() {
            return Ok(received.data);
        }
        match received.status {
            Ok(()) => thread::sleep(EMPTY_READ_PAUSE),
            // The transfer timed out: the deadline check above ends the loop.
            Err(TransferError::Cancelled) => {}
            Err(error) => return Err(transfer_error(error, "reading from the printer")),
        }
    }
}

/// When a drain stops, and when it gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DrainLimits {
    /// How long the endpoint must send nothing before the drain ends.
    quiet: Duration,
    /// The longest a drain may take.
    longest: Duration,
    /// The most bytes a drain discards.
    most_bytes: usize,
}

/// Leftovers of an abandoned page arrive in bursts with pauses shorter than `quiet`; a page is
/// at most a few MiB and arrives within seconds.
const DRAIN_LIMITS: DrainLimits = DrainLimits {
    quiet: Duration::from_millis(500),
    longest: Duration::from_secs(15),
    most_bytes: 256 * 1024 * 1024,
};

/// Discards stale data; returns how many bytes. A clean endpoint costs one read, unless `dirty`
/// (an exchange was abandoned): then, or once anything stale turns up, it reads until the
/// endpoint has been quiet for [`DrainLimits::quiet`].
fn drain(
    input: &mut impl BulkIn,
    dirty: bool,
    limits: &DrainLimits,
) -> Result<usize, TransportError> {
    if dirty {
        return drain_until_quiet(input, 0, limits);
    }
    let received = input.receive(DRAIN_READ_TIMEOUT);
    if let Err(error) = received.status
        && error != TransferError::Cancelled
    {
        return Err(drain_error(error));
    }
    if received.data.is_empty() {
        return Ok(0);
    }
    drain_until_quiet(input, received.data.len(), limits)
}

/// Discards data until none has come for [`DrainLimits::quiet`]; returns `discarded` plus the
/// bytes discarded here.
fn drain_until_quiet(
    input: &mut impl BulkIn,
    mut discarded: usize,
    limits: &DrainLimits,
) -> Result<usize, TransportError> {
    let started = Instant::now();
    let mut last_data = started;
    loop {
        let quiet_left = limits.quiet.saturating_sub(last_data.elapsed());
        if quiet_left.is_zero() {
            return Ok(discarded);
        }
        if started.elapsed() >= limits.longest || discarded >= limits.most_bytes {
            return Err(TransportError::Io(
                "the printer keeps sending data left from an earlier request".to_owned(),
            ));
        }
        let received = input.receive(quiet_left);
        if !received.data.is_empty() {
            discarded += received.data.len();
            last_data = Instant::now();
        }
        match received.status {
            Ok(()) if received.data.is_empty() => thread::sleep(EMPTY_READ_PAUSE),
            Ok(()) | Err(TransferError::Cancelled) => {}
            Err(error) => return Err(drain_error(error)),
        }
    }
}

fn drain_error(error: TransferError) -> TransportError {
    transfer_error(error, "draining the printer's old answers")
}

/// Sends one request, ended by a zero-length packet when its last packet is full, so the
/// printer sees where it ends. An empty request sends nothing.
fn write_message(output: &mut impl BulkOut, data: &[u8]) -> Result<(), TransportError> {
    if data.is_empty() {
        return Ok(());
    }
    let sent = |result: Result<(), TransferError>| {
        result.map_err(|error| transfer_error(error, "writing to the printer"))
    };
    sent(output.send(data, WRITE_TIMEOUT))?;
    if needs_zero_length_packet(data.len(), output.max_packet_size()) {
        sent(output.send(&[], WRITE_TIMEOUT))?;
    }
    Ok(())
}

/// Whether a transfer of `len` bytes ends in a full packet, which does not mark its end.
fn needs_zero_length_packet(len: usize, max_packet_size: usize) -> bool {
    max_packet_size != 0 && len.is_multiple_of(max_packet_size)
}

fn transfer_error(error: TransferError, what: &str) -> TransportError {
    match error {
        TransferError::Cancelled => TransportError::Timeout,
        TransferError::Disconnected => TransportError::Disconnected,
        TransferError::Stall
        | TransferError::Fault
        | TransferError::InvalidArgument
        | TransferError::Unknown(_) => TransportError::Io(format!("{what} failed: {error}")),
    }
}

#[cfg(test)]
mod tests;
