#![warn(missing_docs)]
//! riverpackage — Rivers bundle validator and packager.
//!
//! Commands: init, validate, preflight, pack

use std::path::Path;
use uuid::Uuid;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let result = match args[1].as_str() {
        "--version" | "-V" | "version" => {
            println!("riverpackage {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
            return;
        }
        "init" => {
            let bundle_name = if args.len() >= 3 { &args[2] } else {
                eprintln!("Usage: riverpackage init <bundle-name> [--driver <driver>]");
                std::process::exit(1);
            };
            let driver = parse_flag(&args, "--driver").unwrap_or("faker");
            cmd_init(bundle_name, driver)
        }
        "validate" => cmd_validate(&args[2..]),
        "preflight" => {
            let bundle_dir = if args.len() >= 3 { &args[2] } else { "." };
            cmd_preflight(bundle_dir)
        }
        "pack" => {
            let bundle_dir = if args.len() >= 3 { &args[2] } else { "." };
            let output = if args.len() >= 4 { &args[3] } else { "bundle.tar.gz" };
            cmd_pack(bundle_dir, output)
        }
        "import-exec" => {
            let name = if args.len() >= 3 { &args[2] } else {
                eprintln!("Usage: riverpackage import-exec <command-name> <script-path> [--input-mode stdin|args|both]");
                std::process::exit(1);
            };
            let script_path = if args.len() >= 4 { &args[3] } else {
                eprintln!("Usage: riverpackage import-exec <command-name> <script-path> [--input-mode stdin|args|both]");
                std::process::exit(1);
            };
            let input_mode = if args.len() >= 6 && args[4] == "--input-mode" { &args[5] } else { "stdin" };
            cmd_import_exec(name, script_path, input_mode)
        }
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        other => { eprintln!("Unknown command: {other}"); print_usage(); std::process::exit(1); }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("riverpackage — Rivers bundle validator and packager");
    eprintln!();
    eprintln!("Usage: riverpackage <command> [bundle_dir] [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  init <name> [--driver <d>]  Scaffold a new bundle (drivers: faker, postgres, sqlite, mysql)");
    eprintln!("  validate [dir] [--format text|json] [--config <path>]");
    eprintln!("                              Validate bundle (Layers 1-4): structural, existence, cross-ref");
    eprintln!("  preflight [dir]             Validate + check schema/parameter orphans");
    eprintln!("  pack [dir] [output]         Package bundle into a .tar.gz archive");
    eprintln!("  import-exec <name> <path>   Generate ExecDriver TOML config for a script");
    eprintln!();
    eprintln!("Exit codes (validate):");
    eprintln!("  0  All checks passed");
    eprintln!("  1  Validation errors found");
    eprintln!("  2  Config error (bundle directory not found)");
    eprintln!("  3  Internal error");
}

/// Extract a named flag value from args, e.g. `--driver faker` → `Some("faker")`.
fn parse_flag<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
}

fn cmd_init(bundle_arg: &str, driver: &str) -> Result<(), String> {
    // Normalise driver alias
    let driver = match driver {
        "pg" => "postgres",
        other => other,
    };
    if !matches!(driver, "faker" | "postgres" | "sqlite" | "mysql") {
        return Err(format!(
            "unknown driver '{}' — supported: faker, postgres, sqlite, mysql",
            driver
        ));
    }

    // bundle_arg may be a path like /tmp/my-app — the logical app name is the basename.
    let bundle_root = Path::new(bundle_arg);
    let bundle_name = bundle_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("cannot determine bundle name from '{}'", bundle_arg))?;

    // Top-level bundle directory must not already exist
    if bundle_root.exists() {
        return Err(format!("directory '{}' already exists", bundle_arg));
    }

    let app_dir = bundle_root.join(bundle_name);
    let schemas_dir = app_dir.join("schemas");

    std::fs::create_dir_all(&schemas_dir)
        .map_err(|e| format!("create directory '{}': {e}", schemas_dir.display()))?;

    // --- bundle manifest.toml ---
    let bundle_manifest = format!(
        "bundleName    = \"{name}\"\nbundleVersion = \"1.0.0\"\nsource        = \"local\"\napps          = [\"{name}\"]\n",
        name = bundle_name
    );
    write_file(&bundle_root.join("manifest.toml"), &bundle_manifest)?;

    // --- app manifest.toml ---
    let app_id = Uuid::new_v4();
    let app_manifest = format!(
        "appName    = \"{name}\"\nversion    = \"1.0.0\"\ntype       = \"app-service\"\nappId      = \"{id}\"\nentryPoint = \"{name}\"\nsource     = \"local\"\n",
        name = bundle_name,
        id = app_id,
    );
    write_file(&app_dir.join("manifest.toml"), &app_manifest)?;

    // --- resources.toml (varies by driver) ---
    let (ds_name, resources_toml) = build_resources(bundle_name, driver);
    write_file(&app_dir.join("resources.toml"), &resources_toml)?;

    // --- app.toml ---
    let app_toml = build_app_toml(bundle_name, driver, &ds_name);
    write_file(&app_dir.join("app.toml"), &app_toml)?;

    // --- schemas/item.schema.json ---
    let schema_json = build_schema_json(driver);
    write_file(&schemas_dir.join("item.schema.json"), &schema_json)?;

    // Success output
    println!("Bundle created: {}/", bundle_arg);
    println!("  {}/manifest.toml", bundle_arg);
    println!("  {dir}/{name}/manifest.toml", dir = bundle_arg, name = bundle_name);
    println!("  {dir}/{name}/resources.toml", dir = bundle_arg, name = bundle_name);
    println!("  {dir}/{name}/app.toml", dir = bundle_arg, name = bundle_name);
    println!("  {dir}/{name}/schemas/item.schema.json", dir = bundle_arg, name = bundle_name);
    println!();
    println!("Next steps:");
    println!("  1. Edit {dir}/{name}/resources.toml with your datasource", dir = bundle_arg, name = bundle_name);
    println!("  2. Edit {dir}/{name}/schemas/ with your data model", dir = bundle_arg, name = bundle_name);
    println!("  3. Edit {dir}/{name}/app.toml with your DataViews and Views", dir = bundle_arg, name = bundle_name);
    println!("  4. riverpackage validate {}/", bundle_arg);

    Ok(())
}

/// Write `content` to `path`, propagating errors as `String`.
fn write_file(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content)
        .map_err(|e| format!("write '{}': {e}", path.display()))
}

/// Build resources.toml content and return `(datasource_name, toml_text)`.
///
/// Each datasource entry requires: `name`, `driver`, `x-type`, `required`.
/// `x-type` is the canonical driver type identifier used by the validator (Layer 1, S003).
fn build_resources(bundle_name: &str, driver: &str) -> (String, String) {
    match driver {
        "faker" => (
            "data".into(),
            "[[datasources]]\nname       = \"data\"\ndriver     = \"faker\"\nx-type     = \"faker\"\nnopassword = true\nrequired   = true\n".into(),
        ),
        "postgres" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"postgres\"\nx-type   = \"postgres\"\nhost     = \"localhost\"\nport     = 5432\ndatabase = \"{name}\"\nusername = \"postgres\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        "sqlite" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"sqlite\"\nx-type   = \"sqlite\"\nhost     = \"{name}.db\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        "mysql" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"mysql\"\nx-type   = \"mysql\"\nhost     = \"localhost\"\nport     = 3306\ndatabase = \"{name}\"\nusername = \"root\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        // unreachable — validated above
        _ => ("db".into(), String::new()),
    }
}

/// Build app.toml content for the scaffolded app.
///
/// DataView required fields: `name`, `datasource`.
/// View required fields: `path`, `method`, `view_type`, `handler`.
/// Handler required fields: `type`.
fn build_app_toml(bundle_name: &str, _driver: &str, ds_name: &str) -> String {
    let schema_path = "schemas/item.schema.json";
    let _ = bundle_name; // ds_name carries the relevant datasource identity
    format!(
        r#"[data.dataviews.list_items]
name          = "list_items"
datasource    = "{ds}"
query         = "SELECT * FROM items LIMIT ${{limit}}"
return_schema = "{schema}"

[[data.dataviews.list_items.parameters]]
name    = "limit"
type    = "integer"
default = 20

[api.views.items]
path       = "/items"
method     = "GET"
view_type  = "Rest"

[api.views.items.handler]
type     = "dataview"
dataview = "list_items"
"#,
        ds = ds_name,
        schema = schema_path,
    )
}

/// Build schemas/item.schema.json content.
fn build_schema_json(driver: &str) -> String {
    if driver == "faker" {
        r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "id": {
      "type": "string",
      "faker": "datatype.uuid"
    },
    "name": {
      "type": "string",
      "faker": "name.fullName"
    },
    "email": {
      "type": "string",
      "faker": "internet.email"
    }
  },
  "required": ["id", "name", "email"]
}
"#
        .into()
    } else {
        r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "id": {
      "type": "integer"
    },
    "name": {
      "type": "string"
    },
    "email": {
      "type": "string",
      "format": "email"
    }
  },
  "required": ["id", "name", "email"]
}
"#
        .into()
    }
}

/// Run the full 4-layer validation pipeline on a bundle directory.
///
/// Returns the report (used by `cmd_validate`, `cmd_preflight`, and `cmd_pack`).
fn run_validate(bundle_dir: &str) -> Result<rivers_runtime::ValidationReport, String> {
    let path = Path::new(bundle_dir);
    if !path.is_dir() {
        return Err(format!("'{}' is not a directory", bundle_dir));
    }

    let config = rivers_runtime::ValidationConfig {
        bundle_dir: path.to_path_buf(),
        engines: None,
    };

    Ok(rivers_runtime::validate_bundle_full(&config))
}

fn cmd_validate(args: &[String]) -> Result<(), String> {
    // Parse flags: --format text|json, --config <path>, positional bundle_dir
    let mut format = "text";
    let mut _config_path: Option<&str> = None;
    let mut bundle_dir = ".";
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" if i + 1 < args.len() => {
                format = match args[i + 1].as_str() {
                    "json" => "json",
                    "text" => "text",
                    other => {
                        eprintln!("Error: unknown format '{}' (expected: text, json)", other);
                        std::process::exit(2);
                    }
                };
                i += 2;
            }
            "--config" if i + 1 < args.len() => {
                _config_path = Some(&args[i + 1]);
                i += 2;
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: unknown flag '{}'", arg);
                std::process::exit(2);
            }
            _ => {
                bundle_dir = &args[i];
                i += 1;
            }
        }
    }

    let path = Path::new(bundle_dir);
    if !path.is_dir() {
        eprintln!("Error: '{}' is not a directory", bundle_dir);
        std::process::exit(2);
    }

    // Wire --config into engine discovery so Layer 4 can compile-check handlers.
    let engines = _config_path.and_then(|cfg| {
        match rivers_runtime::discover_engines(Path::new(cfg)) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("Warning: --config '{}': {e}", cfg);
                None
            }
        }
    });

    let config = rivers_runtime::ValidationConfig {
        bundle_dir: path.to_path_buf(),
        engines,
    };

    let report = rivers_runtime::validate_bundle_full(&config);

    match format {
        "json" => println!("{}", rivers_runtime::format_json(&report)),
        _ => print!("{}", rivers_runtime::format_text(&report)),
    }

    std::process::exit(report.exit_code());
}

fn cmd_preflight(bundle_dir: &str) -> Result<(), String> {
    // Run full validation pipeline
    let report = run_validate(bundle_dir)?;

    if report.has_errors() {
        // Print the text report so the user sees what went wrong
        print!("{}", rivers_runtime::format_text(&report));
        return Err(format!(
            "{} validation error(s) found",
            report.summary.total_failed
        ));
    }

    // Additional preflight checks (schema reference orphans)
    let path = Path::new(bundle_dir);
    let mut warnings: Vec<String> = Vec::new();

    let manifest_content = std::fs::read_to_string(path.join("manifest.toml"))
        .map_err(|e| format!("read manifest: {e}"))?;
    let manifest: toml::Value = toml::from_str(&manifest_content)
        .map_err(|e| format!("parse manifest: {e}"))?;

    if let Some(apps) = manifest.get("apps").and_then(|a| a.as_array()) {
        for app_val in apps {
            if let Some(app_name) = app_val.as_str() {
                preflight_app(path, app_name, &mut warnings);
            }
        }
    }

    if warnings.is_empty() {
        println!("Preflight OK: {bundle_dir}");
    } else {
        for w in &warnings {
            eprintln!("  WARNING: {w}");
        }
        println!("{} warnings", warnings.len());
    }

    Ok(())
}

fn preflight_app(bundle_path: &Path, app_name: &str, warnings: &mut Vec<String>) {
    let app_dir = bundle_path.join(app_name);
    let app_toml_path = app_dir.join("app.toml");

    let content = match std::fs::read_to_string(&app_toml_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let app_config: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Check schema file references
    if let Some(data) = app_config.get("data") {
        if let Some(dataviews) = data.get("dataviews").and_then(|d| d.as_table()) {
            for (dv_name, dv_config) in dataviews {
                for schema_field in &["get_schema", "post_schema", "put_schema", "delete_schema", "return_schema"] {
                    if let Some(schema_path) = dv_config.get(*schema_field).and_then(|v| v.as_str()) {
                        let full_path = app_dir.join(schema_path);
                        if !full_path.exists() {
                            warnings.push(format!(
                                "{app_name}: dataview '{dv_name}' references schema '{schema_path}' but file not found"
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn cmd_import_exec(name: &str, script_path: &str, input_mode: &str) -> Result<(), String> {
    use sha2::{Sha256, Digest};

    let path = Path::new(script_path);
    let abs_path = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve path '{}': {e}", script_path))?;

    if !abs_path.is_file() {
        return Err(format!("'{}' is not a file", abs_path.display()));
    }

    // Validate input_mode
    match input_mode {
        "stdin" | "args" | "both" => {}
        other => return Err(format!("invalid input_mode '{other}' — must be stdin, args, or both")),
    }

    // Compute SHA-256
    let bytes = std::fs::read(&abs_path)
        .map_err(|e| format!("cannot read '{}': {e}", abs_path.display()))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));

    // Print resources.toml snippet
    println!("# --- Add to resources.toml ---");
    println!();
    println!("[[datasources]]");
    println!("name       = \"exec_tools\"");
    println!("driver     = \"rivers-exec\"");
    println!("nopassword = true");
    println!("required   = true");

    // Print app.toml snippet
    println!();
    println!("# --- Add to app.toml ---");
    println!();
    println!("[data.datasources.exec_tools]");
    println!("name              = \"exec_tools\"");
    println!("driver            = \"rivers-exec\"");
    println!("run_as_user       = \"rivers-exec\"");
    println!("working_directory = \"/var/rivers/exec-scratch\"");
    println!("max_concurrent    = 10");
    println!();
    println!("[data.datasources.exec_tools.commands.{name}]");
    println!("path       = \"{}\"", abs_path.display());
    println!("sha256     = \"{hash}\"");
    println!("input_mode = \"{input_mode}\"");

    if input_mode == "args" || input_mode == "both" {
        println!("# args_template.0 = \"{{param1}}\"  # uncomment and edit");
        println!("# args_template.1 = \"--flag\"");
        println!("# args_template.2 = \"{{param2}}\"");
    }
    if input_mode == "both" {
        println!("# stdin_key     = \"data\"  # uncomment — key whose value is sent on stdin");
    }

    println!();
    println!("# SHA-256: {hash}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── --config flag tests ───────────────────────────────────────────────

    /// Verify that `--config` is recognized by the argument parser.
    #[test]
    fn config_flag_is_parsed() {
        let args: Vec<String> = vec![
            "--config".into(),
            "/etc/rivers/riversd.toml".into(),
        ];
        let parsed = parse_flag(&args, "--config");
        assert_eq!(parsed, Some("/etc/rivers/riversd.toml"));
    }

    /// Verify that --config appearing after a positional arg is also extracted.
    #[test]
    fn config_flag_is_parsed_after_positional() {
        let args: Vec<String> = vec![
            "my-bundle".into(),
            "--format".into(),
            "json".into(),
            "--config".into(),
            "/opt/rivers/config/riversd.toml".into(),
        ];
        let parsed = parse_flag(&args, "--config");
        assert_eq!(parsed, Some("/opt/rivers/config/riversd.toml"));
    }

    #[test]
    fn validate_catches_missing_manifest() {
        let report = run_validate("/tmp/nonexistent_bundle_dir_12345");
        // Should return Ok(report) with errors inside
        match report {
            Ok(r) => assert!(r.has_errors(), "nonexistent dir should have errors"),
            Err(_) => {} // Also acceptable — dir doesn't exist
        }
    }

    #[test]
    fn validate_address_book_bundle() {
        // Use the reference bundle in the repo
        let bundle_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../address-book-bundle");
        if std::path::Path::new(bundle_dir).exists() {
            let report = run_validate(bundle_dir).expect("run_validate should succeed");
            assert!(
                !report.has_errors(),
                "address-book-bundle should validate without errors.\nReport:\n{}",
                rivers_runtime::format_text(&report),
            );
        }
    }

    #[test]
    fn validate_catches_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        std::fs::write(&manifest, "invalid { toml content").unwrap();
        let report = run_validate(tmp.path().to_str().unwrap()).expect("run_validate should succeed");
        assert!(report.has_errors(), "invalid TOML should produce errors");
    }

    #[test]
    fn preflight_on_nonexistent_dir_fails() {
        let result = cmd_preflight("/tmp/nonexistent_12345");
        assert!(result.is_err());
    }

    // ── Golden: init → validate round-trip (all drivers) ─────────────

    /// Verifies that `cmd_init` produces a bundle that passes structural
    /// validation without errors for the faker driver (default scaffold).
    #[test]
    fn init_faker_validates_without_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("my-app");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "faker").expect("cmd_init should succeed for faker driver");

        let report = run_validate(bundle_str).expect("run_validate should succeed");
        assert!(
            !report.has_errors(),
            "faker init bundle should validate without errors.\nReport:\n{}",
            rivers_runtime::format_text(&report),
        );
    }

    /// Verifies that `cmd_init` with postgres driver also validates.
    #[test]
    fn init_postgres_validates_without_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("pg-app");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "postgres").expect("cmd_init should succeed for postgres driver");

        let report = run_validate(bundle_str).expect("run_validate should succeed");
        assert!(
            !report.has_errors(),
            "postgres init bundle should validate without errors.\nReport:\n{}",
            rivers_runtime::format_text(&report),
        );
    }

    /// Verifies that `cmd_init` with sqlite driver also validates.
    #[test]
    fn init_sqlite_validates_without_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("sqlite-app");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "sqlite").expect("cmd_init should succeed for sqlite driver");

        let report = run_validate(bundle_str).expect("run_validate should succeed");
        assert!(
            !report.has_errors(),
            "sqlite init bundle should validate without errors.\nReport:\n{}",
            rivers_runtime::format_text(&report),
        );
    }

    /// Verifies that `cmd_init` with mysql driver also validates.
    #[test]
    fn init_mysql_validates_without_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("mysql-app");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "mysql").expect("cmd_init should succeed for mysql driver");

        let report = run_validate(bundle_str).expect("run_validate should succeed");
        assert!(
            !report.has_errors(),
            "mysql init bundle should validate without errors.\nReport:\n{}",
            rivers_runtime::format_text(&report),
        );
    }

    /// Verifies that `cmd_init` creates the expected files.
    #[test]
    fn init_creates_expected_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("test-bundle");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "faker").expect("cmd_init should succeed");

        // Bundle manifest
        assert!(bundle_path.join("manifest.toml").exists(), "bundle manifest.toml should exist");
        // App directory named after the bundle
        let app_dir = bundle_path.join("test-bundle");
        assert!(app_dir.exists(), "app directory should exist");
        assert!(app_dir.join("manifest.toml").exists(), "app manifest.toml should exist");
        assert!(app_dir.join("resources.toml").exists(), "resources.toml should exist");
        assert!(app_dir.join("app.toml").exists(), "app.toml should exist");
        assert!(app_dir.join("schemas/item.schema.json").exists(), "schema json should exist");
    }

    /// Verifies that the `cmd_init` output cannot be created twice (idempotency guard).
    #[test]
    fn init_fails_if_dir_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("dup-app");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "faker").expect("first init should succeed");
        let result = cmd_init(bundle_str, "faker");
        assert!(result.is_err(), "second init into same dir should fail");
    }

    /// Verifies that `cmd_init` rejects unknown drivers.
    #[test]
    fn init_rejects_unknown_driver() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("bad-driver-app");
        let bundle_str = bundle_path.to_str().unwrap();
        let result = cmd_init(bundle_str, "cassandra");
        assert!(result.is_err(), "unknown driver should be rejected");
    }

    /// Verifies `cmd_pack` renames .zip output to .tar.gz with a warning.
    #[test]
    fn pack_zip_extension_is_corrected_to_tar_gz() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("pack-test");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "faker").expect("init should succeed");

        let out_zip = tmp.path().join("out.zip");
        let out_tar = tmp.path().join("out.tar.gz");

        let result = cmd_pack(bundle_str, out_zip.to_str().unwrap());
        assert!(result.is_ok(), "pack should succeed: {result:?}");
        assert!(out_tar.exists(), ".tar.gz artifact should exist");
        assert!(!out_zip.exists(), ".zip should not exist");
    }

    /// Verifies `cmd_pack` produces a .tar.gz when given the correct extension.
    #[test]
    fn pack_produces_tar_gz() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle_path = tmp.path().join("pack-tar-test");
        let bundle_str = bundle_path.to_str().unwrap();

        cmd_init(bundle_str, "faker").expect("init should succeed");

        let out = tmp.path().join("bundle.tar.gz");
        let result = cmd_pack(bundle_str, out.to_str().unwrap());
        assert!(result.is_ok(), "pack should succeed: {result:?}");
        assert!(out.exists(), ".tar.gz artifact should exist");
    }

    #[test]
    fn run_validate_returns_report_with_layers() {
        let bundle_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../address-book-bundle");
        if std::path::Path::new(bundle_dir).exists() {
            let report = run_validate(bundle_dir).expect("run_validate should succeed");
            assert_eq!(report.bundle_name, "address-book");
            assert_eq!(report.bundle_version, "1.0.0");
            // Should have at least the structural layer with results
            assert!(!report.layers["structural_toml"].results.is_empty());
        }
    }
}

fn cmd_pack(bundle_dir: &str, output: &str) -> Result<(), String> {
    // Validate first
    let report = run_validate(bundle_dir)?;
    if report.has_errors() {
        print!("{}", rivers_runtime::format_text(&report));
        return Err(format!(
            "{} validation error(s) — cannot pack",
            report.summary.total_failed
        ));
    }

    // Normalise output path: always produce .tar.gz regardless of what the
    // caller requested. If the caller passed a .zip extension (common mistake),
    // swap it out and warn rather than silently producing the wrong artifact.
    let tar_output = if output.ends_with(".zip") {
        let base = &output[..output.len() - 4];
        let corrected = format!("{base}.tar.gz");
        eprintln!(
            "warning: .zip output is not supported — producing {corrected} instead"
        );
        corrected
    } else if output.ends_with(".tar.gz") {
        output.to_string()
    } else {
        format!("{output}.tar.gz")
    };

    let status = std::process::Command::new("tar")
        .args(["-czf", &tar_output, "-C", bundle_dir, "."])
        .status()
        .map_err(|e| format!("tar command failed: {e}"))?;

    if !status.success() {
        return Err(format!("tar exited with status {status}"));
    }

    println!("Packed: {tar_output}");
    Ok(())
}
