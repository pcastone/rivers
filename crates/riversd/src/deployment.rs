//! App bundle deployment and lifecycle management.
//!
//! Per `rivers-application-spec.md` §7-9, §12, §14.
//!
//! Manages the deployment lifecycle: resource resolution, startup order,
//! health checks, and redeployment with zero-downtime.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::admin::{Deployment, DeploymentState};
use crate::init_handler::{ApplicationContext, InitHandlerConfig};
use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};

// ── App Type ────────────────────────────────────────────────────

/// Application type within a bundle.
///
/// Per spec §7: app-service starts before app-main.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppType {
    AppService,
    AppMain,
}

impl AppType {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "app-service" => Some(AppType::AppService),
            "app-main" => Some(AppType::AppMain),
            _ => None,
        }
    }
}

// ── App Manifest ────────────────────────────────────────────────

/// Parsed app manifest (per-app `manifest.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    pub app_id: String,
    pub app_type: String,
    pub name: String,
    pub port: u16,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

// ── Bundle Manifest ─────────────────────────────────────────────

/// Parsed bundle manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub name: String,
    pub version: String,
    pub apps: Vec<String>,
}

// ── Resource Resolution ─────────────────────────────────────────

/// A resolved resource reference.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedResource {
    pub name: String,
    pub resource_type: ResourceType,
    pub resolved: bool,
    pub error: Option<String>,
}

/// Types of resources that need resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ResourceType {
    Datasource,
    Service,
    LockboxAlias,
}

/// Resolve all resources for an app deployment.
///
/// Per spec §8: LockBox alias resolution, datasource objectId matching,
/// service endpoint construction.
pub fn resolve_resources(
    datasource_names: &[String],
    service_names: &[String],
    lockbox_aliases: &[String],
    available_datasources: &[String],
    available_services: &HashMap<String, String>,
) -> Vec<ResolvedResource> {
    let mut results = Vec::new();

    // Resolve datasources
    for name in datasource_names {
        let resolved = available_datasources.contains(name);
        results.push(ResolvedResource {
            name: name.clone(),
            resource_type: ResourceType::Datasource,
            resolved,
            error: if resolved {
                None
            } else {
                Some(format!("datasource '{}' not found", name))
            },
        });
    }

    // Resolve services
    for name in service_names {
        let resolved = available_services.contains_key(name);
        results.push(ResolvedResource {
            name: name.clone(),
            resource_type: ResourceType::Service,
            resolved,
            error: if resolved {
                None
            } else {
                Some(format!("service '{}' not found", name))
            },
        });
    }

    // Resolve LockBox aliases
    for alias in lockbox_aliases {
        // LockBox resolution is always deferred to startup
        results.push(ResolvedResource {
            name: alias.clone(),
            resource_type: ResourceType::LockboxAlias,
            resolved: true, // assume available (checked at startup)
            error: None,
        });
    }

    results
}

/// Check if all resources were resolved successfully.
pub fn all_resources_resolved(resources: &[ResolvedResource]) -> bool {
    resources.iter().all(|r| r.resolved)
}

// ── Startup Order ───────────────────────────────────────────────

/// An app in the startup sequence.
#[derive(Debug, Clone)]
pub struct StartupEntry {
    pub app_name: String,
    pub app_type: AppType,
    pub port: u16,
    pub dependencies: Vec<String>,
}

/// Compute startup order for apps in a bundle.
///
/// Per spec §9: all app-services before app-mains,
/// app-services in parallel unless inter-dependent.
pub fn compute_startup_order(apps: &[StartupEntry]) -> Vec<Vec<String>> {
    let mut phases: Vec<Vec<String>> = Vec::new();

    // Phase 1: all app-services (parallel unless dependencies)
    let services: Vec<&StartupEntry> = apps
        .iter()
        .filter(|a| a.app_type == AppType::AppService)
        .collect();

    if !services.is_empty() {
        // Simple topological sort for services
        let mut remaining: Vec<&StartupEntry> = services;
        let mut started: Vec<String> = Vec::new();

        while !remaining.is_empty() {
            let (ready, not_ready): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|app| {
                app.dependencies.iter().all(|dep| started.contains(dep))
            });

            if ready.is_empty() && !not_ready.is_empty() {
                // Circular dependency — force start remaining
                let names: Vec<String> = not_ready.iter().map(|a| a.app_name.clone()).collect();
                phases.push(names);
                break;
            }

            let names: Vec<String> = ready.iter().map(|a| a.app_name.clone()).collect();
            started.extend(names.clone());
            phases.push(names);
            remaining = not_ready;
        }
    }

    // Phase 2: all app-mains (parallel)
    let mains: Vec<String> = apps
        .iter()
        .filter(|a| a.app_type == AppType::AppMain)
        .map(|a| a.app_name.clone())
        .collect();

    if !mains.is_empty() {
        phases.push(mains);
    }

    phases
}

// ── Preflight Check ─────────────────────────────────────────────

/// Preflight validation result.
#[derive(Debug, Clone, Serialize)]
pub struct PreflightResult {
    pub passed: bool,
    pub checks: Vec<PreflightCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightCheck {
    pub name: String,
    pub passed: bool,
    pub message: Option<String>,
}

/// Run preflight checks for a bundle deployment.
///
/// Per spec §12 / SHAPE-19: appId unique, app type valid, bundle structure.
/// Port conflict is NOT checked at preflight — the OS reports bind failures at startup.
pub fn run_preflight(
    apps: &[AppManifest],
    used_app_ids: &[String],
) -> PreflightResult {
    let mut checks = Vec::new();

    // Check for duplicate app IDs
    let mut id_set = std::collections::HashSet::new();
    for app in apps {
        if used_app_ids.contains(&app.app_id) || !id_set.insert(&app.app_id) {
            checks.push(PreflightCheck {
                name: format!("appid_{}", app.app_id),
                passed: false,
                message: Some(format!("appId '{}' is already in use", app.app_id)),
            });
        } else {
            checks.push(PreflightCheck {
                name: format!("appid_{}", app.app_id),
                passed: true,
                message: None,
            });
        }
    }

    // Check app type validity
    for app in apps {
        if AppType::from_str_opt(&app.app_type).is_none() {
            checks.push(PreflightCheck {
                name: format!("type_{}", app.name),
                passed: false,
                message: Some(format!("unknown app type: '{}'", app.app_type)),
            });
        }
    }

    let passed = checks.iter().all(|c| c.passed);
    PreflightResult { passed, checks }
}

// ── Deployment Manager ──────────────────────────────────────────

/// Manages active deployments.
pub struct DeploymentManager {
    deployments: tokio::sync::RwLock<HashMap<String, Deployment>>,
}

impl DeploymentManager {
    pub fn new() -> Self {
        Self {
            deployments: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a new deployment.
    pub async fn create(&self, app_id: String, bundle_name: String) -> Deployment {
        let deployment = Deployment::new(app_id, bundle_name);
        let id = deployment.deploy_id.clone();
        self.deployments.write().await.insert(id, deployment.clone());
        deployment
    }

    /// Get a deployment by ID.
    pub async fn get(&self, deploy_id: &str) -> Option<Deployment> {
        self.deployments.read().await.get(deploy_id).cloned()
    }

    /// Transition a deployment to a new state.
    pub async fn transition(
        &self,
        deploy_id: &str,
        new_state: DeploymentState,
    ) -> Result<(), DeploymentError> {
        let mut deployments = self.deployments.write().await;
        let deployment = deployments
            .get_mut(deploy_id)
            .ok_or_else(|| DeploymentError::NotFound(deploy_id.to_string()))?;

        deployment
            .transition(new_state)
            .map_err(|e| DeploymentError::InvalidTransition(e.to_string()))
    }

    /// List all deployments.
    pub async fn list(&self) -> Vec<Deployment> {
        self.deployments.read().await.values().cloned().collect()
    }

    /// Get deployments for a specific app.
    pub async fn list_for_app(&self, app_id: &str) -> Vec<Deployment> {
        self.deployments
            .read()
            .await
            .values()
            .filter(|d| d.app_id == app_id)
            .cloned()
            .collect()
    }
}

impl Default for DeploymentManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Health Check Backoff (B4.1) ──────────────────────────────

/// Exponential backoff configuration for health checks.
///
/// Per spec §12.3: health checks during deployment use exponential backoff.
#[derive(Debug, Clone)]
pub struct HealthCheckBackoff {
    /// Initial delay in milliseconds.
    pub initial_delay_ms: u64,
    /// Maximum delay in milliseconds.
    pub max_delay_ms: u64,
    /// Multiplier per retry.
    pub multiplier: f64,
    /// Maximum number of retries.
    pub max_retries: u32,
}

impl Default for HealthCheckBackoff {
    fn default() -> Self {
        Self {
            initial_delay_ms: 100,
            max_delay_ms: 10_000,
            multiplier: 2.0,
            max_retries: 10,
        }
    }
}

impl HealthCheckBackoff {
    /// Calculate delay for a given attempt (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        let delay = self.initial_delay_ms as f64 * self.multiplier.powi(attempt as i32);
        (delay as u64).min(self.max_delay_ms)
    }
}

// ── Zero-Downtime Redeployment (B4.2) ──────────────────────

/// State for a rolling redeployment.
///
/// Per spec §14: new bundle starts alongside old, traffic switches after health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RedeployPhase {
    /// New version starting.
    Starting,
    /// New version passed health checks, draining old.
    Draining,
    /// Old version fully drained, swap complete.
    Complete,
    /// Redeployment failed, rollback.
    RolledBack,
}

/// A redeployment operation tracking old → new transition.
#[derive(Debug, Clone, Serialize)]
pub struct RedeploymentState {
    pub old_deploy_id: String,
    pub new_deploy_id: String,
    pub phase: RedeployPhase,
    pub started_at: String,
}

impl RedeploymentState {
    pub fn new(old_deploy_id: String, new_deploy_id: String) -> Self {
        Self {
            old_deploy_id,
            new_deploy_id,
            phase: RedeployPhase::Starting,
            started_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Advance the redeployment to the next phase.
    pub fn advance(&mut self) -> Result<(), DeploymentError> {
        self.phase = match self.phase {
            RedeployPhase::Starting => RedeployPhase::Draining,
            RedeployPhase::Draining => RedeployPhase::Complete,
            _ => {
                return Err(DeploymentError::InvalidTransition(format!(
                    "cannot advance from {:?}",
                    self.phase
                )))
            }
        };
        Ok(())
    }

    /// Roll back the redeployment.
    pub fn rollback(&mut self) {
        self.phase = RedeployPhase::RolledBack;
    }
}

// ── Auth Scope Carry-Over (B4.3) ────────────────────────────

/// Inter-service auth scope used during redeployment.
///
/// Per spec §14.2: services may need to validate tokens from
/// the old deployment during the drain window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthScopeCarryOver {
    /// App ID this scope belongs to.
    pub app_id: String,
    /// Session signing keys from the old deployment.
    pub old_session_keys: Vec<String>,
    /// Expiry for carry-over acceptance.
    pub valid_until: String,
}

impl AuthScopeCarryOver {
    pub fn new(app_id: String, old_keys: Vec<String>, drain_timeout_secs: u64) -> Self {
        let valid_until = (chrono::Utc::now()
            + chrono::Duration::try_seconds(drain_timeout_secs as i64)
                .unwrap_or_else(|| chrono::Duration::seconds(30)))
        .to_rfc3339();
        Self {
            app_id,
            old_session_keys: old_keys,
            valid_until,
        }
    }

    /// Check if the carry-over is still valid.
    pub fn is_valid(&self) -> bool {
        if let Ok(until) = chrono::DateTime::parse_from_rfc3339(&self.valid_until) {
            chrono::Utc::now() < until
        } else {
            false
        }
    }
}

// ── Init Handler Dispatch (§13.7) ────────────────────────────

/// Execute an app's init handler via ProcessPool.
///
/// Per spec §13.7: fires after resource resolution, before app accepts traffic.
pub async fn execute_init_handler(
    pool: &ProcessPoolManager,
    init_config: &InitHandlerConfig,
    app_context: &ApplicationContext,
) -> Result<(), String> {
    let handler = match &init_config.init_handler {
        Some(h) => h,
        None => return Ok(()), // No init handler declared
    };
    let entrypoint_name = init_config
        .init_entrypoint
        .as_deref()
        .unwrap_or("init");

    let entrypoint = Entrypoint {
        module: handler.clone(),
        function: entrypoint_name.to_string(),
        language: if handler.ends_with(".ts") {
            "typescript"
        } else {
            "javascript"
        }
        .to_string(),
    };

    let args = serde_json::json!({
        "app_id": app_context.app_id,
        "app_name": app_context.app_name,
        "config": app_context.config,
    });

    let task_ctx = TaskContextBuilder::new()
        .entrypoint(entrypoint)
        .args(args)
        .app_id(app_context.app_id.clone())
        .trace_id(format!("init:{}", app_context.app_id))
        .build()
        .map_err(|e| format!("init handler context build: {e}"))?;

    pool.dispatch("default", task_ctx)
        .await
        .map_err(|e| format!("init handler dispatch: {e}"))?;

    tracing::info!(
        target: "rivers.deployment",
        app = %app_context.app_name,
        "init handler executed"
    );
    Ok(())
}

// ── Error Types ─────────────────────────────────────────────────

/// Deployment errors.
#[derive(Debug, thiserror::Error)]
pub enum DeploymentError {
    #[error("deployment not found: {0}")]
    NotFound(String),

    #[error("invalid transition: {0}")]
    InvalidTransition(String),

    #[error("resource resolution failed: {0}")]
    ResolutionFailed(String),

    #[error("health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("preflight failed: {0}")]
    PreflightFailed(String),
}
