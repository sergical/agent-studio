// ============================================================================
// SkillRepairCard - shown in place of the SKILL.md card when the currently
// shown location's symlink is broken. Walks the user through fixing the
// deployment: re-link it to a healthy copy of the same skill, reinstall it
// from its source, or remove the dangling link. Replaces the doomed
// `readInstalledSkillMd` error state for a deployment we already know can't
// be read (see `isUnresolvedDeployment`).
// ============================================================================

import { useState } from "react";
import { TriangleAlert } from "lucide-react";
import { Button, RadioGroup, RadioGroupItem } from "@skill-studio/ui";
import {
  agentIdFromDeploymentLabel,
  deploymentLabel,
  homeRelativePath,
  isUnresolvedDeployment,
  parseSkillSource,
} from "@skill-studio/lib";
import type { AgentId, Deployment, InstalledSkill, InstallScope } from "@skill-studio/lib";
import { addSkill, repairSkillLink } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";

interface SkillRepairCardProps {
  skill: InstalledSkill;
  /** The unresolved deployment this location shows. */
  deployment: Deployment;
}

type FixOption =
  | { kind: "relink"; target: Deployment }
  | { kind: "reinstall" }
  | { kind: "remove" };

function optionKey(option: FixOption): string {
  return option.kind === "relink" ? `relink:${option.target.path}` : option.kind;
}

/** The reinstall source `npx skills add` can use - only real repo sources, not the "manual" placeholder `InstalledSkill.source` carries otherwise. */
function reinstallSource(skill: InstalledSkill): string | null {
  return skill.source_kind === "skills-sh" || skill.source_kind === "dotagents"
    ? skill.source
    : null;
}

export function SkillRepairCard({ skill, deployment }: SkillRepairCardProps) {
  const addToast = useAppStore((state) => state.addToast);

  const healthyDeployments = skill.deployments.filter(
    (d) => d.path !== deployment.path && !isUnresolvedDeployment(d),
  );
  const source = reinstallSource(skill);
  const deploymentAgent = agentIdFromDeploymentLabel(deployment.agent);
  const reinstallAgent: AgentId | null =
    (deployment.scope === "global" || deployment.scope === "project") &&
    deploymentAgent &&
    deploymentAgent !== "shared"
      ? deploymentAgent
      : null;

  const options: FixOption[] = healthyDeployments.map((target) => ({ kind: "relink", target }));
  if (source && reinstallAgent) options.push({ kind: "reinstall" });
  options.push({ kind: "remove" });

  const [selectedKey, setSelectedKey] = useState(() => optionKey(options[0]));
  const selected = options.find((o) => optionKey(o) === selectedKey) ?? options[0];
  const [isFixing, setIsFixing] = useState(false);

  const rawTarget = deployment.symlink_target ?? "an unknown target";

  const handleFix = async () => {
    if (isFixing) return;
    setIsFixing(true);
    try {
      if (selected.kind === "relink") {
        await repairSkillLink(deployment.path, "relink", selected.target.path);
        addToast({
          type: "success",
          title: "Re-linked",
          message: `Re-linked to ${homeRelativePath(selected.target.path)}.`,
        });
      } else if (selected.kind === "reinstall") {
        if (!source || !reinstallAgent) {
          setIsFixing(false);
          return;
        }
        const scope: InstallScope = deployment.scope === "project" ? "project" : "global";
        const parsedSource = parseSkillSource(source);
        if ("error" in parsedSource || parsedSource.kind !== "github" || !parsedSource.repo) {
          addToast({
            type: "error",
            title: "Couldn't fix this location",
            message: `No GitHub repository is available for ${skill.name}`,
          });
          setIsFixing(false);
          return;
        }
        const result = await addSkill({
          source: {
            ...parsedSource,
            path: parsedSource.path ?? skill.name,
            skillName: skill.name,
          },
          method: "skills-sh",
          scope,
          destination: "universal",
          agents: reinstallAgent === "claude-code" ? ["claude-code"] : [],
          disabled_harnesses: [],
          project_path: scope === "project" ? deployment.project_path : undefined,
          trial: false,
        });
        if (result.warning) {
          addToast({
            type: "warning",
            title: "Reinstalled with a warning",
            message: result.warning,
          });
        }
        addToast({
          type: "success",
          title: "Reinstalled",
          message: `Reinstalled from ${source}.`,
        });
      } else {
        await repairSkillLink(deployment.path, "remove");
        addToast({
          type: "success",
          title: "Removed broken link",
          message: "Agents stop seeing this skill here.",
        });
      }
      setIsFixing(false);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't fix this location",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsFixing(false);
    }
  };

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div className="flex items-center justify-between gap-3 text-body font-semibold text-text-primary">
        <span className="flex items-center gap-2">
          <TriangleAlert size={15} className="text-warning" />
          Repair this location
        </span>
        <span
          className="truncate text-caption font-normal text-text-tertiary"
          title={deployment.path}
        >
          {deploymentLabel(deployment)}
        </span>
      </div>

      <p className="m-0 max-w-[62ch] p-3 pb-1 text-body leading-[1.5] text-text-secondary">
        <span className="font-mono text-small">{homeRelativePath(deployment.path)}</span> points to{" "}
        <span className="font-mono text-small">{homeRelativePath(rawTarget)}</span> — missing.
      </p>

      <RadioGroup
        className="gap-2 px-3 pb-1"
        value={selectedKey}
        onValueChange={(value) => setSelectedKey(value)}
      >
        {options.map((option) => (
          <RepairOptionRow key={optionKey(option)} option={option} skill={skill} source={source} />
        ))}
      </RadioGroup>

      <div className="flex items-center gap-3 px-3 pt-2 pb-1">
        <Button
          className="gap-2 bg-accent text-text-on-accent hover:bg-accent-hover"
          onClick={handleFix}
          disabled={isFixing || !selected}
        >
          {isFixing ? (
            <>
              <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
              Working…
            </>
          ) : (
            "Fix it"
          )}
        </Button>
        <span className="text-caption text-text-tertiary">
          {selected?.kind === "reinstall"
            ? "Re-link and remove are reversible from Activity → History."
            : "Reversible from Activity → History."}
        </span>
      </div>
    </div>
  );
}

interface RepairOptionRowProps {
  option: FixOption;
  skill: InstalledSkill;
  source: string | null;
}

function RepairOptionRow({ option, skill, source }: RepairOptionRowProps) {
  const { title, description } = describeOption(option, skill, source);
  return (
    <label className="flex cursor-pointer gap-3 rounded-sm border border-border-subtle bg-bg-elevated px-3.5 py-3 transition-colors hover:bg-bg-hover has-data-checked:border-accent has-data-checked:bg-accent-softer has-data-checked:shadow-[inset_0_0_0_1px_var(--color-accent)]">
      <RadioGroupItem
        value={optionKey(option)}
        className="mt-0.5 size-[15px] flex-none border-[1.5px] border-border-strong data-checked:border-accent data-checked:bg-accent data-checked:text-text-on-accent"
      />
      <span className="flex flex-col gap-0.5">
        <span className="font-medium text-text-primary">{title}</span>
        <span className="max-w-[58ch] text-small text-text-tertiary">{description}</span>
      </span>
    </label>
  );
}

interface RepairOptionCopy {
  title: string;
  description: string;
}

function describeOption(
  option: FixOption,
  skill: InstalledSkill,
  source: string | null,
): RepairOptionCopy {
  if (option.kind === "relink") {
    const path = homeRelativePath(option.target.path);
    return {
      title: `Re-link to ${path}`,
      description: `Re-links to ${path}. This location follows that copy from now on.`,
    };
  }
  if (option.kind === "reinstall") {
    return {
      title: `Reinstall from ${source}`,
      description: `Runs \`npx skills add ${source}\` and recreates this copy.`,
    };
  }
  return {
    title: "Remove broken link",
    description: `Deletes the dangling link. Agents stop seeing ${skill.name} here.`,
  };
}
