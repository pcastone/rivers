use thiserror::Error;

/// Top-level error type for the Rivers framework.
///
/// Each variant maps to a distinct subsystem so callers can match
/// on the error source without inspecting message strings.
#[derive(Error, Debug)]
pub enum RiversError {
    #[error("config error: {0}")]
    Config(String),

    #[error("driver error: {0}")]
    Driver(String),

    #[error("dataview error: {0}")]
    DataView(String),

    #[error("pool error: {0}")]
    Pool(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("internal error: {0}")]
    Internal(String),
}
