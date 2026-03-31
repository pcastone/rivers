use thiserror::Error;

/// Top-level error type for the Rivers framework.
///
/// Each variant maps to a distinct subsystem so callers can match
/// on the error source without inspecting message strings.
#[derive(Error, Debug)]
pub enum RiversError {
    /// Configuration parsing or validation failure.
    #[error("config error: {0}")]
    Config(String),

    /// Driver-level error (connection, query, protocol).
    #[error("driver error: {0}")]
    Driver(String),

    /// DataView resolution or execution failure.
    #[error("dataview error: {0}")]
    DataView(String),

    /// Connection pool error (exhausted, timeout, health check).
    #[error("pool error: {0}")]
    Pool(String),

    /// File or network I/O error.
    #[error("io error: {0}")]
    Io(String),

    /// Catch-all for unexpected internal errors.
    #[error("internal error: {0}")]
    Internal(String),
}
