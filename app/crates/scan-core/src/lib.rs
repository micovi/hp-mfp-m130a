//! Everything that talks to the HP M130a's scanner: eSCL over the printer's IPP-over-USB
//! interface, and the scanned JPEG pages written into one PDF.

pub mod error;
pub mod escl;
pub mod http;
pub mod jpeg;
pub mod pdf;
pub mod scanner;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod usb;

pub use error::ScanError;
pub use escl::{
    Capabilities, ColorMode, EsclError, JobInfo, RESOLUTIONS, ScanSettings, ScannerState,
    ScannerStatus,
};
pub use jpeg::{JpegError, JpegInfo, jpeg_info};
pub use pdf::{Page, PageError, PdfError, write_pdf};
pub use scanner::{Scanner, Timing};
pub use usb::{UsbTransport, printer_connected};
