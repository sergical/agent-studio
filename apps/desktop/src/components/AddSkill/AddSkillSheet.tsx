// ============================================================================
// AddSkillSheet - Right-side sheet for adding a skill from a source string:
// parses the Source field live, lists the skill folders a GitHub source
// actually holds (one skill, or a picker for a folder of them), offers a
// Method and Destination controls, Universal visibility, a
// Global/Project Scope, and an optional "Try for 24 hours" trial. Submits to
// a background Add Skill operation (`start_add_skill_operation` /
// `start_add_skills_operation`) so `npx` never runs on the UI thread.
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
import { UniversalVisibilitySelector } from "../SkillStore/UniversalVisibilitySelector";
import {
  universalDisabledHarnesses,
  universalInstallHarnesses,
} from "../SkillStore/universal-install-visibility";
import { ProjectDirectorySelect } from "../SkillStore/ProjectDirectorySelect";
import { ScopeToggleGroup } from "../SkillStore/ScopeToggleGroup";
import { SkillStore } from "../SkillStore/SkillStore";
import { SkillDestinationSelector } from "../SkillStore/SkillDestinationSelector";
import { CheckboxControl } from "../ui/CheckboxControl";
import { availableAddSkillMethods, isAddSkillFormValid } from "./add-skill-form";
import type { AddSkillSheetMethod } from "./add-skill-form";
import {
  abandonPackImportTrust,
  cancelAddSkillOperation,
  confirmAddSkillTrust,
  confirmSkillPackTrust,
  getAddMethodDefaults,
  getAddSkillOperation,
  importSkillPack,
  listGithubSkills,
  onAddSkillOperation,
  startAddSkillOperation,
  startAddSkillsOperation,
} from "../../lib/skill-api";
import {
  applyAddSkillOperationEvent,
  listenForAddSkillOperation,
} from "../../hooks/useAddSkillOperation";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import {
  addSkillFinishAction,
  addSkillOperationProgressCopy,
  addSkillOperationTerminalCopy,
  isAddSkillOperationCancellable,
  isAddSkillOperationTerminal,
  normalizeInstallHarnesses,
  parseSkillSource,
  shouldConsumeAddSkillOperation,
  toWireParsedSkillSource,
  trialSelectionForDestination,
} from "@skill-studio/lib";
import type {
  AddSkillOperationEvent,
  PackImportRequest,
  ParsedSkillSource,
} from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { useAppStore } from "../../store/appStore";
import type {
  AddMethod,
  AddMethodDefaults,
  AgentId,
  GithubSkillEntry,
  GithubSkillListing,
  InstallScope,
  PerHarnessDestinationId,
  SkillDestination,
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
type SheetMethod = AddSkillSheetMethod;
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
  dotagents: "Tracked in agents.toml. Installs to Universal and updates with dotagents.",
  "skills-sh": "Tracked in .skill-lock.json. Installs to Universal.",
  copy: "Untracked. Supports Universal or independent Per harness copies.",
  pack: "Imports every skill in this share pack to Universal.",
} satisfies Record<SheetMethod, string>;

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
  /** The installed readers of the Universal folder left switched on. `null`
   * until `getAddMethodDefaults` answers, which is what seeds it. */
  enabledReaders: AgentId[] | null;
  /** Whether Claude Code gets a link to the Universal deployment. */
  claudeLink: boolean;
  destination: SkillDestination;
  perHarnesses: PerHarnessDestinationId[];
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
    destination: "universal",
    perHarnesses: [],
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
  | { type: "set_destination"; destination: SkillDestination }
  | { type: "set_per_harness"; harness: PerHarnessDestinationId; enabled: boolean }
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
    case "set_destination":
      return {
        ...state,
        destination: action.destination,
        trial: trialSelectionForDestination(action.destination, state.trial),
      };
    case "set_per_harness":
      return {
        ...state,
        perHarnesses: action.enabled
          ? [...state.perHarnesses, action.harness]
          : state.perHarnesses.filter((id) => id !== action.harness),
      };
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

interface ListingResult {
  requestKey: string;
  state: ListingState;
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

  const [result, setResult] = useState<ListingResult | null>(null);
  const [retryKey, setRetryKey] = useState<string | null>(null);
  const [listingCache, setListingCache] = useState(() => new Map<string, GithubSkillListing>());
  const requestIdRef = useRef(0);
  const forceRefresh = key !== null && retryKey === key;
  const requestKey = key ? `${key}|${forceRefresh ? "refresh" : "cached"}` : null;
  const cached = key && !forceRefresh ? listingCache.get(key) : undefined;
  const state: ListingState =
    !key || !repo
      ? IDLE_LISTING
      : cached
        ? { status: "ready", listing: cached, error: null }
        : result?.requestKey === requestKey
          ? result.state
          : { status: "loading", listing: null, error: null };

  useEffect(() => {
    if (!key || !repo || !requestKey || cached) return;
    const requestId = ++requestIdRef.current;
    const timer = setTimeout(async () => {
      try {
        const listing = await listGithubSkills(repo, path, ref, forceRefresh);
        if (requestIdRef.current !== requestId) return;
        setListingCache((current) => new Map(current).set(key, listing));
        setResult({
          requestKey,
          state: { status: "ready", listing, error: null },
        });
      } catch (err) {
        if (requestIdRef.current !== requestId) return;
        setResult({
          requestKey,
          state: {
            status: "error",
            listing: null,
            error: err instanceof Error ? err.message : "Could not reach GitHub",
          },
        });
      }
    }, LISTING_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [key, repo, path, ref, forceRefresh, requestKey, cached]);

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
          <Button variant="link" className="h-auto p-0 text-caption" onClick={state.retry}>
            Retry
          </Button>
        </p>
      </div>
    );
  }

  const { skills, truncated } = state.listing;
  const allSelected = selectedPaths.length === skills.length;
  const selectedPathSet = new Set(selectedPaths);

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
            <Button
              variant="link"
              className="h-auto p-0 text-caption font-medium"
              onClick={() =>
                onSelectedPathsChange(allSelected ? [] : skills.map((skill) => skill.path))
              }
            >
              {allSelected ? "Select none" : "Select all"}
            </Button>
          </div>
          <ul className="m-0 flex list-none flex-col p-0">
            {skills.map((skill) => (
              <li key={skill.path} className="flex h-9 items-center gap-2">
                <label className="flex min-w-0 flex-1 items-center gap-2">
                  <CheckboxControl
                    checked={selectedPathSet.has(skill.path)}
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
 * `availableAddSkillMethods(parsed, defaults)`. `noMethodsAvailable` disables every
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
  const methodSet = new Set(methods);
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
          const disabled = noMethodsAvailable || (methods.length > 0 && !methodSet.has(m));
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

function applyFinishAction(
  status: AddSkillOperationEvent,
  closeSheet: () => void,
  openSkill: (name: string) => void,
  addToast: ReturnType<typeof useAppStore.getState>["addToast"],
  dispatch: Dispatch<FormAction>,
) {
  const action = addSkillFinishAction(status);
  if (action.kind === "error") {
    dispatch({ type: "submit_error", error: action.error });
    return;
  }
  closeSheet();
  addToast({
    type: action.message ? "warning" : "success",
    title: action.title,
    message: action.message,
  });
  if (action.failedTitle) {
    addToast({
      type: "error",
      title: action.failedTitle,
      message: action.failedMessage,
    });
  }
  if (action.openName) openSkill(action.openName);
  dispatch({ type: "submit_end" });
}

/**
 * Owns `handleSubmit` and the derived `isValid` flag. Add Skill listens
 * first, then starts the background operation so queued progress shows
 * before any `npx` work.
 */
function useAddSkillSubmit(input: {
  parsed: ParsedSkillSource | { error: string };
  method: SheetMethod;
  noMethodsAvailable: boolean;
  destination: SkillDestination;
  agents: AgentId[];
  disabledHarnesses: AgentId[];
  scope: InstallScope;
  projectPath: string | null;
  trial: boolean;
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
    destination,
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
  const [operation, setOperation] = useState<AddSkillOperationEvent | undefined>(undefined);
  const [packTrust, setPackTrust] = useState<
    { identities: string[]; confirmationToken: string; requestKey: string } | undefined
  >(undefined);
  const [trustBusy, setTrustBusy] = useState(false);
  const operationIdRef = useRef<string | undefined>(undefined);
  const consumedIdRef = useRef<string | undefined>(undefined);
  const unlistenRef = useRef<(() => void) | undefined>(undefined);
  const packTrustTokenRef = useRef<string | undefined>(undefined);
  const isValid = isAddSkillFormValid({
    parsed,
    noMethodsAvailable,
    destination,
    agents,
    scope,
    projectPath,
    trial,
    githubEntries,
  });

  const packSource =
    "error" in parsed
      ? undefined
      : parsed.kind === "github"
        ? parsed.repo
        : parsed.kind === "local"
          ? parsed.localPath
          : undefined;
  const packRequest: PackImportRequest | undefined =
    method === "pack" && packSource
      ? {
          source: packSource,
          agents,
          method: "pack",
          destination: "universal",
          scope: "global",
          project_path: null,
        }
      : undefined;
  const packRequestKey = packRequest ? JSON.stringify(packRequest) : undefined;
  const activePackTrust = packTrust?.requestKey === packRequestKey ? packTrust : undefined;

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
      const token = packTrustTokenRef.current;
      packTrustTokenRef.current = undefined;
      if (token) void abandonPackImportTrust(token).catch(() => undefined);
    };
  }, []);

  const claimPackTrustToken = (expected?: string) => {
    const token = packTrustTokenRef.current;
    if (!token || (expected && token !== expected)) return undefined;
    packTrustTokenRef.current = undefined;
    return token;
  };

  const abandonActivePackTrust = () => {
    const token = claimPackTrustToken();
    setPackTrust(undefined);
    if (token) void abandonPackImportTrust(token).catch(() => undefined);
  };

  useEffect(() => {
    if (!operation || !shouldConsumeAddSkillOperation(operation, consumedIdRef.current)) return;
    consumedIdRef.current = operation.operation_id;
    applyFinishAction(operation, closeSheet, openSkill, addToast, dispatch);
    operationIdRef.current = undefined;
  }, [addToast, closeSheet, dispatch, openSkill, operation]);

  const handleSubmit = async () => {
    if ("error" in parsed || !isValid || operationIdRef.current) return;
    if (method === "pack" && !packRequest) {
      dispatch({ type: "submit_error", error: "Pack import needs a repository or local folder" });
      return;
    }
    dispatch({ type: "submit_start" });
    try {
      if (method === "pack") {
        const preflight = await importSkillPack(packRequest!);
        if (preflight.status === "needs-trust") {
          abandonActivePackTrust();
          packTrustTokenRef.current = preflight.confirmation_token;
          setPackTrust({
            identities: preflight.identities,
            confirmationToken: preflight.confirmation_token,
            requestKey: packRequestKey!,
          });
          dispatch({ type: "submit_end" });
          return;
        }
        const result = preflight.result;
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
      const projectArg = scope === "project" ? (projectPath ?? null) : null;
      const operationId = crypto.randomUUID();
      operationIdRef.current = operationId;
      consumedIdRef.current = undefined;
      setOperation({
        operation_id: operationId,
        sequence: 0,
        phase: "queued",
        message: "Waiting to add skill",
      });
      unlistenRef.current?.();
      const unlisten = await listenForAddSkillOperation({
        isCancelled: () => false,
        listen: onAddSkillOperation,
        onEvent: (incoming) => {
          const trackedId = operationIdRef.current;
          setOperation((current) => applyAddSkillOperationEvent(current, incoming, trackedId));
        },
      });
      unlistenRef.current = unlisten;
      const queued = githubEntries
        ? await startAddSkillsOperation(operationId, {
            source: toWireParsedSkillSource(parsed),
            skills: githubEntries,
            method,
            destination,
            agents,
            disabled_harnesses: disabledHarnesses,
            scope,
            project_path: projectArg,
            trial,
          })
        : await startAddSkillOperation(operationId, {
            source: toWireParsedSkillSource(parsed),
            method,
            destination,
            agents,
            disabled_harnesses: disabledHarnesses,
            scope,
            project_path: projectArg,
            trial,
          });
      setOperation((current) => applyAddSkillOperationEvent(current, queued, operationId));
      const snapshot = await getAddSkillOperation(operationId);
      setOperation((current) => applyAddSkillOperationEvent(current, snapshot, operationId));
    } catch (err) {
      unlistenRef.current?.();
      unlistenRef.current = undefined;
      operationIdRef.current = undefined;
      dispatch({
        type: "submit_error",
        error: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const handleCancelOperation = async () => {
    if (activePackTrust) {
      abandonActivePackTrust();
      dispatch({ type: "submit_end" });
      closeSheet();
      return;
    }
    const operationId = operationIdRef.current;
    if (operationId && operation?.phase === "needs-trust") {
      unlistenRef.current?.();
      unlistenRef.current = undefined;
      operationIdRef.current = undefined;
      setOperation(undefined);
      dispatch({ type: "submit_end" });
      closeSheet();
      try {
        await cancelAddSkillOperation(operationId);
      } catch (error) {
        addToast({
          type: "error",
          title: "Could not decline repository trust",
          message: error instanceof Error ? error.message : "Unknown error",
        });
      }
      return;
    }
    if (!operationId || !operation || !isAddSkillOperationCancellable(operation.phase)) {
      closeSheet();
      return;
    }
    try {
      const next = await cancelAddSkillOperation(operationId);
      setOperation((current) => applyAddSkillOperationEvent(current, next, operationId));
    } catch (error) {
      dispatch({
        type: "submit_error",
        error: error instanceof Error ? error.message : "Could not cancel",
      });
    }
  };

  const handleTrustAndRetry = async () => {
    if (activePackTrust) {
      if (!packRequest || trustBusy) return;
      const confirmationToken = claimPackTrustToken(activePackTrust.confirmationToken);
      if (!confirmationToken) return;
      setTrustBusy(true);
      try {
        const result = await confirmSkillPackTrust(confirmationToken, packRequest);
        setPackTrust(undefined);
        closeSheet();
        const total = result.bundled.length + result.referenced.length;
        addToast(
          result.errors.length > 0
            ? {
                type: "warning",
                title: `Imported ${total} skill${total !== 1 ? "s" : ""}`,
                message: result.errors.join("; "),
              }
            : {
                type: "success",
                title: `Imported ${total} skill${total !== 1 ? "s" : ""}`,
              },
        );
      } catch (error) {
        void abandonPackImportTrust(confirmationToken).catch(() => undefined);
        setPackTrust(undefined);
        dispatch({
          type: "submit_error",
          error: error instanceof Error ? error.message : "Pack trust confirmation failed",
        });
      }
      setTrustBusy(false);
      return;
    }
    const operationId = operationIdRef.current;
    const identity = operation?.untrusted_source?.identity;
    if (!operationId || !identity || trustBusy) return;
    setTrustBusy(true);
    try {
      const retryOperationId = crypto.randomUUID();
      const retry = await confirmAddSkillTrust(operationId, retryOperationId, identity);
      operationIdRef.current = retry.operation_id;
      consumedIdRef.current = undefined;
      setOperation(retry);
      const snapshot = await getAddSkillOperation(retry.operation_id);
      setOperation((current) => applyAddSkillOperationEvent(current, snapshot, retry.operation_id));
    } catch (error) {
      dispatch({
        type: "submit_error",
        error: error instanceof Error ? error.message : "Trust confirmation failed",
      });
    }
    setTrustBusy(false);
  };

  return {
    isValid,
    handleSubmit,
    handleCancelOperation,
    handleTrustAndRetry,
    operation,
    packTrust: activePackTrust,
    trustBusy,
  };
}

/** The "Add by source" tab's form fields, everything below the Method picker. */
function ManualTabFields({
  method,
  destination,
  scope,
  projectPath,
  userAddedProjects,
  trial,
  submitError,
  dispatch,
  onBrowseProject,
  installedReaders,
  enabledReaders,
  perHarnesses,
  onReaderEnabledChange,
  claudeReadsShared,
  claudeLink,
  onClaudeLinkChange,
  onDestinationChange,
  onHarnessChange,
}: {
  method: SheetMethod;
  destination: SkillDestination;
  scope: InstallScope;
  projectPath: string | null;
  userAddedProjects: string[];
  trial: boolean;
  submitError: string | null;
  dispatch: Dispatch<FormAction>;
  onBrowseProject: () => void;
  installedReaders: AgentId[];
  enabledReaders: AgentId[];
  perHarnesses: PerHarnessDestinationId[];
  onReaderEnabledChange: (agent: AgentId, enabled: boolean) => void;
  claudeReadsShared: boolean;
  claudeLink: boolean;
  onClaudeLinkChange: (on: boolean) => void;
  onDestinationChange: (destination: SkillDestination) => void;
  onHarnessChange: (harness: PerHarnessDestinationId, enabled: boolean) => void;
}) {
  return (
    <>
      {method === "pack" && (
        <p className="m-0 text-caption text-text-tertiary">
          Imports every skill in this repo's pack to the Universal folder, plus any agents.toml row
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

      <SkillDestinationSelector
        destination={destination}
        harnesses={destination === "universal" ? [] : perHarnesses}
        scope={method === "pack" ? "global" : scope}
        onDestinationChange={onDestinationChange}
        onHarnessChange={onHarnessChange}
        perHarnessDisabledReason={
          method === "copy"
            ? undefined
            : method === "pack"
              ? "Pack imports deploy to Universal."
              : `${METHOD_LABELS[method]} installs to Universal. Choose Copy for Per harness.`
        }
      />

      {destination === "universal" && (
        <UniversalVisibilitySelector
          readers={installedReaders}
          enabledReaders={enabledReaders}
          onReaderEnabledChange={onReaderEnabledChange}
          claudeReadsShared={claudeReadsShared}
          claudeLink={claudeLink}
          onClaudeLinkChange={onClaudeLinkChange}
          scope={method === "pack" ? "global" : scope}
        />
      )}

      {method !== "pack" && (
        <div className="flex flex-col gap-2">
          <label className="flex items-center gap-2 text-body text-text-primary">
            <CheckboxControl
              checked={trial}
              disabled={destination === "per-harness"}
              onCheckedChange={(next) => dispatch({ type: "set_trial", trial: next })}
            />
            Try for 24 hours
          </label>
          <p className="m-0 text-caption text-text-tertiary">
            {destination === "per-harness"
              ? "Trials are available only for Universal installs."
              : "Removed automatically after 24 h unless you keep it."}
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
  operation,
  packTrust,
  trustBusy,
  onCancel,
  onSubmit,
  onTrustAndRetry,
}: {
  method: SheetMethod;
  submitLabel: string;
  isValid: boolean;
  isSubmitting: boolean;
  operation: AddSkillOperationEvent | undefined;
  packTrust: { identities: string[]; confirmationToken: string; requestKey: string } | undefined;
  trustBusy: boolean;
  onCancel: () => void;
  onSubmit: () => void;
  onTrustAndRetry: () => void;
}) {
  if (packTrust || operation?.phase === "needs-trust") {
    const identities = packTrust
      ? packTrust.identities
      : [operation?.untrusted_source?.identity ?? "this repository"];
    const isPackTrust = !!packTrust;
    return (
      <div className="flex flex-col gap-3 border-t border-border px-5 py-4">
        <div>
          <p className="m-0 text-body font-medium text-text-primary">
            Trust {identities.length === 1 ? "this repository" : "these repositories"}?
          </p>
          <ul className="m-0 mt-1 list-inside list-disc text-small text-text-secondary">
            {identities.map((identity) => (
              <li key={identity}>{identity}</li>
            ))}
          </ul>
          <p className="m-0 mt-2 text-caption text-text-tertiary">
            Skills from this source can run on your machine. Confirm only if you trust it.
          </p>
        </div>
        <div className="flex justify-end gap-2">
          <Button
            variant="outline"
            className="h-(--control-height) rounded-md px-3.5 text-body font-medium"
            onClick={onCancel}
            disabled={trustBusy}
          >
            Close
          </Button>
          <Button
            className="h-(--control-height) rounded-md bg-accent px-3.5 text-body font-medium text-text-on-accent hover:bg-accent-hover"
            onClick={onTrustAndRetry}
            disabled={trustBusy}
          >
            {isPackTrust
              ? `Trust ${identities.length === 1 ? "repository" : "repositories"} and import`
              : "Trust repository and retry"}
          </Button>
        </div>
      </div>
    );
  }

  const inProgress = isSubmitting && operation && !isAddSkillOperationTerminal(operation.phase);
  const terminalFailure =
    operation &&
    (operation.phase === "failed" ||
      operation.phase === "cancelled" ||
      operation.phase === "timed-out");
  return (
    <div className="flex flex-col gap-2 border-t border-border px-5 py-4">
      {inProgress && (
        <p className="m-0 text-caption text-text-tertiary">
          {addSkillOperationProgressCopy(operation)}
        </p>
      )}
      {terminalFailure && (
        <p className="m-0 text-small text-error" role="alert">
          {addSkillOperationTerminalCopy(operation)}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button
          variant="outline"
          className="h-(--control-height) rounded-md px-3.5 text-body font-medium"
          onClick={onCancel}
        >
          Cancel
        </Button>
        <Button
          className="h-(--control-height) rounded-md bg-accent px-3.5 text-body font-medium text-text-on-accent hover:bg-accent-hover"
          onClick={onSubmit}
          disabled={!isValid || isSubmitting}
        >
          {inProgress ? "Adding…" : method === "pack" ? "Import pack" : submitLabel}
        </Button>
      </div>
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
    destination,
    perHarnesses,
    scope,
    projectPath,
    trial,
    isSubmitting,
    submitError,
  } = form;

  const sourceInputRef = useRef<HTMLInputElement>(null);

  // What dotagents/skills.sh/the Universal folder look like on this machine -
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
  const sourceMethods = availableAddSkillMethods(parsed, defaults);
  const methods =
    destination === "per-harness"
      ? sourceMethods.filter((candidate) => candidate === "copy")
      : sourceMethods;
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
  const pickedReaderSet = new Set(pickedReaders ?? []);
  const enabledReaders =
    pickedReaders === null
      ? installedReaders
      : installedReaders.filter((id) => pickedReaderSet.has(id));
  const agents = normalizeInstallHarnesses(
    destination,
    destination === "universal"
      ? universalInstallHarnesses(enabledReaders, claudeLink)
      : perHarnesses,
  );
  const disabledHarnesses =
    destination === "universal"
      ? universalDisabledHarnesses(installedReaders, enabledReaders, claudeReadsShared, claudeLink)
      : [];

  // A GitHub source is resolved to its actual skill folders before install -
  // a pasted `/tree/.../skills` URL can hold many of them. Pack imports read
  // the repo's own agents.toml instead, so they skip the listing.
  const listingEnabled = !("error" in parsed) && parsed.kind === "github" && method !== "pack";
  const listingState = useGithubSkillListing(parsed, listingEnabled);
  const listedSkills = listingState.listing?.skills ?? [];

  const [selection, setSelection] = useState<{
    listing: GithubSkillListing | null;
    paths: string[];
  }>({ listing: null, paths: [] });
  // Every new listing starts checked without synchronizing derived state in an effect.
  const selectedPaths =
    selection.listing === listingState.listing
      ? selection.paths
      : listedSkills.map((skill) => skill.path);
  const selectedPathSet = new Set(selectedPaths);

  const githubEntries = listingEnabled
    ? listedSkills.filter((skill) => selectedPathSet.has(skill.path))
    : null;

  const {
    isValid,
    handleSubmit,
    handleCancelOperation,
    handleTrustAndRetry,
    operation,
    packTrust,
    trustBusy,
  } = useAddSkillSubmit({
    parsed,
    method,
    noMethodsAvailable,
    destination,
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
        if (
          !open &&
          !(isSubmitting && operation && isAddSkillOperationCancellable(operation.phase))
        ) {
          void handleCancelOperation();
        }
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
              onSelectedPathsChange={(paths) =>
                setSelection({ listing: listingState.listing, paths })
              }
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
              destination={destination}
              scope={scope}
              projectPath={projectPath}
              userAddedProjects={userAddedProjects}
              trial={trial}
              submitError={
                operation &&
                (operation.phase === "failed" ||
                  operation.phase === "cancelled" ||
                  operation.phase === "timed-out")
                  ? null
                  : submitError
              }
              dispatch={dispatch}
              onBrowseProject={handleBrowseProject}
              installedReaders={installedReaders}
              enabledReaders={enabledReaders}
              perHarnesses={perHarnesses}
              onReaderEnabledChange={(agent, enabled) =>
                dispatch({ type: "set_reader_enabled", agent, enabled })
              }
              claudeReadsShared={claudeReadsShared}
              claudeLink={claudeLink}
              onClaudeLinkChange={(on) => dispatch({ type: "set_claude_link", claudeLink: on })}
              onDestinationChange={(next) =>
                dispatch({ type: "set_destination", destination: next })
              }
              onHarnessChange={(harness, enabled) =>
                dispatch({ type: "set_per_harness", harness, enabled })
              }
            />
          </TabsContent>
        </Tabs>

        {sheetTab === "manual" && (
          <ManualTabFooter
            method={method}
            submitLabel={submitLabel}
            isValid={isValid && !listingBlocks}
            isSubmitting={isSubmitting}
            operation={operation}
            packTrust={packTrust}
            trustBusy={trustBusy}
            onCancel={handleCancelOperation}
            onSubmit={handleSubmit}
            onTrustAndRetry={handleTrustAndRetry}
          />
        )}
      </DrawerContent>
    </Drawer>
  );
}
