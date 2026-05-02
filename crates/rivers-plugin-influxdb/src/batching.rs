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
///
/// RW4.4.d: Each buffered entry carries its target bucket. Cross-bucket
/// batching is rejected — if a write arrives for a different bucket than
/// the one already buffered, the connection returns an error. Callers
/// that need to write to multiple buckets should use separate connections.
pub(crate) struct BatchingInfluxConnection {
    pub(crate) inner: InfluxConnection,
    /// Each entry is `(bucket, line_protocol_line)`.
    pub(crate) buffer: Mutex<Vec<(String, String)>>,
    pub(crate) max_size: usize,
    pub(crate) flush_interval_ms: u64,
    pub(crate) last_flush: Mutex<std::time::Instant>,
}

impl BatchingInfluxConnection {
    /// Flush all buffered lines to InfluxDB in a single batch write.
    ///
    /// All lines in the buffer must target the same bucket (enforced at
    /// write time). If somehow mixed buckets are present (shouldn't happen),
    /// this returns an error rather than silently dropping data.
    async fn flush_buffer(&self) -> Result<(), DriverError> {
        let buf = self.buffer.lock().await;
        if buf.is_empty() {
            return Ok(());
        }

        // Verify all lines share the same bucket.
        let bucket = buf[0].0.clone();
        if buf.iter().any(|(b, _)| b != &bucket) {
            return Err(DriverError::Query(
                "influxdb: batch contains writes for multiple buckets — \
                 use separate connections per bucket"
                    .into(),
            ));
        }

        let batch: String = buf.iter().map(|(_, line)| line.as_str()).collect::<Vec<_>>().join("\n");
        let count = buf.len();
        // Do NOT clear buf yet — only clear after confirmed HTTP success so we
        // don't lose buffered writes on a transient failure.
        drop(buf);

        debug!(lines = count, bucket = %bucket, "influxdb: flushing write batch");

        let url = format!(
            "{}/api/v2/write?org={}&bucket={}",
            self.inner.base_url,
            urlencoded(&self.inner.org),
            urlencoded(&bucket),
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

        // HTTP succeeded — now it's safe to clear the buffer and update flush timestamp.
        self.buffer.lock().await.clear();
        *self.last_flush.lock().await = std::time::Instant::now();

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
                let bucket = query.target.clone();

                // RW4.4.d: Reject cross-bucket batching. If the buffer already
                // contains lines for a different bucket, fail fast rather than
                // silently mixing writes into the wrong batch URL.
                {
                    let buf = self.buffer.lock().await;
                    if let Some((existing_bucket, _)) = buf.first() {
                        if existing_bucket != &bucket {
                            return Err(DriverError::Query(format!(
                                "influxdb: cross-bucket batching is not allowed — \
                                 buffer holds writes for '{}', got write for '{}'. \
                                 Use separate connections per bucket.",
                                existing_bucket, bucket
                            )));
                        }
                    }
                }

                // Build line protocol and buffer it with its bucket.
                let line = build_line_protocol(query)?;
                let mut buf = self.buffer.lock().await;
                buf.push((bucket, line));
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
                    column_names: None,
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
