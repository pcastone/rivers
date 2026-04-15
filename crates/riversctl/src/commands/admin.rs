#[cfg(feature = "admin-api")]
use std::collections::HashMap;

// ── Admin API helpers ────────────────────────────────────────────────────────

#[cfg(feature = "admin-api")]
pub fn sign_request(method: &str, path: &str, body: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    // Server expects epoch milliseconds, not RFC 3339
    let ts_ms = chrono::Utc::now().timestamp_millis().to_string();
    headers.insert("X-Rivers-Timestamp".into(), ts_ms.clone());

    if let Ok(key_path) = std::env::var("RIVERS_ADMIN_KEY") {
        if let Ok(key_hex) = std::fs::read_to_string(&key_path) {
            let key_hex = key_hex.trim();
            if let Ok(seed_bytes) = hex::decode(key_hex) {
                if seed_bytes.len() == 32 {
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
            }
        }
    }

    headers
}

#[cfg(feature = "admin-api")]
pub async fn admin_get(url: &str, path: &str) -> Result<serde_json::Value, String> {
    let headers = sign_request("GET", path, "");
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{url}{path}"));
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse JSON: {e}"))
}

#[cfg(feature = "admin-api")]
pub async fn admin_post(url: &str, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let headers = sign_request("POST", path, &body_str);
    let client = reqwest::Client::new();
    let mut req = client.post(format!("{url}{path}"))
        .header("Content-Type", "application/json")
        .body(body_str);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("parse JSON: {e}"))
}

// ── Admin API commands ──────────────────────────────────────────────────────

#[cfg(feature = "admin-api")]
pub async fn cmd_status(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/status").await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_deploy(url: &str, bundle_path: &str) -> Result<(), String> {
    let body = serde_json::json!({ "bundle_path": bundle_path });
    let data = admin_post(url, "/admin/deploy", &body).await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_drivers(url: &str) -> Result<(), String> {
    let data = admin_get(url, "/admin/drivers").await?;
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
    let data = admin_get(url, "/admin/datasources").await?;
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
    Ok(())
}

#[cfg(feature = "admin-api")]
pub async fn cmd_health(_url: &str) -> Result<(), String> {
    let main_url = std::env::var("RIVERS_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let data = admin_get(&main_url, "/health/verbose").await?;
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
        Err(_) => {
            eprintln!("Admin API unreachable — falling back to signal");
            signal_riversd(Signal::Kill)
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
        Err(_) => {
            eprintln!("Admin API unreachable — falling back to signal");
            signal_riversd(Signal::Term)
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
    let data = admin_get(url, &path).await?;
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
    let data = admin_get(url, &path).await?;
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
    let data = admin_post(url, &path, &serde_json::json!({})).await?;
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
    let data = admin_post(url, &path, &serde_json::json!({})).await?;
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
            let data = admin_get(url, "/admin/log/levels").await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("set") => {
            if args.len() < 3 {
                return Err("Usage: riversctl log set <event> <level>".into());
            }
            let body = serde_json::json!({ "event": args[1], "level": args[2] });
            let data = admin_post(url, "/admin/log/set", &body).await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        Some("reset") => {
            let data = admin_post(url, "/admin/log/reset", &serde_json::json!({})).await?;
            println!("{}", serde_json::to_string_pretty(&data).unwrap());
        }
        _ => return Err("Usage: riversctl log <levels|set|reset>".into()),
    }
    Ok(())
}
