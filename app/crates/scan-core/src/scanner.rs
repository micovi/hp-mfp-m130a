//! One scan, as the M130a runs it: wait for Idle, create the job, poll for its page, delete it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ScanError;
use crate::escl::{
    Capabilities, ColorMode, ScanSettings, ScannerState, ScannerStatus, job_path,
    parse_capabilities, parse_status, scan_settings_xml,
};
use crate::http::{Request, Response, exchange};
use crate::pdf::Page;
use crate::transport::Transport;

const CAPABILITIES_PATH: &str = "/eSCL/ScannerCapabilities";
const STATUS_PATH: &str = "/eSCL/ScannerStatus";
const JOBS_PATH: &str = "/eSCL/ScanJobs";

/// The pixel count of a 150 dpi page, per square inch: the work [`Timing::page`] is sized for.
const BASE_WORK: u32 = 150 * 150;
/// No page deadline is longer than this many times [`Timing::page`].
const MAX_DEADLINE_FACTOR: u32 = 4;

/// How long to wait for the scanner, and how often to ask it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The pause between two status polls, and between two page polls.
    pub poll: Duration,
    /// The longest the printer may stay silent within one HTTP exchange.
    pub request: Duration,
    /// How long to wait for the scanner to become idle before calling it busy.
    pub idle_wait: Duration,
    /// How long a 150 dpi grey page may take; [`Timing::page_deadline`] scales it.
    pub page: Duration,
}

impl Default for Timing {
    /// Poll every second, allow 10 s of silence, wait 30 s for Idle and 60 s for a 150 dpi grey
    /// page.
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(1),
            request: Duration::from_secs(10),
            idle_wait: Duration::from_secs(30),
            page: Duration::from_secs(60),
        }
    }
}

impl Timing {
    /// How long the scanner may answer "not yet" before the page counts as lost.
    ///
    /// [`Timing::page`] at 150 dpi grey, growing with the pixel count and three times for
    /// colour; never below [`Timing::page`] nor above four times it (240 s by default, which
    /// 300 dpi grey already reaches).
    #[must_use]
    pub fn page_deadline(&self, settings: &ScanSettings) -> Duration {
        let samples = match settings.color {
            ColorMode::Grey => 1,
            ColorMode::Colour => 3,
        };
        let dpi = u32::from(settings.dpi);
        let work = dpi
            .saturating_mul(dpi)
            .saturating_mul(samples)
            .clamp(BASE_WORK, BASE_WORK * MAX_DEADLINE_FACTOR);
        self.page.saturating_mul(work) / BASE_WORK
    }
}

/// The M130a's scanner, reached through `T`.
#[derive(Debug)]
pub struct Scanner<T: Transport> {
    transport: T,
    capabilities: Capabilities,
    timing: Timing,
}

impl<T: Transport> Scanner<T> {
    /// A scanner on `transport`, after reading what it can do.
    ///
    /// # Errors
    ///
    /// [`ScanError::Timeout`], [`ScanError::Disconnected`] or [`ScanError::Protocol`] when the
    /// capabilities cannot be read.
    pub fn with_transport(mut transport: T, timing: Timing) -> Result<Self, ScanError> {
        let xml = get(
            &mut transport,
            CAPABILITIES_PATH,
            timing.request,
            "the scanner's capabilities",
        )?;
        let capabilities = parse_capabilities(&xml).map_err(|error| protocol(&error))?;
        Ok(Self {
            transport,
            capabilities,
            timing,
        })
    }

    /// What the flatbed offers, as read when the scanner was opened.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The scanner's state now.
    ///
    /// # Errors
    ///
    /// [`ScanError::Timeout`], [`ScanError::Disconnected`] or [`ScanError::Protocol`] when the
    /// status cannot be read.
    pub fn status(&mut self) -> Result<ScannerStatus, ScanError> {
        let xml = get(
            &mut self.transport,
            STATUS_PATH,
            self.timing.request,
            "the scanner's status",
        )?;
        parse_status(&xml).map_err(|error| protocol(&error))
    }

    /// Scans one page from the flatbed.
    ///
    /// Waits for the scanner to be idle, creates a job, polls it until the page arrives, and
    /// deletes the job whatever happened once it exists, ignoring the answer. `cancel` is checked
    /// before every request; once set, the job (if any) is deleted and the scan stops.
    ///
    /// # Errors
    ///
    /// [`ScanError::Unsupported`] for settings the scanner does not offer (nothing is sent);
    /// [`ScanError::ScannerBusy`] when it is not idle within [`Timing::idle_wait`];
    /// [`ScanError::JobRejected`] when it refuses the job; [`ScanError::Timeout`] when the page
    /// is not there by [`Timing::page_deadline`] or the printer goes silent;
    /// [`ScanError::NoPage`] when the job ends without one; [`ScanError::Cancelled`];
    /// [`ScanError::Disconnected`]; [`ScanError::Protocol`] for answers that make no sense.
    pub fn scan_page(
        &mut self,
        settings: &ScanSettings,
        cancel: &AtomicBool,
    ) -> Result<Page, ScanError> {
        let xml =
            scan_settings_xml(settings, &self.capabilities).map_err(ScanError::Unsupported)?;
        self.wait_for_idle(cancel)?;
        let job = self.create_job(&xml)?;
        let page = self.fetch_page(&job, *settings, cancel);
        self.delete_job(&job);
        page
    }

    /// The transport, for tests to check their script.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn wait_for_idle(&mut self, cancel: &AtomicBool) -> Result<(), ScanError> {
        let deadline = Instant::now() + self.timing.idle_wait;
        loop {
            check(cancel)?;
            if self.status()?.state == ScannerState::Idle {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ScanError::ScannerBusy);
            }
            thread::sleep(self.timing.poll);
        }
    }

    /// Posts the scan settings; returns the new job's path.
    fn create_job(&mut self, xml: &str) -> Result<String, ScanError> {
        let request = Request {
            method: "POST",
            path: JOBS_PATH,
            content_type: Some("text/xml"),
            body: xml.as_bytes(),
        };
        let response = self.send(&request, "the scan job")?;
        if response.status != 201 {
            return Err(ScanError::rejected(response.status, &response.body));
        }
        let location = response
            .header("Location")
            .ok_or_else(|| ScanError::Protocol("the scan job has no Location".to_owned()))?;
        job_path(location).map_err(|error| protocol(&error))
    }

    fn fetch_page(
        &mut self,
        job: &str,
        settings: ScanSettings,
        cancel: &AtomicBool,
    ) -> Result<Page, ScanError> {
        let path = format!("{job}/NextDocument");
        let deadline = Instant::now() + self.timing.page_deadline(&settings);
        loop {
            check(cancel)?;
            let response = self.send(&get_request(&path), "the page")?;
            match response.status {
                200 => {
                    return Page::new(response.body, settings.dpi)
                        .map_err(|error| protocol(&error));
                }
                503 => {}
                404 => return Err(ScanError::NoPage),
                other => return Err(ScanError::Protocol(format!("HTTP {other} for the page"))),
            }
            if Instant::now() >= deadline {
                return Err(ScanError::Timeout {
                    waiting_for: "the page".to_owned(),
                });
            }
            thread::sleep(self.timing.poll);
        }
    }

    /// Deletes the job and ignores the answer: after its page the M130a answers 503 a few times
    /// and then 404, and the related M28w keeps answering 503, so the answer says nothing useful.
    fn delete_job(&mut self, job: &str) {
        let request = Request {
            method: "DELETE",
            ..get_request(job)
        };
        let _ignored = exchange(&mut self.transport, &request, self.timing.request);
    }

    fn send(&mut self, request: &Request, waiting_for: &str) -> Result<Response, ScanError> {
        exchange(&mut self.transport, request, self.timing.request)
            .map_err(|error| ScanError::from_http(error, waiting_for))
    }
}

fn get_request(path: &str) -> Request<'_> {
    Request {
        method: "GET",
        path,
        content_type: None,
        body: b"",
    }
}

/// GETs an eSCL document that must come back 200 OK; returns its body.
fn get<T: Transport>(
    transport: &mut T,
    path: &str,
    silence: Duration,
    waiting_for: &str,
) -> Result<Vec<u8>, ScanError> {
    let response = exchange(transport, &get_request(path), silence)
        .map_err(|error| ScanError::from_http(error, waiting_for))?;
    if response.status != 200 {
        return Err(ScanError::Protocol(format!(
            "HTTP {} for {waiting_for}",
            response.status
        )));
    }
    Ok(response.body)
}

fn check(cancel: &AtomicBool) -> Result<(), ScanError> {
    if cancel.load(Ordering::SeqCst) {
        Err(ScanError::Cancelled)
    } else {
        Ok(())
    }
}

fn protocol(error: &impl ToString) -> ScanError {
    ScanError::Protocol(error.to_string())
}

#[cfg(test)]
mod tests;
