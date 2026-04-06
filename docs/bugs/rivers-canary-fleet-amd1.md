# Rivers Canary Fleet — Amendment AMD-1

**Date:** 2026-04-02
**Applies to:** `rivers-canary-fleet-spec.md` v1.0
**Resolves:** All findings in `canary-fleet-gap-analysis.md`
**Instruction:** Absorb into source spec. After absorption, this file is historical only.

---

## AMD-1.1 — Naming Convention Rules (NEW SECTION)

Insert after "Design Principles" section, before "Final Bundle Structure":

---

### Naming Conventions — Mandatory

These rules are not guidelines. Violations produce broken wiring at runtime.

**Rule 1: Datasource name identity.** The datasource name in `resources.toml`, `app.toml [data.datasources.*]`, and handler `resources = [...]` MUST be identical strings. There is no aliasing. If `resources.toml` says `name = "canary-pg"`, then `app.toml` MUST use `[data.datasources.canary-pg]` and views MUST use `resources = ["canary-pg"]`.

```
# CORRECT — all three match:
resources.toml:   name = "canary-pg"
app.toml:         [data.datasources.canary-pg]
view handler:     resources = ["canary-pg"]

# WRONG — name mismatch:
resources.toml:   name = "canary-pg"
app.toml:         [data.datasources.pg]        ← BREAKS: "pg" ≠ "canary-pg"
```

**Rule 2: Path prefix slash.** Every `path` value in `[api.views.*]` MUST begin with `/`. No exceptions.

```
# CORRECT
path = "/canary/sql/pg/insert"

# WRONG
path = "canary/sql/pg/insert"    ← BREAKS: missing leading /
```

**Rule 3: Language field.** All `.ts` handler files MUST use `language = "typescript"`. The value `"javascript"` is for `.js` files only.

```
# CORRECT — .ts file
module   = "handlers/sql-tests.ts"
language = "typescript"

# WRONG — .ts file declared as javascript
module   = "handlers/sql-tests.ts"
language = "javascript"            ← BREAKS: swc compiler not invoked
```

**Rule 4: Test ID format.** Test IDs use uppercase with hyphens: `PROFILE-DOMAIN-FEATURE-OPERATION`. Every token is separated by a single hyphen. Multi-word tokens use hyphens, not concatenation.

```
# CORRECT
RT-CTX-TRACE-ID
RT-CTX-APP-ID
RT-CTX-STORE-GET-SET
RT-RIVERS-CRYPTO-HASH

# WRONG
RT-CTX-TRACEID         ← missing hyphen in TRACE-ID
RT-CTX-APPID           ← missing hyphen in APP-ID
RT-CTX-STORE           ← too vague, spec says STORE-GET-SET
RT-CRYPTO-HASHPASSWORD ← missing RIVERS prefix, wrong concatenation
```

The spec's Test Inventory tables are the canonical source. Copy test_id values verbatim — do not rename, abbreviate, or combine.

**Rule 5: One test per endpoint.** Each spec test_id gets its own endpoint. Do NOT combine multiple test_ids into a single handler (e.g., don't merge NOSQL-MONGO-INSERT and NOSQL-MONGO-FIND into NOSQL-MONGO-CRUD). The self-reporting protocol requires each verdict to have exactly one test_id.

**Rule 6: HTTP method fidelity.** The Test Inventory table's "Method" column is the required HTTP method. INSERT/UPDATE/DELETE tests use POST/PUT/DELETE respectively. Do not implement write tests as GET.

---

## AMD-1.2 — Init Handler DDL Wiring (REPLACES Part 3 init handler section)

The init handler requires DDL DataViews declared in `app.toml`. The handler calls these DataViews, which execute through the three-gate enforcement path. The init handler CANNOT use `ctx.datasource().fromQuery("CREATE TABLE...")` — pseudo DataViews don't support DDL. DDL must go through declared DataViews whose queries are DDL statements.

### canary-sql/app.toml — DDL DataViews (add before CRUD DataViews)

```toml
# ─────────────────────────────────────────────
# DDL DataViews — used ONLY by init handler
# These execute through the three-gate enforcement path:
#   Gate 1: driver guard (is_ddl_statement check)
#   Gate 2: ApplicationInit execution context
#   Gate 3: ddl_whitelist in riversd.toml
# ─────────────────────────────────────────────

[data.dataviews.pg_ddl_create]
datasource = "canary-pg"
query      = "CREATE TABLE IF NOT EXISTS canary_records (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), zname TEXT NOT NULL, age INTEGER NOT NULL, email TEXT, created_at TIMESTAMPTZ DEFAULT NOW())"

[data.dataviews.mysql_ddl_create]
datasource = "canary-mysql"
query      = "CREATE TABLE IF NOT EXISTS canary_records (id CHAR(36) PRIMARY KEY, zname VARCHAR(255) NOT NULL, age INT NOT NULL, email VARCHAR(255), created_at DATETIME DEFAULT CURRENT_TIMESTAMP)"

[data.dataviews.sqlite_ddl_create]
datasource = "canary-sqlite"
query      = "CREATE TABLE IF NOT EXISTS canary_records (id TEXT PRIMARY KEY, zname TEXT NOT NULL, age INTEGER NOT NULL, email TEXT, created_at TEXT DEFAULT (datetime('now')))"
```

### canary-sql/libraries/handlers/init.ts (REPLACES previous version)

```typescript
// Application Init Handler — runs in ApplicationInit execution context.
// This is the ONLY context where DDL is permitted.
//
// IMPORTANT: DDL goes through declared DataViews, NOT pseudo DataViews.
// The DataView name must match a [data.dataviews.*] entry in app.toml
// whose query IS the DDL statement.

export function initDatabase(ctx: any): void {
  // Gate test: these DataViews have DDL queries.
  // They will succeed because:
  //   Gate 1: driver recognizes CREATE TABLE as DDL → routes to ddl_execute()
  //   Gate 2: we are in ApplicationInit context → DDL allowed
  //   Gate 3: canary-pg@aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee01 is in ddl_whitelist
  ctx.dataview("pg_ddl_create", {});
  ctx.dataview("mysql_ddl_create", {});
  ctx.dataview("sqlite_ddl_create", {});

  // Seed PostgreSQL with 200 rows for max_rows test.
  // Use pg_insert DataView (the CRUD one, not DDL).
  for (let i = 0; i < 200; i++) {
    ctx.dataview("pg_insert", {
      zname: `seed-user-${i}`,
      age: 20 + (i % 60),
      email: `seed${i}@test.rivers`
    });
  }

  Rivers.log.info("canary-sql init complete: tables created, 200 PG seed rows inserted");
}
```

---

## AMD-1.3 — NoSQL Complete Config (REPLACES Part 4 sparse description)

Part 4 originally said "follow the identical pattern as canary-sql." This was insufficient. Here is the complete config.

### canary-nosql/app.toml

```toml
# ─────────────────────────────────────────────
# Datasources
# ─────────────────────────────────────────────

[data.datasources.canary-mongo]
driver = "mongodb"

[data.datasources.canary-mongo.config]
database = "canary"

[data.datasources.canary-es]
driver     = "elasticsearch"
nopassword = true

[data.datasources.canary-couch]
driver = "couchdb"

[data.datasources.canary-couch.config]
database = "canary"

[data.datasources.canary-cassandra]
driver     = "cassandra"
nopassword = true

[data.datasources.canary-cassandra.config]
keyspace = "canary"

[data.datasources.canary-ldap]
driver = "ldap"

[data.datasources.canary-ldap.config]
base_dn = "dc=rivers,dc=test"

[data.datasources.canary-redis]
driver = "redis"

# ─────────────────────────────────────────────
# DataViews — MongoDB
# ─────────────────────────────────────────────

[data.dataviews.mongo_insert]
datasource    = "canary-mongo"
query         = "canary_docs"
method        = "POST"
return_schema = "schemas/mongo-doc.schema.json"

[[data.dataviews.mongo_insert.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.mongo_insert.parameters]]
name     = "age"
type     = "integer"
required = true

[[data.dataviews.mongo_insert.parameters]]
name     = "tag"
type     = "string"
required = false

[data.dataviews.mongo_find]
datasource    = "canary-mongo"
query         = "canary_docs"
method        = "GET"
return_schema = "schemas/mongo-doc.schema.json"

[[data.dataviews.mongo_find.parameters]]
name     = "zname"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — Elasticsearch
# ─────────────────────────────────────────────

[data.dataviews.es_index]
datasource    = "canary-es"
query         = "canary_index"
method        = "POST"
return_schema = "schemas/es-doc.schema.json"

[[data.dataviews.es_index.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.es_index.parameters]]
name     = "age"
type     = "integer"
required = true

[data.dataviews.es_search]
datasource    = "canary-es"
query         = "canary_index"
method        = "GET"
return_schema = "schemas/es-doc.schema.json"

[[data.dataviews.es_search.parameters]]
name     = "zname"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — CouchDB
# ─────────────────────────────────────────────

[data.dataviews.couch_put]
datasource    = "canary-couch"
query         = "canary"
method        = "POST"
return_schema = "schemas/couch-doc.schema.json"

[[data.dataviews.couch_put.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.couch_put.parameters]]
name     = "age"
type     = "integer"
required = true

[data.dataviews.couch_get]
datasource    = "canary-couch"
query         = "canary"
method        = "GET"
return_schema = "schemas/couch-doc.schema.json"

[[data.dataviews.couch_get.parameters]]
name     = "zname"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — Cassandra
# ─────────────────────────────────────────────

[data.dataviews.cassandra_insert]
datasource    = "canary-cassandra"
query         = "INSERT INTO canary_rows (id, zname, age) VALUES ($id, $zname, $age)"
return_schema = "schemas/cassandra-row.schema.json"

[[data.dataviews.cassandra_insert.parameters]]
name     = "id"
type     = "string"
required = true

[[data.dataviews.cassandra_insert.parameters]]
name     = "zname"
type     = "string"
required = true

[[data.dataviews.cassandra_insert.parameters]]
name     = "age"
type     = "integer"
required = true

[data.dataviews.cassandra_select]
datasource    = "canary-cassandra"
query         = "SELECT * FROM canary_rows WHERE zname = $zname"
return_schema = "schemas/cassandra-row.schema.json"

[[data.dataviews.cassandra_select.parameters]]
name     = "zname"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — LDAP
# ─────────────────────────────────────────────

[data.dataviews.ldap_search]
datasource    = "canary-ldap"
query         = "(cn=$cn)"
method        = "GET"
return_schema = "schemas/ldap-entry.schema.json"

[[data.dataviews.ldap_search.parameters]]
name     = "cn"
type     = "string"
required = true

# ─────────────────────────────────────────────
# DataViews — Redis
# ─────────────────────────────────────────────

[data.dataviews.redis_set]
datasource = "canary-redis"
query      = "SET"

[[data.dataviews.redis_set.parameters]]
name     = "key"
type     = "string"
required = true

[[data.dataviews.redis_set.parameters]]
name     = "value"
type     = "string"
required = true

[data.dataviews.redis_get]
datasource    = "canary-redis"
query         = "GET"
return_schema = "schemas/redis-kv.schema.json"

[[data.dataviews.redis_get.parameters]]
name     = "key"
type     = "string"
required = true

# ─────────────────────────────────────────────
# Views — MongoDB
# ─────────────────────────────────────────────

[api.views.mongo_insert]
path      = "/canary/nosql/mongo/insert"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.mongo_insert.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "mongoInsert"
resources  = ["canary-mongo"]

[api.views.mongo_insert.parameter_mapping.body]
zname = "zname"
age   = "age"
tag   = "tag"

[api.views.mongo_find]
path      = "/canary/nosql/mongo/find"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.mongo_find.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "mongoFind"
resources  = ["canary-mongo"]

[api.views.mongo_find.parameter_mapping.query]
zname = "zname"

[api.views.mongo_admin_reject]
path      = "/canary/nosql/mongo/admin-reject"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.mongo_admin_reject.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/negative-nosql.ts"
entrypoint = "mongoAdminReject"
resources  = ["canary-mongo"]

# ─────────────────────────────────────────────
# Views — Elasticsearch
# ─────────────────────────────────────────────

[api.views.es_index]
path      = "/canary/nosql/es/index"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.es_index.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "esIndex"
resources  = ["canary-es"]

[api.views.es_index.parameter_mapping.body]
zname = "zname"
age   = "age"

[api.views.es_search]
path      = "/canary/nosql/es/search"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.es_search.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "esSearch"
resources  = ["canary-es"]

[api.views.es_search.parameter_mapping.query]
zname = "zname"

# ─────────────────────────────────────────────
# Views — CouchDB
# ─────────────────────────────────────────────

[api.views.couch_put]
path      = "/canary/nosql/couch/put"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.couch_put.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "couchPut"
resources  = ["canary-couch"]

[api.views.couch_put.parameter_mapping.body]
zname = "zname"
age   = "age"

[api.views.couch_get]
path      = "/canary/nosql/couch/get"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.couch_get.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "couchGet"
resources  = ["canary-couch"]

[api.views.couch_get.parameter_mapping.query]
zname = "zname"

# ─────────────────────────────────────────────
# Views — Cassandra
# ─────────────────────────────────────────────

[api.views.cassandra_insert]
path      = "/canary/nosql/cassandra/insert"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.cassandra_insert.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "cassandraInsert"
resources  = ["canary-cassandra"]

[api.views.cassandra_insert.parameter_mapping.body]
zname = "zname"
age   = "age"

[api.views.cassandra_select]
path      = "/canary/nosql/cassandra/select"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.cassandra_select.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "cassandraSelect"
resources  = ["canary-cassandra"]

[api.views.cassandra_select.parameter_mapping.query]
zname = "zname"

# ─────────────────────────────────────────────
# Views — LDAP
# ─────────────────────────────────────────────

[api.views.ldap_search]
path      = "/canary/nosql/ldap/search"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.ldap_search.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "ldapSearch"
resources  = ["canary-ldap"]

[api.views.ldap_search.parameter_mapping.query]
cn = "cn"

# ─────────────────────────────────────────────
# Views — Redis
# ─────────────────────────────────────────────

[api.views.redis_set]
path      = "/canary/nosql/redis/set"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.redis_set.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "redisSet"
resources  = ["canary-redis"]

[api.views.redis_set.parameter_mapping.body]
key   = "key"
value = "value"

[api.views.redis_get]
path      = "/canary/nosql/redis/get"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.redis_get.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/nosql-tests.ts"
entrypoint = "redisGet"
resources  = ["canary-redis"]

[api.views.redis_get.parameter_mapping.query]
key = "key"

[api.views.redis_admin_reject]
path      = "/canary/nosql/redis/admin-reject"
method    = "POST"
view_type = "Rest"
auth      = "session"

[api.views.redis_admin_reject.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/negative-nosql.ts"
entrypoint = "redisAdminReject"
resources  = ["canary-redis"]
```

### canary-nosql/schemas/mongo-doc.schema.json

```json
{
  "type": "object",
  "driver": "mongodb",
  "description": "Canary test document for MongoDB",
  "fields": [
    { "name": "_id",   "type": "string",  "required": false },
    { "name": "zname", "type": "string",  "required": true  },
    { "name": "age",   "type": "integer", "required": true  },
    { "name": "tag",   "type": "string",  "required": false }
  ]
}
```

### canary-nosql/schemas/es-doc.schema.json

```json
{
  "type": "object",
  "driver": "elasticsearch",
  "description": "Canary test document for Elasticsearch",
  "fields": [
    { "name": "zname", "type": "string",  "required": true  },
    { "name": "age",   "type": "integer", "required": true  }
  ]
}
```

### canary-nosql/schemas/couch-doc.schema.json

```json
{
  "type": "object",
  "driver": "couchdb",
  "description": "Canary test document for CouchDB",
  "fields": [
    { "name": "_id",   "type": "string",  "required": false },
    { "name": "_rev",  "type": "string",  "required": false },
    { "name": "zname", "type": "string",  "required": true  },
    { "name": "age",   "type": "integer", "required": true  }
  ]
}
```

### canary-nosql/schemas/cassandra-row.schema.json

```json
{
  "type": "object",
  "driver": "cassandra",
  "description": "Canary test row for Cassandra",
  "fields": [
    { "name": "id",    "type": "string",  "required": true  },
    { "name": "zname", "type": "string",  "required": true  },
    { "name": "age",   "type": "integer", "required": true  }
  ]
}
```

### canary-nosql/schemas/ldap-entry.schema.json

```json
{
  "type": "object",
  "driver": "ldap",
  "description": "Canary test entry for LDAP search",
  "fields": [
    { "name": "dn",   "type": "string", "required": true  },
    { "name": "cn",   "type": "string", "required": true  },
    { "name": "mail", "type": "string", "required": false }
  ]
}
```

### canary-nosql/libraries/handlers/nosql-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

// ─── MongoDB ───

export function mongoInsert(ctx: any): void {
  const t = new TestResult("NOSQL-MONGO-INSERT", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("mongo_insert", {
      zname: "CanaryMongo",
      age: 33,
      tag: "canary-test"
    });
    t.assert("insert_ok", result != null);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function mongoFind(ctx: any): void {
  const t = new TestResult("NOSQL-MONGO-FIND", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("mongo_find", { zname: "CanaryMongo" });
    t.assert("rows_returned", result?.rows?.length > 0,
      `count=${result?.rows?.length}`);
    t.assertEquals("zname_correct", "CanaryMongo",
      result?.rows?.[0]?.zname);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── Elasticsearch ───

export function esIndex(ctx: any): void {
  const t = new TestResult("NOSQL-ES-INDEX", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("es_index", {
      zname: "CanaryES",
      age: 44
    });
    t.assert("index_ok", result != null);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function esSearch(ctx: any): void {
  const t = new TestResult("NOSQL-ES-SEARCH", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("es_search", { zname: "CanaryES" });
    t.assert("rows_returned", result?.rows?.length > 0,
      `count=${result?.rows?.length}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── CouchDB ───

export function couchPut(ctx: any): void {
  const t = new TestResult("NOSQL-COUCH-PUT", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("couch_put", {
      zname: "CanaryCouch",
      age: 55
    });
    t.assert("put_ok", result != null);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function couchGet(ctx: any): void {
  const t = new TestResult("NOSQL-COUCH-GET", "NOSQL", "data-layer §3.1");
  try {
    const result = ctx.dataview("couch_get", { zname: "CanaryCouch" });
    t.assert("rows_returned", result?.rows?.length > 0);
    t.assertEquals("zname_correct", "CanaryCouch",
      result?.rows?.[0]?.zname);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── Cassandra ───

export function cassandraInsert(ctx: any): void {
  const t = new TestResult("NOSQL-CASSANDRA-INSERT", "NOSQL",
    "data-layer §3.1");
  try {
    const id = Rivers.crypto.randomHex(16);
    ctx.dataview("cassandra_insert", {
      id: id,
      zname: "CanaryCassandra",
      age: 66
    });
    t.assert("insert_ok", true);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function cassandraSelect(ctx: any): void {
  const t = new TestResult("NOSQL-CASSANDRA-SELECT", "NOSQL",
    "data-layer §3.1");
  try {
    const result = ctx.dataview("cassandra_select", {
      zname: "CanaryCassandra"
    });
    t.assert("rows_returned", result?.rows?.length > 0);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── LDAP ───

export function ldapSearch(ctx: any): void {
  const t = new TestResult("NOSQL-LDAP-SEARCH", "NOSQL",
    "driver-spec §6.3");
  try {
    const result = ctx.dataview("ldap_search", { cn: "admin" });
    t.assert("search_returned", result != null);
    t.assert("has_entries", result?.rows?.length > 0,
      `count=${result?.rows?.length}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

// ─── Redis ───

export function redisSet(ctx: any): void {
  const t = new TestResult("NOSQL-REDIS-SET", "NOSQL", "data-layer §4.1");
  try {
    ctx.dataview("redis_set", {
      key: "canary:test:key",
      value: "canary-value-42"
    });
    t.assert("set_ok", true);
    // Verify by reading back
    const result = ctx.dataview("redis_get", { key: "canary:test:key" });
    t.assertEquals("value_matches", "canary-value-42", result?.rows?.[0]?.value);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function redisGet(ctx: any): void {
  const t = new TestResult("NOSQL-REDIS-GET", "NOSQL", "data-layer §4.1");
  try {
    // Set a known key first
    ctx.dataview("redis_set", {
      key: "canary:get:test",
      value: "get-test-value"
    });
    const result = ctx.dataview("redis_get", { key: "canary:get:test" });
    t.assert("result_returned", result != null);
    t.assertEquals("value_correct", "get-test-value",
      result?.rows?.[0]?.value);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-nosql/libraries/handlers/negative-nosql.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function mongoAdminReject(ctx: any): void {
  const t = new TestResult("NOSQL-MONGO-ADMIN-REJECT", "NOSQL",
    "feature-inventory §21.1");
  try {
    // Attempt drop_collection — must be rejected with Forbidden
    const ds = ctx.datasource("canary-mongo");
    const dv = ds.fromQuery("drop_collection:canary_docs").build();
    ctx.dataview(dv);
    t.assert("admin_rejected", false,
      "drop_collection executed — admin guard broken");
  } catch (e) {
    const errStr = String(e);
    t.assert("admin_rejected", true, `threw: ${errStr}`);
    t.assert("error_is_forbidden",
      errStr.toLowerCase().includes("forbidden"),
      `error: ${errStr}`);
  }
  ctx.resdata = t.finish();
}

export function redisAdminReject(ctx: any): void {
  const t = new TestResult("NOSQL-REDIS-ADMIN-REJECT", "NOSQL",
    "feature-inventory §21.1");
  try {
    // Attempt FLUSHDB — must be rejected with Forbidden
    const ds = ctx.datasource("canary-redis");
    const dv = ds.fromQuery("FLUSHDB").build();
    ctx.dataview(dv);
    t.assert("admin_rejected", false,
      "FLUSHDB executed — admin guard broken");
  } catch (e) {
    const errStr = String(e);
    t.assert("admin_rejected", true, `threw: ${errStr}`);
    t.assert("error_is_forbidden",
      errStr.toLowerCase().includes("forbidden"),
      `error: ${errStr}`);
  }
  ctx.resdata = t.finish();
}
```

---

## AMD-1.4 — Missing RUNTIME Handler Files

### canary-handlers/libraries/handlers/eventbus-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function eventbusPublish(ctx: any): void {
  const t = new TestResult("RT-EVENTBUS-PUBLISH", "RUNTIME",
    "eventbus §12.1");
  try {
    // Publish an event to the "canary.ping" topic.
    // canary-streams has an SSE view triggered by this topic.
    // The Rust integration test connects to the SSE view first,
    // then calls this endpoint, then verifies the SSE client received the event.
    const ds = ctx.datasource("eventbus");
    const dv = ds.fromQuery("canary.ping")
      .build();
    ctx.dataview(dv, { message: "ping-from-canary-handlers" });
    t.assert("publish_ok", true, "event published to canary.ping");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

### canary-handlers/libraries/handlers/storage-tests.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function storeGetSet(ctx: any): void {
  const t = new TestResult("RT-CTX-STORE-GET-SET", "RUNTIME",
    "storage-engine §11.5");
  try {
    // SET
    ctx.store.set("canary-roundtrip", "hello-canary", 60);
    // GET
    const val = ctx.store.get("canary-roundtrip");
    t.assertEquals("value_roundtrip", "hello-canary", val);
    // DEL
    ctx.store.del("canary-roundtrip");
    // Verify deleted
    const deleted = ctx.store.get("canary-roundtrip");
    t.assert("deleted", deleted === null || deleted === undefined,
      `after delete: ${deleted}`);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function storeNamespace(ctx: any): void {
  const t = new TestResult("RT-CTX-STORE-NAMESPACE", "RUNTIME",
    "storage-engine §11.3");

  // Reserved prefix — must fail
  const reservedPrefixes = ["session:", "csrf:", "cache:", "raft:", "rivers:"];
  for (const prefix of reservedPrefixes) {
    try {
      ctx.store.get(`${prefix}hijack`);
      t.assert(`rejected_${prefix.replace(":", "")}`, false,
        `${prefix} was accessible`);
    } catch (e) {
      t.assert(`rejected_${prefix.replace(":", "")}`, true,
        `threw: ${String(e)}`);
    }
  }

  // Custom key — must succeed
  try {
    ctx.store.set("canary-ns-test", "ok", 60);
    const val = ctx.store.get("canary-ns-test");
    t.assertEquals("custom_key_works", "ok", val);
    ctx.store.del("canary-ns-test");
  } catch (e) {
    t.fail(`custom namespace failed: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}
```

### canary-handlers/libraries/handlers/rivers-api.ts

```typescript
import { TestResult } from "./test-harness.ts";

export function riversLog(ctx: any): void {
  const t = new TestResult("RT-RIVERS-LOG", "RUNTIME", "processpool §9.10");
  try {
    Rivers.log.info("canary log test — info");
    t.assert("log_info_ok", true);
  } catch (e) {
    t.assert("log_info_ok", false, `threw: ${String(e)}`);
  }
  try {
    Rivers.log.warn("canary log test — warn");
    t.assert("log_warn_ok", true);
  } catch (e) {
    t.assert("log_warn_ok", false, `threw: ${String(e)}`);
  }
  try {
    Rivers.log.error("canary log test — error");
    t.assert("log_error_ok", true);
  } catch (e) {
    t.assert("log_error_ok", false, `threw: ${String(e)}`);
  }
  ctx.resdata = t.finish();
}

export function riversCryptoHash(ctx: any): void {
  const t = new TestResult("RT-RIVERS-CRYPTO-HASH", "RUNTIME",
    "processpool §9.10");
  try {
    const hash = Rivers.crypto.hashPassword("canary-password");
    t.assertExists("hash_returned", hash);
    t.assertType("hash_is_string", hash, "string");
    const valid = Rivers.crypto.verifyPassword("canary-password", hash);
    t.assert("verify_correct_password", valid === true);
    const invalid = Rivers.crypto.verifyPassword("wrong-password", hash);
    t.assert("verify_wrong_password", invalid === false);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function riversCryptoRandom(ctx: any): void {
  const t = new TestResult("RT-RIVERS-CRYPTO-RANDOM", "RUNTIME",
    "processpool §9.10");
  try {
    const hex = Rivers.crypto.randomHex(32);
    t.assertExists("hex_returned", hex);
    t.assertEquals("hex_length", 64, hex.length);  // 32 bytes = 64 hex chars
    const b64 = Rivers.crypto.randomBase64url(32);
    t.assertExists("base64url_returned", b64);
    t.assert("base64url_length", b64.length >= 43,
      `length=${b64.length}`);
    // Two calls should produce different values
    const hex2 = Rivers.crypto.randomHex(32);
    t.assert("hex_not_repeated", hex !== hex2,
      "two randomHex calls returned same value");
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function riversCryptoHmac(ctx: any): void {
  const t = new TestResult("RT-RIVERS-CRYPTO-HMAC", "RUNTIME",
    "processpool §9.10");
  try {
    const hmac1 = Rivers.crypto.hmac("sha256", "secret-key", "message");
    const hmac2 = Rivers.crypto.hmac("sha256", "secret-key", "message");
    t.assertExists("hmac_returned", hmac1);
    t.assertEquals("hmac_deterministic", hmac1, hmac2);
    const hmac3 = Rivers.crypto.hmac("sha256", "different-key", "message");
    t.assert("hmac_key_sensitive", hmac1 !== hmac3);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function riversCryptoTiming(ctx: any): void {
  const t = new TestResult("RT-RIVERS-CRYPTO-TIMING", "RUNTIME",
    "processpool §9.10");
  try {
    const eq = Rivers.crypto.timingSafeEqual("abc123", "abc123");
    t.assert("equal_strings_true", eq === true);
    const neq = Rivers.crypto.timingSafeEqual("abc123", "xyz789");
    t.assert("unequal_strings_false", neq === false);
    const diffLen = Rivers.crypto.timingSafeEqual("short", "muchlonger");
    t.assert("different_length_false", diffLen === false);
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}

export function fakerDeterminism(ctx: any): void {
  const t = new TestResult("RT-FAKER-DETERMINISM", "RUNTIME",
    "data-layer §4.1");
  try {
    const r1 = ctx.dataview("faker_seeded", { seed: 42, limit: 3 });
    const r2 = ctx.dataview("faker_seeded", { seed: 42, limit: 3 });
    t.assert("both_returned", r1?.rows?.length > 0 && r2?.rows?.length > 0);
    t.assertEquals("deterministic_output",
      JSON.stringify(r1.rows), JSON.stringify(r2.rows));
  } catch (e) {
    t.fail(String(e));
  }
  ctx.resdata = t.finish();
}
```

---

## AMD-1.5 — Missing Streams Handler Files

### canary-streams/libraries/handlers/kafka-consumer.ts

```typescript
import { TestResult } from "./test-harness.ts";

// MessageConsumer handler — triggered by Kafka messages on "canary.kafka.test" topic.
// This handler writes the received message to StorageEngine so the Rust integration
// test can verify receipt by reading ctx.store after publishing to Kafka.

export function onMessage(ctx: any): void {
  const message = ctx.request?.body || {};
  // Store the message in application KV for verification
  ctx.store.set("canary:kafka:last-message",
    JSON.stringify({
      received_at: Date.now(),
      payload: message
    }), 120);
  Rivers.log.info("canary kafka consumer received message");
}
```

### canary-streams/libraries/handlers/ws-handler.ts

```typescript
// WebSocket echo handler — receives a message, sends it back

export function onConnection(ctx: any): void {
  Rivers.log.info(`canary ws echo: connection ${ctx.ws.connection_id}`);
}

export function onMessage(ctx: any): void {
  // Echo the received message back to sender
  Rivers.stream.push({
    test_id: "STREAM-WS-ECHO",
    echo: ctx.ws.message,
    connection_id: ctx.ws.connection_id
  });
}

export function onBroadcastConnection(ctx: any): void {
  Rivers.log.info(`canary ws broadcast: connection ${ctx.ws.connection_id}`);
}

export function onBroadcastMessage(ctx: any): void {
  // Broadcast to all connected clients (not just sender)
  Rivers.stream.push({
    test_id: "STREAM-WS-BROADCAST",
    broadcast: ctx.ws.message,
    from: ctx.ws.connection_id
  });
}
```

### canary-streams/libraries/handlers/sse-handler.ts

```typescript
// SSE tick handler — pushes a counter on each tick

let tickCount = 0;

export function onTick(ctx: any): void {
  tickCount++;
  Rivers.stream.push({
    test_id: "STREAM-SSE-TICK",
    tick: tickCount,
    timestamp: Date.now()
  });
}

// SSE event-triggered handler — fires when "canary.ping" event arrives from EventBus

export function onEventTriggered(ctx: any): void {
  Rivers.stream.push({
    test_id: "STREAM-SSE-EVENT",
    event: ctx.request?.body || {},
    triggered_at: Date.now()
  });
}
```

### canary-streams/libraries/handlers/poll-handler.ts

```typescript
// Polling on_change handler — invoked when hash diff detects data change

export function onPollChange(ctx: any): void {
  Rivers.stream.push({
    test_id: "STREAM-POLL-HASH",
    changed: true,
    data: ctx.data,
    detected_at: Date.now()
  });
}
```

---

## AMD-1.6 — V8 Timeout Handler Clarification

The RT-V8-TIMEOUT handler MUST contain an actual infinite loop. The handler is expected to never return — the V8 watchdog thread terminates it externally. The Rust integration test expects a non-200 response (timeout error), not a 200 with a verdict.

**This is a test where the handler deliberately fails.** The absence of a verdict IS the test. The Rust harness asserts:

```rust
// Handler runs while(true){} — watchdog kills it
// Response should be a timeout error, not a handler verdict
let response = client.get_with_timeout(
    "/canary/proxy/rt/v8/timeout",
    Duration::from_secs(15)
).await;
assert_ne!(response.status(), 200,
    "V8 timeout: handler returned 200 — watchdog did not fire");
```

The same applies to RT-V8-HEAP — the handler allocates until killed. The Rust harness expects a non-200 error response.

Do NOT add assertions or early returns to these handlers. Their purpose is to trigger the guardrail, not to report about it.

---

## AMD-1.7 — Resources.toml LockBox Block Syntax Fix

The original spec's `resources.toml` had inline `[datasources.lockbox]` blocks that are syntactically ambiguous when multiple `[[datasources]]` use them. The correct TOML for array-of-tables with nested fields:

### canary-nosql/resources.toml (CORRECTED)

```toml
[[datasources]]
name     = "canary-mongo"
driver   = "mongodb"
x-type   = "mongodb"
required = true
lockbox  = { alias = "canary-mongo" }

[[datasources]]
name       = "canary-es"
driver     = "elasticsearch"
x-type     = "elasticsearch"
nopassword = true
required   = true

[[datasources]]
name     = "canary-couch"
driver   = "couchdb"
x-type   = "couchdb"
required = true
lockbox  = { alias = "canary-couch" }

[[datasources]]
name       = "canary-cassandra"
driver     = "cassandra"
x-type     = "cassandra"
nopassword = true
required   = true

[[datasources]]
name     = "canary-ldap"
driver   = "ldap"
x-type   = "ldap"
required = true
lockbox  = { alias = "canary-ldap" }

[[datasources]]
name     = "canary-redis"
driver   = "redis"
x-type   = "redis"
required = true
lockbox  = { alias = "canary-redis" }

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

### canary-sql/resources.toml (CORRECTED)

```toml
[[datasources]]
name     = "canary-pg"
driver   = "postgresql"
x-type   = "postgresql"
required = true
lockbox  = { alias = "canary-pg" }

[[datasources]]
name     = "canary-mysql"
driver   = "mysql"
x-type   = "mysql"
required = true
lockbox  = { alias = "canary-mysql" }

[[datasources]]
name       = "canary-sqlite"
driver     = "sqlite"
x-type     = "sqlite"
nopassword = true
required   = true

[[services]]
name     = "canary-guard"
appId    = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeee00"
required = true
```

Apply the same inline-table `lockbox = { alias = "..." }` pattern to all `resources.toml` files across the bundle.

---

## AMD-1.8 — Bonus Tests In Implementation

The gap analysis found tests in the implementation that aren't in the spec. These are legitimate additions. Add to the Test Inventory tables:

### AUTH profile — add:

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| AUTH-GUARD-CLAIMS | `/canary/auth/claims` | POST | Guard returns IdentityClaims with correct fields | auth-session §3.3 |

### PROXY profile — add:

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| PROXY-HEALTH | `/canary/proxy/health` | GET | Health endpoint returns 200 through proxy | httpd §14.1 |
| PROXY-RESPONSE-ENVELOPE | `/canary/proxy/envelope` | GET | Response envelope format matches SHAPE-2 | shaping §SHAPE-2 |

### NOSQL profile — add (connectivity baseline):

| Test ID | Endpoint | Method | What It Tests | Spec Ref |
|---------|----------|--------|---------------|----------|
| NOSQL-MONGO-PING | `/canary/nosql/mongo/ping` | GET | MongoDB connectivity | driver-spec §6.6 |
| NOSQL-ES-PING | `/canary/nosql/es/ping` | GET | Elasticsearch connectivity | driver-spec §6.6 |
| NOSQL-COUCH-PING | `/canary/nosql/couch/ping` | GET | CouchDB connectivity | driver-spec §6.6 |
| NOSQL-CASSANDRA-PING | `/canary/nosql/cassandra/ping` | GET | Cassandra connectivity | driver-spec §6.6 |
| NOSQL-LDAP-PING | `/canary/nosql/ldap/ping` | GET | LDAP connectivity | driver-spec §6.6 |

Ping tests call `health_check` on the driver connection and report pass/fail. They're useful as a smoke test before running CRUD tests.

---

## AMD-1.9 — Updated Test Count

| Profile | Positive | Negative | Total |
|---------|----------|----------|-------|
| AUTH | 8 | 2 | 10 |
| SQL | 14 | 3 | 17 |
| NOSQL | 14 | 2 | 16 |
| RUNTIME | 16 | 9 | 25 |
| STREAM | 7 | 2 | 9 |
| PROXY | 6 | 0 | 6 |
| **Total** | **65** | **18** | **83** |

---

## Absorption Checklist

After absorbing this amendment into `rivers-canary-fleet-spec.md`:

- [ ] Naming Convention Rules section added after Design Principles
- [ ] canary-sql/app.toml has DDL DataViews before CRUD DataViews
- [ ] canary-sql init.ts uses DataView names, not raw SQL
- [ ] canary-nosql/app.toml is complete (all 6 drivers, all DataViews, all Views)
- [ ] All 6 schema files for NoSQL drivers present
- [ ] nosql-tests.ts has all 10 positive handlers
- [ ] negative-nosql.ts has mongoAdminReject + redisAdminReject
- [ ] eventbus-tests.ts exists
- [ ] kafka-consumer.ts exists
- [ ] ws-handler.ts, sse-handler.ts, poll-handler.ts exist
- [ ] All resources.toml files use inline-table lockbox syntax
- [ ] All test IDs match spec inventory verbatim
- [ ] All paths begin with `/`
- [ ] All .ts handlers use `language = "typescript"`
- [ ] V8 timeout/heap handlers contain actual dangerous code
- [ ] Bonus tests added to inventory tables
- [ ] Test count updated to 83
