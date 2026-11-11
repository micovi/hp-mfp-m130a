//! `m130a-scan`: scans pages from the HP M130a into one PDF.

mod cli;
mod commands;
mod error;

use std::io::{self, Write};
use std::process::{self, ExitCode};
use std::sync::Arc;
use std::thread;

use clap::Parser;
use scan_core::Scanner;
use signal_hook::consts::SIGINT;
use signal_hook::iterator::Signals;

use crate::cli::Cli;
use crate::commands::{Console, Interrupt, run};
use crate::error::CliError;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(usage) => {
            let _unreported = usage.print();
            return cli::exit_code(&usage);
        }
    };
    let mut console = Console {
        input: io::stdin().lock(),
        out: io::stdout(),
        err: io::stderr(),
    };
    let outcome = watch_ctrl_c()
        .and_then(|interrupt| run(&cli.command, Scanner::open, &interrupt, &mut console));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _unreported = writeln!(console.err, "{}", error.report());
            ExitCode::FAILURE
        }
    }
}

fn watch_ctrl_c() -> Result<Arc<Interrupt>, CliError> {
    let interrupt = Arc::new(Interrupt::default());
    let mut signals = Signals::new([SIGINT]).map_err(CliError::CtrlC)?;
    let handler = Arc::clone(&interrupt);
    thread::spawn(move || {
        for _ in signals.forever() {
            on_ctrl_c(&handler);
        }
    });
    Ok(interrupt)
}

#[expect(
    clippy::exit,
    reason = "Ctrl-C between scans ends the program, as it would without a handler; \
              main is blocked reading the answer to a prompt, so this thread has to"
)]
fn on_ctrl_c(interrupt: &Interrupt) {
    if !interrupt.press() {
        let _unreported = writeln!(io::stderr(), "\nCancelled; nothing was saved.");
        process::exit(1);
    }
}
