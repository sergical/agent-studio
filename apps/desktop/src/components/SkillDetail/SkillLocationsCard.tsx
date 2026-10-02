// ============================================================================
// SkillLocationsCard - "Where it lives": one scope block per Global/project,
// each with the shared-folder accordion (if any) plus every harness's own
// entry as a flat sibling row, an uppercase scope eyebrow when the skill
// lives in a project, and the Invocation footer (SkillInvocationFooter). Row
// rendering lives in SkillLocationScope/SkillLocationRow so this file stays
// under react-doctor's line cap.
// ============================================================================

import { Button } from "@skill-studio/ui";
import { MaterializeRootDialog } from "../ui/MaterializeRootDialog";
import { MakeIndependentCopyDialog } from "./MakeIndependentCopyDialog";
import { TooltipControl } from "../ui/TooltipControl";
import { RemoveDeploymentsDialog } from "./RemoveDeploymentsDialog";
import { SkillInvocationFooter } from "./SkillInvocationFooter";
import { SkillLocationScope } from "./SkillLocationScope";
import { SplitSkillDialog } from "./SplitSkillDialog";
import { UninstallPluginDialog } from "./UninstallPluginDialog";
import { useLocationActions } from "./skill-location-actions";
import {
  buildInvocationFiles,
  buildScopeGroups,
  promoteToGlobal,
  scopeGroupsHaveDrift,
  titleLink,
} from "./skill-location-status";
import type { InstalledSkill } from "@skill-studio/lib";

interface SkillLocationsCardProps {
  skill: InstalledSkill;
  /** Opens `SkillCompareDialog` - shown as a "Compare copies" title link only when a copy has drifted. */
  onCompareCopies?: () => void;
}

interface LocationActionForTitle {
  kind: "unpark" | "compare" | "install-again";
}

const TITLE_LINK_ACTIONS = {
  Unpark: { kind: "unpark" },
  "Compare copies": { kind: "compare" },
  "Install again": { kind: "install-again" },
  "Enable everywhere": { kind: "unpark" },
} satisfies Record<NonNullable<ReturnType<typeof titleLink>>, LocationActionForTitle>;

/**
 * "Where it lives": Global first, then one block per project, each folded to
 * its own rollup dot until opened, plus the Invocation footer. A dotagents/skills.sh-managed
 * skill's shared file forks first, same rule as the SKILL.md editor, so an invocation change sticks.
 */
export function SkillLocationsCard({ skill, onCompareCopies }: SkillLocationsCardProps) {
  const actions = useLocationActions(skill, onCompareCopies);

  const groups = buildScopeGroups(skill);
  const files = buildInvocationFiles(groups);
  const hasDrift = scopeGroupsHaveDrift(groups);
  const link = titleLink(skill, hasDrift);
  const promote = link ? null : promoteToGlobal(groups);
  const showEyebrows = groups.some((g) => !g.isGlobal);

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div
        id="skill-locations-heading"
        tabIndex={-1}
        className="flex items-baseline justify-between gap-3 text-body font-semibold text-text-primary outline-none"
      >
        Locations
        {link && (
          <Button
            variant="link"
            className="h-auto p-0 text-small font-normal"
            onClick={() => actions.run(TITLE_LINK_ACTIONS[link])}
          >
            {link}
          </Button>
        )}
        {promote && (
          <TooltipControl content="Copies it to ~/.agents/skills, where every project reads it.">
            <Button
              variant="link"
              className="h-auto p-0 text-small font-normal"
              onClick={() =>
                actions.run({
                  kind: "promote-global",
                  source: promote.path,
                  agents: promote.agents,
                })
              }
            >
              Promote to global
            </Button>
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

      {files.length > 0 && <SkillInvocationFooter skill={skill} files={files} />}

      {actions.materializeRequest && (
        <MaterializeRootDialog
          target={actions.materializeRequest.target}
          harness={actions.materializeRequest.harness}
          harnessLabel={actions.materializeRequest.harnessLabel}
          root={actions.materializeRequest.root}
          intent={{ kind: "convert-only" }}
          onClose={actions.closeMaterializeRequest}
        />
      )}
      {actions.independentCopyRequest && (
        <MakeIndependentCopyDialog
          skillName={skill.name}
          deployment={actions.independentCopyRequest.deployment}
          scopeLabel={actions.independentCopyRequest.scopeLabel}
          onClose={actions.closeIndependentCopyRequest}
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
      {actions.splitRequest && (
        <SplitSkillDialog
          skillName={skill.name}
          target={actions.splitRequest.target}
          projectPath={actions.splitRequest.projectPath}
          readers={actions.splitRequest.readers}
          onClose={actions.closeSplitRequest}
        />
      )}
      {actions.pluginUninstallRequest && (
        <UninstallPluginDialog
          deployment={actions.pluginUninstallRequest}
          onClose={actions.closePluginUninstallRequest}
        />
      )}
    </div>
  );
}
