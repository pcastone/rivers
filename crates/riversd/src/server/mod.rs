//! HTTP server entry point and router construction.
//!
//! Per `rivers-httpd-spec.md` §1-3, §13.

mod context;
mod router;
mod view_dispatch;
mod streaming;
mod handlers;
mod admin_auth;
mod drivers;
mod lifecycle;
#[cfg(feature = "metrics")]
pub mod metrics;
mod validation;

// ── Re-exports — preserve `crate::server::*` import paths ────────

pub use context::{LogController, AppContext};
pub use router::{build_main_router, build_admin_router};
pub use drivers::register_all_drivers;
pub use lifecycle::{
    maybe_spawn_http_redirect_server,
    run_http_redirect_server,
    run_server_no_ssl,
    run_server_with_listener_with_control,
    run_server_with_listener_and_log,
};
pub use validation::{
    validate_admin_access_control,
    validate_server_tls,
    shutdown_signal,
    ServerError,
};

// ── Unit Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn run_server_rejects_missing_tls_config() {
        use rivers_runtime::rivers_core::ServerConfig;
        let config = ServerConfig::default();
        let result = super::validate_server_tls(&config, false);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("TLS is required") || msg.contains("[base.tls]"));
    }

    #[test]
    fn run_server_skips_tls_validation_with_no_ssl() {
        use rivers_runtime::rivers_core::ServerConfig;
        let config = ServerConfig::default();
        let result = super::validate_server_tls(&config, true);
        assert!(result.is_ok());
    }

    #[test]
    fn admin_access_control_rejects_no_public_key() {
        use rivers_runtime::rivers_core::config::{AdminApiConfig, AdminTlsConfig};
        let mut admin = AdminApiConfig::default();
        admin.enabled = true;
        admin.public_key = None;
        admin.tls = Some(AdminTlsConfig {
            server_cert: None,
            server_key: None,
            ca_cert: None,
            require_client_cert: false,
        });
        let result = super::validate_admin_access_control(&admin);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("public_key"));
    }

    #[test]
    fn admin_access_control_accepts_with_public_key() {
        use rivers_runtime::rivers_core::config::{AdminApiConfig, AdminTlsConfig};
        let mut admin = AdminApiConfig::default();
        admin.enabled = true;
        admin.public_key = Some("/etc/admin.pub".to_string());
        admin.tls = Some(AdminTlsConfig {
            server_cert: None, server_key: None, ca_cert: None,
            require_client_cert: false,
        });
        let result = super::validate_admin_access_control(&admin);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn redirect_server_responds_with_301() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(super::run_http_redirect_server(
            listener,
            443,
            shutdown_rx,
        ));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/foo?bar=1"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), 301);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("https://"), "location was: {location}");
        assert!(location.contains("/foo?bar=1"), "location was: {location}");

        let _ = shutdown_tx.send(true);
        handle.abort();
    }
}
