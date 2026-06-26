import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

type Status = "idle" | "loading" | "error";

interface Skill {
  id: string;
  name: string;
  instructions: string;
  enabled: boolean;
  order: number;
  base_skill_id?: string | null;
}

interface SkillsConfig {
  global_instructions: string;
  skills: Skill[];
  builtin_enabled: Record<string, boolean>;
}

interface SkillItem {
  id: string;
  name: string;
  description: string;
}

const BUILTIN_ITEMS: SkillItem[] = [
  { id: "__proofread__", name: "Proofread", description: "Fix spelling and grammar while preserving your tone and voice." },
  { id: "__formal_email__", name: "Formal Email", description: "Rewrite as a polished, professional business email." },
  { id: "__summarise__", name: "Summarise (Meeting Notes)", description: "Summarise as concise meeting notes with key points and action items." },
  { id: "__shorten__", name: "Shorten", description: "Shorten the text while preserving its full meaning." },
];

export default function Overlay() {
  const [status, _setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [capturedText, setCapturedText] = useState<string | null>(null);
  const [items, setItems] = useState<SkillItem[]>(BUILTIN_ITEMS);
  const [focusedIndex, setFocusedIndex] = useState(0);

  const statusRef = useRef<Status>("idle");
  const itemsRef = useRef<SkillItem[]>(BUILTIN_ITEMS);

  const setStatus = useCallback((s: Status) => {
    statusRef.current = s;
    _setStatus(s);
  }, []);

  function buildItems(cfg: SkillsConfig): SkillItem[] {
    const builtins = BUILTIN_ITEMS.filter((b) => cfg.builtin_enabled?.[b.id] !== false);

    const enabled = [...cfg.skills]
      .filter((s) => s.enabled)
      .sort((a, b) => a.order - b.order);
    const customItems = enabled.map((s) => {
      let description = s.instructions.trim();
      if (!description) {
        if (s.base_skill_id) {
          const baseName = BUILTIN_ITEMS.find((b) => b.id === s.base_skill_id)?.name
            ?? enabled.find((b) => b.id === s.base_skill_id)?.name;
          description = baseName ? `Based on ${baseName}` : "No additional instructions.";
        } else {
          description = "No additional instructions.";
        }
      }
      return { id: s.id, name: s.name, description };
    });
    return [...builtins, ...customItems];
  }

  async function refreshData() {
    const [text, cfg] = await Promise.all([
      invoke<string | null>("get_captured_text"),
      invoke<SkillsConfig>("get_skills_config"),
    ]);
    setCapturedText(text);
    const list = buildItems(cfg);
    setItems(list);
    itemsRef.current = list;
  }

  useEffect(() => {
    refreshData();

    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (!focused || statusRef.current === "loading") return;
        setStatus("idle");
        setError(null);
        setFocusedIndex(0);
        refreshData();
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => unlisten?.();
  }, [setStatus]);

  // Keyboard navigation
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (statusRef.current === "loading") return;

      if (e.key === "Escape") {
        getCurrentWindow().hide();
        return;
      }

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
    setStatus("loading");
    setError(null);
    try {
      const result = await invoke<string>("rewrite_with_skill", { skillId });
      await invoke("paste_text", { result });
      setStatus("idle");
    } catch (err) {
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
    <div className="flex items-center justify-center w-screen h-screen bg-transparent">
      <div
        className="w-[480px] rounded-2xl border border-white/10 bg-[#0F1117] shadow-[0_0_40px_rgba(0,0,0,0.8)] p-5 select-none"
        style={{ fontFamily: "-apple-system, 'Segoe UI', sans-serif" }}
      >
        {/* Header */}
        <div className="mb-4">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-white/60 text-sm">✦</span>
            <span className="text-white font-medium text-sm">How should this be rewritten?</span>
          </div>
          {preview && (
            <p className="text-white/35 text-xs pl-5 truncate">"{preview}"</p>
          )}
          {!capturedText && status === "idle" && (
            <p className="text-amber-400/80 text-xs pl-5">
              No text captured — highlight some text first.
            </p>
          )}
        </div>

        {/* Skills list */}
        {status !== "loading" && (
          <>
            <div className="space-y-1 max-h-[300px] overflow-y-auto mb-3">
              {items.map((item, i) => {
                const focused = focusedIndex === i;
                return (
                  <button
                    key={item.id}
                    onClick={() => handleSelect(item.id)}
                    onMouseEnter={() => setFocusedIndex(i)}
                    className={`w-full text-left px-3 py-2.5 rounded-lg border transition-all duration-100 ${
                      focused
                        ? "bg-white/12 border-white/20 text-white"
                        : "bg-white/4 border-white/8 text-white/70 hover:bg-white/8 hover:text-white/90"
                    }`}
                  >
                    <div className="flex items-center gap-2">
                      {focused && (
                        <span className="text-white/50 text-xs leading-none">›</span>
                      )}
                      <span className={`text-sm font-medium ${!focused && "pl-4"}`}>
                        {item.name}
                      </span>
                    </div>
                    {focused && item.description && (
                      <p className="text-white/40 text-xs mt-1 pl-4 leading-relaxed line-clamp-2">
                        {item.description}
                      </p>
                    )}
                  </button>
                );
              })}
            </div>

            {status === "error" && error && (
              <p className="text-red-400/90 text-xs mb-3 px-1">{error}</p>
            )}

            <p className="text-white/20 text-xs text-center">
              ↑↓ navigate · Enter select · Esc dismiss
            </p>
          </>
        )}

        {/* Loading state */}
        {status === "loading" && (
          <div className="flex flex-col items-center justify-center py-8 gap-3">
            <div className="flex gap-1">
              {[0, 1, 2].map((i) => (
                <div
                  key={i}
                  className="w-1.5 h-1.5 rounded-full bg-white/50 animate-bounce"
                  style={{ animationDelay: `${i * 150}ms` }}
                />
              ))}
            </div>
            <p className="text-white/40 text-xs">Rewriting…</p>
          </div>
        )}
      </div>
    </div>
  );
}
