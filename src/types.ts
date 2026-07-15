export interface Skill {
  id: string;
  name: string;
  instructions: string;
  enabled: boolean;
  order: number;
  base_skill_id?: string | null;
  updated_at_ms?: number;
}

export interface SkillsConfig {
  global_instructions: string;
  skills: Skill[];
  builtin_enabled: Record<string, boolean>;
  default_skill_id: string;
  deleted_skills?: Record<string, number>;
  scalar_updated_at_ms?: number;
}

export interface Config {
  hotkey: string;
  super_hotkey: string;
  default_skill_id: string;
  model: string;
  restore_clipboard: boolean;
  restore_delay_ms: number;
  paste_delay_ms: number;
  bubble_enabled: boolean;
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
