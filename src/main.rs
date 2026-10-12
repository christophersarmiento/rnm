//! rnm: a fast, lightweight Node.js version manager compatible with nvm.

mod cli;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use rnm::commands::{self, Context, Failure};

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
      if !failure.message.is_empty() {
        let _ = writeln!(std::io::stderr(), "{}", failure.message);
      }
      ExitCode::from(failure.code)
    },
  }
}

fn run(command: Command) -> Result<(), Failure> {
  let ctx = Context::from_env()?;
  let stdout = std::io::stdout();
  let mut out = stdout.lock();
  let mut err = std::io::stderr();
  match command {
    | Command::Env(args) => commands::env(&ctx, &args.into(), &mut out, &mut err),
    | Command::Use(args) => commands::use_version(&ctx, &args.into(), &mut out),
    | Command::Current => commands::current(&ctx, &mut out),
    | Command::Install(args) => commands::install(&ctx, &args.into(), &mut out, &mut err),
    | Command::Uninstall { spec, lts } => {
      commands::uninstall(&ctx, spec.as_deref(), lts.as_deref(), &mut out, &mut err)
    },
    | Command::Ls(args) => commands::ls(&ctx, &args.into(), &mut out),
    | Command::Alias {
      name,
      target,
      no_colors,
    } => {
      let terminal = std::io::stdout().is_terminal();
      let (name, target) = (name.as_deref(), target.as_deref());
      commands::alias(&ctx, name, target, no_colors, terminal, &mut out, &mut err)
    },
    | Command::Unalias { name } => commands::unalias(&ctx, &name, &mut out, &mut err),
    | Command::Which { spec, silent } => commands::which(&ctx, spec.as_deref(), silent, &mut out),
    | Command::Exec(args) => commands::exec(&ctx, &args.into(), &mut out),
    | Command::LsRemote(args) => commands::ls_remote(&ctx, &args.into(), &mut out, &mut err),
  }
}
