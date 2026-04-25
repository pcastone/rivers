//! `inject_rivers_global()` -- Rivers.log, Rivers.crypto, Rivers.keystore,
//! Rivers.env, Rivers.db bindings and their callbacks.

use super::super::types::*;
use super::task_locals::*;
use super::init::v8_str;
use super::http::{
    json_to_v8,
    rivers_http_get_callback, rivers_http_post_callback,
    rivers_http_put_callback, rivers_http_del_callback,
};

/// Extract optional structured fields from a V8 value for `Rivers.log`.
///
/// Per spec SS5.2: `Rivers.log.info(msg, fields?)` supports an optional
/// second argument containing a fields object for structured logging.
/// Returns a JSON string of the fields, or empty string if no fields.
fn extract_log_fields(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> String {
    if val.is_undefined() || val.is_null() {
        return String::new();
    }
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(val) {
        if let Some(json_str) = v8::json::stringify(scope, obj.into()) {
            return json_str.to_rust_string_lossy(scope);
        }
    }
    String::new()
}

fn current_app_name() -> String {
    super::task_locals::TASK_APP_NAME.with(|c| {
        c.borrow().clone().unwrap_or_else(|| "unknown".to_string())
    })
}

/// Write a structured log line to the app's per-app log file (in addition to tracing).
fn write_to_app_log(app: &str, level: &str, msg: &str, fields: &str) {
    if let Some(router) = rivers_runtime::rivers_core::app_log_router::global_router() {
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let line = if fields.is_empty() {
            format!(r#"{{"timestamp":"{timestamp}","level":"{level}","app":"{app}","message":"{msg}"}}"#)
        } else {
            format!(r#"{{"timestamp":"{timestamp}","level":"{level}","app":"{app}","message":"{msg}","fields":{fields}}}"#)
        };
        router.write(app, &line);
    }
}

/// Inject the `Rivers` global utility namespace.
///
/// - `Rivers.log.{info,warn,error}` -- native V8 callbacks -> Rust `tracing` (P2.1).
///   Supports optional structured fields: `Rivers.log.info(msg, { key: val })`.
/// - `Rivers.crypto.randomHex` -- real randomness via `rand` (P2.2).
/// - `Rivers.crypto.hashPassword/verifyPassword` -- bcrypt cost 12 (P3.6).
/// - `Rivers.crypto.timingSafeEqual` -- constant-time comparison (P3.6).
/// - `Rivers.crypto.randomBase64url` -- real random base64url (P3.6).
/// - `Rivers.crypto.hmac` -- real HMAC-SHA256 via `hmac` crate (V2).
/// - `Rivers.http.{get,post,put,del}` -- real outbound HTTP via reqwest + async bridge (V2).
///   Only injected when `TaskContext.http` is `Some` (capability gating per spec SS10.5).
/// - `Rivers.env` -- task environment variables from `TaskContext.env` (V2).
/// - `console.{log,warn,error}` -- delegates to `Rivers.log` (P2.3).
pub(super) fn inject_rivers_global(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
) -> Result<(), TaskError> {
    let global = scope.get_current_context().global(scope);

    // ── Rivers object ────────────────────────────────────────────
    let rivers_key = v8::String::new(scope, "Rivers")
        .ok_or_else(|| TaskError::Internal("failed to create 'Rivers' key".into()))?;
    let rivers_obj = v8::Object::new(scope);

    // ── Rivers.log (native V8 -> tracing, with optional structured fields) ──
    let log_obj = v8::Object::new(scope);

    let info_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::info!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::info!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "INFO", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.info".into()))?;
    let info_key = v8_str(scope, "info")?;
    log_obj.set(scope, info_key.into(), info_fn.into());

    let warn_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::warn!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::warn!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "WARN", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.warn".into()))?;
    let warn_key = v8_str(scope, "warn")?;
    log_obj.set(scope, warn_key.into(), warn_fn.into());

    let error_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         _rv: v8::ReturnValue| {
            let msg = args.get(0).to_rust_string_lossy(scope);
            let fields = extract_log_fields(scope, args.get(1));
            let app = current_app_name();
            if fields.is_empty() {
                tracing::error!(target: "rivers.handler", app = %app, "{}", msg);
            } else {
                tracing::error!(target: "rivers.handler", app = %app, fields = %fields, "{}", msg);
            }
            write_to_app_log(&app, "ERROR", &msg, &fields);
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.log.error".into()))?;
    let error_key = v8_str(scope, "error")?;
    log_obj.set(scope, error_key.into(), error_fn.into());

    let log_key = v8_str(scope, "log")?;
    rivers_obj.set(scope, log_key.into(), log_obj.into());

    // ── Rivers.crypto (native implementations) ───────────────────
    let crypto_obj = v8::Object::new(scope);

    // Rivers.crypto.randomHex -- real randomness via rand (P2.2)
    let random_hex_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let hex_str = hex::encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &hex_str) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomHex".into()))?;
    let random_hex_key = v8_str(scope, "randomHex")?;
    crypto_obj.set(scope, random_hex_key.into(), random_hex_fn.into());

    // Rivers.crypto.hashPassword -- bcrypt cost 12 (P3.6)
    let hash_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            match bcrypt::hash(pw, 12) {
                Ok(hashed) => {
                    if let Some(v8_str) = v8::String::new(scope, &hashed) {
                        rv.set(v8_str.into());
                    }
                }
                Err(e) => {
                    let msg = v8::String::new(scope, &format!("hashPassword failed: {e}")).unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hashPassword".into()))?;
    let hash_pw_key = v8_str(scope, "hashPassword")?;
    crypto_obj.set(scope, hash_pw_key.into(), hash_pw_fn.into());

    // Rivers.crypto.verifyPassword -- bcrypt verify (P3.6)
    let verify_pw_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let pw = args.get(0).to_rust_string_lossy(scope);
            let hash = args.get(1).to_rust_string_lossy(scope);
            match bcrypt::verify(pw, &hash) {
                Ok(valid) => rv.set(v8::Boolean::new(scope, valid).into()),
                Err(_) => rv.set(v8::Boolean::new(scope, false).into()),
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.verifyPassword".into()))?;
    let verify_pw_key = v8_str(scope, "verifyPassword")?;
    crypto_obj.set(scope, verify_pw_key.into(), verify_pw_fn.into());

    // Rivers.crypto.timingSafeEqual -- constant-time comparison (P3.6)
    let timing_safe_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let a = args.get(0).to_rust_string_lossy(scope);
            let b = args.get(1).to_rust_string_lossy(scope);
            // Constant-time comparison: always compare all bytes
            let equal = a.len() == b.len()
                && a.as_bytes()
                    .iter()
                    .zip(b.as_bytes())
                    .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                    == 0;
            rv.set(v8::Boolean::new(scope, equal).into());
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.timingSafeEqual".into()))?;
    let timing_safe_key = v8_str(scope, "timingSafeEqual")?;
    crypto_obj.set(scope, timing_safe_key.into(), timing_safe_fn.into());

    // Rivers.crypto.randomBase64url -- real random base64url (P3.6)
    let random_b64_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use base64::Engine;
            use rand::Rng;
            let len = args.get(0).int32_value(scope).unwrap_or(16) as usize;
            let len = len.min(1024); // cap to prevent abuse
            let bytes: Vec<u8> = (0..len).map(|_| rand::thread_rng().gen()).collect();
            let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
            if let Some(v8_str) = v8::String::new(scope, &encoded) {
                rv.set(v8_str.into());
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.randomBase64url".into()))?;
    let random_b64_key = v8_str(scope, "randomBase64url")?;
    crypto_obj.set(scope, random_b64_key.into(), random_b64_fn.into());

    // Rivers.crypto.hmac -- HMAC-SHA256 with LockBox alias resolution (Wave 9)
    //
    // Arg 0: alias name (resolved via LockBox) or raw key (fallback when no lockbox)
    // Arg 1: data string to HMAC
    // Returns: hex-encoded HMAC-SHA256
    let hmac_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            type HmacSha256 = Hmac<Sha256>;

            let alias_or_key = args.get(0).to_rust_string_lossy(scope);
            let data = args.get(1).to_rust_string_lossy(scope);

            // Try LockBox resolution first, fall back to raw key
            let key_result: Result<String, String> = TASK_LOCKBOX.with(|lb| {
                let lb = lb.borrow();
                match lb.as_ref() {
                    Some(ctx) => {
                        let metadata = ctx.resolver.resolve(&alias_or_key)
                            .ok_or_else(|| format!("lockbox alias not found: '{alias_or_key}'"))?;
                        let resolved = rivers_runtime::rivers_core::lockbox::fetch_secret_value(
                            metadata, &ctx.keystore_path, &ctx.identity_str,
                        ).map_err(|e| format!("lockbox fetch failed: {e}"))?;
                        Ok(resolved.value)
                    }
                    None => {
                        // No lockbox configured -- use as raw key (dev/test mode)
                        Ok(alias_or_key.clone())
                    }
                }
            });

            match key_result {
                Ok(key) => {
                    match HmacSha256::new_from_slice(key.as_bytes()) {
                        Ok(mut mac) => {
                            mac.update(data.as_bytes());
                            let result = hex::encode(mac.finalize().into_bytes());
                            if let Some(v8_str) = v8::String::new(scope, &result) {
                                rv.set(v8_str.into());
                            }
                        }
                        Err(e) => {
                            let msg = v8::String::new(
                                scope,
                                &format!("Rivers.crypto.hmac() key error: {e}"),
                            )
                            .unwrap();
                            let exception = v8::Exception::error(scope, msg);
                            scope.throw_exception(exception);
                        }
                    }
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.hmac".into()))?;
    let hmac_key = v8_str(scope, "hmac")?;
    crypto_obj.set(scope, hmac_key.into(), hmac_fn.into());

    // Rivers.crypto.encrypt -- AES-256-GCM encrypt via app keystore (App Keystore feature)
    //
    // Args:
    //   0: keyName (string) -- name of the key in the app keystore
    //   1: plaintext (string) -- data to encrypt
    //   2: options (optional object) -- { aad?: string }
    // Returns: { ciphertext: string, nonce: string, key_version: number }
    let encrypt_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let key_name = args.get(0).to_rust_string_lossy(scope);
            let plaintext = args.get(1).to_rust_string_lossy(scope);

            // Extract optional AAD from options object
            let aad: Option<String> = if args.length() > 2 && args.get(2).is_object() {
                let opts = args.get(2).to_object(scope).unwrap();
                let aad_key = v8::String::new(scope, "aad").unwrap();
                let aad_val = opts.get(scope, aad_key.into());
                aad_val.and_then(|v| {
                    if v.is_undefined() || v.is_null() { None }
                    else { Some(v.to_rust_string_lossy(scope)) }
                })
            } else {
                None
            };

            let result = TASK_KEYSTORE.with(|ks| {
                let ks = ks.borrow();
                match ks.as_ref() {
                    Some(ctx) => {
                        let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                        ctx.keystore.encrypt_with_key(&key_name, plaintext.as_bytes(), aad_bytes)
                            .map_err(|e| e.to_string())
                    }
                    None => Err("keystore not configured: no [[keystores]] resource declared".to_string()),
                }
            });

            match result {
                Ok(enc) => {
                    let obj = v8::Object::new(scope);

                    let ct_key = v8::String::new(scope, "ciphertext").unwrap();
                    let ct_val = v8::String::new(scope, &enc.ciphertext).unwrap();
                    obj.set(scope, ct_key.into(), ct_val.into());

                    let nonce_key = v8::String::new(scope, "nonce").unwrap();
                    let nonce_val = v8::String::new(scope, &enc.nonce).unwrap();
                    obj.set(scope, nonce_key.into(), nonce_val.into());

                    let ver_key = v8::String::new(scope, "key_version").unwrap();
                    let ver_val = v8::Integer::new(scope, enc.key_version as i32);
                    obj.set(scope, ver_key.into(), ver_val.into());

                    rv.set(obj.into());
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.encrypt".into()))?;
    let encrypt_key = v8_str(scope, "encrypt")?;
    crypto_obj.set(scope, encrypt_key.into(), encrypt_fn.into());

    // Rivers.crypto.decrypt -- AES-256-GCM decrypt via app keystore (App Keystore feature)
    //
    // Args:
    //   0: keyName (string) -- name of the key in the app keystore
    //   1: ciphertext (string) -- base64 ciphertext from encrypt()
    //   2: nonce (string) -- base64 nonce from encrypt()
    //   3: options (object) -- { key_version: number, aad?: string }
    // Returns: plaintext string
    let decrypt_fn = v8::Function::new(
        scope,
        |scope: &mut v8::HandleScope,
         args: v8::FunctionCallbackArguments,
         mut rv: v8::ReturnValue| {
            let key_name = args.get(0).to_rust_string_lossy(scope);
            let ciphertext = args.get(1).to_rust_string_lossy(scope);
            let nonce = args.get(2).to_rust_string_lossy(scope);

            // Extract key_version (required) and aad (optional) from options
            let (key_version, aad): (Option<u32>, Option<String>) = if args.length() > 3 && args.get(3).is_object() {
                let opts = args.get(3).to_object(scope).unwrap();

                let ver_key = v8::String::new(scope, "key_version").unwrap();
                let ver_val = opts.get(scope, ver_key.into())
                    .and_then(|v| v.int32_value(scope))
                    .map(|v| v as u32);

                let aad_key = v8::String::new(scope, "aad").unwrap();
                let aad_val = opts.get(scope, aad_key.into())
                    .and_then(|v| {
                        if v.is_undefined() || v.is_null() { None }
                        else { Some(v.to_rust_string_lossy(scope)) }
                    });

                (ver_val, aad_val)
            } else {
                (None, None)
            };

            let key_version = match key_version {
                Some(v) => v,
                None => {
                    let msg = v8::String::new(scope, "Rivers.crypto.decrypt: options.key_version is required").unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    scope.throw_exception(exc);
                    return;
                }
            };

            let result = TASK_KEYSTORE.with(|ks| {
                let ks = ks.borrow();
                match ks.as_ref() {
                    Some(ctx) => {
                        let aad_bytes = aad.as_ref().map(|a| a.as_bytes());
                        ctx.keystore.decrypt_with_key(&key_name, &ciphertext, &nonce, key_version, aad_bytes)
                            .map_err(|e| {
                                // Generic error for auth failures -- no oracle
                                match e {
                                    rivers_keystore_engine::AppKeystoreError::KeyNotFound { .. } => e.to_string(),
                                    rivers_keystore_engine::AppKeystoreError::KeyVersionNotFound { .. } => e.to_string(),
                                    _ => "decryption failed".to_string(),
                                }
                            })
                    }
                    None => Err("keystore not configured: no [[keystores]] resource declared".to_string()),
                }
            });

            match result {
                Ok(plaintext_bytes) => {
                    let plaintext = String::from_utf8_lossy(&plaintext_bytes);
                    if let Some(v8_str) = v8::String::new(scope, &plaintext) {
                        rv.set(v8_str.into());
                    }
                }
                Err(msg) => {
                    let err_msg = v8::String::new(scope, &msg).unwrap();
                    let exception = v8::Exception::error(scope, err_msg);
                    scope.throw_exception(exception);
                }
            }
        },
    )
    .ok_or_else(|| TaskError::Internal("failed to create Rivers.crypto.decrypt".into()))?;
    let decrypt_key = v8_str(scope, "decrypt")?;
    crypto_obj.set(scope, decrypt_key.into(), decrypt_fn.into());

    let crypto_key = v8_str(scope, "crypto")?;
    rivers_obj.set(scope, crypto_key.into(), crypto_obj.into());

    // ── Rivers.keystore (key metadata -- App Keystore feature) ────
    let ks_available = TASK_KEYSTORE.with(|ks| ks.borrow().is_some());
    if ks_available {
        let keystore_obj = v8::Object::new(scope);

        // Rivers.keystore.has(name) -- returns boolean
        let has_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    ks.borrow().as_ref()
                        .map(|ctx| ctx.keystore.has_key(&name))
                        .unwrap_or(false)
                });
                rv.set(v8::Boolean::new(scope, result).into());
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.has".into()))?;
        let has_key = v8_str(scope, "has")?;
        keystore_obj.set(scope, has_key.into(), has_fn.into());

        // Rivers.keystore.info(name) -- returns {name, type, version, created_at} or throws
        let info_fn = v8::Function::new(
            scope,
            |scope: &mut v8::HandleScope,
             args: v8::FunctionCallbackArguments,
             mut rv: v8::ReturnValue| {
                let name = args.get(0).to_rust_string_lossy(scope);
                let result = TASK_KEYSTORE.with(|ks| {
                    let ks = ks.borrow();
                    match ks.as_ref() {
                        Some(ctx) => ctx.keystore.key_info(&name)
                            .map_err(|e| e.to_string()),
                        None => Err("keystore not configured".to_string()),
                    }
                });

                match result {
                    Ok(info) => {
                        // Build a V8 object with the metadata
                        let obj = v8::Object::new(scope);

                        let name_key = v8::String::new(scope, "name").unwrap();
                        let name_val = v8::String::new(scope, &info.name).unwrap();
                        obj.set(scope, name_key.into(), name_val.into());

                        let type_key = v8::String::new(scope, "type").unwrap();
                        let type_val = v8::String::new(scope, &info.key_type).unwrap();
                        obj.set(scope, type_key.into(), type_val.into());

                        let ver_key = v8::String::new(scope, "version").unwrap();
                        let ver_val = v8::Integer::new(scope, info.current_version as i32);
                        obj.set(scope, ver_key.into(), ver_val.into());

                        let created_key = v8::String::new(scope, "created_at").unwrap();
                        let created_val = v8::String::new(scope, &info.created.to_rfc3339()).unwrap();
                        obj.set(scope, created_key.into(), created_val.into());

                        rv.set(obj.into());
                    }
                    Err(msg) => {
                        let err_msg = v8::String::new(scope, &msg).unwrap();
                        let exception = v8::Exception::error(scope, err_msg);
                        scope.throw_exception(exception);
                    }
                }
            },
        )
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.keystore.info".into()))?;
        let info_key = v8_str(scope, "info")?;
        keystore_obj.set(scope, info_key.into(), info_fn.into());

        let ks_key = v8_str(scope, "keystore")?;
        rivers_obj.set(scope, ks_key.into(), keystore_obj.into());
    }

    // ── Rivers.http -- real outbound HTTP via async bridge (V2) ──
    // Per spec SS10.5: only injected when allow_outbound_http = true (capability gating).
    // When not injected, `Rivers.http` is undefined in JS -- natural V8 behavior.
    let http_enabled = TASK_HTTP_ENABLED.with(|h| *h.borrow());
    if http_enabled {
        let http_obj = v8::Object::new(scope);

        let http_get_fn = v8::Function::new(scope, rivers_http_get_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.get".into()))?;
        let get_key = v8_str(scope, "get")?;
        http_obj.set(scope, get_key.into(), http_get_fn.into());

        let http_post_fn = v8::Function::new(scope, rivers_http_post_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.post".into()))?;
        let post_key = v8_str(scope, "post")?;
        http_obj.set(scope, post_key.into(), http_post_fn.into());

        let http_put_fn = v8::Function::new(scope, rivers_http_put_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.put".into()))?;
        let put_key = v8_str(scope, "put")?;
        http_obj.set(scope, put_key.into(), http_put_fn.into());

        let http_del_fn = v8::Function::new(scope, rivers_http_del_callback)
            .ok_or_else(|| TaskError::Internal("failed to create Rivers.http.del".into()))?;
        let del_key = v8_str(scope, "del")?;
        http_obj.set(scope, del_key.into(), http_del_fn.into());

        let http_key = v8_str(scope, "http")?;
        rivers_obj.set(scope, http_key.into(), http_obj.into());
    }

    // ── Rivers.__directDispatch -- typed-proxy dispatch for Direct datasources ──
    // Called only by the typed-proxy codegen (Task 29d). Handlers reach the
    // typed proxy via `ctx.datasource(name)`, not this raw entrypoint.
    let direct_dispatch_fn = v8::Function::new(
        scope,
        super::direct_dispatch::rivers_direct_dispatch_callback,
    )
    .ok_or_else(|| TaskError::Internal("failed to create __directDispatch".into()))?;
    let direct_key = v8_str(scope, "__directDispatch")?;
    rivers_obj.set(scope, direct_key.into(), direct_dispatch_fn.into());

    // ── Rivers.__brokerPublish -- broker producer dispatch (BR-2026-04-23) ──
    // Called by broker-proxy codegen; handlers reach it via
    // `ctx.datasource("<broker>").publish(msg)`. See
    // bugs/bugreport_2026-04-23.md.
    let broker_publish_fn = v8::Function::new(
        scope,
        super::broker_dispatch::rivers_broker_publish_callback,
    )
    .ok_or_else(|| TaskError::Internal("failed to create __brokerPublish".into()))?;
    let broker_key = v8_str(scope, "__brokerPublish")?;
    rivers_obj.set(scope, broker_key.into(), broker_publish_fn.into());

    // ── Rivers.db (imperative transaction API — spec §6 alternate form) ──
    // begin/commit/rollback/batch mirror the ctx.transaction() RAII form
    // but give handlers explicit control over the transaction boundary.
    // ctx.dataview() calls inside an open Rivers.db transaction are routed
    // through the held connection (same TransactionMap as ctx.transaction).
    let db_obj = v8::Object::new(scope);

    let db_begin_fn = v8::Function::new(scope, db_begin_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.begin".into()))?;
    let db_begin_key = v8_str(scope, "begin")?;
    db_obj.set(scope, db_begin_key.into(), db_begin_fn.into());

    let db_commit_fn = v8::Function::new(scope, db_commit_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.commit".into()))?;
    let db_commit_key = v8_str(scope, "commit")?;
    db_obj.set(scope, db_commit_key.into(), db_commit_fn.into());

    let db_rollback_fn = v8::Function::new(scope, db_rollback_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.rollback".into()))?;
    let db_rollback_key = v8_str(scope, "rollback")?;
    db_obj.set(scope, db_rollback_key.into(), db_rollback_fn.into());

    let db_batch_fn = v8::Function::new(scope, db_batch_callback)
        .ok_or_else(|| TaskError::Internal("failed to create Rivers.db.batch".into()))?;
    let db_batch_key = v8_str(scope, "batch")?;
    db_obj.set(scope, db_batch_key.into(), db_batch_fn.into());

    let db_key = v8_str(scope, "db")?;
    rivers_obj.set(scope, db_key.into(), db_obj.into());

    // ── Rivers.env -- task environment variables (V2) ─────────────
    let env_map = TASK_ENV.with(|e| e.borrow().clone()).unwrap_or_default();
    let env_json = serde_json::to_value(&env_map)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let env_val = json_to_v8(scope, &env_json)?;
    let env_key = v8_str(scope, "env")?;
    rivers_obj.set(scope, env_key.into(), env_val);

    // Set Rivers on global
    global.set(scope, rivers_key.into(), rivers_obj.into());

    // ── console.{log,warn,error} via JS eval ─────────────────────
    // X1.2: console delegates forward structured fields when the last argument is an object.
    let js_extras = r#"
        // console.{log,warn,error} -> Rivers.log (P2.3)
        var console = {
            log: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.info(args.join(' '), last);
            },
            warn: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.warn(args.join(' '), last);
            },
            error: function() {
                var args = Array.from(arguments);
                var last = args.length > 1 && typeof args[args.length - 1] === 'object' ? args.pop() : undefined;
                Rivers.log.error(args.join(' '), last);
            },
        };
    "#;
    let js_src = v8::String::new(scope, js_extras)
        .ok_or_else(|| TaskError::Internal("failed to create extras source string".into()))?;
    let script = v8::Script::compile(scope, js_src, None)
        .ok_or_else(|| TaskError::Internal("failed to compile Rivers extras".into()))?;
    script
        .run(scope)
        .ok_or_else(|| TaskError::Internal("failed to run Rivers extras".into()))?;

    Ok(())
}

// ── Rivers.db callback helpers ────────────────────────────────────────────

fn db_throw(scope: &mut v8::HandleScope, message: &str) {
    if let Some(msg) = v8::String::new(scope, message) {
        let exc = v8::Exception::error(scope, msg);
        scope.throw_exception(exc);
    }
}

/// `Rivers.db.begin(datasource)` — begin an explicit transaction.
///
/// Spec §6: stores the connection in `TASK_TRANSACTION` so subsequent
/// `ctx.dataview()` calls route through the held connection.
fn db_begin_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    use std::sync::Arc;
    use rivers_runtime::rivers_driver_sdk::DriverError;

    let ds_name = args.get(0).to_rust_string_lossy(scope);
    if ds_name.is_empty() {
        db_throw(scope, "Rivers.db.begin: datasource name is required");
        return;
    }

    // Reject nested transactions (spec §6.2)
    let already_active = TASK_TRANSACTION.with(|t| t.borrow().is_some());
    if already_active {
        db_throw(scope, "TransactionError: nested transactions not supported");
        return;
    }

    // Resolve datasource config
    let resolved = TASK_DS_CONFIGS.with(|c| c.borrow().get(&ds_name).cloned());
    let resolved = match resolved {
        Some(r) => r,
        None => {
            db_throw(scope, &format!("TransactionError: datasource \"{ds_name}\" not found in task config"));
            return;
        }
    };

    // Get driver factory
    let factory = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone());
    let factory = match factory {
        Some(f) => f,
        None => {
            db_throw(scope, "TransactionError: driver factory not available");
            return;
        }
    };

    // Get tokio runtime
    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            db_throw(scope, &format!("TransactionError: {e}"));
            return;
        }
    };

    // Connect and begin transaction
    let txn_map = Arc::new(crate::transaction::TransactionMap::new());
    let txn_map_ref = txn_map.clone();
    let ds_for_begin = ds_name.clone();
    let begin_outcome: Result<(), DriverError> = rt.block_on(async move {
        let conn = factory.connect(&resolved.driver_name, &resolved.params).await?;
        txn_map_ref.begin(&ds_for_begin, conn).await
    });

    match begin_outcome {
        Ok(()) => {
            TASK_TRANSACTION.with(|t| {
                *t.borrow_mut() = Some(TaskTransactionState {
                    map: txn_map,
                    datasource: ds_name,
                });
            });
        }
        Err(e) => {
            let msg = match &e {
                DriverError::Unsupported(_) => {
                    format!("TransactionError: datasource \"{ds_name}\" does not support transactions")
                }
                _ => format!("TransactionError: begin failed: {e}"),
            };
            db_throw(scope, &msg);
        }
    }
}

/// `Rivers.db.commit(datasource)` — commit the active explicit transaction.
fn db_commit_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);

    // Take the transaction state
    let state = TASK_TRANSACTION.with(|t| t.borrow_mut().take());
    let state = match state {
        Some(s) => s,
        None => {
            db_throw(scope, "TransactionError: no active transaction to commit");
            return;
        }
    };

    // Validate datasource matches (if caller passed one)
    if !ds_name.is_empty() && state.datasource != ds_name {
        let msg = format!(
            "TransactionError: active transaction is on \"{}\", not \"{ds_name}\"",
            state.datasource
        );
        // Restore state before throwing
        TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
        db_throw(scope, &msg);
        return;
    }

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            // Restore state so caller can rollback
            TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
            db_throw(scope, &format!("TransactionError: {e}"));
            return;
        }
    };

    let ds = state.datasource.clone();
    let commit_res = rt.block_on(state.map.commit(&ds));
    // Connection drops → pool slot released

    if let Err(e) = commit_res {
        let driver_msg = format!("{e}");
        // Spec §6 + financial-correctness: stash for TaskError::TransactionCommitFailed upgrade.
        TASK_COMMIT_FAILED.with(|c| {
            *c.borrow_mut() = Some((ds.clone(), driver_msg.clone()));
        });
        db_throw(
            scope,
            &format!("TransactionError: commit failed on datasource '{ds}': {driver_msg}"),
        );
    }
}

/// `Rivers.db.rollback(datasource)` — rollback the active explicit transaction.
fn db_rollback_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let ds_name = args.get(0).to_rust_string_lossy(scope);

    let state = TASK_TRANSACTION.with(|t| t.borrow_mut().take());
    let state = match state {
        Some(s) => s,
        None => {
            // No active transaction — silently succeed (idempotent rollback).
            return;
        }
    };

    if !ds_name.is_empty() && state.datasource != ds_name {
        let msg = format!(
            "TransactionError: active transaction is on \"{}\", not \"{ds_name}\"",
            state.datasource
        );
        TASK_TRANSACTION.with(|t| *t.borrow_mut() = Some(state));
        db_throw(scope, &msg);
        return;
    }

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(_) => return, // best-effort
    };

    let ds = state.datasource.clone();
    if let Err(e) = rt.block_on(state.map.rollback(&ds)) {
        tracing::warn!(
            target: "rivers.handler",
            datasource = %ds,
            error = %e,
            "Rivers.db.rollback: rollback failed"
        );
    }
}

/// `Rivers.db.batch(dataview, [...params])` — execute a DataView once per
/// param entry and return an array of results.
///
/// Routes each execution through the active `TASK_TRANSACTION` connection
/// (if any) exactly as `ctx.dataview()` does — the TransactionMap
/// take/return protocol is used per-iteration so the connection is
/// exclusively held for each call.
fn db_batch_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    use rivers_runtime::rivers_driver_sdk::types::QueryValue;

    let dv_name = args.get(0).to_rust_string_lossy(scope);

    // Parse the params array from arg[1]
    let params_val = args.get(1);
    let params_json: Vec<serde_json::Map<String, serde_json::Value>> =
        if params_val.is_array() || params_val.is_object() {
            if let Some(json_str) = v8::json::stringify(scope, params_val) {
                let s = json_str.to_rust_string_lossy(scope);
                match serde_json::from_str::<serde_json::Value>(&s) {
                    Ok(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| {
                            if let serde_json::Value::Object(m) = v { Some(m) } else { None }
                        })
                        .collect(),
                    _ => vec![],
                }
            } else {
                vec![]
            }
        } else {
            vec![]
        };

    let executor = TASK_DV_EXECUTOR.with(|e| e.borrow().clone());
    let executor = match executor {
        Some(e) => e,
        None => {
            db_throw(scope, &format!("Rivers.db.batch: no DataViewExecutor available"));
            return;
        }
    };

    // Namespace the dataview name (same logic as ctx_dataview_callback)
    let namespaced = TASK_DV_NAMESPACE.with(|n| {
        n.borrow().as_ref()
            .filter(|ns| !ns.is_empty() && !dv_name.contains(':'))
            .map(|ns| format!("{ns}:{dv_name}"))
            .unwrap_or_else(|| dv_name.clone())
    });

    let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();

    let rt = match get_rt_handle() {
        Ok(r) => r,
        Err(e) => {
            db_throw(scope, &format!("Rivers.db.batch: {e}"));
            return;
        }
    };

    // Execute each param set, routing through the active transaction if present.
    let results: Vec<serde_json::Value> = {
        let mut out = Vec::with_capacity(params_json.len());
        for entry in params_json {
            let qp: std::collections::HashMap<String, QueryValue> = entry
                .into_iter()
                .map(|(k, v)| (k, super::datasource::json_to_query_value(v)))
                .collect();

            let txn_state: Option<(std::sync::Arc<crate::transaction::TransactionMap>, String)> =
                TASK_TRANSACTION.with(|t| {
                    t.borrow().as_ref().map(|s| (s.map.clone(), s.datasource.clone()))
                });

            let exec_res = rt.block_on(async {
                if let Some((map, ds)) = txn_state {
                    if let Some(mut conn) = map.take_connection(&ds).await {
                        let res = executor
                            .execute(&namespaced, qp, "GET", &trace_id, Some(&mut conn))
                            .await;
                        map.return_connection(&ds, conn).await;
                        res
                    } else {
                        Err(rivers_runtime::dataview_engine::DataViewError::Driver(
                            format!("transaction connection for '{ds}' unavailable"),
                        ))
                    }
                } else {
                    executor.execute(&namespaced, qp, "GET", &trace_id, None).await
                }
            });

            match exec_res {
                Ok(response) => {
                    out.push(serde_json::json!({
                        "rows": response.query_result.rows,
                        "affected_rows": response.query_result.affected_rows,
                        "last_insert_id": response.query_result.last_insert_id,
                    }));
                }
                Err(e) => {
                    // On any execution error, throw JS exception immediately
                    let msg = format!("Rivers.db.batch('{}') error: {e}", dv_name);
                    db_throw(scope, &msg);
                    return;
                }
            }
        }
        out
    };

    // Convert results array to V8
    let json_str = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
    if let Some(v8_str) = v8::String::new(scope, &json_str) {
        if let Some(parsed) = v8::json::parse(scope, v8_str.into()) {
            rv.set(parsed);
        }
    }
}
