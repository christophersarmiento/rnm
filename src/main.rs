//! rnm: a fast, lightweight Node.js version manager compatible with nvm.

mod cli;

use clap::Parser;

fn main() {
  let cli = cli::Cli::parse();
  match cli.command {
    | None => {
      // No subcommand: print help, like nvm.
      let _ = <cli::Cli as clap::CommandFactory>::command().print_help();
    },
  }
}
