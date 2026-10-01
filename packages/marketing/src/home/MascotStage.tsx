// ============================================================================
// Skill Studio - Mascot stage
// The app icon with a messy pile of skills around it. Pressing the mascot tidies the
// pile: kept skills line up, and the broken, unused and duplicate ones are parked.
// ============================================================================
import { useLayoutEffect, useRef, useState } from "react";
import * as stylex from "@stylexjs/stylex";

import { siteTokens } from "../SiteTheme.stylex";

type ChipState = "ok" | "outdated" | "broken" | "unused" | "duplicate";

interface Chip {
  name: string;
  state: ChipState;
  messy: [x: number, y: number, r: number];
  tidyLeft: boolean;
}

// Messy offsets are from the mascot centre, in px on a full-width stage.
const chips: ReadonlyArray<Chip> = [
  { name: "agent-browser", state: "ok", messy: [-335, -20, -9], tidyLeft: true },
  { name: "motion", state: "broken", messy: [-215, 40, 14], tidyLeft: true },
  { name: "commit", state: "ok", messy: [-330, 96, 6], tidyLeft: true },
  { name: "pdf-tools", state: "unused", messy: [-205, -150, -14], tidyLeft: true },
  { name: "frontend-design", state: "ok", messy: [-260, 150, -4], tidyLeft: true },
  { name: "release-notes", state: "unused", messy: [262, -128, 9], tidyLeft: false },
  { name: "tailwind", state: "ok", messy: [300, -40, -12], tidyLeft: false },
  { name: "writing-skills", state: "duplicate", messy: [205, 54, 8], tidyLeft: false },
  { name: "prepare-videos", state: "ok", messy: [320, 118, -7], tidyLeft: false },
  { name: "deploy-preview", state: "unused", messy: [190, 160, 15], tidyLeft: false },
  { name: "skill-creator", state: "outdated", messy: [105, -178, -6], tidyLeft: false },
];

// Outdated skills stay in the tidy set: tidying updates them instead of parking them.
const isKept = (chip: Chip) => chip.state === "ok" || chip.state === "outdated";
const PARKED_COUNT = chips.filter((chip) => !isKept(chip)).length;
const UPDATED_COUNT = chips.filter((chip) => chip.state === "outdated").length;
const TIDY_COLUMNS = [true, false].map((left) =>
  chips.filter((chip) => isKept(chip) && chip.tidyLeft === left),
);
const TIDY_ROWS = Math.max(...TIDY_COLUMNS.map((column) => column.length));
const TIDY_GAP = 12;
const ROW_GAP = 10;
const MASCOT_LIFT = -4;
// Messy offsets shrink with the stage below its full 860px width. Pure CSS, so the
// prerendered pile already fits before any script runs.
const STAGE_UNIT = "min(1px, 100cqw / 860)";
const INTRO_SEEN_KEY = "skill-studio:mascot-intro";

const stateLabel = {
  ok: "used",
  outdated: "update",
  broken: "broken",
  unused: "unused",
  duplicate: "duplicate",
} satisfies Record<ChipState, string>;

interface StageMetrics {
  stageWidth: number;
  mascotWidth: number;
  mascotHeight: number;
  chipWidth: number;
  chipHeight: number;
}

interface TidyLayout {
  // translateX anchor in %: -100 pins a chip's right edge to x, 0 its left edge.
  chips: Map<string, { x: number; y: number; anchor: number }>;
  mascotY: number;
}

function measureStage(stage: HTMLElement, mascot: HTMLElement): StageMetrics {
  const kept = Array.from(stage.querySelectorAll<HTMLElement>("[data-kept]"));
  return {
    stageWidth: stage.clientWidth,
    mascotWidth: mascot.offsetWidth,
    mascotHeight: mascot.offsetHeight,
    chipWidth: Math.max(...kept.map((chip) => chip.offsetWidth)),
    chipHeight: kept[0]?.offsetHeight ?? 0,
  };
}

// Columns sit beside the mascot when the widest kept chip fits there; otherwise they
// stack under it and the mascot moves up to make room.
function tidyLayout(metrics: StageMetrics): TidyLayout {
  const rowStep = metrics.chipHeight + ROW_GAP;
  const besideMascot =
    metrics.stageWidth / 2 >= metrics.mascotWidth / 2 + TIDY_GAP + metrics.chipWidth;
  const top = -(metrics.mascotHeight + TIDY_GAP + TIDY_ROWS * rowStep - ROW_GAP) / 2;
  const layout: TidyLayout = {
    chips: new Map(),
    mascotY: besideMascot ? MASCOT_LIFT : top + metrics.mascotHeight / 2,
  };
  TIDY_COLUMNS.forEach((column, index) => {
    const side = index === 0 ? -1 : 1;
    column.forEach((chip, slot) => {
      layout.chips.set(chip.name, {
        anchor: side < 0 ? -100 : 0,
        x: side * (besideMascot ? metrics.mascotWidth / 2 + TIDY_GAP : TIDY_GAP / 2),
        y: besideMascot
          ? (slot - (column.length - 1) / 2) * rowStep
          : top + metrics.mascotHeight + TIDY_GAP + slot * rowStep + metrics.chipHeight / 2,
      });
    });
  });
  return layout;
}

function useStageMetrics() {
  const stageRef = useRef<HTMLDivElement>(null);
  const mascotRef = useRef<HTMLButtonElement>(null);
  const [metrics, setMetrics] = useState<StageMetrics | null>(null);
  useLayoutEffect(() => {
    const stage = stageRef.current;
    const mascot = mascotRef.current;
    if (!stage || !mascot) return;
    const measure = () => setMetrics(measureStage(stage, mascot));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);
  return { stageRef, mascotRef, metrics };
}

function readIntroSeen() {
  try {
    return sessionStorage.getItem(INTRO_SEEN_KEY) === "1";
  } catch {
    return false;
  }
}

function markIntroSeen() {
  try {
    sessionStorage.setItem(INTRO_SEEN_KEY, "1");
  } catch {
    // Private windows can block storage; the intro then plays on every visit.
  }
}

function chipTransform(chip: Chip, tidy: boolean, layout: TidyLayout | null) {
  const placed = tidy ? layout?.chips.get(chip.name) : undefined;
  if (placed)
    return `translate(${placed.anchor}%, -50%) translate(${placed.x}px, ${placed.y}px) rotate(0deg)`;
  const [x, y, rotation] = chip.messy;
  return `translate(-50%, -50%) translate(calc(${x} * ${STAGE_UNIT}), calc(${y} * ${STAGE_UNIT})) rotate(${tidy ? 0 : rotation}deg)`;
}

function MascotChips({
  tidy,
  animate,
  layout,
}: {
  tidy: boolean;
  animate: boolean;
  layout: TidyLayout | null;
}) {
  return chips.map((chip, index) => {
    const kept = isKept(chip);
    const updated = tidy && chip.state === "outdated";
    return (
      <span
        key={chip.name}
        aria-hidden="true"
        data-kept={kept || undefined}
        {...stylex.props(
          styles.chip,
          styles.chipPosition(chipTransform(chip, tidy, layout), index * 28),
          tidy && !kept && styles.chipParked,
          !animate && styles.still,
        )}
      >
        <span
          {...stylex.props(
            styles.dot,
            updated ? styles.ok : styles[chip.state],
            !animate && styles.still,
          )}
        />
        {chip.name}
        <span {...stylex.props(styles.chipState)}>
          {updated ? "updated" : stateLabel[chip.state]}
        </span>
      </span>
    );
  });
}

export function MascotStage() {
  // Prerendered HTML shows the messy pile; the intro tidies it once per session.
  const [tidy, setTidy] = useState(false);
  // Off until the first paint, so a return visit shows the tidy pile without replaying it.
  const [animate, setAnimate] = useState(false);
  const { stageRef, mascotRef, metrics } = useStageMetrics();
  const layout = metrics ? tidyLayout(metrics) : null;

  useLayoutEffect(() => {
    if (readIntroSeen()) {
      setTidy(true);
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setAnimate(true));
      });
      return () => cancelAnimationFrame(frame);
    }
    setAnimate(true);
    const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    // Marked when the intro plays, not on mount: StrictMode's second effect run in dev
    // would otherwise take the return-visit path.
    const timer = window.setTimeout(
      () => {
        markIntroSeen();
        setTidy(true);
      },
      reduceMotion ? 0 : 900,
    );
    return () => window.clearTimeout(timer);
  }, []);

  return (
    <div {...stylex.props(styles.visual)}>
      <div ref={stageRef} {...stylex.props(styles.stage)}>
        <MascotChips tidy={tidy} animate={animate} layout={layout} />
        <button
          ref={mascotRef}
          type="button"
          aria-pressed={tidy}
          aria-label="Tidy the skills"
          onClick={() => setTidy((current) => !current)}
          {...stylex.props(
            styles.mascotButton,
            tidy && layout && styles.mascotLift(layout.mascotY),
            !animate && styles.still,
          )}
        >
          <img
            src="/skill-studio-logo.png"
            alt=""
            width={168}
            height={168}
            {...stylex.props(styles.mascot)}
          />
        </button>
      </div>
      <p {...stylex.props(styles.hint)} aria-live="polite">
        {tidy
          ? `${PARKED_COUNT} skills parked, ${UPDATED_COUNT} updated. Press the mascot to undo.`
          : "Press the mascot to tidy up."}
      </p>
    </div>
  );
}

const settle = "cubic-bezier(0.32, 0.72, 0, 1)";

const styles = stylex.create({
  visual: { textAlign: "center", width: "100%" },
  stage: {
    alignItems: "center",
    display: "flex",
    height: 340,
    justifyContent: "center",
    containerType: "inline-size",
    position: "relative",
    width: "100%",
    "@media (max-width: 720px)": { height: 260 },
  },
  mascotButton: {
    backgroundColor: "transparent",
    borderRadius: 32,
    borderWidth: 0,
    cursor: "pointer",
    padding: 8,
    position: "relative",
    transition: `transform 700ms ${settle}`,
    WebkitTapHighlightColor: "transparent",
    ":focus-visible": { outline: `2px solid ${siteTokens.text}`, outlineOffset: 4 },
    "@media (prefers-reduced-motion: reduce)": { transition: "none" },
  },
  mascotLift: (y: number) => ({ transform: `translateY(${y}px)` }),
  mascot: {
    display: "block",
    filter: "drop-shadow(0 24px 40px oklch(0.45 0.2 293 / .45))",
    height: "auto",
    transition: `transform 500ms ${settle}`,
    width: 168,
    "@media (hover: hover) and (pointer: fine)": {
      ":hover": { transform: "rotate(-5deg) scale(1.04)" },
    },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 720px)": { width: 112 },
    "@media (prefers-reduced-motion: reduce)": { transition: "none" },
  },
  chip: {
    alignItems: "center",
    backgroundColor: siteTokens.surface,
    borderColor: siteTokens.border,
    borderRadius: 999,
    borderStyle: "solid",
    borderWidth: 1,
    boxShadow: "0 1px 2px oklch(0 0 0 / .12), 0 6px 18px oklch(0 0 0 / .14)",
    color: siteTokens.text,
    display: "inline-flex",
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: 13,
    gap: 8,
    height: 36,
    left: "50%",
    paddingInline: "12px 14px",
    pointerEvents: "none",
    position: "absolute",
    top: "50%",
    transition: `transform 700ms ${settle}, opacity 400ms ease-out`,
    userSelect: "none",
    whiteSpace: "nowrap",
    "@media (max-width: 720px)": { fontSize: 11, gap: 6, height: 28, paddingInline: "9px 10px" },
    "@media (prefers-reduced-motion: reduce)": { transition: "opacity 200ms ease-out" },
  },
  chipPosition: (transform: string, delay: number) => ({
    transform,
    transitionDelay: `${delay}ms`,
  }),
  chipParked: { opacity: 0 },
  chipState: {
    color: siteTokens.muted,
    fontFamily: "-apple-system, BlinkMacSystemFont, sans-serif",
    fontSize: 11,
    "@media (max-width: 720px)": { display: "none" },
  },
  dot: {
    borderRadius: 999,
    flexShrink: 0,
    height: 7,
    transition: "background-color 300ms ease-out 500ms",
    width: 7,
  },
  ok: { backgroundColor: "oklch(0.75 0.17 150)" },
  outdated: { backgroundColor: "oklch(0.72 0.15 240)" },
  broken: { backgroundColor: "oklch(0.66 0.21 25)" },
  unused: { backgroundColor: "oklch(0.6 0.02 290)" },
  duplicate: { backgroundColor: "oklch(0.8 0.15 80)" },
  still: { transition: "none" },
  hint: {
    color: siteTokens.muted,
    fontSize: 13,
    margin: "6px 0 0",
    minHeight: 20,
  },
});
