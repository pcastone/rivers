//! Storage backend implementations for Rivers.
//!
//! Provides Redis and SQLite backends for the `StorageEngine` trait.
//! Can be compiled statically or loaded as a cdylib from `lib/`.

mod redis_backend;
mod sqlite_backend;

pub use redis_backend::RedisStorageEngine;
pub use sqlite_backend::SqliteStorageEngine;
