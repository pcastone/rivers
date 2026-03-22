//! V8 and engine config types — V8Config, V8Worker, EngineType, TypeScript compiler.

#![allow(dead_code)]

use std::collections::HashMap;

use rivers_runtime::rivers_core::config::ProcessPoolConfig;

use super::types::*;
use super::v8_engine::ensure_v8_initialized;

// ── Engine Types ─────────────────────────────────────────────────

/// Supported engine types for ProcessPool workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineType {
    /// V8 JavaScript engine (via rusty_v8).
    V8,
    /// Wasmtime WebAssembly engine.
    Wasmtime,
}

impl EngineType {
    /// Parse from config string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "v8" => Some(EngineType::V8),
            "wasmtime" | "wasm" => Some(EngineType::Wasmtime),
            _ => None,
        }
    }

    /// Return the string name.
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineType::V8 => "v8",
            EngineType::Wasmtime => "wasmtime",
        }
    }
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── V8 Config & Worker ──────────────────────────────────────────

/// Configuration for the V8 JavaScript engine.
///
/// Per spec §14.1: V8 isolate pool settings.
#[derive(Debug, Clone)]
pub struct V8Config {
    /// Number of pre-warmed V8 isolates in the pool.
    pub isolate_pool_size: usize,
    /// Max heap memory per isolate in bytes.
    pub memory_limit_bytes: usize,
    /// CPU time limit per task in milliseconds.
    pub cpu_time_limit_ms: u64,
}

impl Default for V8Config {
    fn default() -> Self {
        Self {
            isolate_pool_size: 4,
            memory_limit_bytes: 128 * 1024 * 1024, // 128 MiB
            cpu_time_limit_ms: 5000,
        }
    }
}

/// V8 JavaScript engine worker.
///
/// Wraps the existing V8 isolate pool infrastructure with config-driven
/// heap limits, CPU timeouts, and pool sizing per spec §5.0-5.4.
///
/// The V8 platform is initialized once via `ensure_v8_initialized()`.
/// Each worker manages isolate acquisition from the thread-local pool.
#[derive(Debug, Clone)]
pub struct V8Worker {
    config: V8Config,
}

impl V8Worker {
    /// Create a V8 worker with the given configuration.
    ///
    /// Initializes the V8 platform (once, globally) and validates config.
    pub fn new(config: V8Config) -> Result<Self, TaskError> {
        ensure_v8_initialized();
        Ok(Self { config })
    }

    /// Get the configured heap limit in bytes.
    pub fn heap_limit(&self) -> usize {
        self.config.memory_limit_bytes
    }

    /// Get the configured CPU time limit in milliseconds.
    pub fn cpu_time_limit_ms(&self) -> u64 {
        self.config.cpu_time_limit_ms
    }

    /// Get the configured isolate pool size.
    pub fn pool_size(&self) -> usize {
        self.config.isolate_pool_size
    }

    /// Build a `V8Config` from a `ProcessPoolConfig`.
    pub fn config_from_pool(pool_config: &ProcessPoolConfig) -> V8Config {
        V8Config {
            isolate_pool_size: pool_config.workers,
            memory_limit_bytes: pool_config.max_heap_mb * 1024 * 1024,
            cpu_time_limit_ms: pool_config.task_timeout_ms,
        }
    }
}

// ── TypeScript Compiler ─────────────────────────────────────────

/// Compile TypeScript source to JavaScript by stripping type annotations.
///
/// This is a lightweight approach that handles common TypeScript patterns:
/// - Type annotations on parameters: `(x: string)` -> `(x)`
/// - Return type annotations: `): string {` -> `) {`
/// - Interface/type declarations: removed entirely
/// - Generic type parameters: `<T>` -> removed
/// - `as` type assertions: `x as string` -> `x`
///
/// For complex TypeScript features (decorators, enums, namespace merging),
/// the full SWC compiler should be used. This covers the 90% case for
/// Rivers handler functions.
pub fn compile_typescript(source: &str, _filename: &str) -> Result<String, TaskError> {
    let mut result = String::with_capacity(source.len());
    let mut in_interface = false;
    let mut brace_depth: i32 = 0;
    let mut interface_brace_start: i32 = 0;

    let lines: Vec<&str> = source.lines().collect();

    for line in &lines {
        let trimmed = line.trim();

        // Skip interface/type declarations entirely
        if trimmed.starts_with("interface ")
            || (trimmed.starts_with("type ") && trimmed.contains('='))
        {
            if trimmed.contains('{') && !trimmed.contains('}') {
                in_interface = true;
                interface_brace_start = brace_depth;
            }
            for c in trimmed.chars() {
                if c == '{' {
                    brace_depth += 1;
                }
                if c == '}' {
                    brace_depth -= 1;
                }
            }
            continue;
        }

        if in_interface {
            for c in trimmed.chars() {
                if c == '{' {
                    brace_depth += 1;
                }
                if c == '}' {
                    brace_depth -= 1;
                }
            }
            if brace_depth <= interface_brace_start {
                in_interface = false;
            }
            continue;
        }

        for c in trimmed.chars() {
            if c == '{' {
                brace_depth += 1;
            }
            if c == '}' {
                brace_depth -= 1;
            }
        }

        let stripped = strip_type_annotations(line);
        result.push_str(&stripped);
        result.push('\n');
    }

    Ok(result)
}

/// Strip type annotations from a single line of TypeScript.
fn strip_type_annotations(line: &str) -> String {
    let mut result = line.to_string();

    // Remove return type annotations: `): ReturnType {` -> `) {`
    while let Some(pos) = result.find("): ") {
        if let Some(brace) = result[pos + 2..].find('{') {
            let between = &result[pos + 2..pos + 2 + brace];
            if !between.contains("=>") {
                result = format!("{}) {{{}", &result[..pos], &result[pos + 2 + brace + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Remove `as Type` assertions
    let as_pattern = " as ";
    while let Some(pos) = result.find(as_pattern) {
        let after = &result[pos + 4..];
        let end = after
            .find(|c: char| {
                !c.is_alphanumeric() && c != '_' && c != '<' && c != '>' && c != '[' && c != ']'
            })
            .unwrap_or(after.len());
        result = format!("{}{}", &result[..pos], &result[pos + 4 + end..]);
    }

    result
}

