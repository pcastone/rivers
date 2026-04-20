//! V8 JavaScript engine -- thread-locals, isolate pool, execute_js_task, all V8 callbacks.
//!
//! This module contains all code that depends on the `v8` crate.
//! Gated behind #[cfg(feature = "static-engines")].

mod catalog;
mod direct_dispatch;
mod proxy_codegen;
mod task_locals;
mod init;
mod execution;
mod context;
mod datasource;
mod rivers_global;
mod http;

// Re-export public API used by process_pool/mod.rs and siblings
pub(crate) use execution::execute_js_task;
pub(crate) use execution::is_module_syntax;
pub(crate) use init::ensure_v8_initialized;
pub(crate) use init::DEFAULT_HEAP_LIMIT;

// Test-only re-exports
#[cfg(test)]
pub(crate) use init::SCRIPT_CACHE;
#[cfg(test)]
pub(crate) use init::clear_script_cache;
