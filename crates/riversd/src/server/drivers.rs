//! Driver registration — built-in and static-plugin drivers.
//!
//! cdylib driver plugins are disabled — they cause SIGABRT when the plugin
//! creates its own tokio runtime inside the host process. All drivers are
//! compiled statically via `static-builtin-drivers` and `static-plugins`
//! features. Engine dylibs (V8, WASM) are unaffected.

/// Register all drivers — built-in and static-plugin — into a `DriverFactory`.
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

    if !ignore.is_empty() {
        tracing::info!(ignored = ?ignore, "drivers ignored — bundles referencing these will fail validation");
    }
}
