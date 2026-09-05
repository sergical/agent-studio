// ============================================================================
// Skill Studio - Installed skill source ledger
// Quiet lifecycle summary for an installed skill header.
// ============================================================================

import type { ReactNode } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { buildInstalledSkillSourceLedgerModel } from "./installed-skill-source-ledger-model";

interface InstalledSkillSourceLedgerProps {
  skill: InstalledSkill;
}

interface InstalledSkillSourceLedgerRowProps {
  label: string;
  secondary?: boolean;
  children: ReactNode;
}

function InstalledSkillSourceLedgerRow({
  label,
  secondary = false,
  children,
}: InstalledSkillSourceLedgerRowProps) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border-subtle pb-2 last:border-b-0 last:pb-0">
      <dt className={`text-caption ${secondary ? "text-text-quaternary" : "text-text-tertiary"}`}>
        {label}
      </dt>
      <dd
        className={`m-0 min-w-0 text-right text-small ${secondary ? "text-text-tertiary" : "text-text-primary"}`}
      >
        {children}
      </dd>
    </div>
  );
}

/** Shows source, ownership, lifecycle management, dates, update state, and secondary size facts. */
export function InstalledSkillSourceLedger({ skill }: InstalledSkillSourceLedgerProps) {
  const ledger = buildInstalledSkillSourceLedgerModel(skill);

  return (
    <dl
      className="flex min-w-0 flex-col gap-2 rounded-lg bg-bg-secondary p-4"
      aria-label="Source and lifecycle"
    >
      <InstalledSkillSourceLedgerRow label="Source">
        <span className="break-words font-mono">{ledger.source}</span>
      </InstalledSkillSourceLedgerRow>
      <InstalledSkillSourceLedgerRow label="Lifecycle owner">
        {ledger.lifecycleOwner}
      </InstalledSkillSourceLedgerRow>
      <InstalledSkillSourceLedgerRow label="Lifecycle">
        {ledger.lifecycleManagement}
      </InstalledSkillSourceLedgerRow>
      <InstalledSkillSourceLedgerRow label="Installed">
        {ledger.installed}
      </InstalledSkillSourceLedgerRow>
      {ledger.lastModified && (
        <InstalledSkillSourceLedgerRow label="Last modified">
          {ledger.lastModified}
        </InstalledSkillSourceLedgerRow>
      )}
      <InstalledSkillSourceLedgerRow label="Update state">
        {ledger.updateState}
      </InstalledSkillSourceLedgerRow>
      {ledger.size && (
        <InstalledSkillSourceLedgerRow label="Size" secondary>
          {ledger.size}
        </InstalledSkillSourceLedgerRow>
      )}
      {ledger.tokens && (
        <InstalledSkillSourceLedgerRow label="Tokens" secondary>
          {ledger.tokens}
        </InstalledSkillSourceLedgerRow>
      )}
    </dl>
  );
}
