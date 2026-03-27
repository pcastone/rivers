# Tutorial: NATS Datasource

**Rivers v0.50.1**

## Overview

The NATS driver (`rivers-plugin-nats`) implements `MessageBrokerDriver`. It provides JetStream-based pub/sub with pull consumers, subject filtering via wildcards, and request/reply support. Unlike Kafka and RabbitMQ, the NATS plugin is a `MessageBrokerDriver` only -- it does not implement the `DatabaseDriver` interface.

Use NATS when you need lightweight, low-latency pub/sub messaging with subject-based routing. NATS is appropriate for microservice communication, real-time event distribution, and request/reply patterns where simplicity and speed are prioritized over complex routing topologies.

## Prerequisites

- A running NATS server with JetStream enabled
- LockBox initialized with NATS credentials
- The `rivers-plugin-nats` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name nats/prod \
    --type string
# Value: myuser:mypassword
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the NATS datasource.

```toml
# resources.toml
[[datasources]]
name     = "event_bus_nats"
driver   = "nats"
x-type   = "broker"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the NATS connection and consumer subscriptions. The driver uses JetStream pull consumers with configurable `filter_subject` wildcard support.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.event_bus_nats]
driver             = "nats"
host               = "nats.internal"
port               = 4222
credentials_source = "lockbox://nats/prod"

[data.datasources.event_bus_nats.extra]
stream = "EVENTS"

[data.datasources.event_bus_nats.connection_pool]
max_size              = 10
connection_timeout_ms = 2000

[data.datasources.event_bus_nats.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 10000

# Consumer configuration
[data.datasources.event_bus_nats.consumer]
group_prefix = "rivers"
app_id       = "notification-service"
reconnect_ms = 3000

[[data.datasources.event_bus_nats.consumer.subscriptions]]
topic      = "events.orders.>"
event_name = "order.event"
ack_mode   = "auto"
max_retries = 3

[data.datasources.event_bus_nats.consumer.subscriptions.on_failure]
mode = "drop"

[[data.datasources.event_bus_nats.consumer.subscriptions]]
topic      = "events.users.created"
event_name = "user.created"
ack_mode   = "manual"
max_retries = 5

[data.datasources.event_bus_nats.consumer.subscriptions.on_failure]
mode        = "redirect"
destination = "events.users.dlq"
```

In NATS, the `topic` field maps to a JetStream subject filter. Wildcard patterns are supported:
- `events.orders.>` matches `events.orders.created`, `events.orders.updated`, and any deeper subjects
- `events.users.*` matches `events.users.created` but not `events.users.profile.updated`

The `BrokerMetadata::Nats` variant carries `sequence`, `stream`, and `consumer` for each message.

## Step 3: Define a Schema

Create a schema for the notification event payload.

```json
// schemas/notification_event.schema.json
{
  "type": "object",
  "description": "Notification event message payload",
  "fields": [
    { "name": "event_id",   "type": "uuid",     "required": true  },
    { "name": "event_type", "type": "string",   "required": true  },
    { "name": "subject",    "type": "string",   "required": true  },
    { "name": "recipient",  "type": "string",   "required": true  },
    { "name": "body",       "type": "json",     "required": true  },
    { "name": "created_at", "type": "datetime", "required": true  }
  ]
}
```

## Step 4: Create a DataView

Since NATS implements only `MessageBrokerDriver` (not `DatabaseDriver`), producing messages is done through a `CodeComponent` handler that uses the NATS datasource as a declared resource. There is no DataView-based produce operation for NATS.

For consuming, the `BrokerConsumerBridge` handles continuous delivery into `MessageConsumer` views automatically based on the subscription config from Step 2.

If your app also needs to publish messages to NATS subjects, use a CodeComponent:

```typescript
// handlers/publish.ts
export async function publishEvent(ctx: Rivers.ViewContext) {
    const { subject, payload } = ctx.request.body;
    // Publish via the NATS resource handle
    await Rivers.resource("event_bus_nats").publish({
        destination: subject,
        payload: JSON.stringify(payload),
    });
    return { status: "published" };
}
```

Define a CodeComponent-backed DataView if you want a REST interface for publishing:

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews -- not applicable for NATS (MessageBrokerDriver only)
# Use CodeComponent handlers to publish messages.
# ─────────────────────────────────────────────
```

## Step 5: Create a View

### REST view for publishing (via CodeComponent)

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# REST endpoint to publish an event via CodeComponent
[api.views.publish_event]
path      = "events/publish"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.publish_event.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/publish.ts"
entrypoint = "publishEvent"
resources  = ["event_bus_nats"]
```

### MessageConsumer view for processing events

```toml
# MessageConsumer -- processes order events from NATS
[api.views.process_order_event]
view_type = "MessageConsumer"

[api.views.process_order_event.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "onOrderEvent"
resources  = ["event_bus_nats"]

[api.views.process_order_event.on_event]
topic        = "order.event"
handler      = "handlers/orders.ts"
handler_mode = "stream"

# MessageConsumer -- processes user creation events
[api.views.process_user_created]
view_type = "MessageConsumer"

[api.views.process_user_created.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/users.ts"
entrypoint = "onUserCreated"
resources  = ["event_bus_nats"]

[api.views.process_user_created.on_event]
topic        = "user.created"
handler      = "handlers/users.ts"
handler_mode = "stream"
```

The handler receives the NATS message payload as the request body:

```typescript
// handlers/orders.ts
export async function onOrderEvent(ctx: Rivers.ViewContext) {
    const event = ctx.request.body;
    // event.event_id, event.event_type, event.subject, etc.
    // Process the order event...
}
```

## Testing

Publish an event via the REST endpoint:

```bash
curl -k -X POST https://localhost:8080/<bundle>/<app>/events/publish \
  -H "Content-Type: application/json" \
  -d '{
    "subject": "events.orders.created",
    "payload": {
      "event_id": "550e8400-e29b-41d4-a716-446655440000",
      "event_type": "order.created",
      "subject": "events.orders.created",
      "recipient": "fulfillment-service",
      "body": {"order_id": "abc-123"},
      "created_at": "2026-03-24T10:00:00Z"
    }
  }'
```

Check consumer processing via logs:

```bash
riversctl logs --app notification-service --level info
```

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"nats"` |
| `host` | string | yes | NATS server host |
| `port` | integer | yes | NATS server port (default: 4222) |
| `credentials_source` | string | yes | LockBox URI for credentials (`username:password`) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `stream` | string | -- | JetStream stream name for durable subscriptions |

### Consumer config (`[data.datasources.*.consumer]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `group_prefix` | string | `"rivers"` | Prefix for derived consumer name |
| `app_id` | string | -- | Application identifier in consumer derivation |
| `reconnect_ms` | integer | `5000` | Delay before reconnect attempt on failure |

### Subscription config (`[[data.datasources.*.consumer.subscriptions]]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `topic` | string | -- | NATS subject filter (supports `*` and `>` wildcards) |
| `event_name` | string | -- | EventBus event name for consumed messages |
| `ack_mode` | string | `"auto"` | `"auto"` or `"manual"` acknowledgment (deferred manual ack) |
| `max_retries` | integer | `3` | Retry attempts before failure policy executes |

### Failure policy (`[...subscriptions.on_failure]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | -- | `"dead_letter"`, `"redirect"`, `"requeue"`, or `"drop"` |
| `destination` | string | -- | Target subject for redirect or dead-letter |

### Subject wildcard patterns

| Pattern | Matches | Example |
|---------|---------|---------|
| `events.orders.*` | Single token wildcard | `events.orders.created`, `events.orders.updated` |
| `events.orders.>` | Multi-token wildcard | `events.orders.created`, `events.orders.items.added` |
| `events.orders.created` | Exact match | `events.orders.created` only |
