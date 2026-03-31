//! Shared TLS certificate generation and validation.
//!
//! Used by riversd (startup auto-gen) and riversctl (tls subcommands).

use crate::config::TlsX509Config;
use std::path::Path;

/// Generate a self-signed certificate using rcgen 0.13, write PEM files to disk.
///
/// Uses x509 fields from `[base.tls.x509]` (or fixed admin defaults).
/// Creates parent directories if they don't exist.
pub fn generate_self_signed_cert(
    x509: &TlsX509Config,
    cert_path: &str,
    key_path: &str,
) -> Result<(), String> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, Ia5String, SanType};

    let mut params = CertificateParams::default();

    // Distinguished name
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, &x509.common_name);
    if let Some(ref org) = x509.organization {
        dn.push(DnType::OrganizationName, org);
    }
    if let Some(ref country) = x509.country {
        dn.push(DnType::CountryName, country);
    }
    if let Some(ref state) = x509.state {
        dn.push(DnType::StateOrProvinceName, state);
    }
    if let Some(ref locality) = x509.locality {
        dn.push(DnType::LocalityName, locality);
    }
    params.distinguished_name = dn;

    // SANs — IP addresses use SanType::IpAddress, DNS names use SanType::DnsName(Ia5String)
    let mut sans = Vec::new();
    for s in &x509.san {
        if let Ok(ip) = s.parse::<std::net::IpAddr>() {
            sans.push(SanType::IpAddress(ip));
        } else {
            let ia5 = Ia5String::try_from(s.clone())
                .map_err(|e| format!("invalid DNS SAN '{s}': {e}"))?;
            sans.push(SanType::DnsName(ia5));
        }
    }
    params.subject_alt_names = sans;

    // Validity — rcgen 0.13 uses ::time::OffsetDateTime
    let not_before = ::time::OffsetDateTime::now_utc();
    let not_after = not_before + ::time::Duration::days(x509.days as i64);
    params.not_before = not_before;
    params.not_after = not_after;

    // rcgen 0.13 generation API
    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| format!("key generation failed: {e}"))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("cert generation failed: {e}"))?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Ensure parent directory exists
    if let Some(parent) = Path::new(cert_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create cert dir {}: {e}", parent.display()))?;
    }

    std::fs::write(cert_path, cert_pem)
        .map_err(|e| format!("failed to write cert to {cert_path}: {e}"))?;
    std::fs::write(key_path, key_pem)
        .map_err(|e| format!("failed to write key to {key_path}: {e}"))?;

    // Restrict private key file permissions to owner-only (0o600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod key file {key_path}: {e}"))?;
    }

    Ok(())
}

/// Generate a CSR (Certificate Signing Request) from x509 config.
///
/// Returns the PEM-encoded CSR string.
pub fn generate_csr(x509: &TlsX509Config) -> Result<String, String> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, Ia5String, SanType};

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, &x509.common_name);
    if let Some(ref org) = x509.organization {
        dn.push(DnType::OrganizationName, org);
    }
    if let Some(ref country) = x509.country {
        dn.push(DnType::CountryName, country);
    }
    if let Some(ref state) = x509.state {
        dn.push(DnType::StateOrProvinceName, state);
    }
    if let Some(ref locality) = x509.locality {
        dn.push(DnType::LocalityName, locality);
    }
    params.distinguished_name = dn;

    let mut sans = Vec::new();
    for s in &x509.san {
        if let Ok(ip) = s.parse::<std::net::IpAddr>() {
            sans.push(SanType::IpAddress(ip));
        } else {
            let ia5 = Ia5String::try_from(s.clone())
                .map_err(|e| format!("invalid DNS SAN '{s}': {e}"))?;
            sans.push(SanType::DnsName(ia5));
        }
    }
    params.subject_alt_names = sans;

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| format!("key generation failed: {e}"))?;
    let csr = params.serialize_request(&key_pair)
        .map_err(|e| format!("CSR generation failed: {e}"))?
        .pem()
        .map_err(|e| format!("CSR PEM encoding failed: {e}"))?;

    Ok(csr)
}

/// Certificate info returned by `inspect_cert`.
pub struct CertInfo {
    /// Certificate subject distinguished name.
    pub subject: String,
    /// Certificate issuer distinguished name.
    pub issuer: String,
    /// Whether the certificate is self-signed (subject == issuer).
    pub is_self_signed: bool,
    /// Subject Alternative Names (DNS names and IP addresses).
    pub sans: Vec<String>,
    /// Validity start date (ISO 8601).
    pub not_before: String,
    /// Validity end date (ISO 8601).
    pub not_after: String,
    /// Human-readable expiry summary (e.g. "365 days left", "EXPIRED").
    pub expiry_summary: String,
    /// SHA-256 fingerprint of the DER-encoded certificate.
    pub fingerprint: String,
}

/// Parse a PEM cert file and return structured info.
pub fn inspect_cert(cert_path: &str) -> Result<CertInfo, String> {
    use sha2::{Digest, Sha256};
    use x509_parser::prelude::*;
    use std::fmt::Write as FmtWrite;

    let cert_pem = std::fs::read_to_string(cert_path)
        .map_err(|e| format!("cannot read {cert_path}: {e}"))?;

    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| format!("failed to parse PEM: {e:?}"))?;
    let (_, cert) = parse_x509_certificate(&pem.contents)
        .map_err(|e| format!("failed to parse X.509 cert: {e:?}"))?;

    let subject = cert.subject().to_string();
    let issuer = cert.issuer().to_string();
    let is_self_signed = cert.subject() == cert.issuer();

    let sans: Vec<String> = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value.general_names.iter().map(|n| format!("{n}")).collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let not_before = cert.validity().not_before.to_datetime();
    let not_after = cert.validity().not_after.to_datetime();

    let now = ::time::OffsetDateTime::now_utc();
    let expires_in = not_after - now;
    let expiry_summary = format_expiry(expires_in);

    let fingerprint_bytes = Sha256::digest(&pem.contents);
    let mut fingerprint = String::new();
    for (i, b) in fingerprint_bytes.iter().enumerate() {
        if i > 0 { let _ = write!(fingerprint, ":"); }
        let _ = write!(fingerprint, "{b:02X}");
    }

    Ok(CertInfo {
        subject,
        issuer,
        is_self_signed,
        sans,
        not_before: not_before.date().to_string(),
        not_after: not_after.date().to_string(),
        expiry_summary,
        fingerprint: format!("SHA256:{fingerprint}"),
    })
}

/// Compute a human-readable expiry summary for a PEM cert file.
///
/// Returns "365 days left", "48 hours left", "EXPIRED", or "(unreadable)".
pub fn cert_expiry_summary(path: &str) -> String {
    use x509_parser::prelude::*;

    let pem_str = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return "(unreadable)".to_string(),
    };
    let (_, pem) = match parse_x509_pem(pem_str.as_bytes()) {
        Ok(p) => p,
        Err(_) => return "(unreadable)".to_string(),
    };
    let (_, cert) = match parse_x509_certificate(&pem.contents) {
        Ok(c) => c,
        Err(_) => return "(unreadable)".to_string(),
    };

    let not_after = cert.validity().not_after.to_datetime();
    let now = ::time::OffsetDateTime::now_utc();
    format_expiry(not_after - now)
}

fn format_expiry(expires_in: ::time::Duration) -> String {
    if expires_in.whole_hours() < 0 {
        format!("EXPIRED {} days ago", (-expires_in.whole_days()))
    } else if expires_in.whole_hours() < 48 {
        format!("{} hours left", expires_in.whole_hours())
    } else {
        format!("{} days left", expires_in.whole_days())
    }
}

/// Validate that a cert file parses and the cert/key pair match.
pub fn validate_cert_key_pair(cert_path: &str, key_path: &str) -> Result<(), String> {
    let cert_bytes = std::fs::read(cert_path)
        .map_err(|e| format!("cannot read cert {cert_path}: {e}"))?;
    let key_bytes = std::fs::read(key_path)
        .map_err(|e| format!("cannot read key {key_path}: {e}"))?;

    let mut cert_reader = std::io::BufReader::new(cert_bytes.as_slice());
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to parse cert PEM: {e}"))?;

    if certs.is_empty() {
        return Err(format!("no certificates found in {cert_path}"));
    }

    let mut key_reader = std::io::BufReader::new(key_bytes.as_slice());
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| format!("failed to parse key PEM: {e}"))?
        .ok_or_else(|| format!("no private key found in {key_path}"))?;

    // Use rustls to verify the pair matches
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("cert/key pair mismatch: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn generate_self_signed_cert_creates_pem_files() {
        use crate::config::TlsX509Config;
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test.crt");
        let key_path = dir.path().join("test.key");

        super::generate_self_signed_cert(
            &TlsX509Config::default(),
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        )
        .unwrap();

        let bytes = std::fs::read(&cert_path).unwrap();
        assert!(bytes.starts_with(b"-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn validate_cert_key_pair_accepts_matching_pair() {
        use crate::config::TlsX509Config;
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test.crt");
        let key_path = dir.path().join("test.key");

        super::generate_self_signed_cert(
            &TlsX509Config::default(),
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        )
        .unwrap();

        super::validate_cert_key_pair(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        )
        .unwrap();
    }
}
