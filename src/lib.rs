//! rnm: a fast, lightweight Node.js version manager compatible with nvm.
//!
//! The library holds all logic; `main.rs` is only the command-line front end.

/// What `rnm --version` prints: `RNM_BUILD_VERSION` when the build sets it
/// (for example `0.1.0-nightly.20261013+ec57833`), otherwise Cargo's version.
pub const VERSION: &str = match option_env!("RNM_BUILD_VERSION") {
  | Some(version) if !version.is_empty() => version,
  | _ => env!("CARGO_PKG_VERSION"),
};

pub mod commands;
pub mod config;
pub mod fs;
pub mod http;
pub mod install;
pub mod multishell;
pub mod nvm_dir;
pub mod platform;
pub mod remote;
pub mod shell;
pub mod sys;
pub mod version;
