// ============================================================================
// ActivityHeatmap - Year heatmap with Monday-aligned weeks, quantile shading,
// and one shared hover card. Pointer hover waits 150ms for the first card,
// then moves instantly between days. Arrow keys move the active day and show
// its card.
// ============================================================================

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, PointerEvent, ReactNode } from "react";
import { createPortal } from "react-dom";
import { formatDay, uses } from "@skill-studio/lib";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const SHADES = [0, 18, 36, 58, 82];
const OPEN_DELAY_MS = 150;
const REOPEN_GRACE_MS = 300;

function levels(days: Record<string, number>): (count: number) => number {
  const sorted = Object.values(days)
    .filter((n) => n > 0)
    .sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.floor(q * (sorted.length - 1))] ?? 0;
  const cuts = [at(0.25), at(0.5), at(0.75)];
  return (count) => (count <= 0 ? 0 : 1 + cuts.filter((c) => count > c).length);
}

function shade(level: number, tint: string): string | undefined {
  if (level === 0) return undefined;
  return `color-mix(in oklch, ${tint} ${SHADES[level]}%, var(--color-bg-tertiary))`;
}

/**
 * The active day, defaulting to the most recent date and following `selected` when it changes.
 * Adjusts during render rather than in an effect, so the heatmap never paints a frame with the
 * previous day still active. See
 * https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes
 */
function useActiveKey(dates: string[], selected: string | null) {
  const [activeKey, setActiveKey] = useState(dates[dates.length - 1]);
  const [prevSelected, setPrevSelected] = useState(selected);
  if (selected !== prevSelected) {
    setPrevSelected(selected);
    if (selected) setActiveKey(selected);
  }
  return [activeKey, setActiveKey] as const;
}

interface GridProps {
  slots: (string | null)[];
  days: Record<string, number>;
  idPrefix: string;
  activeKey: string;
  selected: string | null;
  tint: string;
}

function Grid({ slots, days, idPrefix, activeKey, selected, tint }: GridProps) {
  const levelOf = levels(days);
  return slots.map((key, i) => {
    if (!key) return <div key={`blank-${i}`} role="presentation" data-blank="" />;
    const count = days[key] ?? 0;
    return (
      <div
        key={key}
        id={`${idPrefix}-${key}`}
        role="option"
        aria-selected={selected === key}
        aria-label={`${formatDay(key)}: ${uses(count)}`}
        data-key={key}
        data-active={activeKey === key ? "" : undefined}
        data-selected={selected === key ? "true" : undefined}
        className="aspect-square cursor-default rounded-[2px] bg-bg-tertiary outline-offset-1 group-focus-visible:data-active:outline-2 group-focus-visible:data-active:outline-text-secondary data-selected:z-1 data-selected:ring-1 data-selected:ring-text-primary data-selected:ring-offset-1 data-selected:ring-offset-bg-primary"
        style={{ backgroundColor: shade(levelOf(count), tint) }}
      />
    );
  });
}

interface ActivityHeatmapProps {
  /** Inclusive local "YYYY-MM-DD" day keys, oldest first - from `heatmapDateRangeLocal`. */
  dates: string[];
  days: Record<string, number>;
  label: string;
  /** Card body for a day; null keeps the card closed for that day. */
  renderCard: (dayKey: string) => ReactNode;
  cardWidth: number;
  selected?: string | null;
  onSelect?: (dayKey: string | null) => void;
  tint?: string;
}

export function ActivityHeatmap({
  dates,
  days,
  label,
  renderCard,
  cardWidth,
  selected: selectedProp,
  onSelect,
  tint: tintProp,
}: ActivityHeatmapProps) {
  const selected = selectedProp ?? null;
  const tint = tintProp ?? "var(--color-accent)";
  const idPrefix = useId().replace(/:/g, "");
  const [hoverKey, setHoverKey] = useState<string | null>(null);
  const [activeKey, setActiveKey] = useActiveKey(dates, selected);
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const [openCount, setOpenCount] = useState(0);
  const timer = useRef<number | undefined>(undefined);
  const closedAt = useRef(0);
  const cardRef = useRef<HTMLDivElement>(null);

  // Monday-aligned lead offset: the grid's first row is always Monday, so a
  // date range that starts on any other weekday leaves blank slots before it
  // instead of misaligning every day in the grid by that many rows.
  const lead = (new Date(`${dates[0]}T00:00:00`).getDay() + 6) % 7;
  const weeks = Math.ceil((lead + dates.length) / 7);
  const slots: (string | null)[] = Array.from({ length: weeks * 7 }, (_, i) =>
    i >= lead && i < lead + dates.length ? dates[i - lead] : null,
  );

  const monthLabels: { col: number; span: number; label: string }[] = [];
  let lastMonth = -1;
  for (let col = 0; col < weeks; col++) {
    const first = slots.slice(col * 7, col * 7 + 7).find((k) => k !== null);
    if (!first) continue;
    const month = Number(first.slice(5, 7)) - 1;
    if (month !== lastMonth) {
      if (monthLabels.length) {
        const previous = monthLabels[monthLabels.length - 1];
        previous.span = col - previous.col;
      }
      monthLabels.push({ col, span: weeks - col, label: MONTHS[month] });
      lastMonth = month;
    }
  }
  // A sliver of a month at the left edge has no room for its label.
  const visibleMonthLabels = monthLabels.filter((m) => m.span >= 2);

  const shownKey = hoverKey ?? (keyboardOpen ? activeKey : null);
  const isOpen = shownKey !== null;

  useEffect(() => () => window.clearTimeout(timer.current), []);

  useLayoutEffect(() => {
    const card = cardRef.current;
    const cell = shownKey ? document.getElementById(`${idPrefix}-${shownKey}`) : null;
    if (!card || !cell) return;
    const r = cell.getBoundingClientRect();
    const h = card.offsetHeight;
    const gap = 8;
    const below = r.top - gap - h < 8;
    const left = Math.min(
      Math.max(8, r.left + r.width / 2 - cardWidth / 2),
      window.innerWidth - cardWidth - 8,
    );
    card.style.left = `${left}px`;
    card.style.top = `${below ? r.bottom + gap : r.top - gap - h}px`;
    card.style.transformOrigin = `${r.left + r.width / 2 - left}px ${below ? "0" : "100%"}`;
  }, [shownKey, idPrefix, cardWidth]);

  function open(key: string) {
    if (isOpen || Date.now() - closedAt.current < REOPEN_GRACE_MS) {
      setHoverKey(key);
      return;
    }
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      setOpenCount((n) => n + 1);
      setHoverKey(key);
    }, OPEN_DELAY_MS);
  }

  function close() {
    window.clearTimeout(timer.current);
    if (isOpen) closedAt.current = Date.now();
    setHoverKey(null);
    setKeyboardOpen(false);
  }

  function onPointerOver(e: PointerEvent<HTMLDivElement>) {
    // SAFETY: pointerover only bubbles from elements this grid renders, all HTMLElements.
    const target = e.target as HTMLElement;
    const key = target.dataset.key;
    if (key) {
      open(key);
    } else if (target.dataset.blank !== undefined) {
      window.clearTimeout(timer.current);
      if (hoverKey) closedAt.current = Date.now();
      setHoverKey(null);
    }
  }

  function onClick(e: MouseEvent<HTMLDivElement>) {
    // SAFETY: click only bubbles from elements this grid renders, all HTMLElements.
    const key = (e.target as HTMLElement).dataset.key;
    if (!key || !onSelect) return;
    setActiveKey(key);
    onSelect(selected === key ? null : key);
  }

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const index = dates.indexOf(activeKey);
    const moves = {
      ArrowLeft: -7,
      ArrowRight: 7,
      ArrowUp: -1,
      ArrowDown: 1,
      Home: -index,
      End: dates.length - 1 - index,
    } satisfies Record<string, number>;
    if (e.key in moves) {
      // SAFETY: `e.key in moves` above just confirmed `e.key` names one of `moves`' own keys.
      const delta = moves[e.key as keyof typeof moves];
      const next = dates[Math.min(dates.length - 1, Math.max(0, index + delta))];
      setActiveKey(next);
      setHoverKey(null);
      if (selected && onSelect) {
        // The open day's details already describe it; the arrows move them instead of a card.
        onSelect(next);
      } else {
        if (!keyboardOpen) setOpenCount((n) => n + 1);
        setKeyboardOpen(true);
      }
    } else if ((e.key === "Enter" || e.key === " ") && onSelect) {
      onSelect(selected === activeKey ? null : activeKey);
    } else if (e.key === "Escape" && (keyboardOpen || selected)) {
      if (keyboardOpen) setKeyboardOpen(false);
      else onSelect?.(null);
    } else {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
  }

  const card = shownKey ? renderCard(shownKey) : null;
  const total = Object.values(days).reduce((a, b) => a + b, 0);

  return (
    <div className="flex flex-col gap-1.5">
      <div
        className="ml-8.5 grid gap-[3px] text-small text-text-tertiary"
        style={{ gridTemplateColumns: `repeat(${weeks}, minmax(0, 1fr))` }}
        aria-hidden="true"
      >
        {visibleMonthLabels.map(({ col, span, label: month }) => (
          <span
            key={`${col}-${month}`}
            className="overflow-hidden whitespace-nowrap"
            style={{ gridColumn: `${col + 1} / span ${span}` }}
          >
            {month}
          </span>
        ))}
      </div>
      <div className="flex gap-1.5">
        <div
          className="grid w-7 shrink-0 grid-rows-7 gap-[3px] text-caption leading-none text-text-tertiary"
          aria-hidden="true"
        >
          {["Mon", "", "Wed", "", "Fri", "", ""].map((d, i) => (
            <span key={i} className="flex items-center">
              {d}
            </span>
          ))}
        </div>
        <div
          role="listbox"
          tabIndex={0}
          aria-label={`${label}, ${uses(total)}. Arrow keys move by day and week.`}
          aria-activedescendant={`${idPrefix}-${activeKey}`}
          className="group grid flex-1 grid-flow-col grid-rows-7 gap-[3px] rounded-[3px]"
          style={{ gridTemplateColumns: `repeat(${weeks}, minmax(0, 1fr))` }}
          onPointerOver={onPointerOver}
          onPointerLeave={close}
          onClick={onClick}
          onKeyDown={onKeyDown}
          onBlur={close}
        >
          <Grid
            slots={slots}
            days={days}
            idPrefix={idPrefix}
            activeKey={activeKey}
            selected={selected}
            tint={tint}
          />
        </div>
      </div>
      <div
        className="flex items-center justify-end gap-1 text-caption text-text-quaternary"
        aria-hidden="true"
      >
        <span className="mr-1">Less</span>
        {[0, 1, 2, 3, 4].map((level) => (
          <span
            key={level}
            className="size-2.5 rounded-[2px] bg-bg-tertiary"
            style={{ backgroundColor: shade(level, tint) }}
          />
        ))}
        <span className="ml-1">More</span>
      </div>
      {card &&
        createPortal(
          <div
            key={openCount}
            ref={cardRef}
            role="presentation"
            aria-hidden="true"
            className="pointer-events-none fixed top-0 left-0 z-(--z-tooltip) animate-[activityHoverCardIn_150ms_cubic-bezier(0.23,1,0.32,1)] rounded-md border border-border bg-bg-elevated px-3 py-2.5 text-small text-text-secondary shadow-md"
            style={{ width: cardWidth }}
          >
            {card}
          </div>,
          document.body,
        )}
    </div>
  );
}
