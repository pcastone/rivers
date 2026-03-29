//! ExecDriver configuration types and parsing.
//!
//! Configuration is extracted from `ConnectionParams.options`, which comes from
//! the TOML datasource `extra` map. Nested command configs arrive as flattened
//! dot-separated keys (e.g. `commands.network_scan.path`).

mod parser;
mod types;
mod validator;

pub use types::*;
