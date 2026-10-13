//! Command-line interface definition.
//!
//! Subcommands use nvm's names first; fnm's names are accepted as aliases.

use clap::{Args, Parser, Subcommand};
use std::io::IsTerminal;

use rnm::commands::{
  EnvOptions, ExecOptions, InstallOptions, LsOptions, LsRemoteOptions, Order, Progress, UseOptions,
};
use rnm::shell::{CdHook, ShellKind};
use rnm::version::LATEST_LTS;

/// What `--version` prints: `RNM_BUILD_VERSION` when the build sets it (for
/// example `0.1.0-nightly.20261013+ec57833`), otherwise Cargo's version.
const VERSION: &str = match option_env!("RNM_BUILD_VERSION") {
  | Some(version) if !version.is_empty() => version,
  | _ => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(name = "rnm", version = VERSION, about, propagate_version = true)]
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
  Env(EnvArgs),

  /// Switch this shell to an installed version.
  ///
  /// Without a version, uses the nearest .nvmrc or .node-version.
  Use(UseArgs),

  /// Print the active version.
  Current,

  /// Download and install a version, then switch this shell to it.
  ///
  /// Without a version, installs the one in the nearest .nvmrc or
  /// .node-version. The first install also creates the `default` alias.
  #[command(alias = "i")]
  Install(InstallArgs),

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

  /// List installed versions and aliases.
  #[command(alias = "list")]
  Ls(LsArgs),

  /// List aliases, or create one.
  ///
  /// `rnm alias` lists all aliases, `rnm alias <name>` those starting with
  /// <name>, and `rnm alias <name> <version>` points <name> at <version>.
  Alias {
    name: Option<String>,
    #[arg(value_name = "VERSION")]
    target: Option<String>,
    /// Never color the output.
    #[arg(long)]
    no_colors: bool,
  },

  /// Delete an alias.
  Unalias { name: String },

  /// Print the path of a version's node binary.
  Which {
    /// A version, alias, current, or system. Defaults to the nearest
    /// .nvmrc or .node-version.
    #[arg(value_name = "VERSION")]
    spec: Option<String>,
    /// Do not print which version file was used.
    #[arg(long)]
    silent: bool,
  },

  /// Run a command with a version's node first on PATH.
  ///
  /// `rnm exec 20 node app.js`. Without a version, uses the nearest .nvmrc
  /// or .node-version.
  Exec(ExecArgs),

  /// Print a shell completion script for rnm.
  ///
  /// Load it from your shell's startup file:
  ///   bash        eval "$(rnm completions --shell bash)"
  ///   zsh         eval "$(rnm completions --shell zsh)"   (after compinit)
  ///   fish        rnm completions --shell fish | source
  ///   PowerShell  rnm completions --shell powershell | Out-String | Invoke-Expression
  /// Nushell cannot load generated code at startup; save it once instead:
  ///   rnm completions --shell nu | save -f ($nu.data-dir | path join vendor/autoload/rnm-completions.nu)
  #[command(verbatim_doc_comment)]
  Completions {
    /// The shell to generate completions for. Detected from the parent
    /// process when omitted.
    #[arg(long, value_enum)]
    shell: Option<ShellKind>,
  },

  /// List the versions available on the mirror.
  ///
  /// Also refreshes the LTS aliases (lts/*, lts/<name>) that `use` reads.
  #[command(alias = "list-remote")]
  LsRemote(LsRemoteArgs),
}

#[derive(Debug, Args)]
pub struct EnvArgs {
  /// The shell to generate code for. Detected from the parent process
  /// when omitted.
  #[arg(long, value_enum)]
  pub shell: Option<ShellKind>,
  /// Print the environment variables as JSON instead of shell code.
  #[arg(long)]
  pub json: bool,
  /// Switch versions automatically, like nvm's README recipes: after every
  /// directory change (and now), use the nearest .nvmrc or .node-version,
  /// in this directory or a parent. Without one, use the `default` alias.
  #[arg(long)]
  pub use_on_cd: bool,
  /// With --use-on-cd: install a missing version instead of reporting it.
  #[arg(long, requires = "use_on_cd")]
  pub install_if_missing: bool,
}

#[derive(Debug, Args)]
#[allow(
  clippy::struct_excessive_bools,
  reason = "each bool is one command-line flag"
)]
pub struct UseArgs {
  /// A version (18, v20.11.1), lts/*, lts/<name>, node, system, or an
  /// alias.
  #[arg(value_name = "VERSION")]
  pub spec: Option<String>,
  /// Use the newest LTS, or the named LTS line with --lts=<name>.
  #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
  pub lts: Option<String>,
  /// Print nothing on success.
  #[arg(long)]
  pub silent: bool,
  /// Print nothing if this version is already in use.
  #[arg(long)]
  pub silent_if_unchanged: bool,
  /// Write the version to ./.nvmrc.
  #[arg(long, short = 'w')]
  pub save: bool,
  /// Install the version first if it is not installed.
  #[arg(long)]
  pub install_if_missing: bool,
  /// Run by the `env --use-on-cd` hook: without a version file, switch to
  /// the `default` alias if there is one.
  #[arg(long, hide = true, conflicts_with_all = ["spec", "lts", "save"])]
  pub from_hook: bool,
}

#[derive(Debug, Args)]
#[allow(
  clippy::struct_excessive_bools,
  reason = "each bool is one command-line flag"
)]
pub struct InstallArgs {
  /// A version (18, v20.11.1), lts/*, lts/<name>, or node.
  #[arg(value_name = "VERSION")]
  pub spec: Option<String>,
  /// Install the newest LTS, or the named LTS line with --lts=<name>.
  #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
  pub lts: Option<String>,
  /// Point the `default` alias at the requested version.
  #[arg(long, conflicts_with = "alias")]
  pub default: bool,
  /// Point this alias at the requested version.
  #[arg(long, require_equals = true, value_name = "NAME")]
  pub alias: Option<String>,
  /// Write the installed version to ./.nvmrc.
  #[arg(long, short = 'w')]
  pub save: bool,
  /// Do not switch this shell to the installed version.
  #[arg(long)]
  pub no_use: bool,
  /// Do not draw a download progress bar.
  #[arg(long)]
  pub no_progress: bool,
  /// Use no network: pick among installed and cached versions, and install
  /// from the download cache.
  #[arg(long)]
  pub offline: bool,
  /// nvm's "binary only" flag. rnm always installs binaries, so it has no
  /// effect.
  #[arg(short = 'b', hide = true)]
  pub binary_only: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
  /// Only versions matching this (18, v20.1, iojs, system, or an alias).
  #[arg(value_name = "VERSION")]
  pub pattern: Option<String>,
  /// Never color the output.
  #[arg(long)]
  pub no_colors: bool,
  /// Do not list aliases.
  #[arg(long)]
  pub no_alias: bool,
}

#[derive(Debug, Args)]
pub struct ExecArgs {
  /// The version to use (fnm's spelling); otherwise the first argument is
  /// the version when it names an installed one.
  #[arg(long, value_name = "VERSION")]
  pub using: Option<String>,
  /// Do not print `Running node …`.
  #[arg(long)]
  pub silent: bool,
  /// [VERSION] COMMAND [ARGS]...
  #[arg(
    value_name = "ARGS",
    trailing_var_arg = true,
    allow_hyphen_values = true,
    required = true
  )]
  pub args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct LsRemoteArgs {
  /// Only versions starting with this prefix (18, v20.1), or lts/*,
  /// lts/<name>.
  #[arg(value_name = "VERSION")]
  pub pattern: Option<String>,
  /// The same as VERSION (fnm's spelling).
  #[arg(long, value_name = "VERSION", conflicts_with = "pattern")]
  pub filter: Option<String>,
  /// The listing order: oldest first (asc, nvm's order) or newest first.
  #[arg(long, value_enum, default_value_t = SortOrder::Asc)]
  pub sort: SortOrder,
  /// Only the newest matching version.
  #[arg(long)]
  pub latest: bool,
  /// Only LTS versions, or one LTS line with --lts=<name>.
  #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = LATEST_LTS, value_name = "NAME")]
  pub lts: Option<String>,
  /// Never color the output.
  #[arg(long)]
  pub no_colors: bool,
}

/// A progress bar when stderr is a terminal, unless `--no-progress`.
fn progress(no_progress: bool) -> Progress {
  if !no_progress && std::io::stderr().is_terminal() {
    Progress::Bar
  } else {
    Progress::Hidden
  }
}

impl From<EnvArgs> for EnvOptions {
  fn from(args: EnvArgs) -> Self {
    Self {
      shell: args.shell,
      json: args.json,
      use_on_cd: args.use_on_cd.then_some(CdHook {
        install_if_missing: args.install_if_missing,
      }),
      progress: progress(false),
    }
  }
}

impl From<UseArgs> for UseOptions {
  fn from(args: UseArgs) -> Self {
    Self {
      version: args.spec,
      lts: args.lts,
      silent: args.silent,
      silent_if_unchanged: args.silent_if_unchanged,
      save: args.save,
      from_hook: args.from_hook,
      install_if_missing: args.install_if_missing,
      progress: progress(false),
    }
  }
}

impl From<InstallArgs> for InstallOptions {
  fn from(args: InstallArgs) -> Self {
    Self {
      version: args.spec,
      lts: args.lts,
      default: args.default,
      alias: args.alias,
      save: args.save,
      no_use: args.no_use,
      progress: progress(args.no_progress),
      offline: args.offline,
    }
  }
}

impl From<LsArgs> for LsOptions {
  fn from(args: LsArgs) -> Self {
    Self {
      pattern: args.pattern,
      no_colors: args.no_colors,
      no_alias: args.no_alias,
      terminal: std::io::stdout().is_terminal(),
    }
  }
}

/// `ls-remote --sort` values, fnm's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SortOrder {
  /// Oldest first.
  Asc,
  /// Newest first.
  Desc,
}

impl From<LsRemoteArgs> for LsRemoteOptions {
  fn from(args: LsRemoteArgs) -> Self {
    Self {
      pattern: args.pattern.or(args.filter),
      order: match args.sort {
        | SortOrder::Asc => Order::Ascending,
        | SortOrder::Desc => Order::Descending,
      },
      latest_only: args.latest,
      lts: args.lts,
      no_colors: args.no_colors,
      terminal: std::io::stdout().is_terminal(),
    }
  }
}

impl From<ExecArgs> for ExecOptions {
  fn from(args: ExecArgs) -> Self {
    Self {
      using: args.using,
      args: args.args,
      silent: args.silent,
    }
  }
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
