//! `riversctl tls` subcommands.
//!
//! Per spec §6: gen, request, import, show, list, expire.
//! Port-based targeting: --port P identifies which server's certs to operate on.

use std::path::Path;
use rivers_runtime::rivers_core_config::config::ServerConfig;

/// Parsed `riversctl tls` subcommand.
#[derive(Debug)]
pub enum TlsCommand {
    Gen     { port: Option<u16> },
    Request { port: Option<u16> },
    Import  { cert: String, key: String, port: Option<u16> },
    Show    { port: Option<u16> },
    List,
    Expire  { port: Option<u16> },
}

/// Parse `riversctl tls <subcommand> [args]`.
pub fn parse_tls_args(args: &[&str]) -> Result<TlsCommand, String> {
    let sub = args.first().ok_or("Usage: riversctl tls <gen|request|import|show|list|expire>")?;
    let rest = &args[1..];

    match *sub {
        "gen"     => Ok(TlsCommand::Gen     { port: parse_port(rest)? }),
        "request" => Ok(TlsCommand::Request { port: parse_port(rest)? }),
        "show"    => Ok(TlsCommand::Show    { port: parse_port(rest)? }),
        "list"    => Ok(TlsCommand::List),
        "import"  => {
            if rest.len() < 2 {
                return Err("Usage: riversctl tls import <cert> <key> [--port P]".to_string());
            }
            let cert = rest[0].to_string();
            let key = rest[1].to_string();
            let port = parse_port(&rest[2..])?;
            Ok(TlsCommand::Import { cert, key, port })
        }
        "expire"  => {
            let has_yes = rest.contains(&"--yes");
            if !has_yes {
                return Err("riversctl tls expire requires --yes flag".to_string());
            }
            Ok(TlsCommand::Expire { port: parse_port(rest)? })
        }
        other => Err(format!("unknown tls subcommand: {other}")),
    }
}

fn parse_port(args: &[&str]) -> Result<Option<u16>, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--port" {
            let val = args.get(i + 1)
                .ok_or("--port requires a value")?;
            let port = val.parse::<u16>()
                .map_err(|_| format!("--port value must be a valid port number (1–65535), got '{val}'"))?;
            if port == 0 {
                return Err(format!("--port value must be a valid port number (1–65535), got '0'"));
            }
            return Ok(Some(port));
        }
        i += 1;
    }
    Ok(None)
}

/// Resolve cert/key paths for a given port from the server config.
///
/// Lookup: base.port → [base.tls]; admin_api.port → [base.admin_api.tls]; else error.
pub fn resolve_cert_paths(
    config: &ServerConfig,
    port: Option<u16>,
) -> Result<(String, String), String> {
    let target_port = port.unwrap_or(config.base.port);

    if target_port == config.base.port {
        let tls = config.base.tls.as_ref()
            .ok_or("no [base.tls] configured")?;
        let cert = tls.cert.as_deref()
            .ok_or("cert path not set in [base.tls] — use auto-gen paths instead")?;
        let key = tls.key.as_deref()
            .ok_or("key path not set in [base.tls]")?;
        return Ok((cert.to_string(), key.to_string()));
    }

    if let Some(admin_port) = config.base.admin_api.port {
        if target_port == admin_port {
            let tls = config.base.admin_api.tls.as_ref()
                .ok_or("no [base.admin_api.tls] configured")?;
            let cert = tls.server_cert.as_deref()
                .ok_or("server_cert not set in [base.admin_api.tls]")?;
            let key = tls.server_key.as_deref()
                .ok_or("server_key not set in [base.admin_api.tls]")?;
            return Ok((cert.to_string(), key.to_string()));
        }
    }

    Err(format!("no server configured on port {target_port}"))
}

/// Execute a tls command.
pub fn run_tls_cmd(cmd: TlsCommand, config: &ServerConfig) -> Result<(), String> {
    match cmd {
        TlsCommand::Show { port }              => cmd_show(config, port),
        TlsCommand::List                       => cmd_list(config),
        TlsCommand::Expire { port }            => cmd_expire(config, port),
        TlsCommand::Import { cert, key, port } => cmd_import(config, &cert, &key, port),
        TlsCommand::Gen { port }               => cmd_gen(config, port),
        TlsCommand::Request { port }           => cmd_request(config, port),
    }
}

fn cmd_show(config: &ServerConfig, port: Option<u16>) -> Result<(), String> {
    let (cert_path, _) = resolve_cert_paths(config, port)?;
    let info = rivers_runtime::rivers_core::tls::inspect_cert(&cert_path)?;

    println!("Subject:     {}", info.subject);
    if info.is_self_signed {
        println!("Issuer:      {} (self-signed)", info.issuer);
    } else {
        println!("Issuer:      {}", info.issuer);
    }
    println!("SANs:        {}", if info.sans.is_empty() { "(none)".to_string() } else { info.sans.join(", ") });
    println!("Valid:       {} → {}", info.not_before, info.not_after);
    println!("Expires:     {}", info.expiry_summary);
    println!("Fingerprint: {}", info.fingerprint);

    Ok(())
}

fn cert_expiry_summary(path: &str) -> String {
    rivers_runtime::rivers_core::tls::cert_expiry_summary(path)
}

fn cmd_list(config: &ServerConfig) -> Result<(), String> {
    let data_dir = config.data_dir.as_deref().unwrap_or("data");

    println!("Managed certificates:");

    if let Some(ref tls) = config.base.tls {
        if let Some(ref cert) = tls.cert {
            println!("  [main]  {} — {}", cert, cert_expiry_summary(cert));
        }
    }

    if let Some(ref admin_tls) = config.base.admin_api.tls {
        if let Some(ref cert) = admin_tls.server_cert {
            println!("  [admin] {} — {}", cert, cert_expiry_summary(cert));
        }
    }

    // Scan auto-gen files
    let tls_dir = Path::new(data_dir).join("tls");
    let tls_dir = tls_dir.to_string_lossy().to_string();
    if let Ok(entries) = std::fs::read_dir(&tls_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("auto-") && name.ends_with(".crt") {
                println!("  [auto]  {} — {}", entry.path().display(), cert_expiry_summary(&entry.path().to_string_lossy()));
            }
        }
    }

    Ok(())
}

fn cmd_expire(config: &ServerConfig, port: Option<u16>) -> Result<(), String> {
    // Try to resolve explicit cert paths first; fall back to auto-gen paths
    let (cert_path, key_path) = match resolve_cert_paths(config, port) {
        Ok(paths) => paths,
        Err(_) => {
            // auto-gen fallback: compute paths from data_dir + app_id
            let data_dir = config.data_dir.as_deref().unwrap_or("data");
            let app_id = config.app_id.as_deref().unwrap_or("default");
            let target_port = port.unwrap_or(config.base.port);
            let tls_base = Path::new(data_dir).join("tls");
            if target_port == config.base.port {
                (
                    tls_base.join(format!("auto-{app_id}.crt")).to_string_lossy().to_string(),
                    tls_base.join(format!("auto-{app_id}.key")).to_string_lossy().to_string(),
                )
            } else if config.base.admin_api.port == Some(target_port) {
                (
                    tls_base.join(format!("auto-admin-{app_id}.crt")).to_string_lossy().to_string(),
                    tls_base.join(format!("auto-admin-{app_id}.key")).to_string_lossy().to_string(),
                )
            } else {
                return Err(format!("no server configured on port {target_port}"));
            }
        }
    };

    if Path::new(&cert_path).exists() {
        std::fs::remove_file(&cert_path)
            .map_err(|e| format!("failed to remove {cert_path}: {e}"))?;
        println!("Removed: {cert_path}");
    }
    if Path::new(&key_path).exists() {
        std::fs::remove_file(&key_path)
            .map_err(|e| format!("failed to remove {key_path}: {e}"))?;
        println!("Removed: {key_path}");
    }

    println!("Certificate expired. Restart riversd to trigger re-generation.");
    Ok(())
}

fn cmd_import(config: &ServerConfig, cert: &str, key: &str, port: Option<u16>) -> Result<(), String> {
    let (dest_cert, dest_key) = resolve_cert_paths(config, port)?;

    rivers_runtime::rivers_core::tls::validate_cert_key_pair(cert, key)?;

    std::fs::copy(cert, &dest_cert)
        .map_err(|e| format!("failed to copy cert to {dest_cert}: {e}"))?;
    std::fs::copy(key, &dest_key)
        .map_err(|e| format!("failed to copy key to {dest_key}: {e}"))?;

    println!("Imported:");
    println!("  cert → {dest_cert}");
    println!("  key  → {dest_key}");
    println!("Restart riversd to apply.");
    Ok(())
}

fn cmd_gen(config: &ServerConfig, port: Option<u16>) -> Result<(), String> {
    let target_port = port.unwrap_or(config.base.port);
    let (cert_path, key_path) = resolve_cert_paths(config, port)?;

    let x509 = if target_port == config.base.port {
        config.base.tls.as_ref()
            .map(|t| t.x509.clone())
            .unwrap_or_default()
    } else {
        rivers_runtime::rivers_core_config::config::TlsX509Config::default()
    };

    rivers_runtime::rivers_core::tls::generate_self_signed_cert(&x509, &cert_path, &key_path)?;
    println!("Generated self-signed certificate:");
    println!("  cert → {cert_path}");
    println!("  key  → {key_path}");
    println!("WARN: self-signed cert — not for production");
    Ok(())
}

fn cmd_request(config: &ServerConfig, port: Option<u16>) -> Result<(), String> {
    let target_port = port.unwrap_or(config.base.port);
    let x509 = if target_port == config.base.port {
        config.base.tls.as_ref()
            .map(|t| t.x509.clone())
            .unwrap_or_default()
    } else {
        rivers_runtime::rivers_core_config::config::TlsX509Config::default()
    };

    let csr = rivers_runtime::rivers_core::tls::generate_csr(&x509)?;
    print!("{csr}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tls_show_default_port() {
        let args: &[&str] = &["show"];
        let cmd = parse_tls_args(args).unwrap();
        assert!(matches!(cmd, TlsCommand::Show { port: None }));
    }

    #[test]
    fn parse_tls_show_with_port() {
        let args: &[&str] = &["show", "--port", "9443"];
        let cmd = parse_tls_args(args).unwrap();
        assert!(matches!(cmd, TlsCommand::Show { port: Some(9443) }));
    }

    #[test]
    fn parse_tls_expire_requires_yes() {
        let args: &[&str] = &["expire"];
        let result = parse_tls_args(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("--yes"));
    }

    #[test]
    fn parse_tls_expire_with_yes() {
        let args: &[&str] = &["expire", "--yes"];
        let cmd = parse_tls_args(args).unwrap();
        assert!(matches!(cmd, TlsCommand::Expire { port: None }));
    }

    #[test]
    fn parse_tls_import_requires_cert_and_key() {
        let args: &[&str] = &["import", "/cert.crt"];
        let result = parse_tls_args(args);
        assert!(result.is_err());
    }

    #[test]
    fn parse_tls_import_with_cert_and_key() {
        let args: &[&str] = &["import", "/cert.crt", "/cert.key"];
        let cmd = parse_tls_args(args).unwrap();
        assert!(matches!(cmd, TlsCommand::Import { .. }));
    }
}
