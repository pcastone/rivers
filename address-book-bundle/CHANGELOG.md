# Address Book Bundle — CHANGELOG

---

## [Decision] — TOML field names: explicit `name` in datasource and dataview tables

**File:** `address-book-service/app.toml`, `address-book-main/app.toml`
**Description:** `DatasourceConfig` and `DataViewConfig` Rust structs both have a required `name: String` field with no serde default. When TOML deserializes `HashMap<String, DatasourceConfig>` or `HashMap<String, DataViewConfig>`, the HashMap key is the map index but the struct fields come entirely from the table body. The spec (`rivers-app-development.md`) omits the `name` field from datasource and dataview table examples.
**Spec reference:** §Datasource Configuration, §DataView Configuration
**Resolution:** Added `name = "..."` explicitly to each `[data.datasources.X]` and `[data.dataviews.X]` table to match what the current code requires.

---

## [Gap] — Handler type string: `"dataview"` not `"data_view"`

**File:** `address-book-service/app.toml`, `address-book-main/app.toml`
**Description:** `HandlerConfig` is a serde-tagged enum with `rename_all = "lowercase"`. The variant `Dataview` serializes as `"dataview"`. The spec uses `type = "data_view"` (snake_case) which would fail to deserialize.
**Spec reference:** §View Configuration — REST View with DataView Handler
**Resolution:** Used `type = "dataview"` (no underscore) throughout to match the compiled code.

---

## [Gap] — DataView cache config key: `caching` not `cache`

**File:** `address-book-service/app.toml`, `address-book-main/app.toml`
**Description:** `DataViewConfig` has a field `caching: Option<DataViewCachingConfig>`. The spec shows `[data.dataviews.X.cache]` which maps to an unknown key and is silently ignored by serde. Cache would not function if the spec format were used.
**Spec reference:** §DataView Configuration
**Resolution:** Used `[data.dataviews.X.caching]` with `ttl_seconds` to match the Rust struct field name.

---

## [Gap] — SPA config section: `[static_files]` not `[spa]`

**File:** `address-book-main/app.toml`
**Description:** `AppConfig` has a `static_files: Option<AppStaticFilesConfig>` field. The spec shows `[spa]` with `root_path`, but the Rust struct field is `static_files` with `root` (not `root_path`). Using `[spa]` or `root_path` would be silently ignored.
**Spec reference:** §SPA Configuration
**Resolution:** Used `[static_files]` with `root = "libraries/spa"` to match the current Rust structs.

---

## [Decision] — SPA built with npm/rollup; `node_modules/` excluded from bundle

**File:** `address-book-main/libraries/`
**Description:** Running `npm install && npm run build` in `address-book-main/libraries/` produces `spa/bundle.js` (41 KB) and `spa/bundle.css` (2.2 KB). The `node_modules/` directory is not part of the bundle — only `spa/` output is needed at runtime.
**Spec reference:** §Part 4 — Svelte SPA
**Resolution:** Added rollup warning suppression note: add `"type": "module"` to `package.json` to silence the ES module warning (cosmetic only, does not affect output).

---

## [Known Limitation] — Faker driver returns stub data, not contact-shaped records

**File:** `address-book-service/app.toml`, `address-book-service/schemas/contact.schema.json`
**Description:** The current `FakerDriver` implementation (`rivers-core/src/drivers/faker.rs`) generates rows with only `id` (Integer) and `name` (String) fields. It does not read the schema's `faker` attribute dot-notation to generate realistic contact data. API endpoints will respond correctly (routes matched, 200 status) but data will not match the contact schema shape.
**Spec reference:** §address-book-service/schemas/contact.schema.json
**Resolution:** UNRESOLVED — awaiting faker driver implementation that reads schema field `faker` attributes.

---

## [Known Limitation] — DataViewExecutor not wired; view responses return stubs

**File:** `crates/riversd/src/server.rs`
**Description:** Phase AB's `run_server_with_listener_and_log` populates `ctx.view_router` from the bundle but does not build or wire a `DataViewExecutor` into `ctx.dataview_executor`. When `executor` is `None`, `execute_rest_view` returns `{"_stub": true, "_dataview": "...", "_params": {...}}` instead of real query results.
**Spec reference:** §Expected Behavior
**Resolution:** UNRESOLVED — DataViewExecutor wiring from bundle config is a future phase task.
