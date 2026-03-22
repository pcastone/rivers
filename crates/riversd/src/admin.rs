//! Admin API authentication and authorization.
//!
//! Per `rivers-httpd-spec.md` §15.
//!
//! Admin server features:
//! - Ed25519 request authentication (X-Rivers-Signature, X-Rivers-Timestamp)
//! - RBAC: roles → permissions, identity → role binding
//! - IP allowlist enforcement
//! - Deployment lifecycle endpoints

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Admin Auth Config ───────────────────────────────────────────

/// Admin API authentication configuration.
///
/// Per spec §15: Ed25519 signatures, replay protection, RBAC.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdminAuthConfig {
    /// Ed25519 public key for request verification (base64-encoded).
    pub public_key: Option<String>,

    /// IP allowlist. Empty = all IPs allowed.
    #[serde(default)]
    pub ip_allowlist: Vec<String>,

    /// Disable admin auth (`--no-admin-auth` flag).
    #[serde(default)]
    pub no_auth: bool,

    /// Role → permissions mapping.
    #[serde(default)]
    pub roles: HashMap<String, Vec<AdminPermission>>,

    /// Identity (CN or key fingerprint) → role binding.
    #[serde(default)]
    pub identity_roles: HashMap<String, String>,

    /// Replay window in seconds (default: 300 = ±5 min).
    #[serde(default = "default_replay_window")]
    pub replay_window_secs: u64,
}

fn default_replay_window() -> u64 {
    300
}

// ── Permissions ─────────────────────────────────────────────────

/// Admin API permissions.
///
/// Per spec §15.4.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminPermission {
    /// Read status, drivers, datasources.
    StatusRead,
    /// Deploy bundles.
    DeployWrite,
    /// Approve/reject deployments.
    DeployApprove,
    /// Promote deployments.
    DeployPromote,
    /// Read deployment list.
    DeployRead,
    /// Full admin access.
    Admin,
}

impl AdminPermission {
    /// Check if this permission grants access to another permission.
    ///
    /// Admin grants all permissions.
    pub fn grants(&self, required: &AdminPermission) -> bool {
        self == &AdminPermission::Admin || self == required
    }
}

// ── Request Authentication ──────────────────────────────────────

/// Validate admin request timestamp for replay protection.
///
/// Per spec §15.3: ±5 minute replay window.
/// `replay_window_ms` is in **milliseconds** (e.g. 300_000 for ±5 min).
pub fn validate_timestamp(timestamp_str: &str, replay_window_ms: u64) -> Result<(), AdminError> {
    let ts: i64 = timestamp_str
        .parse()
        .map_err(|_| AdminError::InvalidTimestamp(timestamp_str.to_string()))?;

    let now = chrono::Utc::now().timestamp_millis();
    let diff = (now - ts).unsigned_abs();

    if diff > replay_window_ms {
        return Err(AdminError::ReplayDetected {
            drift_secs: diff,
            max_secs: replay_window_ms,
        });
    }

    Ok(())
}

/// Check if a remote IP is in the allowlist.
///
/// Per spec §15.4: supports both exact IPs and CIDR ranges (e.g. `"10.0.0.0/8"`).
/// Empty allowlist = all IPs allowed.
pub fn check_ip_allowlist(remote_ip: &str, allowlist: &[String]) -> Result<(), AdminError> {
    if allowlist.is_empty() {
        return Ok(());
    }

    let addr: std::net::IpAddr = match remote_ip.parse() {
        Ok(a) => a,
        Err(_) => return Err(AdminError::IpNotAllowed(remote_ip.to_string())),
    };

    for entry in allowlist {
        // Try CIDR first (e.g. "10.0.0.0/8"), then exact IP
        if let Ok(net) = entry.parse::<ipnet::IpNet>() {
            if net.contains(&addr) {
                return Ok(());
            }
        } else if let Ok(exact) = entry.parse::<std::net::IpAddr>() {
            if exact == addr {
                return Ok(());
            }
        } else {
            tracing::warn!(entry = %entry, "malformed IP allowlist entry, skipping");
        }
    }

    Err(AdminError::IpNotAllowed(remote_ip.to_string()))
}

/// Check if an identity has the required permission.
pub fn check_permission(
    identity: &str,
    required: &AdminPermission,
    config: &AdminAuthConfig,
) -> Result<(), AdminError> {
    // No auth mode — all permissions granted
    if config.no_auth {
        return Ok(());
    }

    // Look up role for identity
    let role = config
        .identity_roles
        .get(identity)
        .ok_or_else(|| AdminError::IdentityNotFound(identity.to_string()))?;

    // Look up permissions for role
    let permissions = config
        .roles
        .get(role)
        .ok_or_else(|| AdminError::RoleNotFound(role.clone()))?;

    // Check if any permission grants access
    if permissions.iter().any(|p| p.grants(required)) {
        Ok(())
    } else {
        Err(AdminError::PermissionDenied {
            identity: identity.to_string(),
            required: format!("{:?}", required),
        })
    }
}

// ── Deployment Types ────────────────────────────────────────────

/// Deployment state machine.
///
/// Per spec §15.6: PENDING → RESOLVING → STARTING → RUNNING / FAILED.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentState {
    Pending,
    Resolving,
    Starting,
    Running,
    Failed,
    Stopping,
    Stopped,
}

/// A deployment record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub deploy_id: String,
    pub app_id: String,
    pub bundle_name: String,
    pub state: DeploymentState,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
}

impl Deployment {
    pub fn new(app_id: String, bundle_name: String) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            deploy_id: format!("deploy_{}", uuid::Uuid::new_v4()),
            app_id,
            bundle_name,
            state: DeploymentState::Pending,
            created_at: now.clone(),
            updated_at: now,
            error: None,
        }
    }

    /// Transition to a new state.
    pub fn transition(&mut self, new_state: DeploymentState) -> Result<(), AdminError> {
        let valid = match (&self.state, &new_state) {
            (DeploymentState::Pending, DeploymentState::Resolving) => true,
            (DeploymentState::Resolving, DeploymentState::Starting) => true,
            (DeploymentState::Resolving, DeploymentState::Failed) => true,
            (DeploymentState::Starting, DeploymentState::Running) => true,
            (DeploymentState::Starting, DeploymentState::Failed) => true,
            (DeploymentState::Running, DeploymentState::Stopping) => true,
            (DeploymentState::Stopping, DeploymentState::Stopped) => true,
            _ => false,
        };

        if !valid {
            return Err(AdminError::InvalidTransition {
                from: format!("{:?}", self.state),
                to: format!("{:?}", new_state),
            });
        }

        self.state = new_state;
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }
}

// ── Error Types ─────────────────────────────────────────────────

/// Admin API errors.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("replay detected: drift {drift_secs}s exceeds window {max_secs}s")]
    ReplayDetected { drift_secs: u64, max_secs: u64 },

    #[error("IP not allowed: {0}")]
    IpNotAllowed(String),

    #[error("identity not found: {0}")]
    IdentityNotFound(String),

    #[error("role not found: {0}")]
    RoleNotFound(String),

    #[error("permission denied: {identity} lacks {required}")]
    PermissionDenied { identity: String, required: String },

    #[error("invalid state transition: {from} → {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Ed25519 signature verification not yet available")]
    SignatureVerificationUnavailable,
}
