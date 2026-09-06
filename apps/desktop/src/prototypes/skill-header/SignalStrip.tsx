// ============================================================================
// PROTOTYPE. Variant 2: Signal Strip — horizontal-density axis.
// Full-width identity and description, then a compact labeled strip:
// Source, Status, Activity, Footprint. No rail and no enclosing card.
// No location, harness, scope, path, or invocation in this header.
// ============================================================================

import type { ReactNode } from "react";
import { BackButton, HeaderActionCluster, HeaderStatus } from "./HeaderControls";
import type { SkillHeaderVariantProps } from "./fixture";

function StripCell({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 border-b border-border-subtle pb-3 pr-0 min-[900px]:border-b-0 min-[900px]:border-r min-[900px]:pr-6 min-[900px]:pb-0 min-[900px]:last:border-r-0 min-[900px]:last:pr-0">
      <dt className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        {label}
      </dt>
      <dd className="m-0 min-w-0 text-small text-text-primary">{children}</dd>
    </div>
  );
}

export function SignalStrip({
  skill,
  assistantOpen,
  feedback,
  onBack,
  onToggleAssistant,
  onAction,
}: SkillHeaderVariantProps) {
  return (
    <header className="flex w-full flex-col gap-5 border-b border-border-subtle pb-6">
      <div className="flex items-center justify-between gap-4">
        <BackButton flashed={feedback === "Returned to Skills"} onBack={onBack} />
        <HeaderActionCluster
          assistantOpen={assistantOpen}
          onToggleAssistant={onToggleAssistant}
          onAction={onAction}
        />
      </div>

      <div>
        <h1 className="text-display font-semibold tracking-tight text-text-primary">
          {skill.name}
        </h1>
        <p className="mt-3 text-body leading-[1.55] text-text-secondary">{skill.description}</p>
      </div>

      <dl className="grid w-full grid-cols-2 gap-x-6 gap-y-4 pt-1 min-[900px]:grid-cols-4 min-[900px]:gap-x-0 min-[900px]:gap-y-0">
        <StripCell label="Source">
          <span className="block font-medium text-text-primary">{skill.sourceKind}</span>
          <span className="block truncate font-mono text-caption text-text-tertiary">
            {skill.source}
          </span>
        </StripCell>
        <StripCell label="Status">
          <span className="block font-medium text-text-primary">{skill.updateState}</span>
          <span className="block truncate text-caption text-text-tertiary">
            Edited {skill.edited}
          </span>
          <span className="block truncate text-caption text-text-tertiary">
            Installed {skill.installed}
          </span>
        </StripCell>
        <StripCell label="Activity">
          <span className="block font-medium text-text-primary">{skill.uses}</span>
          <span className="block text-caption text-text-tertiary">Rolling 30 days</span>
        </StripCell>
        <StripCell label="Footprint">
          <span className="block font-medium text-text-primary">{skill.size}</span>
          <span className="block text-caption text-text-tertiary">{skill.tokens}</span>
        </StripCell>
      </dl>

      <HeaderStatus message={feedback} />
    </header>
  );
}
