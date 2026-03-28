//! TaskContext auto-enrichment — wires capabilities from shared state.
//!
//! Called from dispatch sites to automatically inject all available
//! capabilities (keystore, storage, driver_factory, dataview_executor)
//! without each site needing to know about them individually.
//!
//! **Pattern:** Every `TaskContextBuilder::new()` dispatch site calls
//! `enrich(builder, app_id)` before `.build()`. When a new capability
//! is added (e.g. lockbox HMAC keys), it's wired here once — every
//! handler gets it automatically.

use rivers_runtime::process_pool::TaskContextBuilder;

/// Enrich a TaskContextBuilder with all capabilities available from shared state.
///
/// New capabilities added here are automatically available to all handlers.
/// Dispatch sites call this instead of manually wiring each capability.
///
/// # Arguments
/// * `builder` — The partially-built TaskContext (entrypoint, args, trace_id already set).
/// * `app_id` — The app's entry_point name, used for keystore scoping. Pass `""` if unknown.
pub fn enrich(mut builder: TaskContextBuilder, app_id: &str) -> TaskContextBuilder {
    // ── App identity ────────────────────────────────────────────
    if !app_id.is_empty() {
        builder = builder.app_id(app_id.into());
    }

    // ── Keystore (Rivers.keystore + Rivers.crypto.encrypt/decrypt) ──
    if let Some(resolver) = crate::process_pool::get_keystore_resolver() {
        if let Some(ks) = resolver.get_for_entry_point(app_id) {
            builder = builder.keystore(ks.clone());
        }
    }

    // TODO: Wire storage_engine, driver_factory, dataview_executor
    // when they become available via globals. Currently only set for
    // dynamic engines in engine_loader.rs; static engines get them
    // via different paths. The enrichment layer establishes the pattern;
    // capabilities can be added incrementally.

    builder
}
