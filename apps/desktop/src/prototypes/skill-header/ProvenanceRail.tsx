// ============================================================================
// PROTOTYPE. Variant 1: Provenance Rail — split axis.
// Left: identity and purpose only.
// Right: Source, Freshness, Activity, Footprint as a labeled rail.
// No location, harness, scope, path, or invocation in this header.
// ============================================================================

import type { ReactNode } from "react";
import { BackButton, HeaderActionCluster, HeaderStatus } from "./HeaderControls";
import type { SkillHeaderVariantProps } from "./fixture";

function RailFact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        {label}
      </dt>
      <dd className="m-0 text-body text-text-primary">{children}</dd>
    </div>
  );
}

export function ProvenanceRail({
  skill,
  assistantOpen,
  feedback,
  onBack,
  onToggleAssistant,
  onAction,
}: SkillHeaderVariantProps) {
  return (
    <header className="flex w-full flex-col gap-4">
      <div className="flex items-center justify-between gap-4">
        <BackButton flashed={feedback === "Returned to Skills"} onBack={onBack} />
        <HeaderActionCluster
          assistantOpen={assistantOpen}
          onToggleAssistant={onToggleAssistant}
          onAction={onAction}
        />
      </div>

      <div className="grid w-full grid-cols-1 gap-8 min-[900px]:grid-cols-[minmax(0,1fr)_minmax(240px,32%)]">
        <div className="min-w-0">
          <h1 className="text-display font-semibold tracking-tight text-text-primary">
            {skill.name}
          </h1>
          <p className="mt-4 text-body leading-[1.55] text-text-secondary">{skill.description}</p>
        </div>

        <dl className="flex flex-col gap-4 border-t border-border-subtle pt-4 min-[900px]:border-t-0 min-[900px]:border-l min-[900px]:pt-0 min-[900px]:pl-6">
          <RailFact label="Source">
            <span className="block font-medium text-text-primary">{skill.sourceKind}</span>
            <span className="block font-mono text-small text-text-tertiary">{skill.source}</span>
          </RailFact>
          <RailFact label="Freshness">
            <span className="block font-medium text-text-primary">{skill.updateState}</span>
            <span className="block text-small text-text-tertiary">Edited {skill.edited}</span>
            <span className="block text-small text-text-tertiary">Installed {skill.installed}</span>
          </RailFact>
          <RailFact label="Activity">
            <span className="block text-text-primary">{skill.uses}</span>
          </RailFact>
          <RailFact label="Footprint">
            <span className="block text-text-primary">{skill.size}</span>
            <span className="block text-small text-text-tertiary">{skill.tokens}</span>
          </RailFact>
        </dl>
      </div>

      <HeaderStatus message={feedback} />
    </header>
  );
}
