// ============================================================================
// @skill-studio/ui - cn() utility
// Merges clsx's conditional class handling with tailwind-merge's conflict
// resolution, so a caller's className overrides a component's own variant
// classes instead of both landing in the DOM.
// ============================================================================

import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The app names its font sizes (--text-caption … --text-display). Without registering them,
// tailwind-merge keeps a base class such as `text-sm` next to a caller's `text-body` and the
// base wins, so the token never applies.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [
        {
          text: [
            "caption",
            "small",
            "body",
            "emphasis",
            "heading",
            "heading-lg",
            "title",
            "display",
          ],
        },
      ],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
