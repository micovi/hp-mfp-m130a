//! Why a scan failed, worded for the person at the scanner.

use crate::escl::EsclError;
use crate::http::HttpError;
use crate::transport::TransportError;

/// Why talking to the scanner failed. Each message is one sentence: what happened, then what to
/// do about it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// No printer is attached by USB.
    #[error("the printer is not connected; connect it by USB and turn it on")]
    NotConnected,
    /// Another program holds every interface the scanner could be reached through.
    #[error("the printer's USB interface is in use by another app; quit that app and try again")]
    InterfaceBusy,
    /// The scanner did not become idle in time to take a job.
    #[error("the scanner is busy; try again in a moment")]
    ScannerBusy,
    /// The scanner answered the scan request with something other than 201 Created.
    #[error("{}", rejection_message(*status, detail))]
    JobRejected {
        /// The HTTP status code.
        status: u16,
        /// The first line of the answer's body, shortened; empty for none.
        detail: String,
    },
    /// The printer went silent, or the page did not arrive by its deadline.
    #[error(
        "the printer took too long to send {waiting_for}; \
         check that it is on and not showing an error, then try again"
    )]
    Timeout {
        /// What was awaited, such as "the page".
        waiting_for: String,
    },
    /// The printer went away mid-exchange.
    #[error("the printer was disconnected; reconnect its USB cable and try again")]
    Disconnected,
    /// The caller's cancel flag stopped the scan.
    #[error("the scan was cancelled; start a new scan when ready")]
    Cancelled,
    /// The job ended without a page, as it does when started while the scanner warms up.
    #[error("the scanner finished without sending a page; wait a moment, then scan again")]
    NoPage,
    /// The scan settings ask for something the scanner does not offer.
    #[error(transparent)]
    Unsupported(EsclError),
    /// The printer's answer broke HTTP or eSCL, or the USB transfer failed; the cause, described.
    #[error(
        "communication with the printer failed ({0}); turn the printer off and on, then try again"
    )]
    Protocol(String),
}

/// The longest rejection detail shown, in characters.
const DETAIL_MAX: usize = 100;

fn rejection_message(status: u16, detail: &str) -> String {
    let answer = if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    };
    format!("the scanner refused the scan ({answer}); check the printer's display, then try again")
}

impl ScanError {
    /// The error for an HTTP exchange that failed while waiting for `waiting_for`, such as
    /// "the page".
    pub(crate) fn from_http(error: HttpError, waiting_for: &str) -> Self {
        match error {
            HttpError::Transport(TransportError::Disconnected)
            | HttpError::Incomplete(TransportError::Disconnected) => Self::Disconnected,
            HttpError::Transport(TransportError::Timeout)
            | HttpError::Incomplete(TransportError::Timeout) => Self::Timeout {
                waiting_for: waiting_for.to_owned(),
            },
            other => Self::Protocol(other.to_string()),
        }
    }

    /// A [`ScanError::JobRejected`] for a scan request answered with `status` and `body`.
    pub(crate) fn rejected(status: u16, body: &[u8]) -> Self {
        let text = String::from_utf8_lossy(body);
        let first_line = text.lines().next().unwrap_or_default().trim();
        let mut detail: String = first_line.chars().take(DETAIL_MAX).collect();
        if first_line.chars().nth(DETAIL_MAX).is_some() {
            detail.push('…');
        }
        Self::JobRejected { status, detail }
    }
}

#[cfg(test)]
mod tests;
