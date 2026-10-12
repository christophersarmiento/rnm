//! The mirror's release list: fetching and caching `index.json`, resolving
//! version specs against it, and keeping nvm's LTS alias files current.

mod cache;
mod index;
mod lts;
mod resolve;

pub use cache::{IndexError, IndexWarning, Loaded, load_index};
pub use index::{Release, RemoteIndex, is_valid_codename};
pub use lts::refresh_lts_aliases;
pub use resolve::RemoteResolveError;
