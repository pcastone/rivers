//! LockBox -- Age-encrypted local secret resolver.
//!
//! Per `rivers-lockbox-spec.md`, amended by SHAPE-5.
//!
//! Manages an Age-encrypted TOML keystore. At startup, `riversd`
//! validates entries and builds an in-memory name+alias -> entry index
//! for O(1) lookup. Secret values are never held in memory -- they are
//! read from disk, decrypted, used, and zeroized on every access.
//!
//! CodeComponent isolates never receive raw credentials -- only opaque
//! datasource tokens. Credentials stay host-side.

pub mod types;
pub mod validation;
pub mod resolver;
pub mod crypto;
pub mod key_source;
pub mod startup;

pub use types::*;
pub use validation::*;
pub use resolver::*;
pub use crypto::*;
pub use key_source::*;
pub use startup::*;
