//! DriverFactory — driver registry and plugin loading.
//!
//! Per `rivers-driver-spec.md` §7-§9.
//!
//! `DriverFactory` holds named registries for `DatabaseDriver` and
//! `MessageBrokerDriver` implementations. Built-in drivers register
//! at construction. Plugin drivers are loaded from shared libraries
//! at startup via `load_plugins()`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rivers_driver_sdk::{
    Connection, ConnectionParams, DatabaseDriver, DriverError, MessageBrokerDriver,
};

/// Callback type for event notifications from the DriverFactory.
///
/// Higher layers (e.g., the server startup code) wire this to the EventBus
/// to emit `DriverRegistered` and `PluginLoadFailed` events.
pub type EventNotifier = Box<dyn Fn(&str, serde_json::Value) + Send + Sync>;

// Re-export DriverRegistrar from the SDK so it's available from rivers_core::driver_factory
pub use rivers_driver_sdk::DriverRegistrar;

// ── DriverFactory ───────────────────────────────────────────────────

/// Central registry for all driver implementations.
///
/// Per spec §8. Two separate registries:
/// - `drivers` — `DatabaseDriver` implementations
/// - `broker_drivers` — `MessageBrokerDriver` implementations
pub struct DriverFactory {
    drivers: HashMap<String, Arc<dyn DatabaseDriver>>,
    broker_drivers: HashMap<String, Arc<dyn MessageBrokerDriver>>,
    /// Optional callback for emitting events to the EventBus.
    event_notifier: Option<EventNotifier>,
}

impl DriverFactory {
    /// Create a new empty factory.
    ///
    /// Built-in drivers should be registered via `register_database_driver()`
    /// after construction. This keeps the factory decoupled from specific
    /// driver implementations.
    pub fn new() -> Self {
        Self {
            drivers: HashMap::new(),
            broker_drivers: HashMap::new(),
            event_notifier: None,
        }
    }

    /// Set the event notifier callback.
    ///
    /// When set, the factory emits `DriverRegistered` events on successful
    /// registration and `PluginLoadFailed` events on plugin load failures.
    pub fn set_event_notifier(&mut self, notifier: EventNotifier) {
        self.event_notifier = Some(notifier);
    }

    /// Emit an event via the notifier, if one is set.
    fn emit_event(&self, event_type: &str, payload: serde_json::Value) {
        if let Some(ref notifier) = self.event_notifier {
            notifier(event_type, payload);
        }
    }

    /// Register a database driver.
    pub fn register_database_driver(&mut self, driver: Arc<dyn DatabaseDriver>) {
        let name = driver.name().to_string();
        self.drivers.insert(name.clone(), driver);
        self.emit_event(
            "DriverRegistered",
            serde_json::json!({ "driver_name": name, "driver_type": "database" }),
        );
    }

    /// Register a message broker driver.
    pub fn register_broker_driver(&mut self, driver: Arc<dyn MessageBrokerDriver>) {
        let name = driver.name().to_string();
        self.broker_drivers.insert(name.clone(), driver);
        self.emit_event(
            "DriverRegistered",
            serde_json::json!({ "driver_name": name, "driver_type": "broker" }),
        );
    }

    /// Look up a database driver by name and create a connection.
    ///
    /// Per spec §8.1. Returns `DriverError::UnknownDriver` if name is not registered.
    ///
    /// All drivers are statically compiled (cdylib plugin loading is disabled pending
    /// Plugin ABI v2). The previous `spawn_blocking` + dedicated `Runtime::new()` isolation
    /// is removed: it caused the tokio background connection task spawned by drivers like
    /// `PostgresDriver` and `MySQLDriver` to be cancelled the moment the temporary runtime
    /// was dropped, rendering every returned connection immediately dead
    /// ("connection closed" / "Tokio 1.x context was found, but it is being shutdown").
    /// Calling `driver.connect()` directly in the current async context is safe — the
    /// background task is spawned on the caller's runtime and survives for the connection
    /// lifetime.  When cdylib plugins are re-enabled (Plugin ABI v2), this isolation
    /// strategy will need to be revisited.
    pub async fn connect(
        &self,
        driver_name: &str,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let driver = self
            .drivers
            .get(driver_name)
            .ok_or_else(|| DriverError::UnknownDriver(driver_name.to_string()))?
            .clone();
        driver.connect(params).await
    }

    /// Get a reference to a database driver by name.
    pub fn get_driver(&self, name: &str) -> Option<&Arc<dyn DatabaseDriver>> {
        self.drivers.get(name)
    }

    /// Get a reference to a broker driver by name.
    pub fn get_broker_driver(&self, name: &str) -> Option<&Arc<dyn MessageBrokerDriver>> {
        self.broker_drivers.get(name)
    }

    /// List all registered database driver names.
    pub fn driver_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.drivers.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// List all registered broker driver names.
    pub fn broker_driver_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.broker_drivers.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Total number of registered drivers (database + broker).
    pub fn total_count(&self) -> usize {
        self.drivers.len() + self.broker_drivers.len()
    }
}

impl Default for DriverFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DriverFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverFactory")
            .field("drivers", &self.driver_names())
            .field("broker_drivers", &self.broker_driver_names())
            .field("has_event_notifier", &self.event_notifier.is_some())
            .finish()
    }
}

// ── DriverRegistrar impl ────────────────────────────────────────────

impl DriverRegistrar for DriverFactory {
    fn register_database_driver(&mut self, driver: Arc<dyn DatabaseDriver>) {
        self.register_database_driver(driver);
    }

    fn register_broker_driver(&mut self, driver: Arc<dyn MessageBrokerDriver>) {
        self.register_broker_driver(driver);
    }
}

// ── Plugin Loading ──────────────────────────────────────────────────

/// Result of attempting to load a single plugin.
#[derive(Debug)]
pub enum PluginLoadResult {
    /// Plugin loaded and registered successfully.
    Success {
        /// Filesystem path of the loaded shared library.
        path: String,
        /// Driver names that were registered by this plugin.
        driver_names: Vec<String>,
    },
    /// Plugin failed to load.
    Failed {
        /// Filesystem path that was attempted.
        path: String,
        /// Human-readable failure reason.
        reason: String,
    },
}

/// Load plugins from a directory.
///
/// Per spec §7.1-§7.3 and §9.1:
/// 1. Scan directory for shared libraries (.so, .dylib, .dll)
/// 2. Canonicalize paths to prevent duplicate loading via symlinks
/// 3. Load each library via `libloading`
/// 4. Check `_rivers_abi_version()` against `ABI_VERSION`
/// 5. Call `_rivers_register_driver()` inside `catch_unwind`
///
/// Returns a list of load results for each plugin attempted.
pub fn load_plugins(
    plugin_dir: &Path,
    factory: &mut DriverFactory,
) -> Vec<PluginLoadResult> {
    let mut results = Vec::new();
    let mut loaded_paths = std::collections::HashSet::new();

    // Read directory entries
    let entries = match std::fs::read_dir(plugin_dir) {
        Ok(entries) => entries,
        Err(e) => {
            let path_str = plugin_dir.display().to_string();
            let reason = format!("cannot read plugin directory: {}", e);
            factory.emit_event(
                "PluginLoadFailed",
                serde_json::json!({ "path": &path_str, "reason": &reason }),
            );
            results.push(PluginLoadResult::Failed {
                path: path_str,
                reason,
            });
            return results;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Filter by extension
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "so" | "dylib" | "dll") {
            continue;
        }

        // Canonical path deduplication (spec §7.1)
        let canonical = match path.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                let path_str = path.display().to_string();
                let reason = format!("cannot canonicalize path: {}", e);
                factory.emit_event(
                    "PluginLoadFailed",
                    serde_json::json!({ "path": &path_str, "reason": &reason }),
                );
                results.push(PluginLoadResult::Failed {
                    path: path_str,
                    reason,
                });
                continue;
            }
        };

        if !loaded_paths.insert(canonical.clone()) {
            // Already loaded this exact file (via symlink)
            continue;
        }

        let result = load_single_plugin(&canonical, factory);
        // Emit PluginLoadFailed for failed plugin loads
        if let PluginLoadResult::Failed { ref path, ref reason } = result {
            factory.emit_event(
                "PluginLoadFailed",
                serde_json::json!({ "path": path, "reason": reason }),
            );
        }
        results.push(result);
    }

    results
}

/// Load a single plugin shared library.
///
/// Per spec §7.2-§7.3:
/// - Load library via `libloading`
/// - Check `_rivers_abi_version` symbol
/// - Call `_rivers_register_driver` inside `catch_unwind`
fn load_single_plugin(
    path: &Path,
    factory: &mut DriverFactory,
) -> PluginLoadResult {
    let path_str = path.display().to_string();

    // Load library
    // SAFETY: Loading shared libraries is inherently unsafe. We validate ABI
    // version and wrap registration in catch_unwind per spec §7.3.
    let lib = match unsafe { libloading::Library::new(path) } {
        Ok(lib) => lib,
        Err(e) => {
            return PluginLoadResult::Failed {
                path: path_str,
                reason: format!("cannot load library: {}", e),
            };
        }
    };

    // Check ABI version (spec §7.2)
    let abi_version = match unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"_rivers_abi_version") } {
        Ok(func) => unsafe { func() },
        Err(_) => {
            return PluginLoadResult::Failed {
                path: path_str,
                reason: "missing _rivers_abi_version symbol".to_string(),
            };
        }
    };

    if abi_version != rivers_driver_sdk::ABI_VERSION {
        return PluginLoadResult::Failed {
            path: path_str,
            reason: format!(
                "ABI version mismatch: expected {}, got {}",
                rivers_driver_sdk::ABI_VERSION,
                abi_version
            ),
        };
    }

    // Get registration function
    // Note: trait objects are not FFI-safe in general, but Rivers plugins are
    // always compiled with the same Rust toolchain and ABI version is checked.
    #[allow(improper_ctypes_definitions)]
    type RegisterFn = unsafe extern "C" fn(registrar: &mut dyn DriverRegistrar);
    let register_fn = match unsafe { lib.get::<RegisterFn>(b"_rivers_register_driver") } {
        Ok(func) => *func,
        Err(_) => {
            return PluginLoadResult::Failed {
                path: path_str,
                reason: "missing _rivers_register_driver symbol".to_string(),
            };
        }
    };

    // Record driver names before registration to detect what was added
    let db_before: std::collections::HashSet<String> =
        factory.drivers.keys().cloned().collect();
    let broker_before: std::collections::HashSet<String> =
        factory.broker_drivers.keys().cloned().collect();

    // Call registration inside catch_unwind (spec §7.3)
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: ABI version verified above. Plugin implements the Rivers driver SDK.
        unsafe { register_fn(factory) };
    }));

    match panic_result {
        Ok(()) => {
            // Determine which drivers were newly registered
            let mut new_names: Vec<String> = factory
                .drivers
                .keys()
                .filter(|k| !db_before.contains(*k))
                .cloned()
                .collect();
            new_names.extend(
                factory
                    .broker_drivers
                    .keys()
                    .filter(|k| !broker_before.contains(*k))
                    .cloned(),
            );
            new_names.sort();

            // Keep the library alive — unloading is UB (spec §7.2)
            std::mem::forget(lib);

            PluginLoadResult::Success {
                path: path_str,
                driver_names: new_names,
            }
        }
        Err(panic_info) => {
            // Keep the library alive even on panic
            std::mem::forget(lib);

            let reason = if let Some(s) = panic_info.downcast_ref::<String>() {
                format!("plugin panicked during registration: {}", s)
            } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                format!("plugin panicked during registration: {}", s)
            } else {
                "plugin panicked during registration (unknown payload)".to_string()
            };

            PluginLoadResult::Failed {
                path: path_str,
                reason,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc as StdArc, Mutex};

    #[cfg(feature = "drivers")]
    #[test]
    fn test_event_notifier_fires_on_driver_registration() {
        let events: StdArc<Mutex<Vec<(String, serde_json::Value)>>> =
            StdArc::new(Mutex::new(Vec::new()));

        let events_clone = events.clone();
        let notifier: EventNotifier = Box::new(move |event_type, payload| {
            events_clone
                .lock()
                .unwrap()
                .push((event_type.to_string(), payload));
        });

        let mut factory = DriverFactory::new();
        factory.set_event_notifier(notifier);

        // Register a driver — should emit DriverRegistered
        factory
            .register_database_driver(Arc::new(crate::drivers::FakerDriver::new()));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "DriverRegistered");
        assert_eq!(captured[0].1["driver_name"], "faker");
        assert_eq!(captured[0].1["driver_type"], "database");
    }

    #[cfg(feature = "drivers")]
    #[test]
    fn test_no_events_without_notifier() {
        // Should not panic when no notifier is set
        let mut factory = DriverFactory::new();
        factory.register_database_driver(Arc::new(crate::drivers::FakerDriver::new()));
        assert_eq!(factory.driver_names(), vec!["faker"]);
    }

    #[test]
    fn test_plugin_load_failed_event_on_bad_directory() {
        let events: StdArc<Mutex<Vec<(String, serde_json::Value)>>> =
            StdArc::new(Mutex::new(Vec::new()));

        let events_clone = events.clone();
        let notifier: EventNotifier = Box::new(move |event_type, payload| {
            events_clone
                .lock()
                .unwrap()
                .push((event_type.to_string(), payload));
        });

        let mut factory = DriverFactory::new();
        factory.set_event_notifier(notifier);

        let results = load_plugins(Path::new("/nonexistent/plugin/dir"), &mut factory);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], PluginLoadResult::Failed { .. }));

        let captured = events.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, "PluginLoadFailed");
        assert!(captured[0].1["path"]
            .as_str()
            .unwrap()
            .contains("nonexistent"));
    }
}
