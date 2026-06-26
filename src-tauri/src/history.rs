use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs, path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub timestamp_ms: i64,
    pub skill_id: String,
    pub skill_name: String,
    pub input_text: String,
    pub output_text: String,
    pub output_word_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistoryStore {
    #[serde(default)]
    pub entries: Vec<HistoryEntry>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn count_words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

pub fn load(path: &PathBuf) -> HistoryStore {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(store: &HistoryStore, path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string(store)?)?;
    Ok(())
}
