// ============================================================================
// @skill-studio/ui - Kbd
// One keyboard glyph or key name, for shortcut hints in tooltips, menus, and
// the command palette. Callers pass the macOS glyphs (⌘ ⇧ ⌥ ↵ ↑ ↓) or a
// plain letter, one `Kbd` per key.
// ============================================================================

import type { ComponentProps } from "react";
import { cn } from "../lib/cn";

export function Kbd({ className, ...props }: ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        "inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-xs border border-border-subtle px-1 font-sans text-caption text-text-tertiary",
        className,
      )}
      {...props}
    />
  );
}
