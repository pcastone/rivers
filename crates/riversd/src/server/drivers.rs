//! Driver registration — built-in and plugin drivers.

/// Register all drivers — built-in and plugin — into a `DriverFactory`.
///
/// Centralises the 15+ individual registration calls so that both
/// bundle-load and future code paths share one inventory.
///
/// Drivers listed in `ignore` are skipped with an INFO log.
pub fn register_all_drivers(
    factory: &mut rivers_runtime::rivers_core::DriverFactory,
    ignore: &[String],
) {
    // Built-in drivers — statically linked when feature is enabled
    #[cfg(feature = "static-builtin-drivers")]
    {
        rivers_runtime::rivers_core::register_builtin_drivers(factory);
    }

    // Static plugin drivers (only when compiled with "static-plugins" feature)
    #[cfg(feature = "static-plugins")]
    {
        use std::sync::Arc as A;
        let static_plugins: Vec<(&str, Box<dyn FnOnce(&mut rivers_runtime::rivers_core::DriverFactory)>)> = vec![
            ("cassandra",      Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_cassandra::CassandraDriver)); })),
            ("couchdb",        Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_couchdb::CouchDBDriver)); })),
            ("mongodb",        Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_mongodb::MongoDriver)); })),
            ("elasticsearch",  Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_elasticsearch::ElasticsearchDriver)); })),
            ("influxdb",       Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_influxdb::InfluxDriver)); })),
            ("ldap",           Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_ldap::LdapDriver)); })),
            ("kafka",          Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_kafka::KafkaDriver)); })),
            ("rabbitmq",       Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_rabbitmq::RabbitMqDriver)); })),
            ("nats",           Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_nats::NatsDriver)); })),
            ("rivers-exec",    Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_exec::ExecDriver)); })),
        ];
        for (name, register_fn) in static_plugins {
            if ignore.iter().any(|i| i == name) {
                tracing::info!(driver = name, "driver ignored per [plugins].ignore config");
            } else {
                register_fn(factory);
            }
        }
    }

    // Dynamic drivers from lib/ directory (builtin drivers dylib)
    let lib_dir = std::path::Path::new("lib");
    if lib_dir.is_dir() {
        let results = rivers_runtime::rivers_core::driver_factory::load_plugins(lib_dir, factory);
        for result in &results {
            match result {
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Success { path, driver_names } => {
                    // Check if any loaded driver names are in the ignore list
                    let ignored: Vec<&str> = driver_names.iter()
                        .filter(|d| ignore.iter().any(|i| i == *d))
                        .map(|d| d.as_str())
                        .collect();
                    if !ignored.is_empty() {
                        tracing::info!(path = %path, drivers = ?ignored, "driver library loaded but drivers ignored per config");
                    } else {
                        tracing::info!(path = %path, drivers = ?driver_names, "loaded driver library");
                    }
                }
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Failed { path, reason } => {
                    tracing::warn!(path = %path, reason = %reason, "failed to load driver library");
                }
            }
        }
    }

    // Dynamic plugin drivers from plugins/ directory
    let plugin_dir = std::path::Path::new("plugins");
    if plugin_dir.is_dir() {
        let results = rivers_runtime::rivers_core::driver_factory::load_plugins(plugin_dir, factory);
        for result in &results {
            match result {
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Success { path, driver_names } => {
                    let ignored: Vec<&str> = driver_names.iter()
                        .filter(|d| ignore.iter().any(|i| i == *d))
                        .map(|d| d.as_str())
                        .collect();
                    if !ignored.is_empty() {
                        tracing::info!(path = %path, drivers = ?ignored, "plugin loaded but drivers ignored per config");
                    } else {
                        tracing::info!(path = %path, drivers = ?driver_names, "loaded driver plugin");
                    }
                }
                rivers_runtime::rivers_core::driver_factory::PluginLoadResult::Failed { path, reason } => {
                    tracing::warn!(path = %path, reason = %reason, "failed to load driver plugin");
                }
            }
        }
    }

    if !ignore.is_empty() {
        tracing::info!(ignored = ?ignore, "drivers ignored — bundles referencing these will fail validation");
    }
}
