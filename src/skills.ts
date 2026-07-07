import type { SkillsConfig } from "./types";

export interface BuiltinSkill {
  id: string;
  name: string;
  description: string;
}

export const BUILTIN_SKILLS: BuiltinSkill[] = [
  {
    id: "__proofread__",
    name: "Proofread",
    description: "Fixes grammar, spelling, and punctuation before you send. Retains your writing style as is.",
  },
  {
    id: "__polish__",
    name: "Polish",
    description: "Refines your text so it's ready for a third party to review — professional and considered.",
  },
  {
    id: "__summarise__",
    name: "Summarise",
    description: "Condenses a long thought or chunk, elevating the best bits so your message gets across.",
  },
  {
    id: "__enhance__",
    name: "Enhance",
    description: "Writing feels too thin? Beefs up your email, proposal, or summary so it's polished and ready to go.",
  },
];

export interface SkillItem {
  id: string;
  name: string;
  description: string;
}

export function buildItems(cfg: SkillsConfig): SkillItem[] {
  const builtins = BUILTIN_SKILLS.filter((b) => cfg.builtin_enabled?.[b.id] !== false);

  const enabled = [...cfg.skills]
    .filter((s) => s.enabled)
    .sort((a, b) => a.order - b.order);

  const customItems = enabled.map((s) => {
    let description = s.instructions.trim();
    if (!description) {
      if (s.base_skill_id) {
        const baseName =
          BUILTIN_SKILLS.find((b) => b.id === s.base_skill_id)?.name ??
          enabled.find((b) => b.id === s.base_skill_id)?.name;
        description = baseName ? `Based on ${baseName}` : "No additional instructions.";
      } else {
        description = "No additional instructions.";
      }
    }
    return { id: s.id, name: s.name, description };
  });

  return [...builtins, ...customItems];
}
