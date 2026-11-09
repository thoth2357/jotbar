//! App settings, stored as JSON in the platform config dir
//! (~/.config/jotbar/config.json on Linux).

use jotbar_core::remote::{expand_home, Target};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Where notes live. Default: platform data dir (~/.local/share/jotbar).
    pub notes_dir: Option<String>,
    /// small | regular | large | xlarge
    pub size: String,
    pub hide_on_blur: bool,
    pub font_size: u32,
    /// Global hotkey (works on X11/macOS/Windows; on Wayland bind `jotbar --toggle` instead).
    pub hotkey: String,
    pub sync: SyncConfig,
    pub magic: MagicConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SyncConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MagicConfig {
    pub enabled: bool,
    /// Ollama base URL.
    pub endpoint: String,
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            notes_dir: None,
            size: "regular".into(),
            hide_on_blur: true,
            font_size: 15,
            hotkey: "CommandOrControl+Shift+Space".into(),
            sync: SyncConfig::default(),
            magic: MagicConfig::default(),
        }
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig { enabled: false, interval_secs: 60, target: None }
    }
}

impl Default for MagicConfig {
    fn default() -> Self {
        MagicConfig { enabled: false, endpoint: "http://127.0.0.1:11434".into(), model: String::new() }
    }
}

pub fn size_px(size: &str) -> (f64, f64) {
    match size {
        "small" => (340.0, 400.0),
        "large" => (560.0, 660.0),
        "xlarge" => (760.0, 840.0),
        _ => (440.0, 540.0),
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| ".".into()).join("jotbar/config.json")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        jotbar_core::atomic_write(&path, &serde_json::to_vec_pretty(self)?)?;
        // The file can hold sync credentials: keep it private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn data_dir(&self) -> PathBuf {
        match self.notes_dir.as_deref().map(str::trim) {
            Some(d) if !d.is_empty() => expand_home(d),
            _ => dirs::data_dir().unwrap_or_else(|| ".".into()).join("jotbar"),
        }
    }
}
