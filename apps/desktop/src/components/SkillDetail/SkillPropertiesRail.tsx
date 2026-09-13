// ============================================================================
// SkillPropertiesRail - The skill page's right-hand properties: Location,
// Harnesses, Invocation, Source, Lifecycle, Tokens, Installed/Modified. Every
// derived fact reuses the Locations card's own helpers (`buildScopeGroups`,
// `buildInvocationFiles`, the source ledger model) so the rail and the card
// never disagree about the same skill.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { AlertTriangle } from "lucide-react";
import {
  Button,
  Popover,
  PopoverContent,
  PopoverTrigger,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@skill-studio/ui";
import { formatTokens } from "@skill-studio/lib";
import type { AgentId, InstalledSkill, InvocationPolicy } from "@skill-studio/lib";
import { setDeploymentEnabled, setHarnessEnabled } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { HarnessStack } from "../SkillList/HarnessStack";
import { DEFAULT_HARNESS_LIST, whereFacts } from "../SkillList/skill-row-state";
import { SwitchControl } from "../ui/SwitchControl";
import { buildInstalledSkillSourceLedgerModel } from "./installed-skill-source-ledger-model";
import { setInvocationForFile } from "./skill-location-actions";
import { canToggleHarness } from "./skill-location-helpers";
import {
  buildInvocationFiles,
  buildScopeGroups,
  INVOCATION_POLICY_OPTIONS,
  scopeGroupsHaveDrift,
} from "./skill-location-status";
import type { AgentLocationRow } from "./skill-location-status";
import type { SkillPageAction } from "./skill-page-actions";

interface SkillPropertiesRailProps {
  skill: InstalledSkill;
  /** The header's "Update"/"Pull latest" action, reused for Source's own Update button - `null` when nothing is available. */
  updateAction: SkillPageAction | null;
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

function invocationLabel(policy: InvocationPolicy): string {
  return INVOCATION_POLICY_OPTIONS.find((option) => option.value === policy)?.label ?? "Mixed";
}

/** Scrolls `SkillLocationsCard` into view and focuses its heading - the Location row's "show locations" affordance. */
function showLocations() {
  const heading = document.getElementById("skill-locations-heading");
  heading?.scrollIntoView({ block: "start" });
  heading?.focus();
}

export function SkillPropertiesRail({ skill, updateAction }: SkillPropertiesRailProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [announcement, setAnnouncement] = useState<{
    kind: "status" | "alert";
    text: string;
  } | null>(null);
  const [pendingHarness, setPendingHarness] = useState<AgentId | null>(null);
  const [isSavingInvocation, setIsSavingInvocation] = useState(false);

  const groups = buildScopeGroups(skill);
  const files = buildInvocationFiles(groups, skill);
  const ledger = buildInstalledSkillSourceLedgerModel(skill);
  const hasDrift = scopeGroupsHaveDrift(groups);

  const reach = whereFacts(skill, DEFAULT_HARNESS_LIST);
  const reachedHarnesses = reach.harnesses.filter((h) => h.reached);
  const harnessCount = reachedHarnesses.length + (reach.universal.present ? 1 : 0);
  const allRows: AgentLocationRow[] = groups.flatMap((group) =>
    group.rows.filter((row): row is AgentLocationRow => row.kind !== "shared"),
  );
  const rowForHarness = (harness: AgentId): AgentLocationRow | null =>
    allRows.find((row) => row.harness === harness && row.hasSwitch) ??
    allRows.find((row) => row.harness === harness) ??
    null;

  const announceError = (title: string, message: string) => {
    setAnnouncement({ kind: "alert", text: message });
    addToast({ type: "error", title, message });
  };

  const toggleHarness = async (harness: AgentId, row: AgentLocationRow, enabled: boolean) => {
    setPendingHarness(harness);
    try {
      if (row.kind === "reader") {
        await setHarnessEnabled(row.lifecycleTarget, harness, enabled);
      } else if (row.deployment) {
        await (row.deployment.disabled_by === "studio-moved" || !canToggleHarness(row.deployment)
          ? setDeploymentEnabled({ deployment_id: row.deployment.id }, enabled)
          : setHarnessEnabled({ deployment_id: row.deployment.id }, harness, enabled));
      }
      setAnnouncement({ kind: "status", text: "Saved" });
    } catch (err) {
      announceError(
        enabled ? "Couldn't enable" : "Couldn't disable",
        err instanceof Error ? err.message : "Unknown error",
      );
    } finally {
      setPendingHarness(null);
    }
  };

  const invocationPolicies = new Set(files.map((file) => file.invocation));
  const hasEditableFile = files.some((file) => file.editable);

  const handleSetInvocationAll = async (policy: InvocationPolicy) => {
    setIsSavingInvocation(true);
    try {
      for (const file of files) {
        if (file.editable) await setInvocationForFile(skill, file, policy);
      }
      setAnnouncement({ kind: "status", text: "Saved" });
    } catch (err) {
      announceError(
        "Couldn't change invocation policy",
        err instanceof Error ? err.message : "Unknown error",
      );
    } finally {
      setIsSavingInvocation(false);
    }
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
              {reachedHarnesses.length === 0 ? (
                <p className="m-0 text-small text-text-tertiary">
                  {reach.universal.present
                    ? "Only in the shared Universal folder."
                    : "No harness reaches this skill."}
                </p>
              ) : (
                reachedHarnesses.map((h) => {
                  const row = rowForHarness(h.harness);
                  return (
                    <div key={h.harness} className="flex h-7 items-center justify-between gap-2">
                      <span className="truncate text-small text-text-secondary">{h.label}</span>
                      <SwitchControl
                        checked={row?.switchOn ?? true}
                        disabled={!row?.hasSwitch || pendingHarness === h.harness}
                        onCheckedChange={(next) => row && toggleHarness(h.harness, row, next)}
                        ariaLabel={`Enabled for ${h.label}`}
                      />
                    </div>
                  );
                })
              )}
            </PopoverContent>
          </Popover>
        </PropertyRow>

        <PropertyRow label="Invocation">
          {files.length === 0 ? (
            <span className="text-text-tertiary">—</span>
          ) : invocationPolicies.size <= 1 ? (
            <Select
              items={INVOCATION_POLICY_OPTIONS}
              value={files[0].invocation}
              disabled={isSavingInvocation || !hasEditableFile}
              onValueChange={(next) => {
                if (!next) return;
                // SAFETY: `items` is INVOCATION_POLICY_OPTIONS, so every value Select can emit is an InvocationPolicy.
                handleSetInvocationAll(next as InvocationPolicy);
              }}
            >
              <SelectTrigger
                aria-label={`Invocation: ${invocationLabel(files[0].invocation)}, edit`}
                className="h-7 w-full min-w-0 justify-between gap-1.5 rounded-sm border-none bg-transparent px-1.5 -mx-1.5 text-small text-text-primary hover:bg-bg-hover dark:bg-transparent dark:hover:bg-bg-hover"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent
                alignItemWithTrigger={false}
                className="w-auto min-w-(--anchor-width) gap-px rounded-md border border-border bg-bg-secondary p-1 shadow-md"
              >
                {INVOCATION_POLICY_OPTIONS.map((option) => (
                  <SelectItem
                    key={option.value}
                    value={option.value}
                    className="h-7 rounded-sm px-2.5 text-small text-text-secondary data-highlighted:bg-bg-hover data-highlighted:text-text-primary"
                  >
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <span className="flex items-center gap-1.5">
              <span>Mixed</span>
              <Button
                variant="link"
                className="h-auto p-0 text-small font-normal"
                onClick={showLocations}
              >
                Show locations
              </Button>
            </span>
          )}
        </PropertyRow>

        <PropertyRow label="Source">
          <div className="flex flex-col gap-0.5 py-1">
            <span className="truncate font-mono text-text-primary">{ledger.source}</span>
            <span className="text-caption text-text-tertiary">{ledger.lifecycleOwner}</span>
            {updateAction && (
              <Button
                variant="ghost"
                className={`${EDIT_BUTTON_CLASS} w-fit text-accent`}
                onClick={updateAction.run}
                disabled={updateAction.busy}
              >
                {updateAction.busy ? "Working…" : updateAction.label}
              </Button>
            )}
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
