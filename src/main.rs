//! rnm: a fast, lightweight Node.js version manager compatible with nvm.

mod cli;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use rnm::commands::{
  self, Context, Failure, InstallOptions, LsRemoteOptions, Progress, UseOptions,
};

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
    | Command::Install {
      spec,
      lts,
      default,
      alias,
      save,
      no_use,
      no_progress,
      binary_only: _,
    } => {
      let opts = InstallOptions {
        version: spec,
        lts,
        default,
        alias,
        save,
        no_use,
        progress: if no_progress || !std::io::stderr().is_terminal() {
          Progress::Hidden
        } else {
          Progress::Bar
        },
      };
      commands::install(&ctx, &opts, &mut out, &mut std::io::stderr())
    },
    | Command::Uninstall { spec, lts } => commands::uninstall(
      &ctx,
      spec.as_deref(),
      lts.as_deref(),
      &mut out,
      &mut std::io::stderr(),
    ),
    | Command::LsRemote {
      pattern,
      lts,
      no_colors,
    } => {
      let opts = LsRemoteOptions {
        pattern,
        lts,
        no_colors,
        terminal: std::io::stdout().is_terminal(),
      };
      commands::ls_remote(&ctx, &opts, &mut out, &mut std::io::stderr())
    },
  }
}
