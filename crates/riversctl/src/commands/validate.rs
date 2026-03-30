/// Validate a bundle or output config JSON Schema.
///
/// `riversctl validate <bundle_path>` — run all validation checks on a bundle
/// `riversctl validate --schema server|app|bundle` — output JSON Schema to stdout
pub fn cmd_validate(args: &[String]) -> Result<(), String> {
    // --schema mode: output JSON Schema
    if args.first().map(|s| s.as_str()) == Some("--schema") {
        let schema_type = args.get(1).map(|s| s.as_str()).unwrap_or("server");
        let schema = match schema_type {
            "server" => rivers_runtime::rivers_core_config::server_config_schema(),
            "app" => rivers_runtime::app_config_schema(),
            "bundle" => rivers_runtime::bundle_manifest_schema(),
            other => return Err(format!("unknown schema type '{}' (expected: server, app, bundle)", other)),
        };
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        return Ok(());
    }

    // Bundle validation mode
    let bundle_path = args.first().ok_or("Usage: riversctl validate <bundle_path> | --schema <type>")?;
    let path = std::path::Path::new(bundle_path);

    if !path.is_dir() {
        return Err(format!("bundle path '{}' is not a directory", bundle_path));
    }

    println!("Validating bundle: {}", path.display());

    // Load bundle
    let bundle = rivers_runtime::load_bundle(path)
        .map_err(|e| format!("bundle load failed: {}", e))?;

    println!("  Loaded: {} app(s)", bundle.apps.len());
    for app in &bundle.apps {
        println!("    - {} ({})", app.manifest.app_name, app.manifest.app_type);
    }

    // Run bundle validation (9 checks: view types, datasource refs, DataView refs,
    // invalidates targets, duplicate names, service refs, schema files, etc.)
    let mut error_count = 0;
    if let Err(errors) = rivers_runtime::validate_bundle(&bundle) {
        for e in &errors {
            eprintln!("  [ERROR] {}", e);
        }
        error_count += errors.len();
    }

    // Check keystore files exist (warning only — file may be created at runtime)
    // NOTE: Lockbox alias existence is verified at startup by bundle_loader.rs,
    // not here. riversctl validate runs offline without Lockbox access.
    // See rivers-feature-request-app-keystore.md §8.
    let has_keystores = bundle.apps.iter().any(|a| !a.config.data.keystore.is_empty());
    for app in &bundle.apps {
        for (name, ks_config) in &app.config.data.keystore {
            let ks_path = app.app_dir.join(&ks_config.path);
            if !ks_path.exists() {
                eprintln!("  [WARN]  keystore '{}' file not found: {}", name, ks_path.display());
            }
        }
    }
    if has_keystores {
        eprintln!("  [NOTE]  Keystore lockbox aliases will be verified at startup (requires Lockbox access)");
    }

    // Run driver name validation (hardcoded names — avoids pulling in DriverFactory + all drivers)
    let known: Vec<&str> = vec![
        // Built-in database drivers
        "eventbus", "faker", "memcached", "mysql", "postgres", "redis", "rps-client", "sqlite",
        // Plugin database drivers
        "cassandra", "couchdb", "elasticsearch", "http", "influxdb", "ldap", "mongodb", "rivers-exec",
        // Plugin broker drivers
        "kafka", "nats", "rabbitmq", "redis-streams",
    ];
    let driver_errors = rivers_runtime::validate_known_drivers(&bundle, &known);
    for e in &driver_errors {
        eprintln!("  [WARN]  {}", e);
    }

    if error_count == 0 {
        println!("  [PASS]  Bundle is valid ({} warning(s))", driver_errors.len());
        Ok(())
    } else {
        Err(format!("{} validation error(s) found", error_count))
    }
}
