#![warn(missing_docs)]
//! Rivers daemon — HTTP server, routing, ProcessPool dispatch, engine loading.

pub mod admin;
pub mod admin_auth;
pub mod admin_handlers;
pub mod tls;
pub mod cli;
pub mod backpressure;
/// App-level circuit breaker registry for manual DataView traffic control.
pub mod circuit_breaker;
pub mod broker_bridge;
pub mod bundle_diff;
pub mod bundle_loader;
pub mod engine_loader;
pub mod deployment;
pub mod error_response;
pub mod cors;
pub mod csrf;
pub mod graphql;
pub mod guard;
pub mod hot_reload;
pub mod health;
pub mod keystore;
pub mod init_handler;
pub mod message_consumer;
pub mod middleware;
pub mod polling;
pub mod pool;
pub mod process_pool;
pub mod rate_limit;
pub mod runtime;
pub mod schema_introspection;
pub mod security_pipeline;
pub mod server;
pub mod service_discovery;
pub mod session;
pub mod sse;
pub mod shutdown;
pub mod streaming;
pub mod static_files;
pub mod task_enrichment;
/// Per-request transaction state management.
pub mod transaction;
pub mod view_engine;
pub mod websocket;
/// MCP view type — JSON-RPC dispatcher for AI tool access.
pub mod mcp;
