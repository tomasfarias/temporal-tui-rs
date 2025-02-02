use std::path;
use std::str;

use serde_derive::Deserialize;

use crate::theme::{Theme, SOLARIZED_DARK_HIGH_CONTRAST};

fn default_port() -> u16 {
    7233
}

#[derive(Debug, Deserialize)]
pub struct ThemeSettings {
    name: Option<String>,
    #[serde(default)]
    #[serde(flatten)]
    theme: Theme,
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
    #[serde(rename = "theme")]
    pub theme_settings: Option<ThemeSettings>,
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
            .add_source(config::File::from(config_path).required(false))
            .add_source(config::Environment::with_prefix("temporal_tui"))
            .build()?;

        s.try_deserialize()
    }

    pub fn theme(&self) -> Result<Theme, anyhow::Error> {
        if let Some(theme_settings) = self.theme_settings.as_ref() {
            if let Some(owned_theme_name) = theme_settings.name.as_ref() {
                let theme_name = owned_theme_name.to_lowercase();
                match theme_name.as_str() {
                    "solarized_dark_high_contrast" => Ok(SOLARIZED_DARK_HIGH_CONTRAST),
                    s => Err(anyhow::anyhow!("unsupported theme {}", s)),
                }
            } else {
                Ok(theme_settings.theme)
            }
        } else {
            Ok(Theme::default())
        }
    }
}
