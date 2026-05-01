//! Static plugin registry integration test (RW3.2.a).
//!
//! Verifies that every plugin listed in the `static-plugins` Cargo feature is
//! present in the driver inventory produced by `register_all_drivers()`.
//!
//! If a plugin is added to `static-plugins` in `Cargo.toml` without a
//! matching call in `server::drivers::register_all_drivers()`, this test
//! will fail, preventing silent inventory drift.
//!
//! Guard: `#[cfg(feature = "static-plugins")]` — the test compiles and runs
//! only when the feature is enabled (it is part of the `default` feature set,
//! so plain `cargo test -p riversd` includes it).

#[cfg(feature = "static-plugins")]
mod static_plugin_registry {
    use rivers_runtime::rivers_core::DriverFactory;
    use riversd::server::register_all_drivers;

    /// Canonical list of driver names that must appear in the registry when
    /// the `static-plugins` feature is active.
    ///
    /// - Database plugins register under their own name (e.g. "cassandra").
    /// - Broker plugins register under their own name (e.g. "kafka").
    /// - The exec plugin registers as "rivers-exec" (its `Driver::name()`
    ///   return value), which is intentionally distinct from the crate name
    ///   "exec" to avoid conflicts with the built-in exec primitive.
    const EXPECTED_PLUGINS: &[&str] = &[
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

    /// Every plugin listed in `static-plugins` must be registered by
    /// `register_all_drivers()` with no entries in the ignore list.
    #[test]
    fn all_static_plugins_are_registered() {
        let mut factory = DriverFactory::new();
        register_all_drivers(&mut factory, &[]);

        let db_names = factory.driver_names();
        let broker_names = factory.broker_driver_names();

        let missing: Vec<&str> = EXPECTED_PLUGINS
            .iter()
            .copied()
            .filter(|name| !db_names.contains(name) && !broker_names.contains(name))
            .collect();

        assert!(
            missing.is_empty(),
            "static-plugins inventory is incomplete — missing drivers: {:?}\n\
             registered database drivers:  {:?}\n\
             registered broker drivers:    {:?}\n\
             \n\
             Fix: add the missing driver(s) to `register_all_drivers()` in\n\
             crates/riversd/src/server/drivers.rs",
            missing,
            db_names,
            broker_names,
        );
    }

    /// No plugin name in EXPECTED_PLUGINS should appear in both the database
    /// registry and the broker registry — that would indicate a double
    /// registration bug.
    #[test]
    fn no_plugin_registered_twice() {
        let mut factory = DriverFactory::new();
        register_all_drivers(&mut factory, &[]);

        let db_names = factory.driver_names();
        let broker_names = factory.broker_driver_names();

        let duplicates: Vec<&str> = EXPECTED_PLUGINS
            .iter()
            .copied()
            .filter(|name| db_names.contains(name) && broker_names.contains(name))
            .collect();

        assert!(
            duplicates.is_empty(),
            "drivers registered in BOTH database and broker registries: {:?}",
            duplicates,
        );
    }

    /// Verify the ignore list is respected — plugins in the ignore list must
    /// not appear in the registry after `register_all_drivers()` completes.
    #[test]
    fn ignored_plugins_are_excluded() {
        let ignore = vec!["cassandra".to_string(), "kafka".to_string()];
        let mut factory = DriverFactory::new();
        register_all_drivers(&mut factory, &ignore);

        let db_names = factory.driver_names();
        let broker_names = factory.broker_driver_names();

        assert!(
            !db_names.contains(&"cassandra"),
            "cassandra should be absent when ignored, got db_names: {:?}",
            db_names,
        );
        assert!(
            !broker_names.contains(&"kafka"),
            "kafka should be absent when ignored, got broker_names: {:?}",
            broker_names,
        );

        // All other expected plugins should still be registered.
        let still_missing: Vec<&str> = EXPECTED_PLUGINS
            .iter()
            .copied()
            .filter(|name| !ignore.contains(&name.to_string()))
            .filter(|name| !db_names.contains(name) && !broker_names.contains(name))
            .collect();

        assert!(
            still_missing.is_empty(),
            "non-ignored plugins are missing from registry: {:?}",
            still_missing,
        );
    }
}
