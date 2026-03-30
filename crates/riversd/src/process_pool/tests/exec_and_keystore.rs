//! ExecDriver subprocess tests + keystore encrypt/decrypt/AAD/tamper tests.

use super::*;
use super::helpers::make_js_task;

// ── ExecDriver Application-Level Tests ──────────────────────────
//
// Tests the full flow: JS handler -> ctx.datasource().build() ->
// DriverFactory -> ExecDriver.connect() -> ExecConnection.execute()
// -> script execution -> JSON result back to JS.

#[cfg(unix)]
fn make_exec_script(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn sha256_file(path: &std::path::Path) -> String {
    use sha2::{Sha256, Digest};
    let bytes = std::fs::read(path).unwrap();
    hex::encode(Sha256::digest(&bytes))
}

#[cfg(unix)]
fn make_exec_params(
    dir: &std::path::Path,
    commands: &[(&str, &std::path::Path, &str, &str)],
) -> rivers_runtime::rivers_driver_sdk::ConnectionParams {
    let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
    let mut options = HashMap::new();
    options.insert("run_as_user".into(), user);
    options.insert("working_directory".into(), dir.to_str().unwrap().into());

    for (name, path, sha256, input_mode) in commands {
        options.insert(format!("commands.{name}.path"), path.to_str().unwrap().into());
        options.insert(format!("commands.{name}.sha256"), sha256.to_string());
        options.insert(format!("commands.{name}.input_mode"), input_mode.to_string());
    }

    rivers_runtime::rivers_driver_sdk::ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn exec_driver_base_test_no_params() {
    // Base test: script with no parameters, just returns a fixed JSON response
    use rivers_runtime::rivers_core::DriverFactory;

    let dir = tempfile::tempdir().unwrap();
    let script = make_exec_script(dir.path(), "hello.sh",
        "#!/bin/sh\necho '{\"status\":\"ok\",\"message\":\"hello from exec driver\"}'\n"
    );
    let hash = sha256_file(&script);
    let params = make_exec_params(dir.path(), &[("hello", &script, &hash, "stdin")]);

    let mut factory = DriverFactory::new();
    factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("ops".into(), DatasourceToken("ops".into()))
        .datasource_config("ops".into(), ResolvedDatasource {
            driver_name: "rivers-exec".into(),
            params,
        })
        .driver_factory(std::sync::Arc::new(factory))
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                var result = ctx.datasource("ops")
                    .fromQuery("query", { command: "hello" })
                    .build();
                // result.rows[0].result contains the script's JSON output
                var output = result.rows[0].result;
                return {
                    status: output.status,
                    message: output.message,
                    has_rows: result.rows.length === 1
                };
            }"#
        }))
        .trace_id("exec-base".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["status"], "ok");
    assert_eq!(result.value["message"], "hello from exec driver");
    assert_eq!(result.value["has_rows"], true);
}

#[cfg(unix)]
#[tokio::test]
async fn exec_driver_parameter_test_stdin() {
    // Parameter test: view sends parameters to the script via stdin
    // Script reads JSON from stdin, processes it, returns enriched JSON
    use rivers_runtime::rivers_core::DriverFactory;

    let dir = tempfile::tempdir().unwrap();
    let script = make_exec_script(dir.path(), "process.sh",
        r#"#!/bin/sh
# Read JSON from stdin, extract fields, return processed result
INPUT=$(cat)
# Use simple shell to echo back with added field
echo "{\"received\":$INPUT,\"processed\":true}"
"#
    );
    let hash = sha256_file(&script);
    let params = make_exec_params(dir.path(), &[("process", &script, &hash, "stdin")]);

    let mut factory = DriverFactory::new();
    factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("tools".into(), DatasourceToken("tools".into()))
        .datasource_config("tools".into(), ResolvedDatasource {
            driver_name: "rivers-exec".into(),
            params,
        })
        .driver_factory(std::sync::Arc::new(factory))
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                // Handler sends parameters to the exec command
                var result = ctx.datasource("tools")
                    .fromQuery("query", {
                        command: "process",
                        args: { cidr: "10.0.1.0/24", ports: [22, 80, 443] }
                    })
                    .build();

                var output = result.rows[0].result;
                return {
                    received_cidr: output.received.cidr,
                    received_ports: output.received.ports,
                    processed: output.processed
                };
            }"#
        }))
        .trace_id("exec-params".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["received_cidr"], "10.0.1.0/24");
    assert_eq!(result.value["received_ports"][0], 22);
    assert_eq!(result.value["received_ports"][1], 80);
    assert_eq!(result.value["received_ports"][2], 443);
    assert_eq!(result.value["processed"], true);
}

#[cfg(unix)]
#[tokio::test]
async fn exec_driver_parameter_test_args_mode() {
    // Parameter test with args mode: view sends parameters via CLI args
    use rivers_runtime::rivers_core::DriverFactory;

    let dir = tempfile::tempdir().unwrap();
    let script = make_exec_script(dir.path(), "lookup.sh",
        r#"#!/bin/sh
# Receives arguments: $1=domain, $2=record_type
echo "{\"domain\":\"$1\",\"type\":\"$2\",\"resolved\":true}"
"#
    );
    let hash = sha256_file(&script);

    // Build params with args_template for this command
    let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
    let mut options = HashMap::new();
    options.insert("run_as_user".into(), user);
    options.insert("working_directory".into(), dir.path().to_str().unwrap().into());
    options.insert("commands.dns_lookup.path".into(), script.to_str().unwrap().into());
    options.insert("commands.dns_lookup.sha256".into(), hash);
    options.insert("commands.dns_lookup.input_mode".into(), "args".into());
    // args_template uses indexed keys: .0, .1, etc.
    options.insert("commands.dns_lookup.args_template.0".into(), "{domain}".into());
    options.insert("commands.dns_lookup.args_template.1".into(), "{record_type}".into());

    let params = rivers_runtime::rivers_driver_sdk::ConnectionParams {
        host: String::new(), port: 0, database: String::new(),
        username: String::new(), password: String::new(), options,
    };

    let mut factory = DriverFactory::new();
    factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("dns".into(), DatasourceToken("dns".into()))
        .datasource_config("dns".into(), ResolvedDatasource {
            driver_name: "rivers-exec".into(),
            params,
        })
        .driver_factory(std::sync::Arc::new(factory))
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                var result = ctx.datasource("dns")
                    .fromQuery("query", {
                        command: "dns_lookup",
                        args: { domain: "example.com", record_type: "A" }
                    })
                    .build();

                var output = result.rows[0].result;
                return {
                    domain: output.domain,
                    type: output.type,
                    resolved: output.resolved
                };
            }"#
        }))
        .trace_id("exec-args".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["domain"], "example.com");
    assert_eq!(result.value["type"], "A");
    assert_eq!(result.value["resolved"], true);
}

#[cfg(unix)]
#[tokio::test]
async fn exec_driver_error_propagation() {
    // Test that script errors propagate correctly back to JS handler
    use rivers_runtime::rivers_core::DriverFactory;

    let dir = tempfile::tempdir().unwrap();
    let script = make_exec_script(dir.path(), "fail.sh",
        "#!/bin/sh\necho 'script error: invalid input' >&2\nexit 1\n"
    );
    let hash = sha256_file(&script);
    let params = make_exec_params(dir.path(), &[("failing", &script, &hash, "stdin")]);

    let mut factory = DriverFactory::new();
    factory.register_database_driver(std::sync::Arc::new(rivers_plugin_exec::ExecDriver));

    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .datasource("ops".into(), DatasourceToken("ops".into()))
        .datasource_config("ops".into(), ResolvedDatasource {
            driver_name: "rivers-exec".into(),
            params,
        })
        .driver_factory(std::sync::Arc::new(factory))
        .args(serde_json::json!({
            "_source": r#"function handler(ctx) {
                try {
                    ctx.datasource("ops")
                        .fromQuery("query", { command: "failing" })
                        .build();
                    return { threw: false };
                } catch (e) {
                    return {
                        threw: true,
                        message: e.message,
                        has_stderr: e.message.indexOf("script error") !== -1
                    };
                }
            }"#
        }))
        .trace_id("exec-error".into())
        .build()
        .unwrap();

    let result = execute_js_task(ctx, 10000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    assert_eq!(result.value["has_stderr"], true);
}

// ── Application Keystore Tests ──────────────────────────────────
//
// These tests exercise the full application-level keystore feature:
// Rivers.keystore.has/info and Rivers.crypto.encrypt/decrypt running
// through the V8 engine with a real keystore via the shared resolver.

/// Helper: create a test keystore with a named key.
fn make_test_keystore(key_name: &str) -> std::sync::Arc<rivers_keystore_engine::AppKeystore> {
    std::sync::Arc::new(rivers_keystore_engine::create_test_keystore(key_name))
}

fn make_ks_task(source: &str, function: &str, ks: std::sync::Arc<rivers_keystore_engine::AppKeystore>) -> TaskContext {
    TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: "inline".into(),
            function: function.into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({ "_source": source }))
        .trace_id("ks-test".into())
        .app_id("test-app".into())
        .keystore(ks)
        .build()
        .unwrap()
}

#[tokio::test]
async fn keystore_has_returns_true_for_existing_key() {
    let ks = make_test_keystore("credential-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            return {
                exists: Rivers.keystore.has("credential-key"),
                missing: Rivers.keystore.has("nonexistent")
            };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["exists"], true);
    assert_eq!(result.value["missing"], false);
}

#[tokio::test]
async fn keystore_info_returns_metadata() {
    let ks = make_test_keystore("test-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var info = Rivers.keystore.info("test-key");
            return {
                name: info.name,
                type: info.type,
                version: info.version,
                has_created: typeof info.created_at === "string"
            };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["name"], "test-key");
    assert_eq!(result.value["type"], "aes-256");
    assert_eq!(result.value["version"], 1);
    assert_eq!(result.value["has_created"], true);
}

#[tokio::test]
async fn keystore_info_throws_for_missing_key() {
    let ks = make_test_keystore("real-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            try {
                Rivers.keystore.info("nonexistent");
                return { threw: false };
            } catch (e) {
                return { threw: true, message: e.message };
            }
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    assert!(result.value["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn crypto_encrypt_decrypt_round_trip() {
    let ks = make_test_keystore("secret-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var enc = Rivers.crypto.encrypt("secret-key", "hello world");

            // Verify encrypt result shape
            if (typeof enc.ciphertext !== "string") return { error: "no ciphertext" };
            if (typeof enc.nonce !== "string") return { error: "no nonce" };
            if (typeof enc.key_version !== "number") return { error: "no key_version" };

            // Decrypt and verify
            var dec = Rivers.crypto.decrypt("secret-key", enc.ciphertext, enc.nonce, {
                key_version: enc.key_version
            });

            return {
                plaintext: dec,
                key_version: enc.key_version,
                ciphertext_length: enc.ciphertext.length
            };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["plaintext"], "hello world");
    assert_eq!(result.value["key_version"], 1);
    assert!(result.value["ciphertext_length"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn crypto_encrypt_with_aad() {
    let ks = make_test_keystore("aad-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var enc = Rivers.crypto.encrypt("aad-key", "secret data", { aad: "device-123" });

            // Decrypt with matching AAD
            var dec = Rivers.crypto.decrypt("aad-key", enc.ciphertext, enc.nonce, {
                key_version: enc.key_version,
                aad: "device-123"
            });

            // Decrypt with wrong AAD should fail
            var wrongAad = false;
            try {
                Rivers.crypto.decrypt("aad-key", enc.ciphertext, enc.nonce, {
                    key_version: enc.key_version,
                    aad: "wrong-device"
                });
            } catch (e) {
                wrongAad = true;
            }

            return { plaintext: dec, wrong_aad_threw: wrongAad };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["plaintext"], "secret data");
    assert_eq!(result.value["wrong_aad_threw"], true);
}

#[tokio::test]
async fn crypto_encrypt_nonexistent_key_throws() {
    let ks = make_test_keystore("real-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            try {
                Rivers.crypto.encrypt("nonexistent", "data");
                return { threw: false };
            } catch (e) {
                return { threw: true, message: e.message };
            }
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    assert!(result.value["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn crypto_decrypt_tampered_ciphertext_throws_generic_error() {
    let ks = make_test_keystore("tamper-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var enc = Rivers.crypto.encrypt("tamper-key", "sensitive");

            // Tamper with ciphertext
            var tampered = "AAAA" + enc.ciphertext.substring(4);

            try {
                Rivers.crypto.decrypt("tamper-key", tampered, enc.nonce, {
                    key_version: enc.key_version
                });
                return { threw: false };
            } catch (e) {
                return {
                    threw: true,
                    is_generic: e.message === "decryption failed"
                };
            }
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    assert_eq!(result.value["is_generic"], true, "error should be generic, not leak details");
}

#[tokio::test]
async fn crypto_decrypt_requires_key_version() {
    let ks = make_test_keystore("ver-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var enc = Rivers.crypto.encrypt("ver-key", "data");

            // Decrypt without options should throw (no 4th argument)
            try {
                Rivers.crypto.decrypt("ver-key", enc.ciphertext, enc.nonce);
                return { threw: false };
            } catch (e) {
                return { threw: true, message: e.message };
            }
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["threw"], true);
    assert!(result.value["message"].as_str().unwrap().contains("key_version"));
}

#[tokio::test]
async fn crypto_nonce_uniqueness() {
    let ks = make_test_keystore("nonce-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            var enc1 = Rivers.crypto.encrypt("nonce-key", "same data");
            var enc2 = Rivers.crypto.encrypt("nonce-key", "same data");
            return {
                nonces_differ: enc1.nonce !== enc2.nonce,
                ciphertexts_differ: enc1.ciphertext !== enc2.ciphertext
            };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["nonces_differ"], true);
    assert_eq!(result.value["ciphertexts_differ"], true);
}

#[tokio::test]
async fn keystore_not_available_when_no_keystore() {
    // When no keystore on TaskContext, Rivers.keystore should be undefined
    let ctx = make_js_task(
        r#"function handler(ctx) {
            return {
                has_keystore: typeof Rivers.keystore !== "undefined"
            };
        }"#,
        "handler",
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["has_keystore"], false);
}

#[tokio::test]
async fn full_credential_store_workflow() {
    let ks = make_test_keystore("credential-key");
    let ctx = make_ks_task(
        r#"function handler(ctx) {
            // Simulate the Network Inventory use case from the spec

            // Step 1: Check key exists
            if (!Rivers.keystore.has("credential-key")) {
                return { error: "key not found" };
            }

            // Step 2: Get key metadata
            var meta = Rivers.keystore.info("credential-key");
            if (meta.type !== "aes-256") {
                return { error: "wrong key type: " + meta.type };
            }

            // Step 3: Encrypt a credential (simulating user submitting a password)
            var password = "super-secret-switch-password-123!";
            var enc = Rivers.crypto.encrypt("credential-key", password);

            // Step 4: Store would go to database -- we just hold in memory
            var stored = {
                encrypted_pass: enc.ciphertext,
                pass_nonce: enc.nonce,
                pass_key_ver: enc.key_version
            };

            // Step 5: Retrieve and decrypt (simulating automation fetching credential)
            var decrypted = Rivers.crypto.decrypt(
                "credential-key",
                stored.encrypted_pass,
                stored.pass_nonce,
                { key_version: stored.pass_key_ver }
            );

            // Step 6: Verify round-trip
            return {
                success: decrypted === password,
                key_version: stored.pass_key_ver,
                key_type: meta.type,
                key_name: meta.name
            };
        }"#,
        "handler",
        ks,
    );
    let result = execute_js_task(ctx, 5000, 0, DEFAULT_HEAP_LIMIT, 0.8, None).await.unwrap();
    assert_eq!(result.value["success"], true);
    assert_eq!(result.value["key_version"], 1);
    assert_eq!(result.value["key_type"], "aes-256");
    assert_eq!(result.value["key_name"], "credential-key");
}
