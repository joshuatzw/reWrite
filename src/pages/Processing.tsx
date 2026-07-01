import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, primaryMonitor, LogicalPosition } from "@tauri-apps/api/window";
import logoWhite from "../assets/rewrite_logo_white.png";

export default function Processing() {
  const [animKey, setAnimKey] = useState(0);

  useEffect(() => {
    // Position window at bottom-center of primary monitor
    (async () => {
      try {
        const win = getCurrentWindow();
        const monitor = await primaryMonitor();
        if (!monitor) return;
        const sf = monitor.scaleFactor;
        const { width, height } = monitor.size;
        const { x: mx, y: my } = monitor.position;
        const winW = 160;
        const winH = 160;
        const lx = mx / sf + (width / sf - winW) / 2;
        const ly = my / sf + height / sf - winH - 80;
        await win.setPosition(new LogicalPosition(lx, ly));
      } catch (_) {}
    })();
  }, []);

  useEffect(() => {
    // Replay appear animation each time the window is shown
    let unlisten: (() => void) | undefined;
    listen("processing:show", () => {
      setAnimKey((k) => k + 1);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
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
        <div style={{ position: "relative", width: 112, height: 112 }}>
          {/* Spinning outer ring */}
          <div
            style={{
              position: "absolute",
              inset: 0,
              borderRadius: "50%",
              border: "2.5px solid rgba(255,255,255,0.1)",
              borderTopColor: "rgba(255,255,255,0.88)",
              borderRightColor: "rgba(255,255,255,0.28)",
              animation: "rw-spin 1.4s linear infinite",
            }}
          />
          {/* Inner pulsing circle with logo */}
          <div
            style={{
              position: "absolute",
              inset: 10,
              borderRadius: "50%",
              background: "rgba(22, 22, 26, 0.86)",
              backdropFilter: "blur(20px) saturate(1.5)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              boxShadow:
                "0 12px 40px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.07)",
              animation: "rw-pulse 2s ease-in-out infinite",
            }}
          >
            <img
              src={logoWhite}
              alt="reWrite"
              style={{ height: 34, width: "auto", opacity: 0.92, userSelect: "none" }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
