import { useState } from "react";
import type { ReactNode } from "react";
import logoBlack from "../assets/rewrite_logo_black.png";
import { ACCENT, APP_VERSION } from "./settingsConstants";
import { initialsFromEmail } from "./settingsHelpers";
import type { ActiveView, AuthState } from "./settingsTypes";

export function Toggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <div
      onClick={(e) => { e.stopPropagation(); onToggle(); }}
      style={{
        width: 42, height: 24, borderRadius: 13, cursor: "pointer",
        position: "relative", transition: "background .18s", flexShrink: 0,
        background: on ? ACCENT : "var(--rw-toggle-off)",
      }}
    >
      <div style={{
        position: "absolute", top: 3, left: on ? 21 : 3,
        width: 18, height: 18, borderRadius: "50%",
        background: "var(--rw-on-accent)", boxShadow: "0 1px 3px rgba(0,0,0,.25)",
        transition: "left .18s",
      }} />
    </div>
  );
}

export const IconHome = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
    <rect x="3.5" y="3.5" width="7" height="7" rx="1.6" />
    <rect x="13.5" y="3.5" width="7" height="7" rx="1.6" />
    <rect x="3.5" y="13.5" width="7" height="7" rx="1.6" />
    <rect x="13.5" y="13.5" width="7" height="7" rx="1.6" />
  </svg>
);

export const IconHistory = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3.2 12a8.8 8.8 0 1 0 2.9-6.5L3 8" />
    <path d="M3 4v4h4" />
    <path d="M12 7.4V12l3 1.8" />
  </svg>
);

export const IconBook = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3 5.2A2.2 2.2 0 0 1 5.2 3H12v16.5H5.2A2.2 2.2 0 0 0 3 21.7z" />
    <path d="M21 5.2A2.2 2.2 0 0 0 18.8 3H12v16.5h6.8A2.2 2.2 0 0 1 21 21.7z" />
  </svg>
);

export const IconGear = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <circle cx="12" cy="12" r="3.1" />
    <path d="M19.1 14.5a1.55 1.55 0 0 0 .31 1.71l.05.05a2 2 0 1 1-2.83 2.83l-.05-.05a1.55 1.55 0 0 0-1.71-.31 1.55 1.55 0 0 0-.94 1.42V20a2 2 0 0 1-4 0v-.07a1.55 1.55 0 0 0-1.02-1.42 1.55 1.55 0 0 0-1.71.31l-.05.05a2 2 0 1 1-2.83-2.83l.05-.05a1.55 1.55 0 0 0 .31-1.71 1.55 1.55 0 0 0-1.42-.94H4a2 2 0 0 1 0-4h.07a1.55 1.55 0 0 0 1.42-1.02 1.55 1.55 0 0 0-.31-1.71l-.05-.05a2 2 0 1 1 2.83-2.83l.05.05a1.55 1.55 0 0 0 1.71.31H9.7a1.55 1.55 0 0 0 .94-1.42V4a2 2 0 0 1 4 0v.07a1.55 1.55 0 0 0 .94 1.42 1.55 1.55 0 0 0 1.71-.31l.05-.05a2 2 0 1 1 2.83 2.83l-.05.05a1.55 1.55 0 0 0-.31 1.71v.06a1.55 1.55 0 0 0 1.42.94H20a2 2 0 0 1 0 4h-.07a1.55 1.55 0 0 0-1.42.94z" />
  </svg>
);

export const IconLock = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
    <path d="M8 10.5V7a4 4 0 0 1 8 0v3.5" />
  </svg>
);

export const IconShield = () => (
  <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 3.2 4.5 6v6.2c0 4.6 3.1 8.2 7.5 9.6 4.4-1.4 7.5-5 7.5-9.6V6z" />
    <path d="M9 12.2l2.1 2.1L15.3 10" />
  </svg>
);

function NavButton({ label, icon, active, onClick, locked, dot }: { label: string; icon: ReactNode; active: boolean; onClick: () => void; locked?: boolean; dot?: string }) {
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
        background: active ? "var(--rw-bg-primary)" : hov ? "var(--rw-hover-overlay)" : "transparent",
        color: locked ? "var(--rw-text-faint)" : active ? "var(--rw-text-primary)" : "var(--rw-text-secondary)",
        border: active ? "1px solid var(--rw-border)" : "1px solid transparent",
        boxShadow: active ? "0 1px 2px rgba(20,20,26,.10)" : "none",
      }}
    >
      {icon}
      <span style={{ flex: 1 }}>{label}</span>
      {dot && <span title={dot === "var(--rw-danger)" ? "Action needed" : "Enabled"} style={{ width: 8, height: 8, borderRadius: "50%", background: dot, flexShrink: 0 }} />}
      {locked && <IconLock />}
    </button>
  );
}

export function Sidebar({ active, setActive, authState, accessibilityGranted }: { active: ActiveView; setActive: (v: ActiveView) => void; authState: AuthState; accessibilityGranted: boolean }) {
  return (
    <aside style={{ width: 250, minWidth: 250, background: "var(--rw-bg-secondary)", borderRight: "1px solid var(--rw-border)", display: "flex", flexDirection: "column", padding: "30px 20px 22px" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "6px 4px 30px" }}>
        <img src={logoBlack} alt="reWrite" style={{ height: 58, width: "auto" }} />
      </div>
      <nav style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        <NavButton label="Home"     icon={<IconHome />}    active={active === "home"}     onClick={() => setActive("home")} />
        <NavButton label="History"  icon={<IconHistory />} active={active === "history"}  onClick={() => setActive("history")} />
        <NavButton label="Skills"   icon={<IconBook />}    active={active === "skills"}   onClick={() => setActive("skills")} locked={!authState.is_subscribed} />
        <NavButton label="Accessibility" icon={<IconShield />} active={active === "accessibility"} onClick={() => setActive("accessibility")} dot={accessibilityGranted ? "var(--rw-success)" : "var(--rw-danger)"} />
        <NavButton label="Settings" icon={<IconGear />}    active={active === "settings"} onClick={() => setActive("settings")} />
      </nav>
      <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: 16 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 11, padding: "9px 11px", borderRadius: 11, background: "var(--rw-bg-secondary-raised)" }}>
          <div style={{ width: 34, height: 34, borderRadius: "50%", background: "var(--rw-accent)", color: "var(--rw-on-accent)", display: "flex", alignItems: "center", justifyContent: "center", fontWeight: 600, fontSize: 13, letterSpacing: .3, flexShrink: 0 }}>
            {initialsFromEmail(authState.email)}
          </div>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 13.5, fontWeight: 600, color: "var(--rw-text-primary)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{authState.email}</div>
            <div style={{ fontSize: 11.5, color: "var(--rw-text-muted)" }}>{authState.is_subscribed ? "reWrite Pro" : "Free plan"}</div>
          </div>
        </div>
        <div style={{ fontFamily: "'Playfair Display', serif", fontStyle: "italic", fontSize: 13, color: "var(--rw-text-faint)", paddingLeft: 4 }}>Version {APP_VERSION}</div>
      </div>
    </aside>
  );
}
