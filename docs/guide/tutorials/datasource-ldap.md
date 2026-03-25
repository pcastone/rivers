# Tutorial: LDAP Datasource

**Rivers v0.50.1**

## Overview

The LDAP driver is currently an honest stub in Rivers. It registers successfully, appears in `GET /admin/drivers`, but returns `DriverError::NotImplemented` on all operations. This tutorial documents the intended configuration and usage patterns for when the driver is fully implemented.

LDAP is a directory service protocol. Use the LDAP datasource when you need to query organizational directories for user records, group memberships, or authentication lookups. LDAP is appropriate for enterprise integrations where Active Directory or OpenLDAP is the authoritative source for identity and organizational data.

The `pattern` schema attribute is supported for LDAP, enabling regex validation on directory attribute values.

## Prerequisites

- A running LDAP directory server (OpenLDAP, Active Directory, etc.) accessible from the Rivers host
- LockBox initialized with LDAP bind credentials
- The `rivers-plugin-ldap` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name ldap/prod \
    --type string
# Value: cn=admin,dc=example,dc=com:mypassword (bind DN:password)
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the LDAP datasource.

```toml
# resources.toml
[[datasources]]
name     = "directory"
driver   = "ldap"
x-type   = "database"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the LDAP connection. The `base_dn` in the `extra` config sets the root of the directory tree for all queries.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.directory]
driver             = "ldap"
host               = "ldap.internal"
port               = 636
credentials_source = "lockbox://ldap/prod"

[data.datasources.directory.extra]
base_dn  = "dc=example,dc=com"
scheme   = "ldaps"
timeout  = "5000"

[data.datasources.directory.connection_pool]
max_size              = 10
connection_timeout_ms = 3000
idle_timeout_ms       = 60000

[data.datasources.directory.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 15000
```

## Step 3: Define a Schema

Create a schema for LDAP directory entries. The `pattern` attribute is supported for the LDAP driver, enabling regex validation on directory attribute values at both build time and deploy time.

```json
// schemas/user_entry.schema.json
{
  "type": "object",
  "description": "LDAP user directory entry",
  "fields": [
    { "name": "dn",             "type": "string",  "required": true  },
    { "name": "uid",            "type": "string",  "required": true, "pattern": "^[a-z][a-z0-9._-]{2,31}$" },
    { "name": "cn",             "type": "string",  "required": true  },
    { "name": "sn",             "type": "string",  "required": true  },
    { "name": "givenName",      "type": "string",  "required": true  },
    { "name": "mail",           "type": "email",   "required": true  },
    { "name": "telephoneNumber","type": "phone",   "required": false },
    { "name": "title",          "type": "string",  "required": false },
    { "name": "department",     "type": "string",  "required": false },
    { "name": "memberOf",       "type": "json",    "required": false }
  ]
}
```

The `pattern` attribute on the `uid` field enforces that user IDs match the regex `^[a-z][a-z0-9._-]{2,31}$` -- lowercase alphanumeric starting with a letter, 3-32 characters. This validation runs at schema validation time on query results.

```json
// schemas/group_entry.schema.json
{
  "type": "object",
  "description": "LDAP group directory entry",
  "fields": [
    { "name": "dn",          "type": "string", "required": true  },
    { "name": "cn",          "type": "string", "required": true  },
    { "name": "description", "type": "string", "required": false },
    { "name": "member",      "type": "json",   "required": false },
    { "name": "gidNumber",   "type": "integer","required": false }
  ]
}
```

## Step 4: Create a DataView

Define DataViews for LDAP search operations. The `query` field contains an LDAP search filter string. Parameters are substituted into the filter at execution time.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

# Search users by department
[data.dataviews.search_users]
datasource      = "directory"
query           = "search"
return_schema   = "schemas/user_entry.schema.json"
validate_result = true

[data.dataviews.search_users.caching]
ttl_seconds = 120

[[data.dataviews.search_users.parameters]]
name     = "filter"
type     = "string"
required = true

[[data.dataviews.search_users.parameters]]
name     = "base_dn"
type     = "string"
required = false

[[data.dataviews.search_users.parameters]]
name     = "scope"
type     = "string"
required = false

[[data.dataviews.search_users.parameters]]
name     = "limit"
type     = "integer"
required = false

# Get a single user by uid
[data.dataviews.get_user]
datasource      = "directory"
query           = "search"
return_schema   = "schemas/user_entry.schema.json"
validate_result = true

[data.dataviews.get_user.caching]
ttl_seconds = 300

[[data.dataviews.get_user.parameters]]
name     = "filter"
type     = "string"
required = true

# Search groups
[data.dataviews.search_groups]
datasource    = "directory"
query         = "search"
return_schema = "schemas/group_entry.schema.json"

[data.dataviews.search_groups.caching]
ttl_seconds = 300

[[data.dataviews.search_groups.parameters]]
name     = "filter"
type     = "string"
required = true

[[data.dataviews.search_groups.parameters]]
name     = "limit"
type     = "integer"
required = false

# Find group members
[data.dataviews.get_group_members]
datasource    = "directory"
query         = "search"
return_schema = "schemas/user_entry.schema.json"

[data.dataviews.get_group_members.caching]
ttl_seconds = 120

[[data.dataviews.get_group_members.parameters]]
name     = "filter"
type     = "string"
required = true
```

## Step 5: Create a View

Expose the LDAP search DataViews as REST endpoints. LDAP search filters are passed as query parameters and assembled into the filter expression.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# Search users
[api.views.search_users]
path            = "directory/users"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_users.handler]
type     = "dataview"
dataview = "search_users"

[api.views.search_users.parameter_mapping.query]
filter  = "filter"
base_dn = "base_dn"
scope   = "scope"
limit   = "limit"

# Get user by uid
[api.views.get_user]
path            = "directory/users/{uid}"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_user.handler]
type     = "dataview"
dataview = "get_user"

[api.views.get_user.parameter_mapping.path]
uid = "filter"

# Search groups
[api.views.search_groups]
path            = "directory/groups"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.search_groups.handler]
type     = "dataview"
dataview = "search_groups"

[api.views.search_groups.parameter_mapping.query]
filter = "filter"
limit  = "limit"

# Get group members
[api.views.get_group_members]
path            = "directory/groups/{cn}/members"
method          = "GET"
view_type       = "Rest"
response_format = "envelope"
auth            = "none"

[api.views.get_group_members.handler]
type     = "dataview"
dataview = "get_group_members"

[api.views.get_group_members.parameter_mapping.path]
cn = "filter"
```

## Testing

Search users by department:

```bash
curl -k "https://localhost:8080/<bundle>/<app>/directory/users?filter=(department=Engineering)&limit=50"
```

Get a single user by uid:

```bash
curl -k https://localhost:8080/<bundle>/<app>/directory/users/jdoe
```

Search groups:

```bash
curl -k "https://localhost:8080/<bundle>/<app>/directory/groups?filter=(cn=dev*)"
```

Get members of a group:

```bash
curl -k https://localhost:8080/<bundle>/<app>/directory/groups/engineering/members
```

Search with a compound filter:

```bash
curl -k "https://localhost:8080/<bundle>/<app>/directory/users?filter=(%26(department=Engineering)(title=Senior*))"
```

**Note:** The LDAP driver is currently a stub and will return `DriverError::NotImplemented` for all operations. These examples document the intended interface.

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"ldap"` |
| `host` | string | yes | LDAP server host |
| `port` | integer | yes | LDAP port (389 for LDAP, 636 for LDAPS) |
| `credentials_source` | string | yes | LockBox URI for bind credentials (`bindDN:password`) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `base_dn` | string | -- | Base distinguished name for directory searches |
| `scheme` | string | `"ldaps"` | Connection scheme (`"ldap"` or `"ldaps"`) |
| `timeout` | string | `"5000"` | Operation timeout in milliseconds |

### Connection pool (`[data.datasources.*.connection_pool]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_size` | integer | `10` | Maximum connections in the pool |
| `min_idle` | integer | `0` | Minimum idle connections maintained |
| `connection_timeout_ms` | integer | `500` | Timeout for acquiring a connection |
| `idle_timeout_ms` | integer | `30000` | Close idle connections after this duration |
| `max_lifetime_ms` | integer | `300000` | Maximum connection lifetime before recycling |

### Schema attributes supported by LDAP

| Attribute | Description |
|-----------|-------------|
| `pattern` | Regex pattern for string field validation (shared with postgresql, mysql) |

### LDAP search parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `filter` | string | LDAP search filter (RFC 4515 syntax) |
| `base_dn` | string | Override the default base DN for this query |
| `scope` | string | Search scope: `"base"`, `"one"`, `"sub"` (default: `"sub"`) |
| `limit` | integer | Maximum entries to return |

### LDAP filter syntax (RFC 4515)

| Filter | Meaning |
|--------|---------|
| `(uid=jdoe)` | Exact match |
| `(cn=John*)` | Prefix match |
| `(department=Engineering)` | Attribute equality |
| `(&(department=Engineering)(title=Senior*))` | AND compound filter |
| `(\|(department=Engineering)(department=Product))` | OR compound filter |
| `(!(department=HR))` | NOT filter |
| `(memberOf=cn=devs,ou=groups,dc=example,dc=com)` | Group membership check |
