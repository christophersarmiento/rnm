//! Node.js versions, nvm's version-spec grammar, and version files.
//!
//! Everything here is pure parsing. Resolving a spec against installed or
//! remote versions happens elsewhere.

mod exact;
mod file;
mod range;
mod spec;

pub use exact::{ParseVersionError, Version};
pub use file::{
  NvmrcContent, VersionFile, VersionFileError, VersionFileKind, decode_version_file,
  find_version_file, parse_node_version, parse_nvmrc,
};
pub use range::{NodeRange, ParseRangeError};
pub use spec::{
  IOJS_PREFIX, LATEST_LTS, LTS_PREFIX, LtsSpec, SpecError, VersionReq, VersionSpec,
  is_valid_alias_component, looks_like_range,
};
