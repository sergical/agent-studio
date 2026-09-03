// ============================================================================
// @skill-studio/ui - Progress
// Vendored from shadcn's Base UI registry (`shadcn add progress -b base`).
// ============================================================================

import { Progress as ProgressPrimitive } from "@base-ui/react/progress";

import { cn } from "../lib/cn";

function Progress({ className, value, ...props }: ProgressPrimitive.Root.Props) {
  return (
    <ProgressPrimitive.Root
      data-slot="progress"
      value={value}
      className={cn("relative w-full", className)}
      {...props}
    >
      <ProgressPrimitive.Track
        data-slot="progress-track"
        className="relative h-2 w-full overflow-hidden rounded-full bg-bg-tertiary"
      >
        <ProgressPrimitive.Indicator
          data-slot="progress-indicator"
          className="h-full w-full flex-1 bg-primary transition-transform duration-200 ease-out"
          style={{ transform: `translateX(-${100 - (value ?? 100)}%)` }}
        />
      </ProgressPrimitive.Track>
    </ProgressPrimitive.Root>
  );
}

export { Progress };
