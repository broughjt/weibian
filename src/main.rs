mod build;
mod config;
mod file_store;
mod import_graph;
mod world;

use std::process::ExitCode;

use clap::Parser;
use config::{Arguments, Command};
use ecow::eco_format;
use termcolor::{ColorChoice, StandardStream};
use typst::diag::StrResult;

use crate::build::BuildState;

fn main() -> ExitCode {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error) => {
            error.print().expect("Failed to print clap error");
            return ExitCode::from(error.exit_code() as u8);
        }
    };

    match dispatch(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(arguments: Arguments) -> StrResult<()> {
    let Arguments {
        config_file,
        command,
    } = arguments;
    let config = config::BuildConfig::try_load(config_file)?;

    match command {
        Command::Build => {
            let build_state = BuildState::new(config);
            let diagnostics = build_state.build().map_err(|e| eco_format!("{e}"))?;
            let stderr = StandardStream::stderr(ColorChoice::Auto);
            let any_errors = build_state
                .emit_diagnostics(&mut stderr, &diagnostics)
                .map_err(|e| eco_format!("{e}"))?;

            if any_errors {
                return Err(eco_format!("build completed with errors"));
            }

            Ok(())
        }
        Command::Watch => {
            todo!("watch")
        }
    }
}
