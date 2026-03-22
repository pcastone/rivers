//! LockBox configuration type (light — no encryption dependencies).

use serde::Deserialize;

/// `[lockbox]` section in `riversd.conf`.
///
/// Per spec §5. This is the config struct only — encryption/resolver
/// logic lives in `rivers-core::lockbox`.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct LockBoxConfig {
    pub path: Option<String>,

    #[serde(default = "default_key_source")]
    pub key_source: String,

    pub key_file: Option<String>,

    #[serde(default = "default_key_env_var")]
    pub key_env_var: String,

    pub agent_socket: Option<String>,

    pub recipient_file: Option<String>,
}

fn default_key_source() -> String {
    "env".to_string()
}

fn default_key_env_var() -> String {
    "RIVERS_LOCKBOX_KEY".to_string()
}
