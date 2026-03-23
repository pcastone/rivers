//! TLS validation, acceptor construction, and auto-gen orchestration for riversd.
//!
//! Per spec `docs/superpowers/specs/2026-03-18-rivers-httpd-tls-design.md`.
//! Cert generation and pair validation are in rivers_runtime::rivers_core::tls.

use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use rivers_runtime::rivers_core::config::{AdminTlsConfig, TlsConfig, TlsX509Config};

/// Validate `[base.tls]` section.
///
/// Rules:
/// - None (absent) → error: TLS is required
/// - cert set without key (or vice versa) → error
/// - cert + key both absent → OK (auto-gen path)
/// - cert + key both set → OK (provided cert path)
pub fn validate_tls_config(tls: &Option<TlsConfig>) -> Result<(), String> {
    let tls = tls.as_ref().ok_or_else(|| {
        "TLS is required: add [base.tls] to your config (cert/key optional — omit for auto-gen)".to_string()
    })?;

    match (&tls.cert, &tls.key) {
        (Some(_), None) | (None, Some(_)) => {
            Err("both cert and key must be set in [base.tls], or omit both for auto-gen".to_string())
        }
        _ => Ok(()),
    }
}

/// Validate `[base.admin_api.tls]` section.
///
/// Rules:
/// - None (absent) → error: admin TLS is required
/// - server_cert set without server_key (or vice versa) → error
/// - require_client_cert = true + ca_cert absent → error
pub fn validate_admin_tls_config(tls: &Option<AdminTlsConfig>) -> Result<(), String> {
    let tls = tls.as_ref().ok_or_else(|| {
        "admin TLS is required: add [base.admin_api.tls] to your config".to_string()
    })?;

    match (&tls.server_cert, &tls.server_key) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "both server_cert and server_key must be set in [base.admin_api.tls], or omit both for auto-gen"
                    .to_string(),
            );
        }
        _ => {}
    }

    if tls.require_client_cert && tls.ca_cert.is_none() {
        return Err(
            "ca_cert required in [base.admin_api.tls] when require_client_cert = true".to_string(),
        );
    }

    Ok(())
}

/// Validate that redirect_port does not conflict with base_port.
pub fn validate_redirect_port(base_port: u16, redirect_port: u16) -> Result<(), String> {
    if redirect_port == base_port {
        Err(format!(
            "redirect_port ({redirect_port}) cannot equal base.port ({base_port})"
        ))
    } else {
        Ok(())
    }
}

/// Build a TlsAcceptor from PEM cert + key files.
///
/// When `http2` is true, ALPN is configured to advertise both `h2` and `http/1.1`.
/// `min_version` controls the minimum TLS version: `"tls13"` for TLS 1.3 only,
/// otherwise TLS 1.2+ (rustls default).
pub fn load_tls_acceptor(
    cert_path: &str,
    key_path: &str,
    http2: bool,
    min_version: &str,
) -> Result<TlsAcceptor, String> {
    let cert_bytes = std::fs::read(cert_path)
        .map_err(|e| format!("cannot read cert {cert_path}: {e}"))?;
    let key_bytes = std::fs::read(key_path)
        .map_err(|e| format!("cannot read key {key_path}: {e}"))?;

    let mut cert_reader = std::io::BufReader::new(cert_bytes.as_slice());
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to parse cert PEM: {e}"))?;

    let mut key_reader = std::io::BufReader::new(key_bytes.as_slice());
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| format!("failed to parse key PEM: {e}"))?
        .ok_or_else(|| format!("no private key found in {key_path}"))?;

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Configure minimum TLS version
    let versions: Vec<&'static rustls::SupportedProtocolVersion> = match min_version {
        "tls13" => vec![&rustls::version::TLS13],
        _ => vec![&rustls::version::TLS12, &rustls::version::TLS13],
    };

    let mut tls_config = rustls::ServerConfig::builder_with_protocol_versions(&versions)
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS config error: {e}"))?;

    if http2 {
        tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    } else {
        tls_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    }

    Ok(TlsAcceptor::from(Arc::new(tls_config)))
}

/// Auto-gen or reuse a self-signed cert for the main server.
///
/// Called at startup when `[base.tls]` has no cert/key paths.
/// Writes to `{data_dir}/tls/auto-{app_id}.crt` and `.key`.
/// On restart: validates existing pair; re-generates if invalid.
///
/// Returns (cert_path, key_path) of the cert to use.
pub fn maybe_autogen_tls_cert(
    x509: &TlsX509Config,
    data_dir: &str,
    app_id: &str,
) -> Result<(String, String), String> {
    let cert_path = format!("{data_dir}/tls/auto-{app_id}.crt");
    let key_path = format!("{data_dir}/tls/auto-{app_id}.key");

    if Path::new(&cert_path).exists() && Path::new(&key_path).exists() {
        match rivers_runtime::rivers_core::tls::validate_cert_key_pair(&cert_path, &key_path) {
            Ok(()) => {
                tracing::info!("TLS: reusing existing auto-generated cert");
                return Ok((cert_path, key_path));
            }
            Err(e) => {
                tracing::warn!("TLS: existing auto-gen cert invalid ({}), regenerating", e);
            }
        }
    } else if Path::new(&cert_path).exists() || Path::new(&key_path).exists() {
        tracing::warn!(
            "TLS: only one of cert/key found in {}/tls/; regenerating pair",
            data_dir
        );
    }

    rivers_runtime::rivers_core::tls::generate_self_signed_cert(x509, &cert_path, &key_path)?;
    tracing::warn!(
        cert = %cert_path,
        "TLS: using auto-generated self-signed cert — not for production"
    );
    Ok((cert_path, key_path))
}

/// Auto-gen or reuse a self-signed cert for the admin server.
///
/// Uses fixed defaults: CN=rivers-admin-{app_id}, SAN=localhost/127.0.0.1, days=365.
/// Writes to `{data_dir}/tls/auto-admin-{app_id}.crt` and `.key`.
pub fn maybe_autogen_admin_tls_cert(
    data_dir: &str,
    app_id: &str,
) -> Result<(String, String), String> {
    let cert_path = format!("{data_dir}/tls/auto-admin-{app_id}.crt");
    let key_path = format!("{data_dir}/tls/auto-admin-{app_id}.key");

    if Path::new(&cert_path).exists() && Path::new(&key_path).exists() {
        match rivers_runtime::rivers_core::tls::validate_cert_key_pair(&cert_path, &key_path) {
            Ok(()) => {
                tracing::info!("admin TLS: reusing existing auto-generated cert");
                return Ok((cert_path, key_path));
            }
            Err(e) => {
                tracing::warn!("admin TLS: existing cert invalid ({}), regenerating", e);
            }
        }
    } else if Path::new(&cert_path).exists() || Path::new(&key_path).exists() {
        tracing::warn!(
            "TLS: only one of cert/key found in {}/tls/; regenerating pair",
            data_dir
        );
    }

    let x509 = TlsX509Config {
        common_name: format!("rivers-admin-{app_id}"),
        san: vec!["localhost".to_string(), "127.0.0.1".to_string()],
        days: 365,
        ..Default::default()
    };
    rivers_runtime::rivers_core::tls::generate_self_signed_cert(&x509, &cert_path, &key_path)?;
    tracing::warn!(
        cert = %cert_path,
        "admin TLS: using auto-generated self-signed cert — not for production"
    );
    Ok((cert_path, key_path))
}

#[cfg(test)]
mod tests {
    use rivers_runtime::rivers_core::config::{TlsConfig, TlsX509Config, AdminTlsConfig};

    #[test]
    fn validate_tls_config_rejects_absent_tls_section() {
        let result = super::validate_tls_config(&None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("TLS is required"));
    }

    #[test]
    fn validate_tls_config_rejects_cert_without_key() {
        let mut tls = TlsConfig::default();
        tls.cert = Some("/etc/cert.crt".to_string());
        tls.key = None;
        let result = super::validate_tls_config(&Some(tls));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("both cert and key must be set"));
    }

    #[test]
    fn validate_tls_config_rejects_key_without_cert() {
        let mut tls = TlsConfig::default();
        tls.cert = None;
        tls.key = Some("/etc/cert.key".to_string());
        let result = super::validate_tls_config(&Some(tls));
        assert!(result.is_err());
    }

    #[test]
    fn validate_tls_config_accepts_both_paths_set() {
        let mut tls = TlsConfig::default();
        tls.cert = Some("/etc/cert.crt".to_string());
        tls.key = Some("/etc/cert.key".to_string());
        let result = super::validate_tls_config(&Some(tls));
        assert!(result.is_ok());
    }

    #[test]
    fn validate_tls_config_accepts_both_paths_absent_autogen() {
        let tls = TlsConfig::default(); // cert=None, key=None → auto-gen path
        let result = super::validate_tls_config(&Some(tls));
        assert!(result.is_ok());
    }

    #[test]
    fn validate_admin_tls_rejects_absent_section() {
        let result = super::validate_admin_tls_config(&None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("admin TLS is required"));
    }

    #[test]
    fn validate_admin_tls_rejects_mtls_without_ca_cert() {
        let tls = AdminTlsConfig {
            ca_cert: None,
            server_cert: None,
            server_key: None,
            require_client_cert: true,
        };
        let result = super::validate_admin_tls_config(&Some(tls));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ca_cert required"));
    }

    #[test]
    fn validate_redirect_port_rejects_conflict_with_base_port() {
        let result = super::validate_redirect_port(443, 443);
        assert!(result.is_err());
    }

    #[test]
    fn validate_redirect_port_accepts_different_ports() {
        let result = super::validate_redirect_port(443, 80);
        assert!(result.is_ok());
    }

    #[test]
    fn autogen_main_cert_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_str().unwrap();

        let (cert_path, key_path) = super::maybe_autogen_tls_cert(
            &TlsX509Config::default(),
            data_dir,
            "test-app",
        ).unwrap();

        assert!(std::path::Path::new(&cert_path).exists());
        assert!(std::path::Path::new(&key_path).exists());
    }
}
