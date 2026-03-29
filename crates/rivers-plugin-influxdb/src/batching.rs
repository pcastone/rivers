//! Write-batching wrapper for InfluxConnection.

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use rivers_driver_sdk::{Connection, DriverError, Query, QueryResult};

use crate::connection::InfluxConnection;
use crate::protocol::{build_line_protocol, urlencoded};

// ── Batching Connection ───────────────────────────────────────────────

/// Write-batching wrapper for InfluxConnection.
///
/// Accumulates line protocol writes in a buffer. Flushes when:
/// - Buffer reaches max_size lines
/// - flush_interval_ms has elapsed since last flush
/// - A non-write operation is executed (query/ping)
/// - The connection is dropped
pub(crate) struct BatchingInfluxConnection {
    pub(crate) inner: InfluxConnection,
    pub(crate) buffer: Mutex<Vec<String>>,
    pub(crate) max_size: usize,
    pub(crate) flush_interval_ms: u64,
    pub(crate) last_flush: Mutex<std::time::Instant>,
}

impl BatchingInfluxConnection {
    /// Flush all buffered lines to InfluxDB in a single batch write.
    async fn flush_buffer(&self) -> Result<(), DriverError> {
        let mut buf = self.buffer.lock().await;
        if buf.is_empty() {
            return Ok(());
        }

        let batch = buf.join("\n");
        let count = buf.len();
        buf.clear();
        drop(buf);

        *self.last_flush.lock().await = std::time::Instant::now();

        debug!(lines = count, "influxdb: flushing write batch");

        let url = format!(
            "{}/api/v2/write?org={}",
            self.inner.base_url,
            urlencoded(&self.inner.org)
        );
        let resp = self
            .inner
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.inner.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(batch)
            .send()
            .await
            .map_err(|e| DriverError::Query(format!("influxdb batch write failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "influxdb batch write returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// Check if a time-based flush is needed.
    async fn should_time_flush(&self) -> bool {
        let last = self.last_flush.lock().await;
        last.elapsed().as_millis() >= self.flush_interval_ms as u128
    }
}

#[async_trait]
impl Connection for BatchingInfluxConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        match query.operation.as_str() {
            "write" | "insert" => {
                // Build line protocol and buffer it
                let line = build_line_protocol(query)?;
                let mut buf = self.buffer.lock().await;
                buf.push(line);
                let should_size_flush = buf.len() >= self.max_size;
                drop(buf);

                // Flush if buffer full or time elapsed
                if should_size_flush || self.should_time_flush().await {
                    self.flush_buffer().await?;
                }

                Ok(QueryResult {
                    rows: Vec::new(),
                    affected_rows: 1,
                    last_insert_id: None,
                })
            }
            _ => {
                // For non-write operations, flush buffer first then delegate
                self.flush_buffer().await?;
                self.inner.execute(query).await
            }
        }
    }

    async fn ping(&mut self) -> Result<(), DriverError> {
        self.flush_buffer().await?;
        self.inner.ping().await
    }

    fn driver_name(&self) -> &str {
        "influxdb"
    }
}

impl Drop for BatchingInfluxConnection {
    fn drop(&mut self) {
        // Best-effort flush on drop — can't await in drop, so log warning if buffer not empty
        let buf = self.buffer.try_lock();
        if let Ok(buf) = buf {
            if !buf.is_empty() {
                tracing::warn!(
                    lines = buf.len(),
                    "influxdb: dropping connection with unflushed write batch"
                );
            }
        }
    }
}
