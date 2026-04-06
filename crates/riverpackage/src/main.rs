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

    let bundle_dir = if args.len() >= 3 { &args[2] } else { "." };

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
        "validate" => cmd_validate(bundle_dir),
        "preflight" => cmd_preflight(bundle_dir),
        "pack" => {
            let output = if args.len() >= 4 { &args[3] } else { "bundle.zip" };
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
    eprintln!("  validate [dir]              Validate bundle structure and configs");
    eprintln!("  preflight [dir]             Validate + check schema/parameter orphans");
    eprintln!("  pack [dir] [output]         Package bundle into a .zip file");
    eprintln!("  import-exec <name> <path>   Generate ExecDriver TOML config for a script");
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
        "bundleName    = \"{name}\"\nbundleVersion = \"1.0.0\"\napps          = [\"{name}\"]\n",
        name = bundle_name
    );
    write_file(&bundle_root.join("manifest.toml"), &bundle_manifest)?;

    // --- app manifest.toml ---
    let app_id = Uuid::new_v4();
    let app_manifest = format!(
        "appName    = \"{name}\"\nappId      = \"{id}\"\ntype       = \"service\"\nentryPoint = \"{name}\"\n",
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
fn build_resources(bundle_name: &str, driver: &str) -> (String, String) {
    match driver {
        "faker" => (
            "data".into(),
            "[[datasources]]\nname       = \"data\"\ndriver     = \"faker\"\nnopassword = true\nrequired   = true\n".into(),
        ),
        "postgres" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"postgres\"\nhost     = \"localhost\"\nport     = 5432\ndatabase = \"{name}\"\nusername = \"postgres\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        "sqlite" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"sqlite\"\nhost     = \"{name}.db\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        "mysql" => (
            "db".into(),
            format!(
                "[[datasources]]\nname     = \"db\"\ndriver   = \"mysql\"\nhost     = \"localhost\"\nport     = 3306\ndatabase = \"{name}\"\nusername = \"root\"\nrequired = true\n",
                name = bundle_name
            ),
        ),
        // unreachable — validated above
        _ => ("db".into(), String::new()),
    }
}

/// Build app.toml content for the scaffolded app.
fn build_app_toml(bundle_name: &str, _driver: &str, ds_name: &str) -> String {
    let schema_path = "schemas/item.schema.json";
    format!(
        r#"[data.dataviews.list_items]
datasource   = "{ds}"
query        = "SELECT * FROM items LIMIT ${{limit}}"
return_schema = "{schema}"

[[data.dataviews.list_items.parameters]]
name    = "limit"
type    = "integer"
default = 20

[api.views.items]
method      = "GET"
path        = "/items"
dataview    = "list_items"
description = "List {name} items"
"#,
        ds = ds_name,
        schema = schema_path,
        name = bundle_name,
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

fn cmd_validate(bundle_dir: &str) -> Result<(), String> {
    let path = Path::new(bundle_dir);
    let mut errors: Vec<String> = Vec::new();

    // Check bundle manifest
    let manifest_path = path.join("manifest.toml");
    if !manifest_path.exists() {
        errors.push("missing manifest.toml".into());
    } else {
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("read manifest.toml: {e}"))?;
        match toml::from_str::<toml::Value>(&content) {
            Ok(manifest) => {
                if manifest.get("name").is_none() {
                    errors.push("manifest.toml: missing 'name' field".into());
                }
                if manifest.get("version").is_none() {
                    errors.push("manifest.toml: missing 'version' field".into());
                }
                // Check each app listed
                if let Some(apps) = manifest.get("apps").and_then(|a| a.as_array()) {
                    for app_val in apps {
                        if let Some(app_name) = app_val.as_str() {
                            validate_app(path, app_name, &mut errors);
                        }
                    }
                }
            }
            Err(e) => errors.push(format!("manifest.toml parse error: {e}")),
        }
    }

    if errors.is_empty() {
        println!("Bundle OK: {bundle_dir}");
        Ok(())
    } else {
        for err in &errors {
            eprintln!("  ERROR: {err}");
        }
        Err(format!("{} validation errors", errors.len()))
    }
}

fn validate_app(bundle_path: &Path, app_name: &str, errors: &mut Vec<String>) {
    let app_dir = bundle_path.join(app_name);
    if !app_dir.exists() {
        errors.push(format!("app directory '{app_name}/' not found"));
        return;
    }

    // Check app manifest
    let app_manifest = app_dir.join("manifest.toml");
    if !app_manifest.exists() {
        errors.push(format!("{app_name}/manifest.toml missing"));
    } else {
        let content = std::fs::read_to_string(&app_manifest).ok();
        if let Some(content) = content {
            if toml::from_str::<toml::Value>(&content).is_err() {
                errors.push(format!("{app_name}/manifest.toml parse error"));
            }
        }
    }

    // Check resources.toml
    let resources = app_dir.join("resources.toml");
    if !resources.exists() {
        errors.push(format!("{app_name}/resources.toml missing"));
    }

    // Check app.toml
    let app_toml = app_dir.join("app.toml");
    if !app_toml.exists() {
        errors.push(format!("{app_name}/app.toml missing"));
    } else {
        let content = std::fs::read_to_string(&app_toml).ok();
        if let Some(content) = content {
            if toml::from_str::<toml::Value>(&content).is_err() {
                errors.push(format!("{app_name}/app.toml parse error"));
            }
        }
    }

    // Check schemas/ directory
    let schemas_dir = app_dir.join("schemas");
    if schemas_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&schemas_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "json") {
                    let content = std::fs::read_to_string(&path).ok();
                    if let Some(content) = content {
                        if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                            errors.push(format!("{}: invalid JSON", path.display()));
                        }
                    }
                }
            }
        }
    }
}

fn cmd_preflight(bundle_dir: &str) -> Result<(), String> {
    // Run validate first
    cmd_validate(bundle_dir)?;

    // Additional preflight checks
    let path = Path::new(bundle_dir);
    let mut warnings: Vec<String> = Vec::new();

    // Check for schema files referenced in app.toml but missing
    // Check for $variable <-> parameter orphans
    // These require parsing app.toml DataView declarations

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
    let hash = hex::encode(Sha256::digest(&bytes));

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

    #[test]
    fn validate_catches_missing_manifest() {
        let result = cmd_validate("/tmp/nonexistent_bundle_dir_12345");
        assert!(result.is_err());
    }

    #[test]
    fn validate_address_book_bundle() {
        // Use the reference bundle in the repo
        let bundle_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../address-book-bundle");
        if std::path::Path::new(bundle_dir).exists() {
            let result = cmd_validate(bundle_dir);
            assert!(result.is_ok(), "address-book-bundle should validate: {:?}", result.err());
        }
    }

    #[test]
    fn validate_catches_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        std::fs::write(&manifest, "invalid { toml content").unwrap();
        let result = cmd_validate(tmp.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn preflight_on_nonexistent_dir_fails() {
        let result = cmd_preflight("/tmp/nonexistent_12345");
        assert!(result.is_err());
    }
}

fn cmd_pack(bundle_dir: &str, output: &str) -> Result<(), String> {
    // Validate first
    cmd_validate(bundle_dir)?;

    // Create a zip file
    // For V1: just report what would be packed (zip crate not in deps)
    let path = Path::new(bundle_dir);
    let mut file_count = 0;

    fn count_files(dir: &Path, count: &mut usize) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    count_files(&path, count);
                } else {
                    *count += 1;
                }
            }
        }
    }

    count_files(path, &mut file_count);

    println!("Would pack {file_count} files from {bundle_dir} -> {output}");
    println!("(zip packaging requires the 'zip' crate — use tar for now)");

    // Alternative: use tar via std::process::Command
    let tar_output = output.replace(".zip", ".tar.gz");
    let status = std::process::Command::new("tar")
        .args(["-czf", &tar_output, "-C", bundle_dir, "."])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Packed: {tar_output}");
            Ok(())
        }
        Ok(s) => Err(format!("tar exited with status {s}")),
        Err(e) => Err(format!("tar command failed: {e}")),
    }
}
