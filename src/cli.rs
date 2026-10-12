//! Command-line interface definition.
//!
//! Subcommands use nvm's names first; fnm's names are accepted as aliases.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rnm", version, about, propagate_version = true)]
pub struct Cli {
  #[command(subcommand)]
  pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {}

#[cfg(test)]
mod tests {
  use super::Cli;
  use clap::CommandFactory;

  #[test]
  fn cli_definition_is_valid() {
    Cli::command().debug_assert();
  }
}
