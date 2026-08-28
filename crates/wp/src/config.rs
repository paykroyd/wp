//! User configuration: `~/.config/wp/config.toml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum KeymapChoice {
    #[default]
    Modern,
    Classic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub keymap: KeymapChoice,
    /// Show the F-key template legend at the bottom of the screen.
    pub fkey_legend: bool,
    pub autosave_seconds: u64,
    /// Wrap column when saving plain text (0 = no wrapping).
    pub text_wrap: usize,
    /// Show the first-run hint line.
    pub show_hint: bool,
    /// Draw a blank row between paragraphs that have paragraph spacing.
    pub draft_spacing: bool,
    /// Extra bindings: key → command id, e.g. `"ctrl+shift+b" = "bold"`.
    pub bindings: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            keymap: KeymapChoice::Modern,
            fkey_legend: false,
            autosave_seconds: 30,
            text_wrap: 0,
            show_hint: true,
            draft_spacing: true,
            bindings: BTreeMap::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("wp");
        }
    }
    home().join(".config").join("wp")
}

pub fn state_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("wp");
        }
    }
    home().join(".local").join("state").join("wp")
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    /// Returns (config, existed).
    pub fn load() -> (Config, bool) {
        let p = config_path();
        match std::fs::read_to_string(&p) {
            Ok(s) => (toml::from_str(&s).unwrap_or_default(), true),
            Err(_) => (Config::default(), false),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = config_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let s = toml::to_string_pretty(self).unwrap_or_default();
        let header = "# wp configuration. keymap = \"modern\" | \"classic\".\n# Rebind keys under [bindings], e.g. \"ctrl+shift+b\" = \"bold\".\n# Command ids: Ctrl+K in wp lists them.\n\n";
        std::fs::write(p, format!("{}{}", header, s))
    }
}
