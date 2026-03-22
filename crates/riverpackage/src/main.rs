//! riverpackage — Rivers bundle validator and packager.
//!
//! Commands: validate, preflight, pack

use std::path::Path;

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
        "validate" => cmd_validate(bundle_dir),
        "preflight" => cmd_preflight(bundle_dir),
        "pack" => {
            let output = if args.len() >= 4 { &args[3] } else { "bundle.zip" };
            cmd_pack(bundle_dir, output)
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
    eprintln!("  validate [dir]         Validate bundle structure and configs");
    eprintln!("  preflight [dir]        Validate + check schema/parameter orphans");
    eprintln!("  pack [dir] [output]    Package bundle into a .zip file");
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
