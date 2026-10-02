// ============================================================================
// SkillPropertiesRail - The skill page's right-hand properties: Location,
// Harnesses, Source, Lifecycle, Tokens, Installed/Modified. Invocation lives
// on the Locations card only, next to the files it sets. Every derived fact
// reuses the Locations card's own helpers (`buildScopeGroups`, the source
// ledger model) so the rail and the card never disagree about the same skill.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { AlertTriangle } from "lucide-react";
import { Button, Popover, PopoverContent, PopoverTrigger } from "@skill-studio/ui";
import { formatTokens } from "@skill-studio/lib";
import type { AgentId, InstalledSkill } from "@skill-studio/lib";
import { restoreMovedDeployment, setHarnessEnabled } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { HarnessStack } from "../SkillList/HarnessStack";
import { DEFAULT_HARNESS_LIST, whereFacts } from "../SkillList/skill-row-state";
import { SwitchControl } from "../ui/SwitchControl";
import { buildInstalledSkillSourceLedgerModel } from "./installed-skill-source-ledger-model";
import { canOfferHarnessSwitchForRow, harnessSwitchOffTitle } from "./skill-location-helpers";
import { buildScopeGroups, scopeGroupsHaveDrift } from "./skill-location-status";
import type { AgentLocationRow } from "./skill-location-status";
import { railHarnessEntries, readerToggleAction } from "./skill-properties-rail-model";

interface SkillPropertiesRailProps {
  skill: InstalledSkill;
}

/** One property row: a 96px label column and a value column, min height 28px. */
function PropertyRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid min-h-7 grid-cols-[96px_minmax(0,1fr)] items-start gap-1">
      <dt className="pt-1 text-small text-text-tertiary">{label}</dt>
      <dd className="m-0 min-w-0 select-text text-small text-text-primary">{children}</dd>
    </div>
  );
}

/** A row's editable value: a full-width, left-aligned ghost button matching the read-only text's baseline. */
const EDIT_BUTTON_CLASS =
  "h-7 w-full min-w-0 justify-start gap-1.5 rounded-sm px-1.5 -mx-1.5 text-small font-normal text-text-primary";

/** "Global", "Project · foo", or "Global + 2 projects" - `groups` is already sorted Global-first by `buildScopeGroups`. */
function locationValue(groups: ReturnType<typeof buildScopeGroups>): string {
  if (groups.length === 0) return "—";
  if (groups.length === 1) return groups[0].label;
  const extra = groups.length - 1;
  return `${groups[0].label} + ${extra} project${extra === 1 ? "" : "s"}`;
}

/** Scrolls `SkillLocationsCard` into view and focuses its heading - the Location row's "show locations" affordance. */
function showLocations() {
  const heading = document.getElementById("skill-locations-heading");
  heading?.scrollIntoView({ block: "start" });
  heading?.focus();
}

export function SkillPropertiesRail({ skill }: SkillPropertiesRailProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [announcement, setAnnouncement] = useState<{
    kind: "status" | "alert";
    text: string;
  } | null>(null);
  const [pendingHarness, setPendingHarness] = useState<AgentId | null>(null);

  const groups = buildScopeGroups(skill);
  const ledger = buildInstalledSkillSourceLedgerModel(skill);
  const hasDrift = scopeGroupsHaveDrift(groups);

  const reach = whereFacts(skill, DEFAULT_HARNESS_LIST);
  const harnessCount =
    reach.harnesses.filter((h) => h.reached).length + (reach.universal.present ? 1 : 0);
  const harnessEntries = railHarnessEntries(skill, groups);

  const announceError = (title: string, message: string) => {
    setAnnouncement({ kind: "alert", text: message });
    addToast({ type: "error", title, message });
  };

  const toggleHarness = async (harness: AgentId, row: AgentLocationRow, enabled: boolean) => {
    setPendingHarness(harness);
    try {
      const readerAction = readerToggleAction(row, enabled);
      if (readerAction) {
        await setHarnessEnabled(readerAction.target, readerAction.agent, readerAction.enabled);
      } else if (row.deployment) {
        if (row.deployment.disabled_by === "studio-moved") {
          await restoreMovedDeployment({ deployment_id: row.deployment.id });
        } else {
          await setHarnessEnabled({ deployment_id: row.deployment.id }, harness, enabled);
        }
      }
      setAnnouncement({ kind: "status", text: "Saved" });
    } catch (err) {
      announceError(
        enabled ? "Couldn't enable" : "Couldn't disable",
        err instanceof Error ? err.message : "Unknown error",
      );
    }
    setPendingHarness(null);
  };

  return (
    <aside aria-label="Properties" className="sticky top-5 flex min-w-0 flex-col gap-1 self-start">
      <div
        role={announcement?.kind === "alert" ? "alert" : "status"}
        aria-live={announcement?.kind === "alert" ? "assertive" : "polite"}
        className="sr-only"
      >
        {announcement?.text}
      </div>

      <dl className="flex flex-col gap-1">
        <PropertyRow label="Location">
          <Button
            variant="ghost"
            className={EDIT_BUTTON_CLASS}
            aria-label={`Location: ${locationValue(groups)}, show locations`}
            onClick={showLocations}
          >
            {hasDrift && (
              <AlertTriangle size={12} className="shrink-0 text-warning" aria-hidden="true" />
            )}
            <span className="truncate">{locationValue(groups)}</span>
          </Button>
        </PropertyRow>

        <PropertyRow label="Harnesses">
          <Popover>
            <PopoverTrigger
              className={`${EDIT_BUTTON_CLASS} inline-flex cursor-pointer items-center border-0 bg-transparent hover:bg-bg-hover`}
              aria-label={`Harnesses: ${harnessCount} harnesses, edit`}
            >
              <HarnessStack skill={skill} harnessList={DEFAULT_HARNESS_LIST} />
              <span className="tabular-nums text-text-tertiary">{harnessCount}</span>
            </PopoverTrigger>
            <PopoverContent align="start" aria-label="Harnesses" className="w-64 gap-1.5">
              {harnessEntries.length === 0 ? (
                <p className="m-0 text-small text-text-tertiary">
                  {reach.universal.present
                    ? "Only in the shared Universal folder."
                    : "No harness reaches this skill."}
                </p>
              ) : (
                harnessEntries.map((h) => {
                  const row = h.row;
                  const offerSwitch = row != null && canOfferHarnessSwitchForRow(row);
                  return (
                    <div key={h.harness} className="flex h-7 items-center justify-between gap-2">
                      <span className="truncate text-small text-text-secondary">{h.label}</span>
                      <SwitchControl
                        checked={row?.switchOn ?? false}
                        disabled={!offerSwitch || pendingHarness === h.harness}
                        onCheckedChange={(next) => row && toggleHarness(h.harness, row, next)}
                        ariaLabel={`Enabled for ${h.label}`}
                        title={offerSwitch || !row ? undefined : harnessSwitchOffTitle(row)}
                      />
                    </div>
                  );
                })
              )}
            </PopoverContent>
          </Popover>
        </PropertyRow>

        <PropertyRow label="Source">
          <div className="flex flex-col gap-0.5 py-1">
            <span className="truncate font-mono text-text-primary">{ledger.source}</span>
            <span className="text-caption text-text-tertiary">{ledger.lifecycleOwner}</span>
          </div>
        </PropertyRow>

        <PropertyRow label="Lifecycle">
          <span className="py-1">
            {ledger.lifecycleOwner} · {ledger.lifecycleManagement}
          </span>
        </PropertyRow>

        <PropertyRow label="Tokens">
          <span className="py-1 tabular-nums">
            Prompt {formatTokens(skill.description_tokens)} · Full{" "}
            {formatTokens(skill.skill_md_tokens)}
          </span>
        </PropertyRow>

        <PropertyRow label="Installed">
          <span className="py-1">
            {ledger.installed}
            {ledger.lastModified && (
              <span className="text-text-tertiary"> · Modified {ledger.lastModified}</span>
            )}
          </span>
        </PropertyRow>
      </dl>
    </aside>
  );
}
