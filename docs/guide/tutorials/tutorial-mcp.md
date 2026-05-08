# Tutorial: MCP (Model Context Protocol)

**Rivers v0.54.0**

## Overview

MCP (Model Context Protocol) exposes your DataViews as AI-consumable tools via JSON-RPC 2.0. By configuring an MCP view, you enable AI models (Claude, ChatGPT, etc.) to discover and call your data operations as first-class tools in their agentic workflows.

MCP in Rivers includes three capabilities:

- **Tools** — Whitelisted DataViews exposed as callable tools with descriptions, parameters, and AI hints
- **Resources** — Read-only DataViews available as context data (markdown, JSON, etc.)
- **Prompts** — Markdown templates with argument substitution for AI instruction workflows

This tutorial covers adding an MCP view to your app, testing it with curl, and configuring tools, resources, and prompts.

## Prerequisites

- A running Rivers instance (see the [Getting Started tutorial](tutorial-getting-started.md))
- A bundle with DataViews configured (or use the address-book bundle)
- `curl` for testing MCP JSON-RPC calls

---

## Step 1: Add MCP to Your App

Open your app's `app.toml` file and add an MCP view:

```toml
[api.views.mcp]
path      = "/mcp"
view_type = "Mcp"                # Case-sensitive: "Mcp", not "MCP" or "mcp"
method    = "POST"               # Required: MCP is JSON-RPC 2.0 over POST
auth      = "none"               # Or "session" / a guard reference

[api.views.mcp.handler]
type = "none"                    # Required sentinel — MCP dispatch is internal,
                                 # no user handler runs
```

This creates a JSON-RPC 2.0 endpoint at `POST /mcp` that accepts `initialize`, `tools/list`, `tools/call`, `resources/list`, `prompts/list`, and other MCP methods.

### Common Errors When Adding an MCP View

| Error | Cause | Fix |
|-------|-------|-----|
| `invalid view_type` | Used `"MCP"` or `"mcp"` | Use `"Mcp"` — capital M, lowercase cp |
| `missing method` | Omitted `method = "POST"` | MCP is JSON-RPC 2.0 over HTTP POST, so the field is required |
| `missing handler` | Omitted the `[api.views.mcp.handler]` section | Add `[api.views.mcp.handler]` with `type = "none"` — this sentinel tells Rivers the dispatcher is MCP-internal, not a user handler |
| `invalid guard type` | Used `guard = "guard_name"` (string reference) | MCP endpoints use the standard `auth` field — see the view layer spec for allowed values |

Reload the bundle:

```bash
/opt/rivers/bin/riversctl doctor --fix
/opt/rivers/bin/riversctl stop
/opt/rivers/bin/riversctl start
```

---

## Step 2: Expose DataViews as Tools

Add a `tools` section to your MCP view to expose DataViews as callable tools:

```toml
[api.views.mcp.tools.search_contacts]
dataview    = "search_contacts"
description = "Search contacts by name or email"
hints       = { read_only = true }

[api.views.mcp.tools.list_all_contacts]
dataview    = "list_contacts"
description = "List all contacts in the system"
hints       = { read_only = true }
```

Each tool maps a named DataView and provides:

| Field | Purpose |
|-------|---------|
| **dataview** | The DataView name to expose (must exist in `data.dataviews`) |
| **description** | Human-readable text explaining what the tool does (shown to AI) |
| **method** | Optional: HTTP method override (GET/POST/PUT/DELETE) |
| **hints** | Tool behavior hints for the AI model |

### Tool Hints

Tool hints inform the AI about the tool's behavior:

```toml
[api.views.mcp.tools.update_contact]
dataview    = "update_contact"
description = "Update a contact's information"
hints       = { 
  read_only     = false,      # True if the tool doesn't modify state
  destructive   = true,       # True if the tool can delete data
  idempotent    = true,       # True if safe to retry
  open_world    = true        # True if tool talks to external systems
}
```

---

## Step 3: Test with curl

### Initialize

First, establish an MCP session by calling `initialize`:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {}
  }'
```

Expected response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2024-11-05",
    "serverInfo": {
      "name": "rivers-mcp",
      "version": "0.54.0"
    },
    "capabilities": {
      "tools": { "listChanged": false }
    }
  }
}
```

### List Tools

Request the list of available tools:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }'
```

Expected response:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "tools": [
      {
        "name": "search_contacts",
        "description": "Search contacts by name or email",
        "inputSchema": {
          "type": "object",
          "properties": {
            "query": {
              "type": "string",
              "description": "Search term"
            }
          },
          "required": ["query"]
        }
      },
      {
        "name": "list_all_contacts",
        "description": "List all contacts in the system",
        "inputSchema": { "type": "object", "properties": {} }
      }
    ]
  }
}
```

### Call a Tool

Ask the MCP server to call a tool on your behalf:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 3,
    "method": "tools/call",
    "params": {
      "name": "search_contacts",
      "arguments": {
        "query": "john"
      }
    }
  }'
```

Expected response (results depend on your datasource):

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "[{\"id\":\"123\",\"name\":\"John Doe\",\"email\":\"john@example.com\"}]"
      }
    ]
  }
}
```

### Error Handling

If you call a method that doesn't exist, you receive a standard JSON-RPC error:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 4,
    "method": "nonexistent",
    "params": {}
  }'
```

Expected response:

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "error": {
    "code": -32601,
    "message": "Method not found"
  }
}
```

---

## Step 4: Add Resources

MCP resources expose read-only DataViews as context data for AI models:

```toml
[api.views.mcp.resources.contact_schema]
dataview    = "get_contact_schema"
description = "JSON schema for contact objects"
mime_type   = "application/json"
```

To list resources:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 5,
    "method": "resources/list",
    "params": {}
  }'
```

To read a resource:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 6,
    "method": "resources/read",
    "params": {
      "uri": "contact_schema"
    }
  }'
```

---

## Step 5: Add Prompts

MCP prompts are markdown templates with argument substitution. You can guide AI workflows with custom instructions:

```toml
[api.views.mcp.prompts.contact_workflow]
description = "Workflow for searching and updating contacts"
template    = "libraries/prompts/contact-workflow.md"

[[api.views.mcp.prompts.contact_workflow.arguments]]
name        = "task"
description = "The task to perform: search, list, or update"
required    = true

[[api.views.mcp.prompts.contact_workflow.arguments]]
name        = "query"
description = "Optional search query"
required    = false
default     = ""
```

Create the markdown template at `libraries/prompts/contact-workflow.md`:

```markdown
# Contact Workflow

## Task: {task}

You are helping with contact management. The available tools are:
- `search_contacts`: Search for contacts by name or email
- `list_all_contacts`: Get all contacts
- `update_contact`: Modify an existing contact

Your goal is to: {task}

{query:
  Search criteria: {query}
}
```

To list prompts:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 7,
    "method": "prompts/list",
    "params": {}
  }'
```

To get a prompt with arguments:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 8,
    "method": "prompts/get",
    "params": {
      "name": "contact_workflow",
      "arguments": {
        "task": "search for all customers in New York",
        "query": "city=New York"
      }
    }
  }'
```

---

## Step 6: Session Management

MCP sessions are stateful and identified by the `Mcp-Session-Id` header. Sessions persist for a configurable TTL (default: 1 hour).

### Create a Session

The first `initialize` call creates a session. The server returns a session ID in the response:

```bash
RESPONSE=$(curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {}
  }')

# Extract the session ID from response headers if provided
SESSION_ID=$(echo "$RESPONSE" | grep -o "Mcp-Session-Id: [^[:space:]]*" | cut -d' ' -f2) || SESSION_ID="default"
```

### Use an Existing Session

Include the session ID in subsequent requests:

```bash
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -H "Mcp-Session-Id: $SESSION_ID" \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
  }'
```

### Configure Session TTL

In your MCP view, optionally set a custom session TTL:

```toml
[api.views.mcp.session]
ttl_seconds = 7200  # 2 hours
```

---

## Step 7: Validation and Best Practices

### Validation During Bundle Load

When you deploy a bundle, `riverpackage validate` checks MCP configurations:

- All referenced DataViews must exist
- Tool descriptions are recommended but optional
- Resources must map to read-only DataViews (no writes)
- Prompt templates must exist at the specified path
- Multiple `view_type = "Mcp"` views per app and per bundle are allowed — each registers its own JSON-RPC endpoint

**Example:**

```bash
riverpackage validate canary-bundle
```

If validation succeeds:

```
Bundle:     canary-bundle
✓ Structure validation
✓ Existence validation
✓ Cross-reference validation
  (mcp tools: search_contacts, list_all_contacts)
✓ Syntax validation
```

### Best Practices

1. **Use clear tool descriptions** — AI models use descriptions to decide whether to call a tool. Be specific about what the tool does and what parameters it expects.

2. **Keep tools focused** — Expose one logical operation per tool. Don't bundle unrelated queries into a single tool.

3. **Mark read-only tools** — Set `hints = { read_only = true }` for tools that don't modify state. This helps AI models understand the cost of calling the tool.

4. **Document required parameters** — DataView parameters become tool input schema fields. Document them in your DataView configuration.

5. **Use resources for context** — Instead of expecting the AI to discover schema information, expose it via resources. This ensures the AI has accurate information about your API.

6. **Test with a real AI client** — While curl works for basic testing, test your MCP configuration with an actual AI client (Claude Desktop, ChatGPT, etc.) to verify parameter handling and response format.

---

## Summary

## Resource Subscriptions (Live Updates)

Rivers supports the MCP `resources/subscribe` extension, which lets clients receive push notifications when a resource changes.

### Marking a Resource as Subscribable

Add `subscribable = true` and an optional `poll_interval_seconds` to any resource:

```toml
[api.views.mcp.resources.contacts]
dataview          = "list_contacts"
description       = "All contacts"
subscribable      = true
poll_interval_seconds = 10
```

- `subscribable = true` tells Rivers to start a background poller for this resource when a client subscribes.
- `poll_interval_seconds` (default: 5) controls how often the poller re-executes the DataView. The server enforces a floor via `[mcp] min_poll_interval_seconds` in `riversd.toml` (default: 1 second).

### The read-then-subscribe pattern

Clients receive **change notifications only**, not an initial data snapshot. The recommended pattern is:

1. Call `resources/read` to get the current state.
2. Call `resources/subscribe` to register for updates.
3. When the SSE stream delivers a `notifications/resources/updated` event, call `resources/read` again to get the new data.

This keeps the subscription channel thin (event only, no payload) and avoids large initial data transfers for rarely-changing resources.

### ORDER BY requirement for subscribable DataViews

The change poller detects changes by SHA-256 hashing the serialized rows. For this to work correctly, the DataView's query **must include a deterministic `ORDER BY` clause**. Without a stable order, row set hash changes may be triggered by ordering variation rather than data changes, resulting in spurious notifications.

```toml
# Good: deterministic order
[data.dataviews.list_contacts]
query = "SELECT id, name, email FROM contacts ORDER BY id"

# Bad: non-deterministic order causes false-positive notifications
[data.dataviews.list_contacts]
query = "SELECT id, name, email FROM contacts"
```

### Server-side subscription configuration

In `riversd.toml`:

```toml
[mcp]
max_subscriptions_per_session = 100   # cap per SSE session (default: 100)
min_poll_interval_seconds     = 1     # floor for poll_interval_seconds (default: 1)
```

### Opening the SSE stream

The SSE stream is opened by sending `GET` to the MCP endpoint with `Accept: text/event-stream` and a valid `Mcp-Session-Id` header. Rivers returns a chunked SSE response. The stream stays open and Rivers sends keepalive comment frames every 30 seconds.

```bash
# In one terminal: open the SSE stream
curl -N -H "Accept: text/event-stream" \
  -H "Mcp-Session-Id: <session-id>" \
  https://localhost:8080/my-app/mcp

# In another terminal: subscribe to a resource
curl -X POST \
  -H "Content-Type: application/json" \
  -H "Mcp-Session-Id: <session-id>" \
  -d '{"jsonrpc":"2.0","id":10,"method":"resources/subscribe","params":{"uri":"rivers://my-app/contacts"}}' \
  https://localhost:8080/my-app/mcp
```

When the data changes, the SSE stream delivers:

```
data: {"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"rivers://my-app/contacts"}}
```

---

This tutorial covered:

1. **Adding MCP to your app** — Create an `[api.views.mcp]` view with POST method
2. **Exposing tools** — Map DataViews to tools with `[api.views.mcp.tools.*]` sections
3. **Testing with curl** — Initialize, list tools, and call tools via JSON-RPC
4. **Adding resources** — Expose read-only DataViews as context data
5. **Adding prompts** — Create markdown templates for AI workflows
6. **Session management** — Understand MCP session lifecycle and TTL
7. **Validation** — Verify MCP configurations during bundle deployment
8. **Resource subscriptions** — Live change notifications via SSE

MCP makes your Rivers application discoverable and accessible to AI models, enabling powerful AI-assisted workflows without custom code.
