// ============================================================================
// SkillTestForm - The "Test" action's inline form: prompt, where to run
// (scratch / worktree / in place), a project picker for the latter two, and
// scratch-only "also install" and fixture inputs
// ============================================================================

import { useMemo, useState } from "react";
import { Button, Textarea } from "@skill-studio/ui";
import type { SkillRunTargetKind } from "../../lib/skill-run-target-types";
import type { InstalledSkill } from "../../lib/skill-types";
import { CheckboxControl } from "../ui/CheckboxControl";

export interface SkillTestRunParams {
  prompt: string;
  targetKind: SkillRunTargetKind;
  projectPath: string | undefined;
  extraSkillNames: string[];
  fixture: string | undefined;
}

interface SkillTestFormProps {
  skill: InstalledSkill;
  /** Every other own skill (not this one) available to "also install". */
  otherOwnSkills: InstalledSkill[];
  /** Projects where this skill is deployed, else every tracked project. */
  candidateProjects: string[];
  isRunning: boolean;
  onRun: (params: SkillTestRunParams) => void;
}

const TARGET_LABELS: [SkillRunTargetKind, string][] = [
  ["scratch", "Scratch"],
  ["worktree", "Worktree"],
  ["in_place", "In place"],
];

const FIXTURE_PLACEHOLDER = `=== path/to/file.ext
file contents

=== another/file.ext
more contents`;

/**
 * Renders inside `SkillAssistantPanel` when "Test" is active: a prompt box,
 * a Scratch/Worktree/In place segmented control, a project picker for the
 * latter two, and (Scratch only) an "also install" multi-select plus a
 * fixture textarea. Cmd+Enter runs, same as the Ask box.
 */
export function SkillTestForm({
  skill,
  otherOwnSkills,
  candidateProjects,
  isRunning,
  onRun,
}: SkillTestFormProps) {
  const [prompt, setPrompt] = useState("");
  const [targetKind, setTargetKind] = useState<SkillRunTargetKind>("scratch");
  const [projectPath, setProjectPath] = useState<string | undefined>(candidateProjects[0]);
  const [extraQuery, setExtraQuery] = useState("");
  const [extraSkillNames, setExtraSkillNames] = useState<string[]>([]);
  const [fixture, setFixture] = useState("");

  const filteredExtraSkills = useMemo(() => {
    const q = extraQuery.trim().toLowerCase();
    return q ? otherOwnSkills.filter((s) => s.name.toLowerCase().includes(q)) : otherOwnSkills;
  }, [otherOwnSkills, extraQuery]);

  const toggleExtraSkill = (name: string) => {
    setExtraSkillNames((prev) =>
      prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name],
    );
  };

  const canRun =
    prompt.trim().length > 0 && !isRunning && (targetKind === "scratch" || projectPath);

  const handleRun = () => {
    if (!canRun) return;
    onRun({
      prompt,
      targetKind,
      projectPath: targetKind === "scratch" ? undefined : projectPath,
      extraSkillNames,
      fixture: targetKind === "scratch" && fixture.trim() ? fixture : undefined,
    });
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      handleRun();
    }
  };

  return (
    <div className="flex flex-col gap-2.5">
      <Textarea
        className="resize-none rounded-sm border-border bg-bg-tertiary px-2.5 py-2 text-small text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
        placeholder={`What should the skill do? e.g. Use the ${skill.name} skill to …`}
        rows={2}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={handleKeyDown}
        disabled={isRunning}
      />

      <div className="flex flex-col gap-1.5">
        <span className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
          Where
        </span>
        <div className="harness-segmented-control">
          {TARGET_LABELS.map(([kind, label]) => (
            <button
              key={kind}
              type="button"
              className={`harness-segmented-control-item ${kind === targetKind ? "active" : ""}`}
              aria-pressed={kind === targetKind}
              onClick={() => setTargetKind(kind)}
              disabled={isRunning}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {targetKind !== "scratch" && (
        <div className="flex flex-col gap-1.5">
          {candidateProjects.length > 0 ? (
            <select
              className="rounded-sm border border-border bg-bg-tertiary px-2 py-1.5 text-small text-text-primary"
              value={projectPath ?? ""}
              onChange={(e) => setProjectPath(e.target.value || undefined)}
              disabled={isRunning}
            >
              {candidateProjects.map((path) => (
                <option key={path} value={path}>
                  {path}
                </option>
              ))}
            </select>
          ) : (
            <p className="m-0 text-caption text-text-tertiary">
              No tracked project is a git repository – only Scratch is available.
            </p>
          )}
          {targetKind === "in_place" && (
            <p className="m-0 text-caption text-text-tertiary">
              Requires a clean working tree; commit or stash first. Revert removes every change made
              in the project since the run started.
            </p>
          )}
        </div>
      )}

      {targetKind === "scratch" && (
        <>
          {otherOwnSkills.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="text-caption font-semibold tracking-[0.04em] text-text-tertiary uppercase">
                Also install
              </span>
              <input
                className="text-control"
                value={extraQuery}
                onChange={(e) => setExtraQuery(e.target.value)}
                placeholder="Filter skills…"
                disabled={isRunning}
              />
              <div className="flex max-h-40 flex-col gap-0.5 overflow-y-auto rounded-sm border border-border-subtle px-2 py-1">
                {filteredExtraSkills.map((other) => (
                  <label
                    key={other.name}
                    className="flex cursor-pointer items-center gap-1.5 py-0.5 text-small text-text-secondary"
                  >
                    <CheckboxControl
                      checked={extraSkillNames.includes(other.name)}
                      onCheckedChange={() => toggleExtraSkill(other.name)}
                      disabled={isRunning}
                    />
                    {other.name}
                  </label>
                ))}
              </div>
            </div>
          )}
          <Textarea
            className="resize-none rounded-sm border-border bg-bg-tertiary px-2.5 py-2 font-mono text-caption text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
            placeholder={FIXTURE_PLACEHOLDER}
            rows={4}
            value={fixture}
            onChange={(e) => setFixture(e.target.value)}
            disabled={isRunning}
          />
        </>
      )}

      <div className="flex items-center justify-between gap-2">
        <span />
        <Button size="sm" onClick={handleRun} disabled={!canRun}>
          Run test
        </Button>
      </div>
    </div>
  );
}
