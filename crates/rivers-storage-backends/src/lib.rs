//! Storage backend implementations for Rivers.
//!
//! Provides Redis and SQLite backends for the `StorageEngine` trait
//! defined in `rivers-core-config`. Can be compiled statically (rlib)
//! or loaded as a cdylib from `lib/`.

#![warn(missing_docs)]

/// Redis-backed StorageEngine (single-node and cluster).
mod redis_backend;
/// SQLite-backed StorageEngine (WAL mode, file or in-memory).
mod sqlite_backend;

pub use redis_backend::RedisStorageEngine;
pub use sqlite_backend::SqliteStorageEngine;
