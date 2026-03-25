# Tutorial: RabbitMQ Datasource

**Rivers v0.50.1**

## Overview

The RabbitMQ driver (`rivers-plugin-rabbitmq`) implements both `DatabaseDriver` and `MessageBrokerDriver`. As a `DatabaseDriver`, it supports discrete operations -- publish a single message, declare exchanges and queues. As a `MessageBrokerDriver`, it provides push-based consumer delivery via `basic_consume` (not polling) through the `BrokerConsumerBridge`.

Use RabbitMQ when you need flexible message routing with exchanges, queues, and binding patterns. RabbitMQ is appropriate for task distribution, work queues, pub/sub fanout, and request/reply patterns where message routing topology matters more than raw throughput.

## Prerequisites

- A running RabbitMQ broker accessible from the Rivers host
- LockBox initialized with RabbitMQ credentials
- The `rivers-plugin-rabbitmq` plugin present in the configured plugin directory

### Store credentials in LockBox

```bash
rivers lockbox add \
    --name rabbitmq/prod \
    --type string
# Value: myuser:mypassword
```

## Step 1: Declare the Datasource

In `resources.toml`, declare the RabbitMQ datasource.

```toml
# resources.toml
[[datasources]]
name     = "task_queue"
driver   = "rabbitmq"
x-type   = "broker"
required = true
```

## Step 2: Configure the Datasource

In `app.toml`, configure the RabbitMQ connection, consumer group, and subscriptions. Publisher confirms are enabled by default. The driver uses `basic_consume` (push model) rather than `basic_get` (polling).

```toml
# app.toml

# ─────────────────────────────────────────────
# Datasource
# ─────────────────────────────────────────────

[data.datasources.task_queue]
driver             = "rabbitmq"
host               = "rabbitmq.internal"
port               = 5672
credentials_source = "lockbox://rabbitmq/prod"

[data.datasources.task_queue.extra]
vhost = "/myapp"

[data.datasources.task_queue.connection_pool]
max_size              = 10
connection_timeout_ms = 3000

[data.datasources.task_queue.connection_pool.circuit_breaker]
enabled           = true
failure_threshold = 5
window_ms         = 60000
open_timeout_ms   = 10000

# Consumer configuration
[data.datasources.task_queue.consumer]
group_prefix = "rivers"
app_id       = "task-processor"
reconnect_ms = 5000

[[data.datasources.task_queue.consumer.subscriptions]]
topic      = "tasks.pending"
event_name = "task.submitted"
ack_mode   = "auto"
max_retries = 3

[data.datasources.task_queue.consumer.subscriptions.on_failure]
mode        = "dead_letter"
destination = "tasks_dlq"

[[data.datasources.task_queue.consumer.subscriptions]]
topic      = "notifications"
event_name = "notification.requested"
ack_mode   = "manual"
max_retries = 5

[data.datasources.task_queue.consumer.subscriptions.on_failure]
mode = "requeue"
```

In RabbitMQ, the `topic` field in subscriptions maps to the queue name. The `BrokerMetadata::Rabbit` variant carries `delivery_tag`, `exchange`, and `routing_key` for each consumed message.

## Step 3: Define a Schema

Create a schema for the task message payload.

```json
// schemas/task_event.schema.json
{
  "type": "object",
  "description": "Task event message payload",
  "fields": [
    { "name": "task_id",     "type": "uuid",     "required": true  },
    { "name": "task_type",   "type": "string",   "required": true  },
    { "name": "priority",    "type": "integer",  "required": true  },
    { "name": "payload",     "type": "json",     "required": true  },
    { "name": "submitted_by","type": "uuid",     "required": true  },
    { "name": "created_at",  "type": "datetime", "required": true  }
  ]
}
```

## Step 4: Create a DataView

Define DataViews for publishing messages to RabbitMQ via the `DatabaseDriver` interface.

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# DataViews
# ─────────────────────────────────────────────

# Publish a message to an exchange
[data.dataviews.submit_task]
datasource    = "task_queue"
query         = "publish"
return_schema = "schemas/task_event.schema.json"

[[data.dataviews.submit_task.parameters]]
name     = "exchange"
type     = "string"
required = true

[[data.dataviews.submit_task.parameters]]
name     = "routing_key"
type     = "string"
required = true

[[data.dataviews.submit_task.parameters]]
name     = "payload"
type     = "string"
required = true

# Declare an exchange (admin operation)
[data.dataviews.declare_exchange]
datasource = "task_queue"
query      = "declare_exchange"

[[data.dataviews.declare_exchange.parameters]]
name     = "exchange"
type     = "string"
required = true

[[data.dataviews.declare_exchange.parameters]]
name     = "exchange_type"
type     = "string"
required = true
```

## Step 5: Create a View

### REST view for publishing messages

```toml
# app.toml (continued)

# ─────────────────────────────────────────────
# Views
# ─────────────────────────────────────────────

# REST endpoint to submit a task
[api.views.submit_task]
path      = "tasks"
method    = "POST"
view_type = "Rest"
auth      = "none"

[api.views.submit_task.handler]
type     = "dataview"
dataview = "submit_task"
```

### MessageConsumer view for processing queued tasks

```toml
# MessageConsumer — processes task.submitted events from RabbitMQ
[api.views.process_task]
view_type = "MessageConsumer"

[api.views.process_task.handler]
type       = "codecomponent"
language   = "typescript"
module     = "handlers/tasks.ts"
entrypoint = "onTaskSubmitted"
resources  = ["task_queue"]

[api.views.process_task.on_event]
topic        = "task.submitted"
handler      = "handlers/tasks.ts"
handler_mode = "stream"
```

The handler receives the RabbitMQ message payload as the request body. The `BrokerMetadata::Rabbit` metadata (delivery_tag, exchange, routing_key) is available in the message envelope:

```typescript
// handlers/tasks.ts
export async function onTaskSubmitted(ctx: Rivers.ViewContext) {
    const task = ctx.request.body;
    // task.task_id, task.task_type, task.priority, etc.
    // Process the task...
}
```

## Testing

Submit a task:

```bash
curl -k -X POST https://localhost:8080/<bundle>/<app>/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "exchange": "tasks",
    "routing_key": "tasks.pending",
    "payload": "{\"task_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"task_type\":\"email\",\"priority\":1,\"payload\":{\"to\":\"user@example.com\"},\"submitted_by\":\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\",\"created_at\":\"2026-03-24T10:00:00Z\"}"
  }'
```

Check consumer processing via logs:

```bash
riversctl logs --app task-processor --level info
```

## Configuration Reference

### Datasource fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `driver` | string | yes | Must be `"rabbitmq"` |
| `host` | string | yes | RabbitMQ broker host |
| `port` | integer | yes | RabbitMQ broker port (default: 5672) |
| `credentials_source` | string | yes | LockBox URI for credentials (`username:password`) |

### Extra config (`[data.datasources.*.extra]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `vhost` | string | `"/"` | RabbitMQ virtual host |

### Consumer config (`[data.datasources.*.consumer]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `group_prefix` | string | `"rivers"` | Prefix for derived consumer tag |
| `app_id` | string | -- | Application identifier in consumer derivation |
| `reconnect_ms` | integer | `5000` | Delay before reconnect attempt on failure |

### Subscription config (`[[data.datasources.*.consumer.subscriptions]]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `topic` | string | -- | RabbitMQ queue name to consume from |
| `event_name` | string | -- | EventBus event name for consumed messages |
| `ack_mode` | string | `"auto"` | `"auto"` or `"manual"` acknowledgment |
| `max_retries` | integer | `3` | Retry attempts before failure policy executes |

### Failure policy (`[...subscriptions.on_failure]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | -- | `"dead_letter"`, `"redirect"`, `"requeue"`, or `"drop"` |
| `destination` | string | -- | Target queue/exchange for dead-letter or redirect |

### DatabaseDriver operations

| Operation | Description |
|-----------|-------------|
| `publish` | Publishes a message to an exchange with a routing key |
| `declare_exchange` | Declares an exchange (type: direct, topic, fanout, headers) |
| `declare_queue` | Declares a queue |
| `bind_queue` | Binds a queue to an exchange with a routing key |
| `ping` | Broker health check |
