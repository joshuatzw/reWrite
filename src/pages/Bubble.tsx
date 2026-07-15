import { invoke } from "@tauri-apps/api/core";
import logoBlack from "../assets/logo_transparent.png";

// Tiny clickable dot filling the bubble window. Rust already positions the
// window at the selection anchor on every `selection:detected` event (see
// show_bubble in lib.rs). The click handler used to read the anchor from a
// `selection:detected` listener of its own, but a live trace session showed
// that listener could still be empty by the time a (fast) human click
// arrived — event delivery into a webview that was hidden a moment earlier
// isn't instant. `bubble_clicked` now reads the anchor straight from Rust's
// own last-known state instead, so this click handler needs no payload at all.
export default function Bubble() {
  function handleClick() {
    invoke("bubble_clicked").catch(() => {});
  }

  return (
    <div
      onClick={handleClick}
      style={{
        width: "100vw",
        height: "100vh",
        background: "transparent",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
        userSelect: "none",
      }}
    >
      {/* Outer ring: a conic-gradient rotated slowly via the shared rw-spin
          keyframe (src/index.css) so the outline cycles blue -> green ->
          yellow -> red -> blue. The circular new icon on top masks all but the
          ring, with the reWrite logo centered on it — chosen for contrast against
          dark editor themes (e.g. VS Code) where the original near-black dot
          was invisible, and recognizable as this app's own affordance rather
          than a generic dot.

          Deliberately NOT wired to the --rw-* theme tokens (macOS dark mode
          support, see src/theme.css): this dot floats over arbitrary,
          unpredictable app content (a light Notes doc, a dark VS Code theme,
          anything), not over reWrite's own chrome, so its colors need to stay
          fixed for legibility against whatever is behind it rather than
          follow the user's system light/dark appearance — the same reasoning
          already written down above for why it's white-on-conic-gradient
          instead of a plain dot. Reconsider only if this needs to blend with
          reWrite's own surfaces instead of contrast against arbitrary ones. */}
      <div style={{ position: "relative", width: 30, height: 30 }}>
        <div
          style={{
            position: "absolute",
            inset: 0,
            borderRadius: "50%",
            background:
              "conic-gradient(from 0deg, #2f6fed, #2ecc71, #f1c40f, #e74c3c, #2f6fed)",
            animation: "rw-spin 6s linear infinite",
          }}
        />
        <div
          style={{
            position: "absolute",
            inset: 3.75,
            borderRadius: "50%",
            overflow: "hidden",
            background: "#ffffff",
            boxShadow: "0 0 3px 1px rgba(0,0,0,0.25)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <img
            src={logoBlack}
            alt=""
            style={{ width: "100%", height: "100%", objectFit: "cover", borderRadius: "50%", userSelect: "none", pointerEvents: "none" }}
          />
        </div>
      </div>
    </div>
  );
}
