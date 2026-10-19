//! Binary entry point. Forwards to the library crate so unit and
//! integration tests can exercise the same code path.

use std::process::ExitCode;

use clap::Parser;

use aim::cli;
use aim::output;

fn main() -> ExitCode {
    output::init();

    let args = cli::Cli::parse();
    if args.quiet {
        // Future slices will downgrade the log level here.
        tracing::debug!("quiet mode requested");
    }

    match cli::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let report = miette::Report::new(err);
            eprintln!("{report:?}");
            ExitCode::FAILURE
        }
    }
}
