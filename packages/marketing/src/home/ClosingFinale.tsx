// ============================================================================
// Skill Studio - Closing finale
// The download section and footer over the desktop app's welcome dither, which
// glows up from the bottom and fills a giant "Skill Studio" cut off by the page edge
// ============================================================================

import { useLayoutEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import * as stylex from "@stylexjs/stylex";
import { Dithering } from "@paper-design/shaders-react";

import { siteLayout } from "../SiteChrome";
import { finaleCtaMarker } from "./ClosingFinale.stylex";
import type { SiteTheme } from "../SiteTheme.stylex";

const WORDMARK_TEXT = "Skill Studio";
const WORDMARK_FONT =
  'system-ui, -apple-system, BlinkMacSystemFont, "SF Pro Display", "Segoe UI", sans-serif';
// The share of the letters' height that the page edge cuts off.
const WORDMARK_CROP = 0.28;
const DITHER_SPEED = 0.35;

// Same colours as the desktop app's welcome screen (apps/desktop FirstRunScreen).
const DITHER_COLORS = {
  dark: { back: "#150b2e", front: "#5a45c4" },
  light: { back: "#e4dcff", front: "#5a45c4" },
} as const;

// The faint glow that rises behind the footer. The letters themselves show at full strength.
const GLOW_MASK =
  "radial-gradient(ellipse 120% var(--finale-glow-reach) at 50% 100%, rgb(0 0 0 / var(--finale-glow-alpha)), transparent)";

interface Wordmark {
  url: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

function drawWordmark(width: number) {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const font = (px: number) => `720 ${px}px ${WORDMARK_FONT}`;
  ctx.font = font(100);
  ctx.letterSpacing = "-4px";
  const probe = ctx.measureText(WORDMARK_TEXT);
  const size = (100 * width) / (probe.actualBoundingBoxLeft + probe.actualBoundingBoxRight);
  const tracking = `${-0.04 * size}px`;
  ctx.font = font(size);
  ctx.letterSpacing = tracking;
  const metrics = ctx.measureText(WORDMARK_TEXT);
  const ascent = metrics.actualBoundingBoxAscent;
  const pad = Math.round(size * 0.04);
  const height = Math.round(pad + ascent * (1 - WORDMARK_CROP));
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  ctx.scale(dpr, dpr);
  ctx.font = font(size);
  ctx.letterSpacing = tracking;
  ctx.fillText(WORDMARK_TEXT, metrics.actualBoundingBoxLeft, pad + ascent);
  return { url: canvas.toDataURL(), height };
}

/** Draws the wordmark to fit the slot's content box, positioned relative to the finale. */
function useWordmark(
  finaleRef: React.RefObject<HTMLElement | null>,
  slotRef: React.RefObject<HTMLElement | null>,
) {
  const [wordmark, setWordmark] = useState<Wordmark | null>(null);
  useLayoutEffect(() => {
    const finale = finaleRef.current;
    const slot = slotRef.current;
    if (!finale || !slot) return;
    let cancelled = false;
    let drawnWidth = 0;
    let drawn: { url: string; height: number } | null = null;
    const update = () => {
      const style = getComputedStyle(slot);
      const paddingLeft = Number.parseFloat(style.paddingLeft);
      const width = slot.clientWidth - paddingLeft - Number.parseFloat(style.paddingRight);
      if (width <= 0) return;
      if (width !== drawnWidth) {
        drawn = drawWordmark(width);
        drawnWidth = width;
      }
      if (!drawn || cancelled) return;
      const next = {
        url: drawn.url,
        height: drawn.height,
        width,
        x: slot.offsetLeft + paddingLeft,
        // The slot sits at the bottom, so its top is the finale's height minus its own.
        y: finale.clientHeight - drawn.height,
      };
      setWordmark((prev) =>
        prev && prev.url === next.url && prev.x === next.x && prev.y === next.y ? prev : next,
      );
    };
    void document.fonts.ready.then(() => !cancelled && update());
    const observer = new ResizeObserver(update);
    observer.observe(finale);
    return () => {
      cancelled = true;
      observer.disconnect();
    };
  }, [finaleRef, slotRef]);
  return wordmark;
}

const subscribeReducedMotion = (onChange: () => void) => {
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
};

function useInView(ref: React.RefObject<HTMLElement | null>) {
  const [inView, setInView] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new IntersectionObserver(([entry]) => setInView(entry.isIntersecting));
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return inView;
}

interface ClosingFinaleProps {
  theme: SiteTheme;
  children: ReactNode;
}

export function ClosingFinale({ theme, children }: ClosingFinaleProps) {
  const finaleRef = useRef<HTMLDivElement>(null);
  const slotRef = useRef<HTMLDivElement>(null);
  const wordmark = useWordmark(finaleRef, slotRef);
  const reducedMotion = useSyncExternalStore(
    subscribeReducedMotion,
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    () => true,
  );
  // The shader only runs while the finale is on screen.
  const inView = useInView(finaleRef);
  const colors = DITHER_COLORS[theme];

  return (
    <div ref={finaleRef} {...stylex.props(styles.finale)}>
      {wordmark && (
        <div
          aria-hidden="true"
          {...stylex.props(styles.dither)}
          style={{
            maskImage: `url(${wordmark.url}), ${GLOW_MASK}`,
            maskPosition: `${wordmark.x}px ${wordmark.y}px, 0 0`,
            maskRepeat: "no-repeat",
            maskSize: `${wordmark.width}px ${wordmark.height}px, 100% 100%`,
          }}
        >
          <Dithering
            width="100%"
            height="100%"
            colorBack={colors.back}
            colorFront={colors.front}
            // oxlint-disable-next-line anti-slop/no-shape-in-symbol-names -- Paper Shaders' own prop name
            shape="warp"
            type="4x4"
            size={3}
            scale={0.8}
            speed={inView && !reducedMotion ? DITHER_SPEED : 0}
          />
        </div>
      )}
      {children}
      <div
        ref={slotRef}
        aria-hidden="true"
        {...stylex.props(siteLayout.container)}
        style={{ height: wordmark?.height ?? 0 }}
      />
    </div>
  );
}

const styles = stylex.create({
  finale: {
    isolation: "isolate",
    position: "relative",
    // The glow swells toward the download button while it is hovered.
    "--finale-glow-alpha": {
      default: 0.3,
      "@media (hover: hover) and (pointer: fine)": {
        [stylex.when.descendant(":hover", finaleCtaMarker)]: 0.6,
      },
    },
    "--finale-glow-reach": {
      default: "60%",
      "@media (hover: hover) and (pointer: fine)": {
        [stylex.when.descendant(":hover", finaleCtaMarker)]: "95%",
      },
    },
    transition:
      "--finale-glow-alpha 500ms cubic-bezier(0.32, 0.72, 0, 1), --finale-glow-reach 500ms cubic-bezier(0.32, 0.72, 0, 1)",
  },
  dither: {
    inset: 0,
    pointerEvents: "none",
    position: "absolute",
    userSelect: "none",
    zIndex: -1,
  },
});
