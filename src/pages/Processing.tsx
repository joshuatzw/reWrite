import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getCurrentWindow,
  primaryMonitor,
  LogicalPosition,
  LogicalSize,
} from "@tauri-apps/api/window";
import logoBlack from "../assets/rewrite_logo_black_transparent.png";

type Variant = "normal" | "limit";

// Window footprint per variant. The window is transparent, so it only needs to
// be large enough to contain the visible element plus its glow.
const SIZES: Record<Variant, { w: number; h: number }> = {
  normal: { w: 240, h: 240 },
  limit: { w: 460, h: 200 },
};

const LIMIT_TEXT =
  "You have used up your free trial limit. Please renew to Pro or Max plans to continue using reWrite.";

// Resize and re-anchor to bottom-center of the primary monitor. Runs on mount
// and on every show so a pre-warmed (initially centered) window always lands
// correctly and matches the current variant.
async function layoutBottomCenter(variant: Variant) {
  try {
    const win = getCurrentWindow();
    const monitor = await primaryMonitor();
    if (!monitor) return;
    const sf = monitor.scaleFactor;
    const { width, height } = monitor.size;
    const { x: mx, y: my } = monitor.position;
    const { w, h } = SIZES[variant];
    await win.setSize(new LogicalSize(w, h));
    const lx = mx / sf + (width / sf - w) / 2;
    const ly = my / sf + height / sf - h - 24;
    await win.setPosition(new LogicalPosition(lx, ly));
  } catch (_) {}
}

export default function Processing() {
  const [animKey, setAnimKey] = useState(0);
  const [variant, setVariant] = useState<Variant>("normal");

  useEffect(() => {
    layoutBottomCenter("normal");
  }, []);

  useEffect(() => {
    const unlisteners: Array<() => void> = [];

    listen("processing:show", () => {
      setVariant("normal");
      layoutBottomCenter("normal");
      setAnimKey((k) => k + 1);
    }).then((fn) => unlisteners.push(fn));

    listen("processing:limit", () => {
      setVariant("limit");
      layoutBottomCenter("limit");
      setAnimKey((k) => k + 1);
    }).then((fn) => unlisteners.push(fn));

    return () => unlisteners.forEach((fn) => fn());
  }, []);

  return (
    <div
      style={{
        width: "100vw",
        height: "100vh",
        background: "transparent",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        fontFamily: "'Playfair Display', serif",
      }}
    >
      <div
        key={animKey}
        style={{ animation: "rw-appear 0.4s cubic-bezier(0.34, 1.56, 0.64, 1) both" }}
      >
        {variant === "limit" ? (
          // Pill with the trial-limit message and a red glow. Background
          // follows the brand accent token (src/theme.css); the glow
          // box-shadow colors are left as literals — this is a floating,
          // self-contained notification badge (like Bubble.tsx's dot) whose
          // red/white glow is meant to read as "alert" against any
          // background, not to match the system light/dark appearance.
          <div
            style={{
              maxWidth: 420,
              padding: "16px 28px",
              borderRadius: 26,
              background: "var(--rw-accent)",
              color: "var(--rw-on-accent)",
              fontFamily: "'Hanken Grotesk', system-ui, sans-serif",
              fontSize: 14,
              fontWeight: 500,
              lineHeight: 1.5,
              textAlign: "center",
              userSelect: "none",
              boxShadow:
                "0 0 10px 3px rgba(255,90,90,0.45), 0 0 28px 9px rgba(200,40,40,0.50), 0 0 0 1px rgba(255,120,120,0.25)",
            }}
          >
            {LIMIT_TEXT}
          </div>
        ) : (
          // Opaque white circle carrying the black logo, with a faint
          // grey/white glow. Background is literal white so the black-ink
          // mark reads clearly; glow shadow stays literal too (floating
          // badge, contrast-against-anything by design).
          <div
            style={{
              width: 77,
              height: 77,
              borderRadius: "50%",
              background: "#ffffff",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              boxShadow:
                "0 0 7px 2px rgba(255,255,255,0.28), 0 0 18px 5px rgba(160,160,168,0.35), 0 0 0 1px rgba(255,255,255,0.10)",
              animation: "rw-pulse 2s ease-in-out infinite",
            }}
          >
            <img
              src={logoBlack}
              alt="reWrite"
              style={{ height: 44, width: 44, borderRadius: "50%", objectFit: "cover", userSelect: "none" }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
