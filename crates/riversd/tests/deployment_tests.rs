use std::collections::HashMap;

use riversd::admin::DeploymentState;
use riversd::deployment::{
    all_resources_resolved, compute_startup_order, resolve_resources, run_preflight,
    AppManifest, AppType, AuthScopeCarryOver, DeploymentManager, HealthCheckBackoff,
    RedeployPhase, RedeploymentState, ResourceType, StartupEntry,
};

// ── AppType ─────────────────────────────────────────────────────

#[test]
fn app_type_from_str() {
    assert_eq!(AppType::from_str_opt("app-service"), Some(AppType::AppService));
    assert_eq!(AppType::from_str_opt("app-main"), Some(AppType::AppMain));
    assert_eq!(AppType::from_str_opt("unknown"), None);
}

// ── Resource Resolution ─────────────────────────────────────────

#[test]
fn resolve_all_present() {
    let mut services = HashMap::new();
    services.insert("auth-service".to_string(), "http://localhost:9100".to_string());

    let results = resolve_resources(
        &["postgres".to_string()],
        &["auth-service".to_string()],
        &["db_password".to_string()],
        &["postgres".to_string()],
        &services,
    );

    assert!(all_resources_resolved(&results));
    assert_eq!(results.len(), 3);
}

#[test]
fn resolve_missing_datasource() {
    let results = resolve_resources(
        &["missing_db".to_string()],
        &[],
        &[],
        &["postgres".to_string()],
        &HashMap::new(),
    );

    assert!(!all_resources_resolved(&results));
    assert!(!results[0].resolved);
    assert_eq!(results[0].resource_type, ResourceType::Datasource);
}

#[test]
fn resolve_missing_service() {
    let results = resolve_resources(
        &[],
        &["missing_svc".to_string()],
        &[],
        &[],
        &HashMap::new(),
    );

    assert!(!all_resources_resolved(&results));
    assert!(!results[0].resolved);
    assert_eq!(results[0].resource_type, ResourceType::Service);
}

#[test]
fn resolve_lockbox_always_deferred() {
    let results = resolve_resources(&[], &[], &["secret".to_string()], &[], &HashMap::new());

    assert!(all_resources_resolved(&results));
    assert_eq!(results[0].resource_type, ResourceType::LockboxAlias);
}

// ── Startup Order ───────────────────────────────────────────────

#[test]
fn startup_services_before_mains() {
    let apps = vec![
        StartupEntry {
            app_name: "main-app".into(),
            app_type: AppType::AppMain,
            port: 8080,
            dependencies: vec![],
        },
        StartupEntry {
            app_name: "api-service".into(),
            app_type: AppType::AppService,
            port: 9100,
            dependencies: vec![],
        },
    ];

    let order = compute_startup_order(&apps);
    assert_eq!(order.len(), 2);
    assert!(order[0].contains(&"api-service".to_string()));
    assert!(order[1].contains(&"main-app".to_string()));
}

#[test]
fn startup_dependent_services_ordered() {
    let apps = vec![
        StartupEntry {
            app_name: "auth".into(),
            app_type: AppType::AppService,
            port: 9100,
            dependencies: vec![],
        },
        StartupEntry {
            app_name: "api".into(),
            app_type: AppType::AppService,
            port: 9200,
            dependencies: vec!["auth".to_string()],
        },
    ];

    let order = compute_startup_order(&apps);
    // auth first, then api
    assert!(order.len() >= 2);
    assert!(order[0].contains(&"auth".to_string()));
    assert!(order[1].contains(&"api".to_string()));
}

#[test]
fn startup_independent_services_parallel() {
    let apps = vec![
        StartupEntry {
            app_name: "svc-a".into(),
            app_type: AppType::AppService,
            port: 9100,
            dependencies: vec![],
        },
        StartupEntry {
            app_name: "svc-b".into(),
            app_type: AppType::AppService,
            port: 9200,
            dependencies: vec![],
        },
    ];

    let order = compute_startup_order(&apps);
    // Both should be in same phase
    assert_eq!(order.len(), 1);
    assert_eq!(order[0].len(), 2);
}

#[test]
fn startup_empty_bundle() {
    let order = compute_startup_order(&[]);
    assert!(order.is_empty());
}

#[test]
fn startup_only_mains() {
    let apps = vec![StartupEntry {
        app_name: "main".into(),
        app_type: AppType::AppMain,
        port: 8080,
        dependencies: vec![],
    }];

    let order = compute_startup_order(&apps);
    assert_eq!(order.len(), 1);
    assert!(order[0].contains(&"main".to_string()));
}

// ── Preflight ───────────────────────────────────────────────────

#[test]
fn preflight_passes_valid_bundle() {
    let apps = vec![AppManifest {
        app_id: "app-1".into(),
        app_type: "app-service".into(),
        name: "my-service".into(),
        port: 9100,
        dependencies: vec![],
    }];

    let result = run_preflight(&apps, &[]);
    assert!(result.passed);
}

#[test]
fn preflight_catches_duplicate_app_id() {
    let apps = vec![AppManifest {
        app_id: "existing-id".into(),
        app_type: "app-service".into(),
        name: "my-service".into(),
        port: 9100,
        dependencies: vec![],
    }];

    let result = run_preflight(&apps, &["existing-id".to_string()]);
    assert!(!result.passed);
    assert!(result.checks.iter().any(|c| !c.passed && c.name.contains("appid")));
}

#[test]
fn preflight_catches_invalid_app_type() {
    let apps = vec![AppManifest {
        app_id: "a".into(),
        app_type: "invalid-type".into(),
        name: "bad".into(),
        port: 9100,
        dependencies: vec![],
    }];

    let result = run_preflight(&apps, &[]);
    assert!(!result.passed);
}

// ── DeploymentManager ───────────────────────────────────────────

#[tokio::test]
async fn manager_create_and_get() {
    let mgr = DeploymentManager::new();
    let deploy = mgr.create("app-1".into(), "bundle".into()).await;
    assert_eq!(deploy.state, DeploymentState::Pending);

    let retrieved = mgr.get(&deploy.deploy_id).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().app_id, "app-1");
}

#[tokio::test]
async fn manager_transition() {
    let mgr = DeploymentManager::new();
    let deploy = mgr.create("app-1".into(), "bundle".into()).await;

    mgr.transition(&deploy.deploy_id, DeploymentState::Resolving)
        .await
        .unwrap();

    let updated = mgr.get(&deploy.deploy_id).await.unwrap();
    assert_eq!(updated.state, DeploymentState::Resolving);
}

#[tokio::test]
async fn manager_transition_nonexistent() {
    let mgr = DeploymentManager::new();
    let result = mgr
        .transition("nonexistent", DeploymentState::Resolving)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn manager_list() {
    let mgr = DeploymentManager::new();
    mgr.create("app-1".into(), "b1".into()).await;
    mgr.create("app-2".into(), "b2".into()).await;

    let all = mgr.list().await;
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn manager_list_for_app() {
    let mgr = DeploymentManager::new();
    mgr.create("app-1".into(), "b1".into()).await;
    mgr.create("app-1".into(), "b2".into()).await;
    mgr.create("app-2".into(), "b3".into()).await;

    let app1_deploys = mgr.list_for_app("app-1").await;
    assert_eq!(app1_deploys.len(), 2);
}

// ── Health Check Backoff ────────────────────────────────────

#[test]
fn backoff_exponential_delay() {
    let backoff = HealthCheckBackoff::default();
    assert_eq!(backoff.delay_for_attempt(0), 100);
    assert_eq!(backoff.delay_for_attempt(1), 200);
    assert_eq!(backoff.delay_for_attempt(2), 400);
    assert_eq!(backoff.delay_for_attempt(3), 800);
}

#[test]
fn backoff_capped_at_max() {
    let backoff = HealthCheckBackoff {
        max_delay_ms: 500,
        ..Default::default()
    };
    assert_eq!(backoff.delay_for_attempt(10), 500);
}

// ── Redeployment State ──────────────────────────────────────

#[test]
fn redeployment_advance_phases() {
    let mut state = RedeploymentState::new("old-1".into(), "new-1".into());
    assert_eq!(state.phase, RedeployPhase::Starting);

    state.advance().unwrap();
    assert_eq!(state.phase, RedeployPhase::Draining);

    state.advance().unwrap();
    assert_eq!(state.phase, RedeployPhase::Complete);
}

#[test]
fn redeployment_advance_from_complete_fails() {
    let mut state = RedeploymentState::new("old".into(), "new".into());
    state.advance().unwrap();
    state.advance().unwrap();
    assert!(state.advance().is_err());
}

#[test]
fn redeployment_rollback() {
    let mut state = RedeploymentState::new("old".into(), "new".into());
    state.advance().unwrap(); // Starting → Draining
    state.rollback();
    assert_eq!(state.phase, RedeployPhase::RolledBack);
}

// ── Auth Scope Carry-Over ───────────────────────────────────

#[test]
fn auth_carry_over_valid() {
    let carry = AuthScopeCarryOver::new(
        "app-1".into(),
        vec!["key-1".into()],
        60, // 60 second window
    );
    assert!(carry.is_valid());
}

#[test]
fn auth_carry_over_expired() {
    let carry = AuthScopeCarryOver {
        app_id: "app-1".into(),
        old_session_keys: vec!["key-1".into()],
        valid_until: "2020-01-01T00:00:00Z".into(), // Past date
    };
    assert!(!carry.is_valid());
}
