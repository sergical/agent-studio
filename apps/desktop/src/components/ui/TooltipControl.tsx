// ============================================================================
// TooltipControl - Kit Tooltip wrapper: text-only content. App.tsx's
// `Tooltip.Provider` (delay=400) is the same underlying Base UI provider the
// kit's Tooltip builds on, so every tooltip in the app shares one open/close
// delay group.
// ============================================================================

import { Tooltip, TooltipContent, TooltipTrigger } from "@skill-studio/ui";

/** One line of a tooltip's body - a plain string, or a mono line (a path or a target) - see status-spec.md's fixed tooltip shape. */
export type TooltipLine = string | { text: string; mono: true };

interface TooltipControlProps {
  /** A single block of text (wraps, preserves its own line breaks), or one block per `TooltipLine`. */
  content: string | TooltipLine[];
  children: React.ReactElement;
}

/** Wraps `children` (the trigger) with a 400 ms-delayed, text-only tooltip. */
export function TooltipControl({ content, children }: TooltipControlProps) {
  // A path never wraps: a mono line sets the tooltip's width instead of the 260px cap.
  const hasMono = Array.isArray(content) && content.some((line) => "text" in Object(line));
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent
        sideOffset={6}
        className={`${hasMono ? "max-w-none" : "max-w-65"} flex-col items-start gap-0.5 bg-bg-elevated text-small text-text-primary shadow-md ring-1 ring-border [&>[data-slot=tooltip-arrow]]:hidden`}
      >
        {Array.isArray(content) ? (
          content.map((line) => {
            // `Object(line)` boxes a string primitive into a `String` wrapper, so `"text" in`
            // it never throws - it's just false for a plain line and true for a mono line.
            const isMono = "text" in Object(line);
            // SAFETY: `isMono` just confirmed `line` has a `text` property, which only the
            // mono-line member of `TooltipLine` does.
            const text = isMono ? (line as { text: string }).text : String(line);
            return (
              <span
                key={text}
                className={`block ${isMono ? "font-mono whitespace-nowrap" : "whitespace-pre-line"}`}
              >
                {text}
              </span>
            );
          })
        ) : (
          <span className="block whitespace-pre-line">{content}</span>
        )}
      </TooltipContent>
    </Tooltip>
  );
}
