import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import type { SkillsConfig } from "../types";
import { buildItems, type SkillItem } from "../skills";
import logoBlack from "../assets/logo_transparent.png";

type Status = "idle" | "loading" | "error";

const isLimitError = (msg: string) => /limit|trial|quota|upgrade/i.test(msg);
const MENU_SIZE = { width: 168, height: 180 };
const SPINNER_WINDOW_SIZE = 44;

// Temporary diagnostic for the stuck-error-state investigation: surfaces a
// line in the same terminal trace output Rust-side events use. Remove once
// resolved.
function dbg(msg: string) {
  invoke("debug_trace", { msg: `BubbleMenu ${msg}` }).catch(() => {});
}

// Sprint 3: the real titles-only skill list, wired into the same
// rewrite_with_skill -> paste_text pipeline Overlay.tsx uses. This window is a
// compact dropdown near the 10x10 bubble, not the full skill-picker overlay —
// no descriptions, no captured-text preview, no keyboard-shortcut footer.
export default function BubbleMenu() {
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [items, setItems] = useState<SkillItem[]>([]);

  const statusRef = useRef<Status>("idle");
  const cancelledRef = useRef(false);

  const setWindowSize = useCallback(async (width: number, height: number) => {
    await getCurrentWindow().setSize(new LogicalSize(width, height)).catch(() => {});
  }, []);

  // Dismiss-on-blur: the menu closes once focus genuinely leaves it (e.g. the
  // user clicks back into the source app). Closing is debounced and
  // re-verified via isFocused() rather than acting on the raw event directly:
  // show() + set_focus() on a window that was just hidden a moment ago can
  // produce a transient false-then-true (or true-then-false) activation blip
  // on Windows, and reacting to that blip instantly closed the menu before
  // the user ever saw it (reported as "menu never appears"). Re-checking after
  // a short delay filters that out while still closing promptly on a real
  // blur.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (timeoutId) clearTimeout(timeoutId);
        if (focused) return;
        timeoutId = setTimeout(async () => {
          const stillFocused = await getCurrentWindow().isFocused().catch(() => true);
          if (!stillFocused) {
            invoke("close_bubble_menu").catch(() => {});
          }
        }, 150);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      unlisten?.();
      if (timeoutId) clearTimeout(timeoutId);
    };
  }, []);

  // Fetch the skill list fresh every time the window becomes focused, same
  // idea as Overlay's refreshData — but this window never needs the captured
  // text itself (bubble_clicked already stashed it in AppState before this
  // window was shown), so only get_skills_config is called.
  const refreshSkills = useCallback(async () => {
    const cfg = await invoke<SkillsConfig>("get_skills_config");
    setItems(buildItems(cfg));
  }, []);

  const resetForNewSelection = useCallback((source: string, cancelInFlight = false) => {
    dbg(`resetForNewSelection called (source=${source}, statusRef=${statusRef.current})`);
    if (cancelInFlight) {
      cancelledRef.current = true;
    } else if (statusRef.current === "loading") {
      return;
    }
    setWindowSize(MENU_SIZE.width, MENU_SIZE.height);
    statusRef.current = "idle";
    setStatus("idle");
    setError(null);
    refreshSkills();
  }, [refreshSkills, setWindowSize]);

  const closeMenu = useCallback((source: string) => {
    resetForNewSelection(source, true);
    invoke("close_bubble_menu").catch(() => {});
  }, [resetForNewSelection]);

  // Rust keeps this prewarmed webview alive and parks it off-screen on close.
  // Explicit events reset stale error/loading state without forcing a reload
  // during the click that opens the menu.
  useEffect(() => {
    dbg("mounted");
    setWindowSize(MENU_SIZE.width, MENU_SIZE.height);
    refreshSkills();

    const onVisibilityChange = () => {
      dbg(`visibilitychange fired, document.visibilityState=${document.visibilityState}`);
      if (document.visibilityState === "visible") resetForNewSelection("visibilitychange");
    };
    document.addEventListener("visibilitychange", onVisibilityChange);

    let unlistenFocus: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        dbg(`onFocusChanged fired, focused=${focused}`);
        if (focused) resetForNewSelection("focusChanged");
      })
      .then((fn) => {
        unlistenFocus = fn;
      });

    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      unlistenFocus?.();
    };
  }, [refreshSkills, resetForNewSelection, setWindowSize]);

  useEffect(() => {
    const unlisteners: Array<() => void> = [];

    listen("bubble-menu:show", () => {
      dbg("bubble-menu:show received");
      resetForNewSelection("showEvent", true);
    }).then((fn) => unlisteners.push(fn));

    listen("bubble-menu:reset", () => {
      dbg("bubble-menu:reset received");
      resetForNewSelection("resetEvent", true);
    }).then((fn) => unlisteners.push(fn));

    listen("selection:detected", () => {
      dbg("selection:detected received");
      resetForNewSelection("selectionDetected", true);
    }).then((fn) => unlisteners.push(fn));

    listen("selection:cleared", () => {
      dbg("selection:cleared received");
      resetForNewSelection("selectionCleared", true);
    }).then((fn) => unlisteners.push(fn));

    return () => unlisteners.forEach((fn) => fn());
  }, [resetForNewSelection]);

  useEffect(() => {
    dbg(`render: status=${status} error=${error} items=${items.length}`);
  }, [status, error, items]);

  async function handleSelect(skillId: string) {
    if (statusRef.current === "loading") {
      dbg(`handleSelect ignored while loading skill=${skillId}`);
      return;
    }
    dbg(`handleSelect start skill=${skillId}`);
    cancelledRef.current = false;
    statusRef.current = "loading";
    setStatus("loading");
    setError(null);
    setWindowSize(SPINNER_WINDOW_SIZE, SPINNER_WINDOW_SIZE);
    try {
      const result = await invoke<string>("rewrite_with_skill", { skillId });
      if (cancelledRef.current) return;
      // paste_text hides this window itself (see commands.rs), which already
      // covers "close the menu after a successful paste".
      const traceId = Date.now();
      dbg(`paste#${traceId}: invoking paste_text result_len=${result.length}`);
      await invoke("paste_text", { result, traceId });
      dbg(`paste#${traceId}: paste_text returned`);
      statusRef.current = "idle";
      setStatus("idle");
    } catch (err) {
      if (cancelledRef.current) return;
      dbg(`handleSelect error: ${String(err)}`);
      setWindowSize(MENU_SIZE.width, MENU_SIZE.height);
      statusRef.current = "error";
      setStatus("error");
      setError(String(err));
    }
  }

  if (status === "loading") {
    return (
      <div
        style={{
          width: "100vw",
          height: "100vh",
          background: "transparent",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontFamily: "'Hanken Grotesk', system-ui, sans-serif",
          userSelect: "none",
        }}
      >
        {/* Same fixed-palette spinner design as Bubble.tsx's dot (see the
            comment there): this floats over arbitrary app content while the
            menu is loading, so it deliberately does not follow --rw-* dark
            mode tokens. */}
        <div style={{ position: "relative", width: 30, height: 30 }}>
          <div
            style={{
              position: "absolute",
              inset: 0,
              borderRadius: "50%",
              background:
                "conic-gradient(from 0deg, #2f6fed, #2ecc71, #f1c40f, #e74c3c, #2f6fed)",
              animation: "rw-spin .55s linear infinite",
              boxShadow: "0 2px 10px rgba(0,0,0,.18)",
            }}
          />
          <div
            style={{
              position: "absolute",
              inset: 4,
              borderRadius: "50%",
              background: "#ffffff",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <img src={logoBlack} alt="" style={{ width: 14, height: "auto", userSelect: "none", pointerEvents: "none" }} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div style={{ width: "100vw", height: "100vh", background: "transparent", fontFamily: "'Hanken Grotesk', system-ui, sans-serif" }}>
      <div
        style={{
          width: "100%",
          height: "100%",
          borderRadius: 8,
          border: "1px solid var(--rw-border)",
          background: "var(--rw-bg-primary)",
          boxShadow: "0 8px 28px rgba(20,20,26,.16), 0 2px 8px rgba(20,20,26,.08)",
          display: "flex",
          flexDirection: "column",
          userSelect: "none",
          overflow: "hidden",
        }}
      >
        {status === "error" && error ? (
          <div style={{ flex: 1, padding: "10px 12px", display: "flex", alignItems: "center" }}>
            {isLimitError(error) ? (
              <p style={{ fontSize: 11, color: "var(--rw-danger)", lineHeight: 1.45, margin: 0 }}>
                Free limit reached. Please{" "}
                <a
                  onClick={() => {
                    // Dismiss the menu first (while its JS is still
                    // foreground) so it doesn't cover the Settings window,
                    // then open Settings without blocking on the round-trip.
                    // Goes through Rust so the menu is parked/reloaded instead
                    // of hidden with stale state.
                    closeMenu("limitLink");
                    invoke("open_settings").catch(() => {});
                  }}
                  style={{ color: "var(--rw-danger)", fontWeight: 700, textDecoration: "underline", cursor: "pointer" }}
                >
                  renew to Pro or Max
                </a>{" "}
                to continue.
              </p>
            ) : (
              <p style={{ fontSize: 11, color: "var(--rw-danger)", lineHeight: 1.45, margin: 0 }}>{error}</p>
            )}
          </div>
        ) : (
          <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 4, display: "flex", flexDirection: "column", gap: 2 }}>
            {items.map((item) => (
              <button
                key={item.id}
                onClick={() => handleSelect(item.id)}
                style={{
                  width: "100%",
                  textAlign: "left",
                  padding: "6px 9px",
                  minHeight: 27,
                  borderRadius: 6,
                  border: "none",
                  background: "transparent",
                  cursor: "pointer",
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: "var(--rw-text-primary)",
                  transition: "background .1s",
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.background = "var(--rw-divider)";
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.background = "transparent";
                }}
              >
                {item.name}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
