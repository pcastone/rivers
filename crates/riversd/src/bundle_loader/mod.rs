//! Bundle loading and wiring extracted from server.rs (AN13.2).
//!
//! Loads a Rivers bundle from disk, resolves LockBox credentials,
//! registers DataViews, builds ConnectionParams, wires broker bridges
//! and MessageConsumer handlers, and detects guard views.

mod types;
mod load;
mod wire;
mod reload;

pub use load::load_and_wire_bundle;
pub use reload::rebuild_views_and_dataviews;
pub use types::ReloadSummary;
