# Tutorial: Monitoring with Prometheus

**Rivers v0.53.0**

## Overview

Rivers includes a built-in Prometheus metrics exporter that exposes HTTP request counters, latency histograms, engine execution metrics, and runtime gauges on a dedicated scrape endpoint. Enabling it requires two lines of TOML configuration.

The metrics endpoint runs on a separate port from the main application server. This keeps scrape traffic isolated from production API traffic and lets you restrict access with firewall rules without affecting the application.

Use Prometheus metrics when you need dashboards for request rates, latency percentiles, error rates, engine performance, or active connection counts. The tutorial covers enabling metrics, scraping the endpoint, understanding the available metrics, and pointing Grafana at the data.

## Prerequisites

- A running Rivers instance (see the [Getting Started tutorial](tutorial-getting-started.md))
- Prometheus installed (for scraping)
- Grafana installed (optional -- for dashboards)

---

## Step 1: Enable Metrics in riversd.toml

Add the `[metrics]` section to your `riversd.toml`:

```toml
[metrics]
enabled = true
port    = 9091
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable Prometheus metrics collection and scrape endpoint |
| `port` | integer | `9091` | Port for the HTTP scrape endpoint |

The metrics endpoint listens on all interfaces (`0.0.0.0`) on the configured port. It serves plain HTTP (not TLS) -- this is standard for Prometheus scrape targets.

Restart Rivers after editing the config:

```bash
/opt/rivers/bin/riversctl stop
/opt/rivers/bin/riversctl start
```

You should see a log entry confirming metrics are active:

```
INFO metrics endpoint listening on 0.0.0.0:9091
```

---

## Step 2: Verify the Metrics Endpoint

Scrape the metrics endpoint with curl:

```bash
curl http://localhost:9091/metrics
```

If no requests have been made yet, you will see the gauge metrics with initial values:

```
# HELP rivers_active_connections Current number of active connections
# TYPE rivers_active_connections gauge
rivers_active_connections 0

# HELP rivers_loaded_apps Number of loaded applications
# TYPE rivers_loaded_apps gauge
rivers_loaded_apps 1
```

---

## Step 3: Generate Traffic and Scrape

Make a few requests to your API to generate metric data:

```bash
curl -k https://localhost:8080/my-api/my-api/items
curl -k https://localhost:8080/my-api/my-api/items?limit=5
curl -k https://localhost:8080/my-api/my-api/items?limit=10
curl -k https://localhost:8080/nonexistent/path
```

Now scrape again:

```bash
curl http://localhost:9091/metrics
```

Example output:

```
# HELP rivers_http_requests_total Total HTTP requests by method and status
# TYPE rivers_http_requests_total counter
rivers_http_requests_total{method="GET",status="200"} 3
rivers_http_requests_total{method="GET",status="404"} 1

# HELP rivers_http_request_duration_ms HTTP request duration in milliseconds
# TYPE rivers_http_request_duration_ms histogram
rivers_http_request_duration_ms_bucket{method="GET",le="1"} 0
rivers_http_request_duration_ms_bucket{method="GET",le="5"} 2
rivers_http_request_duration_ms_bucket{method="GET",le="10"} 3
rivers_http_request_duration_ms_bucket{method="GET",le="25"} 3
rivers_http_request_duration_ms_bucket{method="GET",le="50"} 3
rivers_http_request_duration_ms_bucket{method="GET",le="100"} 3
rivers_http_request_duration_ms_bucket{method="GET",le="+Inf"} 3
rivers_http_request_duration_ms_sum{method="GET"} 12.456
rivers_http_request_duration_ms_count{method="GET"} 3

# HELP rivers_engine_executions_total Total engine executions by engine and success
# TYPE rivers_engine_executions_total counter
rivers_engine_executions_total{engine="v8",success="true"} 3

# HELP rivers_engine_execution_duration_ms Engine execution duration in milliseconds
# TYPE rivers_engine_execution_duration_ms histogram
rivers_engine_execution_duration_ms_bucket{engine="v8",le="1"} 1
rivers_engine_execution_duration_ms_bucket{engine="v8",le="5"} 3
rivers_engine_execution_duration_ms_bucket{engine="v8",le="+Inf"} 3
rivers_engine_execution_duration_ms_sum{engine="v8"} 7.234
rivers_engine_execution_duration_ms_count{engine="v8"} 3

# HELP rivers_active_connections Current number of active connections
# TYPE rivers_active_connections gauge
rivers_active_connections 0

# HELP rivers_loaded_apps Number of loaded applications
# TYPE rivers_loaded_apps gauge
rivers_loaded_apps 1
```

---

## Step 4: Available Metrics

Rivers exposes six metrics. Here is the complete reference:

### Counters

| Metric | Labels | Description |
|--------|--------|-------------|
| `rivers_http_requests_total` | `method`, `status` | Total HTTP requests. Incremented on every completed request. Labels include the HTTP method (`GET`, `POST`, etc.) and the response status code (`200`, `404`, `500`, etc.). |
| `rivers_engine_executions_total` | `engine`, `success` | Total CodeComponent engine executions. The `engine` label is `v8` or `wasmtime`. The `success` label is `true` or `false`. |

### Histograms

| Metric | Labels | Description |
|--------|--------|-------------|
| `rivers_http_request_duration_ms` | `method` | HTTP request duration in milliseconds, measured from request received to response sent. Use `histogram_quantile()` in PromQL for percentiles. |
| `rivers_engine_execution_duration_ms` | `engine` | CodeComponent execution duration in milliseconds. Measures the time spent inside the V8 or WASM engine for a single handler invocation. |

### Gauges

| Metric | Labels | Description |
|--------|--------|-------------|
| `rivers_active_connections` | (none) | Current number of active TCP connections to the server. |
| `rivers_loaded_apps` | (none) | Number of applications currently loaded from the bundle. |

---

## Step 5: Configure Prometheus to Scrape Rivers

Add a scrape job to your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'rivers'
    scrape_interval: 15s
    static_configs:
      - targets: ['localhost:9091']
        labels:
          instance: 'rivers-prod'
```

If Rivers runs on a different host, replace `localhost` with the host address. The metrics port (9091) serves plain HTTP.

Reload Prometheus:

```bash
# If running as a service
systemctl reload prometheus

# Or send SIGHUP
kill -HUP $(pidof prometheus)
```

Verify the target is healthy in the Prometheus UI at `http://localhost:9090/targets`. The `rivers` job should show as `UP`.

---

## Step 6: Grafana Dashboard Setup

### Add the data source

1. Open Grafana (default: `http://localhost:3000`)
2. Go to **Configuration** > **Data Sources** > **Add data source**
3. Select **Prometheus**
4. Set the URL to `http://localhost:9090` (your Prometheus server, not the Rivers metrics port)
5. Click **Save & Test**

### Useful PromQL queries

**Request rate (requests per second):**

```promql
rate(rivers_http_requests_total[5m])
```

**Request rate by status code:**

```promql
sum by (status) (rate(rivers_http_requests_total[5m]))
```

**Error rate (4xx + 5xx):**

```promql
sum(rate(rivers_http_requests_total{status=~"4..|5.."}[5m]))
```

**P95 request latency:**

```promql
histogram_quantile(0.95, rate(rivers_http_request_duration_ms_bucket[5m]))
```

**P99 request latency:**

```promql
histogram_quantile(0.99, rate(rivers_http_request_duration_ms_bucket[5m]))
```

**Active connections:**

```promql
rivers_active_connections
```

**Engine execution success rate:**

```promql
sum(rate(rivers_engine_executions_total{success="true"}[5m]))
/
sum(rate(rivers_engine_executions_total[5m]))
```

**Engine P95 latency:**

```promql
histogram_quantile(0.95, rate(rivers_engine_execution_duration_ms_bucket[5m]))
```

### Suggested dashboard panels

| Panel | Type | Query |
|-------|------|-------|
| Request Rate | Graph | `rate(rivers_http_requests_total[5m])` |
| Error Rate | Graph | `sum(rate(rivers_http_requests_total{status=~"5.."}[5m]))` |
| P95 Latency | Graph | `histogram_quantile(0.95, rate(rivers_http_request_duration_ms_bucket[5m]))` |
| Active Connections | Stat | `rivers_active_connections` |
| Loaded Apps | Stat | `rivers_loaded_apps` |
| Engine Success Rate | Gauge | Success rate query above |
| Engine Latency | Graph | `histogram_quantile(0.95, rate(rivers_engine_execution_duration_ms_bucket[5m]))` |

---

## Summary

This tutorial covered:

1. Enabling Prometheus metrics with `[metrics] enabled = true` in `riversd.toml`
2. Scraping the metrics endpoint at `http://localhost:9091/metrics`
3. Understanding the six available metrics: two counters, two histograms, and two gauges
4. Configuring Prometheus to scrape Rivers
5. Setting up Grafana with PromQL queries for request rates, latency percentiles, error rates, and engine performance
