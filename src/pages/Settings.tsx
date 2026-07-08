import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { check as checkForUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import type { Config, HistoryEntry, Skill, SkillsConfig } from "../types";

interface AuthState {
  logged_in: boolean;
  email: string;
  is_subscribed: boolean;
  subscription_valid_until: string | null;
  rewrite_count: number;
}
import { BUILTIN_SKILLS } from "../skills";
import logoBlack from "../assets/rewrite_logo_black.png";

// ── Constants ──────────────────────────────────────────────────────────────────

const ACCENT = "#16161a";
const FREE_TIER_MONTHLY_LIMIT = 3;

const BUILTIN_SKILL_OPTIONS = BUILTIN_SKILLS.map((b) => ({ id: b.id, name: b.name }));

// ── Helpers ────────────────────────────────────────────────────────────────────

function getGreeting(): string {
  const h = new Date().getHours();
  if (h < 12) return "Good morning";
  if (h < 17) return "Good afternoon";
  return "Good evening";
}

function hotkeyParts(raw: string): string[] {
  return raw.split("+").map((k) => k.trim().charAt(0).toUpperCase() + k.trim().slice(1));
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  const todayStr = new Date().toDateString();
  const yestStr = new Date(Date.now() - 86400000).toDateString();
  if (d.toDateString() === todayStr) {
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (d.toDateString() === yestStr) return "Yesterday";
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function truncate(text: string, max: number): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length > max ? clean.slice(0, max).trimEnd() + "…" : clean;
}

function firstNameFromEmail(email: string): string {
  const local = email.split("@")[0];
  const part = local.split(/[._]/)[0];
  return part.charAt(0).toUpperCase() + part.slice(1);
}

function initialsFromEmail(email: string): string {
  const parts = email.split("@")[0].split(/[._]/);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return parts[0].slice(0, 2).toUpperCase();
}

function formatRenewalDate(isoStr: string | null): string {
  if (!isoStr) return "";
  return new Date(isoStr).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
}

interface DayStats { streakDays: number; weekDots: boolean[] }
function computeStreak(entries: HistoryEntry[]): DayStats {
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

interface WordStats { total: number; last7: number[]; weekWords: number }
function computeWordStats(entries: HistoryEntry[]): WordStats {
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

function groupByDate(entries: HistoryEntry[]): { label: string; items: HistoryEntry[] }[] {
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

// ── Toggle ─────────────────────────────────────────────────────────────────────

function Toggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <div
      onClick={(e) => { e.stopPropagation(); onToggle(); }}
      style={{
        width: 42, height: 24, borderRadius: 13, cursor: "pointer",
        position: "relative", transition: "background .18s", flexShrink: 0,
        background: on ? ACCENT : "#d7d8dc",
      }}
    >
      <div style={{
        position: "absolute", top: 3, left: on ? 21 : 3,
        width: 18, height: 18, borderRadius: "50%",
        background: "#fff", boxShadow: "0 1px 3px rgba(0,0,0,.25)",
        transition: "left .18s",
      }} />
    </div>
  );
}

// ── Icons ──────────────────────────────────────────────────────────────────────

const IconHome = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
    <rect x="3.5" y="3.5" width="7" height="7" rx="1.6" />
    <rect x="13.5" y="3.5" width="7" height="7" rx="1.6" />
    <rect x="3.5" y="13.5" width="7" height="7" rx="1.6" />
    <rect x="13.5" y="13.5" width="7" height="7" rx="1.6" />
  </svg>
);

const IconHistory = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3.2 12a8.8 8.8 0 1 0 2.9-6.5L3 8" />
    <path d="M3 4v4h4" />
    <path d="M12 7.4V12l3 1.8" />
  </svg>
);

const IconBook = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3 5.2A2.2 2.2 0 0 1 5.2 3H12v16.5H5.2A2.2 2.2 0 0 0 3 21.7z" />
    <path d="M21 5.2A2.2 2.2 0 0 0 18.8 3H12v16.5h6.8A2.2 2.2 0 0 1 21 21.7z" />
  </svg>
);

const IconGear = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <circle cx="12" cy="12" r="3.1" />
    <path d="M19.1 14.5a1.55 1.55 0 0 0 .31 1.71l.05.05a2 2 0 1 1-2.83 2.83l-.05-.05a1.55 1.55 0 0 0-1.71-.31 1.55 1.55 0 0 0-.94 1.42V20a2 2 0 0 1-4 0v-.07a1.55 1.55 0 0 0-1.02-1.42 1.55 1.55 0 0 0-1.71.31l-.05.05a2 2 0 1 1-2.83-2.83l.05-.05a1.55 1.55 0 0 0 .31-1.71 1.55 1.55 0 0 0-1.42-.94H4a2 2 0 0 1 0-4h.07a1.55 1.55 0 0 0 1.42-1.02 1.55 1.55 0 0 0-.31-1.71l-.05-.05a2 2 0 1 1 2.83-2.83l.05.05a1.55 1.55 0 0 0 1.71.31H9.7a1.55 1.55 0 0 0 .94-1.42V4a2 2 0 0 1 4 0v.07a1.55 1.55 0 0 0 .94 1.42 1.55 1.55 0 0 0 1.71-.31l.05-.05a2 2 0 1 1 2.83 2.83l-.05.05a1.55 1.55 0 0 0-.31 1.71v.06a1.55 1.55 0 0 0 1.42.94H20a2 2 0 0 1 0 4h-.07a1.55 1.55 0 0 0-1.42.94z" />
  </svg>
);

const IconLock = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
    <path d="M8 10.5V7a4 4 0 0 1 8 0v3.5" />
  </svg>
);

// ── Login view ────────────────────────────────────────────────────────────────

function LoginView({ onLogin }: { onLogin: () => void }) {
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = email.trim();
    if (!trimmed) return;
    setLoading(true);
    setError(null);
    try {
      await invoke("send_magic_link", { email: trimmed });
      setSent(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  // Listen for auth:complete while this view is shown
  useEffect(() => {
    const unlisten = listen("auth:complete", () => onLogin());
    return () => { unlisten.then((fn) => fn()); };
  }, [onLogin]);

  return (
    <div style={{ display: "flex", height: "100vh", alignItems: "center", justifyContent: "center", background: "#e6e7ea", fontFamily: "'Hanken Grotesk', system-ui, sans-serif" }}>
      <div style={{ background: "#fff", borderRadius: 20, padding: "48px 44px", width: 400, boxShadow: "0 8px 40px rgba(20,20,26,.10)" }}>
        <div style={{ textAlign: "center", marginBottom: 36 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "center", marginBottom: 24 }}>
            <img src={logoBlack} alt="reWrite" style={{ height: 52, width: "auto" }} />
          </div>
          <h2 style={{ fontFamily: "'Playfair Display', serif", fontSize: 26, fontWeight: 700, color: "#16161a", marginBottom: 8 }}>Welcome to reWrite</h2>
          <p style={{ fontSize: 14.5, color: "#74777e" }}>Enter your email to sign in or create an account.</p>
        </div>

        {sent ? (
          <div style={{ textAlign: "center", padding: "24px 0" }}>
            <div style={{ width: 52, height: 52, borderRadius: "50%", background: "#16161a", display: "flex", alignItems: "center", justifyContent: "center", margin: "0 auto 18px" }}>
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#fff" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z" /><polyline points="22,6 12,13 2,6" /></svg>
            </div>
            <div style={{ fontSize: 17, fontWeight: 600, color: "#16161a", marginBottom: 8 }}>Check your email</div>
            <div style={{ fontSize: 14, color: "#74777e", lineHeight: 1.5 }}>
              We sent a magic link to <strong>{email}</strong>.<br />Click it to sign in, and this window will update automatically.
            </div>
            <button onClick={() => { setSent(false); setEmail(""); }} style={{ marginTop: 22, fontSize: 13.5, color: "#86898f", background: "none", border: "none", cursor: "pointer", fontFamily: "inherit", textDecoration: "underline" }}>
              Use a different email
            </button>
          </div>
        ) : (
          <form onSubmit={handleSubmit} style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="your@email.com"
              autoFocus
              style={{ border: "1px solid #e0e1e4", borderRadius: 10, padding: "13px 15px", fontSize: 15, color: "#16161a", outline: "none", fontFamily: "inherit" }}
            />
            {error && <div style={{ fontSize: 12.5, color: "#c0392b" }}>{error}</div>}
            <button
              type="submit"
              disabled={!email.trim() || loading}
              style={{ background: email.trim() ? "#16161a" : "#9a9da3", color: "#fff", border: "none", borderRadius: 10, padding: "13px", fontSize: 15, fontWeight: 600, cursor: email.trim() ? "pointer" : "not-allowed", fontFamily: "inherit", transition: "background .15s" }}
            >
              {loading ? "Sending…" : "Send magic link"}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}

// ── NavButton ──────────────────────────────────────────────────────────────────

type ActiveView = "home" | "history" | "skills" | "settings";

function NavButton({ label, icon, active, onClick, locked }: { label: string; icon: React.ReactNode; active: boolean; onClick: () => void; locked?: boolean }) {
  const [hov, setHov] = useState(false);
  return (
    <button
      onClick={onClick}
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{
        display: "flex", alignItems: "center", gap: 13, width: "100%",
        padding: "11px 14px", borderRadius: 11,
        fontFamily: "'Hanken Grotesk', sans-serif", fontSize: 15.5, fontWeight: 500,
        cursor: "pointer", textAlign: "left", transition: "background .15s, color .15s",
        background: active ? "#fff" : hov ? "rgba(0,0,0,.04)" : "transparent",
        color: locked ? "#a7aab0" : active ? "#16161a" : "#44464d",
        border: active ? "1px solid #e3e4e7" : "1px solid transparent",
        boxShadow: active ? "0 1px 2px rgba(20,20,26,.10)" : "none",
      }}
    >
      {icon}
      <span style={{ flex: 1 }}>{label}</span>
      {locked && <IconLock />}
    </button>
  );
}

// ── Sidebar ────────────────────────────────────────────────────────────────────

function Sidebar({ active, setActive, authState }: { active: ActiveView; setActive: (v: ActiveView) => void; authState: AuthState }) {
  return (
    <aside style={{ width: 250, minWidth: 250, background: "#e6e7ea", borderRight: "1px solid #dcdde1", display: "flex", flexDirection: "column", padding: "30px 20px 22px" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "6px 4px 30px" }}>
        <img src={logoBlack} alt="reWrite" style={{ height: 58, width: "auto" }} />
      </div>
      <nav style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        <NavButton label="Home"     icon={<IconHome />}    active={active === "home"}     onClick={() => setActive("home")} />
        <NavButton label="History"  icon={<IconHistory />} active={active === "history"}  onClick={() => setActive("history")} />
        <NavButton label="Skills"   icon={<IconBook />}    active={active === "skills"}   onClick={() => setActive("skills")} locked={!authState.is_subscribed} />
        <NavButton label="Settings" icon={<IconGear />}    active={active === "settings"} onClick={() => setActive("settings")} />
      </nav>
      <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: 16 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 11, padding: "9px 11px", borderRadius: 11, background: "#dddee2" }}>
          <div style={{ width: 34, height: 34, borderRadius: "50%", background: "#16161a", color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", fontWeight: 600, fontSize: 13, letterSpacing: .3, flexShrink: 0 }}>
            {initialsFromEmail(authState.email)}
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 13.5, fontWeight: 600, color: "#1f2026", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{authState.email}</div>
            <div style={{ fontSize: 11.5, color: "#83868d" }}>{authState.is_subscribed ? "reWrite Pro" : "Free plan"}</div>
          </div>
        </div>
        <div style={{ fontFamily: "'Playfair Display', serif", fontStyle: "italic", fontSize: 13, color: "#9a9da3", paddingLeft: 4 }}>Version 1.0.5</div>
      </div>
    </aside>
  );
}

// ── Home View ──────────────────────────────────────────────────────────────────

function HomeView({ history, skillsConfig, config, authState }: { history: HistoryEntry[]; skillsConfig: SkillsConfig; config: Config; authState: AuthState }) {
  const greet = getGreeting();
  const { streakDays, weekDots } = computeStreak(history);
  const { total, last7, weekWords } = computeWordStats(history);
  const maxBar = Math.max(...last7, 1);

  const DAY_LABELS = ["M", "T", "W", "T", "F", "S", "S"];

  const onboarding = [
    { done: history.length > 0,                                                    label: "reWrite your first email" },
    { done: skillsConfig.skills.length > 0,                                        label: "Craft your first skill" },
    { done: config.super_hotkey !== "ctrl+shift+period",                           label: "Set up your super hotkey" },
    { done: history.some((e) => e.skill_id === "__summarise__"),                   label: "Summarise your meeting notes" },
  ];
  const doneCount = onboarding.filter((t) => t.done).length;

  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      <header style={{ marginBottom: 34 }}>
        <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 50, lineHeight: 1.02, color: "#16161a", letterSpacing: -.5 }}>
          {greet}, {firstNameFromEmail(authState.email)}
        </h1>
        <p style={{ fontSize: 16, color: "#74777e", marginTop: 10 }}>Let's knock something off your to-do list.</p>
      </header>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 20, marginBottom: 20 }}>
        {/* Streak */}
        <div style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 16, padding: "26px 28px" }}>
          <div style={{ fontSize: 13.5, color: "#8a8d93", letterSpacing: .2 }}>Streak</div>
          <div style={{ fontSize: 31, fontWeight: 700, color: "#16161a", marginTop: 7, display: "flex", alignItems: "center", gap: 9 }}>
            {streakDays > 0 ? `${streakDays} day${streakDays !== 1 ? "s" : ""} streak` : "No streak yet"}{" "}
            {streakDays >= 7 ? "🔥" : streakDays > 0 ? "✨" : ""}
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 22 }}>
            {weekDots.map((done, i) => (
              <div key={i} style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 7 }}>
                <div style={{ width: 24, height: 24, borderRadius: "50%", background: done ? ACCENT : "#fff", border: done ? "none" : "2px dashed #c9cbd0" }} />
                <span style={{ fontSize: 11, color: "#a7aab0", fontWeight: 600 }}>{DAY_LABELS[i]}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Words written */}
        <div style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 16, padding: "26px 28px" }}>
          <div style={{ fontSize: 13.5, color: "#8a8d93", letterSpacing: .2 }}>Total words written</div>
          <div style={{ fontSize: 31, fontWeight: 700, color: "#16161a", marginTop: 7, fontVariantNumeric: "tabular-nums" }}>
            {total.toLocaleString()}
          </div>
          <div style={{ display: "flex", alignItems: "flex-end", gap: 5, height: 34, marginTop: 18 }}>
            {last7.map((h, i) => {
              const pct = maxBar > 0 ? Math.max((h / maxBar) * 100, h > 0 ? 8 : 0) : 0;
              const isRecent = i >= 5;
              return (
                <div key={i} style={{ flex: 1, height: `${pct || 4}%`, background: isRecent ? ACCENT : "#e6e7ea", borderRadius: 3, opacity: pct === 0 ? .35 : 1 }} />
              );
            })}
          </div>
          <div style={{ fontSize: 12.5, color: "#8a8d93", marginTop: 12 }}>
            {weekWords > 0
              ? <><span style={{ color: ACCENT, fontWeight: 600 }}>+{weekWords.toLocaleString()}</span> this week</>
              : "No rewrites this week yet"}
          </div>
        </div>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 20 }}>
        {/* Onboarding */}
        <div style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 16, padding: "26px 28px" }}>
          <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", marginBottom: 20 }}>
            <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 21, color: "#16161a" }}>Get to know reWrite</h3>
            <span style={{ fontSize: 12.5, color: "#9a9da3", fontWeight: 500 }}>{doneCount} of {onboarding.length}</span>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
            {onboarding.map((t, i) => (
              <div key={i} style={{ display: "flex", alignItems: "center", gap: 13, padding: "9px 4px" }}>
                {t.done ? (
                  <span style={{ width: 22, height: 22, borderRadius: "50%", background: ACCENT, display: "flex", alignItems: "center", justifyContent: "center", flexShrink: 0 }}>
                    <svg width="12" height="12" viewBox="0 0 14 14" fill="none" stroke="#fff" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M3 7.5 6 10.5 11.5 4" /></svg>
                  </span>
                ) : (
                  <span style={{ width: 22, height: 22, borderRadius: "50%", border: "2px solid #d2d4d8", flexShrink: 0 }} />
                )}
                <span style={{ fontSize: 15, color: t.done ? "#a7aab0" : "#2c2e34", textDecoration: t.done ? "line-through" : "none" }}>{t.label}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Video placeholder */}
        <div style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 16, padding: "14px 14px 22px", display: "flex", flexDirection: "column" }}>
          <div style={{ position: "relative", width: "100%", height: 208, borderRadius: 11, overflow: "hidden", background: "repeating-linear-gradient(135deg,#eef0f2 0 13px,#e7e9ec 13px 26px)", display: "flex", alignItems: "center", justifyContent: "center" }}>
            <span style={{ position: "absolute", top: 12, left: 14, fontFamily: "monospace", fontSize: 11, letterSpacing: .5, color: "#aeb1b7" }}>intro video · 0:48</span>
            <div style={{ width: 60, height: 60, borderRadius: "50%", background: "rgba(22,22,26,.92)", display: "flex", alignItems: "center", justifyContent: "center", boxShadow: "0 6px 18px rgba(20,20,26,.25)" }}>
              <svg width="22" height="22" viewBox="0 0 24 24" fill="#fff"><path d="M8 5.5v13l11-6.5z" /></svg>
            </div>
          </div>
          <div style={{ padding: "18px 14px 2px" }}>
            <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 21, color: "#16161a" }}>Introducing reWrite</h3>
            <p style={{ fontSize: 14.5, color: "#74777e", marginTop: 6, lineHeight: 1.45 }}>Your daily tasks, made easier, every word in your voice.</p>
          </div>
        </div>
      </div>

      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 8, marginTop: 28, fontSize: 13, color: "#9a9da3" }}>
        <span>reWrite lives in your taskbar — when you need it, fire up</span>
        <div style={{ display: "flex", alignItems: "center", gap: 5 }}>
          {hotkeyParts(config.hotkey).map((k, i) => (
            <kbd key={i} style={{ fontFamily: "'Hanken Grotesk', sans-serif", fontSize: 11.5, fontWeight: 600, color: "#5b5e66", background: "#f3f4f5", border: "1px solid #e2e3e6", borderBottomWidth: 2, borderRadius: 6, padding: "3px 7px" }}>{k}</kbd>
          ))}
        </div>
        <span>or click the taskbar icon.</span>
      </div>
    </div>
  );
}

// ── History View ───────────────────────────────────────────────────────────────

function HistoryItemRow({ entry }: { entry: HistoryEntry }) {
  const [hov, setHov] = useState(false);
  const title = truncate(entry.input_text.split("\n")[0], 55) || "Untitled";
  const preview = truncate(entry.output_text, 90);
  const timeStr = formatTime(entry.timestamp_ms);
  const words = `${entry.output_word_count} word${entry.output_word_count !== 1 ? "s" : ""}`;

  return (
    <div
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{
        display: "flex", alignItems: "center", gap: 18,
        background: "#fff", border: `1px solid ${hov ? "#d6d8dc" : "#e8e9ec"}`,
        borderRadius: 13, padding: "17px 20px", cursor: "pointer",
        transition: "border-color .14s, box-shadow .14s",
        boxShadow: hov ? "0 3px 10px rgba(20,20,26,.05)" : "none",
      }}
    >
      <span style={{ fontFamily: "monospace", fontSize: 11, fontWeight: 600, letterSpacing: .4, textTransform: "uppercase", color: "#5b5e66", background: "#f1f2f4", border: "1px solid #e6e7ea", padding: "5px 9px", borderRadius: 7, flexShrink: 0, minWidth: 88, textAlign: "center" }}>
        {entry.skill_name}
      </span>
      <div style={{ minWidth: 0, flex: 1 }}>
        <div style={{ fontSize: 15, fontWeight: 600, color: "#1f2026", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{title}</div>
        <div style={{ fontSize: 13.5, color: "#86898f", marginTop: 3, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{preview}</div>
      </div>
      <div style={{ textAlign: "right", flexShrink: 0 }}>
        <div style={{ fontSize: 13, color: "#5b5e66", fontWeight: 500 }}>{words}</div>
        <div style={{ fontSize: 12.5, color: "#a7aab0", marginTop: 3 }}>{timeStr}</div>
      </div>
    </div>
  );
}

function HistoryView({ history }: { history: HistoryEntry[] }) {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");

  const skillIds = new Set(history.map((e) => e.skill_id));
  const chips: { k: string; label: string }[] = [
    { k: "all", label: "All" },
    ...BUILTIN_SKILLS.filter((b) => skillIds.has(b.id)).map((b) => ({ k: b.id, label: b.name })),
    ...[...skillIds].some((id) => !id.startsWith("__")) ? [{ k: "custom", label: "Custom" }] : [],
  ];

  const filtered = history.filter((e) => {
    if (filter !== "all") {
      if (filter === "custom" && e.skill_id.startsWith("__")) return false;
      if (filter !== "custom" && e.skill_id !== filter) return false;
    }
    if (search.trim()) {
      const q = search.toLowerCase();
      return e.input_text.toLowerCase().includes(q) || e.output_text.toLowerCase().includes(q) || e.skill_name.toLowerCase().includes(q);
    }
    return true;
  });

  const groups = groupByDate(filtered);

  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      <header style={{ marginBottom: 26 }}>
        <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 46, lineHeight: 1.02, color: "#16161a", letterSpacing: -.5 }}>History</h1>
        <p style={{ fontSize: 16, color: "#74777e", marginTop: 9 }}>Every piece you've re:Written, kept close.</p>
      </header>

      <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: 22 }}>
        <div style={{ flex: 1, display: "flex", alignItems: "center", gap: 11, background: "#f4f5f6", border: "1px solid #e8e9ec", borderRadius: 11, padding: "11px 15px" }}>
          <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="#9a9da3" strokeWidth="1.8" strokeLinecap="round"><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg>
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search your history…"
            style={{ background: "none", border: "none", outline: "none", fontSize: 14.5, color: "#1f2026", flex: 1, fontFamily: "inherit" }}
          />
          {search && (
            <button onClick={() => setSearch("")} style={{ background: "none", border: "none", cursor: "pointer", color: "#a7aab0", fontSize: 16, lineHeight: 1, padding: 0, fontFamily: "inherit" }}>×</button>
          )}
        </div>
        <div style={{ display: "flex", gap: 7, flexWrap: "wrap" }}>
          {chips.map((c) => {
            const isActive = filter === c.k;
            return (
              <button key={c.k} onClick={() => setFilter(c.k)} style={{ padding: "9px 16px", borderRadius: 9, fontSize: 13.5, fontWeight: 500, cursor: "pointer", transition: "all .14s", background: isActive ? "#16161a" : "#fff", color: isActive ? "#fff" : "#5b5e66", border: `1px solid ${isActive ? "#16161a" : "#e3e4e7"}` }}>
                {c.label}
              </button>
            );
          })}
        </div>
      </div>

      {history.length === 0 ? (
        <div style={{ textAlign: "center", padding: "80px 40px", color: "#a7aab0" }}>
          <div style={{ fontFamily: "'Playfair Display', serif", fontSize: 22, fontWeight: 600, color: "#16161a", marginBottom: 8 }}>Nothing here yet</div>
          <div style={{ fontSize: 14.5, color: "#74777e" }}>Your rewrites will appear here after you use reWrite for the first time.</div>
        </div>
      ) : groups.length === 0 ? (
        <div style={{ textAlign: "center", padding: "60px 40px", color: "#a7aab0", fontSize: 14.5 }}>No results match your filter.</div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 26 }}>
          {groups.map((group, gi) => (
            <div key={gi}>
              <div style={{ fontSize: 12, fontWeight: 600, letterSpacing: 1.2, textTransform: "uppercase", color: "#a7aab0", marginBottom: 11 }}>{group.label}</div>
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {group.items.map((entry) => <HistoryItemRow key={entry.id} entry={entry} />)}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Skills View ────────────────────────────────────────────────────────────────

function BuiltinSkillCard({ name, desc, enabled, onToggle }: { name: string; desc: string; enabled: boolean; onToggle: () => void }) {
  return (
    <div style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 15, padding: "24px 24px 22px", display: "flex", flexDirection: "column" }}>
      <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 14 }}>
        <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 22, color: "#16161a", lineHeight: 1.15 }}>{name}</h3>
        <Toggle on={enabled} onToggle={onToggle} />
      </div>
      <p style={{ fontSize: 14, color: "#74777e", lineHeight: 1.5, marginTop: 9, minHeight: 42 }}>{desc}</p>
      <div style={{ marginTop: 16 }}>
        <span style={{ fontSize: 11.5, fontWeight: 500, color: "#5b5e66", background: "#f3f4f5", border: "1px solid #ebecee", padding: "4px 10px", borderRadius: 7 }}>Built-in</span>
      </div>
    </div>
  );
}

function CustomSkillCard({ skill, onToggle, onEdit, onDelete }: { skill: Skill; onToggle: () => void; onEdit: () => void; onDelete: () => void }) {
  const [hov, setHov] = useState(false);
  const baseName = BUILTIN_SKILL_OPTIONS.find((b) => b.id === skill.base_skill_id)?.name;
  const tags = baseName ? [baseName, "Custom"] : ["Custom"];
  const desc = skill.instructions.trim() || "No instructions provided.";

  return (
    <div
      onMouseEnter={() => setHov(true)}
      onMouseLeave={() => setHov(false)}
      style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 15, padding: "24px 24px 22px", display: "flex", flexDirection: "column", position: "relative", transition: "box-shadow .14s", boxShadow: hov ? "0 4px 16px rgba(20,20,26,.08)" : "none" }}
    >
      {hov && (
        <div style={{ position: "absolute", bottom: 16, right: 16, display: "flex", gap: 6, background: "rgba(255,255,255,.92)", backdropFilter: "blur(2px)", padding: 4, borderRadius: 9, boxShadow: "0 2px 8px rgba(20,20,26,.1)" }}>
          <button onClick={onEdit} title="Edit" style={{ width: 28, height: 28, borderRadius: 7, background: "#f3f4f5", border: "1px solid #e6e7ea", cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center", color: "#5b5e66" }}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" /><path d="M18.5 2.5a2.12 2.12 0 0 1 3 3L12 15l-4 1 1-4z" /></svg>
          </button>
          <button onClick={onDelete} title="Delete" style={{ width: 28, height: 28, borderRadius: 7, background: "#fdf1f1", border: "1px solid #fad8d8", cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center", color: "#c0392b" }}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><polyline points="3 6 5 6 21 6" /><path d="M19 6l-1 14H6L5 6" /><path d="M10 11v6m4-6v6" /><path d="M9 6V4h6v2" /></svg>
          </button>
        </div>
      )}
      <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 14 }}>
        <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 22, color: "#16161a", lineHeight: 1.15 }}>{skill.name}</h3>
        <Toggle on={skill.enabled} onToggle={onToggle} />
      </div>
      <p style={{ fontSize: 14, color: "#74777e", lineHeight: 1.5, marginTop: 9, minHeight: 42, overflow: "hidden", maxHeight: "3.15em" }}>{desc}</p>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 7, marginTop: 16, paddingRight: 84 }}>
        {tags.map((t, i) => (
          <span key={i} style={{ fontSize: 11.5, fontWeight: 500, color: "#5b5e66", background: "#f3f4f5", border: "1px solid #ebecee", padding: "4px 10px", borderRadius: 7 }}>{t}</span>
        ))}
      </div>
    </div>
  );
}

function CreateSkillCard({ onClick }: { onClick: () => void }) {
  const [hov, setHov] = useState(false);
  return (
    <div onClick={onClick} onMouseEnter={() => setHov(true)} onMouseLeave={() => setHov(false)}
      style={{ border: "1.5px dashed #d2d4d8", borderRadius: 15, padding: 24, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 11, cursor: "pointer", minHeight: 160, transition: "border-color .14s, background .14s", borderColor: hov ? "#16161a" : "#d2d4d8", background: hov ? "#fafafb" : "transparent" }}
    >
      <span style={{ width: 42, height: 42, borderRadius: "50%", background: "#f1f2f4", display: "flex", alignItems: "center", justifyContent: "center" }}>
        <svg width="20" height="20" viewBox="0 0 24 24" stroke="#5b5e66" strokeWidth="1.8" strokeLinecap="round" fill="none"><line x1="12" y1="6" x2="12" y2="18" /><line x1="6" y1="12" x2="18" y2="12" /></svg>
      </span>
      <span style={{ fontSize: 14.5, fontWeight: 500, color: "#5b5e66" }}>Create a new skill</span>
    </div>
  );
}

function SkillModal({ skill, allSkills, onSave, onClose, error }: { skill: Skill | null; allSkills: Skill[]; onSave: (name: string, instructions: string, baseSkillId: string) => void; onClose: () => void; error: string | null }) {
  const [name, setName] = useState(skill?.name ?? "");
  const [instructions, setInstructions] = useState(skill?.instructions ?? "");
  const [baseSkillId, setBaseSkillId] = useState<string>(skill?.base_skill_id ?? "");
  const nameRef = useRef<HTMLInputElement>(null);
  const otherSkills = skill ? allSkills.filter((s) => s.id !== skill.id) : allSkills;

  useEffect(() => { nameRef.current?.focus(); }, []);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    onSave(name.trim(), instructions, baseSkillId);
  }

  return (
    <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.45)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 50 }}>
      <div style={{ background: "#fff", borderRadius: 16, padding: 28, width: 460, boxShadow: "0 20px 60px rgba(0,0,0,.2)" }}>
        <div style={{ fontFamily: "'Playfair Display', serif", fontSize: 22, fontWeight: 600, color: "#16161a", marginBottom: 22 }}>{skill ? "Edit skill" : "New skill"}</div>
        <form onSubmit={handleSubmit} style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <div>
            <label style={{ fontSize: 12, fontWeight: 600, color: "#74777e", letterSpacing: .5, display: "block", marginBottom: 6, textTransform: "uppercase" }}>Name</label>
            <input ref={nameRef} value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Slack Casual" style={{ width: "100%", border: "1px solid #e0e1e4", borderRadius: 9, padding: "10px 13px", fontSize: 14.5, color: "#16161a", outline: "none", boxSizing: "border-box", fontFamily: "inherit" }} />
          </div>
          <div>
            <label style={{ fontSize: 12, fontWeight: 600, color: "#74777e", letterSpacing: .5, display: "block", marginBottom: 6, textTransform: "uppercase" }}>Base skill <span style={{ fontWeight: 400, color: "#9a9da3", textTransform: "none" }}>(optional)</span></label>
            <select value={baseSkillId} onChange={(e) => setBaseSkillId(e.target.value)} style={{ width: "100%", border: "1px solid #e0e1e4", borderRadius: 9, padding: "10px 13px", fontSize: 14.5, color: "#16161a", background: "#fff", outline: "none", cursor: "pointer", boxSizing: "border-box", fontFamily: "inherit" }}>
              <option value="">None</option>
              <optgroup label="Built-in">
                {BUILTIN_SKILL_OPTIONS.map((b) => <option key={b.id} value={b.id}>{b.name}</option>)}
              </optgroup>
              {otherSkills.length > 0 && (
                <optgroup label="Custom">
                  {otherSkills.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
                </optgroup>
              )}
            </select>
            <div style={{ fontSize: 12, color: "#9a9da3", marginTop: 5 }}>Your instructions stack on top of the selected base skill.</div>
          </div>
          <div>
            <label style={{ fontSize: 12, fontWeight: 600, color: "#74777e", letterSpacing: .5, display: "block", marginBottom: 6, textTransform: "uppercase" }}>Instructions</label>
            <textarea value={instructions} onChange={(e) => setInstructions(e.target.value)} placeholder="Describe how this skill should rewrite text…" rows={4} style={{ width: "100%", border: "1px solid #e0e1e4", borderRadius: 9, padding: "10px 13px", fontSize: 14.5, color: "#16161a", outline: "none", resize: "vertical", fontFamily: "inherit", boxSizing: "border-box" }} />
          </div>
          {error && <div style={{ fontSize: 12.5, color: "#c0392b" }}>{error}</div>}
          <div style={{ display: "flex", gap: 10, justifyContent: "flex-end", marginTop: 4 }}>
            <button type="button" onClick={onClose} style={{ fontSize: 13.5, color: "#74777e", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 18px", cursor: "pointer", fontFamily: "inherit" }}>Cancel</button>
            <button type="submit" disabled={!name.trim()} style={{ fontSize: 13.5, color: "#fff", background: name.trim() ? "#16161a" : "#9a9da3", border: "none", borderRadius: 9, padding: "10px 18px", cursor: name.trim() ? "pointer" : "not-allowed", fontFamily: "inherit", fontWeight: 600 }}>
              {skill ? "Save changes" : "Create skill"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function SkillsLockedView() {
  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", textAlign: "center", padding: "70px 40px", border: "1px solid #e8e9ec", borderRadius: 15, background: "#fbfbfc" }}>
        <div style={{ width: 52, height: 52, borderRadius: "50%", background: "#16161a", color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", marginBottom: 20 }}>
          <IconLock />
        </div>
        <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 28, color: "#16161a", marginBottom: 8 }}>Skills are a Pro feature</h1>
        <p style={{ fontSize: 14.5, color: "#74777e", maxWidth: 380, marginBottom: 26, lineHeight: 1.5 }}>
          Creating custom skills and managing built-ins is available on Pro and Max plans. Free plan rewrites still use the built-in skills from the overlay.
        </p>
        <div style={{ display: "flex", gap: 10 }}>
          <button onClick={() => invoke("open_checkout", { plan: "pro" })} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#16161a", border: "none", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Upgrade to Pro</button>
          <button onClick={() => invoke("open_checkout", { plan: "max" })} style={{ fontSize: 13.5, fontWeight: 600, color: "#16161a", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Upgrade to Max</button>
        </div>
      </div>
    </div>
  );
}

function SkillsView() {
  const [config, setConfig] = useState<SkillsConfig>({ global_instructions: "", skills: [], builtin_enabled: {} });
  const [showCreate, setShowCreate] = useState(false);
  const [editingSkill, setEditingSkill] = useState<Skill | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    invoke<SkillsConfig>("get_skills_config").then((cfg) => {
      setConfig({ ...cfg, skills: [...cfg.skills].sort((a, b) => a.order - b.order) });
    });
  }, []);

  function isBuiltinEnabled(id: string): boolean {
    return config.builtin_enabled[id] !== false;
  }

  async function handleToggleBuiltin(id: string) {
    const newEnabled = !isBuiltinEnabled(id);
    const updated = { ...config, builtin_enabled: { ...config.builtin_enabled, [id]: newEnabled } };
    setConfig(updated);
    try {
      await invoke("toggle_builtin_skill", { id, enabled: newEnabled });
    } catch (err) {
      setSaveError(String(err));
    }
  }

  async function handleToggleCustom(id: string) {
    const updated = { ...config, skills: config.skills.map((s) => s.id === id ? { ...s, enabled: !s.enabled } : s) };
    setConfig(updated);
    try {
      await invoke("save_skills_config", { config: updated });
    } catch (err) {
      setSaveError(String(err));
    }
  }

  async function handleCreate(name: string, instructions: string, baseSkillId: string) {
    try {
      const skill = await invoke<Skill>("create_skill", { name, instructions, baseSkillId: baseSkillId || null });
      setConfig((prev) => ({ ...prev, skills: [...prev.skills, skill] }));
      setShowCreate(false);
      setSaveError(null);
    } catch (err) {
      setSaveError(String(err));
    }
  }

  async function handleEdit(id: string, name: string, instructions: string, baseSkillId: string) {
    const updated = { ...config, skills: config.skills.map((s) => s.id === id ? { ...s, name, instructions, base_skill_id: baseSkillId || null } : s) };
    setConfig(updated);
    setEditingSkill(null);
    try {
      await invoke("save_skills_config", { config: updated });
    } catch (err) {
      setSaveError(String(err));
    }
  }

  async function handleDelete(id: string) {
    try {
      await invoke("delete_skill", { id });
      setConfig((prev) => ({ ...prev, skills: prev.skills.filter((s) => s.id !== id) }));
      setSaveError(null);
    } catch (err) {
      setSaveError(String(err));
    }
  }

  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      {deleteConfirmId && (
        <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,.45)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 50 }}>
          <div style={{ background: "#fff", borderRadius: 16, padding: 28, width: 320, boxShadow: "0 20px 60px rgba(0,0,0,.2)" }}>
            <div style={{ fontFamily: "'Playfair Display', serif", fontSize: 20, fontWeight: 600, color: "#16161a", marginBottom: 8 }}>Delete skill?</div>
            <div style={{ fontSize: 13.5, color: "#74777e", marginBottom: 24 }}>This will permanently remove the skill.</div>
            <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
              <button onClick={() => setDeleteConfirmId(null)} style={{ fontSize: 13.5, color: "#74777e", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "9px 16px", cursor: "pointer", fontFamily: "inherit" }}>Cancel</button>
              <button onClick={async () => { await handleDelete(deleteConfirmId); setDeleteConfirmId(null); }} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#c0392b", border: "none", borderRadius: 9, padding: "9px 16px", cursor: "pointer", fontFamily: "inherit" }}>Delete</button>
            </div>
          </div>
        </div>
      )}

      {(showCreate || editingSkill) && (
        <SkillModal
          skill={editingSkill}
          allSkills={config.skills}
          onSave={editingSkill ? (n, i, b) => handleEdit(editingSkill.id, n, i, b) : handleCreate}
          onClose={() => { setShowCreate(false); setEditingSkill(null); setSaveError(null); }}
          error={saveError}
        />
      )}

      <header style={{ display: "flex", alignItems: "flex-end", justifyContent: "space-between", marginBottom: 24 }}>
        <div>
          <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 46, lineHeight: 1.02, color: "#16161a", letterSpacing: -.5 }}>Skills</h1>
          <p style={{ fontSize: 16, color: "#74777e", marginTop: 9 }}>Teach reWrite the voice you want. Toggle to show or hide in the overlay.</p>
        </div>
        <button onClick={() => setShowCreate(true)} style={{ display: "flex", alignItems: "center", gap: 9, background: "#16161a", color: "#fff", borderRadius: 11, padding: "12px 18px", fontSize: 14.5, fontWeight: 600, cursor: "pointer", whiteSpace: "nowrap", border: "none", fontFamily: "inherit" }}>
          <svg width="16" height="16" viewBox="0 0 24 24" stroke="#fff" strokeWidth="2" strokeLinecap="round" fill="none"><line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" /></svg>
          New skill
        </button>
      </header>

      {saveError && !showCreate && !editingSkill && (
        <div style={{ fontSize: 13, color: "#c0392b", marginBottom: 16 }}>{saveError}</div>
      )}

      {/* Built-in skills section */}
      <div style={{ marginBottom: 28 }}>
        <div style={{ fontSize: 12, fontWeight: 600, letterSpacing: 1.1, textTransform: "uppercase", color: "#a7aab0", marginBottom: 14 }}>Built-in</div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
          {BUILTIN_SKILLS.map((b) => (
            <BuiltinSkillCard
              key={b.id}
              name={b.name}
              desc={b.description}
              enabled={isBuiltinEnabled(b.id)}
              onToggle={() => handleToggleBuiltin(b.id)}
            />
          ))}
        </div>
      </div>

      {/* Custom skills section */}
      <div>
        <div style={{ fontSize: 12, fontWeight: 600, letterSpacing: 1.1, textTransform: "uppercase", color: "#a7aab0", marginBottom: 14 }}>Custom</div>
        {config.skills.length === 0 ? (
          <div style={{ textAlign: "center", padding: "48px 40px", border: "1.5px dashed #d2d4d8", borderRadius: 15 }}>
            <div style={{ fontFamily: "'Playfair Display', serif", fontSize: 20, fontWeight: 600, color: "#16161a", marginBottom: 8 }}>No custom skills yet</div>
            <div style={{ fontSize: 14.5, color: "#74777e", marginBottom: 22 }}>Create your first skill to extend reWrite with your own voice.</div>
            <button onClick={() => setShowCreate(true)} style={{ fontSize: 14.5, fontWeight: 600, color: "#16161a", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 11, padding: "11px 22px", cursor: "pointer", fontFamily: "inherit" }}>Create a skill</button>
          </div>
        ) : (
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
            {config.skills.map((skill) => (
              <CustomSkillCard key={skill.id} skill={skill} onToggle={() => handleToggleCustom(skill.id)} onEdit={() => setEditingSkill(skill)} onDelete={() => setDeleteConfirmId(skill.id)} />
            ))}
            <CreateSkillCard onClick={() => setShowCreate(true)} />
          </div>
        )}
      </div>
    </div>
  );
}

// ── Settings View ──────────────────────────────────────────────────────────────

function SettingsView({ authState, onLogout }: { authState: AuthState; onLogout: () => void }) {
  const [hotkey, setHotkey] = useState("ctrl+shift+r");
  const [superHotkey, setSuperHotkey] = useState("ctrl+shift+period");
  const [defaultSkillId, setDefaultSkillId] = useState("__proofread__");
  const [skillsConfig, setSkillsConfig] = useState<SkillsConfig>({ global_instructions: "", skills: [], builtin_enabled: {} });

  const [editingHotkey, setEditingHotkey] = useState(false);
  const [newHotkey, setNewHotkey] = useState("");
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [hotkeySaving, setHotkeySaving] = useState(false);

  const [editingSuperHotkey, setEditingSuperHotkey] = useState(false);
  const [newSuperHotkey, setNewSuperHotkey] = useState("");
  const [superHotkeyError, setSuperHotkeyError] = useState<string | null>(null);
  const [superHotkeySaving, setSuperHotkeySaving] = useState(false);

  const [startup, setStartup] = useState(true);
  const [sounds, setSounds] = useState(false);

  // Bubble toggle: unlike the other rows above (each with a dedicated
  // command), there's no `update_bubble_enabled` command — this is the first
  // Settings row to save an arbitrary field via the generic `save_config`.
  // Keep the last-fetched full Config around so the save always round-trips
  // every field, not just this one.
  const [bubbleEnabled, setBubbleEnabled] = useState(true);
  const [fullConfig, setFullConfig] = useState<Config | null>(null);
  const [bubbleError, setBubbleError] = useState<string | null>(null);

  const [appVersion, setAppVersion] = useState("");
  const [updateStatus, setUpdateStatus] = useState<"idle" | "checking" | "up-to-date" | "downloading" | "ready" | "error">("idle");
  const [updateError, setUpdateError] = useState<string | null>(null);

  const newHotkeyRef = useRef<HTMLInputElement>(null);
  const newSuperHotkeyRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    invoke<Config>("get_config").then((cfg) => {
      setHotkey(cfg.hotkey);
      setSuperHotkey(cfg.super_hotkey);
      setDefaultSkillId(cfg.default_skill_id);
      setBubbleEnabled(cfg.bubble_enabled);
      setFullConfig(cfg);
    });
    invoke<SkillsConfig>("get_skills_config").then(setSkillsConfig);
    getVersion().then(setAppVersion);
  }, []);

  async function handleCheckForUpdates() {
    setUpdateStatus("checking");
    setUpdateError(null);
    try {
      const update = await checkForUpdate();
      if (!update) {
        setUpdateStatus("up-to-date");
        return;
      }
      setUpdateStatus("downloading");
      await update.downloadAndInstall();
      setUpdateStatus("ready");
      await relaunch();
    } catch (err) {
      setUpdateStatus("error");
      setUpdateError(String(err));
    }
  }

  useEffect(() => { if (editingHotkey) newHotkeyRef.current?.focus(); }, [editingHotkey]);
  useEffect(() => { if (editingSuperHotkey) newSuperHotkeyRef.current?.focus(); }, [editingSuperHotkey]);

  async function handleSaveHotkey(e: React.FormEvent) {
    e.preventDefault();
    const key = newHotkey.trim().toLowerCase();
    if (!key) return;
    setHotkeySaving(true);
    setHotkeyError(null);
    try {
      await invoke("update_hotkey", { hotkey: key });
      setHotkey(key);
      setEditingHotkey(false);
      setNewHotkey("");
    } catch (err) {
      setHotkeyError(String(err));
    } finally {
      setHotkeySaving(false);
    }
  }

  async function handleSaveSuperHotkey(e: React.FormEvent) {
    e.preventDefault();
    const key = newSuperHotkey.trim().toLowerCase();
    if (!key) return;
    setSuperHotkeySaving(true);
    setSuperHotkeyError(null);
    try {
      await invoke("update_super_hotkey", { hotkey: key });
      setSuperHotkey(key);
      setEditingSuperHotkey(false);
      setNewSuperHotkey("");
    } catch (err) {
      setSuperHotkeyError(String(err));
    } finally {
      setSuperHotkeySaving(false);
    }
  }

  async function handleToggleBubble() {
    if (!fullConfig) return;
    const newEnabled = !bubbleEnabled;
    const updated = { ...fullConfig, bubble_enabled: newEnabled };
    setBubbleEnabled(newEnabled);
    setFullConfig(updated);
    try {
      await invoke("save_config", { config: updated });
    } catch (err) {
      setBubbleError(String(err));
    }
  }

  async function handleDefaultSkillChange(skillId: string) {
    setDefaultSkillId(skillId);
    try {
      await invoke("set_default_skill", { skillId });
    } catch (err) {
      console.error("Failed to save default skill:", err);
    }
  }

  const divider = <div style={{ height: 1, background: "#f0f1f3", margin: "0 -24px" }} />;

  function PrefRow({ label, sub, right }: { label: string; sub?: string; right: React.ReactNode }) {
    return (
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "18px 0" }}>
        <div>
          <div style={{ fontSize: 15, fontWeight: 600, color: "#1f2026" }}>{label}</div>
          {sub && <div style={{ fontSize: 13, color: "#86898f", marginTop: 2 }}>{sub}</div>}
        </div>
        {right}
      </div>
    );
  }

  const allSkillOptions = [
    ...BUILTIN_SKILL_OPTIONS,
    ...skillsConfig.skills.sort((a, b) => a.order - b.order).map((s) => ({ id: s.id, name: s.name })),
  ];

  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      <header style={{ marginBottom: 30 }}>
        <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 46, lineHeight: 1.02, color: "#16161a", letterSpacing: -.5 }}>Settings</h1>
        <p style={{ fontSize: 16, color: "#74777e", marginTop: 9 }}>Your plan, your preferences, your shortcuts.</p>
      </header>

      <div style={{ maxWidth: 720, display: "flex", flexDirection: "column", gap: 22 }}>
        {/* Account */}
        <section style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 15, padding: "22px 24px", display: "flex", alignItems: "center", gap: 18 }}>
          <div style={{ width: 52, height: 52, borderRadius: "50%", background: "#16161a", color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", fontWeight: 600, fontSize: 18, flexShrink: 0 }}>
            {initialsFromEmail(authState.email)}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 17, fontWeight: 600, color: "#1f2026", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{firstNameFromEmail(authState.email)}</div>
            <div style={{ fontSize: 13.5, color: "#86898f", marginTop: 2, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{authState.email}</div>
          </div>
          <button
            onClick={async () => { await invoke("logout"); onLogout(); }}
            style={{ fontSize: 13.5, fontWeight: 600, color: "#c0392b", background: "#fdf1f1", border: "1px solid #fad8d8", borderRadius: 9, padding: "9px 15px", cursor: "pointer", fontFamily: "inherit", flexShrink: 0 }}
          >
            Sign out
          </button>
        </section>

        {/* Plan & billing */}
        <section style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 15, padding: 24, overflow: "hidden" }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 18 }}>
            <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 20, color: "#16161a" }}>Plan &amp; billing</h3>
            <span style={{ fontSize: 11.5, fontWeight: 600, letterSpacing: .5, textTransform: "uppercase", color: authState.is_subscribed ? "#fff" : "#5b5e66", background: authState.is_subscribed ? ACCENT : "#f1f2f4", padding: "5px 11px", borderRadius: 7, border: authState.is_subscribed ? "none" : "1px solid #e3e4e7" }}>
              {authState.is_subscribed ? "Pro" : "Free"}
            </span>
          </div>

          {authState.is_subscribed ? (
            <>
              <div style={{ fontSize: 15, color: "#5b5e66" }}>
                {authState.subscription_valid_until
                  ? <>Renews <strong style={{ color: "#1f2026" }}>{formatRenewalDate(authState.subscription_valid_until)}</strong></>
                  : "Active subscription"}
              </div>
              <div style={{ display: "flex", gap: 10, marginTop: 22 }}>
                <button onClick={() => invoke("open_checkout", { plan: "pro" })} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#16161a", border: "none", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Change plan</button>
                <button onClick={() => invoke("open_billing_portal")} style={{ fontSize: 13.5, fontWeight: 600, color: "#16161a", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Manage billing</button>
              </div>
            </>
          ) : (
            <>
              <div style={{ fontSize: 15, color: "#5b5e66", marginBottom: 14 }}>
                <strong style={{ color: "#1f2026" }}>{authState.rewrite_count}</strong> / {FREE_TIER_MONTHLY_LIMIT} rewrites used this month
              </div>
              <div style={{ background: "#f3f4f5", borderRadius: 8, height: 6, overflow: "hidden" }}>
                <div style={{ background: authState.rewrite_count >= FREE_TIER_MONTHLY_LIMIT - 1 ? "#c0392b" : "#16161a", height: "100%", width: `${Math.min((authState.rewrite_count / FREE_TIER_MONTHLY_LIMIT) * 100, 100)}%`, borderRadius: 8, transition: "width .3s" }} />
              </div>
              <div style={{ display: "flex", gap: 10, marginTop: 22 }}>
                <button onClick={() => invoke("open_checkout", { plan: "pro" })} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#16161a", border: "none", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Upgrade to Pro</button>
                <button onClick={() => invoke("open_checkout", { plan: "max" })} style={{ fontSize: 13.5, fontWeight: 600, color: "#16161a", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 17px", cursor: "pointer", fontFamily: "inherit" }}>Upgrade to Max</button>
              </div>
            </>
          )}
        </section>

        {/* Preferences */}
        <section style={{ background: "#fff", border: "1px solid #e8e9ec", borderRadius: 15, padding: "8px 24px" }}>

          {/* Overlay hotkey */}
          {editingHotkey ? (
            <div style={{ padding: "18px 0", borderBottom: "1px solid #f0f1f3" }}>
              <div style={{ fontSize: 15, fontWeight: 600, color: "#1f2026", marginBottom: 10 }}>Overlay hotkey</div>
              <form onSubmit={handleSaveHotkey} style={{ display: "flex", gap: 8 }}>
                <input ref={newHotkeyRef} value={newHotkey} onChange={(e) => setNewHotkey(e.target.value)} placeholder={hotkey} style={{ flex: 1, border: "1px solid #e0e1e4", borderRadius: 9, padding: "10px 13px", fontSize: 14, color: "#16161a", outline: "none", fontFamily: "monospace" }} />
                <button type="submit" disabled={!newHotkey.trim() || hotkeySaving} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#16161a", border: "none", borderRadius: 9, padding: "10px 16px", cursor: "pointer", fontFamily: "inherit" }}>{hotkeySaving ? "…" : "Save"}</button>
                <button type="button" onClick={() => { setEditingHotkey(false); setHotkeyError(null); setNewHotkey(""); }} style={{ fontSize: 13.5, color: "#74777e", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 14px", cursor: "pointer", fontFamily: "inherit" }}>Cancel</button>
              </form>
              {hotkeyError && <div style={{ fontSize: 12, color: "#c0392b", marginTop: 6 }}>{hotkeyError}</div>}
              <div style={{ fontSize: 12, color: "#9a9da3", marginTop: 6 }}>e.g. <code>ctrl+shift+r</code> or <code>ctrl+alt+space</code></div>
            </div>
          ) : (
            <PrefRow
              label="Overlay hotkey"
              sub="Summon the skill picker from anywhere"
              right={
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  {hotkeyParts(hotkey).map((k, i) => (
                    <kbd key={i} style={{ fontFamily: "'Hanken Grotesk', sans-serif", fontSize: 12.5, fontWeight: 600, color: "#3a3c42", background: "#f3f4f5", border: "1px solid #e2e3e6", borderBottomWidth: 2, borderRadius: 7, padding: "5px 9px" }}>{k}</kbd>
                  ))}
                  <button onClick={() => { setEditingHotkey(true); setNewHotkey(hotkey); }} style={{ fontSize: 12.5, fontWeight: 500, color: "#86898f", background: "transparent", border: "none", cursor: "pointer", marginLeft: 4, fontFamily: "inherit" }}>Change</button>
                </div>
              }
            />
          )}
          {divider}

          {/* Super hotkey */}
          {editingSuperHotkey ? (
            <div style={{ padding: "18px 0", borderBottom: "1px solid #f0f1f3" }}>
              <div style={{ fontSize: 15, fontWeight: 600, color: "#1f2026", marginBottom: 4 }}>Super hotkey</div>
              <div style={{ fontSize: 13, color: "#86898f", marginBottom: 10 }}>Instantly applies the default skill, no overlay shown.</div>
              <form onSubmit={handleSaveSuperHotkey} style={{ display: "flex", gap: 8 }}>
                <input ref={newSuperHotkeyRef} value={newSuperHotkey} onChange={(e) => setNewSuperHotkey(e.target.value)} placeholder={superHotkey} style={{ flex: 1, border: "1px solid #e0e1e4", borderRadius: 9, padding: "10px 13px", fontSize: 14, color: "#16161a", outline: "none", fontFamily: "monospace" }} />
                <button type="submit" disabled={!newSuperHotkey.trim() || superHotkeySaving} style={{ fontSize: 13.5, fontWeight: 600, color: "#fff", background: "#16161a", border: "none", borderRadius: 9, padding: "10px 16px", cursor: "pointer", fontFamily: "inherit" }}>{superHotkeySaving ? "…" : "Save"}</button>
                <button type="button" onClick={() => { setEditingSuperHotkey(false); setSuperHotkeyError(null); setNewSuperHotkey(""); }} style={{ fontSize: 13.5, color: "#74777e", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "10px 14px", cursor: "pointer", fontFamily: "inherit" }}>Cancel</button>
              </form>
              {superHotkeyError && <div style={{ fontSize: 12, color: "#c0392b", marginTop: 6 }}>{superHotkeyError}</div>}
              <div style={{ fontSize: 12, color: "#9a9da3", marginTop: 6 }}>e.g. <code>ctrl+shift+period</code></div>
            </div>
          ) : (
            <PrefRow
              label="Super hotkey"
              sub="Instantly applies your default skill, no overlay"
              right={
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  {hotkeyParts(superHotkey).map((k, i) => (
                    <kbd key={i} style={{ fontFamily: "'Hanken Grotesk', sans-serif", fontSize: 12.5, fontWeight: 600, color: "#3a3c42", background: "#f3f4f5", border: "1px solid #e2e3e6", borderBottomWidth: 2, borderRadius: 7, padding: "5px 9px" }}>{k}</kbd>
                  ))}
                  <button onClick={() => { setEditingSuperHotkey(true); setNewSuperHotkey(superHotkey); }} style={{ fontSize: 12.5, fontWeight: 500, color: "#86898f", background: "transparent", border: "none", cursor: "pointer", marginLeft: 4, fontFamily: "inherit" }}>Change</button>
                </div>
              }
            />
          )}
          {divider}

          <PrefRow
            label="Default skill"
            sub="Applied by the super hotkey"
            right={
              <select
                value={defaultSkillId}
                onChange={(e) => handleDefaultSkillChange(e.target.value)}
                style={{ fontSize: 14, fontWeight: 500, color: "#2c2e34", background: "#f3f4f5", border: "1px solid #e6e7ea", borderRadius: 9, padding: "8px 32px 8px 13px", cursor: "pointer", fontFamily: "inherit", outline: "none", appearance: "none", backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='%2386898f' stroke-width='2' stroke-linecap='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E")`, backgroundRepeat: "no-repeat", backgroundPosition: "right 10px center" }}
              >
                {allSkillOptions.map((o) => (
                  <option key={o.id} value={o.id}>{o.name}</option>
                ))}
              </select>
            }
          />
          {divider}

          <PrefRow
            label="Selection bubble"
            sub="Show a quick-rewrite bubble near text you highlight"
            right={<Toggle on={bubbleEnabled} onToggle={handleToggleBubble} />}
          />
          {bubbleError && <div style={{ fontSize: 12, color: "#c0392b", marginTop: -10, marginBottom: 10 }}>{bubbleError}</div>}
          {divider}

          <PrefRow label="Launch on startup" sub="Open reWrite when you sign in" right={<Toggle on={startup} onToggle={() => setStartup((s) => !s)} />} />
          {divider}
          <PrefRow label="Sound on rewrite" sub="Play a chime when text is ready" right={<Toggle on={sounds} onToggle={() => setSounds((s) => !s)} />} />
        </section>

        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "4px 6px 8px" }}>
          <div
            style={{ fontFamily: "'Playfair Display', serif", fontStyle: "italic", fontSize: 14, color: updateStatus === "error" ? "#c05a4a" : "#9a9da3" }}
            title={updateStatus === "error" ? updateError ?? undefined : undefined}
          >
            reWrite {appVersion}
            {updateStatus === "idle" && ". Check for updates any time."}
            {updateStatus === "checking" && ". Checking for updates…"}
            {updateStatus === "up-to-date" && ". You're up to date."}
            {updateStatus === "downloading" && ". Downloading update…"}
            {updateStatus === "ready" && ". Restarting…"}
            {updateStatus === "error" && ". Couldn't check for updates."}
          </div>
          <button
            onClick={handleCheckForUpdates}
            disabled={updateStatus === "checking" || updateStatus === "downloading" || updateStatus === "ready"}
            style={{ fontSize: 13, fontWeight: 600, color: "#86898f", background: "transparent", border: "none", cursor: "pointer", padding: "6px 4px", fontFamily: "inherit", opacity: (updateStatus === "checking" || updateStatus === "downloading" || updateStatus === "ready") ? 0.5 : 1 }}
          >
            {updateStatus === "checking" ? "Checking…" : updateStatus === "downloading" ? "Downloading…" : "Check for updates"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Root ───────────────────────────────────────────────────────────────────────

export default function Settings() {
  const [active, setActive] = useState<ActiveView>("home");
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [skillsConfig, setSkillsConfig] = useState<SkillsConfig>({ global_instructions: "", skills: [], builtin_enabled: {} });
  const [config, setConfig] = useState<Config>({ hotkey: "ctrl+shift+r", super_hotkey: "ctrl+shift+period", default_skill_id: "__proofread__", model: "claude-sonnet-4-6", restore_clipboard: true, restore_delay_ms: 500, paste_delay_ms: 400, bubble_enabled: true });
  const [authState, setAuthState] = useState<AuthState | null>(null);

  async function loadAuthState() {
    const state = await invoke<AuthState>("get_auth_state");
    setAuthState(state);
  }

  useEffect(() => {
    loadAuthState();
    const unlistenAuth = listen("auth:complete", () => loadAuthState());
    const unlistenUsage = listen("usage:updated", () => loadAuthState());
    // The overlay's "renew" link opens this window straight to a given tab.
    const unlistenNav = listen<ActiveView>("settings:navigate", (e) => setActive(e.payload));
    return () => {
      unlistenAuth.then((fn) => fn());
      unlistenUsage.then((fn) => fn());
      unlistenNav.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const blockContextMenu = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", blockContextMenu);
    return () => document.removeEventListener("contextmenu", blockContextMenu);
  }, []);

  useEffect(() => {
    Promise.all([
      invoke<HistoryEntry[]>("get_history"),
      invoke<SkillsConfig>("get_skills_config"),
      invoke<Config>("get_config"),
    ]).then(([h, sc, cfg]) => {
      setHistory(h);
      setSkillsConfig(sc);
      setConfig(cfg);
    });
  }, [active]);

  // Loading
  if (authState === null) return null;

  // Not logged in
  if (!authState.logged_in) return <LoginView onLogin={loadAuthState} />;

  return (
    <div style={{ display: "flex", height: "100vh", overflow: "hidden", fontFamily: "'Hanken Grotesk', system-ui, sans-serif", background: "#e6e7ea" }}>
      <Sidebar active={active} setActive={setActive} authState={authState} />
      <main className="rw-scroll" style={{ flex: 1, overflowY: "auto", background: "#fff", position: "relative" }}>
        {active === "home"     && <HomeView history={history} skillsConfig={skillsConfig} config={config} authState={authState} />}
        {active === "history"  && <HistoryView history={history} />}
        {active === "skills"   && (authState.is_subscribed ? <SkillsView /> : <SkillsLockedView />)}
        {active === "settings" && <SettingsView authState={authState} onLogout={loadAuthState} />}
      </main>
    </div>
  );
}
