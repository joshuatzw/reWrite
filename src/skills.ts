export interface BuiltinSkill {
  id: string;
  name: string;
  description: string;
}

export const BUILTIN_SKILLS: BuiltinSkill[] = [
  {
    id: "__proofread__",
    name: "Proofread",
    description: "Fix spelling, grammar, and punctuation while keeping your exact tone and style.",
  },
  {
    id: "__polish__",
    name: "Polish",
    description: "Refine your text to read as professional and considered, ready for a third party.",
  },
  {
    id: "__summarise__",
    name: "Summarise",
    description: "Condense a long thought into its key points, keeping the critical details.",
  },
  {
    id: "__enhance__",
    name: "Enhance",
    description: "Beef up thin writing into something polished and ready to send.",
  },
];
