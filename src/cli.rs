//! Command-line interface definition.
//!
//! Subcommands use nvm's names first; fnm's names are accepted as aliases.

use clap::{Parser, Subcommand};
use rnm::shell::ShellKind;
use rnm::version::LATEST_LTS;

#[derive(Debug, Parser)]
#[command(name = "rnm", version, about, propagate_version = true)]
pub struct Cli {
  #[command(subcommand)]
  pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
  /// Print shell code that sets up rnm in the current shell.
  ///
  /// Add the output to your shell's startup file, for example
  /// `eval "$(rnm env --shell bash)"` in ~/.bashrc.
  Env {
    /// The shell to generate code for. Detected from the parent process
    /// when omitted.
    #[arg(long, value_enum)]
    shell: Option<ShellKind>,
    /// Print the environment variables as JSON instead of shell code.
    #[arg(long)]
    json: bool,
  },

  /// Switch this shell to an installed version.
  ///
  /// Without a version, uses the nearest .nvmrc or .node-version.
  Use {
    /// A version (18, v20.11.1), lts/*, lts/<name>, node, system, or an
    /// alias.
    #[arg(value_name = "VERSION")]
    spec: Option<String>,
    /// Use the newest LTS, or the named LTS line with --lts=<name>.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
    lts: Option<String>,
    /// Print nothing on success.
    #[arg(long)]
    silent: bool,
    /// Print nothing if this version is already in use.
    #[arg(long)]
    silent_if_unchanged: bool,
    /// Write the version to ./.nvmrc.
    #[arg(long, short = 'w')]
    save: bool,
  },

  /// Print the active version.
  Current,

  /// Download and install a version, then switch this shell to it.
  ///
  /// Without a version, installs the one in the nearest .nvmrc or
  /// .node-version. The first install also creates the `default` alias.
  #[command(alias = "i")]
  Install {
    /// A version (18, v20.11.1), lts/*, lts/<name>, or node.
    #[arg(value_name = "VERSION")]
    spec: Option<String>,
    /// Install the newest LTS, or the named LTS line with --lts=<name>.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
    lts: Option<String>,
    /// Point the `default` alias at the requested version.
    #[arg(long, conflicts_with = "alias")]
    default: bool,
    /// Point this alias at the requested version.
    #[arg(long, require_equals = true, value_name = "NAME")]
    alias: Option<String>,
    /// Write the installed version to ./.nvmrc.
    #[arg(long, short = 'w')]
    save: bool,
    /// Do not switch this shell to the installed version.
    #[arg(long)]
    no_use: bool,
    /// Do not draw a download progress bar.
    #[arg(long)]
    no_progress: bool,
    /// nvm's "binary only" flag. rnm always installs binaries, so it has no
    /// effect.
    #[arg(short = 'b', hide = true)]
    binary_only: bool,
  },

  /// Remove an installed version and the aliases that point at it.
  Uninstall {
    /// A version, alias, lts/*, or lts/<name>, resolved among installed
    /// versions.
    #[arg(value_name = "VERSION", required_unless_present = "lts")]
    spec: Option<String>,
    /// Remove the newest installed LTS, or the named line with --lts=<name>.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME", conflicts_with = "spec")]
    lts: Option<String>,
  },

  /// List the versions available on the mirror.
  ///
  /// Also refreshes the LTS aliases (lts/*, lts/<name>) that `use` reads.
  #[command(alias = "list-remote")]
  LsRemote {
    /// Only versions starting with this prefix (18, v20.1), or lts/*,
    /// lts/<name>.
    #[arg(value_name = "VERSION")]
    pattern: Option<String>,
    /// Only LTS versions, or one LTS line with --lts=<name>.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
    lts: Option<String>,
    /// Never color the output.
    #[arg(long)]
    no_colors: bool,
  },
}

#[cfg(test)]
mod tests {
  use super::Cli;
  use clap::CommandFactory;

  #[test]
  fn cli_definition_is_valid() {
    Cli::command().debug_assert();
  }
}
