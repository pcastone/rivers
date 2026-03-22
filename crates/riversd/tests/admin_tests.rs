use std::collections::HashMap;

use riversd::admin::{
    check_ip_allowlist, check_permission, validate_timestamp, AdminAuthConfig, AdminError,
    AdminPermission, Deployment, DeploymentState,
};

// ── Timestamp Validation ────────────────────────────────────────

#[test]
fn valid_timestamp_within_window() {
    let now = chrono::Utc::now().timestamp_millis().to_string();
    assert!(validate_timestamp(&now, 300_000).is_ok());
}

#[test]
fn invalid_timestamp_format() {
    let result = validate_timestamp("not_a_number", 300_000);
    assert!(matches!(result.unwrap_err(), AdminError::InvalidTimestamp(_)));
}

#[test]
fn expired_timestamp_rejected() {
    let old = (chrono::Utc::now().timestamp_millis() - 600_000).to_string();
    let result = validate_timestamp(&old, 300_000);
    assert!(matches!(result.unwrap_err(), AdminError::ReplayDetected { .. }));
}

#[test]
fn future_timestamp_within_window() {
    let future = (chrono::Utc::now().timestamp_millis() + 100_000).to_string();
    assert!(validate_timestamp(&future, 300_000).is_ok());
}

#[test]
fn future_timestamp_outside_window() {
    let far_future = (chrono::Utc::now().timestamp_millis() + 600_000).to_string();
    let result = validate_timestamp(&far_future, 300_000);
    assert!(matches!(result.unwrap_err(), AdminError::ReplayDetected { .. }));
}

// ── IP Allowlist ────────────────────────────────────────────────

#[test]
fn empty_allowlist_allows_all() {
    assert!(check_ip_allowlist("1.2.3.4", &[]).is_ok());
}

#[test]
fn ip_in_allowlist_allowed() {
    let list = vec!["1.2.3.4".to_string(), "5.6.7.8".to_string()];
    assert!(check_ip_allowlist("1.2.3.4", &list).is_ok());
}

#[test]
fn ip_not_in_allowlist_rejected() {
    let list = vec!["1.2.3.4".to_string()];
    let result = check_ip_allowlist("9.9.9.9", &list);
    assert!(matches!(result.unwrap_err(), AdminError::IpNotAllowed(_)));
}

// ── IP Allowlist — CIDR ─────────────────────────────────────────

#[test]
fn allowlist_cidr_allows_ip_in_range() {
    let list = vec!["10.0.0.0/8".to_string()];
    assert!(check_ip_allowlist("10.1.2.3", &list).is_ok());
}

#[test]
fn allowlist_cidr_rejects_ip_outside_range() {
    let list = vec!["10.0.0.0/8".to_string()];
    let result = check_ip_allowlist("192.168.1.1", &list);
    assert!(matches!(result.unwrap_err(), AdminError::IpNotAllowed(_)));
}

#[test]
fn allowlist_exact_ip_still_works() {
    let list = vec!["192.168.1.50".to_string()];
    assert!(check_ip_allowlist("192.168.1.50", &list).is_ok());
}

#[test]
fn allowlist_ipv6_cidr() {
    let list = vec!["::1/128".to_string()];
    assert!(check_ip_allowlist("::1", &list).is_ok());
}

#[test]
fn allowlist_mixed_cidr_and_exact() {
    let list = vec![
        "10.0.0.0/8".to_string(),
        "192.168.1.50".to_string(),
    ];
    assert!(check_ip_allowlist("10.99.0.1", &list).is_ok());
    assert!(check_ip_allowlist("192.168.1.50", &list).is_ok());
    let result = check_ip_allowlist("172.16.0.1", &list);
    assert!(matches!(result.unwrap_err(), AdminError::IpNotAllowed(_)));
}

#[test]
fn allowlist_malformed_entry_skipped() {
    // Malformed entry is skipped; exact match still works
    let list = vec!["not-an-ip".to_string(), "1.2.3.4".to_string()];
    assert!(check_ip_allowlist("1.2.3.4", &list).is_ok());
    let result = check_ip_allowlist("5.6.7.8", &list);
    assert!(matches!(result.unwrap_err(), AdminError::IpNotAllowed(_)));
}

// ── Permissions ─────────────────────────────────────────────────

#[test]
fn admin_grants_all() {
    assert!(AdminPermission::Admin.grants(&AdminPermission::StatusRead));
    assert!(AdminPermission::Admin.grants(&AdminPermission::DeployWrite));
    assert!(AdminPermission::Admin.grants(&AdminPermission::DeployApprove));
}

#[test]
fn specific_permission_grants_self() {
    assert!(AdminPermission::StatusRead.grants(&AdminPermission::StatusRead));
}

#[test]
fn specific_permission_does_not_grant_other() {
    assert!(!AdminPermission::StatusRead.grants(&AdminPermission::DeployWrite));
}

// ── RBAC ────────────────────────────────────────────────────────

fn test_config() -> AdminAuthConfig {
    let mut roles = HashMap::new();
    roles.insert(
        "operator".to_string(),
        vec![AdminPermission::StatusRead, AdminPermission::DeployRead],
    );
    roles.insert("admin".to_string(), vec![AdminPermission::Admin]);

    let mut identity_roles = HashMap::new();
    identity_roles.insert("alice".to_string(), "admin".to_string());
    identity_roles.insert("bob".to_string(), "operator".to_string());

    AdminAuthConfig {
        roles,
        identity_roles,
        no_auth: false,
        ..Default::default()
    }
}

#[test]
fn check_permission_admin_allows_all() {
    let config = test_config();
    assert!(check_permission("alice", &AdminPermission::DeployWrite, &config).is_ok());
    assert!(check_permission("alice", &AdminPermission::StatusRead, &config).is_ok());
}

#[test]
fn check_permission_operator_limited() {
    let config = test_config();
    assert!(check_permission("bob", &AdminPermission::StatusRead, &config).is_ok());
    assert!(check_permission("bob", &AdminPermission::DeployRead, &config).is_ok());

    let result = check_permission("bob", &AdminPermission::DeployWrite, &config);
    assert!(matches!(result.unwrap_err(), AdminError::PermissionDenied { .. }));
}

#[test]
fn check_permission_unknown_identity() {
    let config = test_config();
    let result = check_permission("unknown", &AdminPermission::StatusRead, &config);
    assert!(matches!(result.unwrap_err(), AdminError::IdentityNotFound(_)));
}

#[test]
fn check_permission_no_auth_allows_all() {
    let config = AdminAuthConfig {
        no_auth: true,
        ..Default::default()
    };
    assert!(check_permission("anyone", &AdminPermission::DeployWrite, &config).is_ok());
}

// ── Deployment State Machine ────────────────────────────────────

#[test]
fn deployment_new() {
    let deploy = Deployment::new("app-1".into(), "my-bundle".into());
    assert!(deploy.deploy_id.starts_with("deploy_"));
    assert_eq!(deploy.state, DeploymentState::Pending);
    assert_eq!(deploy.app_id, "app-1");
    assert!(deploy.error.is_none());
}

#[test]
fn deployment_valid_transitions() {
    let mut deploy = Deployment::new("app".into(), "bundle".into());

    deploy.transition(DeploymentState::Resolving).unwrap();
    assert_eq!(deploy.state, DeploymentState::Resolving);

    deploy.transition(DeploymentState::Starting).unwrap();
    assert_eq!(deploy.state, DeploymentState::Starting);

    deploy.transition(DeploymentState::Running).unwrap();
    assert_eq!(deploy.state, DeploymentState::Running);

    deploy.transition(DeploymentState::Stopping).unwrap();
    assert_eq!(deploy.state, DeploymentState::Stopping);

    deploy.transition(DeploymentState::Stopped).unwrap();
    assert_eq!(deploy.state, DeploymentState::Stopped);
}

#[test]
fn deployment_failure_from_resolving() {
    let mut deploy = Deployment::new("app".into(), "bundle".into());
    deploy.transition(DeploymentState::Resolving).unwrap();
    deploy.transition(DeploymentState::Failed).unwrap();
    assert_eq!(deploy.state, DeploymentState::Failed);
}

#[test]
fn deployment_failure_from_starting() {
    let mut deploy = Deployment::new("app".into(), "bundle".into());
    deploy.transition(DeploymentState::Resolving).unwrap();
    deploy.transition(DeploymentState::Starting).unwrap();
    deploy.transition(DeploymentState::Failed).unwrap();
    assert_eq!(deploy.state, DeploymentState::Failed);
}

#[test]
fn deployment_invalid_transition() {
    let mut deploy = Deployment::new("app".into(), "bundle".into());
    let result = deploy.transition(DeploymentState::Running);
    assert!(matches!(result.unwrap_err(), AdminError::InvalidTransition { .. }));
}

#[test]
fn deployment_updates_timestamp_on_transition() {
    let mut deploy = Deployment::new("app".into(), "bundle".into());
    let original = deploy.updated_at.clone();
    std::thread::sleep(std::time::Duration::from_millis(10));
    deploy.transition(DeploymentState::Resolving).unwrap();
    assert!(deploy.updated_at >= original);
}

#[test]
fn deployment_serialization() {
    let deploy = Deployment::new("app-1".into(), "bundle".into());
    let json = serde_json::to_value(&deploy).unwrap();
    assert_eq!(json["state"], "PENDING");
    assert_eq!(json["app_id"], "app-1");
    assert!(json["deploy_id"].is_string());
}
