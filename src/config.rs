//! Configuration parsing. Reads JSONC (via json5, which accepts comments and
//! trailing commas) into a typed [`Config`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub screens: BTreeMap<String, Screen>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub transition: Transition,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Screen {
    pub name: String,
    pub orientation: Orientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Landscape,
    Portrait,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Defaults {
    #[serde(default)]
    pub auto: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default)]
    pub sequence: bool,
    pub packs_location: String,
    /// Order in which monitors are swapped (and visible in `sequence` mode).
    /// Keys reference [`Config::screens`]. Defaults to sorted screen keys.
    #[serde(default)]
    pub screen_order: Option<Vec<String>>,
    /// Whether switching packs swaps the wallpaper immediately instead of
    /// waiting for the next scheduled tick.
    #[serde(default = "default_true")]
    pub swap_on_pack_change: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            auto: false,
            debug: false,
            sequence: false,
            packs_location: String::from("~/Wallpapers/packs"),
            screen_order: None,
            swap_on_pack_change: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transition {
    #[serde(default)]
    pub next: NextMode,
    #[serde(default = "default_fill_mode")]
    pub fill_mode: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_transition_type")]
    pub transition_type: String,
    #[serde(default = "default_transition_duration")]
    pub transition_duration: u32,
    #[serde(default = "default_transition_step")]
    pub transition_step: u32,
    #[serde(default = "default_transition_fps")]
    pub transition_fps: u32,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            next: NextMode::default(),
            fill_mode: default_fill_mode(),
            interval: default_interval(),
            transition_type: default_transition_type(),
            transition_duration: default_transition_duration(),
            transition_step: default_transition_step(),
            transition_fps: default_transition_fps(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NextMode {
    #[default]
    Ordered,
    Random,
}

fn default_fill_mode() -> String {
    "crop".into()
}
fn default_interval() -> u64 {
    300
}
fn default_transition_type() -> String {
    "fade".into()
}
fn default_transition_duration() -> u32 {
    2
}
fn default_transition_step() -> u32 {
    20
}
fn default_transition_fps() -> u32 {
    60
}

impl Config {
    /// Load and parse a config file. Accepts JSON, JSONC, and JSON5.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading config file: {}", path.display()))?;
        let cfg: Config = json5::from_str(&raw)
            .with_context(|| format!("parsing config file: {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.screens.is_empty() {
            anyhow::bail!("config has no screens defined");
        }
        if let Some(order) = &self.defaults.screen_order {
            for key in order {
                if !self.screens.contains_key(key) {
                    anyhow::bail!(
                        "defaults.screen_order references unknown screen '{}'",
                        key
                    );
                }
            }
        }
        Ok(())
    }

    /// Returns the resolved (tilde-expanded) packs root directory.
    pub fn packs_root(&self) -> PathBuf {
        expand_tilde(&self.defaults.packs_location)
    }

    /// Returns the resolved pack directory for the given profile name.
    pub fn pack_dir(&self, profile: &str) -> PathBuf {
        self.packs_root().join(profile)
    }

    /// Ordered list of screen keys to apply swaps to.
    pub fn ordered_screen_keys(&self) -> Vec<String> {
        match &self.defaults.screen_order {
            Some(order) => order.clone(),
            None => self.screens.keys().cloned().collect(),
        }
    }
}

fn expand_tilde(input: &str) -> PathBuf {
    if input == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(input));
    }
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_with_comments_and_trailing_commas() {
        let src = r#"{
            // a comment
            "screens": {
                "1": { "name": "DP-3", "orientation": "landscape" },
                "2": { "name": "HDMI-A-1", "orientation": "portrait" },
            },
            "defaults": {
                "auto": true,
                "packs_location": "~/Wallpapers/packs",
            },
            "transition": { "next": "ordered", "interval": 5 }
        }"#;
        let cfg: Config = json5::from_str(src).expect("should parse");
        assert_eq!(cfg.screens.len(), 2);
        assert!(cfg.defaults.auto);
        assert_eq!(cfg.transition.interval, 5);
        assert_eq!(cfg.transition.next, NextMode::Ordered);
    }

    #[test]
    fn ordered_screen_keys_falls_back_to_sorted_screens() {
        let src = r#"{
            "screens": {
                "2": { "name": "B", "orientation": "portrait" },
                "1": { "name": "A", "orientation": "landscape" }
            },
            "defaults": { "packs_location": "~/x" }
        }"#;
        let cfg: Config = json5::from_str(src).unwrap();
        assert_eq!(cfg.ordered_screen_keys(), vec!["1", "2"]);
    }

    #[test]
    fn screen_order_must_reference_known_screens() {
        let src = r#"{
            "screens": { "1": { "name": "A", "orientation": "landscape" } },
            "defaults": { "packs_location": "~/x", "screen_order": ["1", "nope"] }
        }"#;
        let cfg: Config = json5::from_str(src).unwrap();
        assert!(cfg.validate().is_err());
    }
}
