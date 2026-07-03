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
    pub tone_of_voice_id: Option<String>,
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
        "__proofread__" => Some(r#"Correct all spelling, grammar, and punctuation errors in the text below.
Do not change the writer's tone, vocabulary, sentence structure, or word choice unless it contains an actual error.
Do not rephrase for style, do not shorten or lengthen it, and do not make it more formal or casual.
Preserve line breaks, formatting, and paragraph structure exactly as given.
[IMPORTANT] Return only the corrected text, with no explanation or commentary."#),
        "__polish__" => Some(r#"Rewrite the text below so it is ready to be shared with a third party (e.g. a colleague, client, or manager) for review.
Fix any grammar or clarity issues, tighten loose phrasing, and adjust tone so it reads as professional and considered.
Keep the length roughly the same; do not summarize or expand significantly.
Preserve the core meaning, intent, and key details exactly. Do not add new claims, arguments, or information.
[IMPORTANT] Return only the rewritten text, with no explanation or commentary."#),
        "__summarise__" => Some(r#"Summarize the text below, keeping only the most important points, decisions, or asks.
Preserve the original intent and any critical details (numbers, names, deadlines, action items); do not lose information that changes the meaning.
Write in clear, complete sentences (not just fragments or bullet-only unless the input is already a list).
Aim for roughly 30-50% of the original length, adjusting based on how much can be safely cut.
[IMPORTANT] Return only the summary, with no explanation or commentary."#),
        "__enhance__" => Some(r#"The text below feels thin or underdeveloped. Rewrite it to be more substantial and persuasive, suitable for a polished email, proposal, or executive summary.
Add depth by strengthening weak statements, making vague points more concrete, and improving the logical flow between ideas, but do not invent specific facts, numbers, or claims that aren't implied by the original.
Elevate the language and structure so it reads as complete and ready to send, without becoming bloated or repetitive.
[IMPORTANT] Return only the rewritten text, with no explanation or commentary."#),
        _ => None,
    }
}

fn skill_core_prompt(config: &SkillsConfig, id: &str) -> String {
    if let Some(builtin) = builtin_core_prompt(id) {
        return builtin.to_string();
    }
    config
        .skills
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.instructions.trim().to_string())
        .unwrap_or_default()
}

/// Resolve the tone-of-voice content to apply for a given skill, if any.
///
/// A custom skill with an explicit `tone_of_voice_id` overrides the global
/// default; otherwise the tone flagged `is_default` (if any) is used. Returns
/// `None` when no tone applies or the resolved tone's content is empty.
pub fn resolve_tone_content(
    config: &SkillsConfig,
    tones: &[crate::tone_of_voice::ToneOfVoice],
    skill_id: &str,
) -> Option<String> {
    // Explicit per-skill tone overrides the default.
    if let Some(skill) = config.skills.iter().find(|s| s.id == skill_id) {
        if let Some(ref tone_id) = skill.tone_of_voice_id {
            if let Some(tone) = tones.iter().find(|t| &t.id == tone_id) {
                let content = tone.content.trim();
                return (!content.is_empty()).then(|| content.to_string());
            }
        }
    }

    // Fall back to the global default tone.
    let content = tones.iter().find(|t| t.is_default)?.content.trim().to_string();
    (!content.is_empty()).then_some(content)
}

pub fn build_system_prompt(
    config: &SkillsConfig,
    skill_id: Option<&str>,
    tone: Option<&str>,
    format: OutputFormat,
) -> String {
    let global = config.global_instructions.trim();
    let skill_instr = skill_id
        .map(|id| skill_core_prompt(config, id))
        .unwrap_or_default();

    let combined = match (global.is_empty(), skill_instr.is_empty()) {
        (true, true) => {
            let base = "Rewrite the following text to improve clarity and flow.".to_string();
            return append_tone_and_close(base, tone, format);
        }
        (false, true) => global.to_string(),
        (true, false) => skill_instr,
        (false, false) => format!("{global}\n\n{skill_instr}"),
    };

    append_tone_and_close(combined, tone, format)
}

/// Append the optional tone block, the output-format instructions, then the
/// trailing "Return only..." line.
fn append_tone_and_close(mut prompt: String, tone: Option<&str>, format: OutputFormat) -> String {
    if let Some(tone) = tone {
        let tone = tone.trim();
        if !tone.is_empty() {
            prompt.push_str(&format!(
                "\n\nApply the following tone of voice / writing style to your output. Match its language, phrasing, and register while still following the instructions above and preserving the original meaning:\n\"\"\"\n{tone}\n\"\"\""
            ));
        }
    }
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
