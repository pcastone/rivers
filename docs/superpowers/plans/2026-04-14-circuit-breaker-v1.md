# Circuit Breaker v1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add app-level manual circuit breakers that let operators trip/reset groups of DataViews via CLI and admin API, with state persisted across restarts.

**Architecture:** A `BreakerRegistry` (per-app `HashMap<String, BreakerEntry>` behind `Arc<RwLock>`) is built from DataView config at bundle load time. State is persisted via StorageEngine. The dispatch pipeline checks the registry before pool acquisition. Admin API endpoints expose trip/reset/status. riversctl provides the CLI interface.

**Tech Stack:** Rust, Axum (admin API routes), serde (config), tokio RwLock (concurrency), StorageEngine (persistence)

**Spec:** `docs/arch/rivers-circuit-breaker-spec.md`

---

## File Map

| Task | File | Action |
|------|------|--------|
| 1 | `crates/rivers-runtime/src/dataview.rs` | Modify — add `circuit_breaker_id` field |
| 1 | `crates/rivers-runtime/src/validate_structural.rs` | Modify — add to known fields |
| 2 | `crates/riversd/src/circuit_breaker.rs` | Create — BreakerRegistry, BreakerEntry, BreakerState |
| 3 | `crates/riversd/src/server/context.rs` | Modify — add registry to AppContext |
| 3 | `crates/riversd/src/bundle_loader/load.rs` | Modify — build registry at bundle load |
| 4 | `crates/riversd/src/view_engine/pipeline.rs` | Modify — breaker check before dispatch |
| 5 | `crates/riversd/src/admin_handlers.rs` | Modify — add breaker endpoints |
| 5 | `crates/riversd/src/server/router.rs` | Modify — register breaker routes |
| 6 | `crates/riversctl/src/commands/admin.rs` | Modify — add breaker commands |
| 6 | `crates/riversctl/src/main.rs` | Modify — add CLI dispatch |
| 7 | `crates/rivers-runtime/src/validate_crossref.rs` | Modify — solo breaker ID warning |

---

### Task 1: Config Schema — Add `circuitBreakerId` to DataView

**Files:**
- Modify: `crates/rivers-runtime/src/dataview.rs:176`
- Modify: `crates/rivers-runtime/src/validate_structural.rs:68-75`

- [ ] **Step 1: Add field to DataViewConfig**

In `crates/rivers-runtime/src/dataview.rs`, find the `DataViewConfig` struct. After the `streaming` field (around line 176), add:

```rust
    #[serde(default)]
    pub circuit_breaker_id: Option<String>,
```

- [ ] **Step 2: Add to DATAVIEW_FIELDS in structural validation**

In `crates/rivers-runtime/src/validate_structural.rs`, find the `DATAVIEW_FIELDS` constant (line 68). Add `"circuitBreakerId"` to the array:

```rust
const DATAVIEW_FIELDS: &[&str] = &[
    "name", "datasource", "query", "parameters", "caching", "max_rows",
    "invalidates", "get_schema", "post_schema", "put_schema", "delete_schema",
    "get_query", "post_query", "put_query", "delete_query",
    "return_schema", "get_parameters", "post_parameters", "put_parameters",
    "delete_parameters", "streaming", "validate_result", "strict_parameters",
    "circuitBreakerId",
];
```

Note: The TOML key is `circuitBreakerId` (camelCase, matching Rivers config convention). The Rust field is `circuit_breaker_id` (snake_case). Serde handles the mapping since `circuitBreakerId` in TOML deserializes to `circuit_breaker_id` in Rust — verify this works, or add `#[serde(rename = "circuitBreakerId")]` if needed.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rivers-runtime`
Expected: Compiles. No existing code references the new field yet.

- [ ] **Step 4: Write a test for deserialization**

In `crates/rivers-runtime/src/dataview.rs`, add to the existing test module (or create one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataview_config_parses_circuit_breaker_id() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
            circuitBreakerId = "Warehouse_Transaction"
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.circuit_breaker_id.as_deref(), Some("Warehouse_Transaction"));
    }

    #[test]
    fn dataview_config_circuit_breaker_id_optional() {
        let toml_str = r#"
            name = "test"
            datasource = "ds"
        "#;
        let cfg: DataViewConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.circuit_breaker_id.is_none());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rivers-runtime -- dataview_config`
Expected: Both tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-runtime/src/dataview.rs crates/rivers-runtime/src/validate_structural.rs
git commit -m "feat(circuit-breaker): add circuitBreakerId field to DataViewConfig"
```

---

### Task 2: BreakerRegistry Module

**Files:**
- Create: `crates/riversd/src/circuit_breaker.rs`

- [ ] **Step 1: Write tests first**

Create `crates/riversd/src/circuit_breaker.rs` with the types and test module:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BreakerState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub struct BreakerEntry {
    #[serde(rename = "breakerId")]
    pub breaker_id: String,
    pub state: BreakerState,
    pub dataviews: Vec<String>,
}

pub struct BreakerRegistry {
    breakers: RwLock<HashMap<String, BreakerEntry>>,
}

impl BreakerRegistry {
    pub fn new() -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, breaker_id: String, dataview_name: String) {
        let mut map = self.breakers.write().await;
        let entry = map.entry(breaker_id.clone()).or_insert_with(|| BreakerEntry {
            breaker_id,
            state: BreakerState::Closed,
            dataviews: Vec::new(),
        });
        if !entry.dataviews.contains(&dataview_name) {
            entry.dataviews.push(dataview_name);
        }
    }

    pub async fn is_open(&self, breaker_id: &str) -> bool {
        let map = self.breakers.read().await;
        map.get(breaker_id)
            .map(|e| e.state == BreakerState::Open)
            .unwrap_or(false)
    }

    pub async fn trip(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = BreakerState::Open;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub async fn reset(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = BreakerState::Closed;
            Some(entry.clone())
        } else {
            None
        }
    }

    pub async fn get(&self, breaker_id: &str) -> Option<BreakerEntry> {
        let map = self.breakers.read().await;
        map.get(breaker_id).cloned()
    }

    pub async fn list(&self) -> Vec<BreakerEntry> {
        let map = self.breakers.read().await;
        let mut entries: Vec<BreakerEntry> = map.values().cloned().collect();
        entries.sort_by(|a, b| a.breaker_id.cmp(&b.breaker_id));
        entries
    }

    pub async fn set_state(&self, breaker_id: &str, state: BreakerState) {
        let mut map = self.breakers.write().await;
        if let Some(entry) = map.get_mut(breaker_id) {
            entry.state = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_creates_closed_breaker() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert_eq!(entry.dataviews, vec!["search_orders"]);
    }

    #[tokio::test]
    async fn register_adds_dataview_to_existing_breaker() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.register("WH_TX".into(), "update_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.dataviews, vec!["search_orders", "update_orders"]);
    }

    #[tokio::test]
    async fn register_deduplicates_dataviews() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.get("WH_TX").await.unwrap();
        assert_eq!(entry.dataviews.len(), 1);
    }

    #[tokio::test]
    async fn trip_sets_state_to_open() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.trip("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
        assert!(reg.is_open("WH_TX").await);
    }

    #[tokio::test]
    async fn reset_sets_state_to_closed() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.trip("WH_TX").await;
        let entry = reg.reset("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
        assert!(!reg.is_open("WH_TX").await);
    }

    #[tokio::test]
    async fn trip_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        reg.trip("WH_TX").await;
        let entry = reg.trip("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Open);
    }

    #[tokio::test]
    async fn reset_idempotent() {
        let reg = BreakerRegistry::new();
        reg.register("WH_TX".into(), "search_orders".into()).await;
        let entry = reg.reset("WH_TX").await.unwrap();
        assert_eq!(entry.state, BreakerState::Closed);
    }

    #[tokio::test]
    async fn trip_unknown_returns_none() {
        let reg = BreakerRegistry::new();
        assert!(reg.trip("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn is_open_unknown_returns_false() {
        let reg = BreakerRegistry::new();
        assert!(!reg.is_open("nonexistent").await);
    }

    #[tokio::test]
    async fn list_returns_sorted_entries() {
        let reg = BreakerRegistry::new();
        reg.register("Zebra".into(), "dv1".into()).await;
        reg.register("Alpha".into(), "dv2".into()).await;
        let entries = reg.list().await;
        assert_eq!(entries[0].breaker_id, "Alpha");
        assert_eq!(entries[1].breaker_id, "Zebra");
    }
}
```

- [ ] **Step 2: Register module in lib.rs**

In `crates/riversd/src/lib.rs`, add:

```rust
pub mod circuit_breaker;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p riversd -- circuit_breaker`
Expected: All 10 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/riversd/src/circuit_breaker.rs crates/riversd/src/lib.rs
git commit -m "feat(circuit-breaker): add BreakerRegistry with state management and tests"
```

---

### Task 3: Wire Registry into AppContext and Bundle Loading

**Files:**
- Modify: `crates/riversd/src/server/context.rs:94-189`
- Modify: `crates/riversd/src/bundle_loader/load.rs:235-262`

- [ ] **Step 1: Add BreakerRegistry to AppContext**

In `crates/riversd/src/server/context.rs`, add the field to `AppContext` (after the `event_bus` field, around line 125):

```rust
    pub circuit_breaker_registry: Arc<crate::circuit_breaker::BreakerRegistry>,
```

Add to the `AppContext::new()` constructor (or `Default` impl) — initialize with:

```rust
    circuit_breaker_registry: Arc::new(crate::circuit_breaker::BreakerRegistry::new()),
```

Make sure `use std::sync::Arc;` is imported (likely already is).

- [ ] **Step 2: Build registry during bundle loading**

In `crates/riversd/src/bundle_loader/load.rs`, find the DataView registration loop (around line 256-262 where `for dv in app.config.data.dataviews.values()`). After DataViews are registered, add breaker registration:

```rust
    // Build circuit breaker registry from DataView config (spec §3)
    for (dv_name, dv_config) in &app.config.data.dataviews {
        if let Some(ref breaker_id) = dv_config.circuit_breaker_id {
            ctx.circuit_breaker_registry
                .register(breaker_id.clone(), dv_name.clone())
                .await;
        }
    }
```

- [ ] **Step 3: Restore persisted breaker state from StorageEngine**

After the registration loop above, add state restoration:

```rust
    // Restore persisted breaker state from StorageEngine (spec §3, REG-3)
    if let Some(ref storage) = ctx.storage_engine {
        for entry in ctx.circuit_breaker_registry.list().await {
            let key = format!("breaker:{}:{}", app_id, entry.breaker_id);
            match storage.get("rivers", &key).await {
                Ok(Some(bytes)) => {
                    if let Ok(state_str) = String::from_utf8(bytes) {
                        if state_str.trim() == "open" {
                            ctx.circuit_breaker_registry
                                .set_state(&entry.breaker_id, crate::circuit_breaker::BreakerState::Open)
                                .await;
                            tracing::info!(
                                breaker = %entry.breaker_id,
                                "restored breaker state: OPEN"
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        breaker = %entry.breaker_id,
                        error = %e,
                        "failed to read persisted breaker state, starting CLOSED"
                    );
                }
            }
        }
    }

    // Log breaker summary
    for entry in ctx.circuit_breaker_registry.list().await {
        tracing::info!(
            breaker = %entry.breaker_id,
            state = ?entry.state,
            dataviews = entry.dataviews.len(),
            "breaker loaded"
        );
    }
```

Note: `app_id` should be the app's `appId` field. Check how `app.manifest.app_id` or similar is accessed in the surrounding code.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p riversd`
Expected: Compiles. May need import adjustments.

- [ ] **Step 5: Commit**

```bash
git add crates/riversd/src/server/context.rs crates/riversd/src/bundle_loader/load.rs
git commit -m "feat(circuit-breaker): wire BreakerRegistry into AppContext and bundle loading"
```

---

### Task 4: DataView Dispatch — Breaker Check Before Execution

**Files:**
- Modify: `crates/riversd/src/view_engine/pipeline.rs:121-147`

- [ ] **Step 1: Add breaker check in pipeline**

In `crates/riversd/src/view_engine/pipeline.rs`, find the `execute_rest_view` function. In the DataView handler branch (around line 121-132), add a breaker check BEFORE the `exec.execute()` call:

```rust
    // Circuit breaker check (spec §4, DSP-1)
    if let Some(ref breaker_id) = dataview_config.circuit_breaker_id {
        if ctx.app_context.circuit_breaker_registry.is_open(breaker_id).await {
            let body = serde_json::json!({
                "error": format!("circuit breaker '{}' is open", breaker_id),
                "breakerId": breaker_id,
                "retryable": true
            });
            return Ok(axum::response::Response::builder()
                .status(503)
                .header("Content-Type", "application/json")
                .header("Retry-After", "30")
                .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap());
        }
    }
```

Note: The exact response construction depends on how the pipeline returns responses. Check the existing error response pattern in the function and match it. The key requirement is: 503 status, `Retry-After: 30` header, JSON body with `breakerId` and `retryable: true`.

You'll need to find where the `DataViewConfig` for the current DataView is accessible in the pipeline. It may be in the `ViewContext` or passed as a parameter. Trace the `dataview_config` variable to understand where `circuit_breaker_id` can be read.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p riversd`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add crates/riversd/src/view_engine/pipeline.rs
git commit -m "feat(circuit-breaker): check breaker state before DataView dispatch, return 503 when open"
```

---

### Task 5: Admin API Endpoints

**Files:**
- Modify: `crates/riversd/src/admin_handlers.rs`
- Modify: `crates/riversd/src/server/router.rs:147-185`

- [ ] **Step 1: Add admin handler functions**

In `crates/riversd/src/admin_handlers.rs`, add these handler functions. Follow the existing pattern — handlers take `State(ctx): State<AppContext>` and return `impl IntoResponse`:

```rust
/// List all circuit breakers for an app.
/// GET /admin/apps/:app_id/breakers
pub async fn admin_list_breakers_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let entries = ctx.circuit_breaker_registry.list().await;
    Json(serde_json::json!(entries))
}

/// Get a single circuit breaker status.
/// GET /admin/apps/:app_id/breakers/:breaker_id
pub async fn admin_get_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.get(&breaker_id).await {
        Some(entry) => Json(serde_json::json!(entry)).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found", breaker_id)})),
        ).into_response(),
    }
}

/// Trip a circuit breaker.
/// POST /admin/apps/:app_id/breakers/:breaker_id/trip
pub async fn admin_trip_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.trip(&breaker_id).await {
        Some(entry) => {
            // Persist state to StorageEngine (spec §3, REG-2)
            if let Some(ref storage) = ctx.storage_engine {
                let key = format!("breaker:{}:{}", app_id, breaker_id);
                if let Err(e) = storage.set("rivers", &key, b"open".to_vec(), None).await {
                    tracing::error!(breaker = %breaker_id, error = %e, "failed to persist breaker state");
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": "failed to persist breaker state"})),
                    ).into_response();
                }
            }
            tracing::info!(breaker = %breaker_id, "circuit breaker TRIPPED");
            Json(serde_json::json!(entry)).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found", breaker_id)})),
        ).into_response(),
    }
}

/// Reset a circuit breaker.
/// POST /admin/apps/:app_id/breakers/:breaker_id/reset
pub async fn admin_reset_breaker_handler(
    State(ctx): State<AppContext>,
    axum::extract::Path((app_id, breaker_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    match ctx.circuit_breaker_registry.reset(&breaker_id).await {
        Some(entry) => {
            // Persist state to StorageEngine (spec §3, REG-2)
            if let Some(ref storage) = ctx.storage_engine {
                let key = format!("breaker:{}:{}", app_id, breaker_id);
                if let Err(e) = storage.set("rivers", &key, b"closed".to_vec(), None).await {
                    tracing::error!(breaker = %breaker_id, error = %e, "failed to persist breaker state");
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": "failed to persist breaker state"})),
                    ).into_response();
                }
            }
            tracing::info!(breaker = %breaker_id, "circuit breaker RESET");
            Json(serde_json::json!(entry)).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("breaker '{}' not found", breaker_id)})),
        ).into_response(),
    }
}
```

Note: The `app_id` path parameter handling needs refinement — currently the BreakerRegistry is global, not per-app. If the codebase supports multiple loaded apps, the registry needs to be keyed by app. Check how `AppContext` handles multi-app bundles and adjust accordingly. The handlers above are a starting pattern; the implementer must verify how apps are resolved.

- [ ] **Step 2: Register routes**

In `crates/riversd/src/server/router.rs`, in the `build_admin_router` function (around line 147-185), add routes before `.with_state(ctx)`:

```rust
    .route("/admin/apps/:app_id/breakers", get(admin_list_breakers_handler))
    .route("/admin/apps/:app_id/breakers/:breaker_id", get(admin_get_breaker_handler))
    .route("/admin/apps/:app_id/breakers/:breaker_id/trip", post(admin_trip_breaker_handler))
    .route("/admin/apps/:app_id/breakers/:breaker_id/reset", post(admin_reset_breaker_handler))
```

Add the handler imports at the top of the file alongside existing handler imports.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p riversd`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/riversd/src/admin_handlers.rs crates/riversd/src/server/router.rs
git commit -m "feat(circuit-breaker): add admin API endpoints for list/get/trip/reset"
```

---

### Task 6: riversctl CLI — breaker Subcommand

**Files:**
- Modify: `crates/riversctl/src/commands/admin.rs`
- Modify: `crates/riversctl/src/main.rs`

- [ ] **Step 1: Add breaker command functions**

In `crates/riversctl/src/commands/admin.rs`, add these functions following the existing `admin_get`/`admin_post` pattern:

```rust
pub async fn cmd_breaker_list(url: &str, app: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers", app);
    let data = admin_get(url, &path).await?;
    let breakers = data.as_array().unwrap_or(&vec![]);
    if breakers.is_empty() {
        println!("No circuit breakers configured for app '{}'", app);
        return Ok(());
    }
    for b in breakers {
        let id = b["breakerId"].as_str().unwrap_or("?");
        let state = b["state"].as_str().unwrap_or("?");
        let dvs = b["dataviews"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("  {:<30} {:<8} ({} dataview{})", id, state, dvs, if dvs == 1 { "" } else { "s" });
    }
    Ok(())
}

pub async fn cmd_breaker_status(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}", app, name);
    let data = admin_get(url, &path).await?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}

pub async fn cmd_breaker_trip(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}/trip", app, name);
    let data = admin_post(url, &path, "").await?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}

pub async fn cmd_breaker_reset(url: &str, app: &str, name: &str) -> Result<(), String> {
    let path = format!("/admin/apps/{}/breakers/{}/reset", app, name);
    let data = admin_post(url, &path, "").await?;
    let state = data["state"].as_str().unwrap_or("?");
    println!("  {} {}", name, state);
    if let Some(dvs) = data["dataviews"].as_array() {
        let names: Vec<&str> = dvs.iter().filter_map(|v| v.as_str()).collect();
        println!("  DataViews: {}", names.join(", "));
    }
    Ok(())
}
```

- [ ] **Step 2: Add CLI dispatch in main.rs**

In `crates/riversctl/src/main.rs`, add a match arm in the command dispatch (around lines 26-82):

```rust
        "breaker" => {
            let app = args.iter()
                .find(|a| a.starts_with("--app="))
                .map(|a| &a[6..])
                .ok_or("--app=<appId|appName> is required")?;

            if args.iter().any(|a| a == "--list") {
                admin::cmd_breaker_list(&admin_url, app).await
            } else if let Some(name_arg) = args.iter().find(|a| a.starts_with("--name=")) {
                let name = &name_arg[7..];
                if args.iter().any(|a| a == "--trip") {
                    admin::cmd_breaker_trip(&admin_url, app, name).await
                } else if args.iter().any(|a| a == "--reset") {
                    admin::cmd_breaker_reset(&admin_url, app, name).await
                } else {
                    admin::cmd_breaker_status(&admin_url, app, name).await
                }
            } else {
                Err("usage: riversctl breaker --app=<appId|appName> --list | --name=<breakerId> [--trip|--reset]".into())
            }
        }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p riversctl`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add crates/riversctl/src/commands/admin.rs crates/riversctl/src/main.rs
git commit -m "feat(circuit-breaker): add riversctl breaker subcommand"
```

---

### Task 7: Bundle Validation — Solo Breaker ID Warning

**Files:**
- Modify: `crates/rivers-runtime/src/validate_crossref.rs`

- [ ] **Step 1: Add solo breaker ID check**

In `crates/rivers-runtime/src/validate_crossref.rs`, in the `validate_crossref` function, after existing DataView checks, add:

```rust
    // Check for circuit breaker IDs referenced by only one DataView (spec §7, VAL-1)
    let mut breaker_usage: HashMap<String, Vec<String>> = HashMap::new();
    for (dv_name, dv_config) in &app.config.data.dataviews {
        if let Some(ref breaker_id) = dv_config.circuit_breaker_id {
            breaker_usage
                .entry(breaker_id.clone())
                .or_default()
                .push(dv_name.clone());
        }
    }

    let all_breaker_ids: Vec<&str> = breaker_usage.keys().map(|s| s.as_str()).collect();
    for (breaker_id, dataviews) in &breaker_usage {
        if dataviews.len() == 1 {
            let mut msg = format!(
                "circuitBreakerId '{}' is referenced by only one DataView ('{}')",
                breaker_id, dataviews[0]
            );
            // Levenshtein suggestion against other breaker IDs in this app
            if all_breaker_ids.len() > 1 {
                let others: Vec<&str> = all_breaker_ids
                    .iter()
                    .filter(|id| **id != breaker_id.as_str())
                    .copied()
                    .collect();
                if let Some(suggestion) = crate::validate_format::suggest_key(breaker_id, &others) {
                    msg = format!("{} — {}", msg, suggestion);
                }
            }
            results.push(
                ValidationResult::warn("CB001", msg)
                    .with_field(breaker_id.clone()),
            );
        }
    }
```

Note: Check the exact location within `validate_crossref` where per-app validation happens. The code above assumes you're inside a `for app in &bundle.apps` loop. Also verify that `crate::validate_format::suggest_key` is accessible from this module — it may need a `pub(crate)` export.

- [ ] **Step 2: Write test for solo breaker warning**

Add a test in the test module of `validate_crossref.rs`:

```rust
#[test]
fn solo_circuit_breaker_id_warns() {
    // Build a minimal bundle with two breaker IDs:
    // "Warehouse_Transaction" used by 2 DataViews (no warning)
    // "Warehous_Transaction" used by 1 DataView (warning with suggestion)
    // ... construct test bundle ...
    
    let results = validate_crossref(&bundle);
    let warning = results
        .iter()
        .find(|r| r.error_code.as_deref() == Some("CB001"))
        .expect("expected CB001 warning for solo breaker ID");
    assert!(warning.message.contains("Warehous_Transaction"));
    assert!(warning.message.contains("Warehouse_Transaction")); // suggestion
}
```

Note: The test construction depends on how `LoadedBundle` is built in test code. Follow the existing test patterns in this file for building test bundles.

- [ ] **Step 3: Run tests**

Run: `cargo test -p rivers-runtime -- validate_crossref`
Expected: All tests pass including the new one.

- [ ] **Step 4: Commit**

```bash
git add crates/rivers-runtime/src/validate_crossref.rs
git commit -m "feat(circuit-breaker): add solo breaker ID validation warning with Levenshtein suggestion"
```

---

### Task 8: Documentation

**Files:**
- Modify: `docs/guide/developer.md` or create `docs/guide/tutorials/tutorial-circuit-breakers.md`

- [ ] **Step 1: Add circuit breaker section to developer guide or create tutorial**

Check the existing tutorial pattern in `docs/guide/tutorials/`. Create a tutorial covering:

- What circuit breakers do and when to use them
- How to add `circuitBreakerId` to DataView config
- How to use `riversctl breaker` commands (list, status, trip, reset)
- Example: trip a breaker, verify 503, reset it
- Persistence behavior across restarts

- [ ] **Step 2: Commit**

```bash
git add docs/guide/tutorials/tutorial-circuit-breakers.md
git commit -m "docs: add circuit breaker tutorial"
```

---

### Task 9: Update ProgramReviewTasks.md

**Files:**
- Modify: `todo/ProgramReviewTasks.md`

- [ ] **Step 1: Mark circuit breaker v1 tasks complete**

Update all Circuit Breaker v1 items from `- [ ]` to `- [x]` in `todo/ProgramReviewTasks.md`.

- [ ] **Step 2: Commit**

```bash
git add todo/ProgramReviewTasks.md
git commit -m "docs: mark circuit breaker v1 tasks complete"
```

---

### Task 10: Final Validation

- [ ] **Step 1: Full workspace compile**

Run: `cargo check --workspace`
Expected: Compiles with no new errors.

- [ ] **Step 2: Run all riversd tests**

Run: `cargo test -p riversd`
Expected: All tests pass.

- [ ] **Step 3: Run all rivers-runtime tests**

Run: `cargo test -p rivers-runtime`
Expected: All tests pass including new circuit breaker validation tests.

- [ ] **Step 4: Validate address-book-bundle still validates**

Run: `cargo run -p riverpackage -- validate address-book-bundle`
Expected: Passes with no new errors or warnings (address-book-bundle has no circuit breaker config).
