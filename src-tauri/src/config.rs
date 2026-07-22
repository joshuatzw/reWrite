use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

fn default_hotkey() -> String {
    "ctrl+shift+r".to_string()
}
fn default_super_hotkey() -> String {
    "ctrl+shift+period".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_restore_clipboard() -> bool {
    true
}
fn default_restore_delay_ms() -> u64 {
    500
}
fn default_paste_delay_ms() -> u64 {
    400
}
fn default_default_skill_id() -> String {
    "__proofread__".to_string()
}
fn default_bubble_enabled() -> bool {
    true
}
/// Whether the first-run onboarding story has been seen and dismissed.
///
/// Defaults to `true` on purpose — the opposite of what a "has the user done
/// this yet?" flag usually wants. Every config.toml written before this field
/// existed belongs to someone already using reWrite, and deserializing those
/// files must not replay an introduction they don't need. A genuine first run
/// has no config.toml at all, which `lib.rs` detects and answers by explicitly
/// setting this to `false` before writing the initial file.
fn default_onboarding_completed() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default = "default_super_hotkey")]
    pub super_hotkey: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_restore_clipboard")]
    pub restore_clipboard: bool,
    #[serde(default = "default_restore_delay_ms")]
    pub restore_delay_ms: u64,
    #[serde(default = "default_paste_delay_ms")]
    pub paste_delay_ms: u64,
    #[serde(default = "default_default_skill_id")]
    pub default_skill_id: String,
    #[serde(default = "default_bubble_enabled")]
    pub bubble_enabled: bool,
    #[serde(default = "default_onboarding_completed")]
    pub onboarding_completed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            super_hotkey: default_super_hotkey(),
            model: default_model(),
            restore_clipboard: default_restore_clipboard(),
            restore_delay_ms: default_restore_delay_ms(),
            paste_delay_ms: default_paste_delay_ms(),
            default_skill_id: default_default_skill_id(),
            bubble_enabled: default_bubble_enabled(),
            onboarding_completed: default_onboarding_completed(),
        }
    }
}

pub fn load(path: &Path) -> Config {
    let mut config: Config = fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();

    if config.hotkey == "ctrl+shift+space" {
        config.hotkey = default_hotkey();
    }
    if config.super_hotkey == "ctrl+shift+r" {
        config.super_hotkey = default_super_hotkey();
    }

    config
}

pub fn save(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, toml::to_string(config)?)?;
    Ok(())
}
