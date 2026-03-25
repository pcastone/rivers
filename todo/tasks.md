# Tasks — Examples, Templates & Tutorials

**Goal:** Create starter template, working examples, JS handler examples, and tutorial wiki entries covering all datasource drivers, WebSocket, and SSE.

---

## 1. First-Time Starter Template

`templates/starter/` — copy-rename-go minimal bundle.

- [x] **T1.1** Bundle manifest, single app-service manifest (faker datasource)
- [x] **T1.2** resources.toml, one schema file, app.toml with 1 DataView + 1 REST view
- [x] **T1.3** Placeholder comments in each file explaining what to change

## 2. Example Bundles

Each self-contained, runnable with `riversd`.

### 2a. hello-api (minimal REST)
- [x] **T2.1** Bundle: app-service, faker datasource, 2 endpoints (list + get-by-id)

### 2b. todo-crud (CRUD with CodeComponent handlers)
- [x] **T2.2** Bundle: app-service, faker datasource, 5 endpoints (list, get, create, update, delete)
- [x] **T2.3** JS handlers: `createTodo`, `updateTodo`, `deleteTodo` with validation + cache invalidates

### 2c. realtime-dashboard (SSE + polling)
- [x] **T2.4** Bundle: app-service, faker datasource, SSE view with polling config + streaming REST export
- [x] **T2.5** JS handler: `streamMetrics` using {chunk, done} protocol

### 2d. chat-app (WebSocket lifecycle hooks)
- [x] **T2.6** Bundle: app-service, WebSocket view with on_connect/on_message/on_disconnect + SSE feed
- [x] **T2.7** JS handlers: `onConnect`, `onMessage`, `onDisconnect`

## 3. JavaScript Handler Examples

`examples/handlers/` — standalone JS files demonstrating patterns.

- [x] **T3.1** `basic-handler.js` — minimal handler reading request, setting resdata
- [x] **T3.2** `crud-handler.js` — create/update/delete with ctx.dataview() calls
- [x] **T3.3** `auth-guard.js` — guard handler with Rivers.crypto password verify + session claims
- [x] **T3.4** `streaming-handler.js` — {chunk, done} protocol for streaming REST
- [x] **T3.5** `websocket-hooks.js` — on_connect, on_message, on_disconnect lifecycle
- [x] **T3.6** `async-handler.js` — Promise.all parallel dataview calls
- [x] **T3.7** `kv-store-handler.js` — ctx.store.set/get/del with TTL
- [x] **T3.8** `outbound-http.js` — Rivers.http.get/post for external API calls

## 4. Tutorial Wiki Entries

`docs/guide/tutorials/` — one tutorial per topic, each a self-contained walkthrough.

### 4a. Datasource Tutorials (all 12 drivers)
- [x] **T4.1** `datasource-faker.md` — faker driver setup, schema with faker attributes, seeded data
- [x] **T4.2** `datasource-postgresql.md` — postgres driver, lockbox credentials, connection pool, SQL queries
- [x] **T4.3** `datasource-mysql.md` — mysql driver, lockbox credentials, connection pool
- [x] **T4.4** `datasource-sqlite.md` — sqlite driver, nopassword, embedded relational
- [x] **T4.5** `datasource-redis.md` — redis driver, cache/sessions/KV, lockbox
- [x] **T4.6** `datasource-http.md` — http driver, inter-service proxy, service references
- [x] **T4.7** `datasource-kafka.md` — kafka broker, message streaming, lockbox
- [x] **T4.8** `datasource-rabbitmq.md` — rabbitmq broker, message queuing
- [x] **T4.9** `datasource-nats.md` — nats broker, pub/sub
- [x] **T4.10** `datasource-elasticsearch.md` — elasticsearch plugin, search queries
- [x] **T4.11** `datasource-mongodb.md` — mongodb plugin, document store
- [x] **T4.12** `datasource-ldap.md` — ldap plugin, directory queries

### 4b. Real-Time Tutorials
- [x] **T4.13** `tutorial-websocket.md` — WebSocket view setup, lifecycle hooks, broadcast vs direct mode, JS handlers
- [x] **T4.14** `tutorial-sse.md` — SSE view setup, polling config, diff strategies, Last-Event-ID reconnection, event buffer
- [x] **T4.15** `tutorial-streaming-rest.md` — streaming REST view, ndjson vs sse format, {chunk, done} handler protocol

### 4c. Handler Tutorials
- [x] **T4.16** `tutorial-js-handlers.md` — JS handler API, ctx object, Rivers globals, async patterns
- [x] **T4.17** `tutorial-auth-sessions.md` — guard views, session auth, Rivers.crypto, RBAC
- [x] **T4.18** `tutorial-graphql.md` — enabling GraphQL, auto-generated queries/mutations/subscriptions

## Validation

- [ ] **T5.1** All example bundles pass `riversctl validate`
- [ ] **T5.2** All JS handler examples have correct ctx/Rivers API usage
- [ ] **T5.3** All tutorials reference correct config keys and driver names
