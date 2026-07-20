import { BUILTIN_SKILLS } from "../skills";

export const APP_VERSION = "1.1.9";
// Brand "ink" accent, defined as a CSS custom property in src/theme.css so it
// automatically follows the OS light/dark appearance (see that file for the
// light/dark values and the reasoning behind them). Every existing call site
// that imports ACCENT picks up dark-mode support for free.
export const ACCENT = "var(--rw-accent)";
export const FREE_TIER_MONTHLY_LIMIT = 50;

export const BUILTIN_SKILL_OPTIONS = BUILTIN_SKILLS.map((b) => ({ id: b.id, name: b.name }));
