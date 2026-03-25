# Tutorial: GraphQL

**Rivers v0.50.1**

## Overview

Rivers can auto-generate a GraphQL API from your existing DataViews and Views. No schema-first SDL required — Rivers builds the GraphQL schema from your TOML config.

- **Query fields** — generated from DataViews with `return_schema`
- **Mutation fields** — generated from CodeComponent-backed POST/PUT/DELETE views
- **Subscription fields** — generated from SSE trigger events via EventBus

---

## Step 1: Enable GraphQL

File: `app.toml`

```toml
[graphql]
enabled        = true
path           = "/graphql"
introspection  = true
max_depth      = 10
max_complexity = 1000
```

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `false` | Enable the GraphQL endpoint |
| `path` | `"/graphql"` | URL path |
| `introspection` | `true` | Allow schema introspection queries |
| `max_depth` | 10 | Maximum query nesting depth |
| `max_complexity` | 1000 | Maximum query complexity score |

---

## Step 2: Define DataViews (Become Queries)

Every DataView with a `return_schema` becomes a GraphQL query field.

```toml
[data.dataviews.list_users]
name          = "list_users"
datasource    = "users_db"
query         = "schemas/user.schema.json"
return_schema = "schemas/user.schema.json"

[[data.dataviews.list_users.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

[data.dataviews.get_user]
name          = "get_user"
datasource    = "users_db"
query         = "schemas/user.schema.json"
return_schema = "schemas/user.schema.json"

[[data.dataviews.get_user.parameters]]
name     = "id"
type     = "uuid"
required = true
```

This generates:

```graphql
type Query {
  list_users(limit: Int = 20): [User!]!
  get_user(id: ID!): User
}

type User {
  id: ID!
  name: String!
  email: String!
  created_at: DateTime!
}
```

### Type Mapping

| Rivers Type | GraphQL Type |
|-------------|-------------|
| `uuid` | `ID` |
| `string` | `String` |
| `integer` | `Int` |
| `float` | `Float` |
| `boolean` | `Boolean` |
| `email` | `String` |
| `phone` | `String` |
| `datetime` | `DateTime` |
| `date` | `Date` |
| `url` | `String` |
| `json` | `JSON` |

---

## Step 3: Define Mutation Views (Become Mutations)

CodeComponent-backed views using POST, PUT, or DELETE methods become GraphQL mutations.

```toml
[api.views.create_user]
path      = "users"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.create_user.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/users.js"
entrypoint = "createUser"
resources  = ["users_db"]
```

This generates:

```graphql
type Mutation {
  create_user(input: CreateUserInput!): User
}
```

---

## Step 4: SSE Views (Become Subscriptions)

SSE views with `sse_trigger_events` become GraphQL subscriptions.

```toml
[api.views.user_events]
path               = "events/users"
method             = "GET"
view_type          = "ServerSentEvents"
sse_trigger_events = ["UserCreated", "UserUpdated"]
```

This generates:

```graphql
type Subscription {
  user_events: UserEvent
}
```

---

## Testing

### GraphQL Playground

When `introspection = true`, a playground is available at:

```
http://localhost:8080/graphql/playground
```

### curl Queries

```bash
# Query
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "{ list_users(limit: 5) { id name email } }"
  }'

# Query with variables
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "query GetUser($id: ID!) { get_user(id: $id) { id name email } }",
    "variables": { "id": "abc-123" }
  }'

# Mutation
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "mutation { create_user(input: { name: \"Alice\", email: \"alice@example.com\" }) { id name } }"
  }'

# Introspection
curl -X POST http://localhost:8080/graphql \
  -H "Content-Type: application/json" \
  -d '{ "query": "{ __schema { types { name } } }" }'
```

---

## Full Example

### Schema

File: `schemas/user.schema.json`

```json
{
  "type": "object",
  "description": "User account",
  "fields": [
    { "name": "id",         "type": "uuid",     "required": true  },
    { "name": "name",       "type": "string",   "required": true  },
    { "name": "email",      "type": "email",    "required": true  },
    { "name": "role",       "type": "string",   "required": true  },
    { "name": "created_at", "type": "datetime", "required": true  }
  ]
}
```

### app.toml

```toml
[graphql]
enabled       = true
introspection = true

[data.datasources.users_db]
name   = "users_db"
driver = "faker"
nopassword = true

[data.datasources.users_db.config]
seed = 42

# Query: list_users
[data.dataviews.list_users]
name          = "list_users"
datasource    = "users_db"
query         = "schemas/user.schema.json"
return_schema = "schemas/user.schema.json"

[[data.dataviews.list_users.parameters]]
name     = "limit"
type     = "integer"
required = false
default  = 20

# Query: get_user
[data.dataviews.get_user]
name          = "get_user"
datasource    = "users_db"
query         = "schemas/user.schema.json"
return_schema = "schemas/user.schema.json"

[[data.dataviews.get_user.parameters]]
name     = "id"
type     = "uuid"
required = true

# REST views (also exposed via GraphQL)
[api.views.list_users]
path      = "users"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.list_users.handler]
type     = "dataview"
dataview = "list_users"

[api.views.list_users.parameter_mapping.query]
limit = "limit"
```

### GraphQL Query

```graphql
{
  list_users(limit: 5) {
    id
    name
    email
    role
    created_at
  }
}
```

---

## Configuration Reference

| Field | Default | Description |
|-------|---------|-------------|
| `graphql.enabled` | `false` | Enable GraphQL |
| `graphql.path` | `"/graphql"` | Endpoint path |
| `graphql.introspection` | `true` | Allow introspection |
| `graphql.max_depth` | 10 | Max query depth |
| `graphql.max_complexity` | 1000 | Max complexity score |
