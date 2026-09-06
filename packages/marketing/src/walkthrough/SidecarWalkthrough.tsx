import { useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import * as stylex from "@stylexjs/stylex";
import { ChevronDown } from "lucide-react";
import { chapters, type WalkthroughProps } from "./walkthrough-chapters";
import { WalkthroughVideo } from "./WalkthroughVideo";
import { walkthroughStyles as s } from "./Walkthrough.stylex";
import "./walkthrough.css";

export function SidecarWalkthrough({ theme }: WalkthroughProps) {
  const [selection, setSelection] = useState({ index: 0, automatic: true, animate: false });
  const list = useRef<HTMLDivElement>(null);
  const previousPositions = useRef<number[]>([]);
  const animations = useRef<Animation[]>([]);
  const compact = useCompactWalkthrough();
  const active = selection.index;

  const selectChapter = (index: number, automatic: boolean, animate: boolean) => {
    previousPositions.current = Array.from(
      list.current?.children ?? [],
      (item) => item.getBoundingClientRect().top,
    );
    animations.current.forEach((animation) => animation.cancel());
    setSelection({ index, automatic, animate });
  };

  useLayoutEffect(() => {
    if (!selection.animate || !list.current) return;
    const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const timing = { duration: 240, easing: "cubic-bezier(0.25, 1, 0.5, 1)" };
    const items = Array.from(list.current.children);
    animations.current = [];
    if (!reducedMotion && !compact) {
      items.forEach((item, index) => {
        const previousTop = previousPositions.current[index];
        if (previousTop === undefined) return;
        const distance = previousTop - item.getBoundingClientRect().top;
        if (Math.abs(distance) < 1) return;
        animations.current.push(
          item.animate(
            [{ transform: `translateY(${distance}px)` }, { transform: "translateY(0)" }],
            timing,
          ),
        );
      });
    }
    const description = items[active]?.querySelector("p");
    if (description)
      animations.current.push(
        description.animate(
          [
            { opacity: 0, transform: reducedMotion ? "none" : "translateY(4px)" },
            { opacity: 1, transform: "none" },
          ],
          timing,
        ),
      );
    if (compact && !selection.automatic) {
      const button = document.getElementById(`sidecar-${active}`);
      if (button && button.getBoundingClientRect().top < 24)
        button.scrollIntoView({ block: "start", behavior: "instant" });
    }
    return () => animations.current.forEach((animation) => animation.cancel());
  }, [selection, active, compact]);

  const video = (
    <WalkthroughVideo
      key={`${active}-${theme}-${compact}`}
      index={active}
      theme={theme}
      compact={compact}
      animate={selection.animate}
      onEnded={() => {
        if (selection.automatic && !matchMedia("(prefers-reduced-motion: reduce)").matches)
          selectChapter((active + 1) % chapters.length, true, true);
      }}
    />
  );

  return (
    <div data-walkthrough="sidecar" data-automatic={selection.automatic}>
      <h2 {...stylex.props(s.sidecarTitle)}>Manage your installed skills.</h2>
      <div {...stylex.props(s.sidecar)}>
        <div ref={list} {...stylex.props(s.sidecarList)}>
          {chapters.map((chapter, index) => (
            <div key={chapter.id} {...stylex.props(s.sidecarItem)}>
              <button
                id={`sidecar-${index}`}
                aria-expanded={compact ? active === index : undefined}
                aria-pressed={compact ? undefined : active === index}
                aria-controls={compact ? `sidecar-panel-${index}` : "sidecar-video"}
                onClick={(event) => selectChapter(index, false, event.detail !== 0)}
                {...stylex.props(
                  s.sidecarButton,
                  active === index && s.selectedText,
                  !selection.animate && s.instant,
                )}
              >
                {chapter.title.replace(/\.$/, "")}
                <ChevronDown
                  size={14}
                  aria-hidden="true"
                  {...stylex.props(
                    s.chevron,
                    active === index && s.chevronOpen,
                    selection.animate && s.chevronMotion,
                  )}
                />
              </button>
              <div
                id={`sidecar-panel-${index}`}
                hidden={active !== index}
                {...stylex.props(active === index && s.sidecarDetail)}
              >
                {active === index && (
                  <>
                    <p {...stylex.props(s.copy)}>{chapter.copy}</p>
                    {compact && video}
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
        {!compact && (
          <div id="sidecar-video" role="region" aria-labelledby={`sidecar-${active}`}>
            {video}
          </div>
        )}
      </div>
    </div>
  );
}

const query = "(max-width: 760px)";
function subscribe(callback: () => void) {
  const media = matchMedia(query);
  media.addEventListener("change", callback);
  return () => media.removeEventListener("change", callback);
}
function useCompactWalkthrough() {
  return useSyncExternalStore(
    subscribe,
    () => matchMedia(query).matches,
    () => false,
  );
}
