// ============================================================================
// SkillTestForm - The "Test" action's inline form: prompt, where to run
// (scratch / worktree / in place), a project picker for the latter two, and
// scratch-only "also install" and fixture inputs
// ============================================================================

import { useMemo, useState } from "react";
import type { SkillRunTargetKind } from "../../lib/skill-run-target-types";
import type { InstalledSkill } from "../../lib/skill-types";

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
    <div className="skill-test-form">
      <textarea
        className="skill-assistant-ask-input"
        placeholder={`What should the skill do? e.g. Use the ${skill.name} skill to …`}
        rows={2}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={handleKeyDown}
        disabled={isRunning}
      />

      <div className="skill-test-form-where">
        <span className="skill-test-form-label">Where</span>
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
        <div className="skill-test-form-project">
          {candidateProjects.length > 0 ? (
            <select
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
            <p className="skill-assistant-panel-note">
              No tracked project is a git repository - only Scratch is available.
            </p>
          )}
          {targetKind === "in_place" && (
            <p className="skill-test-form-tertiary">
              Requires a clean working tree; commit or stash first.
            </p>
          )}
        </div>
      )}

      {targetKind === "scratch" && (
        <>
          {otherOwnSkills.length > 0 && (
            <div className="skill-test-form-extra">
              <span className="skill-test-form-label">Also install</span>
              <input
                className="skill-test-form-extra-search"
                value={extraQuery}
                onChange={(e) => setExtraQuery(e.target.value)}
                placeholder="Filter skills…"
                disabled={isRunning}
              />
              <div className="skill-test-form-extra-list">
                {filteredExtraSkills.map((other) => (
                  <label key={other.name} className="skill-test-form-extra-item">
                    <input
                      type="checkbox"
                      checked={extraSkillNames.includes(other.name)}
                      onChange={() => toggleExtraSkill(other.name)}
                      disabled={isRunning}
                    />
                    {other.name}
                  </label>
                ))}
              </div>
            </div>
          )}
          <textarea
            className="skill-test-form-fixture"
            placeholder={FIXTURE_PLACEHOLDER}
            rows={4}
            value={fixture}
            onChange={(e) => setFixture(e.target.value)}
            disabled={isRunning}
          />
        </>
      )}

      <div className="skill-assistant-ask-footer">
        <span />
        <button
          type="button"
          className="skill-action-button primary"
          onClick={handleRun}
          disabled={!canRun}
        >
          Run test
        </button>
      </div>
    </div>
  );
}
