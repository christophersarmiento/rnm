//! rnm: a fast, lightweight Node.js version manager compatible with nvm.
//!
//! The library holds all logic; `main.rs` is only the command-line front end.

pub mod multishell;
pub mod nvm_dir;
pub mod sys;
pub mod version;
