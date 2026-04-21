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
//   export function handler(ctx: Ctx): void {
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
     */
    encrypt(plaintext: string): string;
    /** AES-256-GCM decrypt — inverse of `encrypt`. */
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
 */
interface Ctx {
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
 *   export function handler(ctx: Ctx): void { ... }
 *
 * Return values are coerced to JSON and used as `ctx.resdata` if the
 * handler did not set `resdata` explicitly.
 */
export type HandlerFn = (ctx: Ctx) => void | unknown;

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

export {};
