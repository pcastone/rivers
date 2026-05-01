// Rivers handler API — TypeScript ambient declarations.
//
// Authoritative source for every surface available inside a CodeComponent
// handler running on Rivers v0.54.x+. Generated to match
// `docs/arch/rivers-javascript-typescript-spec.md §8`.
//
// Usage in a handler project:
//
//   // tsconfig.json
//   {
//     "compilerOptions": {
//       "target": "ES2022",
//       "module": "ES2022",
//       "moduleResolution": "bundler",
//       "strict": true,
//       "types": ["./types/rivers"]
//     }
//   }
//
//   // libraries/handlers/orders.ts
//   export function handler(ctx: ViewContext): void {
//     Rivers.log.info("processing order", { order_id: ctx.request.body.id });
//     ctx.resdata = { ok: true };
//   }
//
// Rivers runtime notes:
// - All `.ts` files under `libraries/` are compiled at bundle load via swc
//   (full-transform pipeline). Compile errors block startup.
// - Imports MUST use explicit `.ts` / `.js` extensions (Deno style).
//   Bare specifiers and absolute paths are rejected at bundle load.
// - Circular imports are rejected at bundle load.
// - `tsx` / JSX is not supported in v1.

// ── Rivers global ───────────────────────────────────────────────

/**
 * The `Rivers` global provides logging, crypto primitives, keystore access,
 * and environment variables. Always available in every handler.
 */
declare const Rivers: RiversGlobal;

interface RiversGlobal {
    /** Structured logger writing to the per-app log file. */
    log: RiversLog;
    /** Crypto primitives — random, hash, HMAC, constant-time compare, app-key encrypt/decrypt. */
    crypto: RiversCrypto;
    /**
     * Application keystore — per-app AES-256-GCM key management. Available
     * only when `[keystore]` is configured in the app manifest.
     * @capability keystore
     */
    keystore: RiversKeystore;
    /** Environment variables exposed to the handler (subset allowlisted by config). */
    env: Readonly<Record<string, string>>;
}

interface RiversLog {
    trace(message: string, fields?: Record<string, unknown>): void;
    debug(message: string, fields?: Record<string, unknown>): void;
    info(message: string, fields?: Record<string, unknown>): void;
    warn(message: string, fields?: Record<string, unknown>): void;
    error(message: string, fields?: Record<string, unknown>): void;
}

interface RiversCrypto {
    /** Fill a Uint8Array-like length with cryptographic random bytes; returns hex. */
    random(byteLength: number): string;
    /** SHA-256 of the input string, returned as hex. */
    hash(input: string): string;
    /** Constant-time string compare (protects against timing side channels). */
    timingSafeEqual(a: string, b: string): boolean;
    /** HMAC-SHA256. Key is a lockbox alias if configured, else a raw hex string. */
    hmac(key: string, message: string): string;
    /**
     * AES-256-GCM encrypt with the app's active keystore key.
     * Throws if `[keystore]` is not configured for the app.
     * @capability keystore
     */
    encrypt(plaintext: string): string;
    /**
     * AES-256-GCM decrypt — inverse of `encrypt`.
     * @capability keystore
     */
    decrypt(ciphertext: string): string;
}

interface RiversKeystore {
    /** List known key IDs for this app. */
    list(): string[];
    /** Return metadata for a specific key: id, algorithm, created_at, rotated_from. */
    info(keyId: string): KeystoreKeyInfo;
}

interface KeystoreKeyInfo {
    id: string;
    algorithm: "AES-256-GCM";
    created_at: string; // ISO-8601
    rotated_from?: string;
}

// ── Handler context (ctx) ──────────────────────────────────────

/**
 * The `ctx` argument passed to every handler carries request metadata,
 * pre-fetched data, the response slot, and all host callbacks.
 *
 * `Ctx` is provided as a short alias for backwards compatibility with
 * handlers written against earlier Rivers releases; new code should
 * prefer `ViewContext`.
 */
interface ViewContext {
    /** Distributed-trace identifier — same value appears in log entries. */
    readonly trace_id: string;
    /** Per-node identifier — useful for debugging multi-node deployments. */
    readonly node_id: string;
    /** Application identifier — same as `appId` in the app manifest. */
    readonly app_id: string;
    /** Allowlisted environment variables. Shortcut for `Rivers.env`. */
    readonly env: Readonly<Record<string, string>>;

    /** Parsed HTTP request — method, path, headers, body, params. */
    readonly request: ParsedRequest;
    /** Session claims if the view has `auth = "session"` and a session is active. */
    readonly session: SessionClaims | null;

    /**
     * Pre-fetched DataView results keyed by dataview name. Populated by
     * the pipeline when `dataviews = [...]` appears on the view config.
     * Handlers may also call `ctx.dataview(name)` to fetch lazily.
     */
    readonly data: Record<string, DataViewResult>;

    /**
     * Handler response payload. Set this to return JSON data. If left
     * `null` / `undefined`, the handler's return value is used instead.
     */
    resdata: unknown;

    /**
     * Execute a DataView synchronously and return its result.
     * - Inside a `ctx.transaction()` callback, routes through the
     *   held connection.
     * - Spec §6.2: cross-datasource calls inside a transaction throw
     *   `TransactionError`.
     */
    dataview(name: string, params?: Record<string, unknown>): DataViewResult;

    /**
     * Begin a transaction on the named datasource. Invokes `fn` with no
     * arguments; commits on clean return, rolls back on throw.
     * Spec §6. `ctx.dataview()` inside `fn` uses the held connection.
     *
     * Throws `TransactionError` for:
     *   - nested calls (no nesting support in v1)
     *   - unknown datasource
     *   - driver that does not support transactions
     *   - cross-datasource `ctx.dataview` inside the callback
     *
     * @capability transaction — datasource driver's `supports_transactions()`
     *   must return `true`. PostgreSQL, MySQL, SQLite ship with transactions
     *   enabled; other drivers vary (see spec §6.4).
     */
    transaction<T>(datasource: string, fn: () => T): T;

    /** Per-request key-value store. Handlers may share values here. */
    store: CtxStore;

    /** Fluent datasource builder — `.fromQuery(sql, params).build()`. */
    datasource(name: string): DatasourceBuilder;

    /**
     * DDL execution. Only reachable from init handlers configured with
     * `[init]` in the app manifest. Rejected elsewhere.
     */
    ddl(datasource: string, statement: string): void;

    /**
     * Request structured user input from the MCP client (P2.6).
     *
     * Suspends the handler and sends an `elicitation/create` request to the
     * MCP client over the session's SSE stream. Returns a Promise that
     * resolves when the client responds (via `elicitation/response`) or times
     * out after 60 seconds.
     *
     * Only available when the handler is invoked via MCP (`tools/call`).
     * Calling from REST or WebSocket handlers throws an Error.
     *
     * On timeout, resolves with `{ action: "cancel" }`.
     *
     * @capability mcp — handler must be a codecomponent view invoked via MCP.
     */
    elicit(spec: ElicitationSpec): Promise<ElicitationResult>;
}

interface ParsedRequest {
    method: "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | string;
    path: string;
    headers: Record<string, string>;
    query: Record<string, string | string[]>;
    params: Record<string, string>;
    body: unknown;
}

interface SessionClaims {
    subject: string;
    expires_at: string; // ISO-8601
    [claim: string]: unknown;
}

interface DataViewResult {
    rows: Array<Record<string, unknown>>;
    affected_rows: number;
    last_insert_id?: number | string | null;
}

interface CtxStore {
    get(key: string): unknown;
    set(key: string, value: unknown, ttl_seconds?: number): void;
    del(key: string): void;
}

interface DatasourceBuilder {
    fromQuery(sql: string, params?: Record<string, unknown>): DatasourceBuilder;
    fromSchema(schema: string, params?: Record<string, unknown>): DatasourceBuilder;
    withGetSchema(schema: string): DatasourceBuilder;
    withPostSchema(schema: string): DatasourceBuilder;
    withPutSchema(schema: string): DatasourceBuilder;
    withDeleteSchema(schema: string): DatasourceBuilder;
    build(): QueryResult;

    /**
     * Publish a message to a message-broker datasource (kafka, rabbitmq,
     * nats, redis-streams). Only available when `name` resolves to a
     * broker datasource; throws `Error("... not a broker datasource")`
     * otherwise. BR-2026-04-23.
     *
     * @capability broker — driver implements `MessageBrokerDriver`.
     */
    publish?(message: OutboundMessage): PublishReceipt;
}

/**
 * Broker publish message. Field names mirror the Rust
 * `rivers-driver-sdk::broker::OutboundMessage` struct verbatim.
 *
 * - `destination` — topic (kafka), routing key (rabbitmq), subject (nats),
 *   stream name (redis-streams). Required, non-empty.
 * - `payload` — `string` (sent as UTF-8 bytes) OR any JSON-serialisable
 *   value (auto JSON-stringified to bytes). Required.
 * - `headers` — driver-specific metadata; all values coerced to strings.
 * - `key` — partition key (kafka) or routing-key suffix (nats). Optional.
 * - `reply_to` — NATS request/reply address. Optional.
 */
interface OutboundMessage {
    destination: string;
    payload: string | object | number | boolean | null;
    headers?: Record<string, string>;
    key?: string;
    reply_to?: string;
}

/**
 * Broker publish receipt. Both fields may be `null` — population is
 * broker-specific (kafka sets both, NATS typically neither).
 */
interface PublishReceipt {
    id: string | null;
    metadata: string | null;
}

interface QueryResult {
    rows: Array<Record<string, unknown>>;
    affected_rows: number;
    last_insert_id?: number | string | null;
}

interface ExecuteResult {
    affected_rows: number;
    last_insert_id?: number | string | null;
}

// ── MCP Elicitation (P2.6) ─────────────────────────────────────

/**
 * Spec passed to `ctx.elicit()` — describes what the MCP client should
 * ask the user and what JSON shape the answer should have.
 *
 * `requestedSchema` is a JSON Schema object that the MCP client uses to
 * render a structured input form.
 */
interface ElicitationSpec {
    /** Short title for the input dialog / prompt. */
    title: string;
    /** Human-readable description of what is being requested. */
    message: string;
    /** JSON Schema describing the expected response shape. */
    requestedSchema: object;
}

/**
 * Result returned by `ctx.elicit()`.
 *
 * `action` indicates what the user did:
 * - `"accept"` — user filled in the form and submitted it. `content` holds the data.
 * - `"decline"` — user dismissed the dialog. `content` is `undefined`.
 * - `"cancel"` — the request timed out (60 s) or the SSE stream was unavailable.
 */
interface ElicitationResult {
    action: "accept" | "decline" | "cancel";
    /** User-supplied data, present only when `action === "accept"`. */
    content?: object;
}

// ── TransactionError ───────────────────────────────────────────

/**
 * Thrown by `ctx.transaction()` and by `ctx.dataview()` calls inside a
 * transaction callback when one of the spec §6 invariants is violated.
 */
declare class TransactionError extends Error {
    readonly name: "TransactionError";
    readonly kind: "nested" | "unsupported" | "cross-datasource" | "unknown-datasource" | "begin-failed" | "commit-failed";
}

// ── Handler entrypoint (user code) ─────────────────────────────

/**
 * Handler signature. Export a function matching this shape; the name
 * must match the `entrypoint` in the view config.
 *
 *   export function handler(ctx: ViewContext): void { ... }
 *
 * Return values are coerced to JSON and used as `ctx.resdata` if the
 * handler did not set `resdata` explicitly.
 */
export type HandlerFn = (ctx: ViewContext) => void | unknown;

// ── Negative declarations (spec §8.3) ──────────────────────────

/*
 * Rivers does NOT provide `console`, `process`, `require`, or `fetch` in
 * handler scope. This file intentionally omits their declarations so the
 * type checker catches incorrect calls at development time. Use `Rivers.log`
 * instead of `console`; import explicit ESM modules with `.ts`/`.js`
 * extensions instead of `require`; outbound HTTP is gated behind view
 * config `allow_outbound_http` and exposed through a different API that
 * will be typed in a future release.
 */

// ── Capability gates (informational) ───────────────────────────

/*
 * Capability tags used in JSDoc across this file:
 *
 *   @capability keystore         — `[data.keystore]` declared in app.toml;
 *                                  Rivers.crypto.encrypt/decrypt, Rivers.keystore
 *   @capability transaction      — datasource driver's supports_transactions
 *                                  returns true; ctx.transaction() + the
 *                                  held-connection path of ctx.dataview
 *   @capability outbound_http    — view's allow_outbound_http = true;
 *                                  reserved for the future typed fetch API
 *   @capability broker           — datasource's driver implements
 *                                  MessageBrokerDriver (kafka, rabbitmq,
 *                                  nats, redis-streams); ctx.datasource(n)
 *                                  returns a proxy with .publish(msg).
 *                                  BR-2026-04-23.
 *
 * Handlers that call a capability-gated surface without the gate enabled
 * will throw at runtime (`Unsupported`, `keystore not configured`, etc.).
 */

// ── Backwards-compatible alias ─────────────────────────────────

/**
 * Short alias for `ViewContext`. Earlier Rivers releases used `Ctx` as
 * the primary interface name; new code should prefer `ViewContext`.
 */
type Ctx = ViewContext;

export {};
