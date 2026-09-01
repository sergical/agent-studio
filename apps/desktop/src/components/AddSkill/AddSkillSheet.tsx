// ============================================================================
// AddSkillSheet - Right-side sheet for adding a skill from a source string:
// parses the Source field live, offers a Method segmented control, reuses
// AgentTargetSelector for Harnesses, a Global/Project Scope, and an optional
// "Try for 24 hours" trial. Submits to the `add_skill` Tauri command.
// ============================================================================

import { useEffect, useReducer, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderPlus, X } from "lucide-react";
import { Button, Input } from "@skill-studio/ui";
import { AgentTargetSelector } from "../SkillStore/AgentTargetSelector";
import { SkillStore } from "../SkillStore/SkillStore";
import { CheckboxControl } from "../ui/CheckboxControl";
import { addSkill, importSkillPack } from "../../lib/skill-api";
import { agentIdFromDeploymentLabel } from "@skill-studio/lib";
import { parseSkillSource } from "@skill-studio/lib";
import type { ParsedSkillSource } from "@skill-studio/lib";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { useAppStore } from "../../store/appStore";
import type { AddMethod, AgentId, InstallScope } from "@skill-studio/lib";

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

const SCOPE_OPTION_CLASS =
  "flex-1 rounded-sm border border-border bg-bg-primary px-2.5 py-2.5 text-body font-medium text-text-secondary transition-colors hover:border-border-focus";
const SCOPE_OPTION_SELECTED_CLASS = "border-accent bg-accent-softer text-accent";

/**
 * Every field on the sheet's manual-add form - reset together each time the
 * sheet opens (see the `reset` action), so a `useReducer` replaces what used
 * to be nine separate `useState` calls all cleared by the same effect.
 */
interface FormState {
  sheetTab: "manual" | "browse";
  source: string;
  methodChoice: SheetMethod;
  agents: AgentId[];
  scope: InstallScope;
  projectPath: string | null;
  trial: boolean;
  isSubmitting: boolean;
  submitError: string | null;
}

function initialFormState(): FormState {
  return {
    sheetTab: "manual",
    source: "",
    methodChoice: "dotagents",
    agents: FALLBACK_HARNESSES,
    scope: "global",
    projectPath: null,
    trial: false,
    isSubmitting: false,
    submitError: null,
  };
}

type FormAction =
  | { type: "reset"; prefill: string; agents: AgentId[]; projectPath: string | null }
  | { type: "set_tab"; tab: FormState["sheetTab"] }
  | { type: "set_source"; source: string }
  | { type: "set_method"; method: SheetMethod }
  | { type: "set_agents"; agents: AgentId[] }
  | { type: "set_scope"; scope: InstallScope }
  | { type: "set_project_path"; path: string | null }
  | { type: "set_trial"; trial: boolean }
  | { type: "submit_start" }
  | { type: "submit_error"; error: string }
  | { type: "submit_end" };

function formReducer(state: FormState, action: FormAction): FormState {
  switch (action.type) {
    case "reset":
      return {
        ...initialFormState(),
        source: action.prefill,
        agents: action.agents,
        projectPath: action.projectPath,
      };
    case "set_tab":
      return { ...state, sheetTab: action.tab };
    case "set_source":
      return { ...state, source: action.source };
    case "set_method":
      return { ...state, methodChoice: action.method };
    case "set_agents":
      return { ...state, agents: action.agents };
    case "set_scope":
      return { ...state, scope: action.scope };
    case "set_project_path":
      return { ...state, projectPath: action.path };
    case "set_trial":
      return { ...state, trial: action.trial };
    case "submit_start":
      return { ...state, isSubmitting: true, submitError: null };
    case "submit_error":
      return { ...state, isSubmitting: false, submitError: action.error };
    case "submit_end":
      return { ...state, isSubmitting: false };
  }
}

/** Source field + live parse feedback. */
function SourceField({
  source,
  parsed,
  onChange,
  inputRef,
}: {
  source: string;
  parsed: ParsedSkillSource | { error: string };
  onChange: (value: string) => void;
  inputRef: React.RefObject<HTMLInputElement | null>;
}) {
  return (
    <div className="flex flex-col gap-2">
      <label
        htmlFor="add-skill-source"
        className="text-caption font-medium tracking-[0.06em] text-text-tertiary uppercase"
      >
        Source
      </label>
      <Input
        id="add-skill-source"
        ref={inputRef}
        type="text"
        className="h-(--control-height) rounded-sm border-border bg-bg-primary text-body text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
        value={source}
        onChange={(e) => onChange(e.target.value)}
        placeholder="owner/repo, a GitHub URL, a skills.sh URL, or a local path"
      />
      <p className={`m-0 text-small ${"error" in parsed ? "text-error" : "text-text-tertiary"}`}>
        {source.trim() ? parseSummary(parsed) : "Enter a source above"}
      </p>
    </div>
  );
}

/** The Method segmented control - which choices are enabled comes from `availableMethods(parsed)`. */
function MethodPicker({
  method,
  methods,
  agents,
  onChange,
}: {
  method: SheetMethod;
  methods: SheetMethod[];
  agents: AgentId[];
  onChange: (method: SheetMethod) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      {/* A heading for the method button group, not a form control's
          label - a `<label>` here would have no associated control. */}
      <span className="text-caption font-medium tracking-[0.06em] text-text-tertiary uppercase">
        Method
      </span>
      <div className="flex flex-wrap gap-1.5">
        {ALL_SHEET_METHODS.map((m) => {
          const disabled = methods.length > 0 && !methods.includes(m);
          return (
            <button
              key={m}
              type="button"
              className={`inline-flex h-[26px] items-center gap-1.5 rounded-sm border border-border bg-bg-tertiary px-2.5 text-caption text-text-tertiary transition-colors enabled:hover:bg-bg-hover enabled:hover:text-text-secondary disabled:cursor-not-allowed ${
                method === m ? "border-text-tertiary text-text-primary" : ""
              } ${disabled ? "opacity-40" : ""}`}
              title={METHOD_TOOLTIPS[m]}
              disabled={disabled}
              onClick={() => onChange(m)}
            >
              {METHOD_LABELS[m]}
            </button>
          );
        })}
      </div>
      {method === "dotagents" && agents.includes("grok-build") && (
        <p className="m-0 text-caption text-text-tertiary">Grok Build reads the shared folder.</p>
      )}
    </div>
  );
}

/** Global/Project scope toggle, plus the project picker and "Choose Directory"/"Add" button. */
function ScopePicker({
  scope,
  projectPath,
  userAddedProjects,
  onScopeChange,
  onProjectPathChange,
  onBrowseProject,
}: {
  scope: InstallScope;
  projectPath: string | null;
  userAddedProjects: string[];
  onScopeChange: (scope: InstallScope) => void;
  onProjectPathChange: (path: string) => void;
  onBrowseProject: () => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      {/* A heading for the scope button group, not a form control's
          label - a `<label>` here would have no associated control. */}
      <span className="text-caption font-medium tracking-[0.06em] text-text-tertiary uppercase">
        Scope
      </span>
      <div className="flex gap-2">
        <button
          type="button"
          className={`${SCOPE_OPTION_CLASS} ${scope === "global" ? SCOPE_OPTION_SELECTED_CLASS : ""}`}
          onClick={() => onScopeChange("global")}
        >
          Global
        </button>
        <button
          type="button"
          className={`${SCOPE_OPTION_CLASS} ${scope === "project" ? SCOPE_OPTION_SELECTED_CLASS : ""}`}
          onClick={() => onScopeChange("project")}
        >
          Project
        </button>
      </div>
      {scope === "project" && (
        <div className="flex gap-2">
          {userAddedProjects.length > 0 && (
            <select
              className="flex-1 rounded-sm border border-border bg-bg-primary px-2.5 py-2.5 text-body text-text-primary"
              aria-label="Project directory"
              value={projectPath ?? ""}
              onChange={(e) => onProjectPathChange(e.target.value)}
            >
              {userAddedProjects.map((p) => (
                <option key={p} value={p}>
                  {p.split("/").pop()} – {p}
                </option>
              ))}
            </select>
          )}
          <Button
            variant="outline"
            className="h-(--control-height) gap-2 rounded-md px-3.5 text-body font-medium"
            onClick={onBrowseProject}
          >
            <FolderPlus size={14} />
            {userAddedProjects.length === 0 ? "Choose Directory" : "Add"}
          </Button>
        </div>
      )}
    </div>
  );
}

export function AddSkillSheet() {
  const { open: isOpen, prefill } = useAppStore((state) => state.addSkillSheet);
  const closeAddSkillSheet = useAppStore((state) => state.closeAddSkillSheet);
  const openSkill = useAppStore((state) => state.openSkill);
  const addToast = useAppStore((state) => state.addToast);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const addProject = useAppStore((state) => state.addProject);
  const { snapshot } = useSkillSnapshot();

  const [form, dispatch] = useReducer(formReducer, undefined, initialFormState);
  const {
    sheetTab,
    source,
    methodChoice,
    agents,
    scope,
    projectPath,
    trial,
    isSubmitting,
    submitError,
  } = form;

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
    const withASkill = agentsWithASkill(snapshot);
    const active = DEFAULT_HARNESS_CANDIDATES.filter((id) => withASkill.has(id));
    dispatch({
      type: "reset",
      prefill: prefill ?? "",
      agents: active.length > 0 ? active : FALLBACK_HARNESSES,
      projectPath: userAddedProjects[0] ?? null,
    });
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

  const parsed = parseSkillSource(source);
  const methods = availableMethods(parsed);

  // Keep the selected method valid as the source changes - e.g. switching
  // from a github source to a local path forces "Copy". Derived during
  // render instead of synced back with an effect, since `methods` is
  // itself derived from `source`.
  const method = methods.length > 0 && !methods.includes(methodChoice) ? methods[0] : methodChoice;

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
      dispatch({ type: "set_project_path", path: selected });
    }
  };

  const handleSubmit = async () => {
    if ("error" in parsed || !isValid) return;
    // Pack imports always target the repo itself, not a sub-path - the
    // sheet's Source field can point at a path within it, but a pack's own
    // agents.toml lives at the repo root. Checked before the try below since
    // the compiler can't follow a `throw` thrown from inside its own catch.
    const packRepo = parsed.kind === "github" ? parsed.repo : undefined;
    if (method === "pack" && !packRepo) {
      dispatch({ type: "submit_error", error: "Pack import needs a GitHub repo" });
      return;
    }
    dispatch({ type: "submit_start" });
    try {
      if (method === "pack") {
        // SAFETY: the `!packRepo` guard above already returned otherwise.
        const result = await importSkillPack(packRepo!, agents);
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
        dispatch({ type: "submit_end" });
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
      dispatch({ type: "submit_end" });
    } catch (err) {
      dispatch({
        type: "submit_error",
        error: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  return (
    <div className="fixed inset-0 z-(--z-modal) flex justify-end">
      {/* Decorative scrim: a pointer affordance duplicating the Escape/Cancel
          path above, so it's aria-hidden rather than a focusable control. */}
      <div
        className="absolute inset-0 bg-scrim"
        aria-hidden="true"
        onMouseDown={() => {
          if (!isSubmitting) closeSheet();
        }}
      />
      <div
        ref={sheetRef}
        className="relative flex h-full w-[420px] max-w-full flex-col border-l border-border bg-bg-secondary"
        role="dialog"
        aria-modal="true"
        aria-label="Add skill"
      >
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <h3 className="m-0 text-pretty text-balance text-emphasis font-semibold text-text-primary">
            Add skill
          </h3>
          <button
            type="button"
            className="flex items-center justify-center rounded-sm border-0 bg-transparent p-1 text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-primary"
            onClick={closeSheet}
            disabled={isSubmitting}
            aria-label="Close"
          >
            <X size={16} />
          </button>
        </div>

        <div className="flex border-b border-border">
          <button
            type="button"
            className={`-mb-px h-10 border-0 border-b-2 bg-transparent px-4 text-body font-medium transition-colors ${
              sheetTab === "manual"
                ? "border-accent text-accent"
                : "border-transparent text-text-tertiary hover:text-text-secondary"
            }`}
            onClick={() => dispatch({ type: "set_tab", tab: "manual" })}
          >
            Add by source
          </button>
          <button
            type="button"
            className={`-mb-px h-10 border-0 border-b-2 bg-transparent px-4 text-body font-medium transition-colors ${
              sheetTab === "browse"
                ? "border-accent text-accent"
                : "border-transparent text-text-tertiary hover:text-text-secondary"
            }`}
            onClick={() => dispatch({ type: "set_tab", tab: "browse" })}
          >
            Browse skills.sh
          </button>
        </div>

        {sheetTab === "browse" ? (
          <div className="flex flex-1 overflow-hidden">
            <SkillStore compact />
          </div>
        ) : (
          <div className="flex flex-1 flex-col gap-5 overflow-y-auto px-5 py-4">
            <SourceField
              source={source}
              parsed={parsed}
              onChange={(value) => dispatch({ type: "set_source", source: value })}
              inputRef={sourceInputRef}
            />

            <MethodPicker
              method={method}
              methods={methods}
              agents={agents}
              onChange={(m) => dispatch({ type: "set_method", method: m })}
            />

            <div className="flex flex-col gap-2">
              <AgentTargetSelector
                selectedAgents={agents}
                onChange={(next) => dispatch({ type: "set_agents", agents: next })}
              />
            </div>

            {method === "pack" && (
              <p className="m-0 text-caption text-text-tertiary">
                Imports every skill in this repo's pack to the shared folder, plus any agents.toml
                row pointing elsewhere - see the "Packs" section of the docs.
              </p>
            )}

            {method !== "pack" && (
              <ScopePicker
                scope={scope}
                projectPath={projectPath}
                userAddedProjects={userAddedProjects}
                onScopeChange={(next) => dispatch({ type: "set_scope", scope: next })}
                onProjectPathChange={(path) => dispatch({ type: "set_project_path", path })}
                onBrowseProject={handleBrowseProject}
              />
            )}

            {method !== "pack" && (
              <div className="flex flex-col gap-2">
                <label className="flex items-center gap-2 text-body text-text-primary">
                  <CheckboxControl
                    checked={trial}
                    onCheckedChange={(next) => dispatch({ type: "set_trial", trial: next })}
                  />
                  Try for 24 hours
                </label>
                <p className="m-0 text-caption text-text-tertiary">
                  Removed automatically after 24 h unless you keep it.
                </p>
              </div>
            )}

            {submitError && (
              <p className="m-0 rounded-md bg-error-soft p-2.5 text-small text-error" role="alert">
                {submitError}
              </p>
            )}
          </div>
        )}

        {sheetTab === "manual" && (
          <div className="flex justify-end gap-2 border-t border-border px-5 py-4">
            <Button
              variant="outline"
              className="h-(--control-height) rounded-md px-3.5 text-body font-medium"
              onClick={closeSheet}
              disabled={isSubmitting}
            >
              Cancel
            </Button>
            <Button
              className="h-(--control-height) rounded-md bg-accent px-3.5 text-body font-medium text-text-on-accent hover:bg-accent-hover"
              onClick={handleSubmit}
              disabled={!isValid || isSubmitting}
            >
              {isSubmitting ? "Adding…" : method === "pack" ? "Import pack" : "Add skill"}
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
