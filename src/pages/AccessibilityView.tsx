import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ACCENT } from "./settingsConstants";

// Poll interval while this view is mounted. The recurring poll uses the
// cheap non-prompting query (`check_accessibility_permission` ->
// `AXIsProcessTrusted()` on macOS, always `true` elsewhere), so polling every
// 1.5s is safe. The one-time mount check below uses the prompting variant
// instead — see `initialRequest` in the effect.
const POLL_INTERVAL_MS = 1500;
// Small pause after the status flips to granted so the "Granted!" state is
// actually readable before we auto-continue back to Home (first-run only).
const GRANTED_CONTINUE_DELAY_MS = 1400;

const WHY_REASONS = [
  {
    title: "Read the currently selected text",
    desc: "reWrite needs to see what you've highlighted in another app before it can rewrite it.",
  },
  {
    title: "Show the rewrite picker near your selection",
    desc: "The skill picker and floating bubble need to know where your selection is on screen.",
  },
  {
    title: "Paste the rewritten text back into the original app",
    desc: "After rewriting, reWrite simulates a paste so the result replaces your original selection.",
  },
];

const STEPS = [
  "Open System Settings",
  "Go to Privacy & Security → Accessibility",
  "Enable reWrite",
  "Return to reWrite",
];

export function AccessibilityView({
  isFirstRun,
  onStatusChange,
  onContinue,
}: {
  /** True only when this view was opened automatically because permission
   * was missing on mount (first launch, or a later launch with a revoked
   * permission) — controls whether granting auto-navigates back to Home.
   * False when the user navigated here manually (e.g. from the sidebar or
   * the Home banner) to check/recover status; in that case we just update
   * the status indicator in place and let the user leave on their own. */
  isFirstRun: boolean;
  /** Called on every poll result, in both directions — not just the first
   * grant — so the parent's status (sidebar dot, Home banner) tracks live
   * reality, including a permission revoked while this view isn't mounted. */
  onStatusChange: (granted: boolean) => void;
  /** Called after a short delay once granted, but only when `isFirstRun`. */
  onContinue: () => void;
}) {
  const [granted, setGranted] = useState<boolean | null>(null);
  const [justGranted, setJustGranted] = useState(false);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const wasGrantedRef = useRef(false);

  // Keep the latest callbacks in refs so the polling effect below doesn't
  // need to restart (and lose its interval timing) whenever the parent
  // re-renders with a fresh closure identity.
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;
  const onContinueRef = useRef(onContinue);
  onContinueRef.current = onContinue;

  useEffect(() => {
    let cancelled = false;

    function handleResult(result: boolean) {
      if (cancelled) return;
      setGranted(result);
      onStatusChangeRef.current(result);
      if (result) {
        if (!wasGrantedRef.current) {
          wasGrantedRef.current = true;
          setJustGranted(true);
          if (isFirstRun) {
            setTimeout(() => {
              if (!cancelled) onContinueRef.current();
            }, GRANTED_CONTINUE_DELAY_MS);
          }
        }
      } else {
        wasGrantedRef.current = false;
      }
    }

    async function initialRequest() {
      try {
        // Call the PROMPTING variant (not just a status check) once when this
        // view mounts. This is the fix for a real show-stopper: the
        // non-prompting `AXIsProcessTrusted()` query alone never registers
        // the app in Privacy & Security > Accessibility at all, so without
        // this call the "Enable reWrite" checklist step has nothing to
        // enable — reWrite simply isn't in the list. Calling the prompting
        // API is what both shows the native "reWrite would like to control
        // this computer" dialog (first time only) AND registers the app.
        //
        // Safe to call unconditionally on every mount: the backend's
        // accessibility_trusted(true) checks AXIsProcessTrusted() first and
        // only invokes the prompting call when that's false, and macOS
        // itself does not re-show the dialog once the user has already
        // answered it once — so an already-granted (or already-denied) user
        // revisiting this view never sees the dialog pop up again.
        const result = await invoke<boolean>("request_accessibility_permission");
        handleResult(result);
      } catch {
        // Fall through to polling below regardless of a transient IPC error.
      }
    }

    async function poll() {
      try {
        // Recurring polls use the non-prompting query — cheap, and avoids
        // any theoretical risk of repeatedly invoking the prompting API on
        // a tight interval.
        const result = await invoke<boolean>("check_accessibility_permission");
        handleResult(result);
      } catch {
        // Transient IPC failure — keep polling, don't flip status on a guess.
      }
    }

    initialRequest();
    const id = setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [isFirstRun]);

  async function handleOpenSettings() {
    setOpening(true);
    setOpenError(null);
    try {
      await invoke("open_accessibility_settings");
    } catch (err) {
      setOpenError(String(err));
    } finally {
      setOpening(false);
    }
  }

  const statusGood = granted === true;

  return (
    <div style={{ padding: "46px 48px 52px", animation: "rwfade .35s ease both" }}>
      <header style={{ marginBottom: 30 }}>
        <h1 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 46, lineHeight: 1.02, color: "var(--rw-text-primary)", letterSpacing: -0.5 }}>
          Accessibility permission
        </h1>
        <p style={{ fontSize: 16, color: "var(--rw-text-muted)", marginTop: 9 }}>
          reWrite needs this macOS permission to capture, position, and paste text system-wide.
        </p>
      </header>

      <div style={{ maxWidth: 720, display: "flex", flexDirection: "column", gap: 22 }}>
        {/* Live status */}
        <section
          style={{
            display: "flex",
            alignItems: "center",
            gap: 14,
            background: statusGood ? "var(--rw-success-bg)" : "var(--rw-danger-bg)",
            border: `1px solid ${statusGood ? "var(--rw-success-border)" : "var(--rw-danger-border)"}`,
            borderRadius: 15,
            padding: "18px 22px",
            transition: "background .2s, border-color .2s",
          }}
        >
          <span
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: granted === null ? "var(--rw-border-hover)" : statusGood ? "var(--rw-success)" : "var(--rw-danger)",
              flexShrink: 0,
            }}
          />
          <div style={{ fontSize: 15, fontWeight: 600, color: statusGood ? "var(--rw-success-text-strong)" : "var(--rw-danger-text-strong)" }}>
            {granted === null
              ? "Checking permission…"
              : statusGood
                ? justGranted
                  ? "Granted! reWrite is ready to go."
                  : "Accessibility is enabled."
                : "Accessibility is not enabled yet."}
          </div>
        </section>

        {/* Why we need this */}
        <section style={{ background: "var(--rw-bg-primary)", border: "1px solid var(--rw-border)", borderRadius: 15, padding: 24 }}>
          <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 20, color: "var(--rw-text-primary)", marginBottom: 16 }}>
            Why reWrite needs Accessibility
          </h3>
          <div style={{ display: "flex", flexDirection: "column", gap: 15 }}>
            {WHY_REASONS.map((r, i) => (
              <div key={i} style={{ display: "flex", gap: 13, alignItems: "flex-start" }}>
                <span
                  style={{
                    width: 24, height: 24, borderRadius: "50%", background: "var(--rw-bg-subtle)", color: "var(--rw-text-secondary)",
                    display: "flex", alignItems: "center", justifyContent: "center",
                    fontSize: 12.5, fontWeight: 700, flexShrink: 0, marginTop: 1,
                  }}
                >
                  {i + 1}
                </span>
                <div>
                  <div style={{ fontSize: 15, fontWeight: 600, color: "var(--rw-text-primary)" }}>{r.title}</div>
                  <div style={{ fontSize: 13.5, color: "var(--rw-text-muted)", marginTop: 2, lineHeight: 1.5 }}>{r.desc}</div>
                </div>
              </div>
            ))}
          </div>
        </section>

        {/* Step-by-step + primary action */}
        <section style={{ background: "var(--rw-bg-primary)", border: "1px solid var(--rw-border)", borderRadius: 15, padding: 24 }}>
          <h3 style={{ fontFamily: "'Playfair Display', serif", fontWeight: 600, fontSize: 20, color: "var(--rw-text-primary)", marginBottom: 16 }}>
            Steps to enable
          </h3>
          <ol style={{ margin: 0, padding: 0, listStyle: "none", display: "flex", flexDirection: "column", gap: 11 }}>
            {STEPS.map((s, i) => (
              <li key={i} style={{ display: "flex", alignItems: "center", gap: 12, fontSize: 14.5, color: "var(--rw-text-primary)" }}>
                <span style={{ fontFamily: "monospace", fontSize: 12, fontWeight: 700, color: "var(--rw-text-muted)", width: 16, flexShrink: 0 }}>
                  {i + 1}.
                </span>
                {s}
              </li>
            ))}
          </ol>

          <button
            onClick={handleOpenSettings}
            disabled={opening}
            style={{
              marginTop: 22,
              background: ACCENT,
              color: "var(--rw-on-accent)",
              border: "none",
              borderRadius: 10,
              padding: "13px 20px",
              fontSize: 14.5,
              fontWeight: 600,
              cursor: opening ? "default" : "pointer",
              fontFamily: "inherit",
              opacity: opening ? 0.7 : 1,
            }}
          >
            {opening ? "Opening…" : "Open System Settings → Accessibility"}
          </button>
          {openError && <div style={{ fontSize: 12.5, color: "var(--rw-danger)", marginTop: 10 }}>{openError}</div>}
        </section>

        {/* Degraded-state messaging while not granted */}
        {!statusGood && (
          <section style={{ background: "var(--rw-bg-subtle)", border: "1px solid var(--rw-border)", borderRadius: 15, padding: "18px 22px" }}>
            <div style={{ fontSize: 13.5, fontWeight: 600, color: "var(--rw-text-secondary)", marginBottom: 8 }}>While Accessibility is off</div>
            <ul style={{ margin: 0, paddingLeft: 18, fontSize: 13.5, color: "var(--rw-text-muted)", lineHeight: 1.65 }}>
              <li>Hotkey capture may fail to read your selected text.</li>
              <li>The floating rewrite bubble is disabled.</li>
              <li>Settings, account, and billing features still work normally.</li>
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}
