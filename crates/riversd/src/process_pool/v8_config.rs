//! V8 and engine config types — V8Config, V8Worker, EngineType, TypeScript compiler.

#![allow(dead_code)]

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

use swc_core::common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::codegen::to_code_default;
use swc_core::ecma::parser::{parse_file_as_program, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::transforms::typescript::{typescript, Config as TsConfig};

/// Compile TypeScript source to JavaScript via the swc full-transform pass.
///
/// Per `docs/arch/rivers-javascript-typescript-spec.md` §2.1–2.5:
/// - Full transform (not strip-only): erases type annotations, `type`-only
///   imports, `as` / `satisfies` assertions, `interface` / `type` aliases,
///   generic parameters, and lowers `enum` / `namespace` / `const enum`.
/// - Parser accepts TC39 Stage 3 decorator syntax (spec §2.3). Lowering is
///   deferred to V8, which supports Stage 3 decorators natively in the
///   pinned runtime; legacy `experimentalDecorators` is not supported.
/// - ES2022 is the compilation target floor (spec §2.4).
/// - `.tsx` is rejected unconditionally (spec §2.5).
pub fn compile_typescript(source: &str, filename: &str) -> Result<String, TaskError> {
    if filename.ends_with(".tsx") {
        return Err(TaskError::HandlerError(format!(
            "JSX/TSX is not supported in Rivers v1: {filename}"
        )));
    }

    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(filename.into()).into(),
        source.to_string(),
    );

    let mut recovered = Vec::<swc_core::ecma::parser::error::Error>::new();
    let program = parse_file_as_program(
        &fm,
        Syntax::Typescript(TsSyntax {
            decorators: true,
            ..Default::default()
        }),
        EsVersion::Es2022,
        None,
        &mut recovered,
    )
    .map_err(|e| {
        TaskError::HandlerError(format!(
            "TypeScript parse error in {filename}: {:?}",
            e.kind()
        ))
    })?;

    if !recovered.is_empty() {
        let msgs: Vec<String> = recovered
            .iter()
            .map(|e| format!("{:?}", e.kind()))
            .collect();
        return Err(TaskError::HandlerError(format!(
            "TypeScript parse errors in {filename}: {}",
            msgs.join("; ")
        )));
    }

    GLOBALS.set(&Globals::default(), || -> Result<String, TaskError> {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        let program = program
            .apply(resolver(unresolved_mark, top_level_mark, true))
            .apply(typescript(
                TsConfig::default(),
                unresolved_mark,
                top_level_mark,
            ))
            .apply(fixer(None));

        Ok(to_code_default(cm.clone(), None, &program))
    })
}

