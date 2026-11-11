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
        let devices = nusb::list_devices()
            .wait()
            .map_err(|error| ScanError::Protocol(format!("listing USB devices failed: {error}")))?;
        let mut failures = Vec::new();
        for info in devices.filter(is_ipp_printer) {
            match claim_any(&info) {
                Ok(transport) => return Ok(transport),
                Err(device_failures) => failures.extend(device_failures),
            }
        }
        Err(open_error(&failures))
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
        drain(&mut self.bulk_in)
    }
}

impl Scanner<UsbTransport> {
    /// The scanner of the printer attached by USB, with the default [`Timing`].
    ///
    /// # Errors
    ///
    /// The errors of [`UsbTransport::open`] and [`Scanner::with_transport`].
    pub fn open() -> Result<Self, ScanError> {
        Self::with_transport(UsbTransport::open()?, Timing::default())
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

/// Claims the first IPP interface of this device that is free; the failures otherwise.
fn claim_any(info: &DeviceInfo) -> Result<UsbTransport, Vec<OpenFailure>> {
    let device = info.open().wait().map_err(|error| {
        vec![OpenFailure::new(
            error.kind(),
            "opening the printer",
            &error,
        )]
    })?;
    let config = device.active_configuration().map_err(|error| {
        vec![OpenFailure::new(
            ErrorKind::Other,
            "reading the printer's configuration",
            &error,
        )]
    })?;
    let mut failures = Vec::new();
    for interface in ipp_interfaces(&config) {
        match claim(&device, interface) {
            Ok(transport) => return Ok(transport),
            Err(failure) => failures.push(failure),
        }
    }
    Err(failures)
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

/// Discards data until a read comes back empty or times out; returns how many bytes.
fn drain(input: &mut impl BulkIn) -> Result<usize, TransportError> {
    let mut discarded = 0;
    loop {
        let received = input.receive(DRAIN_READ_TIMEOUT);
        discarded += received.data.len();
        match received.status {
            Ok(()) if !received.data.is_empty() => {}
            Ok(()) | Err(TransferError::Cancelled) => return Ok(discarded),
            Err(error) => return Err(transfer_error(error, "draining the printer's answers")),
        }
    }
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
