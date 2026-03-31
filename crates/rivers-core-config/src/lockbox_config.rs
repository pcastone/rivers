//! LockBox configuration type (light — no encryption dependencies).

use serde::Deserialize;

/// `[lockbox]` section in `riversd.conf`.
///
/// Per spec §5. This is the config struct only — encryption/resolver
/// logic lives in `rivers-core::lockbox`.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct LockBoxConfig {
    /// Path to the age-encrypted lockbox file.
    pub path: Option<String>,

    /// Key source: `"env"`, `"file"`, or `"agent"` (default: `"env"`).
    #[serde(default = "default_key_source")]
    pub key_source: String,

    /// Path to an age identity file (when `key_source = "file"`).
    pub key_file: Option<String>,

    /// Environment variable holding the age identity (default: `"RIVERS_LOCKBOX_KEY"`).
    #[serde(default = "default_key_env_var")]
    pub key_env_var: String,

    /// Unix socket for an ssh-agent-style key agent.
    pub agent_socket: Option<String>,

    /// Path to an age recipient (public key) file for encryption.
    pub recipient_file: Option<String>,
}

fn default_key_source() -> String {
    "env".to_string()
}

fn default_key_env_var() -> String {
    "RIVERS_LOCKBOX_KEY".to_string()
}
