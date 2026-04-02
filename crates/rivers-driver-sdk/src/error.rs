use thiserror::Error;

/// Driver error type.
///
/// Per spec: "Drivers must not panic. All errors must be returned as DriverError.
/// Error messages must not contain credential material."
#[derive(Error, Debug)]
pub enum DriverError {
    /// DriverFactory lookup miss — no driver registered with this name.
    #[error("unknown driver: {0}")]
    UnknownDriver(String),

    /// connect() failure — could not establish connection.
    #[error("connection error: {0}")]
    Connection(String),

    /// execute() failure — query-level error.
    #[error("query error: {0}")]
    Query(String),

    /// begin/commit/rollback failure.
    #[error("transaction error: {0}")]
    Transaction(String),

    /// Operation not supported by this driver type (permanent).
    /// E.g., a read-only driver receiving a write request.
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// Operation recognized but not yet implemented (temporary).
    /// E.g., a driver stub that has the trait method but no real backend wired.
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// Driver-internal error, unexpected state.
    #[error("internal driver error: {0}")]
    Internal(String),

    /// Operation rejected by security policy.
    /// Semantically distinct from `Unsupported` — the operation is implemented
    /// but blocked (e.g., DDL via execute(), admin op without whitelist).
    #[error("forbidden: {0}")]
    Forbidden(String),
}
