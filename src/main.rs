//! rnm: a fast, lightweight Node.js version manager compatible with nvm.

mod cli;

use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use rnm::commands::{self, Context, Failure, UseOptions};

use crate::cli::{Cli, Command};

fn main() -> ExitCode {
  let cli = Cli::parse();
  let Some(command) = cli.command else {
    // No subcommand: print help, like nvm.
    let _ = Cli::command().print_help();
    return ExitCode::SUCCESS;
  };
  match run(command) {
    | Ok(()) => ExitCode::SUCCESS,
    | Err(failure) => {
      let _ = writeln!(std::io::stderr(), "{}", failure.message);
      ExitCode::from(u8::try_from(failure.code).unwrap_or(1))
    },
  }
}

fn run(command: Command) -> Result<(), Failure> {
  let ctx = Context::from_env()?;
  let stdout = std::io::stdout();
  let mut out = stdout.lock();
  match command {
    | Command::Env { shell, json } => {
      commands::env(&ctx, shell, json, &mut out, &mut std::io::stderr())
    },
    | Command::Use {
      spec,
      lts,
      silent,
      silent_if_unchanged,
      save,
    } => {
      let opts = UseOptions {
        version: spec,
        lts,
        silent,
        silent_if_unchanged,
        save,
      };
      commands::use_version(&ctx, &opts, &mut out)
    },
    | Command::Current => commands::current(&ctx, &mut out),
  }
}
