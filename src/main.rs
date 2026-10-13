//! rnm: a fast, lightweight Node.js version manager compatible with nvm.

mod cli;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use clap_complete_nushell::Nushell;
use rnm::commands::{self, Context, Failure};
use rnm::shell::ShellKind;

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
  if let Command::Completions { shell } = command {
    return completions(shell);
  }
  let ctx = Context::from_env()?;
  let stdout = std::io::stdout();
  let mut out = stdout.lock();
  let mut err = std::io::stderr();
  match command {
    | Command::Env(args) => commands::env(&ctx, &args.into(), &mut out, &mut err),
    | Command::Use(args) => commands::use_version(&ctx, &args.into(), &mut out, &mut err),
    | Command::Current => commands::current(&ctx, &mut out),
    | Command::Doctor => commands::doctor(&ctx, &mut out),
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
    | Command::ReinstallPackages { spec } => {
      commands::reinstall_packages_command(&ctx, &spec, &mut out)
    },
    | Command::InstallLatestNpm => commands::install_latest_npm_command(&ctx, &mut out),
    | Command::LsRemote(args) => commands::ls_remote(&ctx, &args.into(), &mut out, &mut err),
    | Command::Completions { .. } => unreachable!("handled before the context is built"),
  }
}

/// Writes the completion script, generated from the clap definition. It
/// needs no install tree, so it works before rnm is set up.
fn completions(shell: Option<ShellKind>) -> Result<(), Failure> {
  let shell = match shell {
    | Some(shell) => shell,
    | None => commands::infer_shell()?,
  };
  let mut command = without_hidden(&Cli::command());
  let name = command.get_name().to_owned();
  // Generated into memory, because the generators panic on a closed pipe.
  let mut script = Vec::new();
  match shell {
    | ShellKind::Bash => generate(Shell::Bash, &mut command, name, &mut script),
    | ShellKind::Zsh => generate(Shell::Zsh, &mut command, name, &mut script),
    | ShellKind::Fish => generate(Shell::Fish, &mut command, name, &mut script),
    | ShellKind::PowerShell => generate(Shell::PowerShell, &mut command, name, &mut script),
    | ShellKind::Nu => generate(Nushell, &mut command, name, &mut script),
  }
  let _ = std::io::stdout().write_all(&script);
  Ok(())
}

/// A copy of `command` without hidden arguments and subcommands, which the
/// completion generators would otherwise offer (`use --from-hook`).
fn without_hidden(command: &clap::Command) -> clap::Command {
  let mut copy = clap::Command::new(command.get_name().to_owned())
    .args(
      command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .cloned(),
    )
    .subcommands(
      command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .map(without_hidden),
    )
    .visible_aliases(command.get_visible_aliases().map(str::to_owned))
    .propagate_version(command.is_propagate_version_set());
  if let Some(about) = command.get_about() {
    copy = copy.about(about.clone());
  }
  if let Some(version) = command.get_version() {
    copy = copy.version(version.to_owned());
  }
  copy
}
