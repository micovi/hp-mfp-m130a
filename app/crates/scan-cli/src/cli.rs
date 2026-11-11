//! The command line: `m130a-scan status` and `m130a-scan scan --out FILE.pdf`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Parser, Subcommand};
use scan_core::{ColorMode, ScanSettings};

/// Scan multi-page documents from the HP M130a's flatbed into one PDF, over USB.
#[derive(Debug, Parser)]
#[command(name = "m130a-scan", version)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Print the scanner's model, state, resolutions and colour modes.
    Status,
    /// Scan pages from the flatbed into one PDF, asking after each page whether another follows.
    Scan(ScanArgs),
}

/// The options of `m130a-scan scan`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ScanArgs {
    /// The PDF to write.
    #[arg(long, value_name = "FILE.pdf")]
    pub out: PathBuf,
    /// Scan in grey instead of colour.
    #[arg(long)]
    pub grey: bool,
    /// Resolution in dots per inch.
    #[arg(long, default_value = "150", value_parser = dpi_parser())]
    pub dpi: u16,
    /// Replace FILE.pdf if it exists.
    #[arg(long)]
    pub force: bool,
}

impl ScanArgs {
    /// What each page is scanned with.
    pub fn settings(&self) -> ScanSettings {
        ScanSettings {
            color: if self.grey {
                ColorMode::Grey
            } else {
                ColorMode::Colour
            },
            dpi: self.dpi,
        }
    }
}

/// The resolutions the CLI offers, listed in `--help` and in the error for any other value.
fn dpi_parser() -> impl TypedValueParser<Value = u16> {
    PossibleValuesParser::new(["150", "300", "600"]).try_map(|dpi| dpi.parse::<u16>())
}

/// The exit code after clap's help, version or usage error: 0 for help and version, 1 for a
/// usage error (clap's own exit would use 2).
pub fn exit_code(error: &clap::Error) -> ExitCode {
    if error.use_stderr() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests;
