export interface Skill {
  id: string;
  name: string;
  instructions: string;
  enabled: boolean;
  order: number;
  tone_of_voice_id?: string | null;
}

export interface ToneOfVoice {
  id: string;
  name: string;
  content: string;
  is_default: boolean;
}

export interface SkillsConfig {
  global_instructions: string;
  skills: Skill[];
  builtin_enabled: Record<string, boolean>;
}

export interface Config {
  hotkey: string;
  super_hotkey: string;
  default_skill_id: string;
  model: string;
  restore_clipboard: boolean;
  restore_delay_ms: number;
  paste_delay_ms: number;
}

export interface HistoryEntry {
  id: string;
  timestamp_ms: number;
  skill_id: string;
  skill_name: string;
  input_text: string;
  output_text: string;
  output_word_count: number;
}
