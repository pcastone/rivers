# Rivers Provisioning Service (RPS) Specification

**Document Type:** Implementation Specification  
**Version:** 2.0  
**Scope:** RPS Master, RPS Relay, Node Provisioning, Alias Registry, Role System, Secret Broker, ProcessPool (CodeComponent Execution)  
**Status:** Design / Pre-Implementation  
**Depends On:** Epic 1 (Workspace), Epic 4 (EventBus), Epic 5 (LockBox), Epic 19 (App Bundles),  
               Epic 20 (Plugin Loading), Epic 22 (Admin API), Epic 25-27 (Clustering)  
**Key Principle:** The RPS is a Rivers application deployed on Rivers. It eats its own dog food.

**Change Log (v2):**
- Section 2.3: Replaced 2-node Raft model with priority-ordered Trust Bundle model (Remediation SEC-1/2/3)
- Section 2.4: Added dedicated RPS port (separate from application traffic port)
- Section 3 (new): CodeComponent ProcessPool execution model (Remediation SEC-10/SEC-11)
- Section 9.1: Updated bootstrap sequence to include Trust Bundle receipt
- Section 15: Open Questions updated — SEC-14 resolved, SW-2 resolved, SEC-15 resolved

---

## Table of Contents

1. [Philosophy and Design Principles](#1-philosophy-and-design-principles)
2. [Topology](#2-topology)
3. [CodeComponent Execution — ProcessPool Sandbox](#3-codecomponent-execution--processpool-sandbox)
4. [Operational Modes](#4-operational-modes)
5. [Role System](#5-role-system)
6. [Alias Registry](#6-alias-registry)
7. [Poll Protocol](#7-poll-protocol)
8. [Secret Broker](#8-secret-broker)
9. [Bundle and Plugin Distribution](#9-bundle-and-plugin-distribution)
10. [Node Lifecycle](#10-node-lifecycle)
11. [RPS as App Bundle](#11-rps-as-app-bundle)
12. [RPS Client Driver](#12-rps-client-driver)
13. [Security Model](#13-security-model)
14. [Configuration Reference](#14-configuration-reference)
15. [riversctl RPS Commands](#15-riversctl-rps-commands)
16. [Open Questions](#16-open-questions)

---

## 1. Philosophy and Design Principles

### 1.1 Zero Additional Infrastructure

A Rivers cluster with RPS requires no external provisioning infrastructure. No Consul, no etcd, no Vault (optional), no cert-manager, no separate identity service. The RPS ships as App Bundles that deploy to standard `riversd` nodes. Operators who can run `riversd` can run the RPS.

### 1.2 Eat Your Own Dog Food

The RPS master and relay are App Bundles running on `riversd`. Every Rivers capability — Views, DataViews, CodeComponents, the handler pipeline, observability, hot reload, the deployment pipeline — is available to the RPS implementation. The RPS is not a special case. It is a Rivers application that happens to implement cluster provisioning.

Consequences:
- RPS upgrades use `riversctl deploy` and `riversctl promote` — no special tooling
- RPS logic is testable with `riversctl test` — no special test harness
- RPS gets OpenTelemetry, Prometheus metrics, and structured logging for free
- RPS handlers run sandboxed in the ProcessPool — the secret broker handler cannot escape the Rivers runtime

### 1.3 The Alias Contract

The alias is the environment contract between developer and operator. The developer declares what aliases an application needs. The operator defines what each alias means per environment. The RPS enforces the resolution. The application never changes between environments. The alias is the seam.

### 1.4 Nodes Are Cattle

Nodes do not decide what they are. The RPS assigns roles. A role is a complete capability declaration — drivers, datasources (by alias), libraries, and plugins. The RPS provisions everything a node needs to fulfill its role. A node that loses its role loses its provisioned resources. A new node that receives a role gains them.

### 1.5 The Secret Broker Never Stores Secrets

The RPS knows alias names, environment mappings, and which backend holds each secret. It does not store secret values. When a node needs a secret, the RPS fetches it from the backend and delivers it encrypted directly to the requesting node. The secret value touches RPS memory briefly during delivery and is never written to disk, logged, or cached.

### 1.6 Trust Is Bootstrapped, Not Assumed

No node participates in cluster communication before it has been provisioned by the RPS. By the time Raft and Gossip traffic flows, every node holds a signed Node Certificate issued by the RPS master. The Trust Bundle is the authoritative source of truth for RPS instance membership and failover order.

### 1.7 Design Patterns

| Pattern | Application |
|---|---|
| Facade | RPS presents a single provisioning interface over many subsystems |
| Adapter | `rps-client` driver translates Rivers driver contract to RPS protocol |
| Factory | Role system instantiates resource sets from role definitions |
| Observer | Poll protocol — nodes observe sequence number changes |
| Strategy | Secret broker plugs in different backend fetch strategies; ProcessPool uses pluggable engine strategies (V8, Wasmtime) |
| Singleton | RPS master instance per deployment, relay instance per cluster |
| Builder | Node bootstrap accumulates provisioned resources incrementally; TaskContext built by `on_request` handler |

---

## 2. Topology

### 2.1 Full Deployment Topology

```
┌─────────────────────────────────────────────────────────────────┐
│  RPS Tier (port 9443 — dedicated, no app traffic)               │
│                                                                 │
│  ┌─────────────────────┐    ┌─────────────────────┐           │
│  │  RPS Primary        │    │  RPS Alternate-1    │           │
│  │  riversd + rps-     │    │  riversd + rps-     │           │
│  │  master.zip         │    │  secondary.zip      │           │
│  │  (port 9443)        │    │  (hot standby)      │           │
│  └──────────┬──────────┘    └─────────────────────┘           │
│             │ RPS Protocol (mTLS, port 9443)                   │
└─────────────┼───────────────────────────────────────────────────┘
              │
    ┌─────────┼──────────────────────────────┐
    │         │                              │
    ▼         ▼                              ▼
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│ Cluster 1        │  │ Cluster 2        │  │ Cluster N        │
│                  │  │                  │  │                  │
│ node-a  node-b   │  │ node-f  node-g   │  │ ...              │
│ node-c  node-d   │  │ node-h  node-i   │  │                  │
│ node-e (relay)   │  │ node-j (relay)   │  │ node-? (relay)   │
│                  │  │                  │  │                  │
│ 1-5 nodes        │  │ 1-5 nodes        │  │ 1-5 nodes        │
│ + 1 relay        │  │ + 1 relay        │  │ + 1 relay        │
└──────────────────┘  └──────────────────┘  └──────────────────┘
```

A full production deployment supports hundreds of nodes across dozens of clusters, all provisioned by a single RPS primary + alternate pair. The relay is the fan-out mechanism — the primary serves relay connections (tens), not node connections (hundreds).

### 2.2 Cluster Internal Topology

Within a cluster, the relay is a regular cluster member that additionally proxies RPS protocol:

```
                    RPS Primary (port 9443)
                         │
                         │ RPS protocol (mTLS)
                         ▼
┌────────────────────────────────────────────────┐
│  Local Cluster                                 │
│                                                │
│  node-a ──┐                                    │
│  node-b ──┤                                    │
│  node-c ──┼──► node-e (RPS relay) ◄──► Primary │
│  node-d ──┘         │                          │
│                     │ gossip                   │
│                     └──► all nodes             │
│                                                │
└────────────────────────────────────────────────┘
```

Nodes never communicate directly with the RPS primary. All provisioning flows through the relay. The relay caches primary state locally — nodes get fast local responses and the primary is protected from direct node traffic.

### 2.3 Primary / Alternate — Trust Bundle Model

**The 2-node Raft master/secondary model is replaced by a priority-ordered Trust Bundle model.**

The RPS does not run a Raft quorum for its own membership. Quorum adds a split-brain failure mode and requires a minimum of two nodes to be simultaneously reachable for writes. The Trust Bundle model eliminates this dependency: the Trust Bundle is the authoritative membership list for RPS failover, distributed to all nodes and self-sufficient.

#### Trust Bundle Structure

```rust
pub struct TrustBundle {
    pub version:    u64,
    pub issued_at:  DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub members: Vec<TrustMember>,
}

pub struct TrustMember {
    pub priority: u8,           // 1 = primary, 2+ = alternates
    pub address:  String,       // host:port on dedicated RPS port
    pub pubkey:   [u8; 32],     // Ed25519 public key
}
```

Example bundle:
```json
{
  "version": 7,
  "issued_at": "2026-03-10T00:00:00Z",
  "expires_at": "2026-03-11T00:00:00Z",
  "members": [
    { "priority": 1, "address": "rps-primary:9443",  "pubkey": "..." },
    { "priority": 2, "address": "rps-alt1:9443",     "pubkey": "..." },
    { "priority": 3, "address": "rps-alt2:9443",     "pubkey": "..." }
  ]
}
```

Maximum 5 nodes per RPS cluster. This keeps Trust Bundle rotation in the sub-second range when all nodes must re-verify.

#### Failover Sequence

Alternates are passive — they do not heartbeat the primary. Nodes are the failure detectors.

```
1. Node detects missed heartbeats from primary RPS
2. Node pings alternates in priority order from its Trust Bundle
3. Each alternate collects ping reports from nodes
4. When a quorum of node reports agrees primary is unreachable:
   → Alt-1 (priority 2) promotes itself to primary
5. Alt-1 initiates fresh key rotation
6. Alt-1 issues a new Trust Bundle with itself at priority 1
7. New Trust Bundle pushed to all nodes via secrets_seq increment
8. Nodes atomically swap to new bundle (verified by alt-1's pubkey)
```

This is a coordinated handoff, not a split-brain race. The quorum check in step 4 prevents spurious promotions from transient network partitions.

#### Rotation Triggers

| Trigger | Mechanism |
|---|---|
| Time-based | Configurable interval (default 24h). Primary issues new bundle with refreshed expiry. |
| Admin-triggered | `riversctl rps secret rotate` — forces immediate rotation. |
| DR event | Primary failure confirmed by node quorum. Alt-1 promotes and rotates. |

#### Trust Bundle Updates (Member List Changes)

Adding or removing alternates requires no special ceremony. The operator updates the member list via `riversctl`, the primary issues a new Trust Bundle containing the new member list, and the change propagates via the existing `secrets_seq` delta push mechanism. Nodes receive the update on next poll or via gossip wake-up.

### 2.4 Dedicated RPS Port

The RPS runs on a dedicated port (**9443** by default) separate from application traffic (default 8443). This separation is structural:

- Application load balancers never route to port 9443
- Network ACLs can restrict port 9443 to cluster-internal traffic only
- The RPS attack surface is isolated from the application surface
- Misconfigured apps cannot accidentally target the provisioning API

```toml
[base.rps]
port        = 9443
tls_cert    = "/var/rivers/rps/tls.crt"
tls_key     = "/var/rivers/rps/tls.key"
client_ca   = "/var/rivers/rps/node-ca.crt"   # mTLS enforcement
```

Application traffic continues on the standard port. The `--mode rps-master` and `--mode rps-secondary` nodes do not open the application port at all — they bind only to the RPS port.

---

## 3. CodeComponent Execution — ProcessPool Sandbox

All CodeComponents in Rivers — including RPS handlers — execute inside the ProcessPool sandbox. This section documents the unified execution model. For the full standalone ProcessPool specification, see `rivers-processpool-runtime-spec-v2.md`.

### 3.1 Why ProcessPool

The previous in-process JS sandbox used a blocklist approach to deny dangerous globals. Blocklist sandboxing is fundamentally broken for JavaScript: `Object` and `Reflect` are not blocked, allowing prototype chain traversal to recover any deleted global. V8 (now the default runtime) introduces additional JIT-based attack surface that makes in-process sandboxing insufficient.

The ProcessPool model replaces blocklist sandboxing with allowlist capability injection. Handlers receive only what was declared. The sandbox is process-isolated. Engine JIT state cannot escape.

### 3.2 Architecture

```
┌─────────────────────────────────────────────────────┐
│  ProcessPool                                        │
│                                                     │
│  ┌──────────────────┐   ┌──────────────────┐       │
│  │  Worker (V8)     │   │  Worker (WASM)   │       │
│  │  V8 Isolate      │   │  Wasmtime inst.  │  ...  │
│  │  clean context   │   │  clean context   │       │
│  └──────────────────┘   └──────────────────┘       │
│                                                     │
│  Task Queue  ──────────────────────────────────►   │
│  Watchdog Thread (preemption timer)                 │
└─────────────────────────────────────────────────────┘
         ▲                        │
         │ TaskContext            │ result / error
    on_request handler       on_response handler
    (host side)              (host side)
```

| Component | Description |
|---|---|
| ProcessPool | Manages workers, task queue, heap/memory limits. Multiple named pools supported (e.g., `light`, `heavy`, `wasm`). |
| Worker | Holds a V8 Isolate or Wasmtime instance. Picks up tasks, executes, returns result, cleans up. |
| TaskContext | Capability set built by the `on_request` handler. Contains opaque resource tokens, not raw connections or credentials. |
| Watchdog Thread | Timer-based preemption. V8: `TerminateExecution()`. WASM: Wasmtime epoch interruption. |

### 3.3 Capability Model

The view declaration is the complete and static dependency graph. There are no dynamic imports inside JS or WASM handlers. Every library a handler needs must be declared in the view definition:

```toml
[api.views.secret_request]
path                = "/rps/v1/secrets/{alias}"
view_type           = "Rest"
libs                = ["rps_crypto.wasm"]
datasources         = ["rps_db", "lockbox_backends"]
allow_outbound_http = false   # default — vault calls are made host-side

methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/secret_broker.ts",
    entrypoint_function = "onSecretRequest",
    resources           = ["rps_db", "lockbox_backends"]
}}
```

Before task execution begins, the ProcessPool resolves all declared libs. If any are missing, the task fails at dispatch — not mid-execution. This makes the dependency graph statically analyzable.

**SSRF closure:** `allow_outbound_http` defaults to `false`. If false, the `Rivers.http` object is never injected into the V8 context. A handler that attempts outbound HTTP without the capability declared simply has no object to call — the API does not exist inside the isolate. Host-side validation post-DNS checks destination addresses against RFC 1918 ranges even when `allow_outbound_http` is `true`.

### 3.4 Context Construction

The `on_request` handler (host side) builds the `TaskContext` before dispatch. The isolate receives opaque resource tokens — never raw connection strings or credentials:

```rust
let ctx = TaskContext::new()
    .add_datasource("rps_db", lockbox_alias: "rps-db")
    .add_datasource("lockbox_backends", lockbox_alias: "rps-lockbox-meta")
    .add_dataview("alias_lookup")
    .call("handlers/secret_broker.ts", "onSecretRequest", args);

pool.dispatch(ctx).await?
```

The lockbox alias is passed, not the secret. The secret never crosses into the isolate.

### 3.5 Time-Based Preemption

Both engines support transparent time-based preemption with no changes required to handler code:

- **V8:** The watchdog thread calls `TerminateExecution()` on the target Isolate when wall-clock time exceeds the configured limit. The handler receives a termination signal — no cooperative yield required.
- **WASM:** Wasmtime epoch interruption. Epoch check instructions are injected during native compilation — invisible to the `.wasm` binary. Standard unmodified WASM binaries get preemption for free.

### 3.6 Pool Configuration

```toml
[runtime.process_pools.default]
engine          = "v8"
workers         = 4
max_heap_mb     = 128
task_timeout_ms = 5000

[runtime.process_pools.wasm]
engine          = "wasmtime"
workers         = 2
max_memory_mb   = 64
task_timeout_ms = 10000
epoch_interval_ms = 10
```

The pool named in a view's `process_pool` attribute is used for that view's CodeComponents. The `default` pool is used when not specified.

### 3.7 RPS Integration

The view declaration doubles as the RPS role resource declaration. The RPS distributes exactly the libs declared in view definitions to nodes with the relevant role. No undeclared lib ever arrives on a node. A node with role `rps-handler` receives `rps_crypto.wasm` — nothing more.

---

## 4. Operational Modes

The `riversd` binary supports four operational modes via the `--mode` flag. The binary is identical across all modes — mode determines which App Bundles are expected and which subsystems are active.

| Mode | Flag | App Traffic | RPS Role | Port | Clustering |
|---|---|---|---|---|---|
| Node (default) | `--mode node` | Yes (8443) | Consumes RPS via relay | 8443 | Full cluster member |
| RPS Relay | `--mode rps-relay` | Yes (8443) | Proxies to primary, caches | 8443 + 9443 | Full cluster member |
| RPS Primary | `--mode rps-master` | No | Authoritative RPS | 9443 only | Trust Bundle cluster |
| RPS Alternate | `--mode rps-secondary` | No | Hot standby | 9443 only | Trust Bundle cluster |

The primary and alternates do not serve application traffic. They are dedicated provisioning infrastructure. Their attack surface is limited to the RPS protocol port (9443) and the `riversctl` admin API.

### 4.1 Mode Detection and Validation

On startup, `riversd` validates that the correct App Bundles are deployed for the declared mode:

```
rps-master mode requires:  rps-master.zip deployed
rps-relay mode requires:   rps-relay.zip deployed
node mode:                 any app bundles, rps-relay.zip must NOT be deployed
```

If validation fails, startup aborts with a clear error:

```
Error: mode=rps-master but rps-master bundle not deployed.
Run: riversctl deploy rps-master-{version}.zip --cluster rps-cluster
```

---

## 5. Role System

### 5.1 Role as Resource Declaration

A role is a complete, self-contained capability declaration. It specifies everything a node needs to fulfill that capability — drivers, datasources (referenced by alias), libraries, and WASM modules. A node assigned a role is fully provisioned by the RPS to fulfill it. A node with no roles assigned is a blank node — it runs `riversd` but serves no application traffic.

```toml
# roles.toml — managed by RPS primary, distributed to relays

[roles.search]
description = "Elasticsearch-backed full-text search capability"

[[roles.search.resources.drivers]]
name    = "elasticsearch"
alias   = "search"          # resolved by RPS alias registry

[[roles.search.resources.libs]]
name    = "fuse.js"
source  = "external"
version = "7.0.0"
url     = "https://cdn.jsdelivr.net/npm/fuse.js@7.0.0"
hash    = "sha256:abc123..."   # integrity verification

[[roles.search.resources.libs]]
name   = "marketing.js"
source = "bundle"
bundle = "storefront-v2"
path   = "/libs/marketing.js"

[[roles.search.resources.libs]]
name   = "storefront.wasm"
source = "bundle"
bundle = "storefront-v2"
path   = "/libs/storefront.wasm"

# ─────────────────────────────────────────────

[roles.auth]
description = "Authentication and session management"

[[roles.auth.resources.drivers]]
name  = "ldap"
alias = "directory"

[[roles.auth.resources.drivers]]
name  = "redis"
alias = "session-store"

[[roles.auth.resources.libs]]
name    = "jsonwebtoken.js"
source  = "external"
version = "9.0.0"
url     = "https://cdn.jsdelivr.net/npm/jsonwebtoken@9.0.0"
hash    = "sha256:def456..."

# ─────────────────────────────────────────────

[roles.data]
description = "Primary database operations"

[[roles.data.resources.drivers]]
name  = "postgresql"
alias = "primary-db"

[[roles.data.resources.drivers]]
name  = "mongodb"
alias = "document-store"

[[roles.data.resources.drivers]]
name  = "kafka"
alias = "event-stream"
```

### 5.2 Node Role Assignment

Nodes can have multiple roles. A node gets the union of all resources across its assigned roles:

```toml
# Node role assignments — managed by RPS primary

[nodes.node-a]
cluster = "prod-cluster-1"
roles   = ["auth", "api"]

[nodes.node-b]
cluster = "prod-cluster-1"
roles   = ["data", "search"]

[nodes.node-c]
cluster = "prod-cluster-1"
roles   = ["data"]

[nodes.node-d]
cluster = "prod-cluster-1"
roles   = ["search", "api"]

[nodes.node-e]
cluster = "prod-cluster-1"
roles   = ["rps-relay"]     # reserved role — designates this node as relay
```

node-b gets: postgresql driver, mongodb driver, kafka driver, elasticsearch driver, fuse.js, marketing.js, storefront.wasm.

### 5.3 Role Migration

The RPS can reassign roles between nodes without manual intervention. Role migration follows a safe sequence to avoid service disruption:

```
riversctl rps role move --role search --from node-d --to node-f

RPS executes:
  1. Provision node-f with search role resources (parallel to node-d still serving)
  2. Verify node-f healthy and role resources loaded
  3. Update load balancer / gossip routing to include node-f for search traffic
  4. Drain node-d search traffic (drain transfer if in-flight requests)
  5. Remove search role resources from node-d
  6. node-d continues with its remaining roles
```

Step 1 and 2 happen before any traffic moves — zero-downtime role migration.

### 5.4 Role Versioning

Roles are versioned. When a role definition changes (new library version, driver update), the RPS increments the role sequence number. Nodes with that role detect the sequence change on next poll and receive the updated resource set:

```
roles_seq: 14 → 15

Delta: role "search" updated
  lib fuse.js: 7.0.0 → 7.1.0
  hash updated: sha256:xyz789...

Affected nodes: node-b, node-d
Action: download new fuse.js, verify hash, hot-swap library
```

Library hot-swap follows the hot-reload mechanism from Epic 33 — running CodeComponents drain gracefully while the new library loads.

---

## 6. Alias Registry

### 6.1 Alias as Environment Contract

The alias is the only name an application ever uses to reference a datasource or secret. The alias is the same across all environments. What changes per environment is the alias definition — which secret backend, which secret name, which host, which database.

```
Application code:          Rivers.resources.DB
App manifest:              alias: "mysql"
Dev RPS resolves:          mysql-dev02 → SOPS → dev credentials → localhost:3306/appdb_dev
Staging RPS resolves:      mysql-stg-01 → Vault → staging creds → mysql-staging.internal/appdb_stg
Prod RPS resolves:         mysql-p23-d2 → Vault → prod credentials → mysql-prod.internal/appdb_prod

Zero code changes. Zero manifest changes. Zero bundle changes.
```

### 6.2 Alias Definition Structure

```toml
# alias-registry.toml — authoritative on RPS primary

[aliases.mysql]
description = "Primary relational database"
driver      = "mysql"

[aliases.mysql.environments.dev]
secret_name = "mysql-dev02"
backend     = "sops"
host        = "localhost:3306"
database    = "appdb_dev"
pool_max    = 5

[aliases.mysql.environments.staging]
secret_name = "mysql-stg-01"
backend     = "vault"
vault_path  = "secret/staging/mysql"
host        = "mysql-staging.internal:3306"
database    = "appdb_staging"
pool_max    = 10

[aliases.mysql.environments.prod]
secret_name = "mysql-p23-d2"
backend     = "vault"
vault_path  = "secret/prod/mysql"
host        = "mysql-prod-primary.internal:3306"
database    = "appdb_prod"
pool_max    = 50

# ─────────────────────────────────────────────

[aliases.search]
description = "Elasticsearch full-text search"
driver      = "elasticsearch"

[aliases.search.environments.dev]
secret_name = "elastic-dev01"
backend     = "inmemory"
host        = "localhost:9200"

[aliases.search.environments.prod]
secret_name = "elastic-sdf32-42"
backend     = "aws_sm"
arn         = "arn:aws:secretsmanager:us-east-1:123456789:secret:elastic-sdf32-42"
host        = "elasticsearch.prod.internal:9200"

# ─────────────────────────────────────────────

[aliases.rps-db]
description = "RPS primary internal database — RPS uses its own alias system"
driver      = "postgresql"

[aliases.rps-db.environments.prod]
secret_name = "rps-postgres-prod-01"
backend     = "vault"
host        = "localhost:5432"
database    = "rps"

[aliases.rps-db.environments.dev]
secret_name = "rps-sqlite-dev"
backend     = "sops"
host        = "localhost"
database    = "/var/rivers/rps/rps.db"
```

### 6.3 Alias Authorization

Not every application can access every alias. The RPS enforces alias authorization at deploy time and at runtime:

```toml
[aliases.mysql.authorization]
# Which app IDs are permitted to use this alias
allowed_apps = ["order-service", "user-portal", "admin-dashboard"]

# Which roles can provision this alias
allowed_roles = ["data", "api"]
```

At deploy time, `riversctl deploy` validates that the bundle's declared aliases are in `allowed_apps`. At runtime, the relay validates the node's role includes the alias before forwarding the secret request to the primary.

### 6.4 Alias Validation at Deploy Time

```
riversctl deploy order-service-v2.zip --env prod

Validating bundle against prod RPS alias registry...
  ✓ mysql   → mysql-p23-d2 (vault) — authorized for order-service
  ✓ search  → elastic-sdf32-42 (aws_sm) — authorized for order-service
  ✗ redis   → NOT DEFINED for prod environment

Deploy failed: alias "redis" has no prod definition.
Create it with: riversctl rps alias define redis --env prod \
    --secret-name redis-prod-01 --backend vault --host redis.prod.internal:6379
```

Deploy fails at validation — not at runtime. The developer finds out before the bundle reaches production.

### 6.5 Alias CRUD via riversctl

```bash
# Define a new alias
riversctl rps alias define mysql \
    --env prod \
    --driver mysql \
    --secret-name mysql-p23-d2 \
    --backend vault \
    --vault-path "secret/prod/mysql" \
    --host mysql-prod.internal:3306 \
    --database appdb_prod

# List all aliases for an environment
riversctl rps alias list --env prod

# Show alias definition across all environments
riversctl rps alias show mysql

# Update an alias (e.g., host change, secret rotation)
riversctl rps alias update mysql --env prod --host mysql-prod-new.internal:3306

# Remove alias from an environment
riversctl rps alias remove mysql --env staging

# Check which apps use an alias
riversctl rps alias usage mysql --env prod
```

---

## 7. Poll Protocol

### 7.1 Sequence Numbers

The RPS primary maintains a monotonically incrementing sequence number per resource category. Sequence numbers only increment — they never reset. A node that has been offline for a week catches up by comparing its last-known sequences against the current primary sequences.

```
RPS Primary sequence state:
  config_seq:   42
  secrets_seq:  17
  bundles_seq:   8
  plugins_seq:  11
  roles_seq:     3
  aliases_seq:  29
  trust_seq:     7   ← Trust Bundle version (added in v2)
```

### 7.2 Two-Tier Poll

Polling happens at two levels with different intervals:

**Relay polls primary:**
```
Interval: 30 seconds (configurable)
Payload:  relay identity + all category sequences
Response: delta for changed categories only
```

**Nodes poll relay:**
```
Interval: 60 seconds (configurable)
Payload:  node identity + role-relevant sequences
Response: delta for changed categories affecting this node's roles
```

The relay aggregates — if 5 nodes all have stale `secrets_seq:16` and primary is at `secrets_seq:17`, the relay fetches the delta once from the primary and delivers it to all 5 nodes from its cache. The primary sees one request, not five.

### 7.3 Poll Request and Response

**Node → Relay:**
```json
GET /rps/v1/poll
Authorization: NodeCertificate {paseto_token}

{
  "node_id":    "node-a",
  "cluster_id": "prod-cluster-1",
  "sequences": {
    "config_seq":  42,
    "secrets_seq": 16,
    "bundles_seq":  8,
    "plugins_seq":  11,
    "roles_seq":    3,
    "aliases_seq":  29,
    "trust_seq":    7
  }
}
```

**Relay → Node (delta only):**
```json
{
  "current_sequences": {
    "config_seq":  42,
    "secrets_seq": 17,
    "bundles_seq":  8,
    "plugins_seq":  11,
    "roles_seq":    3,
    "aliases_seq":  29,
    "trust_seq":    7
  },
  "deltas": {
    "secrets_seq": {
      "from":    16,
      "to":      17,
      "changes": [
        {
          "alias":   "mysql",
          "reason":  "secret_rotation",
          "action":  "refresh"
        }
      ]
    }
  },
  "relay_state": "fresh",
  "relay_stale_since": null
}
```

A delta with `"action": "refresh"` tells the node to re-request the secret for that alias — the relay delivers the new encrypted secret in a follow-up call. The secret value itself is never in the poll response.

### 7.4 Push for Urgent Updates

Some changes cannot wait for the next poll cycle. Secret rotation, security alerts, role changes, and Trust Bundle updates need immediate propagation. The push path uses the gossip layer within the local cluster:

```rust
// RPS relay injects urgent update into local gossip
GossipEffect::RpsUrgentUpdate {
    category:    RpsCategory::Secrets,
    sequence:    17,
    reason:      "secret_rotation",
    affected:    vec!["mysql"],
}
```

Nodes receive the gossip update and immediately issue a poll request to the relay rather than waiting for the next scheduled poll. The gossip is a wake-up signal — the actual delta still comes via the poll protocol.

Push scenarios:

| Event | Push? | Reason |
|---|---|---|
| Secret rotation | Yes | Security — old credentials may be revoked |
| Role assignment change | Yes | Node needs resources immediately |
| Bundle deployment | Yes | New version should start serving |
| Trust Bundle update | Yes | New RPS membership must propagate immediately |
| Config change | No | Wait for next poll — non-urgent |
| Alias host change | No | Wait for next poll |
| New alias definition | No | Wait for next poll |

### 7.5 Relay Cache and Degraded State

The relay maintains a local cache of all provisioning state for its cluster. If the primary is unreachable, the relay serves cached state with a staleness warning:

```
relay_state: "stale"
relay_stale_since: "2026-03-05T14:32:00Z"
relay_stale_ms: 120000
```

Nodes receiving a stale relay response continue operating on their current state. They do not fail or restart. They emit a `RelayStale` event to the EventBus which surfaces in metrics and alerts.

Stale state implications:

| Operation | Stale relay behavior |
|---|---|
| Normal app requests | Unaffected — node has credentials and config |
| Secret rotation | Delayed until primary reachable |
| New bundle deployment | Blocked |
| Role changes | Blocked |
| New node bootstrap | Blocked — cannot provision a new node without primary |
| DBR recovery | Blocked |

Cache TTL: 24 hours by default. After TTL expires, the relay marks itself unavailable and nodes fall back to their own last-known state. The relay never serves state older than its TTL.

---

## 8. Secret Broker

### 8.1 The Broker Never Stores Secrets

The secret broker is a fetch-and-forward service. Its job:

1. Receive a secret request with alias + requesting node identity
2. Validate authorization (alias allowed for this node's roles + app)
3. Look up which backend and secret name the alias maps to for this environment
4. Fetch the raw secret value from the backend (host side — never in the isolate)
5. Encrypt the secret value using the requesting node's public key
6. Return the encrypted envelope
7. Discard the raw secret value from memory (zeroize)

The secret value exists in RPS memory only during steps 4-6. It is never written to disk, never logged, never cached. The secret broker handler runs inside the ProcessPool; the actual backend fetch (step 4) is a host-side call via opaque resource token — the isolate never sees the raw credential.

### 8.2 Encrypted Delivery

Secrets are delivered using X25519 key exchange + XChaCha20-Poly1305 encryption:

```rust
pub struct SecretEnvelope {
    pub alias:          String,
    pub environment:    String,
    pub driver:         String,
    pub host:           String,
    pub database:       Option<String>,
    pub encrypted_cred: Vec<u8>,   // XChaCha20-Poly1305 encrypted username:password
    pub ephemeral_key:  Vec<u8>,   // X25519 ephemeral public key for decryption
    pub nonce:          Vec<u8>,
    pub issued_at:      DateTime<Utc>,
    pub expires_at:     DateTime<Utc>,
}
```

The requesting node decrypts using its private key + the ephemeral key. No one else can decrypt — not the relay, not other nodes, not the RPS primary after the ephemeral key is discarded.

### 8.3 Secret Request Flow

```
Node A needs credentials for alias "mysql"
        │
        ▼
Node A → Relay: POST /rps/v1/secrets/mysql
    Authorization: NodeCertificate (PASETO)
    Body: { "app_id": "order-service", "node_public_key": "..." }
        │
        ▼
Relay validates: node-a has a role that includes alias "mysql"
Relay forwards to primary (or serves from cache if fresh enough)
        │
        ▼
Primary: alias "mysql" in prod → secret_name "mysql-p23-d2" → Vault backend
Primary handler (in ProcessPool) triggers host-side fetch via resource token
Host fetches raw credential from Vault
Host encrypts with node-a's public key
Host discards raw credential (zeroize)
        │
        ▼
Primary → Relay → Node A: SecretEnvelope (encrypted)
        │
        ▼
Node A decrypts with private key
Node A passes raw credential to Datasource layer
Raw credential held only in Datasource object memory
Never touches disk, never logged
```

### 8.4 Secret Rotation

Secret rotation follows the push + poll pattern:

```
1. Operator rotates secret in Vault (or automated rotation fires)
2. Vault notifies RPS primary via webhook (or operator triggers manually):
   riversctl rps secret rotate --alias mysql --env prod

3. RPS primary increments secrets_seq: 16 → 17

4. RPS pushes GossipEffect::RpsUrgentUpdate to relay
   relay propagates via gossip to all cluster nodes

5. Nodes wake up, poll relay for secrets delta
   Relay delivers: "alias mysql changed, action: refresh"

6. Each node re-requests SecretEnvelope for alias mysql
   Gets new credentials encrypted for that node specifically

7. Each node updates its Datasource connection pool
   Old connections drained, new connections established with new credentials
   Zero-downtime rotation via connection pool drain hooks (Epic 35)
```

The CredentialRotated event (Epic 35) is the hook that triggers step 7 — the connection pool responds by draining old connections gracefully.

### 8.5 Backend Support

| Backend | Config key | Notes |
|---|---|---|
| HashiCorp Vault KV2 | `vault` | Recommended for prod |
| AWS Secrets Manager | `aws_sm` | Cloud-native option |
| Infisical | `infisical` | Open source Vault alternative |
| SOPS file | `sops` | GitOps-friendly, dev/staging |
| InMemory | `inmemory` | Dev only |
| RPS Internal | `rps_internal` | Secrets stored in RPS database — for RPS own credentials only |

The `rps_internal` backend is special — it allows the RPS to store its own operational secrets without requiring an external backend. It is not available to application aliases.

---

## 9. Bundle and Plugin Distribution

### 9.1 Placement Policy

The RPS primary knows which app bundles should run on which nodes. Placement is determined by a combination of role requirements and explicit placement config:

```toml
# Bundle placement — managed by RPS primary

[bundles.order-service]
version  = "2.1.0"
path     = "bundles/order-service-2.1.0.zip"
checksum = "sha256:abc123..."

[bundles.order-service.placement]
strategy = "role"
roles    = ["api", "data"]
min_nodes = 2
max_nodes = 0                # 0 = all eligible nodes

[bundles.storefront]
version  = "1.5.2"
path     = "bundles/storefront-1.5.2.zip"

[bundles.storefront.placement]
strategy  = "explicit"
nodes     = ["node-a", "node-b", "node-d"]

[bundles.rps-master]
version   = "1.0.0"
path      = "bundles/rps-master-1.0.0.zip"

[bundles.rps-master.placement]
strategy  = "mode"
mode      = "rps-master"
```

### 9.2 Plugin Distribution

Driver plugins (.so/.dylib files) are distributed per role. When a role includes a driver, the corresponding plugin binary is provisioned to nodes with that role:

```toml
[plugins.elasticsearch]
path     = "plugins/rivers-elasticsearch-driver-0.4.0.so"
checksum = "sha256:def456..."
roles    = ["data", "search"]

[plugins.neo4j]
path     = "plugins/rivers-neo4j-driver-0.2.1.so"
checksum = "sha256:ghi789..."
roles    = ["graph"]
```

Plugin distribution follows the same sequence-number delta protocol as other provisioning changes. A plugin update increments `plugins_seq`, nodes with the affected role receive the new binary, verify the checksum, and load it via the existing plugin loading mechanism (Epic 20).

### 9.3 Library Distribution

Libraries declared in roles are provisioned to nodes as files. External libraries are fetched by the relay (not by individual nodes) and verified against their declared hash before distribution:
- One fetch per cluster per library version, not one fetch per node
- The relay verifies integrity before distributing
- Nodes receive pre-verified libraries — no external network calls from app nodes

```
Role "search" declares: fuse.js@7.0.0 from external CDN

Relay fetches fuse.js once from CDN
Relay verifies sha256 hash
Relay stores in local bundle cache
Relay distributes to nodes with "search" role via poll delta
Nodes receive and install — no CDN access from app nodes
```

### 9.4 Checksum Verification

Every distributed artifact — bundles, plugins, libraries — carries a SHA-256 checksum. Nodes verify before installing. Verification failure is a security event:

```rust
EventType::ArtifactVerificationFailed {
    node_id:    String,
    artifact:   String,
    expected:   String,
    actual:     String,
    source:     String,
}
```

This event fires to the EventBus and should trigger an alert. A checksum mismatch means either a corrupt delivery or a tampered artifact.

---

## 10. Node Lifecycle

### 10.1 New Node Bootstrap (Join)

A new node joining a cluster goes through a structured bootstrap sequence. The node receives everything it needs in order — identity first, then Trust Bundle, then config, then secrets, then bundles and plugins. It does not serve traffic until the sequence completes.

```
Step 1: Generate identity
    riversd generates Ed25519 keypair on first start
    Stores in /var/rivers/cluster/node.key + node.pub
    Derives node_id from public key hash

Step 2: Present join token to relay
    POST /rps/v1/nodes/join
    Body: { node_public_key, join_token, requested_roles (optional) }

Step 3: Relay forwards to primary
    Primary validates join token (single-use, TTL check)
    Primary issues Node Certificate (PASETO, signed by primary)
    Primary marks join token consumed
    Primary assigns roles (from join request or default)

Step 4: Node receives bootstrap package
    {
      node_certificate: "v4.public...",
      trust_bundle:     { version, members: [...], expires_at },   ← NEW in v2
      assigned_roles:   ["api", "search"],
      config_seq:       42,
      sequences:        { ... current sequences ... }
    }

Step 5: Node validates Trust Bundle
    Verify Trust Bundle signature (signed by primary's Ed25519 key)
    Store locally as authoritative RPS membership list
    Use for future failover decisions

Step 6: Node provisions resources for its roles
    Poll relay for full role resource set (not delta — full on bootstrap)
    Receive driver plugins → install and load
    Receive libraries → verify checksums, install
    Request secret envelopes for all role aliases
    Decrypt and pass to Datasource layer

Step 7: Node requests bundle deployments
    Poll relay for bundles assigned to this node's roles
    Download, verify, deploy app bundles

Step 8: Node joins cluster
    Announce readiness via gossip
    Begin serving traffic
    Begin regular poll cycle
```

The relay coordinates steps 4-8 locally — the primary is only involved in step 3. Bootstrap is fast because the relay has cached provisioning state.

### 10.2 DBR (Damaged Beyond Repair) Replacement

DBR is the procedure for replacing a catastrophically unrecoverable node with a new node that assumes the dead node's identity and role.

**Issuing a DBR token:**

```bash
riversctl rps dbr issue \
    --node node-a \
    --cluster prod-cluster-1 \
    --reason "disk failure — /var/rivers unrecoverable" \
    --issuer "ops@company.com"

Output:
  DBR Token: DBR-7xKm9p2...
  Scope:     node-a in prod-cluster-1
  Expires:   2026-03-05T17:00:00Z (2 hours)
  Single-use: yes
  
  WARNING: This token allows a new node to assume node-a's identity.
  Store securely. Do not share.
```

DBR token issuance requires `cluster_admin` RBAC role. The `--reason` flag is mandatory for audit trail.

**Using a DBR token (node-f replacing dead node-a):**

```bash
riversd --config config.toml \
    --join relay.prod-cluster-1.internal:9091 \
    --dbr-token DBR-7xKm9p2... \
    --assume-identity node-a
```

**DBR bootstrap sequence:**

```
node-f presents DBR token + public key to relay
Relay forwards to primary

Primary validates:
  1. DBR token valid, not expired, not consumed
  2. DBR token scoped to node-a in prod-cluster-1
  3. node-a is confirmed unreachable (quorum check)
  4. If node-a reachable → require quorum vote before proceeding (live node DBR)

Primary accepts DBR:
  1. Marks DBR token consumed
  2. Issues Node Certificate with node_id = "node-a" to node-f's public key
  3. node-f's public key becomes the new key for node-a identity
  4. Old node-a public key revoked (added to revocation list)
  5. node-f proceeds through standard bootstrap with node-a's roles
  6. Trust Bundle delivered to node-f as part of bootstrap package
```

**Live node DBR (node-b accidentally DBR'd):**

```
node-g presents DBR token claiming node-b identity
Primary detects node-b IS reachable

Primary initiates quorum vote:
  "DBR claim filed against live node-b — confirm?"
  Requires majority vote from cluster members

node-b is notified:
  "DBR claim filed against you — vote in progress"
  node-b can challenge the vote

If quorum confirms:
  node-b receives NodeEjected signal
  node-b initiates graceful drain transfer to remaining peers
  node-b shuts down
  node-g assumes node-b identity

If quorum rejects:
  DBR denied
  node-g denied cluster entry
  Alert fired: "DBR attempt on live node rejected"
  DBR token invalidated
```

### 10.3 Node Decommission (Planned)

Planned removal differs from DBR — the node is healthy and participates in its own exit:

```bash
riversctl rps node decommission node-a --cluster prod-cluster-1

Sequence:
  1. Mark node-a as draining in RPS node registry
  2. Stop routing new app traffic to node-a (gossip update)
  3. Trigger drain transfer for any in-flight broker messages
  4. Remove node-a's role assignments (roles migrate to remaining nodes)
  5. node-a acknowledges decommission, shuts down gracefully
  6. Node Certificate revoked
  7. node-a removed from cluster membership
```

### 10.4 Node Health and Monitoring

The relay tracks the health of all nodes in its cluster and reports to the primary:

```json
GET /rps/v1/clusters/prod-cluster-1/nodes

{
  "nodes": [
    {
      "node_id":     "node-a",
      "status":      "healthy",
      "roles":       ["api", "search"],
      "sequences":   { "config_seq": 42, "secrets_seq": 17, "trust_seq": 7 },
      "last_poll":   "14s ago",
      "uptime_ms":   86400000
    },
    {
      "node_id":     "node-b",
      "status":      "stale",
      "stale_since": "5m ago",
      "roles":       ["data"]
    }
  ],
  "relay_status": "fresh",
  "primary_last_seen": "8s ago"
}
```

---

## 11. RPS as App Bundle

### 11.1 Bundle Structure

The RPS primary and relay are App Bundles — the same format as any Rivers application. They are developed, tested, and deployed using standard Rivers tooling.

**rps-master.zip:**
```
rps-master.zip
├── manifest.json
├── backend/
│   ├── handlers/
│   │   ├── alias_registry.ts      ← alias CRUD and environment resolution
│   │   ├── node_registry.ts       ← join, DBR, decommission, node health
│   │   ├── role_manager.ts        ← role definitions, assignment, migration
│   │   ├── secret_broker.ts       ← fetch-and-forward, encrypted delivery
│   │   ├── bundle_distributor.ts  ← placement policy, bundle/plugin delivery
│   │   ├── poll_handler.ts        ← sequence delta computation and response
│   │   ├── trust_bundle.ts        ← Trust Bundle issuance and rotation  ← NEW
│   │   └── dbr_handler.ts         ← DBR token issuance, quorum votes
│   └── lib/
│       └── rps_crypto.wasm        ← X25519 + XChaCha20-Poly1305 (WASM for perf)
└── config/
    ├── views.toml
    ├── dataviews.toml
    └── schemas.toml
```

**rps-relay.zip:**
```
rps-relay.zip
├── manifest.json
├── backend/
│   ├── handlers/
│   │   ├── relay_proxy.ts     ← forward to primary, serve from cache
│   │   ├── cache_manager.ts   ← local state cache, staleness tracking
│   │   ├── gossip_bridge.ts   ← inject urgent updates into local gossip
│   │   └── node_health.ts     ← track local node health, report to primary
│   └── lib/
│       └── rps_crypto.wasm
└── config/
    ├── views.toml
    └── dataviews.toml
```

### 11.2 RPS Views

The RPS API is a standard Rivers View configuration. Every endpoint is a declarative or CodeComponent view. All CodeComponent handlers run inside the ProcessPool (Section 3):

```toml
# rps-master views.toml

[api.views.node_join]
path              = "/rps/v1/nodes/join"
view_type         = "Rest"
libs              = []
datasources       = ["rps_db"]
allow_outbound_http = false
methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/node_registry.ts",
    entrypoint_function = "onNodeJoin",
    resources           = ["rps_db"]
}}

[api.views.node_poll]
path              = "/rps/v1/poll"
view_type         = "Rest"
datasources       = ["rps_db"]
allow_outbound_http = false
methods.GET.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/poll_handler.ts",
    entrypoint_function = "onPoll",
    resources           = ["rps_db"]
}}

[api.views.secret_request]
path              = "/rps/v1/secrets/{alias}"
view_type         = "Rest"
libs              = ["rps_crypto.wasm"]
datasources       = ["rps_db", "lockbox_backends"]
allow_outbound_http = false   # lockbox backend fetch is host-side
methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/secret_broker.ts",
    entrypoint_function = "onSecretRequest",
    resources           = ["rps_db", "lockbox_backends"]
}}

[api.views.trust_bundle_issue]
path              = "/rps/v1/trust-bundle"
view_type         = "Rest"
libs              = ["rps_crypto.wasm"]
datasources       = ["rps_db"]
allow_outbound_http = false
methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/trust_bundle.ts",
    entrypoint_function = "onTrustBundleIssue",
    resources           = ["rps_db"]
}}

[api.views.alias_define]
path              = "/rps/v1/aliases/{alias}"
view_type         = "Rest"
datasources       = ["rps_db"]
allow_outbound_http = false
methods.PUT.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/alias_registry.ts",
    entrypoint_function = "onAliasDefine",
    resources           = ["rps_db"]
}}

[api.views.role_assign]
path              = "/rps/v1/nodes/{node_id}/roles"
view_type         = "Rest"
datasources       = ["rps_db"]
allow_outbound_http = false
methods.PUT.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/role_manager.ts",
    entrypoint_function = "onRoleAssign",
    resources           = ["rps_db"]
}}

[api.views.dbr_issue]
path              = "/rps/v1/dbr/tokens"
view_type         = "Rest"
datasources       = ["rps_db"]
allow_outbound_http = false
methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/dbr_handler.ts",
    entrypoint_function = "onDbrIssue",
    resources           = ["rps_db"]
}}
```

### 11.3 RPS Manifest

The RPS primary bundle declares its own aliases — it uses the same alias system it provides to other applications:

```json
{
  "app": {
    "id":      "rps-master",
    "version": "2.0.0",
    "name":    "Rivers Provisioning Service Primary",
    "app_type": "RestApi"
  },
  "deployment": {
    "strategy":  "rolling",
    "replicas":  1,
    "health_check": { "backend_endpoint": "/rps/v1/health" }
  },
  "backend_config": {
    "api_prefix": "/rps"
  },
  "resource_bindings": {
    "required": {
      "rps_db":           { "alias": "rps-db"           },
      "lockbox_backends": { "alias": "rps-lockbox-meta" }
    }
  },
  "event_subscriptions": [
    "rps.secret.rotation_requested",
    "rps.node.dbr_vote",
    "rps.trust_bundle.rotation_triggered"
  ]
}
```

### 11.4 RPS Deployment Lifecycle

RPS upgrades follow the standard Rivers promotion pipeline:

```bash
# Deploy new RPS version to staging RPS cluster
riversctl deploy rps-master-2.0.0.zip \
    --env staging \
    --cluster rps-staging-cluster

# Run RPS test suite
riversctl test rps-master-2.0.0.zip --cluster rps-staging-cluster

# Approve for production
riversctl deployment approve <deployment-id>

# Promote to production
riversctl promote <deployment-id> --from staging --to prod

# The RPS alternate takes over during primary upgrade
# Zero-downtime — relays fail over to alternate automatically
```

### 11.5 RPS Observability

Because the RPS runs on `riversd`, it receives the full observability stack without any additional instrumentation:

**Traces:** Every provisioning operation — secret fetch, node join, poll response — produces an OpenTelemetry span. A secret rotation that triggers across 50 nodes produces a trace tree showing every node's re-provisioning in sequence.

**Metrics:** Standard Rivers HTTP metrics plus RPS-specific metrics emitted via the EventBus → MetricsCollector path:
- `rps_poll_requests_total` — poll volume per cluster
- `rps_secret_fetches_total` — secret broker fetch counts per alias and backend
- `rps_node_bootstrap_duration_ms` — how long new nodes take to become ready
- `rps_relay_staleness_ms` — how stale each relay's cache is
- `rps_dbr_events_total` — DBR operations per cluster
- `rps_trust_bundle_version` — current Trust Bundle version per cluster

**Logs:** Structured JSON logs with trace_id, app_id=rps-master, node_id. Secret values never appear in logs — the secret broker handler logs alias names and operation outcomes only. OTel log signal is multiplexed over the existing RPS protocol connection for cluster nodes.

---

## 12. RPS Client Driver

### 12.1 Purpose

The `rps-client` is a Rivers driver plugin that speaks the RPS protocol. It is how regular nodes and relays connect to the provisioning service. Regular nodes point it at their local relay. Relays point it at the primary.

The driver name is `"rps-client"`. It implements `DatabaseDriver` — the RPS API is request/response, so the standard driver contract is appropriate.

### 12.2 Connection

```toml
# Regular node config — points at local relay
[data.datasources.rps]
driver             = "rps-client"
host               = "https://node-e.prod-cluster-1.internal:9443"
credentials_source = "lockbox://local/node-cert"

[data.datasources.rps.extra]
mode    = "node"     # or "relay" for relay→primary connections
cluster = "prod-cluster-1"
```

The `credentials_source` references the node's own certificate — the node authenticates to the relay using its Node Certificate (PASETO token derived from its Ed25519 keypair).

### 12.3 Query Model

| Statement | Parameters | Description |
|---|---|---|
| `"poll"` | `sequences` (JSON object) | Check for provisioning updates |
| `"join"` | `join_token`, `public_key`, `requested_roles` | Bootstrap node join |
| `"secret"` | `alias`, `app_id`, `node_public_key` | Request secret envelope |
| `"bundle_list"` | — | List bundles assigned to this node |
| `"bundle_get"` | `bundle_id`, `version` | Download a specific bundle |
| `"plugin_get"` | `plugin_name`, `version` | Download a driver plugin binary |
| `"dbr_join"` | `dbr_token`, `public_key`, `assume_identity` | DBR replacement bootstrap |
| `"trust_bundle"` | — | Request current Trust Bundle |
| `"ping"` | — | RPS relay/primary health check |

### 12.4 Connection Pool

The `rps-client` datasource uses a small connection pool (default `max_size = 3`). Poll requests are frequent but fast — a pool of 3 handles bursts without over-connecting to the relay.

---

## 13. Security Model

### 13.1 Authentication Layers

| Layer | Mechanism | Scope |
|---|---|---|
| Node → Relay | PASETO Node Certificate (Ed25519, issued by RPS primary) | All RPS API calls |
| Relay → Primary | Ed25519 keypair (relay cert, issued at relay bootstrap) | Relay authentication |
| Primary → Alternate | Trust Bundle mutual pubkey verification | Alt promotion handshake |
| Admin API | Ed25519 keypair, localhost-only when unconfigured | riversctl operations |
| mTLS | Client certificates on port 9443 | Transport-level enforcement |

### 13.2 Node Certificate (PASETO)

```
Node Certificate claims:
  node_id:    "node-a"
  cluster_id: "prod-cluster-1"
  roles:      ["api", "search"]
  issued_at:  2026-03-10T00:00:00Z
  expires_at: 2026-03-11T00:00:00Z   (24h TTL, configurable)
  pubkey:     "ed25519:..."           (node's own public key)

Signed by: RPS primary Ed25519 key
```

The relay verifies the PASETO signature on every request — no shared session state needed.

### 13.3 Trust Bundle as Root of Trust

The Trust Bundle is the root of trust for all cluster authentication. It is:
- Signed by the current primary's Ed25519 key
- Delivered to every node at bootstrap and on every rotation
- The source of truth for which RPS instance to contact on failover
- Verified by nodes before accepting any RPS communication from an alternate

An alternate that has not issued a valid Trust Bundle (signed with its known pubkey from a prior bundle) cannot be impersonated.

### 13.4 Gossip and Raft Authentication

All cluster internal communication — Raft RPCs and gossip messages — is authenticated using the Node Certificate's Ed25519 key:

- Gossip messages: HMAC-SHA256 signed using node's private key, verified by recipients against Node Certificate pubkey
- Raft RPCs: Node Certificate validated on all inbound Raft connections
- A node that cannot produce a valid Node Certificate cannot participate in cluster consensus

This closes SEC-1 (unauthenticated Raft), SEC-2 (gossip forgery), and SEC-3 (Raft RPC manipulation).

---

## 14. Configuration Reference

### 14.1 RPS Primary Config

```toml
[base.rps]
port             = 9443
tls_cert         = "/var/rivers/rps/tls.crt"
tls_key          = "/var/rivers/rps/tls.key"
client_ca        = "/var/rivers/rps/node-ca.crt"

[base.rps.trust_bundle]
rotation_interval_h  = 24       # time-based rotation (default 24h)
max_members          = 5        # maximum alternates including primary

[base.rps.secrets]
# How long SecretEnvelopes are valid before nodes must re-request
envelope_ttl_hours   = 1        # 1h default — security > convenience

[base.rps.tokens]
# How long DBR tokens are valid
dbr_token_ttl_hours  = 2

# How long Node Certificates are valid before renewal
node_cert_ttl_hours  = 24

# Relay poll interval enforcement
relay_poll_interval_s = 30
relay_cache_ttl_hours = 24

[data.datasources.rps_db]
driver             = "postgresql"
host               = "localhost:5432"
database           = "rps"
credentials_source = "lockbox://rps/db-creds"

[data.datasources.lockbox_backends]
driver             = "rps-lockbox-meta"
credentials_source = "lockbox://rps/lockbox-meta-creds"

[runtime.process_pools.default]
engine          = "v8"
workers         = 4
max_heap_mb     = 128
task_timeout_ms = 5000

[runtime.process_pools.wasm]
engine             = "wasmtime"
workers            = 2
max_memory_mb      = 64
task_timeout_ms    = 10000
epoch_interval_ms  = 10
```

### 14.2 RPS Relay Config (within cluster config)

```toml
# cluster-prod-1.toml (excerpt — relay node config)

[base.rps_relay]
enabled            = true
primary_url        = "https://rps-primary.internal:9443"
primary_credentials_source = "lockbox://local/relay-cert"
cache_ttl_hours    = 24
stale_warn_minutes = 30
poll_interval_s    = 30

[data.datasources.rps_primary]
driver             = "rps-client"
host               = "https://rps-primary.internal:9443"
credentials_source = "lockbox://local/relay-cert"

[data.datasources.rps_primary.extra]
mode    = "relay"
cluster = "prod-cluster-1"
```

### 14.3 Regular Node Config

```toml
# Regular cluster node config (excerpt)

[data.datasources.rps]
driver             = "rps-client"
host               = "https://node-e.prod-cluster-1.internal:9443"
credentials_source = "lockbox://local/node-cert"

[data.datasources.rps.extra]
mode    = "node"
cluster = "prod-cluster-1"

[base.rps_client]
poll_interval_s    = 60
bootstrap_on_start = true
```

---

## 15. riversctl RPS Commands

```bash
# ── Alias Management ────────────────────────────────────────────

riversctl rps alias define <alias> \
    --env <environment> \
    --driver <driver-name> \
    --secret-name <secret-name> \
    --backend <vault|sops|aws_sm|inmemory> \
    --host <host:port> \
    [--database <db>] \
    [--vault-path <path>] \
    [--arn <aws-arn>]

riversctl rps alias list [--env <environment>]
riversctl rps alias show <alias>
riversctl rps alias update <alias> --env <environment> [--host ...] [--secret-name ...]
riversctl rps alias remove <alias> --env <environment>
riversctl rps alias usage <alias> [--env <environment>]
riversctl rps secret rotate --alias <alias> --env <environment>

# ── Role Management ─────────────────────────────────────────────

riversctl rps role define <role-name> --file role-definition.toml
riversctl rps role list
riversctl rps role show <role-name>
riversctl rps role assign <role-name> --node <node-id> --cluster <cluster-id>
riversctl rps role remove <role-name> --node <node-id> --cluster <cluster-id>
riversctl rps role move --role <role-name> \
    --from <node-id> \
    --to <node-id> \
    --cluster <cluster-id>

# ── Trust Bundle Management ──────────────────────────────────────

# Show current Trust Bundle for a cluster
riversctl rps trust-bundle show --cluster <cluster-id>

# Force immediate rotation
riversctl rps trust-bundle rotate --cluster <cluster-id>

# Add an alternate RPS instance
riversctl rps trust-bundle add-member \
    --address <host:port> \
    --pubkey <ed25519-pubkey> \
    --priority <1-5>

# Remove a member from the Trust Bundle
riversctl rps trust-bundle remove-member --address <host:port>

# Show all nodes' current trust_seq vs primary trust_seq
riversctl rps trust-bundle drift --cluster <cluster-id>

# ── Node Management ─────────────────────────────────────────────

riversctl rps node list --cluster <cluster-id>
riversctl rps node show <node-id> --cluster <cluster-id>
riversctl rps node join-token --cluster <cluster-id> [--ttl-hours 24]
riversctl rps node decommission <node-id> --cluster <cluster-id>

riversctl rps dbr issue \
    --node <node-id> \
    --cluster <cluster-id> \
    --reason "<reason text>"

# ── Cluster and Relay Management ────────────────────────────────

riversctl rps relay register \
    --cluster <cluster-id> \
    --relay-node <node-id>
riversctl rps relay status --cluster <cluster-id>
riversctl rps cluster list
riversctl rps cluster status <cluster-id>

# ── Observability ───────────────────────────────────────────────

riversctl rps health
riversctl rps drift --cluster <cluster-id>
riversctl rps audit alias <alias> --env <environment> [--since 24h]
riversctl rps audit node <node-id> [--since 24h]
```

---

## 16. Open Questions

These items require decisions before implementation begins.

| # | Question | Options | Status |
|---|---|---|---|
| 1 | RPS primary database backend | PostgreSQL only (production), or support SQLite for single-cluster deployments? | **Open** |
| 2 | Relay designation mechanism | Role-based (`rps-relay` role) vs mode-based (`--mode rps-relay`). Role-based is more dynamic; mode-based is more explicit. | **Open** |
| 3 | SecretEnvelope TTL default | 1 hour (more fetches, less exposure) vs 24 hours (less load, longer compromise window). Default set to 1h in this spec pending final decision. | **Open** |
| 4 | Cross-cluster alias overrides | Required for v1 or deferred? | **Open** |
| 5 | RPS primary bootstrap UX | Manual credential config acceptable, or implement guided `riversctl rps init`? | **Open** |
| 6 | Admin API no-auth escape hatch | **Resolved.** `--no-admin-auth` flag enables session-only no-auth for local dev. Localhost-only binding enforced when unconfigured. Staged key rotation for production rotation. |
| 7 | Log shipper | **Resolved.** TCP shipper removed. OTel logs signal multiplexed over existing RPS protocol connection for cluster nodes. Local file default for non-cluster. |
| 8 | WebSocket rate limiting | **Resolved.** Token bucket per-view. Defaults: `messages_per_sec = 100`, `burst = 20`. Configurable per view declaration. |
