use std::process::ExitCode;

use anyhow::anyhow;
use clap::Parser;
use weibian::build::Builder;
use weibian::config::{Arguments, BuildConfig, Command};
use weibian::watch::Watcher;

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
        inputs,
        command,
    } = arguments;
    let config = BuildConfig::try_load(config_file, inputs)?;

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
            let mut watcher = Watcher::new(config);

            watcher.watch()?;

            Ok(())
        }
    }
}
