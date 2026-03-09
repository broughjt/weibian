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
    let config = config::load_config(args.global.config_file.as_deref())?;

    let compile_args = match &args.command {
        Command::Compile(cmd) => &cmd.args,
        Command::Watch(cmd) => &cmd.args,
    };

    let build_config = config::BuildConfig::from(compile_args, &config)?;
    println!("{build_config:#?}");

    Ok(())
}
