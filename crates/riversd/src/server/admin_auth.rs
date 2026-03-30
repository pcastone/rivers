//! Admin API authentication middleware and RBAC helpers.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use sha2::Digest;

use rivers_runtime::rivers_core::ServerConfig;

use crate::error_response;

use super::context::AppContext;

// ── Admin Auth Middleware ────────────────────────────────────────

/// Admin API authentication middleware.
///
/// Per spec §15.3, §18.3: Ed25519 signature verification.
/// Bypassed when `--no-admin-auth` flag is set (AdminApiConfig.no_auth).
pub(super) async fn admin_auth_middleware(
    State(ctx): State<AppContext>,
    request: Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::admin_auth;

    // Check --no-admin-auth flag
    if ctx.config.base.admin_api.no_auth.unwrap_or(false) {
        return next.run(request).await;
    }

    // Check if public_key is configured
    let public_key_hex = match &ctx.config.base.admin_api.public_key {
        Some(pk) => pk.clone(),
        None => {
            unreachable!("startup validation guarantees public_key is present");
        }
    };

    // Parse the configured public key
    let public_key = match admin_auth::parse_public_key(&public_key_hex) {
        Ok(pk) => pk,
        Err(e) => {
            tracing::error!(target: "rivers.admin", "invalid admin public key: {e}");
            return error_response::internal_error("admin auth misconfigured")
                .into_axum_response();
        }
    };

    // Extract signature and timestamp headers
    let signature_hex = request.headers()
        .get("x-rivers-signature")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let timestamp = request.headers()
        .get("x-rivers-timestamp")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (signature_hex, timestamp) {
        (Some(sig_hex), Some(ts)) => {
            // Validate timestamp freshness (300_000 millisecond window)
            if let Err(e) = admin_auth::validate_timestamp(&ts, 300_000) {
                return error_response::unauthorized(e.to_string())
                    .into_axum_response();
            }

            // Decode signature from hex
            let sig_bytes = match hex::decode(&sig_hex) {
                Ok(b) => b,
                Err(_) => {
                    return error_response::unauthorized("invalid signature encoding")
                        .into_axum_response();
                }
            };

            // Consume body, compute SHA-256 hash, then reconstruct the request
            let method = request.method().as_str().to_string();
            let path = request.uri().path().to_string();

            let (parts, body) = request.into_parts();
            let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
                Ok(b) => b,
                Err(_) => {
                    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
                }
            };
            let body_hash = hex::encode(sha2::Sha256::digest(&bytes));
            let request = Request::from_parts(parts, axum::body::Body::from(bytes));

            if let Err(_) = admin_auth::verify_admin_signature(
                &public_key, &method, &path, &ts, &body_hash, &sig_bytes,
            ) {
                return error_response::unauthorized("signature verification failed")
                    .into_axum_response();
            }

            // ── IP allowlist check ──────────────────────────────
            let ip_allowlist = &ctx.config.security.admin_ip_allowlist;
            if !ip_allowlist.is_empty() {
                let remote_ip = request
                    .extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|ci| ci.0.ip().to_string())
                    .unwrap_or_default();
                if let Err(_) = crate::admin::check_ip_allowlist(&remote_ip, ip_allowlist) {
                    return error_response::forbidden("IP not in admin allowlist")
                        .into_axum_response();
                }
            }

            // ── RBAC permission check ───────────────────────────
            // Map request path to required permission
            let required_permission = path_to_admin_permission(&path);
            if let Some(perm) = required_permission {
                // Derive identity from the verified public key fingerprint
            let identity = hex::encode(sha2::Sha256::digest(public_key.as_bytes()));
                let admin_auth_config = ctx.admin_auth_config.as_ref()
                    .unwrap_or(&*DEFAULT_ADMIN_AUTH_CONFIG);
                if let Err(_) = crate::admin::check_permission(&identity, &perm, admin_auth_config) {
                    return error_response::forbidden("permission denied")
                        .into_axum_response();
                }
            }

            next.run(request).await
        }
        _ => {
            error_response::unauthorized("missing X-Rivers-Signature or X-Rivers-Timestamp header")
                .into_axum_response()
        }
    }
}

/// Map an admin API path to the required `AdminPermission`.
///
/// Returns `None` for unknown paths (middleware will allow through).
fn path_to_admin_permission(path: &str) -> Option<crate::admin::AdminPermission> {
    use crate::admin::AdminPermission;
    match path {
        "/admin/status" | "/admin/drivers" | "/admin/datasources" => Some(AdminPermission::StatusRead),
        "/admin/deploy" | "/admin/deploy/test" => Some(AdminPermission::DeployWrite),
        "/admin/deploy/approve" | "/admin/deploy/reject" => Some(AdminPermission::DeployApprove),
        "/admin/deploy/promote" => Some(AdminPermission::DeployPromote),
        "/admin/deployments" => Some(AdminPermission::DeployRead),
        p if p.starts_with("/admin/log") => Some(AdminPermission::Admin),
        "/admin/shutdown" => Some(AdminPermission::Admin),
        _ => None,
    }
}

/// Fallback for when `admin_auth_config` is not initialized (e.g. tests).
static DEFAULT_ADMIN_AUTH_CONFIG: std::sync::LazyLock<crate::admin::AdminAuthConfig> =
    std::sync::LazyLock::new(crate::admin::AdminAuthConfig::default);

/// Build an `AdminAuthConfig` from server config for RBAC checks.
///
/// Called once at startup and stored in `AppContext.admin_auth_config` (AN11.4).
/// Bridges from `rivers_runtime::rivers_core::config::RbacConfig` to the admin module's
/// `AdminAuthConfig` which `check_permission` expects.
pub(super) fn build_admin_auth_config_for_rbac(config: &ServerConfig) -> crate::admin::AdminAuthConfig {
    use crate::admin::{AdminAuthConfig, AdminPermission};

    let mut auth_config = AdminAuthConfig::default();
    if let Some(ref rbac) = config.base.admin_api.rbac {
        // Convert role → Vec<String> to role → Vec<AdminPermission>
        for (role, perms) in &rbac.roles {
            let permissions: Vec<AdminPermission> = perms
                .iter()
                .filter_map(|p| match p.as_str() {
                    "status_read" => Some(AdminPermission::StatusRead),
                    "deploy_write" => Some(AdminPermission::DeployWrite),
                    "deploy_approve" => Some(AdminPermission::DeployApprove),
                    "deploy_promote" => Some(AdminPermission::DeployPromote),
                    "deploy_read" => Some(AdminPermission::DeployRead),
                    "admin" => Some(AdminPermission::Admin),
                    _ => {
                        tracing::warn!(permission = %p, role = %role, "unknown admin permission, skipping");
                        None
                    }
                })
                .collect();
            auth_config.roles.insert(role.clone(), permissions);
        }
        auth_config.identity_roles = rbac.bindings.clone();
    }
    auth_config.no_auth = config.base.admin_api.no_auth.unwrap_or(false);
    auth_config
}
