// ============================================================================
// AddSkillSheet - Right-side sheet for adding a skill from a source string:
// parses the Source field live, lists the skill folders a GitHub source
// actually holds (one skill, or a picker for a folder of them), offers a
// Method segmented control, reuses AgentTargetSelector for Harnesses, a
// Global/Project Scope, and an optional "Try for 24 hours" trial. Submits to
// the `add_skills` Tauri command for GitHub sources, `add_skill` otherwise.
// ============================================================================

import { useEffect, useReducer, useRef, useState } from "react";
import type { Dispatch } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Folder, FolderPlus } from "lucide-react";
import {
  Button,
  Drawer,
  DrawerContent,
  Input,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  ToggleGroup,
  ToggleGroupItem,
} from "@skill-studio/ui";
import {
  AgentTargetSelector,
  installDisabledHarnesses,
  installTargetAgents,
} from "../SkillStore/AgentTargetSelector";
import { ProjectDirectorySelect } from "../SkillStore/ProjectDirectorySelect";
import { ScopeToggleGroup } from "../SkillStore/ScopeToggleGroup";
import { SkillStore } from "../SkillStore/SkillStore";
import { CheckboxControl } from "../ui/CheckboxControl";
import {
  addSkill,
  addSkills,
  getAddMethodDefaults,
  importSkillPack,
  listGithubSkills,
} from "../../lib/skill-api";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import { parseSkillSource } from "@skill-studio/lib";
import type { ParsedSkillSource } from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { useAppStore } from "../../store/appStore";
import type {
  AddMethod,
  AddMethodDefaults,
  AgentId,
  GithubSkillEntry,
  GithubSkillListing,
  InstallScope,
} from "@skill-studio/lib";

const SHEET_TAB_CLASS =
  "text-body font-medium text-text-tertiary after:bg-accent data-active:text-accent hover:text-text-secondary";

/** The uppercase field-group heading used above the Source, Skills, Method,
 * and Scope sections. */
const SECTION_LABEL_CLASS =
  "text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase";

const ALL_METHODS = ["dotagents", "skills-sh", "copy"] as const satisfies AddMethod[];

/**
 * "Pack" isn't a real `AddMethod` - it doesn't run `addSkill`, it runs
 * `importSkillPack` against a share pack's repo (see `skill_pack.rs`'s
 * `import_skill_pack`). Kept out of the shared `AddMethod` type so trial
 * tracking and every other `AddMethod` switch never has to account for it.
 */
type SheetMethod = AddMethod | "pack";
const ALL_SHEET_METHODS = [...ALL_METHODS, "pack"] as const satisfies SheetMethod[];

/** Pack import stays hidden until the `skill-packs` flag ships. */
function sheetMethods(): readonly SheetMethod[] {
  return isFeatureEnabled("skill-packs") ? ALL_SHEET_METHODS : ALL_METHODS;
}

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

/**
 * Which methods a parsed source supports, in the sheet's preferred order -
 * the first entry in each list is also that source kind's default, so the
 * "clamp to a valid method" derivation below doubles as "reset to the
 * default when the source kind changes". Dropping "dotagents" entirely when
 * it can't run (rather than just disabling it) keeps a stale pick from a
 * previous source silently surviving a switch to one where it's unusable.
 * A GitHub source with `has_skill_lock` still defaults to dotagents when
 * it's installed - the sheet has no reason to prefer skills.sh just because
 * it's been used here before.
 */
export function availableMethods(
  parsed: ParsedSkillSource | { error: string },
  defaults: AddMethodDefaults | null,
): SheetMethod[] {
  if ("error" in parsed) return [];
  const dotagentsInstalled = defaults?.dotagents_installed ?? true;
  if (parsed.kind === "github") {
    return dotagentsInstalled
      ? ["dotagents", "skills-sh", "copy", "pack"]
      : ["skills-sh", "copy", "pack"];
  }
  if (parsed.kind === "git") return dotagentsInstalled ? ["dotagents"] : [];
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

/**
 * Every field on the sheet's manual-add form - reset together each time the
 * sheet opens (see the `reset` action), so a `useReducer` replaces what used
 * to be nine separate `useState` calls all cleared by the same effect.
 */
interface FormState {
  sheetTab: "manual" | "browse";
  source: string;
  methodChoice: SheetMethod;
  /** The installed readers of the shared folder left switched on. `null`
   * until `getAddMethodDefaults` answers, which is what seeds it. */
  enabledReaders: AgentId[] | null;
  /** Whether Claude Code's own skills dir gets linked into the shared
   * folder for this install. See `installTargetAgents`. */
  claudeLink: boolean;
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
    enabledReaders: null,
    claudeLink: true,
    scope: "global",
    projectPath: null,
    trial: false,
    isSubmitting: false,
    submitError: null,
  };
}

type FormAction =
  | { type: "reset"; prefill: string; projectPath: string | null }
  | { type: "set_tab"; tab: FormState["sheetTab"] }
  | { type: "set_source"; source: string }
  | { type: "set_method"; method: SheetMethod }
  | { type: "set_readers"; readers: AgentId[] }
  | { type: "set_reader_enabled"; agent: AgentId; enabled: boolean }
  | { type: "set_claude_link"; claudeLink: boolean }
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
        projectPath: action.projectPath,
      };
    case "set_tab":
      return { ...state, sheetTab: action.tab };
    case "set_source":
      return { ...state, source: action.source };
    case "set_method":
      return { ...state, methodChoice: action.method };
    case "set_readers":
      return { ...state, enabledReaders: action.readers };
    case "set_reader_enabled":
      return {
        ...state,
        enabledReaders: action.enabled
          ? [...(state.enabledReaders ?? []), action.agent]
          : (state.enabledReaders ?? []).filter((id) => id !== action.agent),
      };
    case "set_claude_link":
      return { ...state, claudeLink: action.claudeLink };
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
  // Parse errors stay neutral while the user is still typing; they turn red
  // only after the field loses focus, so a fresh sheet never opens "dirty".
  const [touched, setTouched] = useState(false);
  const showError = touched && source.trim().length > 0 && "error" in parsed;
  return (
    <div className="flex flex-col gap-2">
      <label htmlFor="add-skill-source" className={SECTION_LABEL_CLASS}>
        Source
      </label>
      <Input
        id="add-skill-source"
        ref={inputRef}
        type="text"
        className="h-(--control-height) rounded-sm border-border bg-bg-primary text-body text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
        value={source}
        onChange={(e) => onChange(e.target.value)}
        onBlur={() => setTouched(true)}
        placeholder="owner/repo, a GitHub URL, a skills.sh URL, or a local path"
      />
      <p className={`m-0 text-small ${showError ? "text-error" : "text-text-tertiary"}`}>
        {source.trim() ? parseSummary(parsed) : "Paste a repo, URL, or path to get started."}
      </p>
    </div>
  );
}

// ============================================================================
// GitHub skill listing - which folders a source actually holds
// ============================================================================

/** How long after the last keystroke the listing request goes out. */
const LISTING_DEBOUNCE_MS = 400;

interface ListingState {
  status: "idle" | "loading" | "ready" | "error";
  listing: GithubSkillListing | null;
  error: string | null;
}

const IDLE_LISTING: ListingState = { status: "idle", listing: null, error: null };

/** The repo, path, and ref a GitHub source lists under, or `null` when the
 * source isn't one this sheet lists (a parse error, git, or local). */
function listingTarget(parsed: ParsedSkillSource | { error: string }) {
  if ("error" in parsed || parsed.kind !== "github" || !parsed.repo) return null;
  return { repo: parsed.repo, path: parsed.path, ref: parsed.ref };
}

/**
 * Lists `parsed`'s skill folders once the source field settles, keeping the
 * result per repo/path/ref so retyping the same source costs nothing. A
 * newer request always wins: `requestId` invalidates whatever an older one
 * resolves with.
 */
function useGithubSkillListing(
  parsed: ParsedSkillSource | { error: string },
  enabled: boolean,
): ListingState & { retry: () => void } {
  const target = enabled ? listingTarget(parsed) : null;
  const repo = target?.repo;
  const path = target?.path;
  const ref = target?.ref;
  const key = target ? `${repo}|${path ?? ""}|${ref ?? ""}` : null;

  const [state, setState] = useState<ListingState>(IDLE_LISTING);
  const [retryKey, setRetryKey] = useState<string | null>(null);
  const cacheRef = useRef(new Map<string, GithubSkillListing>());
  const requestIdRef = useRef(0);
  const forceRefresh = key !== null && retryKey === key;

  useEffect(() => {
    if (!key || !repo) {
      setState(IDLE_LISTING);
      return;
    }
    const cached = forceRefresh ? undefined : cacheRef.current.get(key);
    if (cached) {
      setState({ status: "ready", listing: cached, error: null });
      return;
    }
    const requestId = ++requestIdRef.current;
    setState({ status: "loading", listing: null, error: null });
    const timer = setTimeout(async () => {
      try {
        const listing = await listGithubSkills(repo, path, ref, forceRefresh);
        if (requestIdRef.current !== requestId) return;
        cacheRef.current.set(key, listing);
        setState({ status: "ready", listing, error: null });
      } catch (err) {
        if (requestIdRef.current !== requestId) return;
        setState({
          status: "error",
          listing: null,
          error: err instanceof Error ? err.message : "Could not reach GitHub",
        });
      }
    }, LISTING_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [key, repo, path, ref, forceRefresh]);

  return { ...state, retry: () => setRetryKey(key) };
}

/** One picker row: name, then its repo-relative path as a caption. */
function SkillRow({ entry, trailing }: { entry: GithubSkillEntry; trailing?: string }) {
  return (
    <span className="flex min-w-0 flex-1 items-baseline gap-2">
      <span className="truncate text-body text-text-primary">{entry.name}</span>
      <span className="truncate text-caption text-text-tertiary">
        {trailing ?? (entry.path || "repo root")}
      </span>
    </span>
  );
}

/**
 * What a GitHub source resolves to, under the Source field: a skeleton while
 * the listing runs, a retryable error, one row for a single skill, or a
 * checkbox list with a select-all header for a folder of them.
 */
function GithubSkillPicker({
  state,
  selectedPaths,
  onSelectedPathsChange,
}: {
  state: ListingState & { retry: () => void };
  selectedPaths: string[];
  onSelectedPathsChange: (paths: string[]) => void;
}) {
  if (state.status === "idle") return null;

  if (state.status === "loading") {
    return (
      <div className="flex flex-col gap-2">
        <span className={SECTION_LABEL_CLASS}>Skills</span>
        <div className="h-9 animate-pulse rounded-sm bg-bg-tertiary" />
      </div>
    );
  }

  if (state.status === "error" || !state.listing) {
    return (
      <div className="flex flex-col gap-2">
        <span className={SECTION_LABEL_CLASS}>Skills</span>
        <p className="m-0 flex h-9 items-center gap-2 text-caption text-error">
          {state.error ?? "Could not list this repo's skills"}
          <button
            type="button"
            className="text-caption font-medium text-accent underline"
            onClick={state.retry}
          >
            Retry
          </button>
        </p>
      </div>
    );
  }

  const { skills, truncated } = state.listing;
  const allSelected = selectedPaths.length === skills.length;

  return (
    <div className="flex flex-col gap-2">
      <span className={SECTION_LABEL_CLASS}>Skills</span>

      {skills.length === 0 && (
        <p className="m-0 flex h-9 items-center text-caption text-text-tertiary">
          No SKILL.md found in this repo or path.
        </p>
      )}

      {skills.length === 1 && (
        <div className="flex h-9 items-center gap-2">
          <Folder size={14} className="shrink-0 text-text-tertiary" />
          <SkillRow entry={skills[0]} />
        </div>
      )}

      {skills.length > 1 && (
        <>
          <div className="flex h-9 items-center justify-between gap-2">
            <span className="text-caption text-text-tertiary">{skills.length} skills</span>
            <button
              type="button"
              className="text-caption font-medium text-accent"
              onClick={() =>
                onSelectedPathsChange(allSelected ? [] : skills.map((skill) => skill.path))
              }
            >
              {allSelected ? "Select none" : "Select all"}
            </button>
          </div>
          <ul className="m-0 flex list-none flex-col p-0">
            {skills.map((skill) => (
              <li key={skill.path} className="flex h-9 items-center gap-2">
                <label className="flex min-w-0 flex-1 items-center gap-2">
                  <CheckboxControl
                    checked={selectedPaths.includes(skill.path)}
                    onCheckedChange={(checked) =>
                      onSelectedPathsChange(
                        checked
                          ? [...selectedPaths, skill.path]
                          : selectedPaths.filter((path) => path !== skill.path),
                      )
                    }
                  />
                  <SkillRow entry={skill} />
                </label>
              </li>
            ))}
          </ul>
        </>
      )}

      {truncated && (
        <p className="m-0 text-caption text-text-tertiary">
          Large repo: showing the first {skills.length} skills GitHub returned
        </p>
      )}
    </div>
  );
}

/**
 * The Method segmented control - which choices are enabled comes from
 * `availableMethods(parsed, defaults)`. `noMethodsAvailable` disables every
 * option (a parsed git source with dotagents missing, its only method);
 * `caption` always shows one line - either that unavailability explanation
 * or the picked method's own tooltip text.
 */
function MethodPicker({
  method,
  methods,
  noMethodsAvailable,
  caption,
  onChange,
}: {
  method: SheetMethod;
  methods: SheetMethod[];
  noMethodsAvailable: boolean;
  caption: string;
  onChange: (method: SheetMethod) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      {/* A heading for the method button group, not a form control's
          label - a `<label>` here would have no associated control. */}
      <span className={SECTION_LABEL_CLASS}>Method</span>
      <ToggleGroup
        variant="segmented"
        aria-label="Install method"
        value={[method]}
        onValueChange={(next) => singleSelectToggleValue<SheetMethod>(next, onChange)}
      >
        {sheetMethods().map((m) => {
          const disabled = noMethodsAvailable || (methods.length > 0 && !methods.includes(m));
          return (
            <ToggleGroupItem
              key={m}
              value={m}
              disabled={disabled}
              className="h-[26px] px-3 text-small"
            >
              {METHOD_LABELS[m]}
            </ToggleGroupItem>
          );
        })}
      </ToggleGroup>
      <p className="m-0 text-caption text-text-tertiary">{caption}</p>
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
      <span className={SECTION_LABEL_CLASS}>Scope</span>
      <ScopeToggleGroup scope={scope} onScopeChange={onScopeChange} />
      {scope === "project" && (
        <div className="flex gap-2">
          {userAddedProjects.length > 0 && (
            <div className="flex-1">
              <ProjectDirectorySelect
                projects={userAddedProjects}
                value={projectPath ?? undefined}
                onChange={onProjectPathChange}
              />
            </div>
          )}
          <Button
            variant="outline"
            className="h-(--control-height) gap-2 rounded-md px-3.5 text-body font-medium"
            onClick={onBrowseProject}
          >
            <FolderPlus size={14} />
            {userAddedProjects.length === 0 ? "Choose directory" : "Add"}
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * Whether the manual-add form is valid to submit - the source parses, at
 * least one install method is available (a git source with dotagents
 * missing has none), a project scope has a path, and a listed GitHub source
 * has at least one picked skill folder. The persistent `noMethodsAvailable`
 * gate lives here rather than only in the footer so the button's disabled
 * state, `handleSubmit`'s early return, and tests all read one definition;
 * only the transient `listingBlocks` loading gate is added at the footer.
 */
export function addSkillFormValid(input: {
  parsed: ParsedSkillSource | { error: string };
  noMethodsAvailable: boolean;
  scope: InstallScope;
  projectPath: string | null;
  githubEntries: GithubSkillEntry[] | null;
}): boolean {
  const { parsed, noMethodsAvailable, scope, projectPath, githubEntries } = input;
  return (
    !("error" in parsed) &&
    !noMethodsAvailable &&
    (scope !== "project" || !!projectPath) &&
    (githubEntries === null || githubEntries.length > 0)
  );
}

/**
 * Owns `handleSubmit` and the derived `isValid` flag: both close over the
 * same handful of form fields plus the three callbacks that fire on success,
 * so pulling them out of `AddSkillSheet` keeps that component's body to the
 * dialog shell and its own effects.
 */
function useAddSkillSubmit(input: {
  parsed: ParsedSkillSource | { error: string };
  method: SheetMethod;
  /** `true` when `parsed` is valid but supports no install method at all
   * (a git source with the dotagents CLI absent). Gated here so the submit
   * button matches the picker's own disabled state. */
  noMethodsAvailable: boolean;
  agents: AgentId[];
  disabledHarnesses: AgentId[];
  scope: InstallScope;
  projectPath: string | null;
  trial: boolean;
  /** The picked GitHub skill folders, or `null` for a source that isn't
   * listed (local, git, or a pack import). */
  githubEntries: GithubSkillEntry[] | null;
  dispatch: Dispatch<FormAction>;
  closeSheet: () => void;
  openSkill: (name: string) => void;
  addToast: ReturnType<typeof useAppStore.getState>["addToast"];
}) {
  const {
    parsed,
    method,
    noMethodsAvailable,
    agents,
    disabledHarnesses,
    scope,
    projectPath,
    trial,
    githubEntries,
    dispatch,
    closeSheet,
    openSkill,
    addToast,
  } = input;
  const isValid = addSkillFormValid({
    parsed,
    noMethodsAvailable,
    scope,
    projectPath,
    githubEntries,
  });

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
      const projectArg = scope === "project" ? (projectPath ?? undefined) : undefined;
      if (githubEntries) {
        const outcomes = await addSkills({
          source: parsed,
          skills: githubEntries,
          method,
          agents,
          disabled_harnesses: disabledHarnesses,
          scope,
          project_path: projectArg,
          trial,
        });
        const installed = outcomes.filter((outcome) => outcome.result);
        const failed = outcomes.filter((outcome) => outcome.error);
        if (installed.length === 0) {
          dispatch({
            type: "submit_error",
            error: failed.map((f) => `${f.name}: ${f.error}`).join("; ") || "Nothing was installed",
          });
          return;
        }
        closeSheet();
        addToast({
          type: "success",
          title: `Added ${installed.length} skill${installed.length !== 1 ? "s" : ""}`,
          message: installed
            .map((outcome) => outcome.result?.warning)
            .filter(Boolean)
            .join("; "),
        });
        if (failed.length > 0) {
          addToast({
            type: "error",
            title: `${failed.length} skill${failed.length !== 1 ? "s" : ""} failed`,
            message: failed.map((f) => `${f.name}: ${f.error}`).join("; "),
          });
        }
        if (installed.length === 1) openSkill(installed[0].name);
        dispatch({ type: "submit_end" });
        return;
      }
      const result = await addSkill({
        source: parsed,
        method,
        agents,
        disabled_harnesses: disabledHarnesses,
        scope,
        project_path: projectArg,
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

  return { isValid, handleSubmit };
}

/** The "Add by source" tab's form fields, everything below the Method picker. */
function ManualTabFields({
  method,
  scope,
  projectPath,
  userAddedProjects,
  trial,
  submitError,
  dispatch,
  onBrowseProject,
  installedReaders,
  enabledReaders,
  onReaderEnabledChange,
  claudeReadsShared,
  claudeLink,
  onClaudeLinkChange,
}: {
  method: SheetMethod;
  scope: InstallScope;
  projectPath: string | null;
  userAddedProjects: string[];
  trial: boolean;
  submitError: string | null;
  dispatch: Dispatch<FormAction>;
  onBrowseProject: () => void;
  installedReaders: AgentId[];
  enabledReaders: AgentId[];
  onReaderEnabledChange: (agent: AgentId, enabled: boolean) => void;
  claudeReadsShared: boolean;
  claudeLink: boolean;
  onClaudeLinkChange: (on: boolean) => void;
}) {
  return (
    <>
      {method === "pack" && (
        <p className="m-0 text-caption text-text-tertiary">
          Imports every skill in this repo's pack to the shared folder, plus any agents.toml row
          pointing elsewhere - see the "Packs" section of the docs.
        </p>
      )}

      {method !== "pack" && (
        <ScopePicker
          scope={scope}
          projectPath={projectPath}
          userAddedProjects={userAddedProjects}
          onScopeChange={(next) => dispatch({ type: "set_scope", scope: next })}
          onProjectPathChange={(path) => dispatch({ type: "set_project_path", path })}
          onBrowseProject={onBrowseProject}
        />
      )}

      <AgentTargetSelector
        readers={installedReaders}
        enabledReaders={enabledReaders}
        onReaderEnabledChange={onReaderEnabledChange}
        claudeReadsShared={claudeReadsShared}
        claudeLink={claudeLink}
        onClaudeLinkChange={onClaudeLinkChange}
        scope={method === "pack" ? "global" : scope}
      />

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
    </>
  );
}

/** Cancel/submit footer, shown only on the "Add by source" tab. */
function ManualTabFooter({
  method,
  submitLabel,
  isValid,
  isSubmitting,
  onCancel,
  onSubmit,
}: {
  method: SheetMethod;
  submitLabel: string;
  isValid: boolean;
  isSubmitting: boolean;
  onCancel: () => void;
  onSubmit: () => void;
}) {
  return (
    <div className="flex justify-end gap-2 border-t border-border px-5 py-4">
      <Button
        variant="outline"
        className="h-(--control-height) rounded-md px-3.5 text-body font-medium"
        onClick={onCancel}
        disabled={isSubmitting}
      >
        Cancel
      </Button>
      <Button
        className="h-(--control-height) rounded-md bg-accent px-3.5 text-body font-medium text-text-on-accent hover:bg-accent-hover"
        onClick={onSubmit}
        disabled={!isValid || isSubmitting}
      >
        {isSubmitting ? "Adding…" : method === "pack" ? "Import pack" : submitLabel}
      </Button>
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

  const [form, dispatch] = useReducer(formReducer, undefined, initialFormState);
  const {
    sheetTab,
    source,
    methodChoice,
    enabledReaders: pickedReaders,
    claudeLink,
    scope,
    projectPath,
    trial,
    isSubmitting,
    submitError,
  } = form;

  const sourceInputRef = useRef<HTMLInputElement>(null);

  // What dotagents/skills.sh/the shared folder look like on this machine -
  // fetched once when the sheet opens, so the Method and Harnesses defaults
  // below reflect this machine instead of a generic guess.
  const [defaults, setDefaults] = useState<AddMethodDefaults | null>(null);

  // Reset the form to its defaults, prefilled from the caller, each time the
  // sheet opens - a stale field from a previous open would be confusing.
  useEffect(() => {
    if (!isOpen) return;
    dispatch({ type: "reset", prefill: prefill ?? "", projectPath: userAddedProjects[0] ?? null });
    getAddMethodDefaults()
      .then((next) => {
        setDefaults(next);
        dispatch({
          type: "set_readers",
          readers: next.installed_harnesses.filter((id) => id !== "claude-code"),
        });
      })
      .catch(() => setDefaults(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, prefill]);

  const closeSheet = () => {
    closeAddSkillSheet();
  };

  const parsed = parseSkillSource(source);
  const methods = availableMethods(parsed, defaults);
  const noMethodsAvailable = !("error" in parsed) && methods.length === 0;

  // Keep the selected method valid as the source changes - e.g. switching
  // from a github source to a local path forces "Copy". Derived during
  // render instead of synced back with an effect, since `methods` is itself
  // derived from `source` and `defaults`; falling back to `methods[0]`
  // (each list's preferred choice, first) also re-defaults the method
  // whenever the source's kind changes out from under a user's own pick.
  const method = methods.length > 0 && !methods.includes(methodChoice) ? methods[0] : methodChoice;
  const methodCaption =
    !("error" in parsed) && parsed.kind === "git" && noMethodsAvailable
      ? "Git URLs need dotagents (not installed)"
      : METHOD_TOOLTIPS[method];

  const installedReaders = (defaults?.installed_harnesses ?? []).filter(
    (id) => id !== "claude-code",
  );
  const claudeReadsShared = defaults?.claude_reads_shared_folder ?? false;
  // Every installed reader starts on; `pickedReaders` only holds a choice
  // once the user has made one, so the list survives `defaults` arriving
  // after the sheet opened. Kept in `installedReaders`' order (`AgentId`'s
  // declaration order) rather than the order the switches were flipped in.
  const enabledReaders =
    pickedReaders === null
      ? installedReaders
      : installedReaders.filter((id) => pickedReaders.includes(id));
  const agents = installTargetAgents(enabledReaders, claudeLink);
  const disabledHarnesses = installDisabledHarnesses(
    installedReaders,
    enabledReaders,
    claudeReadsShared,
    claudeLink,
  );

  // A GitHub source is resolved to its actual skill folders before install -
  // a pasted `/tree/.../skills` URL can hold many of them. Pack imports read
  // the repo's own agents.toml instead, so they skip the listing.
  const listingEnabled = !("error" in parsed) && parsed.kind === "github" && method !== "pack";
  const listingState = useGithubSkillListing(parsed, listingEnabled);
  const listedSkills = listingState.listing?.skills ?? [];

  const [selectedPaths, setSelectedPaths] = useState<string[]>([]);
  // Every listed skill starts checked; a new listing resets the selection.
  useEffect(() => {
    setSelectedPaths(listedSkills.map((skill) => skill.path));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listingState.listing]);

  const githubEntries = listingEnabled
    ? listedSkills.filter((skill) => selectedPaths.includes(skill.path))
    : null;

  const { isValid, handleSubmit } = useAddSkillSubmit({
    parsed,
    method,
    noMethodsAvailable,
    agents,
    disabledHarnesses,
    scope,
    projectPath,
    trial,
    githubEntries,
    dispatch,
    closeSheet,
    openSkill,
    addToast,
  });
  const listingBlocks = listingEnabled && listingState.status !== "ready";
  const submitLabel =
    githubEntries && githubEntries.length > 1
      ? `Install ${githubEntries.length} skills`
      : "Add skill";

  const handleBrowseProject = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Select Project" });
    if (selected) {
      addProject(selected);
      dispatch({ type: "set_project_path", path: selected });
    }
  };

  return (
    <Drawer
      open={isOpen}
      onOpenChange={(open) => {
        if (!open && !isSubmitting) closeSheet();
      }}
    >
      <DrawerContent
        side="right"
        className="w-[420px] bg-bg-secondary"
        aria-label="Add skill"
        initialFocus={sourceInputRef}
      >
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <h3 className="m-0 text-balance text-emphasis font-semibold text-text-primary">
            Add skill
          </h3>
        </div>

        <Tabs
          value={sheetTab}
          onValueChange={(tab) => dispatch({ type: "set_tab", tab })}
          className="flex flex-1 flex-col gap-0 overflow-hidden"
        >
          <TabsList variant="line">
            <TabsTrigger value="manual" className={SHEET_TAB_CLASS}>
              Add by source
            </TabsTrigger>
            <TabsTrigger value="browse" className={SHEET_TAB_CLASS}>
              Browse skills.sh
            </TabsTrigger>
          </TabsList>

          <TabsContent value="browse" className="flex flex-1 overflow-hidden">
            <SkillStore compact />
          </TabsContent>

          <TabsContent
            value="manual"
            className="flex flex-1 flex-col gap-5 overflow-y-auto px-5 py-4"
          >
            <SourceField
              source={source}
              parsed={parsed}
              onChange={(value) => dispatch({ type: "set_source", source: value })}
              inputRef={sourceInputRef}
            />

            <GithubSkillPicker
              state={listingState}
              selectedPaths={selectedPaths}
              onSelectedPathsChange={setSelectedPaths}
            />

            <MethodPicker
              method={method}
              methods={methods}
              noMethodsAvailable={noMethodsAvailable}
              caption={methodCaption}
              onChange={(m) => dispatch({ type: "set_method", method: m })}
            />

            <ManualTabFields
              method={method}
              scope={scope}
              projectPath={projectPath}
              userAddedProjects={userAddedProjects}
              trial={trial}
              submitError={submitError}
              dispatch={dispatch}
              onBrowseProject={handleBrowseProject}
              installedReaders={installedReaders}
              enabledReaders={enabledReaders}
              onReaderEnabledChange={(agent, enabled) =>
                dispatch({ type: "set_reader_enabled", agent, enabled })
              }
              claudeReadsShared={claudeReadsShared}
              claudeLink={claudeLink}
              onClaudeLinkChange={(on) => dispatch({ type: "set_claude_link", claudeLink: on })}
            />
          </TabsContent>
        </Tabs>

        {sheetTab === "manual" && (
          <ManualTabFooter
            method={method}
            submitLabel={submitLabel}
            isValid={isValid && !listingBlocks}
            isSubmitting={isSubmitting}
            onCancel={closeSheet}
            onSubmit={handleSubmit}
          />
        )}
      </DrawerContent>
    </Drawer>
  );
}
