# Tutorial: TypeScript Handlers

**Rivers v0.50.1**

## Overview

Rivers supports TypeScript natively in V8. You get type safety, interfaces, and modern syntax — the V8 engine transpiles TypeScript at load time with no build step needed.

This tutorial covers TypeScript-specific patterns. For general handler concepts (ctx object, Rivers globals, patterns), see the [JavaScript Handlers tutorial](tutorial-js-handlers.md).

---

## Configuration

Use `language = "typescript"` and `.ts` file extensions.

```toml
[api.views.create_order.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/orders.ts"
entrypoint = "createOrder"
resources  = ["orders_db"]
```

### Language Variants

| Value | Behavior |
|-------|----------|
| `"typescript"` | Standard TypeScript |
| `"ts"` | Alias for `"typescript"` |
| `"ts_v8"` | Alias for `"typescript"` |
| `"typescript_strict"` | Strict mode — stricter type checking |

---

## Type Definitions

Define interfaces for the Rivers context to get full type safety in your handlers.

### Core Types

```typescript
// ── Request ──

interface ParsedRequest {
    method: string;
    path: string;
    query_params: Record<string, string>;
    headers: Record<string, string>;
    body: any | null;
    path_params: Record<string, string>;
}

// ── Session ──

interface SessionClaims {
    subject: string;
    username: string;
    email?: string;
    groups: string[];
    [key: string]: any;
}

// ── KV Store ──

interface Store {
    get(key: string): any | null;
    set(key: string, value: any, ttl_ms: number): void;
    del(key: string): void;
}

// ── WebSocket ──

interface WebSocketContext {
    connection_id: string;
    message: any;
}

// ── Main Context ──

interface ViewContext {
    request: ParsedRequest;
    data: Record<string, any>;
    dataview(name: string, params?: Record<string, any>): any;
    store: Store;
    session: SessionClaims;
    ws: WebSocketContext;
    resdata: any;
    trace_id: string;
    app_id: string;
    env: string;
}

// ── Rivers Globals ──

declare const Rivers: {
    log: {
        info(msg: string, data?: Record<string, any>): void;
        warn(msg: string, data?: Record<string, any>): void;
        error(msg: string, data?: Record<string, any>): void;
    };
    crypto: {
        hashPassword(plain: string): string;
        verifyPassword(plain: string, hash: string): boolean;
        randomHex(bytes: number): string;
        randomBase64url(bytes: number): string;
        hmac(key: string, data: string): string;
        timingSafeEqual(a: string, b: string): boolean;
        encrypt(keyName: string, plaintext: string, options?: { aad?: string }): { ciphertext: string; nonce: string; key_version: number };
        decrypt(keyName: string, ciphertext: string, nonce: string, options: { key_version: number; aad?: string }): string;
    };
    keystore: {
        has(name: string): boolean;
        info(name: string): { name: string; type: string; version: number; created_at: string };
    };
    http: {
        get(url: string): Promise<HttpResponse>;
        post(url: string, body?: any): Promise<HttpResponse>;
        put(url: string, body?: any): Promise<HttpResponse>;
        del(url: string): Promise<HttpResponse>;
    };
};

interface HttpResponse {
    status: number;
    body: any;
    headers: Record<string, string>;
}

// ── Streaming ──

declare const __args: {
    iteration: number;
    state: any;
};
```

You can put these in a `libraries/types/rivers.d.ts` file and reference them across handlers.

---

## Basic Handler

```typescript
// libraries/handlers/products.ts

interface Product {
    id: string;
    name: string;
    price: number;
    category: string;
    in_stock: boolean;
}

function listProducts(ctx: ViewContext): void {
    const products: Product[] = ctx.data.list_products;
    const category = ctx.request.query_params.category;

    if (category) {
        ctx.resdata = products.filter((p: Product) => p.category === category);
    } else {
        ctx.resdata = products;
    }
}

function getProduct(ctx: ViewContext): void {
    const id: string = ctx.request.path_params.id;
    const product: Product | null = ctx.dataview("get_product", { id });

    if (!product) {
        throw new Error("product not found");
    }

    ctx.resdata = product;
}
```

---

## CRUD with Typed Request Bodies

```typescript
// libraries/handlers/users.ts

interface User {
    id: string;
    name: string;
    email: string;
    role: string;
    created_at: string;
}

interface CreateUserRequest {
    name: string;
    email: string;
    role?: string;
}

interface UpdateUserRequest {
    name?: string;
    email?: string;
    role?: string;
}

function createUser(ctx: ViewContext): void {
    const body = ctx.request.body as CreateUserRequest;

    if (!body?.name || !body?.email) {
        throw new Error("name and email are required");
    }

    const user: User = ctx.dataview("insert_user", {
        name: body.name,
        email: body.email,
        role: body.role ?? "member",
    });

    Rivers.log.info("user created", { id: user.id, email: user.email });
    ctx.resdata = user;
}

function updateUser(ctx: ViewContext): void {
    const id: string = ctx.request.path_params.id;
    const body = ctx.request.body as UpdateUserRequest;

    const existing: User | null = ctx.dataview("get_user", { id });
    if (!existing) {
        throw new Error("user not found");
    }

    const updated: User = ctx.dataview("update_user", {
        id,
        name: body?.name ?? existing.name,
        email: body?.email ?? existing.email,
        role: body?.role ?? existing.role,
    });

    Rivers.log.info("user updated", { id });
    ctx.resdata = updated;
}

function deleteUser(ctx: ViewContext): void {
    const id: string = ctx.request.path_params.id;

    const existing: User | null = ctx.dataview("get_user", { id });
    if (!existing) {
        throw new Error("user not found");
    }

    ctx.dataview("delete_user", { id });
    Rivers.log.info("user deleted", { id });
    ctx.resdata = { deleted: true, id };
}
```

---

## Auth Guard

```typescript
// libraries/handlers/auth.ts

interface LoginRequest {
    username: string;
    password: string;
}

interface UserRecord {
    id: string;
    username: string;
    email: string;
    password_hash: string;
    groups: string[];
}

// Guard handler — return value becomes session claims
function login(ctx: ViewContext): SessionClaims {
    const body = ctx.request.body as LoginRequest;

    if (!body?.username || !body?.password) {
        throw new Error("username and password are required");
    }

    const user: UserRecord | null = ctx.dataview("get_user_by_username", {
        username: body.username,
    });

    if (!user || !Rivers.crypto.verifyPassword(body.password, user.password_hash)) {
        Rivers.log.warn("login failed", { username: body.username });
        throw new Error("invalid credentials");
    }

    Rivers.log.info("login successful", { user_id: user.id });

    return {
        subject: user.id,
        username: user.username,
        email: user.email,
        groups: user.groups,
    };
}

// Protected endpoint
function getProfile(ctx: ViewContext): void {
    const { subject, username, groups } = ctx.session;
    const user: UserRecord = ctx.dataview("get_user", { id: subject });

    ctx.resdata = {
        id: user.id,
        username,
        email: user.email,
        groups,
    };
}
```

---

## Async / Parallel DataView Calls

```typescript
// libraries/handlers/dashboard.ts

interface DashboardData {
    recent_users: User[];
    recent_orders: Order[];
    metrics: SystemMetrics;
    generated_at: string;
}

interface Order {
    id: string;
    total: number;
    status: string;
    created_at: string;
}

interface SystemMetrics {
    cpu_percent: number;
    mem_mb: number;
    active_connections: number;
}

async function getDashboard(ctx: ViewContext): Promise<void> {
    const [users, orders, metrics] = await Promise.all([
        Promise.resolve(ctx.dataview("recent_users", { limit: 5 })) as Promise<User[]>,
        Promise.resolve(ctx.dataview("recent_orders", { limit: 10 })) as Promise<Order[]>,
        Promise.resolve(ctx.dataview("system_metrics")) as Promise<SystemMetrics>,
    ]);

    const data: DashboardData = {
        recent_users: users,
        recent_orders: orders,
        metrics,
        generated_at: new Date().toISOString(),
    };

    ctx.resdata = data;
}
```

---

## Streaming Handler

```typescript
// libraries/handlers/export.ts

interface ExportRow {
    row_number: number;
    id: string;
    value: string;
    exported_at: string;
}

interface StreamChunk {
    chunk: ExportRow;
    done: false;
}

interface StreamDone {
    done: true;
}

type StreamResult = StreamChunk | StreamDone;

function exportData(ctx: ViewContext): StreamResult {
    const iteration: number = __args.iteration ?? 0;
    const totalRows = 100;

    if (iteration >= totalRows) {
        Rivers.log.info("export complete", { rows: totalRows });
        return { done: true };
    }

    return {
        chunk: {
            row_number: iteration + 1,
            id: Rivers.crypto.randomHex(8),
            value: `row-${iteration + 1}`,
            exported_at: new Date().toISOString(),
        },
        done: false,
    };
}
```

---

## WebSocket Hooks

```typescript
// libraries/handlers/chat.ts

interface ChatMessage {
    type: "chat";
    text: string;
}

interface PingMessage {
    type: "ping";
}

type InboundMessage = ChatMessage | PingMessage;

interface UserInfo {
    username: string;
    joined: string;
}

function onConnect(ctx: ViewContext): object | false {
    const connId: string = ctx.ws.connection_id;
    const username: string = ctx.request.query_params.username ?? "anonymous";

    ctx.store.set(`ws:user:${connId}`, {
        username,
        joined: new Date().toISOString(),
    } as UserInfo, 3_600_000);

    Rivers.log.info("client connected", { connection_id: connId, username });

    return {
        type: "welcome",
        message: `Hello, ${username}!`,
        connection_id: connId,
    };
}

function onMessage(ctx: ViewContext): object | null {
    const connId: string = ctx.ws.connection_id;
    const msg = ctx.ws.message as InboundMessage;
    const user = ctx.store.get(`ws:user:${connId}`) as UserInfo | null;
    const username: string = user?.username ?? "anonymous";

    if (msg.type === "ping") {
        return { type: "pong", timestamp: new Date().toISOString() };
    }

    if (msg.type === "chat") {
        return {
            type: "chat",
            username,
            text: msg.text,
            timestamp: new Date().toISOString(),
        };
    }

    return { type: "error", message: `unknown type: ${(msg as any).type}` };
}

function onDisconnect(ctx: ViewContext): void {
    const connId: string = ctx.ws.connection_id;
    ctx.store.del(`ws:user:${connId}`);
    Rivers.log.info("client disconnected", { connection_id: connId });
}
```

---

## Outbound HTTP

```typescript
// libraries/handlers/integrations.ts

interface WeatherData {
    temperature: number;
    conditions: string;
    humidity: number;
}

async function fetchWeather(ctx: ViewContext): Promise<void> {
    const city: string = ctx.request.query_params.city ?? "London";

    const resp: HttpResponse = await Rivers.http.get(
        `https://api.example.com/weather?city=${encodeURIComponent(city)}`
    );

    if (resp.status !== 200) {
        Rivers.log.error("weather API failed", { status: resp.status, city });
        throw new Error("weather service unavailable");
    }

    const weather = resp.body as WeatherData;

    ctx.resdata = {
        city,
        weather,
        fetched_at: new Date().toISOString(),
    };
}
```

---

## KV Store with Generics

```typescript
// libraries/handlers/cart.ts

interface CartItem {
    product_id: string;
    quantity: number;
    added_at: string;
}

interface Cart {
    items: CartItem[];
    updated_at: string | null;
}

function addToCart(ctx: ViewContext): void {
    const cartId: string = ctx.request.path_params.cart_id;
    const body = ctx.request.body as { product_id: string; quantity?: number };
    const key = `cart:${cartId}`;

    const cart: Cart = ctx.store.get(key) as Cart ?? { items: [], updated_at: null };

    cart.items.push({
        product_id: body.product_id,
        quantity: body.quantity ?? 1,
        added_at: new Date().toISOString(),
    });
    cart.updated_at = new Date().toISOString();

    // TTL 24 hours
    ctx.store.set(key, cart, 86_400_000);

    Rivers.log.info("cart updated", { cart_id: cartId, item_count: cart.items.length });
    ctx.resdata = cart;
}

function getCart(ctx: ViewContext): void {
    const cartId: string = ctx.request.path_params.cart_id;
    const cart: Cart = ctx.store.get(`cart:${cartId}`) as Cart ?? { items: [], updated_at: null };
    ctx.resdata = cart;
}

function clearCart(ctx: ViewContext): void {
    const cartId: string = ctx.request.path_params.cart_id;
    ctx.store.del(`cart:${cartId}`);
    ctx.resdata = { cleared: true };
}
```

---

## Project Layout

Recommended file structure for TypeScript handlers:

```
my-service/
├── libraries/
│   ├── types/
│   │   └── rivers.d.ts          ← shared type definitions
│   └── handlers/
│       ├── auth.ts
│       ├── users.ts
│       ├── orders.ts
│       └── dashboard.ts
├── schemas/
├── app.toml
├── manifest.toml
└── resources.toml
```

Put shared interfaces (ViewContext, SessionClaims, etc.) in `types/rivers.d.ts`. Handler files import implicitly — V8 loads the types at startup.

---

## TypeScript vs JavaScript — When to Use Which

| Use TypeScript when | Use JavaScript when |
|---------------------|---------------------|
| Complex request/response shapes | Simple pass-through handlers |
| Multi-DataView orchestration | Single DataView dispatch |
| Team collaboration (types as docs) | Quick prototypes |
| Auth flows with session claims | One-off scripts |
| Strict mode for production safety | Maximum simplicity |

---

## Configuration Reference

| Config Value | Description |
|-------------|-------------|
| `"typescript"` | Standard TypeScript |
| `"ts"` | Alias for typescript |
| `"ts_v8"` | Alias for typescript |
| `"typescript_strict"` | Strict mode — additional type checks enforced |
