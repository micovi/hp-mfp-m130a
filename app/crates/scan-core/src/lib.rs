//! Everything that talks to the HP M130a's scanner: eSCL over the printer's IPP-over-USB
//! interface, and the scanned JPEG pages written into one PDF.

pub mod http;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
