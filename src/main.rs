mod build;
mod compiler;
mod config;
mod file_store;
mod import_graph;
mod watch;
mod world;

use std::process::ExitCode;

use crate::build::Builder;
use crate::watch::WatchState;
use anyhow::anyhow;
use clap::Parser;
use config::{Arguments, Command};

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
            let builder = Builder::new(config);
            let has_errors = builder.build()?;

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
