//! The byte pipe to the printer: USB in production, a scripted fake in tests.

use std::time::Instant;

/// A bidirectional byte stream to the printer's IPP-over-USB interface.
pub trait Transport {
    /// Writes one whole request (the implementation adds a zero-length packet when needed).
    ///
    /// # Errors
    ///
    /// [`TransportError`] when the bytes could not be sent.
    fn write(&mut self, data: &[u8]) -> Result<(), TransportError>;

    /// Returns the next non-empty bytes, waiting until `deadline` at most.
    ///
    /// # Errors
    ///
    /// [`TransportError::Timeout`] when nothing arrived by `deadline`, or another
    /// [`TransportError`] when the device failed.
    fn read(&mut self, deadline: Instant) -> Result<Vec<u8>, TransportError>;

    /// Discards bytes left from an earlier exchange, up to the first empty read; returns the count.
    ///
    /// # Errors
    ///
    /// [`TransportError`] when the device failed while draining.
    fn drain(&mut self) -> Result<usize, TransportError>;
}

/// Why a [`Transport`] operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Nothing arrived before the deadline.
    #[error("the printer did not answer in time")]
    Timeout,
    /// The device went away.
    #[error("the printer was disconnected")]
    Disconnected,
    /// Any other I/O failure, described.
    #[error("USB I/O failed: {0}")]
    Io(String),
}
