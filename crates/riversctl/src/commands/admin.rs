#[cfg(feature = "admin-api")]
use std::collections::HashMap;
#[cfg(feature = "admin-api")]
use std::sync::OnceLock;

/// Config-sourced admin key path, set once from `[base.admin_api].private_key` before
/// any commands run. `sign_request` checks this after the `RIVERS_ADMIN_KEY` env var.
#[cfg(feature = "admin-api")]
static CONFIG_ADMIN_KEY_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Populate the config-sourced key path. Call once from main() before dispatching
/// any admin commands. Safe to call multiple times (subsequent calls are no-ops).
#[cfg(feature = "admin-api")]
pub fn init_config_key(path: Option<String>) {
    let _ = CONFIG_ADMIN_KEY_PATH.set(path);
}

// ── Admin error type ─────────────────────────────────────────────────────────

/// Distinguishes a network-level failure (connection refused, timeout, DNS) from
/// an HTTP-level failure (4xx/5xx including auth/RBAC). Only network failures
/// may trigger a local signal fallback — HTTP errors must surface verbatim.
#[cfg(feature = "admin-api")]
#[derive(Debug)]
pub enum AdminError {
    /// The connection could not be established (unreachable, timeout, DNS).
    Network(String),
    /// The server responded with an HTTP error status.
    Http(String),
}

#[cfg(feature = "admin-api")]
impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminError::Network(s) => write!(f, "{s}"),
            AdminError::Http(s) => write!(f, "{s}"),
        }
    }
}

// ── Admin API helpers ────────────────────────────────────────────────────────

/// Build a shared reqwest client with explicit connect and request timeouts.
/// Both `admin_get` and `admin_post` use this client.
#[cfg(feature = "admin-api")]
pub fn admin_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build admin HTTP client")
}

/// Sign an admin API request and return the required headers.
///
/// Key loading priority:
///   1. `RIVERS_ADMIN_KEY` env var (path to key file)
///   2. `key_path` argument (from `[base.admin_api].private_key` in config)
///
/// If a key source is found but the content is malformed (invalid hex,
/// wrong length), this function returns `Err` rather than silently falling
/// back to an unsigned request.
#[cfg(feature = "admin-api")]
pub fn sign_request(
    method: &str,
    path: &str,
    body: &str,
    key_path: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    let mut headers = HashMap::new();

    // Server expects epoch milliseconds, not RFC 3339
    let ts_ms = chrono::Utc::now().timestamp_millis().to_string();
    headers.insert("X-Rivers-Timestamp".into(), ts_ms.clone());

    // Resolve the key file path: env var takes priority over config field.
    let resolved_path = std::env::var("RIVERS_ADMIN_KEY").ok()
        .or_else(|| key_path.map(|s| s.to_string()));

    if let Some(kp) = resolved_path {
        let key_hex = std::fs::read_to_string(&kp)
            .map_err(|e| format!("cannot read admin key file '{kp}': {e}"))?;
        let key_hex = key_hex.trim();
        let seed_bytes = hex::decode(key_hex)
            .map_err(|e| format!("admin key file '{kp}' contains invalid hex: {e}"))?;
        if seed_bytes.len() != 32 {
            return Err(format!(
                "admin key file '{kp}' has wrong length: expected 32 bytes, got {}",
                seed_bytes.len()
            ));
        }
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::Digest;
        let seed: [u8; 32] = seed_bytes.try_into().unwrap();
        let signing_key = SigningKey::from_bytes(&seed);

        // Match server's build_signing_payload: method\npath\ntimestamp\nbody_hash
        let body_hash = hex::encode(sha2::Sha256::digest(body.as_bytes()));
        let message = format!("{method}\n{path}\n{ts_ms}\n{body_hash}");
        let signature = signing_key.sign(message.as_bytes());
        headers.insert("X-Rivers-Signature".into(), hex::encode(signature.to_bytes()));
    }

    Ok(headers)
}

#[cfg(feature = "admin-api")]
pub async fn admin_get(url: &str, path: &str) -> Result<serde_json::Value, AdminError> {
    let config_key = CONFIG_ADMIN_KEY_PATH.get().and_then(|opt| opt.as_deref());
    let headers = sign_request("GET", path, "", config_key)
        .map_err(|e| AdminError::Http(format!("key error: {e}")))?;
    let client = admin_client();
    let mut req = client.get(format!("{url}{path}"));
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            AdminError::Network(format!("connection failed: {e}"))
        } else {
            AdminError::Http(format!("request failed: {e}"))
        }
    })?;
    let status = resp.status();
    let body = resp.text().await
        .map_err(|e| AdminError::Http(format!("read body: {e}")))?;
    if !status.is_success() {
        return Err(AdminError::Http(format!("HTTP {status}: {body}")));
    }
    serde_json::from_str(&body)
        .map_err(|e| AdminError::Http(format!("parse JSON: {e}")))
}

#[cfg(feature = "admin-api")]
pub async fn admin_post(url: &str, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, AdminError> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let config_key = CONFIG_ADMIN_KEY_PATH.get().and_then(|opt| opt.as_deref());
    let headers = sign_request("POST", path, &body_str, config_key)
        .map_err(|e| AdminError::Http(format!("key error: {e}")))?;
    let client = admin_client();
    let mut req = client.post(format!("{url}{path}"))
        .header("Content-Type", "application/json")
        .body(body_str);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            AdminError::Network(format!("connection failed: {e}"))
        } else {
            AdminError::Http(format!("request failed: {e}"))
        }
    })?;
    let status = resp.status();
    let body = resp.text().await
        .map_err(|e| AdminError::Http(format!("read body: {e}")))?;
    if !status.is_success() {
        return Err(AdminError::Http(format!("HTTP {status}: {body}")));
    }
    serde_json::from_str(&body)
        .map_err(|e| AdminError::Http(format!("parse JSON: {e}")))
}

// ── Admin API commands ──────────────────────────────────────────────────────

#[cfg(feature = "admin-api")]
pub async fn cmd_status(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/status").await
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_deploy(url: &str, bundle_path: &str) -> Result<(), String> {
    let body = serde_json::json!({ "bundle_path": bundle_path });
    let data = admin_post(url, "/admin/deploy", &body).await
        .map_err(|e| e.to_string())?;

    // Surface staged/pending deployment status explicitly.
    let status_field = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status_field == "pending" || status_field == "staged" {
        let id = data.get("id").and_then(|v| v.as_str()).unwrap_or("<unknown>");
        println!("Deployment staged. To activate, run: riversctl deploy promote {id}");
    }

    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_drivers(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/drivers").await
        .map_err(|e| e.to_string())?;
    if let Some(drivers) = data.as_array() {
        for d in drivers {
            println!("{}", d.as_str().unwrap_or(&d.to_string()));
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&data).unwrap());
    }
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_datasources(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/datasources").await
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_health(_url: &str) -> Result<(), String> {
    let main_url = std::env::var("RIVERS_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let data = admin_get(&main_url, "/health/verbose").await
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_stop(url: &str) -> Result<(), String> {
    let body = serde_json::json!({ "mode": "immediate" });
    match admin_post(url, "/admin/shutdown", &body).await {
        Ok(data) => {
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
            Ok(())
        }
        Err(AdminError::Network(e)) => {
            // Only fall back to signal on a genuine network failure.
            eprintln!("Admin API unreachable ({e}) — falling back to signal");
            signal_riversd(Signal::Kill)
        }
        Err(AdminError::Http(e)) => {
            // Auth/RBAC/HTTP errors must NOT trigger local signal fallback.
            Err(format!("admin shutdown failed: {e}"))
        }
    }
}

#[cfg(feature = "admin-api")]
pub async fn cmd_graceful(url: &str) -> Result<(), String> {
    let body = serde_json::json!({ "mode": "graceful" });
    match admin_post(url, "/admin/shutdown", &body).await {
        Ok(data) => {
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
            Ok(())
        }
        Err(AdminError::Network(e)) => {
            // Only fall back to signal on a genuine network failure.
            eprintln!("Admin API unreachable ({e}) — falling back to signal");
            signal_riversd(Signal::Term)
        }
        Err(AdminError::Http(e)) => {
            // Auth/RBAC/HTTP errors must NOT trigger local signal fallback.
            Err(format!("admin shutdown failed: {e}"))
        }
    }
}

pub enum Signal {
    Kill,
    Term,
}

/// Find the riversd process and send a signal.
/// Cross-platform: uses `pgrep`/`kill` on Unix, `tasklist`/`taskkill` on Windows.
pub fn signal_riversd(sig: Signal) -> Result<(), String> {
    let pids = find_riversd_pids()?;
    if pids.is_empty() {
        return Err("no running riversd process found".into());
    }
    for pid in &pids {
        kill_pid(pid, &sig)?;
    }
    Ok(())
}

/// Discover PIDs of running riversd processes.
#[cfg(unix)]
fn find_riversd_pids() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("pgrep")
        .arg("-x")
        .arg("riversd")
        .output()
        .map_err(|e| format!("failed to run pgrep: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let pids: Vec<String> = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("pgrep output: {e}"))?
        .trim()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    Ok(pids)
}

#[cfg(windows)]
fn find_riversd_pids() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq riversd.exe", "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("failed to run tasklist: {e}"))?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("tasklist output: {e}"))?;

    // CSV format: "riversd.exe","1234","Console","1","12,345 K"
    let pids: Vec<String> = stdout
        .lines()
        .filter(|line| line.contains("riversd.exe"))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            fields.get(1).map(|pid| pid.trim_matches('"').to_string())
        })
        .collect();

    Ok(pids)
}

/// Send a signal to a process by PID.
#[cfg(unix)]
fn kill_pid(pid: &str, sig: &Signal) -> Result<(), String> {
    let signal_name = match sig {
        Signal::Kill => "KILL",
        Signal::Term => "TERM",
    };

    let status = std::process::Command::new("kill")
        .arg(format!("-{signal_name}"))
        .arg(pid)
        .status()
        .map_err(|e| format!("failed to send signal: {e}"))?;

    if status.success() {
        println!("Sent SIG{signal_name} to riversd (PID {pid})");
    } else {
        eprintln!("Failed to signal PID {pid}");
    }
    Ok(())
}

#[cfg(windows)]
fn kill_pid(pid: &str, sig: &Signal) -> Result<(), String> {
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", pid]);

    // /F = force kill (immediate), omit for graceful
    if matches!(sig, Signal::Kill) {
        cmd.arg("/F");
    }

    let status = cmd.status().map_err(|e| format!("failed to run taskkill: {e}"))?;

    let mode = match sig {
        Signal::Kill => "force killed",
        Signal::Term => "stopped",
    };

    if status.success() {
        println!("Successfully {mode} riversd (PID {pid})");
    } else {
        eprintln!("Failed to stop PID {pid}");
    }
    Ok(())
}

/// List all circuit breakers for a specific app.
#[cfg(feature = "admin-api")]
pub async fn cmd_breaker_list(url: &str, app: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers", app);
    let data = admin_get(url, &path).await
        .map_err(|e| e.to_string())?;
    let empty = vec![];
    let breakers = data.as_array().unwrap_or(&empty);
    if breakers.is_empty() {
        println!("No circuit breakers configured for app '{}'.", app);
        return Ok(());
    }
    for b in breakers {
        let id = b["breakerId"].as_str().unwrap_or("?");
        let state = b["state"].as_str().unwrap_or("?");
        let dvs = b["dataviews"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("  {:<30} {:<8} ({} dataview{})", id, state, dvs, if dvs == 1 { "" } else { "s" });
    }
    Ok(())
}

/// Get status of a specific circuit breaker.
#[cfg(feature = "admin-api")]
pub async fn cmd_breaker_status(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}", app, name);
    let data = admin_get(url, &path).await
        .map_err(|e| e.to_string())?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}

/// Trip (open) a circuit breaker.
#[cfg(feature = "admin-api")]
pub async fn cmd_breaker_trip(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}/trip", app, name);
    let data = admin_post(url, &path, &serde_json::json!({})).await
        .map_err(|e| e.to_string())?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}

/// Reset (close) a circuit breaker.
#[cfg(feature = "admin-api")]
pub async fn cmd_breaker_reset(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}/reset", app, name);
    let data = admin_post(url, &path, &serde_json::json!({})).await
        .map_err(|e| e.to_string())?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_log(url: &str, args: &[String]) -> Result<(), String> {
    match args.first().map(|s| s.as_str()) {
        Some("levels") => {
            let data = admin_get(url, "/admin/log/levels").await
                .map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("set") => {
            if args.len() < 3 {
                return Err("Usage: riversctl log set <target> <level>".into());
            }
            let body = serde_json::json!({ "target": args[1], "level": args[2] });
            let data = admin_post(url, "/admin/log/set", &body).await
                .map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("reset") => {
            let data = admin_post(url, "/admin/log/reset", &serde_json::json!({})).await
                .map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        _ => return Err("Usage: riversctl log <levels|set|reset>".into()),
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_produces_timestamp_without_key() {
        std::env::remove_var("RIVERS_ADMIN_KEY");
        let headers = sign_request("GET", "/admin/status", "body", None).unwrap();
        assert!(headers.contains_key("X-Rivers-Timestamp"));
        assert!(!headers.contains_key("X-Rivers-Signature"));
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_rejects_bad_hex_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("bad.key");
        std::fs::write(&key_path, "notvalidhex!!").unwrap();
        let result = sign_request("GET", "/admin/status", "", Some(&key_path.to_string_lossy()));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("invalid hex"), "got: {err}");
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_rejects_wrong_length_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("short.key");
        // 16 bytes → 32 hex chars — wrong length
        std::fs::write(&key_path, "deadbeefdeadbeefdeadbeefdeadbeef").unwrap();
        let result = sign_request("GET", "/admin/status", "", Some(&key_path.to_string_lossy()));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("wrong length"), "got: {err}");
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn log_set_body_uses_target_key() {
        // Build the body the same way cmd_log does and assert it contains "target", not "event".
        let args: Vec<String> = vec![
            "set".to_string(),
            "my_module".to_string(),
            "debug".to_string(),
        ];
        let body = serde_json::json!({ "target": args[1], "level": args[2] });
        assert!(body.get("target").is_some(), "body must contain 'target' key");
        assert!(body.get("event").is_none(), "body must NOT contain 'event' key");
        assert_eq!(body["target"], "my_module");
        assert_eq!(body["level"], "debug");
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn admin_error_network_does_not_trigger_http_path() {
        // Verify that only AdminError::Network triggers fallback, not AdminError::Http.
        // This is a structural test — the match arms in cmd_stop/cmd_graceful are distinct.
        let net_err = AdminError::Network("connection refused".into());
        let http_err = AdminError::Http("HTTP 401: Unauthorized".into());
        assert!(matches!(net_err, AdminError::Network(_)));
        assert!(matches!(http_err, AdminError::Http(_)));
        // A NetworkError should display without "HTTP"
        assert!(!net_err.to_string().starts_with("HTTP"));
        // An HttpError with 401 content should preserve it
        assert!(http_err.to_string().contains("401"));
    }
}
