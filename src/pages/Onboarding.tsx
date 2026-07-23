import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import slide01 from "../assets/onboarding/01-meet-rewrite.png";
import slide02 from "../assets/onboarding/02-mac-accessibility.png";
import slide03 from "../assets/onboarding/03-where-rewrite-works.png";
import slide04 from "../assets/onboarding/04-highlight.png";
import slide05 from "../assets/onboarding/05-tap-the-bubble.png";
import slide06 from "../assets/onboarding/06-choose-a-skill.png";
import slide07 from "../assets/onboarding/07-done.png";
import slide08 from "../assets/onboarding/08-troubleshooting.png";
import slide09 from "../assets/onboarding/09-outro.png";

// Every slide is a full-bleed 1080x1920 artwork, so the only per-slide data the
// UI needs is the artwork itself plus which way its background leans: the deck
// alternates between the near-black and the cream canvas, and the chrome we
// draw on top (progress segments, Skip, chevrons) has to flip with it or it
// disappears into the image. The canvas `dark` selects also becomes the
// window's own background, so any sub-pixel letterboxing left by
// `object-fit: contain` blends into the artwork instead of banding against it.
type Slide = {
  src: string;
  alt: string;
  dark: boolean;
};

const DARK_CANVAS = "#0e0e0e";
const LIGHT_CANVAS = "#f6f5f2";

const SLIDES: Slide[] = [
  { src: slide01, alt: "Meet reWrite — write better, wherever you work", dark: true },
  { src: slide02, alt: "On a Mac? Enable Accessibility", dark: false },
  { src: slide03, alt: "Where reWrite works", dark: true },
  { src: slide04, alt: "Highlight the text you want to improve", dark: false },
  { src: slide05, alt: "Tap the bubble", dark: false },
  { src: slide06, alt: "Choose a skill", dark: false },
  { src: slide07, alt: "Done — the rewritten text is pasted back", dark: true },
  { src: slide08, alt: "Troubleshooting", dark: false },
  { src: slide09, alt: "reWrite — www.rewriteai.dev", dark: false },
];

// Fraction of the window width that acts as the "go back" tap target. Instagram
// weights the forward tap much heavier than the back tap because forward is the
// motion people actually repeat; the same asymmetry keeps an off-centre click
// from accidentally rewinding the deck.
const BACK_ZONE = 0.3;

export default function Onboarding() {
  const [index, setIndex] = useState(0);
  // Drives the one-time "click to continue" nudge. Tap-to-advance is muscle
  // memory on a phone but not on a desktop window, so the first slide says so
  // out loud until the user makes their first move.
  const [showHint, setShowHint] = useState(true);
  // `finish` can be reached from the last slide's tap, the button, Esc and Skip
  // — all of which stay clickable for the moment it takes the window to hide.
  // Latch so the config write and the Settings hand-off only ever happen once.
  const finishedRef = useRef(false);

  const slide = SLIDES[index];
  const isLast = index === SLIDES.length - 1;
  const canvas = slide.dark ? DARK_CANVAS : LIGHT_CANVAS;

  // Decode every frame up front. The deck is nine ~100 KB PNGs bundled into the
  // app, so this costs nothing over the wire, and it means advancing never
  // shows the blank flash of an image being decoded mid-transition.
  useEffect(() => {
    for (const s of SLIDES) {
      const img = new Image();
      img.src = s.src;
    }
  }, []);

  const finish = useCallback(() => {
    if (finishedRef.current) return;
    finishedRef.current = true;
    // Rust persists `onboarding_completed`, hides this window and opens
    // Settings, so the user lands somewhere useful instead of on an empty
    // desktop wondering whether reWrite is actually running.
    invoke("finish_onboarding").catch(() => {});
  }, []);

  const goNext = useCallback(() => {
    setShowHint(false);
    if (isLast) {
      finish();
      return;
    }
    setIndex((i) => Math.min(i + 1, SLIDES.length - 1));
  }, [isLast, finish]);

  const goPrev = useCallback(() => {
    setShowHint(false);
    setIndex((i) => Math.max(i - 1, 0));
  }, []);

  // Whole-window tap target: left edge rewinds, everything else advances. The
  // Skip / Get started buttons stop propagation so they don't also count as a
  // tap on the zone underneath them.
  const handleTap = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const { left, width } = event.currentTarget.getBoundingClientRect();
      if (event.clientX - left < width * BACK_ZONE) {
        goPrev();
      } else {
        goNext();
      }
    },
    [goNext, goPrev],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      switch (event.key) {
        case "ArrowRight":
        case " ":
        case "Enter":
          event.preventDefault();
          goNext();
          break;
        case "ArrowLeft":
        case "Backspace":
          event.preventDefault();
          goPrev();
          break;
        case "Escape":
          event.preventDefault();
          finish();
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [goNext, goPrev, finish]);

  // Chrome colors, derived from the current slide's canvas rather than the
  // system theme — what matters here is contrast against the artwork.
  const chromeStrong = slide.dark ? "#ffffff" : "#16161a";
  const chromeTrack = slide.dark ? "rgba(255,255,255,0.28)" : "rgba(0,0,0,0.16)";
  const chromeFaint = slide.dark ? "rgba(255,255,255,0.55)" : "rgba(0,0,0,0.45)";
  const chromePill = slide.dark ? "rgba(255,255,255,0.12)" : "rgba(0,0,0,0.06)";
  // The hint is the one piece of chrome that lands in the busy lower half of
  // the artwork, where a translucent tint like `chromePill` disappears against
  // a white card. It gets an opaque-enough scrim of its own instead.
  const hintScrim = slide.dark ? "rgba(0,0,0,0.62)" : "rgba(255,255,255,0.82)";
  const hintText = slide.dark ? "rgba(255,255,255,0.82)" : "rgba(0,0,0,0.62)";

  return (
    <div
      onClick={handleTap}
      style={{
        position: "relative",
        width: "100vw",
        height: "100vh",
        overflow: "hidden",
        borderRadius: 14,
        background: canvas,
        cursor: "pointer",
        userSelect: "none",
        fontFamily: "'Hanken Grotesk', system-ui, sans-serif",
        transition: "background 260ms ease",
      }}
    >
      {/* Re-keying on `index` restarts the fade, so each slide arrives with the
          same soft cross-in rather than snapping into place. */}
      <img
        key={index}
        src={slide.src}
        alt={slide.alt}
        draggable={false}
        style={{
          width: "100%",
          height: "100%",
          objectFit: "contain",
          display: "block",
          animation: "rw-story-in 280ms ease-out both",
        }}
      />

      {/* ── Progress segments ───────────────────────────────────────────────
          One bar per slide, filled up to and including the current one. The
          width transition makes the active segment sweep in as you land on it,
          which reads as forward motion without needing a timer. */}
      <div
        style={{
          position: "absolute",
          top: 12,
          left: 12,
          right: 12,
          display: "flex",
          gap: 4,
        }}
      >
        {SLIDES.map((s, i) => (
          <div
            key={s.src}
            style={{
              flex: 1,
              height: 3,
              borderRadius: 2,
              background: chromeTrack,
              overflow: "hidden",
            }}
          >
            <div
              style={{
                width: i <= index ? "100%" : "0%",
                height: "100%",
                borderRadius: 2,
                background: chromeStrong,
                transition: "width 300ms ease-out",
              }}
            />
          </div>
        ))}
      </div>

      {/* ── Skip ────────────────────────────────────────────────────────────
          Hidden on the last slide, where "Get started" is the same action said
          more warmly. Sits top-right, clear of the reWrite mark every slide
          carries in its top-left corner. */}
      {!isLast && (
        <button
          onClick={(event) => {
            event.stopPropagation();
            finish();
          }}
          style={{
            position: "absolute",
            top: 26,
            right: 12,
            padding: "5px 12px",
            border: "none",
            borderRadius: 999,
            background: chromePill,
            color: chromeFaint,
            fontFamily: "inherit",
            fontSize: 11,
            fontWeight: 600,
            letterSpacing: 0.3,
            cursor: "pointer",
          }}
        >
          Skip
        </button>
      )}

      {/* ── First-run nudge ─────────────────────────────────────────────────
          Disappears permanently after the first interaction of any kind.

          Centred by a full-width parent rather than `translateX(-50%)`: the
          entry animations here all animate `transform`, so a transform used
          for layout gets overwritten the moment the keyframes take hold (with
          `both`, permanently). Same reason the Get started button below is
          wrapped instead of self-centred. */}
      {showHint && (
        <div
          style={{
            position: "absolute",
            bottom: 22,
            left: 0,
            right: 0,
            textAlign: "center",
            pointerEvents: "none",
          }}
        >
          <span
            style={{
              display: "inline-block",
              padding: "6px 14px",
              borderRadius: 999,
              background: hintScrim,
              backdropFilter: "blur(6px)",
              color: hintText,
              fontSize: 11,
              fontWeight: 500,
              letterSpacing: 0.4,
              whiteSpace: "nowrap",
              animation: "rwfade 600ms ease-out both",
            }}
          >
            Click the right side to continue
          </span>
        </div>
      )}

      {/* ── Get started ─────────────────────────────────────────────────────
          The last slide is a plain sign-off, so it needs an explicit exit —
          without it there's nothing to say that one more click closes the
          deck. Tapping anywhere still works. */}
      {isLast && (
        <div style={{ position: "absolute", bottom: 34, left: 0, right: 0, textAlign: "center" }}>
          <button
            onClick={(event) => {
              event.stopPropagation();
              finish();
            }}
            style={{
              padding: "11px 30px",
              border: "none",
              borderRadius: 999,
              background: chromeStrong,
              color: canvas,
              fontFamily: "inherit",
              fontSize: 14,
              fontWeight: 600,
              letterSpacing: 0.2,
              cursor: "pointer",
              boxShadow: "0 6px 20px rgba(0,0,0,0.18)",
              animation: "rw-appear 0.4s cubic-bezier(0.34, 1.56, 0.64, 1) both",
            }}
          >
            Get started
          </button>
        </div>
      )}

      {/* ── Back affordance ─────────────────────────────────────────────────
          A faint chevron marking the rewind zone. Non-interactive: the click
          is handled by the zone itself, this is only the label for it. */}
      {index > 0 && (
        <div
          style={{
            position: "absolute",
            left: 10,
            top: "50%",
            transform: "translateY(-50%)",
            color: chromeFaint,
            fontSize: 20,
            lineHeight: 1,
            opacity: 0.5,
            pointerEvents: "none",
          }}
        >
          ‹
        </div>
      )}
    </div>
  );
}
