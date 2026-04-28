# Rivers v0.55.0 — `TextEncoder`/`TextDecoder` absent + `Rivers.crypto.sha256` missing

**Filed:** 2026-04-26
**Reporter:** CB team
**Rivers version:** 0.55.0+1219260426
**Severity:** API gap. Standard Web Platform primitive absent + a common crypto primitive missing from the Rivers crypto helper.

---

## TL;DR

Rivers' V8 isolate intentionally omits `TextEncoder`/`TextDecoder` (per the sandbox allowlist documented in `rivers-processpool-runtime-spec-v2.md` §5.1). The `Rivers.crypto` namespace is the documented escape hatch for handler-side cryptography, but it lacks a generic `sha256(input: string)` / `sha512(input: string)` primitive — only `hashPassword` (bcrypt), `hmac`, `randomHex`, `randomBase64url`, `timingSafeEqual`, and `encrypt`/`decrypt` (AES-GCM) are exposed.

Net effect: code that needs a deterministic SHA-256 hash of an arbitrary string (API key fingerprints, content addressing, signing payloads) has no clean path. Either install `TextEncoder` as a global, or add `Rivers.crypto.sha256(input)` and `sha512(input)` matching the existing helper pattern.

---

## Sandbox is a deliberate choice

We're not asking for the full Web Crypto API — that's much larger than necessary, and the sandbox stance is sound. `rivers-processpool-runtime-spec-v2.md` §5.1 (lines 270–275):

> `build_global_template` constructs an `ObjectTemplate` containing exactly and only the Rivers API objects derived from the `TaskContext`. No prototype chain inheritance from a full `globalThis`. No `process`. No `require`. No `fetch` unless `allow_outbound_http = true`.

Allowlist injection is the right model. The ask is: install one or two more primitives that are common enough to deserve first-class status.

---

## Currently installed `Rivers.crypto` surface

From `crates/riversd/src/process_pool/v8_engine/rivers_global.rs` (line numbers below show callback installation):

| Method | Line | Purpose |
|---|---:|---|
| `randomHex(bytes)` | 250 | Random hex string |
| `hashPassword(plain)` | 275 | bcrypt hash |
| `verifyPassword(plain, hash)` | 293 | bcrypt verify |
| `timingSafeEqual(a, b)` | 315 | Constant-time string compare |
| `randomBase64url(bytes)` | 336 | Random base64url |
| `hmac(key, data)` | 405 | HMAC-SHA256 |
| `encrypt(keyName, plaintext, opts?)` | 475 | AES-256-GCM via keystore |
| `decrypt(keyName, ciphertext, nonce, opts)` | 561 | AES-256-GCM decrypt |

What's missing in the gap:

- `sha256(input: string): string` — hex-encoded SHA-256
- `sha512(input: string): string` — hex-encoded SHA-512

(`TextEncoder` is the alternative — install it as a V8 global, and `crypto.subtle.digest` if exposed; but a single helper method is a smaller, more focused fix.)

---

## Use case (CB's specifically)

API key authentication: the CLI stores a plaintext key in `~/.cb/credentials`. The server stores a fingerprint. Validation requires the server to compute the same fingerprint deterministically from the bearer token in `Authorization: Bearer <key>`.

The natural primitive is `sha256(bearer)`. With neither `TextEncoder` nor `Rivers.crypto.sha256` available, our worked-around path is `Rivers.crypto.hmac("cb-api-keys-v1", bearer)` — same effect (deterministic hash) but semantically wrong (HMAC's purpose is keyed authentication, not hashing). Anyone reviewing the code asks "what's the secret in `cb-api-keys-v1`?"; the answer "nothing — it's just a versioning pepper" is a workaround we'd rather not write.

Other domains that hit this:
- Content-addressed storage (hash-keyed cache lookups)
- Webhook signature verification (Stripe / GitHub style — provider sends `sha256=<hex>`, you must recompute and constant-time compare)
- Idempotency keys (hash request body, dedupe by hash)
- Merkle proofs / git-style content hashes

All of these are reasonable handler workloads; all currently require either the HMAC workaround or a custom JS sha256 implementation (slow, error-prone).

---

## Reproducer

Minimal handler that should work in any modern JS runtime:

```typescript
export function probe(ctx) {
    // Use case: hash a request body for idempotency-key dedup.
    const bytes = new TextEncoder().encode(JSON.stringify(ctx.request.body));
    crypto.subtle.digest("SHA-256", bytes).then(buf => {
        const hex = [...new Uint8Array(buf)].map(b => b.toString(16).padStart(2, "0")).join("");
        ctx.resdata = { hash: hex };
    });
}
```

Observed against Rivers v0.55.0:

```
ReferenceError: TextEncoder is not defined
```

(Or `crypto is not defined` if you skip TextEncoder.)

---

## Recommended fix

**Option A (preferred, smaller surface):** add `Rivers.crypto.sha256` and `Rivers.crypto.sha512`. Implementation mirrors the existing `hmac` callback at line 405 — same `sha2`/`hex` crates already in `rivers-engine-v8/Cargo.toml`:

```rust
fn sha256_callback(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    use sha2::{Digest, Sha256};
    let input = args.get(0).to_rust_string_lossy(scope);
    let hash = Sha256::digest(input.as_bytes());
    let hex = hex::encode(hash);
    rv.set(v8_str_safe(scope, &hex).into());
}

// At rivers_global.rs#build_rivers_global, alongside the hmac install:
let sha256_fn = v8::Function::new(scope, sha256_callback)?;
crypto_obj.set(scope, v8_str(scope, "sha256")?.into(), sha256_fn.into());
let sha512_fn = v8::Function::new(scope, sha512_callback)?;
crypto_obj.set(scope, v8_str(scope, "sha512")?.into(), sha512_fn.into());
```

~30 LOC + 2 unit tests asserting known SHA vectors. Effort: 1 hour.

**Option B (broader surface, more compatible):** install `TextEncoder` and `TextDecoder` as V8 globals. These are part of the WHATWG Encoding spec, available in Node, Deno, Bun, browsers, every Cloudflare Worker. The implementations are trivial (UTF-8 encode/decode). With them installed, handlers can use any standard JS pattern that converts strings to bytes.

Option A is what we'd file as the request. Option B is a reasonable larger change if the Rivers team wants to broaden compatibility with the standard JS ecosystem.

---

## What we don't need

- Full `crypto.subtle` (Web Crypto subtle async API) — overkill, and async API in a sync isolate is awkward
- Full `Buffer` (Node-specific) — out of scope
- Streaming hash APIs — `sha256(input)` over a single string covers the 95% case

---

## References

- Sandbox spec: `docs/arch/rivers-processpool-runtime-spec-v2.md` §5.1, §5.2 (lines 258–305)
- Existing crypto callbacks: `crates/riversd/src/process_pool/v8_engine/rivers_global.rs:250–561`
- Tutorial declaring `Rivers.crypto.*` surface: `docs/guide/tutorials/tutorial-ts-handlers.md` lines 95–140
- TS spec stating "no `console`, `process`, `require`, `fetch`" as negative declarations — does NOT mention `TextEncoder`/`TextDecoder` as deliberately omitted: `docs/arch/rivers-javascript-typescript-spec.md`
