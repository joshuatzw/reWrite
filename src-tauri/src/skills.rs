use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::foreground::OutputFormat;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub enabled: bool,
    pub order: i32,
    #[serde(default)]
    pub base_skill_id: Option<String>,
    /// Logical per-skill edit time used to merge this skill across devices.
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillsConfig {
    #[serde(default)]
    pub global_instructions: String,
    #[serde(default)]
    pub skills: Vec<Skill>,
    /// Tracks which built-in skills are enabled; absent key = enabled (true)
    #[serde(default)]
    pub builtin_enabled: HashMap<String, bool>,
    /// Kept in this blob (and mirrored to config.toml) so only the default
    /// skill preference, not unrelated app settings, follows the account.
    #[serde(default = "default_skill_id")]
    pub default_skill_id: String,
    /// Tombstones for deleted skills: id -> deleted_at_ms, used to stop a
    /// deletion from being resurrected by a stale copy on another device.
    #[serde(default)]
    pub deleted_skills: HashMap<String, i64>,
    /// Logical edit time of the non-skill ("scalar") fields above, merged
    /// by last-write-wins independently of the per-skill merge.
    #[serde(default)]
    pub scalar_updated_at_ms: i64,
}

fn default_skill_id() -> String {
    "__proofread__".to_string()
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            global_instructions: String::new(),
            skills: Vec::new(),
            builtin_enabled: HashMap::new(),
            default_skill_id: default_skill_id(),
            deleted_skills: HashMap::new(),
            scalar_updated_at_ms: 0,
        }
    }
}

pub fn new_id() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

pub fn load(path: &Path) -> SkillsConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(config: &SkillsConfig, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

pub fn file_has_default_skill_id(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|value| value.get("default_skill_id").cloned())
        .is_some()
}

/// Union both tombstone maps, keeping the greater deleted_at_ms per id.
fn merge_tombstones(a: &HashMap<String, i64>, b: &HashMap<String, i64>) -> HashMap<String, i64> {
    let mut merged = a.clone();
    for (id, &deleted_at) in b {
        merged
            .entry(id.clone())
            .and_modify(|existing| *existing = (*existing).max(deleted_at))
            .or_insert(deleted_at);
    }
    merged
}

/// Per-id union of two skill lists: the greater `updated_at_ms` wins, and a
/// skill deleted at/after its last edit is dropped. Order is renumbered
/// densely afterwards so gaps left by drops don't leak into the UI.
fn merge_skill_lists(
    local: &[Skill],
    cloud: &[Skill],
    tombstones: &HashMap<String, i64>,
) -> Vec<Skill> {
    let mut by_id: HashMap<String, Skill> = HashMap::new();
    for skill in local.iter().chain(cloud.iter()) {
        by_id
            .entry(skill.id.clone())
            .and_modify(|existing| {
                if skill.updated_at_ms > existing.updated_at_ms {
                    *existing = skill.clone();
                }
            })
            .or_insert_with(|| skill.clone());
    }
    let mut merged: Vec<Skill> = by_id
        .into_values()
        .filter(|skill| {
            tombstones
                .get(&skill.id)
                .is_none_or(|&deleted_at| deleted_at < skill.updated_at_ms)
        })
        .collect();
    merged.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    for (i, skill) in merged.iter_mut().enumerate() {
        skill.order = i as i32;
    }
    merged
}

/// Merge a local and cloud `SkillsConfig` for cross-device sync: skills merge
/// per-id (union, newest edit wins, tombstones drop deletions), while the
/// scalar fields (global_instructions/builtin_enabled/default_skill_id) merge
/// as one last-write-wins blob using `scalar_updated_at_ms`.
pub fn merge_configs(local: &SkillsConfig, cloud: &SkillsConfig) -> SkillsConfig {
    let tombstones = merge_tombstones(&local.deleted_skills, &cloud.deleted_skills);
    let skills = merge_skill_lists(&local.skills, &cloud.skills, &tombstones);
    let mut deleted_skills = tombstones;
    for skill in &skills {
        deleted_skills.remove(&skill.id);
    }
    let scalar_source = if cloud.scalar_updated_at_ms > local.scalar_updated_at_ms {
        cloud
    } else {
        local
    };
    SkillsConfig {
        global_instructions: scalar_source.global_instructions.clone(),
        skills,
        builtin_enabled: scalar_source.builtin_enabled.clone(),
        default_skill_id: scalar_source.default_skill_id.clone(),
        deleted_skills,
        scalar_updated_at_ms: scalar_source.scalar_updated_at_ms,
    }
}

pub fn builtin_display_name(id: &str) -> Option<&'static str> {
    match id {
        "__proofread__" => Some("Proofread"),
        "__polish__" => Some("Polish"),
        "__summarise__" => Some("Summarise"),
        "__enhance__" => Some("Enhance"),
        _ => None,
    }
}

pub fn skill_display_name(config: &SkillsConfig, id: &str) -> String {
    if let Some(name) = builtin_display_name(id) {
        return name.to_string();
    }
    config
        .skills
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.to_string())
}

pub fn is_builtin_enabled(config: &SkillsConfig, id: &str) -> bool {
    config.builtin_enabled.get(id).copied().unwrap_or(true)
}

fn builtin_core_prompt(id: &str) -> Option<&'static str> {
    match id {
        "__proofread__" => Some("Correct all spelling, grammar, and punctuation errors in the text below.\nDo not change the writer's tone, vocabulary, sentence structure, or word choice unless it contains an actual error.\nDo not rephrase for style, do not shorten or lengthen it, and do not make it more formal or casual.\nPreserve line breaks, formatting, and paragraph structure exactly as given.\nDo not use em dashes."),
        "__polish__" => Some("Rewrite the text below so it is ready to be shared with a third party (e.g. a colleague, client, or manager) for review.\nFix any grammar or clarity issues, tighten loose phrasing, and adjust tone so it reads as professional and considered.\nKeep the length roughly the same; do not summarize or expand significantly.\nPreserve the core meaning, intent, and key details exactly. Do not add new claims, arguments, or information.\nDo not use em dashes."),
        "__summarise__" => Some("Summarize the text below, keeping only the most important points, decisions, or asks.\nPreserve the original intent and any critical details (numbers, names, deadlines, action items); do not lose information that changes the meaning.\nWrite in clear, complete sentences (not just fragments or bullet-only unless the input is already a list).\nAim for roughly 30-50% of the original length, adjusting based on how much can be safely cut.\nDo not use em dashes."),
        "__enhance__" => Some("The text below feels thin or underdeveloped. Rewrite it to be more substantial and persuasive, suitable for a polished email, proposal, or executive summary.\nAdd depth by strengthening weak statements, making vague points more concrete, and improving the logical flow between ideas, but do not invent specific facts, numbers, or claims that aren't implied by the original.\nElevate the language and structure so it reads as complete and ready to send, without becoming bloated or repetitive.\nDo not use em dashes."),
        _ => None,
    }
}

fn resolve_base_prompt(config: &SkillsConfig, base_id: &str) -> Option<String> {
    if let Some(p) = builtin_core_prompt(base_id) {
        return Some(p.to_string());
    }
    config
        .skills
        .iter()
        .find(|s| s.id == base_id)
        .map(|s| s.instructions.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn skill_core_prompt(config: &SkillsConfig, id: &str) -> String {
    if let Some(builtin) = builtin_core_prompt(id) {
        return builtin.to_string();
    }
    if let Some(skill) = config.skills.iter().find(|s| s.id == id) {
        let own = skill.instructions.trim().to_string();
        if let Some(ref base_id) = skill.base_skill_id {
            if let Some(base_prompt) = resolve_base_prompt(config, base_id) {
                if own.is_empty() {
                    return base_prompt;
                }
                return format!("{base_prompt}\n\nThe following instructions take priority and override the above where they conflict:\n{own}");
            }
        }
        return own;
    }
    String::new()
}

pub fn build_system_prompt(
    config: &SkillsConfig,
    skill_id: Option<&str>,
    format: OutputFormat,
) -> String {
    let global = config.global_instructions.trim();
    let skill_instr = skill_id
        .map(|id| skill_core_prompt(config, id))
        .unwrap_or_default();

    let combined = match (global.is_empty(), skill_instr.is_empty()) {
        (true, true) => {
            let base = "Rewrite the following text to improve clarity and flow.".to_string();
            return append_format_and_close(base, format);
        }
        (false, true) => global.to_string(),
        (true, false) => skill_instr,
        (false, false) => format!("{global}\n\n{skill_instr}"),
    };

    append_format_and_close(combined, format)
}

/// Append the output-format instructions, then the trailing "Return only..."
/// line.
fn append_format_and_close(mut prompt: String, format: OutputFormat) -> String {
    match format {
        OutputFormat::Html => prompt.push_str(
            "\n\nFormat your output as semantic inline HTML suitable for pasting directly into a rich-text email or document composer. Use <p> for paragraphs, <strong> or <em> for emphasis, and <ul>/<ol> with <li> for lists. Apply bold and lists ONLY where the content's structure naturally calls for it — never impose structure on text that should stay as written (for example, straightforward proofreading). Do not use Markdown, do not wrap the output in code fences, and do not include <html>, <head>, or <body> tags.",
        ),
        OutputFormat::PlainText => prompt.push_str(
            "\n\nReturn plain text only. Do not use Markdown, HTML tags, or any other markup.",
        ),
    }
    prompt.push_str("\nReturn only the result, without any explanation or preamble.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, order: i32, updated_at_ms: i64) -> Skill {
        Skill {
            id: id.to_string(),
            name: id.to_string(),
            instructions: String::new(),
            enabled: true,
            order,
            base_skill_id: None,
            updated_at_ms,
        }
    }

    fn config_with(skills: Vec<Skill>, scalar_updated_at_ms: i64) -> SkillsConfig {
        SkillsConfig {
            skills,
            scalar_updated_at_ms,
            ..SkillsConfig::default()
        }
    }

    #[test]
    fn cloud_only_skill_appears_in_merge() {
        let local = config_with(vec![skill("a", 0, 1)], 0);
        let cloud = config_with(vec![skill("a", 0, 1), skill("b", 1, 1)], 0);
        let merged = merge_configs(&local, &cloud);
        assert!(merged.skills.iter().any(|s| s.id == "b"));
        assert_eq!(merged.skills.len(), 2);
    }

    #[test]
    fn greater_updated_at_wins_for_shared_id() {
        let mut newer = skill("a", 0, 5);
        newer.name = "newer".to_string();
        let local = config_with(vec![skill("a", 0, 1)], 0);
        let cloud = config_with(vec![newer], 0);
        let merged = merge_configs(&local, &cloud);
        assert_eq!(merged.skills[0].name, "newer");
    }

    #[test]
    fn tombstone_after_edit_removes_skill() {
        let local = config_with(vec![skill("a", 0, 1)], 0);
        let mut cloud = config_with(vec![], 0);
        cloud.deleted_skills.insert("a".to_string(), 5);
        let merged = merge_configs(&local, &cloud);
        assert!(merged.skills.is_empty());
    }

    #[test]
    fn edit_after_deletion_survives_and_drops_tombstone() {
        let mut local = config_with(vec![], 0);
        local.deleted_skills.insert("a".to_string(), 5);
        let cloud = config_with(vec![skill("a", 0, 10)], 0);
        let merged = merge_configs(&local, &cloud);
        assert_eq!(merged.skills.len(), 1);
        assert!(!merged.deleted_skills.contains_key("a"));
    }

    #[test]
    fn scalar_fields_taken_from_greater_scalar_updated_at() {
        let mut local = config_with(vec![], 3);
        local.global_instructions = "local".to_string();
        let mut cloud = config_with(vec![], 7);
        cloud.global_instructions = "cloud".to_string();
        let merged = merge_configs(&local, &cloud);
        assert_eq!(merged.global_instructions, "cloud");
        assert_eq!(merged.scalar_updated_at_ms, 7);
    }

    #[test]
    fn legacy_local_only_skill_preserved() {
        let local = config_with(vec![skill("legacy", 0, 0)], 0);
        let cloud = config_with(vec![skill("cloud-skill", 0, 0)], 0);
        let merged = merge_configs(&local, &cloud);
        assert!(merged.skills.iter().any(|s| s.id == "legacy"));
        assert!(merged.skills.iter().any(|s| s.id == "cloud-skill"));
    }
}
