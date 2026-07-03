use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::foreground::OutputFormat;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub enabled: bool,
    pub order: i32,
    #[serde(default)]
    pub base_skill_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsConfig {
    #[serde(default)]
    pub global_instructions: String,
    #[serde(default)]
    pub skills: Vec<Skill>,
    /// Tracks which built-in skills are enabled; absent key = enabled (true)
    #[serde(default)]
    pub builtin_enabled: HashMap<String, bool>,
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

pub fn builtin_display_name(id: &str) -> Option<&'static str> {
    match id {
        "__proofread__" => Some("Proofread"),
        "__formal_email__" => Some("Formal Email"),
        "__summarise__" => Some("Summarise"),
        "__shorten__" => Some("Shorten"),
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
        "__proofread__" => Some("Proofread the following text. Fix spelling mistakes and improve sentence structure and flow where needed, but preserve the author's tone, voice, and overall message. Do not rephrase ideas or change the content — only correct errors and smooth out awkward phrasing."),
        "__formal_email__" => Some("Rewrite the following as a polished, professional business email. Preserve the meaning. Use clear paragraphs and a respectful, formal tone."),
        "__summarise__" => Some("Summarise the following text as concise bullet points. Every item must be a bullet point starting with '•'. You are not to write any paragraphs or prose. Separate bullet points with break space."),
        "__shorten__" => Some("Shorten the following text while preserving its full meaning. Be concise and remove unnecessary words."),
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
