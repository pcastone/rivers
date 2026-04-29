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
            // RW2.7.e: redis-streams was compiled in via Cargo.toml but was never
            // registered here — bundles using redis-streams would silently fail validation.
            ("redis-streams",  Box::new(|f| { f.register_broker_driver(A::new(rivers_plugin_redis_streams::RedisStreamsDriver)); })),
            ("rivers-exec",    Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_exec::ExecDriver)); })),
            // RW2.7.d: neo4j was listed in Cargo.toml as optional but was not
            // registered here. Added to complete the static-plugin inventory.
            ("neo4j",          Box::new(|f| { f.register_database_driver(A::new(rivers_plugin_neo4j::Neo4jDriver)); })),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that every expected driver name is registered by `register_all_drivers()`.
    ///
    /// This test uses the `static-plugins` + `static-builtin-drivers` features to compile
    /// all drivers in. If a driver is removed from the static list, this test fails,
    /// preventing silent inventory drift.
    #[test]
    #[cfg(all(feature = "static-plugins", feature = "static-builtin-drivers"))]
    fn static_plugin_inventory_is_complete() {
        let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
        register_all_drivers(&mut factory, &[]);

        let db_names: Vec<&str> = factory.driver_names();
        let broker_names: Vec<&str> = factory.broker_driver_names();
        let all_names: Vec<&str> = db_names.iter().chain(broker_names.iter()).copied().collect();

        let expected: &[&str] = &[
            "cassandra",
            "couchdb",
            "mongodb",
            "elasticsearch",
            "influxdb",
            "ldap",
            "kafka",
            "rabbitmq",
            "nats",
            "redis-streams",
            "rivers-exec",
            "neo4j",
        ];

        let mut missing: Vec<&str> = Vec::new();
        for name in expected {
            if !all_names.contains(name) {
                missing.push(name);
            }
        }

        assert!(
            missing.is_empty(),
            "static plugin inventory is incomplete — missing drivers: {:?}\n\
             registered database drivers: {:?}\n\
             registered broker drivers: {:?}",
            missing,
            factory.driver_names(),
            factory.broker_driver_names(),
        );
    }

    /// Verify that `register_all_drivers` respects the ignore list.
    #[test]
    #[cfg(all(feature = "static-plugins", feature = "static-builtin-drivers"))]
    fn ignored_drivers_are_not_registered() {
        let mut factory = rivers_runtime::rivers_core::DriverFactory::new();
        register_all_drivers(&mut factory, &["cassandra".to_string(), "nats".to_string()]);

        let db_names = factory.driver_names();
        let broker_names = factory.broker_driver_names();

        assert!(
            !db_names.contains(&"cassandra"),
            "cassandra should be ignored"
        );
        assert!(
            !broker_names.contains(&"nats"),
            "nats should be ignored"
        );
    }
}
