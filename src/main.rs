mod config;
mod files;
mod world;

use std::process::ExitCode;

use clap::Parser;
use config::{Arguments, Command};
use typst::diag::StrResult;

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
        Command::Compile => {
            for result in config.iter_typst_sources() {
                match result {
                    Ok(path) => println!("{}", path.display()),
                    Err(error) => eprintln!("walk error: {error}"),
                }
            }
            Ok(())
        }
        Command::Watch => {
            todo!("watch")
        }
    }
}
