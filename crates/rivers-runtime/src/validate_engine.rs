//! Engine dylib loading for Layer 4 syntax verification.
//!
//! Per `rivers-bundle-validation-spec.md` §5-6.
//!
//! Loads V8 and Wasmtime engine dylibs via `libloading` and resolves the
//! `_rivers_compile_check` and `_rivers_free_string` FFI symbols for
//! compile-only validation of handler modules.

use std::ffi::{c_char, CStr};
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── FFI Response Types ──────────────────────────────────────────

/// Successful compile check result from an engine dylib.
#[derive(Debug, Clone)]
pub struct CompileCheckResult {
    /// Export names found in the compiled module.
    pub exports: Vec<String>,
}

/// Compile check error from an engine dylib.
#[derive(Debug, Clone)]
pub struct CompileCheckError {
    /// Error message.
    pub message: String,
    /// Source line (1-based), if available.
    pub line: Option<u32>,
    /// Source column (1-based), if available.
    pub column: Option<u32>,
}

/// Raw JSON response from the FFI boundary.
#[derive(Deserialize)]
struct CompileCheckResponse {
    ok: bool,
    #[serde(default)]
    exports: Vec<String>,
    #[serde(default)]
    error: Option<CompileCheckErrorJson>,
}

#[derive(Deserialize)]
struct CompileCheckErrorJson {
    message: String,
    line: Option<u32>,
    column: Option<u32>,
}

// ── Engine Handle ───────────────────────────────────────────────

/// Safe wrapper around an engine dylib's compile_check FFI.
pub struct EngineHandle {
    _lib: libloading::Library,
    compile_check_fn:
        unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> *const c_char,
    free_string_fn: unsafe extern "C" fn(*const c_char),
}

impl EngineHandle {
    /// Load an engine dylib from the given path.
    ///
    /// Resolves `_rivers_compile_check` and `_rivers_free_string` symbols.
    /// Returns `None` if the library cannot be loaded or symbols are missing.
    pub fn load(path: &Path) -> Result<Self, String> {
        let lib = unsafe {
            libloading::Library::new(path)
                .map_err(|e| format!("dlopen '{}': {e}", path.display()))?
        };

        let compile_check_fn = unsafe {
            let sym: libloading::Symbol<
                unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> *const c_char,
            > = lib
                .get(b"_rivers_compile_check")
                .map_err(|e| format!("symbol _rivers_compile_check: {e}"))?;
            *sym
        };

        let free_string_fn = unsafe {
            let sym: libloading::Symbol<unsafe extern "C" fn(*const c_char)> = lib
                .get(b"_rivers_free_string")
                .map_err(|e| format!("symbol _rivers_free_string: {e}"))?;
            *sym
        };

        Ok(Self {
            _lib: lib,
            compile_check_fn,
            free_string_fn,
        })
    }

    /// Run a compile-only check on source bytes.
    ///
    /// Returns `Ok(CompileCheckResult)` with export names on success,
    /// or `Err(CompileCheckError)` with error details on failure.
    pub fn compile_check(
        &self,
        source: &[u8],
        filename: &str,
    ) -> Result<CompileCheckResult, CompileCheckError> {
        let json_ptr = unsafe {
            (self.compile_check_fn)(
                source.as_ptr(),
                source.len(),
                filename.as_ptr(),
                filename.len(),
            )
        };

        if json_ptr.is_null() {
            return Err(CompileCheckError {
                message: "compile_check returned null".into(),
                line: None,
                column: None,
            });
        }

        let json_str = unsafe { CStr::from_ptr(json_ptr) }
            .to_str()
            .map_err(|e| CompileCheckError {
                message: format!("invalid UTF-8 from compile_check: {e}"),
                line: None,
                column: None,
            })?;

        let response: CompileCheckResponse =
            serde_json::from_str(json_str).map_err(|e| CompileCheckError {
                message: format!("invalid JSON from compile_check: {e}"),
                line: None,
                column: None,
            })?;

        // Free the heap-allocated string
        unsafe { (self.free_string_fn)(json_ptr) };

        if response.ok {
            Ok(CompileCheckResult {
                exports: response.exports,
            })
        } else if let Some(err) = response.error {
            Err(CompileCheckError {
                message: err.message,
                line: err.line,
                column: err.column,
            })
        } else {
            Err(CompileCheckError {
                message: "compile_check failed with no error details".into(),
                line: None,
                column: None,
            })
        }
    }
}

// SAFETY: The dylib functions are stateless compile-only checks.
// Each call creates and destroys its own isolate/context.
unsafe impl Send for EngineHandle {}
unsafe impl Sync for EngineHandle {}

// ── Engine Handles Container ────────────────────────────────────

/// Container for loaded engine handles.
///
/// Holds optional V8 and Wasmtime engine handles. If an engine dylib
/// is not available, the corresponding field is `None` and syntax checks
/// for that engine type are skipped with a warning.
pub struct EngineHandles {
    /// V8 engine for TS/JS compile checks.
    pub v8: Option<EngineHandle>,
    /// Wasmtime engine for WASM validation.
    pub wasmtime: Option<EngineHandle>,
}

impl EngineHandles {
    /// Create empty handles (no engines loaded).
    pub fn none() -> Self {
        Self {
            v8: None,
            wasmtime: None,
        }
    }

    /// Whether any engine is available.
    pub fn any_available(&self) -> bool {
        self.v8.is_some() || self.wasmtime.is_some()
    }
}

// ── Engine Configuration ────────────────────────────────────────

/// Engine dylib paths configuration.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// Path to V8 engine dylib.
    pub v8_path: Option<PathBuf>,
    /// Path to Wasmtime engine dylib.
    pub wasmtime_path: Option<PathBuf>,
}

/// Discover engine dylib paths from a `riversd.toml` config file.
///
/// Reads the `[engines]` section:
/// ```toml
/// [engines]
/// v8 = "/path/to/librivers_engine_v8.dylib"
/// wasmtime = "/path/to/librivers_engine_wasm.dylib"
/// ```
pub fn discover_engines(config_path: &Path) -> Result<EngineConfig, String> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| format!("read '{}': {e}", config_path.display()))?;

    let value: toml::Value =
        toml::from_str(&content).map_err(|e| format!("parse '{}': {e}", config_path.display()))?;

    let mut config = EngineConfig::default();

    if let Some(engines) = value.get("engines").and_then(|v| v.as_table()) {
        if let Some(v8) = engines.get("v8").and_then(|v| v.as_str()) {
            config.v8_path = Some(PathBuf::from(v8));
        }
        // Also check "dir" key for directory-based discovery
        if let Some(dir) = engines.get("dir").and_then(|v| v.as_str()) {
            let dir_path = Path::new(dir);
            if dir_path.is_dir() {
                if config.v8_path.is_none() {
                    let v8_dylib = find_engine_dylib(dir_path, "v8");
                    if let Some(p) = v8_dylib {
                        config.v8_path = Some(p);
                    }
                }
                if config.wasmtime_path.is_none() {
                    let wasm_dylib = find_engine_dylib(dir_path, "wasm");
                    if let Some(p) = wasm_dylib {
                        config.wasmtime_path = Some(p);
                    }
                }
            }
        }
        if let Some(wasm) = engines.get("wasmtime").and_then(|v| v.as_str()) {
            config.wasmtime_path = Some(PathBuf::from(wasm));
        }
    }

    Ok(config)
}

/// Find an engine dylib in a directory by searching for matching filenames.
fn find_engine_dylib(dir: &Path, engine_name: &str) -> Option<PathBuf> {
    let pattern = format!("librivers_engine_{}", engine_name);
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.starts_with(&pattern) && (s.ends_with(".dylib") || s.ends_with(".so"))
        })
        .map(|e| e.path())
}

/// Load engine handles from an `EngineConfig`.
///
/// Attempts to load each engine dylib. If loading fails, the engine is
/// set to `None` and a warning is logged. This function never fails —
/// missing engines are graceful degradation per spec §12.
pub fn load_engines(config: &EngineConfig) -> (EngineHandles, Vec<String>) {
    let mut handles = EngineHandles::none();
    let mut warnings = Vec::new();

    if let Some(ref v8_path) = config.v8_path {
        match EngineHandle::load(v8_path) {
            Ok(h) => handles.v8 = Some(h),
            Err(e) => warnings.push(format!("V8 engine: {e}")),
        }
    }

    if let Some(ref wasm_path) = config.wasmtime_path {
        match EngineHandle::load(wasm_path) {
            Ok(h) => handles.wasmtime = Some(h),
            Err(e) => warnings.push(format!("Wasmtime engine: {e}")),
        }
    }

    (handles, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_handles_none_has_no_engines() {
        let h = EngineHandles::none();
        assert!(!h.any_available());
    }

    #[test]
    fn discover_engines_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("riversd.toml");
        std::fs::write(
            &config_path,
            r#"
[engines]
v8 = "/usr/lib/rivers/librivers_engine_v8.dylib"
wasmtime = "/usr/lib/rivers/librivers_engine_wasm.dylib"
"#,
        )
        .unwrap();

        let config = discover_engines(&config_path).unwrap();
        assert_eq!(
            config.v8_path.as_deref(),
            Some(Path::new("/usr/lib/rivers/librivers_engine_v8.dylib"))
        );
        assert_eq!(
            config.wasmtime_path.as_deref(),
            Some(Path::new("/usr/lib/rivers/librivers_engine_wasm.dylib"))
        );
    }

    #[test]
    fn discover_engines_missing_section() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("riversd.toml");
        std::fs::write(&config_path, "[base]\nport = 8080\n").unwrap();

        let config = discover_engines(&config_path).unwrap();
        assert!(config.v8_path.is_none());
        assert!(config.wasmtime_path.is_none());
    }

    #[test]
    fn load_engines_nonexistent_path() {
        let config = EngineConfig {
            v8_path: Some(PathBuf::from("/nonexistent/librivers_engine_v8.dylib")),
            wasmtime_path: None,
        };
        let (handles, warnings) = load_engines(&config);
        assert!(handles.v8.is_none());
        assert!(!warnings.is_empty());
    }
}
