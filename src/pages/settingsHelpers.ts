import type { HistoryEntry } from "../types";

export function getGreeting(): string {
  const h = new Date().getHours();
  if (h < 12) return "Good morning";
  if (h < 17) return "Good afternoon";
  return "Good evening";
}

export function hotkeyParts(raw: string): string[] {
  return raw.split("+").map((k) => k.trim().charAt(0).toUpperCase() + k.trim().slice(1));
}

export function formatTime(ms: number): string {
  const d = new Date(ms);
  const todayStr = new Date().toDateString();
  const yestStr = new Date(Date.now() - 86400000).toDateString();
  if (d.toDateString() === todayStr) {
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (d.toDateString() === yestStr) return "Yesterday";
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function truncate(text: string, max: number): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length > max ? `${clean.slice(0, max).trimEnd()}\u2026` : clean;
}

export function firstNameFromEmail(email: string): string {
  const local = email.split("@")[0];
  const part = local.split(/[._]/)[0];
  return part.charAt(0).toUpperCase() + part.slice(1);
}

export function initialsFromEmail(email: string): string {
  const parts = email.split("@")[0].split(/[._]/);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return parts[0].slice(0, 2).toUpperCase();
}

export function formatRenewalDate(isoStr: string | null): string {
  if (!isoStr) return "";
  return new Date(isoStr).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
}

interface DayStats {
  streakDays: number;
  weekDots: boolean[];
}

export function computeStreak(entries: HistoryEntry[]): DayStats {
  const today = new Date();
  const dow = today.getDay();
  const mondayOffset = dow === 0 ? -6 : 1 - dow;

  const weekDots: boolean[] = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(today);
    d.setDate(today.getDate() + mondayOffset + i);
    const isPast = d <= today;
    const hit = isPast && entries.some((e) => new Date(e.timestamp_ms).toDateString() === d.toDateString());
    weekDots.push(hit);
  }

  let streak = 0;
  for (let i = 0; i < 60; i++) {
    const d = new Date(today);
    d.setDate(today.getDate() - i);
    if (entries.some((e) => new Date(e.timestamp_ms).toDateString() === d.toDateString())) {
      streak++;
    } else {
      break;
    }
  }
  return { streakDays: streak, weekDots };
}

interface WordStats {
  total: number;
  last7: number[];
  weekWords: number;
}

export function computeWordStats(entries: HistoryEntry[]): WordStats {
  const today = new Date();
  const last7: number[] = [];
  for (let i = 6; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(today.getDate() - i);
    const ds = d.toDateString();
    const words = entries.filter((e) => new Date(e.timestamp_ms).toDateString() === ds)
      .reduce((s, e) => s + e.output_word_count, 0);
    last7.push(words);
  }
  const total = entries.reduce((s, e) => s + e.output_word_count, 0);
  const weekWords = last7.reduce((a, b) => a + b, 0);
  return { total, last7, weekWords };
}

/* Time saved = 0.015h per rewrite — CONFIRM constant with user */
export const HOURS_SAVED_PER_REWRITE = 0.015;

export function formatHoursSaved(rewriteCount: number): string {
  // Work in whole minutes so rounding can never surface an impossible
  // "1h 60m" — carry the minute into the hour instead.
  const totalMin = Math.round(HOURS_SAVED_PER_REWRITE * rewriteCount * 60);
  if (totalMin < 60) return `${totalMin}m`;
  return `${Math.floor(totalMin / 60)}h ${totalMin % 60}m`;
}

interface ActivityDay {
  date: Date;
  count: number;
}

// Last 17 weeks (119 days) of activity, oldest first, today last — so
// rendering columns of 7 consecutive entries (top→bottom = chronological)
// naturally places the newest day in the bottom-right cell.
export function computeActivityHeatmap(entries: HistoryEntry[], totalDays = 119): { days: ActivityDay[] } {
  const today = new Date();
  today.setHours(0, 0, 0, 0);

  const counts = new Map<string, number>();
  for (const e of entries) {
    const d = new Date(e.timestamp_ms);
    d.setHours(0, 0, 0, 0);
    const key = d.toDateString();
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }

  const days: ActivityDay[] = [];
  for (let i = totalDays - 1; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(today.getDate() - i);
    days.push({ date: d, count: counts.get(d.toDateString()) ?? 0 });
  }
  return { days };
}

export function computeSkillUsage(entries: HistoryEntry[]): { name: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const e of entries) {
    counts.set(e.skill_name, (counts.get(e.skill_name) ?? 0) + 1);
  }
  return Array.from(counts.entries())
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count)
    .slice(0, 5);
}

export function groupByDate(entries: HistoryEntry[]): { label: string; items: HistoryEntry[] }[] {
  const todayStr = new Date().toDateString();
  const yestStr = new Date(Date.now() - 86400000).toDateString();

  const today = entries.filter((e) => new Date(e.timestamp_ms).toDateString() === todayStr);
  const yesterday = entries.filter((e) => new Date(e.timestamp_ms).toDateString() === yestStr);
  const earlier = entries.filter((e) => {
    const s = new Date(e.timestamp_ms).toDateString();
    return s !== todayStr && s !== yestStr;
  });

  return [
    ...(today.length ? [{ label: "Today", items: today }] : []),
    ...(yesterday.length ? [{ label: "Yesterday", items: yesterday }] : []),
    ...(earlier.length ? [{ label: "Earlier", items: earlier }] : []),
  ];
}
