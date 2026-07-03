use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs, path::Path,
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

pub fn load(path: &Path) -> HistoryStore {
    let Ok(bytes) = fs::read(path) else {
        return HistoryStore::default();
    };
    if bytes.is_empty() {
        return HistoryStore::default();
    }

    // Preferred path: DPAPI-encrypted JSON. If decryption succeeds and the
    // decrypted bytes parse as JSON, use that.
    if let Some(plain) = crate::secure_store::decrypt(&bytes) {
        if let Ok(store) = serde_json::from_slice::<HistoryStore>(&plain) {
            return store;
        }
    }

    // Legacy fallback: the file predates encryption and is raw plaintext JSON.
    // Don't lose existing users' history.
    serde_json::from_slice::<HistoryStore>(&bytes).unwrap_or_default()
}

pub fn save(store: &HistoryStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(store)?;
    let cipher = crate::secure_store::encrypt(&json)?;
    fs::write(path, cipher)?;
    Ok(())
}
