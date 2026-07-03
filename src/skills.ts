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
