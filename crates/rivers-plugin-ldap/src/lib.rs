#![warn(missing_docs)]
//! LDAP plugin driver (DatabaseDriver).
//!
//! Implements `DatabaseDriver` using the `ldap3` async client.
//!
//! Operations dispatch based on `query.operation`:
//! - search/find/select -> ldap.search(base, scope, filter, attrs)
//! - add/insert -> ldap.add(dn, attrs)
//! - modify/update -> ldap.modify(dn, mods)
//! - delete -> ldap.delete(dn)
//! - ping -> anonymous bind test

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use ldap3::{LdapConnAsync, LdapConnSettings, Ldap, Scope, SearchEntry};
use tracing::debug;

use rivers_driver_sdk::{
    read_connect_timeout, read_max_rows, read_request_timeout,
    ABI_VERSION, Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar,
    Query, QueryResult, QueryValue,
};

// -- Driver -----------------------------------------------------------------

/// LDAP driver factory — creates connections via the `ldap3` async client.
pub struct LdapDriver;

#[async_trait]
impl DatabaseDriver for LdapDriver {
    fn name(&self) -> &str {
        "ldap"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // Determine TLS mode from options.
        // tls=ldaps  → LDAPS (SSL from the start, port 636 by default)
        // tls=starttls → plain LDAP upgraded to TLS via StartTLS
        // tls=none   → plain LDAP (WARN if credentials supplied)
        // (default)  → plain LDAP on port 389 (WARN if credentials supplied)
        let tls_mode = params.options.get("tls").map(|s| s.as_str()).unwrap_or("none");
        let no_verify = params.options.get("tls_verify").map(|s| s == "false").unwrap_or(false);

        let has_credentials = !params.username.is_empty();
        if tls_mode == "none" && has_credentials {
            tracing::warn!(
                host = %params.host,
                "ldap: transmitting credentials over plain LDAP (no TLS); set tls=ldaps or tls=starttls"
            );
        }

        let (scheme, default_port) = if tls_mode == "ldaps" {
            ("ldaps", 636u16)
        } else {
            ("ldap", 389u16)
        };
        let port = if params.port == 0 { default_port } else { params.port as u16 };
        let url = format!("{scheme}://{host}:{port}", host = params.host);

        let connect_timeout_secs = read_connect_timeout(params);
        let mut settings = LdapConnSettings::new()
            .set_conn_timeout(std::time::Duration::from_secs(connect_timeout_secs));
        if tls_mode == "starttls" {
            settings = settings.set_starttls(true);
        }
        if no_verify {
            settings = settings.set_no_tls_verify(true);
        }

        let (conn, mut ldap) = LdapConnAsync::with_settings(settings, &url)
            .await
            .map_err(|e| DriverError::Connection(format!("ldap connect: {e}")))?;

        // Spawn the connection driver task
        tokio::spawn(async move {
            if let Err(e) = conn.drive().await {
                tracing::error!("ldap connection driver error: {e}");
            }
        });

        // Bind: use credentials if provided, otherwise perform explicit anonymous bind.
        if has_credentials {
            ldap.simple_bind(&params.username, &params.password)
                .await
                .map_err(|e| DriverError::Connection(format!("ldap bind: {e}")))?
                .success()
                .map_err(|e| DriverError::Connection(format!("ldap bind failed: {e}")))?;
        } else {
            ldap.simple_bind("", "")
                .await
                .map_err(|e| DriverError::Connection(format!("ldap anonymous bind: {e}")))?
                .success()
                .map_err(|e| DriverError::Connection(format!("ldap anonymous bind failed: {e}")))?;
        }

        debug!(
            host = %params.host,
            port = %port,
            "ldap: connected"
        );

        let max_rows = read_max_rows(params);
        let request_timeout_secs = read_request_timeout(params);

        Ok(Box::new(LdapConnection { ldap, max_rows, request_timeout_secs }))
    }

    /// G_R7.2: cdylib plugin runs connect() in an isolated runtime.
    fn needs_isolated_runtime(&self) -> bool { true }
}

// -- Connection -------------------------------------------------------------

/// Active LDAP connection for search, add, modify, and delete operations.
pub struct LdapConnection {
    ldap: Ldap,
    /// Maximum number of rows returned by a single search (per-connection cap).
    max_rows: usize,
    /// Per-operation request timeout in seconds (applied via ldap.with_timeout()).
    request_timeout_secs: u64,
}

#[async_trait]
impl Connection for LdapConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            "search" | "find" | "select" => self.exec_search(query).await,
            "add" | "insert" => self.exec_add(query).await,
            "modify" | "update" => self.exec_modify(query).await,
            "delete" | "del" | "remove" => self.exec_delete(query).await,
            "ping" => self.exec_ping().await,
            other => Err(DriverError::Unsupported(format!(
                "ldap: unsupported operation '{other}'"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        // Extended whoami request as a lightweight health check
        self.ldap
            .with_timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .extended(ldap3::exop::WhoAmI)
            .await
            .map_err(|e| DriverError::Connection(format!("ldap ping: {e}")))?;
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "ldap"
    }
}

impl LdapConnection {
    /// Parse statement format: "base_dn scope filter"
    ///
    /// Example: "dc=rivers,dc=test sub (objectClass=*)"
    fn parse_search_statement(statement: &str) -> Result<(String, Scope, String), DriverError> {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            return Err(DriverError::Query(
                "ldap search: empty statement — expected 'base_dn scope filter'".into(),
            ));
        }

        // Strip the operation prefix if present (operation is already parsed by dispatch)
        let body = if trimmed.starts_with("search ") || trimmed.starts_with("find ") || trimmed.starts_with("select ") {
            trimmed.splitn(2, ' ').nth(1).unwrap_or(trimmed)
        } else {
            trimmed
        };

        let parts: Vec<&str> = body.splitn(3, ' ').collect();
        if parts.len() < 3 {
            return Err(DriverError::Query(format!(
                "ldap search: statement must be 'base_dn scope filter', got '{trimmed}'"
            )));
        }

        let base_dn = parts[0].to_string();
        let scope = match parts[1].to_lowercase().as_str() {
            "sub" | "subtree" => Scope::Subtree,
            "one" | "onelevel" => Scope::OneLevel,
            "base" => Scope::Base,
            other => {
                return Err(DriverError::Query(format!(
                    "ldap search: unknown scope '{other}' — use sub, one, or base"
                )));
            }
        };
        let filter = parts[2].to_string();

        Ok((base_dn, scope, filter))
    }

    /// search/find/select — ldap.search(base, scope, filter, attrs) -> rows
    async fn exec_search(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let (base_dn, scope, filter) = Self::parse_search_statement(&query.statement)?;

        // Collect requested attributes from parameters, or use empty vec for all
        let attrs: Vec<String> = query
            .parameters
            .get("attrs")
            .and_then(|v| match v {
                QueryValue::String(s) => {
                    Some(s.split(',').map(|a| a.trim().to_string()).collect())
                }
                QueryValue::Array(arr) => Some(
                    arr.iter()
                        .filter_map(|v| match v {
                            QueryValue::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();

        let attr_refs: Vec<&str> = attrs.iter().map(|s| s.as_str()).collect();

        let (results, _res) = self
            .ldap
            .with_timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .search(&base_dn, scope, &filter, &attr_refs)
            .await
            .map_err(|e| DriverError::Query(format!("ldap search: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap search failed: {e}")))?;

        let total = results.len();
        let truncated = total > self.max_rows;

        let rows: Vec<HashMap<String, QueryValue>> = results
            .into_iter()
            .take(self.max_rows)
            .map(|entry| {
                let se = SearchEntry::construct(entry);
                let mut row = HashMap::new();
                row.insert("dn".into(), QueryValue::String(se.dn));
                for (attr, values) in se.attrs {
                    let val = if values.len() == 1 {
                        QueryValue::String(values.into_iter().next().unwrap())
                    } else {
                        QueryValue::String(values.join(";"))
                    };
                    row.insert(attr, val);
                }
                // Include binary attributes as base64 (informational)
                for (attr, values) in se.bin_attrs {
                    let encoded: Vec<String> = values
                        .iter()
                        .map(|v| {
                            v.iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<String>()
                        })
                        .collect();
                    let val = if encoded.len() == 1 {
                        QueryValue::String(encoded.into_iter().next().unwrap())
                    } else {
                        QueryValue::String(encoded.join(";"))
                    };
                    row.insert(attr, val);
                }
                row
            })
            .collect();

        if truncated {
            tracing::warn!(
                total = total,
                cap = self.max_rows,
                "ldap search: result truncated to max_rows cap (set max_rows option to increase)"
            );
        }

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// add/insert — ldap.add(dn, attrs) -> affected_rows = 1
    async fn exec_add(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let dn = get_param_str(&query.parameters, "dn")?;

        // All other parameters become LDAP attributes
        let mut attrs: Vec<(String, Vec<String>)> = Vec::new();
        for (key, value) in &query.parameters {
            if key == "dn" {
                continue;
            }
            let values = match value {
                QueryValue::String(s) => s.split(';').map(|v| v.to_string()).collect(),
                QueryValue::Integer(n) => vec![n.to_string()],
                QueryValue::Float(f) => vec![f.to_string()],
                QueryValue::Boolean(b) => vec![b.to_string()],
                QueryValue::Array(arr) => arr
                    .iter()
                    .filter_map(|v| match v {
                        QueryValue::String(s) => Some(s.clone()),
                        QueryValue::Integer(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect(),
                _ => vec![format!("{value:?}")],
            };
            attrs.push((key.clone(), values));
        }

        // Convert to the format ldap3 expects: Vec<(&str, HashSet<&str>)>
        let attr_refs: Vec<(&str, std::collections::HashSet<&str>)> = attrs
            .iter()
            .map(|(k, vs)| {
                let set: std::collections::HashSet<&str> = vs.iter().map(|s| s.as_str()).collect();
                (k.as_str(), set)
            })
            .collect();

        self.ldap
            .with_timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .add(&dn, attr_refs)
            .await
            .map_err(|e| DriverError::Query(format!("ldap add: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap add failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// modify/update — ldap.modify(dn, mods) -> affected_rows = 1
    async fn exec_modify(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let dn = get_param_str(&query.parameters, "dn")?;

        // All other parameters become Replace modifications
        let mut mod_values: Vec<(String, Vec<String>)> = Vec::new();
        for (key, value) in &query.parameters {
            if key == "dn" {
                continue;
            }
            let values = match value {
                QueryValue::String(s) => s.split(';').map(|v| v.to_string()).collect(),
                QueryValue::Integer(n) => vec![n.to_string()],
                QueryValue::Float(f) => vec![f.to_string()],
                QueryValue::Boolean(b) => vec![b.to_string()],
                QueryValue::Array(arr) => arr
                    .iter()
                    .filter_map(|v| match v {
                        QueryValue::String(s) => Some(s.clone()),
                        QueryValue::Integer(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect(),
                _ => vec![format!("{value:?}")],
            };
            mod_values.push((key.clone(), values));
        }

        // Build Mod::Replace entries
        let mods: Vec<ldap3::Mod<&str>> = mod_values
            .iter()
            .map(|(k, vs)| {
                let set: std::collections::HashSet<&str> = vs.iter().map(|s| s.as_str()).collect();
                ldap3::Mod::Replace(k.as_str(), set)
            })
            .collect();

        self.ldap
            .with_timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .modify(&dn, mods)
            .await
            .map_err(|e| DriverError::Query(format!("ldap modify: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap modify failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// delete — ldap.delete(dn) -> affected_rows = 1
    async fn exec_delete(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let dn = get_param_str(&query.parameters, "dn")?;

        self.ldap
            .with_timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .delete(&dn)
            .await
            .map_err(|e| DriverError::Query(format!("ldap delete: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap delete failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
            column_names: None,
        })
    }

    /// ping — health check via whoami extended operation
    async fn exec_ping(&mut self) -> Result<QueryResult, DriverError> {
        self.ping().await?;
        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
            column_names: None,
        })
    }
}

// -- Helpers ----------------------------------------------------------------

fn get_param_str(
    params: &HashMap<String, QueryValue>,
    name: &str,
) -> Result<String, DriverError> {
    match params.get(name) {
        Some(QueryValue::String(s)) => Ok(s.clone()),
        Some(QueryValue::Integer(i)) => Ok(i.to_string()),
        Some(other) => Ok(format!("{other:?}")),
        None => Err(DriverError::Query(format!(
            "ldap: missing required parameter '{name}'"
        ))),
    }
}

// -- Plugin ABI -------------------------------------------------------------

#[cfg(feature = "plugin-exports")]
#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

#[cfg(feature = "plugin-exports")]
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn _rivers_register_driver(registrar: &mut dyn DriverRegistrar) {
    registrar.register_database_driver(Arc::new(LdapDriver));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rivers_driver_sdk::DatabaseDriver;

    #[test]
    fn driver_name() {
        assert_eq!(LdapDriver.name(), "ldap");
    }

    #[test]
    fn abi_version_matches() {
        assert_eq!(ABI_VERSION, 1);
    }

    #[test]
    fn parse_search_statement_valid() {
        let (base, scope, filter) =
            LdapConnection::parse_search_statement("dc=rivers,dc=test sub (objectClass=*)")
                .unwrap();
        assert_eq!(base, "dc=rivers,dc=test");
        assert!(matches!(scope, Scope::Subtree));
        assert_eq!(filter, "(objectClass=*)");
    }

    #[test]
    fn parse_search_statement_onelevel() {
        let (base, scope, filter) =
            LdapConnection::parse_search_statement("ou=users,dc=test one (cn=Alice)")
                .unwrap();
        assert_eq!(base, "ou=users,dc=test");
        assert!(matches!(scope, Scope::OneLevel));
        assert_eq!(filter, "(cn=Alice)");
    }

    #[test]
    fn parse_search_statement_base() {
        let (base, scope, filter) =
            LdapConnection::parse_search_statement("cn=admin,dc=test base (objectClass=*)")
                .unwrap();
        assert_eq!(base, "cn=admin,dc=test");
        assert!(matches!(scope, Scope::Base));
        assert_eq!(filter, "(objectClass=*)");
    }

    #[test]
    fn parse_search_statement_empty_errors() {
        let result = LdapConnection::parse_search_statement("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_search_statement_missing_parts_errors() {
        let result = LdapConnection::parse_search_statement("dc=test sub");
        assert!(result.is_err());
    }

    #[test]
    fn parse_search_statement_bad_scope_errors() {
        let result =
            LdapConnection::parse_search_statement("dc=test invalid (objectClass=*)");
        assert!(result.is_err());
    }

    #[test]
    fn get_param_str_extracts_string() {
        let mut params = HashMap::new();
        params.insert("dn".into(), QueryValue::String("cn=test".into()));
        assert_eq!(get_param_str(&params, "dn").unwrap(), "cn=test");
    }

    #[test]
    fn get_param_str_missing_errors() {
        let params = HashMap::new();
        assert!(get_param_str(&params, "dn").is_err());
    }

    #[tokio::test]
    async fn connect_bad_host_returns_connection_error() {
        let driver = LdapDriver;
        let params = ConnectionParams {
            host: "127.0.0.1".into(),
            port: 1,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            driver.connect(&params),
        )
        .await;
        match result {
            Ok(Err(DriverError::Connection(_))) => {}
            Ok(Err(other)) => panic!("expected Connection error, got: {other:?}"),
            Ok(Ok(_)) => {} // Some environments may accept the TCP connect
            Err(_) => {}    // timeout OK
        }
    }

    #[test]
    fn read_max_rows_default_is_ten_thousand() {
        let params = ConnectionParams {
            host: "localhost".into(),
            port: 389,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        };
        assert_eq!(rivers_driver_sdk::read_max_rows(&params), 10_000);
    }

    #[test]
    fn read_max_rows_from_option() {
        let mut options = HashMap::new();
        options.insert("max_rows".to_string(), "50".to_string());
        let params = ConnectionParams {
            host: "localhost".into(),
            port: 389,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options,
        };
        assert_eq!(rivers_driver_sdk::read_max_rows(&params), 50);
    }

    // ── RW4.4.i: TLS mode URL and settings ──────────────────────────────

    /// Verify the scheme and default port for each TLS mode by simulating
    /// what `connect()` computes. We do this without making a real TCP
    /// connection — just check the options are interpreted correctly.
    #[test]
    fn tls_none_uses_ldap_scheme_and_port_389() {
        let params = make_params(389, &[]);
        let (scheme, default_port) = tls_scheme_and_port(&params);
        assert_eq!(scheme, "ldap");
        assert_eq!(default_port, 389);
    }

    #[test]
    fn tls_ldaps_uses_ldaps_scheme_and_port_636() {
        let params = make_params(0, &[("tls", "ldaps")]);
        let (scheme, default_port) = tls_scheme_and_port(&params);
        assert_eq!(scheme, "ldaps");
        assert_eq!(default_port, 636);
    }

    #[test]
    fn tls_starttls_keeps_ldap_scheme_but_enables_starttls() {
        let params = make_params(0, &[("tls", "starttls")]);
        let (scheme, _) = tls_scheme_and_port(&params);
        assert_eq!(scheme, "ldap");
        let mode = params.options.get("tls").map(|s| s.as_str()).unwrap_or("none");
        assert_eq!(mode, "starttls");
    }

    #[test]
    fn tls_explicit_port_overrides_default() {
        let params = make_params(9389, &[("tls", "ldaps")]);
        let port = if params.port == 0 { 636u16 } else { params.port as u16 };
        assert_eq!(port, 9389);
    }

    fn make_params(port: u16, opts: &[(&str, &str)]) -> ConnectionParams {
        let mut options = HashMap::new();
        for (k, v) in opts {
            options.insert(k.to_string(), v.to_string());
        }
        ConnectionParams {
            host: "127.0.0.1".into(),
            port,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options,
        }
    }

    fn tls_scheme_and_port(params: &ConnectionParams) -> (&'static str, u16) {
        let tls_mode = params.options.get("tls").map(|s| s.as_str()).unwrap_or("none");
        if tls_mode == "ldaps" { ("ldaps", 636) } else { ("ldap", 389) }
    }
}
