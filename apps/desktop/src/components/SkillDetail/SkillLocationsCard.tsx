// ============================================================================
// SkillLocationsCard - "Where it lives": one scope block per Global/project,
// each with the shared-folder accordion (if any) plus every harness's own
// entry as a flat sibling row, an uppercase scope eyebrow when the skill
// lives in a project, and an Invocation footer - one segmented Both/User
// only/Model only control per file. Row rendering lives in
// SkillLocationScope/SkillLocationRow so this file stays under
// react-doctor's line cap.
// ============================================================================

import { useState } from "react";
import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import { forkSkill } from "../../lib/skill-api";
import { lifecycleTargetForDeployment } from "../../lib/skill-lifecycle-target";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon } from "../ui/HarnessIcon";
import { MaterializeRootDialog } from "../ui/MaterializeRootDialog";
import { StatusIcon } from "../ui/StatusIcon";
import { TooltipControl } from "../ui/TooltipControl";
import { RemoveDeploymentsDialog } from "./RemoveDeploymentsDialog";
import { SkillLocationScope } from "./SkillLocationScope";
import { useLocationActions, setSkillInvocation } from "./skill-location-actions";
import {
  buildInvocationFiles,
  buildScopeGroups,
  invocationFooterNote,
  promoteToGlobal,
  titleLink,
  toTooltipLines,
} from "./skill-location-status";
import type { InvocationFile } from "./skill-location-status";
import type { InstalledSkill, InvocationPolicy } from "@skill-studio/lib";

interface SkillLocationsCardProps {
  skill: InstalledSkill;
  /** Opens `SkillCompareDialog` - shown as a "Compare copies" title link only when a copy has drifted. */
  onCompareCopies?: () => void;
}

const INVOCATION_POLICY_OPTIONS: { value: InvocationPolicy; label: string }[] = [
  { value: "both", label: "Both" },
  { value: "user-only", label: "User only" },
  { value: "model-only", label: "Model only" },
];

interface LocationActionForTitle {
  kind: "unpark" | "compare" | "install-again" | "update";
}

const TITLE_LINK_ACTIONS = {
  Unpark: { kind: "unpark" },
  "Compare copies": { kind: "compare" },
  "Install again": { kind: "install-again" },
  "Enable everywhere": { kind: "unpark" },
  Update: { kind: "update" },
} satisfies Record<NonNullable<ReturnType<typeof titleLink>>, LocationActionForTitle>;

/**
 * "Where it lives": Global first, then one block per project, each folded to
 * its own rollup dot until opened, plus an Invocation footer - one file, one
 * segmented control. A dotagents/skills.sh-managed skill's shared file forks
 * first, same rule as the SKILL.md editor, so an invocation change sticks.
 */
export function SkillLocationsCard({ skill, onCompareCopies }: SkillLocationsCardProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [savingFile, setSavingFile] = useState<string | null>(null);
  const actions = useLocationActions(skill, onCompareCopies);

  const groups = buildScopeGroups(skill);
  const files = buildInvocationFiles(groups, skill);
  const hasDrift = groups.some((g) =>
    g.rows.some((r) => r.conditions.some((c) => c.status === "Differs")),
  );
  const link = titleLink(skill, hasDrift);
  const promote = link ? null : promoteToGlobal(groups);
  const showEyebrows = groups.some((g) => !g.isGlobal);

  const handleSetInvocation = (file: InvocationFile, policy: InvocationPolicy) => {
    if (!file.editable || savingFile) return;
    setSavingFile(file.path);
    // Only the global Universal folder can need a fork before editing.
    // `fileEditability` prevents managed Project folders and copies from
    // reaching this branch.
    const isManaged = skill.source_kind === "dotagents" || skill.source_kind === "skills-sh";
    const forkIfNeeded =
      isManaged && file.kind === "shared"
        ? forkSkill(lifecycleTargetForDeployment(file.deployment))
        : Promise.resolve();
    forkIfNeeded
      .then(() => setSkillInvocation(skill.name, `${file.path}/SKILL.md`, policy))
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't change invocation policy",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => setSavingFile(null));
  };

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div className="flex items-baseline justify-between gap-3 text-body font-semibold text-text-primary">
        Locations
        {link && (
          <button
            type="button"
            className="cursor-pointer border-0 bg-transparent p-0 text-small font-normal text-accent transition-colors hover:underline"
            onClick={() => actions.run(TITLE_LINK_ACTIONS[link])}
          >
            {link}
          </button>
        )}
        {promote && (
          <TooltipControl content="Copies it to ~/.agents/skills, where every project reads it.">
            <button
              type="button"
              className="cursor-pointer border-0 bg-transparent p-0 text-small font-normal text-accent transition-colors hover:underline"
              onClick={() =>
                actions.run({
                  kind: "promote-global",
                  source: promote.path,
                  agents: promote.agents,
                })
              }
            >
              Promote to global
            </button>
          </TooltipControl>
        )}
      </div>

      {skill.deployments.length === 0 ? (
        <p className="m-0 px-3 py-6 text-small text-text-tertiary">
          Known only from the lock file — no folder on disk.
        </p>
      ) : (
        <div className="-mx-2 flex flex-col">
          {groups.map((group) => (
            <div key={group.label} className="flex flex-col not-first:mt-3">
              {showEyebrows && (
                <span className="px-2 pb-1.5 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
                  {group.label}
                </span>
              )}
              <SkillLocationScope group={group} showEyebrow={showEyebrows} onAction={actions.run} />
            </div>
          ))}
        </div>
      )}

      {files.length > 0 && (
        <div className="mt-3 flex flex-col gap-1.5 border-t border-border-subtle pt-3">
          <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
            Invocation
          </span>
          {files.map((file) => (
            <div
              key={file.path}
              className="grid h-8 grid-cols-[16px_minmax(0,1fr)_auto_auto] items-center gap-2.5"
            >
              <StatusIcon
                icon={<HarnessIcon harness={file.harness} size={16} />}
                level={file.level ?? undefined}
                tip={toTooltipLines(file.tip)}
              />
              <span className="flex min-w-0 flex-col">
                <TooltipControl content={[{ text: `${file.path}/SKILL.md`, mono: true }]}>
                  <span className="w-fit max-w-full truncate text-body text-text-primary">
                    {file.name}
                  </span>
                </TooltipControl>
                {file.caption && (
                  <span className="truncate text-caption text-text-tertiary">{file.caption}</span>
                )}
              </span>
              {file.chip && (
                <span className="rounded-full bg-bg-tertiary px-1.5 py-0.5 text-caption text-text-tertiary">
                  {file.chip}
                </span>
              )}
              {(() => {
                const group = (
                  <ToggleGroup
                    variant="segmented"
                    aria-label={`Invocation for ${file.name}`}
                    value={[file.invocation]}
                    onValueChange={(next) =>
                      singleSelectToggleValue<InvocationPolicy>(next, (selected) =>
                        handleSetInvocation(file, selected),
                      )
                    }
                  >
                    {INVOCATION_POLICY_OPTIONS.map((option) => (
                      <ToggleGroupItem
                        key={option.value}
                        value={option.value}
                        className="h-[26px] px-3 text-small"
                        disabled={
                          (!file.editable || savingFile === file.path) &&
                          file.invocation !== option.value
                        }
                      >
                        {option.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                );
                return file.editable ? (
                  group
                ) : (
                  <TooltipControl content={file.disabledReason ?? ""}>{group}</TooltipControl>
                );
              })()}
            </div>
          ))}
          <p className="text-small text-text-tertiary">{invocationFooterNote(files, skill.name)}</p>
        </div>
      )}

      {actions.materializeRequest && (
        <MaterializeRootDialog
          target={actions.materializeRequest.target}
          harness={actions.materializeRequest.harness}
          harnessLabel={actions.materializeRequest.harnessLabel}
          root={actions.materializeRequest.root}
          disableSkill={skill.name}
          onClose={actions.closeMaterializeRequest}
        />
      )}
      {actions.removeRequest && (
        <RemoveDeploymentsDialog
          skill={skill}
          scopeLabel={actions.removeRequest.scopeLabel}
          projectPath={actions.removeRequest.projectPath}
          deployment={actions.removeRequest.deployment}
          onClose={actions.closeRemoveRequest}
        />
      )}
    </div>
  );
}
