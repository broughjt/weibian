mod args;
mod config;

use std::process::ExitCode;

use args::*;
use clap::Parser;
use typst::diag::StrResult;

fn main() -> ExitCode {
    let args = match CliArguments::try_parse() {
        Ok(args) => args,
        Err(e) => {
            e.print().expect("failed to print clap error");
            return ExitCode::from(e.exit_code() as u8);
        }
    };

    match dispatch(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: CliArguments) -> StrResult<()> {
    let build_config = config::BuildConfig::try_load(args)?;
    println!("{build_config:#?}");

    for result in build_config.iter_typst_sources() {
        match result {
            Ok(path) => println!("{}", path.display()),
            Err(e) => eprintln!("walk error: {e}"),
        }
    }

    Ok(())
}
