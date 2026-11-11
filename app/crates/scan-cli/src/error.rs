//! Why a command failed, and how the CLI tells the user.

use std::io;
use std::path::PathBuf;

use scan_core::{PdfError, ScanError};

/// Why a command failed. Messages follow scan-core's: lowercase, without a final period;
/// [`CliError::report`] turns them into what the user reads.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// `--out` names an existing file and `--force` was not given.
    #[error("{} already exists; pass --force to replace it, or choose another --out", .0.display())]
    Exists(PathBuf),
    /// The scanner could not be opened or its status read.
    #[error(transparent)]
    Scanner(ScanError),
    /// A page failed (or was cancelled), so nothing was written.
    #[error("{error}")]
    Scan {
        /// Why the page failed.
        error: ScanError,
        /// The pages scanned before it, now discarded.
        scanned: usize,
    },
    /// The scanned pages could not be made into a PDF.
    #[error(transparent)]
    Pdf(PdfError),
    /// The PDF could not be written.
    #[error(
        "could not write {} ({source}); check that its folder exists and is writable, \
         then scan again",
        path.display()
    )]
    Save {
        /// The `--out` path.
        path: PathBuf,
        /// The OS error.
        source: io::Error,
    },
    /// Ctrl-C could not be watched for, so a scan could not be cancelled.
    #[error("could not set up Ctrl-C handling ({0})")]
    CtrlC(io::Error),
    /// Reading the answer to a prompt, or writing to the terminal, failed.
    #[error("the terminal failed ({0})")]
    Terminal(#[from] io::Error),
}

impl CliError {
    /// What the user reads on stderr: the error as a sentence, and, when pages were being
    /// scanned, that no PDF was written. A cancelled scan is not reported as an error.
    pub fn report(&self) -> String {
        match self {
            Self::Scan {
                error: ScanError::Cancelled,
                scanned,
            } => format!("Scan cancelled. {}", nothing_written(*scanned)),
            Self::Scan { error, scanned } => {
                format!("Error: {} {}", sentence(error), nothing_written(*scanned))
            }
            other => format!("Error: {}", sentence(other)),
        }
    }
}

/// A scan-core style message as a sentence: first letter capitalised, final period added.
pub fn sentence(message: &impl ToString) -> String {
    let message = message.to_string();
    let mut chars = message.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}.", first.to_uppercase(), chars.as_str())
}

fn nothing_written(scanned: usize) -> String {
    match scanned {
        0 => "No PDF was written.".to_owned(),
        1 => "No PDF was written; the page already scanned was discarded.".to_owned(),
        _ => format!("No PDF was written; the {scanned} pages already scanned were discarded."),
    }
}

#[cfg(test)]
mod tests;
