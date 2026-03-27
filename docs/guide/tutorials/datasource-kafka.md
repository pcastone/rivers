# Tutorial: Kafka Datasource

**Rivers v0.50.1**

## Overview

The Kafka driver (`rivers-plugin-kafka`) implements both `DatabaseDriver` and `MessageBrokerDriver`. As a `DatabaseDriver`, it supports discrete operations -- produce a single message, fetch by offset, list topics. As a `MessageBrokerDriver`, it powers continuous consumer group processing through the `BrokerConsumerBridge` and `MessageConsumer` views.

Use Kafka when you need durable, ordered, high-throughput message streaming between services. Kafka is appropriate for event sourcing, change data capture, and inter-service communication where message replay and consumer group semantics matter.

## Prerequisites

- A running Kafka broker (or cluster) accessible from the Rivers host
- LockBox initialized with Kafka SASL credentials
- The `rivers-plugin-kafka` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name kafka/prod \
    --type string
# Value: myuser:mypassword (SASL username:password)
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the Kafka datasource as a required dependency. The `x-type` field declares the driver contract for build-time validation.

```toml
# resources.toml
[[datasources]]
name     = "events_kafka"
driver   = "kafka"
x-type   = "broker"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the Kafka connection, consumer group, and subscriptions. The `credentials_source` field references the LockBox entry created above.

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.events_kafka]
driver             = "kafka"
host               = "kafka.internal"
port               = 9092
credentials_source = "lockbox://kafka/prod"

[data.datasources.events_kafka.extra]
bootstrap_servers = "kafka-1.internal:9092,kafka-2.internal:9092,kafka-3.internal:9092"

[data.datasources.events_kafka.connection_pool]
max_size              = 10
connection_timeout_ms = 3000

[data.datasources.events_kafka.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 15000

# Consumer configuration
[data.datasources.events_kafka.consumer]
group_prefix = "rivers"
app_id       = "order-service"
reconnect_ms = 5000

[[data.datasources.events_kafka.consumer.subscriptions]]
topic      = "orders"
event_name = "order.created"
ack_mode   = "auto"
max_retries = 3

[data.datasources.events_kafka.consumer.subscriptions.on_failure]
mode        = "dead_letter"
destination = "orders_dlq"

[[data.datasources.events_kafka.consumer.subscriptions]]
topic      = "payments"
event_name = "payment.completed"
ack_mode   = "auto"
max_retries = 5

[data.datasources.events_kafka.consumer.subscriptions.on_failure]
mode        = "redirect"
destination = "payments_retry"
```

The consumer group ID is derived automatically: `{group_prefix}.{app_id}.{datasource_id}.{component}`. In this case, the consumer group for the `orders` topic would be `rivers.order-service.events_kafka.orders`.

## Step 3: Define a Schema

Create a schema file for the message payload. This schema validates the structure of messages produced and consumed through DataViews.

```json
// schemas/order_event.schema.json
{
  "type": "object",
  "description": "Order event message payload",
  "fields": [
    { "name": "order_id",    "type": "uuid",     "required": true  },
    { "name": "customer_id", "type": "uuid",     "required": true  },
    { "name": "total",       "type": "float",    "required": true  },
    { "name": "status",      "type": "string",   "required": true  },
    { "name": "items",       "type": "json",     "required": true  },
    { "name": "created_at",  "type": "datetime", "required": true  }
  ]
}
```

## Step 4: Create a DataView

Define DataViews for producing messages and listing topics. The Kafka `DatabaseDriver` interface supports discrete operations like `produce`, `fetch`, and `list_topics`.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

# Produce a message to a topic
[data.dataviews.publish_order_event]
datasource    = "events_kafka"
query         = "produce"
return_schema = "schemas/order_event.schema.json"

[[data.dataviews.publish_order_event.parameters]]
name     = "topic"
type     = "string"
required = true

[[data.dataviews.publish_order_event.parameters]]
name     = "key"
type     = "string"
required = false

[[data.dataviews.publish_order_event.parameters]]
name     = "payload"
type     = "string"
required = true

# Fetch messages by offset (direct read, not consumer group)
[data.dataviews.fetch_order_events]
datasource = "events_kafka"
query      = "fetch"

[[data.dataviews.fetch_order_events.parameters]]
name     = "topic"
type     = "string"
required = true

[[data.dataviews.fetch_order_events.parameters]]
name     = "partition"
type     = "integer"
required = false

[[data.dataviews.fetch_order_events.parameters]]
name     = "offset"
type     = "integer"
required = false

# List available topics
[data.dataviews.list_topics]
datasource = "events_kafka"
query      = "list_topics"

[data.dataviews.list_topics.caching]
ttl_seconds = 30
```

## Step 5: Create a View

### REST view for producing messages

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# REST endpoint to produce a message
[api.views.publish_order]
path      = "orders/events"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.publish_order.handler]
type     = "dataview"
dataview = "publish_order_event"

# REST endpoint to list topics
[api.views.list_topics]
path      = "topics"
method    = "GET"
view_type = "Rest"
auth      = "none"

[api.views.list_topics.handler]
type     = "dataview"
dataview = "list_topics"
```

### MessageConsumer view for processing messages

`MessageConsumer` views are event-driven -- they have no HTTP route. The `BrokerConsumerBridge` pulls messages from Kafka, publishes them to the EventBus, and the `MessageConsumer` handler processes them.

```toml
# MessageConsumer — processes order.created events from Kafka
[api.views.process_order]
view_type = "MessageConsumer"

[api.views.process_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/orders.ts"
entrypoint = "onOrderCreated"
resources  = ["events_kafka"]

[api.views.process_order.on_event]
topic        = "order.created"
handler      = "handlers/orders.ts"
handler_mode = "stream"
```

The handler receives the Kafka message payload as the request body:

```typescript
// handlers/orders.ts
export async function onOrderCreated(ctx: Rivers.ViewContext) {
    const event = ctx.request.body;
    // event.order_id, event.customer_id, event.total, etc.
    // Process the order event...
}
```

## Testing

Produce a message:

```bash
curl -k -X POST https://localhost:8080/<bundle>/<app>/orders/events \
  -H "Content-Type: application/json" \
  -d '{
    "topic": "orders",
    "key": "order-123",
    "payload": "{\"order_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"customer_id\":\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\",\"total\":99.95,\"status\":\"created\",\"items\":[],\"created_at\":\"2026-03-24T10:00:00Z\"}"
  }'
```

List topics:

```bash
curl -k https://localhost:8080/<bundle>/<app>/topics
```

Verify the MessageConsumer is processing by checking logs:

```bash
riversctl logs --app order-service --level info
```

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"kafka"` |
| `host` | string | yes | Kafka broker host |
| `port` | integer | yes | Kafka broker port (default: 9092) |
| `credentials_source` | string | yes | LockBox URI for SASL credentials (`username:password`) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bootstrap_servers` | string | -- | Comma-separated list of `host:port` broker addresses |

### Consumer config (`[data.datasources.*.consumer]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `group_prefix` | string | `"rivers"` | Prefix for derived consumer group ID |
| `app_id` | string | -- | Application identifier in consumer group derivation |
| `reconnect_ms` | integer | `5000` | Delay before reconnect attempt on failure |

### Subscription config (`[[data.datasources.*.consumer.subscriptions]]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `topic` | string | -- | Kafka topic name to subscribe to |
| `event_name` | string | -- | EventBus event name for consumed messages |
| `ack_mode` | string | `"auto"` | `"auto"` or `"manual"` acknowledgment |
| `max_retries` | integer | `3` | Retry attempts before failure policy executes |

### Failure policy (`[...subscriptions.on_failure]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | -- | `"dead_letter"`, `"redirect"`, `"requeue"`, or `"drop"` |
| `destination` | string | -- | Target topic/datasource for dead-letter or redirect |

### DatabaseDriver operations

| Operation | Description |
|-----------|-------------|
| `list_topics` | Returns available Kafka topics |
| `produce` | Publishes a single message to a topic |
| `fetch` | Reads messages by topic/partition/offset |
| `ping` | Broker health check |
