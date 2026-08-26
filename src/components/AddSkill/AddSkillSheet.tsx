// ============================================================================
// AddSkillSheet - Right-side sheet for adding a skill from a source string:
// parses the Source field live, offers a Method segmented control, reuses
// AgentTargetSelector for Harnesses, a Global/Project Scope, and an optional
// "Try for 24 hours" trial. Submits to the `add_skill` Tauri command.
// ============================================================================

import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderPlus, X } from "lucide-react";
import { AgentTargetSelector } from "../SkillStore/AgentTargetSelector";
import { SkillStore } from "../SkillStore/SkillStore";
import { addSkill, importSkillPack } from "../../lib/skill-api";
import { agentIdFromDeploymentLabel } from "../../lib/skill-coverage";
import { parseSkillSource } from "../../lib/skill-source-parse";
import type { ParsedSkillSource } from "../../lib/skill-source-parse";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { useAppStore } from "../../store/appStore";
import type { AddMethod, AgentId, InstallScope } from "../../lib/skill-types";

/**
 * The six agents offered as harness defaults - `push_agent_args`'s
 * install-target list plus Grok Build, which reads the shared root directly
 * and is filtered out of the skills.sh argv rather than being disallowed.
 */
const DEFAULT_HARNESS_CANDIDATES: AgentId[] = [
  "claude-code",
  "codex",
  "open-code",
  "pi",
  "cursor",
  "grok-build",
];
const FALLBACK_HARNESSES: AgentId[] = ["claude-code", "codex"];

const ALL_METHODS = ["dotagents", "skills-sh", "copy"] as const satisfies AddMethod[];

/**
 * "Pack" isn't a real `AddMethod` - it doesn't run `addSkill`, it runs
 * `importSkillPack` against a share pack's repo (see `skill_pack.rs`'s
 * `import_skill_pack`). Kept out of the shared `AddMethod` type so trial
 * tracking and every other `AddMethod` switch never has to account for it.
 */
type SheetMethod = AddMethod | "pack";
const ALL_SHEET_METHODS = [...ALL_METHODS, "pack"] as const satisfies SheetMethod[];

const METHOD_LABELS = {
  dotagents: "dotagents",
  "skills-sh": "skills.sh",
  copy: "Copy",
  pack: "Pack",
} satisfies Record<SheetMethod, string>;

const METHOD_TOOLTIPS = {
  dotagents: "Tracked in ~/.agents/agents.toml; updates with dotagents",
  "skills-sh": "Tracked in ~/.agents/.skill-lock.json",
  copy: "Not tracked; updates unavailable",
  pack: "Import every skill in this repo's share pack",
} satisfies Record<SheetMethod, string>;

/** Which methods a parsed source supports, in the sheet's preferred order. */
function availableMethods(parsed: ParsedSkillSource | { error: string }): SheetMethod[] {
  if ("error" in parsed) return [];
  if (parsed.kind === "github") return ["dotagents", "skills-sh", "copy", "pack"];
  if (parsed.kind === "git") return ["dotagents"];
  return ["copy"];
}

/** One-line parse feedback shown beneath the Source field. */
function parseSummary(parsed: ParsedSkillSource | { error: string }): string {
  if ("error" in parsed) return parsed.error;
  if (parsed.kind === "github") {
    return `github · ${parsed.repo}${parsed.path ? ` · ${parsed.path}` : ""}`;
  }
  if (parsed.kind === "git") return `git · ${parsed.url}`;
  return `local · ${parsed.localPath}`;
}

/** Every AgentId with at least one deployed skill in the snapshot, from the six candidates. */
function agentsWithASkill(snapshot: ReturnType<typeof useSkillSnapshot>["snapshot"]): Set<AgentId> {
  const seen = new Set<AgentId>();
  for (const skill of snapshot?.skills ?? []) {
    for (const deployment of skill.deployments) {
      const id = agentIdFromDeploymentLabel(deployment.agent);
      if (id && id !== "shared") seen.add(id);
    }
  }
  return seen;
}

const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function AddSkillSheet() {
  const { open: isOpen, prefill } = useAppStore((state) => state.addSkillSheet);
  const closeAddSkillSheet = useAppStore((state) => state.closeAddSkillSheet);
  const openSkill = useAppStore((state) => state.openSkill);
  const addToast = useAppStore((state) => state.addToast);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const addProject = useAppStore((state) => state.addProject);
  const { snapshot } = useSkillSnapshot();

  const [sheetTab, setSheetTab] = useState<"manual" | "browse">("manual");
  const [source, setSource] = useState("");
  const [method, setMethod] = useState<SheetMethod>("dotagents");
  const [agents, setAgents] = useState<AgentId[]>(FALLBACK_HARNESSES);
  const [scope, setScope] = useState<InstallScope>("global");
  const [projectPath, setProjectPath] = useState<string | null>(null);
  const [trial, setTrial] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const sheetRef = useRef<HTMLDivElement>(null);
  const sourceInputRef = useRef<HTMLInputElement>(null);
  // The element that had focus right before the sheet opened, so a normal
  // close (Cancel/X/Escape/backdrop click) can hand focus back to it.
  const previouslyFocusedRef = useRef<HTMLElement | null>(null);

  // Reset the form to its defaults, prefilled from the caller, each time the
  // sheet opens - a stale field from a previous open would be confusing.
  useEffect(() => {
    if (!isOpen) return;
    // SAFETY: `document.activeElement` is always an `Element` or `null`; DOM
    // focusable elements are `HTMLElement`s, which is all this ref is used
    // to call `.focus()` on.
    previouslyFocusedRef.current = document.activeElement as HTMLElement | null;
    setSheetTab("manual");
    setSource(prefill ?? "");
    setMethod("dotagents");
    const withASkill = agentsWithASkill(snapshot);
    const active = DEFAULT_HARNESS_CANDIDATES.filter((id) => withASkill.has(id));
    setAgents(active.length > 0 ? active : FALLBACK_HARNESSES);
    setScope("global");
    setProjectPath(userAddedProjects[0] ?? null);
    setTrial(false);
    setIsSubmitting(false);
    setSubmitError(null);
    // Autofocus the Source field once the sheet has mounted.
    const id = window.setTimeout(() => sourceInputRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, prefill]);

  // A normal close restores focus to whatever had it before the sheet
  // opened - never called while a submit is in flight (an install already
  // succeeded shouldn't be interrupted by an accidental Escape/backdrop click).
  const closeSheet = () => {
    closeAddSkillSheet();
    previouslyFocusedRef.current?.focus();
  };

  const parsed = useMemo(() => parseSkillSource(source), [source]);
  const methods = useMemo(() => availableMethods(parsed), [parsed]);

  // Keep the selected method valid as the source changes - e.g. switching
  // from a github source to a local path forces "Copy".
  useEffect(() => {
    if (methods.length > 0 && !methods.includes(method)) {
      setMethod(methods[0]);
    }
  }, [methods, method]);

  // Escape closes the sheet, unless it's typed into a nested widget that
  // wants it for its own purposes (mirrors SkillPage's handler).
  useEffect(() => {
    if (!isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        if (isSubmitting) return;
        closeSheet();
        return;
      }
      // Hand-rolled focus trap: Tab/Shift+Tab cycle within the sheet only,
      // since a new dependency isn't available for this.
      if (event.key !== "Tab" || !sheetRef.current) return;
      const focusable = Array.from(
        sheetRef.current.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      ).filter((el) => el.offsetParent !== null);
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey) {
        if (active === first || !sheetRef.current.contains(active)) {
          event.preventDefault();
          last.focus();
        }
      } else if (active === last || !sheetRef.current.contains(active)) {
        event.preventDefault();
        first.focus();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, isSubmitting]);

  if (!isOpen) return null;

  const isValid = !("error" in parsed) && (scope !== "project" || !!projectPath);

  const handleBrowseProject = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Select Project" });
    if (selected) {
      addProject(selected);
      setProjectPath(selected);
    }
  };

  const handleSubmit = async () => {
    if ("error" in parsed || !isValid) return;
    setIsSubmitting(true);
    setSubmitError(null);
    try {
      if (method === "pack") {
        // Pack imports always target the repo itself, not a sub-path - the
        // sheet's Source field can point at a path within it, but a pack's
        // own agents.toml lives at the repo root.
        if (parsed.kind !== "github" || !parsed.repo) {
          throw new Error("Pack import needs a GitHub repo");
        }
        const result = await importSkillPack(parsed.repo, agents);
        closeSheet();
        const total = result.bundled.length + result.referenced.length;
        if (result.errors.length > 0) {
          addToast({
            type: "warning",
            title: `Imported ${total} skill${total !== 1 ? "s" : ""}`,
            message: result.errors.join("; "),
          });
        } else {
          addToast({ type: "success", title: `Imported ${total} skill${total !== 1 ? "s" : ""}` });
        }
        return;
      }
      const result = await addSkill({
        source: parsed,
        method,
        agents,
        scope,
        project_path: scope === "project" ? (projectPath ?? undefined) : undefined,
        trial,
      });
      closeSheet();
      if (result.warning) {
        addToast({ type: "warning", title: `Added ${result.name}`, message: result.warning });
      } else {
        addToast({ type: "success", title: `Added ${result.name}` });
      }
      openSkill(result.name);
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <div
      className="add-skill-sheet-overlay"
      onMouseDown={() => {
        if (!isSubmitting) closeSheet();
      }}
    >
      <div
        ref={sheetRef}
        className="add-skill-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Add skill"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="add-skill-sheet-header">
          <h3>Add skill</h3>
          <button
            type="button"
            className="add-skill-sheet-close"
            onClick={closeSheet}
            disabled={isSubmitting}
          >
            <X size={16} />
          </button>
        </div>

        <div className="add-skill-sheet-tabs">
          <button
            type="button"
            className={`add-skill-sheet-tab ${sheetTab === "manual" ? "active" : ""}`}
            onClick={() => setSheetTab("manual")}
          >
            Add by source
          </button>
          <button
            type="button"
            className={`add-skill-sheet-tab ${sheetTab === "browse" ? "active" : ""}`}
            onClick={() => setSheetTab("browse")}
          >
            Browse skills.sh
          </button>
        </div>

        {sheetTab === "browse" ? (
          <div className="add-skill-sheet-browse">
            <SkillStore compact />
          </div>
        ) : (
          <div className="add-skill-sheet-body">
            <div className="add-skill-sheet-field">
              <label htmlFor="add-skill-source">Source</label>
              <input
                id="add-skill-source"
                ref={sourceInputRef}
                type="text"
                value={source}
                onChange={(e) => setSource(e.target.value)}
                placeholder="owner/repo, a GitHub URL, a skills.sh URL, or a local path"
              />
              <p className={`add-skill-sheet-parse ${"error" in parsed ? "error" : ""}`}>
                {source.trim() ? parseSummary(parsed) : "Enter a source above"}
              </p>
            </div>

            <div className="add-skill-sheet-field">
              <label>Method</label>
              <div className="harness-segmented-control">
                {ALL_SHEET_METHODS.map((m) => {
                  const disabled = methods.length > 0 && !methods.includes(m);
                  return (
                    <button
                      key={m}
                      type="button"
                      className={`harness-segmented-control-item ${method === m ? "active" : ""} ${
                        disabled ? "unavailable" : ""
                      }`}
                      title={METHOD_TOOLTIPS[m]}
                      disabled={disabled}
                      onClick={() => setMethod(m)}
                    >
                      {METHOD_LABELS[m]}
                    </button>
                  );
                })}
              </div>
              {method === "dotagents" && agents.includes("grok-build") && (
                <p className="add-skill-sheet-note">Grok Build reads the shared folder.</p>
              )}
            </div>

            <div className="add-skill-sheet-field">
              <AgentTargetSelector selectedAgents={agents} onChange={setAgents} />
            </div>

            {method === "pack" && (
              <p className="add-skill-sheet-note">
                Imports every skill in this repo's pack to the shared folder, plus any agents.toml
                row pointing elsewhere - see the "Packs" section of the docs.
              </p>
            )}

            {method !== "pack" && (
              <div className="add-skill-sheet-field">
                <label>Scope</label>
                <div className="skill-detail-scope-toggle">
                  <button
                    type="button"
                    className={`scope-option ${scope === "global" ? "selected" : ""}`}
                    onClick={() => setScope("global")}
                  >
                    Global
                  </button>
                  <button
                    type="button"
                    className={`scope-option ${scope === "project" ? "selected" : ""}`}
                    onClick={() => setScope("project")}
                  >
                    Project
                  </button>
                </div>
                {scope === "project" && (
                  <div className="skill-detail-project-select-row">
                    {userAddedProjects.length > 0 && (
                      <select
                        value={projectPath ?? ""}
                        onChange={(e) => setProjectPath(e.target.value)}
                      >
                        {userAddedProjects.map((p) => (
                          <option key={p} value={p}>
                            {p.split("/").pop()} - {p}
                          </option>
                        ))}
                      </select>
                    )}
                    <button
                      type="button"
                      className="skill-action-button"
                      onClick={handleBrowseProject}
                    >
                      <FolderPlus size={14} />
                      {userAddedProjects.length === 0 ? "Choose Directory" : "Add"}
                    </button>
                  </div>
                )}
              </div>
            )}

            {method !== "pack" && (
              <div className="add-skill-sheet-field">
                <label className="add-skill-sheet-checkbox">
                  <input
                    type="checkbox"
                    checked={trial}
                    onChange={(e) => setTrial(e.target.checked)}
                  />
                  Try for 24 hours
                </label>
                <p className="add-skill-sheet-note">
                  Removed automatically after 24 h unless you keep it.
                </p>
              </div>
            )}

            {submitError && (
              <p className="add-skill-sheet-error" role="alert">
                {submitError}
              </p>
            )}
          </div>
        )}

        {sheetTab === "manual" && (
          <div className="add-skill-sheet-footer">
            <button
              type="button"
              className="skill-action-button"
              onClick={closeSheet}
              disabled={isSubmitting}
            >
              Cancel
            </button>
            <button
              type="button"
              className="skill-action-button primary"
              onClick={handleSubmit}
              disabled={!isValid || isSubmitting}
            >
              {isSubmitting ? "Adding…" : method === "pack" ? "Import pack" : "Add skill"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
