//! Last-used Site URL only. Tokens stay in memory this version.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub site_url: String,
}

pub fn load() -> Settings {
    let path = config_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let text = serde_json::to_string_pretty(settings)?;
    fs::write(&path, text).context("write settings")
}

pub fn cache_path() -> PathBuf {
    state_dir().join("cache.sqlite")
}

/// Per-user runtime dir for the sign-in socket. `XDG_RUNTIME_DIR` is private
/// to the login session; the state dir is the fallback.
pub fn runtime_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(xdg).join("comport");
    }
    state_dir()
}

fn config_path() -> PathBuf {
    config_dir().join("settings.json")
}

fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("comport");
    }
    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library/Application Support")
            .join("comport")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join("comport")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".config/comport")
    }
}

fn state_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("navigato/comport");
    }
    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library/Application Support")
            .join("navigato/comport")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join("navigato/comport")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".local/state/navigato/comport")
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
