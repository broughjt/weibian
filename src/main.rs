mod build;
mod compile;
mod config;
mod file_store;
mod import_graph;
mod watch;
mod world;

use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;
use config::{Arguments, Command};
use termcolor::{ColorChoice, StandardStream};

use crate::build::BuildState;
use crate::watch::WatchState;

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

fn dispatch(arguments: Arguments) -> anyhow::Result<()> {
    let Arguments {
        config_file,
        command,
    } = arguments;
    let config = config::BuildConfig::try_load(config_file)?;

    match command {
        Command::Build => {
            let build_state = BuildState::new(config);
            let diagnostics = build_state.build()?;
            let mut stderr = StandardStream::stderr(ColorChoice::Auto);

            let has_errors = build_state.emit_diagnostics(&mut stderr, &diagnostics)?;

            if has_errors {
                return Err(anyhow!("build completed with errors"));
            }

            Ok(())
        }
        Command::Watch => {
            let mut watch_state = WatchState::new(config);

            watch_state.watch()?;

            Ok(())
        }
    }
}
