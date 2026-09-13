// ============================================================================
// RichTooltip - a structured tooltip with icons, a grid of columns, or mono
// paths, built straight on the kit's Tooltip primitives with the same
// sideOffset and shell TooltipControl uses. `RichTooltipScope` shares one
// Tooltip root across a whole list of rows: each `RichTooltip` inside it
// renders only a detached trigger, so a 378-row list mounts one tooltip
// instance instead of hundreds.
// ============================================================================

import { createContext, useContext, useMemo } from "react";
import type { ReactElement, ReactNode } from "react";
import { createTooltipHandle, Tooltip, TooltipContent, TooltipTrigger } from "@skill-studio/ui";
import type { TooltipHandle } from "@skill-studio/ui";

interface RichTooltipProps {
  content: ReactNode;
  children: ReactElement;
}

const CONTENT_CLASS =
  "max-w-none flex-col items-start gap-0.5 bg-bg-elevated text-small text-text-primary shadow-md ring-1 ring-border [&>[data-slot=tooltip-arrow]]:hidden";

/** The handle a `RichTooltipScope` shares with every `RichTooltip` inside it. `null` outside a
 * scope, so `RichTooltip` falls back to mounting its own root. */
const RichTooltipHandleContext = createContext<TooltipHandle<RichTooltipPayload> | null>(null);

/** A trigger's content, resolved when the tooltip opens - it returns `null` to show nothing, e.g.
 * a chip whose name only needs a tooltip once it has actually clipped. */
export type RichTooltipPayload = () => ReactNode;

/** The active scope's handle, for a caller (e.g. `SkillLocationCell`'s clipped-only project chip)
 * that needs to build its own detached trigger instead of going through `RichTooltip` directly.
 * `null` outside a scope. */
export function useRichTooltipHandle(): TooltipHandle<RichTooltipPayload> | null {
  return useContext(RichTooltipHandleContext);
}

/** Wraps a list of rows in one shared Tooltip root, keyed by a handle - the same 400ms provider
 * delay and instant switching between adjacent triggers, but one root instead of one per row. */
export function RichTooltipScope({ children }: { children: ReactNode }) {
  // Created once, not stored in state: the handle never changes, so there's no setter to pair it
  // with.
  const handle = useMemo(() => createTooltipHandle<RichTooltipPayload>(), []);
  return (
    <RichTooltipHandleContext.Provider value={handle}>
      {children}
      <Tooltip handle={handle}>
        {({ payload }) => {
          const content = payload?.();
          if (content === null || content === undefined) return null;
          return (
            <TooltipContent sideOffset={6} className={CONTENT_CLASS}>
              {content}
            </TooltipContent>
          );
        }}
      </Tooltip>
    </RichTooltipHandleContext.Provider>
  );
}

export function RichTooltip({ content, children }: RichTooltipProps) {
  const handle = useContext(RichTooltipHandleContext);
  if (handle) {
    return <TooltipTrigger handle={handle} payload={() => content} render={children} />;
  }
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent sideOffset={6} className={CONTENT_CLASS}>
        {content}
      </TooltipContent>
    </Tooltip>
  );
}
