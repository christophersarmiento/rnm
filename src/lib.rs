//! rnm: a fast, lightweight Node.js version manager compatible with nvm.
//!
//! The library holds all logic; `main.rs` is only the command-line front end.

pub mod commands;
pub mod fs;
pub mod http;
pub mod multishell;
pub mod nvm_dir;
pub mod shell;
pub mod sys;
pub mod version;
