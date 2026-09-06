// ============================================================================
// PROTOTYPE. Throwaway. Three variants of the installed skill header,
// switchable via ?v=1|2|3. Isolated Vite entry — not imported by production.
// ============================================================================

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { stampInitialTheme } from "../../lib/theme";
import { SKILL_HEADER_FIXTURE } from "./fixture";
import { useHeaderFeedback } from "./HeaderControls";
import { ProvenanceRail } from "./ProvenanceRail";
import { SignalStrip } from "./SignalStrip";
import { LocationsStart, PrototypeAssistant, PrototypeSidebar } from "./SkillHeaderHarness";
import { SourceLedger } from "./SourceLedger";
import "../../App.css";
import "./picker.css";

const VARIANTS = [
  { name: "Provenance Rail", Component: ProvenanceRail },
  { name: "Signal Strip", Component: SignalStrip },
  { name: "Source Ledger", Component: SourceLedger },
] as const;

function initialVariant() {
  const value = Number(new URLSearchParams(location.search).get("v"));
  return Number.isInteger(value) && value >= 1 && value <= VARIANTS.length ? value - 1 : 0;
}

function SkillHeaderPrototype() {
  const [current, setCurrent] = useState(initialVariant);
  const [assistantOpen, setAssistantOpen] = useState(false);
  const { feedback, setFeedback } = useHeaderFeedback();
  const picker = useRef<HTMLElement>(null);
  const highlight = useRef<HTMLSpanElement>(null);
  const Variant = VARIANTS[current].Component;

  const select = useCallback(
    (index: number) => {
      if (index < 0 || index >= VARIANTS.length) return;
      setCurrent(index);
      setAssistantOpen(false);
      setFeedback(null);
      const url = new URL(location.href);
      url.searchParams.set("v", String(index + 1));
      history.replaceState(null, "", url);
    },
    [setFeedback],
  );

  useLayoutEffect(() => {
    const move = () => {
      const items = picker.current?.querySelectorAll<HTMLElement>("button.proto-picker-item");
      const item = items?.[current];
      if (item && highlight.current) {
        highlight.current.style.width = `${item.offsetWidth}px`;
        highlight.current.style.transform = `translateX(${item.offsetLeft}px)`;
      }
    };
    move();
    window.addEventListener("resize", move);
    return () => window.removeEventListener("resize", move);
  }, [current]);

  useEffect(() => {
    const frame = requestAnimationFrame(() =>
      requestAnimationFrame(() => picker.current?.setAttribute("data-ready", "")),
    );
    return () => cancelAnimationFrame(frame);
  }, []);

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (
        !(event.target instanceof HTMLElement) ||
        /^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName) ||
        event.target.isContentEditable ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        event.shiftKey
      ) {
        return;
      }
      const number = Number(event.key);
      if (number >= 1 && number <= VARIANTS.length) select(number - 1);
      else if (event.key === "ArrowRight") {
        event.preventDefault();
        select((current + 1) % VARIANTS.length);
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        select((current - 1 + VARIANTS.length) % VARIANTS.length);
      }
    };
    document.addEventListener("keydown", keydown);
    return () => document.removeEventListener("keydown", keydown);
  }, [current, select]);

  return (
    <div className="flex h-screen overflow-hidden bg-bg-primary text-text-primary">
      <PrototypeSidebar />
      <main className="relative min-w-0 flex-1 overflow-y-auto">
        <div className="flex w-full flex-col gap-6 px-8 pt-7 pb-24">
          <Variant
            key={current}
            skill={SKILL_HEADER_FIXTURE}
            assistantOpen={assistantOpen}
            feedback={feedback}
            onBack={() => setFeedback("Returned to Skills")}
            onToggleAssistant={() => {
              const next = !assistantOpen;
              setAssistantOpen(next);
              setFeedback(next ? "Assistant open" : "Assistant closed");
            }}
            onAction={setFeedback}
          />
          <LocationsStart skill={SKILL_HEADER_FIXTURE} />
        </div>
      </main>
      <PrototypeAssistant open={assistantOpen} />
      <nav ref={picker} className="proto-picker" aria-label="Prototype variants">
        <span ref={highlight} className="proto-picker-highlight" aria-hidden="true" />
        {VARIANTS.map((variant, index) => (
          <button
            key={variant.name}
            type="button"
            className="proto-picker-item"
            data-active={current === index ? "" : undefined}
            aria-current={current === index ? "true" : undefined}
            onClick={() => select(index)}
          >
            {variant.name}
          </button>
        ))}
      </nav>
    </div>
  );
}

stampInitialTheme();

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("Skill header prototype root element #root is missing");
}
createRoot(rootElement).render(<SkillHeaderPrototype />);
