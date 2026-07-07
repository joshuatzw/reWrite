import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit, listen } from "@tauri-apps/api/event";
import type { SkillsConfig } from "../types";
import { buildItems, type SkillItem } from "../skills";

type Status = "idle" | "loading" | "error";

const EMPTY_SKILL_ITEMS = buildItems({ global_instructions: "", skills: [], builtin_enabled: {} });

const isLimitError = (msg: string) => /limit|trial|quota|upgrade/i.test(msg);

export default function Overlay() {
  const [status, _setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [capturedText, setCapturedText] = useState<string | null>(null);
  const [items, setItems] = useState<SkillItem[]>(EMPTY_SKILL_ITEMS);
  const [focusedIndex, setFocusedIndex] = useState(0);

  const statusRef = useRef<Status>("idle");
  const itemsRef = useRef<SkillItem[]>(EMPTY_SKILL_ITEMS);
  const cancelledRef = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const setStatus = useCallback((s: Status) => {
    statusRef.current = s;
    _setStatus(s);
  }, []);

  // Single close path used by the X button, the Esc key, and the global Esc
  // hook (via the "overlay:esc" event). The actual hide is delegated to the
  // `close_overlay` Rust command, which runs `overlay.hide()` on the main
  // event-loop thread — the same mechanism the "renew" link uses via
  // `open_settings`. A `hide()` issued straight from JS is silently ignored by
  // Windows when the overlay is the focused foreground window, which is why Esc
  // and the X button previously did nothing.
  const closeOverlay = useCallback(() => {
    cancelledRef.current = true;
    setStatus("idle");
    invoke("close_overlay").catch(() => {});
  }, [setStatus]);

  const refreshData = useCallback(async () => {
    const [text, cfg] = await Promise.all([
      invoke<string | null>("get_captured_text"),
      invoke<SkillsConfig>("get_skills_config"),
    ]);
    setCapturedText(text);
    const list = buildItems(cfg);
    setItems(list);
    itemsRef.current = list;
  }, []);

  // Tell the Rust side the overlay's webview has finished mounting. At startup
  // the window is warmed off-screen; once this fires, Rust hides it again so the
  // first real show is already interactive (see the "warm the overlay" block in
  // lib.rs). Emitting on every mount is harmless — Rust only listens once.
  useEffect(() => {
    emit("overlay:ready");
  }, []);

  useEffect(() => {
    refreshData();

    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (!focused || statusRef.current === "loading") return;
        containerRef.current?.focus();
        setStatus("idle");
        setError(null);
        setFocusedIndex(0);
        refreshData();
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => unlisten?.();
  }, [refreshData, setStatus]);

  // The global Esc hook consumes the keypress before the webview sees it, so
  // it forwards Esc as an event. Route it through the same close handler as
  // the X button. This also covers the case where the overlay is not focused.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("overlay:esc", () => closeOverlay()).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [closeOverlay]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        closeOverlay();
        return;
      }

      if (statusRef.current === "loading") return;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocusedIndex((i) => (i + 1) % itemsRef.current.length);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocusedIndex((i) => (i - 1 + itemsRef.current.length) % itemsRef.current.length);
      } else if (e.key === "Enter") {
        e.preventDefault();
        setFocusedIndex((i) => {
          handleSelect(itemsRef.current[i].id);
          return i;
        });
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  async function handleSelect(skillId: string) {
    cancelledRef.current = false;
    setStatus("loading");
    setError(null);
    try {
      const result = await invoke<string>("rewrite_with_skill", { skillId });
      if (cancelledRef.current) return;
      await invoke("paste_text", { result });
      setStatus("idle");
    } catch (err) {
      if (cancelledRef.current) return;
      setStatus("error");
      setError(String(err));
      getCurrentWindow().show();
    }
  }

  const preview = capturedText
    ? capturedText.length > 60
      ? capturedText.slice(0, 60).trimEnd() + "…"
      : capturedText
    : null;

  return (
    <div ref={containerRef} tabIndex={-1} style={{ outline: "none", width: "100vw", height: "100vh", background: "transparent", fontFamily: "'Hanken Grotesk', system-ui, sans-serif" }}>
      <div style={{
        position: "relative",
        width: "100%", height: "100%", borderRadius: 18,
        border: "1px solid #e0e1e4",
        background: "#fff",
        boxShadow: "0 8px 40px rgba(20,20,26,.16), 0 2px 8px rgba(20,20,26,.08)",
        padding: "20px 20px 16px",
        display: "flex", flexDirection: "column",
        userSelect: "none",
      }}>
        {/* Close button */}
        <button
          onClick={closeOverlay}
          aria-label="Close"
          title="Close (Esc)"
          style={{
            position: "absolute", top: 12, right: 12,
            width: 26, height: 26, borderRadius: 8,
            border: "none", background: "transparent",
            color: "#b6b9bf", cursor: "pointer",
            display: "flex", alignItems: "center", justifyContent: "center",
            fontSize: 18, lineHeight: 1, padding: 0,
            transition: "background .1s, color .1s",
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = "#f0f1f3";
            e.currentTarget.style.color = "#16161a";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "transparent";
            e.currentTarget.style.color = "#b6b9bf";
          }}
        >
          ×
        </button>
        {/* Header */}
        <div style={{ marginBottom: 14 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
            <span style={{ fontFamily: "'Playfair Display', serif", fontWeight: 700, fontSize: 15, color: "#16161a", letterSpacing: -.2 }}>
              How should this be rewritten?
            </span>
          </div>
          {preview && (
            <p style={{ fontSize: 12, color: "#a7aab0", paddingLeft: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              "{preview}"
            </p>
          )}
          {!capturedText && status === "idle" && (
            <p style={{ fontSize: 12, color: "#c0392b", paddingLeft: 0 }}>
              No text captured. Highlight some text first.
            </p>
          )}
        </div>

        {/* Divider */}
        <div style={{ height: 1, background: "#f0f1f3", margin: "0 -20px 12px" }} />

        {/* Skills list */}
        {status !== "loading" && (
          <>
            <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 4, marginBottom: 12 }}>
              {items.map((item, i) => {
                const focused = focusedIndex === i;
                return (
                  <button
                    key={item.id}
                    onClick={() => handleSelect(item.id)}
                    onMouseEnter={() => setFocusedIndex(i)}
                    style={{
                      width: "100%", textAlign: "left",
                      padding: "9px 12px",
                      borderRadius: 10,
                      border: `1px solid ${focused ? "#16161a" : "#e8e9ec"}`,
                      background: focused ? "#16161a" : "#fff",
                      cursor: "pointer",
                      transition: "background .1s, border-color .1s",
                    }}
                  >
                    <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
                      {focused && (
                        <span style={{ color: "#fff", fontSize: 12, lineHeight: 1, flexShrink: 0 }}>›</span>
                      )}
                      <span style={{
                        fontSize: 13.5, fontWeight: 600,
                        color: focused ? "#fff" : "#1f2026",
                        paddingLeft: focused ? 0 : 19,
                      }}>
                        {item.name}
                      </span>
                    </div>
                    {focused && item.description && (
                      <p style={{
                        fontSize: 11.5, color: "rgba(255,255,255,.55)",
                        marginTop: 3, paddingLeft: 19,
                        lineHeight: 1.45,
                        display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden",
                      }}>
                        {item.description}
                      </p>
                    )}
                  </button>
                );
              })}
            </div>

            {status === "error" && error && (
              isLimitError(error) ? (
                <p style={{ fontSize: 12, color: "#c0392b", marginBottom: 8, lineHeight: 1.5 }}>
                  You have used up your free trial limit. Please{" "}
                  <a
                    onClick={() => {
                      // Dismiss the overlay first (while its JS is still
                      // foreground) so it doesn't cover the Settings window,
                      // then open Settings without blocking on the round-trip.
                      getCurrentWindow().hide();
                      invoke("open_settings").catch(() => {});
                    }}
                    style={{ color: "#c0392b", fontWeight: 700, textDecoration: "underline", cursor: "pointer" }}
                  >
                    renew to Pro or Max plans
                  </a>{" "}
                  to continue using reWrite.
                </p>
              ) : (
                <p style={{ fontSize: 12, color: "#c0392b", marginBottom: 8 }}>{error}</p>
              )
            )}

            <p style={{ fontSize: 11, color: "#c4c6cb", textAlign: "center", letterSpacing: .2 }}>
              ↑↓ navigate · Enter select · Esc dismiss
            </p>
          </>
        )}

        {/* Loading state */}
        {status === "loading" && (
          <div style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 10 }}>
            <div style={{ display: "flex", gap: 5 }}>
              {[0, 1, 2].map((i) => (
                <div
                  key={i}
                  style={{
                    width: 6, height: 6, borderRadius: "50%",
                    background: "#16161a",
                    animation: "bounce 1s infinite",
                    animationDelay: `${i * 150}ms`,
                  }}
                />
              ))}
            </div>
            <p style={{ fontFamily: "'Playfair Display', serif", fontStyle: "italic", fontSize: 13, color: "#9a9da3" }}>Rewriting…</p>
          </div>
        )}
      </div>
    </div>
  );
}
