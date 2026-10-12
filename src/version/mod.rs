//! Node.js versions, nvm's version-spec grammar, and version files.
//!
//! Everything here is pure parsing. Resolving a spec against installed or
//! remote versions happens elsewhere.

mod exact;
mod file;
mod spec;

pub use exact::{ParseVersionError, Version};
pub use file::{
  NvmrcContent, VersionFile, VersionFileError, VersionFileKind, decode_version_file,
  find_version_file, parse_node_version, parse_nvmrc,
};
pub use spec::{LtsSpec, SpecError, VersionReq, VersionSpec};
