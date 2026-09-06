// ============================================================================
// PROTOTYPE. Variant 3: Source Ledger — lifecycle axis.
// Identity and purpose on the left. Quiet right panel: repository, manager,
// installed date, last local edit, update state; usage and size secondary.
// No location, harness, scope, path, or invocation in this header.
// ============================================================================

import type { ReactNode } from "react";
import { BackButton, HeaderActionCluster, HeaderStatus } from "./HeaderControls";
import type { SkillHeaderVariantProps } from "./fixture";

function LedgerRow({
  label,
  secondary,
  children,
}: {
  label: string;
  secondary?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border-subtle pb-2 last:border-b-0 last:pb-0">
      <dt className={`text-caption ${secondary ? "text-text-quaternary" : "text-text-tertiary"}`}>
        {label}
      </dt>
      <dd
        className={`m-0 text-right text-small ${secondary ? "text-text-tertiary" : "text-text-primary"}`}
      >
        {children}
      </dd>
    </div>
  );
}

export function SourceLedger({
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

      <div className="grid w-full grid-cols-1 gap-8 min-[900px]:grid-cols-[minmax(0,1.35fr)_minmax(260px,1fr)]">
        <div className="min-w-0">
          <h1 className="text-display font-semibold tracking-tight text-text-primary">
            {skill.name}
          </h1>
          <p className="mt-4 text-body leading-[1.55] text-text-secondary">{skill.description}</p>
        </div>

        <dl className="flex flex-col gap-2 rounded-lg bg-bg-secondary p-4">
          <LedgerRow label="Repository">
            <span className="font-mono">{skill.source}</span>
          </LedgerRow>
          <LedgerRow label="Manager">{skill.sourceKind}</LedgerRow>
          <LedgerRow label="Installed">{skill.installed}</LedgerRow>
          <LedgerRow label="Last local edit">{skill.edited}</LedgerRow>
          <LedgerRow label="Update state">{skill.updateState}</LedgerRow>
          <LedgerRow label="Usage" secondary>
            {skill.uses}
          </LedgerRow>
          <LedgerRow label="Size" secondary>
            {skill.size}
            <span className="mt-0.5 block text-caption text-text-quaternary">{skill.tokens}</span>
          </LedgerRow>
        </dl>
      </div>

      <HeaderStatus message={feedback} />
    </header>
  );
}
