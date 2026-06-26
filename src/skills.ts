export interface BuiltinSkill {
  id: string;
  name: string;
  description: string;
}

export const BUILTIN_SKILLS: BuiltinSkill[] = [
  {
    id: "__proofread__",
    name: "Proofread",
    description: "Fix spelling and grammar while preserving your tone and voice.",
  },
  {
    id: "__formal_email__",
    name: "Formal Email",
    description: "Rewrite as a polished, professional business email.",
  },
  {
    id: "__summarise__",
    name: "Summarise",
    description: "Condense the text into concise bullet points.",
  },
  {
    id: "__shorten__",
    name: "Shorten",
    description: "Shorten the text while preserving its full meaning.",
  },
];
