use std::path;
use std::str;

use serde_derive::Deserialize;
use x509_parser::{certificate::X509Certificate, parse_x509_certificate, pem::parse_x509_pem};

fn default_port() -> u16 {
    7233
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub debug: bool,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub namespace: String,
    pub server_root_ca_cert: path::PathBuf,
    pub client_cert: path::PathBuf,
    pub client_private_key: path::PathBuf,
}

impl Settings {
    pub fn new() -> Result<Self, config::ConfigError> {
        let home: Option<std::path::PathBuf> = std::env::home_dir();

        let config_home = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .and_then(|config_home| {
                let path = std::path::PathBuf::from(config_home);
                if path.is_absolute() {
                    Some(path)
                } else {
                    None
                }
            })
            .or_else(|| home.as_ref().map(|home| home.join(".config")))
            .ok_or(config::ConfigError::Message(
                "home configuration path not found".to_string(),
            ))?
            .join("temporal-tui");

        let config_path = config_home.join("config.toml");

        let s = config::Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(config::File::from(config_path).required(false))
            // Add in settings from the environment (with a prefix of TEMPORAL_TUI)
            .add_source(config::Environment::with_prefix("temporal_tui"))
            .build()?;

        s.try_deserialize()
    }
}
