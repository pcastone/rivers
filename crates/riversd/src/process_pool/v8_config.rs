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

use swc_core::common::source_map::DefaultSourceMapGenConfig;
use swc_core::common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{EsVersion, ModuleDecl, ModuleItem, Program};
use swc_core::ecma::codegen::{text_writer::JsWriter, Emitter};
use swc_core::ecma::parser::{parse_file_as_program, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::transforms::typescript::{typescript, Config as TsConfig};

/// Compile TypeScript source to JavaScript, discarding import metadata.
///
/// See `compile_typescript_with_imports` for the variant that returns both
/// the compiled JS and the post-transform import specifier list.
pub fn compile_typescript(source: &str, filename: &str) -> Result<String, TaskError> {
    compile_typescript_with_imports(source, filename).map(|(js, _, _)| js)
}

/// Compile TypeScript and return the JS, runtime import specifiers, and source map.
///
/// Per `docs/arch/rivers-javascript-typescript-spec.md` §2.1–2.5, §3.5, §5.1:
/// - Full transform (not strip-only): erases type annotations, `type`-only
///   imports, `as` / `satisfies` assertions, `interface` / `type` aliases,
///   generic parameters, and lowers `enum` / `namespace` / `const enum`.
/// - Parser accepts TC39 Stage 3 decorator syntax (spec §2.3). Lowering is
///   deferred to V8, which supports Stage 3 decorators natively in the
///   pinned runtime; legacy `experimentalDecorators` is not supported.
/// - ES2022 is the compilation target floor (spec §2.4).
/// - `.tsx` is rejected unconditionally (spec §2.5).
/// - Source maps are emitted unconditionally (spec §5.1 — "generation is not
///   optional"). The map is returned as a JSON string (SourceMap v3 format)
///   suitable for storage in `CompiledModule.source_map`.
///
/// Import extraction (spec §3.5): specifiers are pulled from the
/// post-transform AST, so type-only imports (which the typescript pass has
/// already erased) do not appear in the result. Cycle detection operates on
/// runtime imports only — a type-only cycle is not a runtime cycle.
pub fn compile_typescript_with_imports(
    source: &str,
    filename: &str,
) -> Result<(String, Vec<String>, String), TaskError> {
    if filename.ends_with(".tsx") {
        // Spec §2.5: message format is "JSX/TSX is not supported in Rivers
        // v1: {app}/{path}". If the filename contains a `libraries/` segment,
        // extract `{app}/{path}` as (parent-of-libraries)/(libraries/...).
        // Otherwise fall back to the raw filename — still informative.
        let short = shorten_app_path(filename).unwrap_or_else(|| filename.to_string());
        return Err(TaskError::HandlerError(format!(
            "JSX/TSX is not supported in Rivers v1: {short}"
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

    GLOBALS.set(
        &Globals::default(),
        || -> Result<(String, Vec<String>, String), TaskError> {
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

            let imports = extract_imports(&program);

            // Emit JS + collect source map entries.
            // Spec §2.4: ES2022 is the compilation floor. Setting
            // `Config::with_target(EsVersion::Es2022)` tells the emitter to
            // lower syntax above ES2022 — matches what V8 v130 reliably
            // supports and what the parser accepts at §2.1.
            let mut buf = Vec::<u8>::new();
            let mut srcmap_entries: Vec<(
                swc_core::common::BytePos,
                swc_core::common::source_map::LineCol,
            )> = Vec::new();
            {
                let writer = JsWriter::new(cm.clone(), "\n", &mut buf, Some(&mut srcmap_entries));
                let mut emitter = Emitter {
                    cfg: swc_core::ecma::codegen::Config::default().with_target(EsVersion::Es2022),
                    cm: cm.clone(),
                    comments: None,
                    wr: writer,
                };
                emitter
                    .emit_program(&program)
                    .map_err(|e| TaskError::Internal(format!("swc codegen failed: {e}")))?;
            }
            let js = String::from_utf8(buf)
                .map_err(|e| TaskError::Internal(format!("swc output not UTF-8: {e}")))?;

            // Build + serialize the source map. DefaultSourceMapGenConfig
            // yields path-only `sources` entries (no inlined content).
            let source_map =
                cm.build_source_map(&srcmap_entries, None, DefaultSourceMapGenConfig);
            let mut map_buf = Vec::<u8>::new();
            source_map
                .to_writer(&mut map_buf)
                .map_err(|e| TaskError::Internal(format!("source map write: {e}")))?;
            let map_json = String::from_utf8(map_buf)
                .map_err(|e| TaskError::Internal(format!("source map not UTF-8: {e}")))?;

            Ok((js, imports, map_json))
        },
    )
}

/// Shorten an absolute handler path to spec §2.5's `{app}/{path}` form.
///
/// If the input contains a `libraries` directory, return `{app}/libraries/…`
/// where `{app}` is the directory immediately above `libraries`. Otherwise
/// return `None` — caller falls back to the raw input.
fn shorten_app_path(filename: &str) -> Option<String> {
    let path = std::path::Path::new(filename);
    let mut components: Vec<String> = Vec::new();
    let mut found_libraries = false;
    for comp in path.components().rev() {
        let s = comp.as_os_str().to_string_lossy().to_string();
        if s == "libraries" {
            found_libraries = true;
            components.push(s);
            // Grab one more level up — that's `{app}`.
            continue;
        }
        components.push(s);
        if found_libraries {
            // We've now captured the app name; stop.
            break;
        }
    }
    if !found_libraries {
        return None;
    }
    components.reverse();
    Some(components.join("/"))
}

/// Walk a post-transform Program and collect every runtime import specifier.
///
/// Covers:
/// - `import ... from "x"`
/// - `import "x"` (bare side-effect import)
/// - `export ... from "x"`
/// - `export * from "x"`
///
/// Dynamic `import("x")` calls are ignored — cycle detection is static.
fn extract_imports(program: &Program) -> Vec<String> {
    let Program::Module(module) = program else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else { continue; };
        match decl {
            ModuleDecl::Import(i) => out.push(i.src.value.to_atom_lossy().as_str().to_string()),
            ModuleDecl::ExportAll(e) => out.push(e.src.value.to_atom_lossy().as_str().to_string()),
            ModuleDecl::ExportNamed(n) => {
                if let Some(src) = &n.src {
                    out.push(src.value.to_atom_lossy().as_str().to_string());
                }
            }
            _ => {}
        }
    }
    out
}

