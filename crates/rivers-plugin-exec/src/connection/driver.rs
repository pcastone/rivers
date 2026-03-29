//! `ExecDriver` — `DatabaseDriver` factory that creates `ExecConnection` instances.
//!
//! Parses datasource config, validates command files at startup (integrity
//! hashing), builds per-command semaphores, and returns a boxed connection.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use rivers_driver_sdk::{Connection, ConnectionParams, DatabaseDriver, DriverError};

use crate::config::ExecConfig;
use crate::integrity::{self, CommandIntegrity};
use crate::schema;

use super::exec_connection::ExecConnection;

// ── Per-command runtime state ─────────────────────────────────────────

/// Per-command runtime state including integrity checker and semaphore.
pub struct CommandRuntime {
    pub config: crate::config::CommandConfig,
    pub integrity: CommandIntegrity,
    pub schema: Option<schema::CompiledSchema>,
    pub semaphore: Option<Arc<Semaphore>>,
}

// ── ExecDriver (DatabaseDriver) ──────────────────────────────────────

/// Factory that creates `ExecConnection` instances from datasource config.
pub struct ExecDriver;

#[async_trait]
impl DatabaseDriver for ExecDriver {
    fn name(&self) -> &str {
        "rivers-exec"
    }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        // 1. Parse config from params.options
        let config = ExecConfig::parse(params)?;

        // 2. Validate config
        config.validate()?;

        // 3. Startup integrity check — hash all command files
        let mut commands = HashMap::new();
        for (name, cmd_config) in &config.commands {
            let mode = cmd_config
                .integrity_check
                .as_ref()
                .unwrap_or(&config.integrity_check);

            // Verify hash at startup
            let pinned = integrity::verify_at_startup(&cmd_config.path, &cmd_config.sha256)?;
            let integrity = CommandIntegrity::new(mode.clone(), pinned);

            // Log integrity mode
            integrity::log_integrity_mode("exec", name, mode);

            // Warn if env_clear is false (spec S11.2, S15.1)
            if !cmd_config.env_clear {
                tracing::warn!(
                    datasource = "exec",
                    command = %name,
                    "env_clear=false — host environment variables will be inherited, risk of credential leakage"
                );
            }

            // Load schema if declared
            let compiled_schema = if let Some(ref schema_path) = cmd_config.args_schema {
                Some(schema::CompiledSchema::load(schema_path)?)
            } else {
                None
            };

            // Build per-command semaphore
            let semaphore = cmd_config.max_concurrent.map(|n| Arc::new(Semaphore::new(n)));

            commands.insert(
                name.clone(),
                CommandRuntime {
                    config: cmd_config.clone(),
                    integrity,
                    schema: compiled_schema,
                    semaphore,
                },
            );
        }

        // 4. Build global semaphore
        let global_semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        Ok(Box::new(ExecConnection {
            config,
            commands,
            global_semaphore,
        }))
    }
}
