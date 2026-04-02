//! InfluxDB connection — executes queries, writes, and pings via HTTP.

use async_trait::async_trait;
use reqwest::Client;

use rivers_driver_sdk::{Connection, DriverError, Query, QueryResult};

use crate::protocol::{build_line_protocol, parse_csv_response, urlencoded};

// ── Connection ─────────────────────────────────────────────────────────

/// Active InfluxDB v2 connection for executing Flux queries and line protocol writes.
pub struct InfluxConnection {
    pub(crate) client: Client,
    pub(crate) base_url: String,
    pub(crate) org: String,
    pub(crate) token: String,
}

#[async_trait]
impl Connection for InfluxConnection {
    fn admin_operations(&self) -> &[&str] {
        &["create_bucket", "delete_bucket"]
    }

    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Gate 1: DDL/admin operation guard
        if let Some(reason) = rivers_driver_sdk::check_admin_guard(query, self.admin_operations()) {
            return Err(DriverError::Forbidden(format!("{reason} — use application init handler")));
        }

        match query.operation.as_str() {
            "query" | "select" | "find" => self.exec_query(query).await,
            "write" | "insert" => self.exec_write(query).await,
            "ping" => self.exec_ping().await,
            other => Err(DriverError::Unsupported(format!(
                "influxdb: unsupported operation '{other}'"
            ))),
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        let resp = self
            .client
            .get(format!("{}/ping", self.base_url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Query(format!(
                "influxdb ping returned status {}",
                resp.status()
            )));
        }
        Ok(())
    }

    fn driver_name(&self) -> &str {
        "influxdb"
    }
}

impl InfluxConnection {
    /// POST /api/v2/query — execute a Flux query.
    ///
    /// The Flux query comes from `query.statement` directly.
    /// The response is annotated CSV which we parse into rows.
    async fn exec_query(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let url = format!(
            "{}/api/v2/query?org={}",
            self.base_url,
            urlencoded(&self.org)
        );

        // InfluxDB v2 query API accepts Flux as application/vnd.flux
        // or as JSON with dialect. We use the simpler vnd.flux content type.
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "application/vnd.flux")
            .header("Accept", "text/csv")
            .body(query.statement.clone())
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb query failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "influxdb query returned {status}: {text}"
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb response read failed: {e}")))?;

        let rows = parse_csv_response(&body);
        let count = rows.len() as u64;

        Ok(QueryResult {
            rows,
            affected_rows: count,
            last_insert_id: None,
        })
    }

    /// POST /api/v2/write — write data in line protocol format.
    ///
    /// Constructs line protocol from query parameters:
    /// - `measurement` key -> measurement name (falls back to query.target)
    /// - `tags` key (JSON object) -> tag set
    /// - `fields` key (JSON object) -> field set
    /// - `timestamp` key -> optional nanosecond timestamp
    ///
    /// Alternatively, if `_line_protocol` is set, it is sent raw.
    async fn exec_write(&self, query: &Query) -> Result<QueryResult, DriverError> {
        let bucket = &query.target;
        let url = format!(
            "{}/api/v2/write?org={}&bucket={}",
            self.base_url,
            urlencoded(&self.org),
            urlencoded(bucket)
        );

        let line = build_line_protocol(query)?;

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(line)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb write failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "influxdb write returned {status}: {text}"
            )));
        }

        Ok(QueryResult {
            rows: Vec::new(),
            affected_rows: 1,
            last_insert_id: None,
        })
    }

    /// GET /ping
    async fn exec_ping(&self) -> Result<QueryResult, DriverError> {
        let resp = self
            .client
            .get(format!("{}/ping", self.base_url))
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb ping failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DriverError::Query(format!(
                "influxdb ping returned status {}",
                resp.status()
            )));
        }
        Ok(QueryResult::empty())
    }
}
