use std::{env, fs, sync::Arc};

use base64::Engine;
use cloneable_errors::{ErrorContext, ResContext, bail};
use serde::Deserialize;

pub struct AppConfig {
    pub app: GithubAppConfig,
}

#[derive(Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub listen: ListenConfig,
    pub app: GithubAppConfig,
}

#[derive(Deserialize, Clone)]
pub struct GithubAppConfig {
    pub webhook_secret: Arc<str>,
    pub private_key: Arc<str>,
    pub client_id: Arc<str>,
}

#[derive(Debug, Deserialize)]
pub struct ListenConfig {
    /// Listen on a TCP socket
    #[serde(default)]
    pub tcp: Option<Box<str>>,
    /// Listen on a unix socket
    #[serde(default)]
    pub unix: Option<Box<str>>,
    /// Mode for the unix socket
    #[serde(default = "default_unix_mode")]
    pub unix_mode: u32,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            tcp: Some("[::]:8080".into()),
            unix: None,
            unix_mode: 0,
        }
    }
}

fn default_unix_mode() -> u32 {
    0o666
}

impl FileConfig {
    pub fn get() -> Result<Self, ErrorContext> {
        if let Some(config_text) = env::var_os("AUTOMERGE_CONFIG") {
            // config file in envvar
            toml::from_slice(config_text.as_encoded_bytes())
                .context("Failed to parse the contents of AUTOMERGE_CONFIG as FileConfig")
        } else if let Some(config_b64) = env::var_os("AUTOMERGE_CONFIG_B64") {
            // config file base64-encoded in envvar
            let decoded = base64::engine::general_purpose::URL_SAFE
                .decode(config_b64.as_encoded_bytes())
                .context("Failed to base64-decode the contents of AUTOMERGE_CONFIG_B64 envvar")?;

            toml::from_slice(&decoded).context("Failed to parse the base64-decoded contents of AUTOMERGE_CONFIG_B64 envvar as FileConfig")
        } else if let Some(config_path) = env::var_os("AUTOMERGE_CONFIG_FILE") {
            // path to config in envvar
            let file = fs::read(&config_path).with_context(|| {
                format!(
                    "Failed to read {} (AUTOMERGE_CONFIG_FILE)",
                    config_path.display()
                )
            })?;

            toml::from_slice(&file).with_context(|| {
                format!(
                    "Failed to parse the contents of {} (AUTOMERGE_CONFIG_FILE) as FileConfig",
                    config_path.display()
                )
            })
        } else {
            bail!(
                "None of the supported AUTOMERGE_CONFIG* envvars present - no app config available"
            );
        }
    }
}
