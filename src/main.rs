mod config;

use std::process::ExitCode;

use config::Arguments;
use clap::Parser;
use typst::diag::StrResult;

fn main() -> ExitCode {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(e) => {
            e.print().expect("failed to print clap error");
            return ExitCode::from(e.exit_code() as u8);
        }
    };

    match dispatch(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(arguments: Arguments) -> StrResult<()> {
    let build_config = config::BuildConfig::try_load(arguments)?;
    println!("{build_config:#?}");

    for result in build_config.iter_typst_sources() {
        match result {
            Ok(path) => println!("{}", path.display()),
            Err(e) => eprintln!("walk error: {e}"),
        }
    }

    Ok(())
}
