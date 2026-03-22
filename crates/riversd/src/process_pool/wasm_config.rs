//! Wasmtime config types — WasmtimeConfig, WasmtimeWorker.

use rivers_runtime::rivers_core::config::ProcessPoolConfig;

use super::types::*;

// ── Wasmtime Config & Worker ────────────────────────────────────

/// Configuration for the Wasmtime WebAssembly engine.
///
/// Per spec §14.2: Wasmtime instance pool settings.
#[derive(Debug, Clone)]
pub struct WasmtimeConfig {
    /// Number of pre-compiled WASM instances in the pool.
    pub instance_pool_size: usize,
    /// Fuel limit for execution metering (0 = unlimited).
    pub fuel_limit: u64,
    /// Maximum WASM linear memory pages (each page = 64 KiB).
    pub memory_pages: u32,
}

impl Default for WasmtimeConfig {
    fn default() -> Self {
        Self {
            instance_pool_size: 4,
            fuel_limit: 1_000_000,
            memory_pages: 256, // 16 MiB
        }
    }
}

/// Wasmtime WebAssembly engine worker.
///
/// Wraps the existing Wasmtime execution infrastructure with config-driven
/// fuel limits, memory limits, and epoch-based preemption per spec §6.0-6.3.
///
/// Wasmtime is already a workspace dependency — no feature flag needed.
#[derive(Debug, Clone)]
pub struct WasmtimeWorker {
    config: WasmtimeConfig,
}

impl WasmtimeWorker {
    /// Create a Wasmtime worker with the given configuration.
    pub fn new(config: WasmtimeConfig) -> Result<Self, TaskError> {
        // Validate wasmtime is available by creating a test engine
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.consume_fuel(true);
        wasmtime::Engine::new(&wasm_config)
            .map_err(|e| TaskError::Internal(format!("wasmtime engine init: {e}")))?;
        Ok(Self { config })
    }

    /// Get the configured fuel limit.
    pub fn fuel_limit(&self) -> u64 {
        self.config.fuel_limit
    }

    /// Get the configured memory pages limit.
    pub fn memory_pages(&self) -> u32 {
        self.config.memory_pages
    }

    /// Get the configured instance pool size.
    pub fn pool_size(&self) -> usize {
        self.config.instance_pool_size
    }

    /// Build a `WasmtimeConfig` from a `ProcessPoolConfig`.
    pub fn config_from_pool(pool_config: &ProcessPoolConfig) -> WasmtimeConfig {
        WasmtimeConfig {
            instance_pool_size: pool_config.workers,
            fuel_limit: pool_config.task_timeout_ms * 1000, // rough: 1ms ≈ 1000 fuel
            memory_pages: (pool_config.max_heap_mb * 1024 / 64) as u32, // MiB → 64KiB pages
        }
    }
}

