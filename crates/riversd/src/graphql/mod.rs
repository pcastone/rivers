//! GraphQL integration types and async-graphql runtime.
//!
//! Per `rivers-view-layer-spec.md` §9.
//!
//! Provides the bridge between DataView configs and GraphQL schema generation.
//! Uses `async_graphql::dynamic` for runtime schema building from resolver mappings.

mod config;
mod mutations;
mod schema_builder;
mod types;

pub use config::*;
pub use mutations::*;
pub use schema_builder::*;
pub use types::*;
