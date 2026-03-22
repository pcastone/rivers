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
    ABI_VERSION, Connection, ConnectionParams, DatabaseDriver, DriverError, DriverRegistrar,
    Query, QueryResult, QueryValue,
};

// -- Driver -----------------------------------------------------------------

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
        let port = if params.port == 0 { 389 } else { params.port };
        let url = format!("ldap://{}:{}", params.host, port);

        let settings = LdapConnSettings::new();
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
        if !params.username.is_empty() {
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

        Ok(Box::new(LdapConnection { ldap }))
    }
}

// -- Connection -------------------------------------------------------------

pub struct LdapConnection {
    ldap: Ldap,
}

#[async_trait]
impl Connection for LdapConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
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

        // Find the scope token (sub, one, base) and split around it
        let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
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
            .search(&base_dn, scope, &filter, &attr_refs)
            .await
            .map_err(|e| DriverError::Query(format!("ldap search: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap search failed: {e}")))?;

        let rows: Vec<HashMap<String, QueryValue>> = results
            .into_iter()
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

        let count = rows.len() as u64;
        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
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
            .add(&dn, attr_refs)
            .await
            .map_err(|e| DriverError::Query(format!("ldap add: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap add failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
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
            .modify(&dn, mods)
            .await
            .map_err(|e| DriverError::Query(format!("ldap modify: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap modify failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// delete — ldap.delete(dn) -> affected_rows = 1
    async fn exec_delete(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        let dn = get_param_str(&query.parameters, "dn")?;

        self.ldap
            .delete(&dn)
            .await
            .map_err(|e| DriverError::Query(format!("ldap delete: {e}")))?
            .success()
            .map_err(|e| DriverError::Query(format!("ldap delete failed: {e}")))?;

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// ping — health check via whoami extended operation
    async fn exec_ping(&mut self) -> Result<QueryResult, DriverError> {
        self.ping().await?;
        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 0,
            last_insert_id: None,
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

#[no_mangle]
pub extern "C" fn _rivers_abi_version() -> u32 {
    ABI_VERSION
}

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
        assert_eq!(_rivers_abi_version(), ABI_VERSION);
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
}
