//! ProcessPool shared types — used by both riversd and cdylib engines.
//!
//! Contains TaskContext, TaskResult, TaskError, the Worker trait,
//! serialization bridge, and TaskContextBuilder.

pub mod types;
mod bridge;

pub use types::*;
pub use bridge::TaskContextBuilder;
