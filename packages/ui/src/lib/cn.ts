// ============================================================================
// @skill-studio/ui - cn() utility
// Merges clsx's conditional class handling with tailwind-merge's conflict
// resolution, so a caller's className overrides a component's own variant
// classes instead of both landing in the DOM.
// ============================================================================

import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
