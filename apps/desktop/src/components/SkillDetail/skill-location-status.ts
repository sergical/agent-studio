// ============================================================================
// skill-location-status - pure status/model logic for the Locations card,
// ported from apps/desktop/prototypes/locations/prototype.js's "Flat +
// footer" variant against real `Deployment`/`InstalledSkill` data. See
// status-spec.md for the severity ladder, rollup rule, and exact copy this
// follows: one severity dot on the identity icon, facts-only chips, and a
// fixed two-line tooltip shape (what / fix / mono path).
// ============================================================================

import {
  agentIdFromDeploymentLabel,
  describeSpecViolations,
  deploymentLinkKind,
  driftingCopies,
  homeRelativePath,
  isBlockingSpecViolation,
  locationSummary,
  parentDirectory,
} from "@skill-studio/lib";
import type {
  AgentId,
  Deployment,
  InstalledSkill,
  InvocationPolicy,
  LifecycleTarget,
} from "@skill-studio/lib";
import type { TooltipLine } from "../ui/TooltipControl";

export type StatusLevel = "error" | "warning" | "off";

/** `rollup`/`skillRollup`'s result: the one dot a folder or the whole skill shows, and its tooltip body. */
export interface RollupResult {
  level: StatusLevel | null;
  tip: string;
}

const RANK = { error: 3, warning: 2, off: 1 } satisfies Record<StatusLevel, number>;

/** The two readers with a per-skill off switch in their own config - see `skill_harness_disable.rs`. */
const READERS_WITH_A_SWITCH: AgentId[] = ["codex", "open-code"];

/** Every action a Locations row's ⋯ menu (or switch) can trigger - handled by `useLocationActions`. */
export type LocationAction =
  | { kind: "relink"; deployment: Deployment }
  | { kind: "remove-link"; deployment: Deployment }
  | { kind: "edit-skill-md"; path: string }
  | { kind: "reveal"; path: string; label: string }
  | { kind: "open-editor"; path: string; label: string }
  | { kind: "compare" }
  | { kind: "convert-root"; target: LifecycleTarget; harness: AgentId; root: string }
  | { kind: "set-enabled"; deployment: Deployment; enabled: boolean }
  | { kind: "set-reader-enabled"; target: LifecycleTarget; agent: AgentId; enabled: boolean }
  | { kind: "park" }
  | { kind: "unpark" }
  | { kind: "remove-scope"; scopeLabel: string; projectPath: string | null }
  | { kind: "remove-deployment"; scopeLabel: string; deployment: Deployment }
  | { kind: "update" }
  | { kind: "install-again" }
  | { kind: "remove-lock-entry" }
  | { kind: "promote-global"; source: string; agents: AgentId[] };

export interface MenuEntry {
  label: string;
  action: LocationAction;
  danger?: boolean;
}

/** One condition a row (or the folder/skill it rolls up into) is in - see status-spec.md §3. */
export interface Condition {
  level: StatusLevel;
  /** Stack-glyph/tooltip status word, e.g. "Broken link", "Off". */
  status: string;
  /** Counted-rollup phrase, singular - "1 <phrase> inside:". */
  phrase: string;
  /** Counted-rollup phrase, plural - "N <plural> inside:". */
  plural: string;
  /** Tooltip line 1. */
  what: string;
  /** Tooltip line 2, omitted when the menu's first item already says it. */
  fix?: string;
  /** Tooltip's mono path/target line, when there is one. */
  path?: string;
  /** Takes over the rollup tooltip's first two lines verbatim instead of the counted summary - parked-but-live only. */
  headline?: boolean;
  menu: MenuEntry[];
  /** A hint line shown under the menu's first item. */
  hint?: string;
}

export type LocationKind = "shared" | "link" | "copy" | "plugin" | "reader";

interface BaseLocationRow {
  harnessLabel: string;
  path: string;
  /** Relationship word only - status words never live here, see status-spec.md §1. */
  caption: string;
  conditions: Condition[];
  level: StatusLevel | null;
  /** `null` for a synthesized reader row - there is no deployment of its own to act on. */
  deployment: Deployment | null;
  /** Exact deployment used by lifecycle actions, including synthesized reader rows. */
  lifecycleTarget: LifecycleTarget;
  hasSwitch: boolean;
  switchOn: boolean;
  chip: "always on" | null;
  invocation: InvocationPolicy | null;
}

/** The shared-folder row - its `harness` is the literal `"shared"`, never a real `AgentId`. */
export interface SharedLocationRow extends BaseLocationRow {
  kind: "shared";
  harness: "shared";
}

/** A per-skill link/copy/plugin row, or a synthesized reader row - always a real `AgentId`. */
export interface AgentLocationRow extends BaseLocationRow {
  kind: "link" | "copy" | "plugin" | "reader";
  harness: AgentId;
}

/** One row on the flat card: the Universal folder, a per-skill link, a copy, a plugin, or a synthesized always-reads-the-folder reader. */
export type LocationRow = SharedLocationRow | AgentLocationRow;

/** One scope block on the card: Global (folds in plugin and parked deployments), or one project. */
export interface ScopeGroup {
  label: string;
  isGlobal: boolean;
  projectPath?: string;
  shared: LocationRow | null;
  rows: LocationRow[];
  folderLevel: StatusLevel | null;
  folderTip: string;
  parkedScope: boolean;
}

/** One row of the Invocation footer - a Universal folder or a copy/plugin, each with its own SKILL.md. */
export interface InvocationFile {
  scopeLabel: string;
  kind: "shared" | "copy" | "plugin";
  harness: AgentId | "shared";
  name: string;
  path: string;
  level: StatusLevel | null;
  tip: string;
  chip: "plugin" | null;
  invocation: InvocationPolicy;
  /** False for a plugin file, a managed project Universal folder, or a managed copy - see `fileEditability`. */
  editable: boolean;
  /** Set alongside `editable: false` - why the segmented control is disabled. */
  disabledReason?: string;
  caption: string;
  deployment: Deployment;
}

/**
 * `buildInvocationFiles`'s per-file editable/disabledReason call: a file's
 * provenance is its own `plugin` field when set, otherwise the whole skill's
 * `source_kind` (there is no finer-grained per-deployment provenance -
 * see skill-list-filter.ts's "'plugin' reaches outside a skill's own
 * source_kind" note). The global Universal folder is always editable - a
 * managed one forks first, the same rule the SKILL.md editor uses - but a
 * managed project Universal folder or a managed copy is not: editing it in
 * place would be overwritten on the next sync/update.
 */
function fileEditability(
  kind: "shared" | "copy" | "plugin",
  isGlobal: boolean,
  skill: InstalledSkill,
  deployment: Deployment,
): Pick<InvocationFile, "editable" | "disabledReason"> {
  if (kind === "plugin") {
    return {
      editable: false,
      disabledReason: `Managed by ${deployment.plugin?.name ?? "a plugin"}; changes would be overwritten on update`,
    };
  }
  if (kind === "shared" && isGlobal) return { editable: true };
  const managedSource =
    skill.source_kind === "dotagents"
      ? "dotagents"
      : skill.source_kind === "skills-sh"
        ? "skills.sh"
        : null;
  if (!managedSource) return { editable: true };
  return {
    editable: false,
    disabledReason: `Managed by ${managedSource}; changes would be overwritten on update`,
  };
}

const harnessLabelFromAgent = (agent: string): string =>
  agent === "shared" ? "Universal folder" : agent;

/** Display label for a synthesized reader row - `deployment.agent` is already a display label, but a reader has no deployment, only its machine `AgentId`. */
function readerLabel(agent: AgentId): string {
  switch (agent) {
    case "codex":
      return "Codex";
    case "open-code":
      return "OpenCode";
    case "pi":
      return "pi";
    case "cursor":
      return "Cursor";
    case "grok-build":
      return "Grok Build";
    default:
      return agent;
  }
}

function harnessId(agent: string): AgentId | null {
  const id = agentIdFromDeploymentLabel(agent);
  return id === "shared" || id === null ? null : id;
}

/** True when a skill parked globally still has a live copy or Universal folder outside the parked one. */
export function liveElsewhere(skill: InstalledSkill): boolean {
  return skill.parked && skill.deployments.some((d) => d.scope !== "parked" && !d.disabled);
}

/** "Missing description", "Missing name", etc, folded into the fixed sentence shapes from status-spec.md §5. */
function specCondition(violations: string[], path: string): Condition {
  const blocking = violations.some(isBlockingSpecViolation);
  const sentence = describeSpecViolations(violations);
  const editAndReveal: MenuEntry[] = [
    { label: "Edit SKILL.md", action: { kind: "edit-skill-md", path } },
    { label: "Reveal in Finder", action: { kind: "reveal", path, label: "the copy" } },
  ];
  return blocking
    ? {
        level: "error",
        status: "Won't load",
        phrase: "won't load",
        plural: "won't load",
        what: `SKILL.md will not load: ${sentence}`,
        fix: "Edit SKILL.md.",
        menu: editAndReveal,
      }
    : {
        level: "warning",
        status: "To check",
        phrase: "to check",
        plural: "to check",
        what: `SKILL.md: ${sentence} The skill still loads.`,
        fix: "Edit SKILL.md.",
        menu: [editAndReveal[0]],
      };
}

/** Off for one harness deployment - which mechanism `disabled_by` names decides the sentence and the fix. */
function offCondition(deployment: Deployment): Condition {
  const label = harnessLabelFromAgent(deployment.agent);
  const base = { level: "off" as const, status: "Off", phrase: "off", plural: "off" };
  const enable = (hint: string, what: string): Condition => ({
    ...base,
    what,
    fix: "Use the switch to turn it on.",
    menu: [
      { label: `Enable for ${label}`, action: { kind: "set-enabled", deployment, enabled: true } },
    ],
    hint,
  });
  switch (deployment.disabled_by) {
    case "codex-config":
      return enable(
        "Turns it back on in Codex's config.toml.",
        "Off for Codex — switched off in ~/.codex/config.toml.",
      );
    case "opencode-permission":
      return enable(
        "Allows it again in opencode.json.",
        "Off for OpenCode — denied in opencode.json.",
      );
    case "claude-link-removed":
      return enable(
        "Restores the link in ~/.claude/skills.",
        "Off for Claude Code — the link under ~/.claude/skills was removed.",
      );
    case "studio-moved":
    default: {
      const parent = homeRelativePath(parentDirectory(deployment.path));
      return {
        ...base,
        what: `Off for ${label} — moved into .skill-studio-disabled.`,
        fix: "Use the switch to move it back.",
        menu: [
          {
            label: `Enable for ${label}`,
            action: { kind: "set-enabled", deployment, enabled: true },
          },
        ],
        hint: `Moves it back into ${parent}.`,
      };
    }
  }
}

/** Off for a synthesized reader row - Codex/OpenCode are the only readers with their own switch. */
function readerOffCondition(agent: AgentId, target: LifecycleTarget): Condition {
  const base = { level: "off" as const, status: "Off", phrase: "off", plural: "off" };
  const action: LocationAction = { kind: "set-reader-enabled", target, agent, enabled: true };
  return agent === "codex"
    ? {
        ...base,
        what: "Off for Codex — switched off in ~/.codex/config.toml.",
        fix: "Use the switch to turn it on.",
        menu: [{ label: "Enable for Codex", action }],
        hint: "Turns it back on in Codex's config.toml.",
      }
    : {
        ...base,
        what: "Off for OpenCode — denied in opencode.json.",
        fix: "Use the switch to turn it on.",
        menu: [{ label: "Enable for OpenCode", action }],
        hint: "Allows it again in opencode.json.",
      };
}

/** Off because the folder that carries this row is parked - the folder's own switch is the fix, not this row's. */
function offBecauseParked(label: string, liveWhere: boolean): Condition {
  return {
    level: "off",
    status: "Off",
    phrase: "off",
    plural: "off",
    what: `Off for ${label} — the folder is parked.`,
    fix: "Use the folder switch to unpark.",
    menu: liveWhere ? [] : [{ label: "Enable everywhere", action: { kind: "unpark" } }],
  };
}

interface GroupContext {
  parkedScope: boolean;
  anyShared: boolean;
  live: boolean;
  otherScopeLabel: string | undefined;
  driftSet: Set<Deployment>;
}

function driftCondition(deployment: Deployment, ctx: GroupContext): Condition {
  const what = ctx.anyShared
    ? "This copy differs from the Universal folder."
    : `This copy differs from the copy in ${ctx.otherScopeLabel ?? "another scope"}.`;
  const label = harnessLabelFromAgent(deployment.agent);
  return {
    level: "warning",
    status: "Differs",
    phrase: "differs",
    plural: "differ",
    what,
    fix: "Compare copies to see the changes.",
    menu: [
      {
        label: ctx.anyShared ? "Compare with the Universal folder…" : "Compare copies…",
        action: { kind: "compare" },
      },
      { label: "Reveal in Finder", action: { kind: "reveal", path: deployment.path, label } },
      // "Delete copy" is deliberately omitted: there is no path-level delete IPC.
    ],
  };
}

/** Conditions for one harness deployment - a per-skill link, a copy, or a plugin (never the Universal folder itself). */
function deploymentConditions(deployment: Deployment, ctx: GroupContext): Condition[] {
  const out: Condition[] = [];
  const label = harnessLabelFromAgent(deployment.agent);

  if (deployment.symlink_is_broken) {
    out.push({
      level: "error",
      status: "Broken link",
      phrase: "broken link",
      plural: "broken links",
      what: "Broken link. The target is missing.",
      fix: "Relink to the folder or remove the link.",
      path: `→ ${homeRelativePath(deployment.symlink_target ?? deployment.path)}`,
      menu: [
        { label: "Relink to the folder", action: { kind: "relink", deployment } },
        { label: "Remove broken link", action: { kind: "remove-link", deployment }, danger: true },
        { label: "Reveal in Finder", action: { kind: "reveal", path: deployment.path, label } },
      ],
    });
  } else if (deployment.symlink_error) {
    out.push({
      level: "error",
      status: "Link unreadable",
      phrase: "unreadable link",
      plural: "unreadable links",
      what: `Link cannot be read: ${deployment.symlink_error}.`,
      fix: "Remove the link or fix permissions.",
      path: `→ ${homeRelativePath(deployment.path)}`,
      menu: [
        { label: "Remove link", action: { kind: "remove-link", deployment }, danger: true },
        { label: "Reveal in Finder", action: { kind: "reveal", path: deployment.path, label } },
      ],
    });
  }

  const violations = deployment.spec_violations ?? [];
  if (violations.length > 0) out.push(specCondition(violations, deployment.path));

  if (ctx.driftSet.has(deployment)) out.push(driftCondition(deployment, ctx));

  if (ctx.parkedScope) {
    out.push(offBecauseParked(label, ctx.live));
  } else if (deployment.disabled) {
    out.push(offCondition(deployment));
  }

  return out.sort((a, b) => RANK[b.level] - RANK[a.level]);
}

function sharedConditions(shared: Deployment, ctx: GroupContext): Condition[] {
  const out: Condition[] = [];
  if (ctx.parkedScope) {
    if (ctx.live) {
      out.push({
        level: "error",
        status: "Parked but live",
        phrase: "parked but live",
        plural: "parked but live",
        headline: true,
        what: "Parked, but a sync created a live folder.",
        fix: "Unpark to reconcile.",
        menu: [{ label: "Unpark", action: { kind: "unpark" } }],
      });
    }
    out.push({
      level: "off",
      status: "Off everywhere",
      phrase: "off",
      plural: "off",
      headline: true,
      what: "Off everywhere — the folder is parked in ~/.agents/skills-parked.",
      fix: "Use the switch to unpark.",
      menu: [
        ...(ctx.live ? [] : [{ label: "Enable everywhere", action: { kind: "unpark" as const } }]),
        {
          label: "Reveal in Finder",
          action: { kind: "reveal", path: shared.path, label: "the Universal folder" },
        },
      ],
    });
  }
  const violations = shared.spec_violations ?? [];
  if (violations.length > 0) out.push(specCondition(violations, shared.path));
  return out.sort((a, b) => RANK[b.level] - RANK[a.level]);
}

function topLevel(conditions: Condition[]): StatusLevel | null {
  return conditions.length ? conditions[0].level : null;
}

/** The icon/dot tooltip: what's wrong, the fix, then any child sentences, then the mono path. */
export function rowTipLines(conditions: Condition[]): string[] {
  if (!conditions.length) return [];
  const [top, ...rest] = conditions;
  return [top.what, top.fix, ...rest.map((c) => c.what), top.path].filter((line): line is string =>
    Boolean(line),
  );
}

/** `rowTipLines`, as `TooltipControl`'s line shape - the mono path line (starts with "→ ") renders in `font-mono`. */
export function tipLines(conditions: Condition[]): TooltipLine[] {
  return rowTipLines(conditions).map((line) =>
    line.startsWith("→ ") ? { text: line, mono: true } : line,
  );
}

/** Any `\n`-joined tooltip body (a folder/skill rollup's `tip`, an `InvocationFile.tip`) as `TooltipControl`'s line shape - the mono path line (starts with "→ ") renders in `font-mono`. */
export function toTooltipLines(tip: string): TooltipLine[] {
  return tip
    .split("\n")
    .filter(Boolean)
    .map((line) => (line.startsWith("→ ") ? { text: line, mono: true } : line));
}

/** Groups `deployments` into scope blocks (Global folds in plugin/parked deployments, then one block per project). */
function scopeKeyOf(d: Deployment): string {
  return d.scope === "project" && d.project_path ? d.project_path : "";
}

function scopeLabelOf(key: string): string {
  if (key === "") return "Global";
  const basename = key.split("/").filter(Boolean).pop() ?? key;
  return `Project · ${basename}`;
}

/**
 * Builds every scope block for `skill`'s Locations card: the Universal folder
 * (when it has one), harness/copy/plugin rows, synthesized reader rows for
 * agents that read the Universal folder natively with no deployment of their
 * own, and each row's/folder's dot and tooltip.
 */
export function buildScopeGroups(skill: InstalledSkill): ScopeGroup[] {
  const byKey = new Map<string, Deployment[]>();
  for (const d of skill.deployments) {
    const key = scopeKeyOf(d);
    const list = byKey.get(key) ?? [];
    list.push(d);
    byKey.set(key, list);
  }

  const summary = locationSummary(skill);
  const driftSet = new Set(driftingCopies(summary));
  const live = liveElsewhere(skill);
  const keys = [...byKey.keys()].sort((a, b) =>
    a === "" ? -1 : b === "" ? 1 : a.localeCompare(b),
  );

  return keys.map((key) => {
    const deployments = byKey.get(key) ?? [];
    const isGlobal = key === "";
    const parkedScope = isGlobal && skill.parked;
    const sharedDeployment =
      deployments.find((d) => deploymentLinkKind(d) === "shared-root") ?? null;
    const otherKey = keys.find((k) => k !== key);
    const ctx: GroupContext = {
      parkedScope,
      anyShared: summary.truth != null,
      live,
      otherScopeLabel: otherKey !== undefined ? scopeLabelOf(otherKey) : undefined,
      driftSet,
    };

    const shared: LocationRow | null = sharedDeployment
      ? (() => {
          const conditions = sharedConditions(sharedDeployment, ctx);
          return {
            kind: "shared",
            harness: "shared",
            harnessLabel: "Universal folder",
            path: sharedDeployment.path,
            caption: "",
            conditions,
            level: topLevel(conditions),
            deployment: sharedDeployment,
            lifecycleTarget: { deployment_id: sharedDeployment.id },
            hasSwitch: true,
            switchOn: !sharedDeployment.disabled && !parkedScope,
            chip: null,
            invocation: sharedDeployment.invocation ?? skill.invocation,
          };
        })()
      : null;

    const restDeployments = deployments.filter((d) => d !== sharedDeployment);
    const rows: LocationRow[] = restDeployments.map((d) => {
      const conditions = deploymentConditions(d, ctx);
      // A harness whose whole skills dir links to the shared root reads the
      // folder like any per-skill link; its switch converts the root on demand.
      const readsFolder = d.is_symlink || d.shared_via_whole_dir_link;
      const kind: "plugin" | "link" | "copy" = d.plugin ? "plugin" : readsFolder ? "link" : "copy";
      const caption = d.plugin
        ? `${d.plugin.name}${d.plugin.version ? ` v${d.plugin.version}` : ""}`
        : "";
      return {
        kind,
        // SAFETY: every deployment Skill Studio scans comes from a
        // first-class agent id or the Universal folder; `d.agent` falls
        // outside `AgentId` only for a harness label `agentIdFromDeploymentLabel`
        // doesn't recognize, which the scanner never produces today.
        harness: (harnessId(d.agent) ?? d.agent) as AgentId,
        harnessLabel: harnessLabelFromAgent(d.agent),
        path: d.path,
        caption,
        conditions,
        level: topLevel(conditions),
        deployment: d,
        lifecycleTarget: { deployment_id: d.id },
        hasSwitch: !d.symlink_is_broken && kind !== "plugin",
        switchOn: !d.disabled && !parkedScope,
        chip: null,
        invocation: d.invocation ?? skill.invocation,
      };
    });

    if (shared) {
      const covered = new Set(rows.map((r) => r.harness));
      const disabledReaders = new Set(sharedDeployment?.disabled_readers ?? []);
      for (const agent of AGENTS_READING_SHARED_ROOT_ORDER) {
        if (covered.has(agent)) continue;
        const disabledForReader = disabledReaders.has(agent);
        const hasSwitch = READERS_WITH_A_SWITCH.includes(agent);
        const conditions: Condition[] = parkedScope
          ? [offBecauseParked(readerLabel(agent), live)]
          : disabledForReader && hasSwitch
            ? [readerOffCondition(agent, shared.lifecycleTarget)]
            : [];
        rows.push({
          kind: "reader",
          harness: agent,
          harnessLabel: readerLabel(agent),
          path: shared.path,
          caption: "",
          conditions,
          level: topLevel(conditions),
          deployment: null,
          lifecycleTarget: shared.lifecycleTarget,
          hasSwitch,
          switchOn: !disabledForReader && !parkedScope,
          chip: hasSwitch ? null : "always on",
          invocation: null,
        });
      }
    }

    // A harness with its own entry carries its own dot, so it never rolls
    // into the folder - only the shared row and synthesized reader rows do.
    const readers = rows.filter((r) => r.kind === "reader");
    const entries: LabeledCondition[] = [
      ...(shared?.conditions.map((c) => ({ condition: c, label: "" })) ?? []),
      ...readers.flatMap((r) => r.conditions.map((c) => ({ condition: c, label: r.harnessLabel }))),
    ];
    // Only rows with a switch of their own count toward "every row off" - an
    // always-on reader (no switch to flip) can't keep the folder out of the
    // all-off rollup on its own, see status-spec.md's "all-off" fixture.
    const switchableRows = readers.filter((r) => r.hasSwitch);
    const allOff =
      (shared != null && !shared.switchOn) ||
      parkedScope ||
      (switchableRows.length > 0 &&
        switchableRows.every((r) => r.conditions.some((c) => c.level === "off")));
    const { level: folderLevel, tip: folderTip } = rollup(entries, allOff);

    return {
      label: scopeLabelOf(key),
      isGlobal,
      projectPath: key === "" ? undefined : key,
      shared,
      rows,
      folderLevel,
      folderTip,
      parkedScope,
    };
  });
}

/** Rows that hang inside the folder accordion: only readers with no entry of their own. */
export function folderReaders(group: ScopeGroup): LocationRow[] {
  return group.rows.filter((row) => row.kind === "reader");
}
/** Rows beside the accordion: every harness with its own filesystem entry. */
export function siblingRows(group: ScopeGroup): LocationRow[] {
  return group.rows.filter((row) => row.kind !== "reader");
}

/** `AGENTS_READING_SHARED_ROOT`, minus Grok Build - it has no row-level condition of its own worth synthesizing today. Kept in its documented order. */
const AGENTS_READING_SHARED_ROOT_ORDER: AgentId[] = [
  "codex",
  "open-code",
  "pi",
  "cursor",
  "grok-build",
];

interface LabeledCondition {
  condition: Condition;
  label: string;
}

/** "1 broken link", "2 broken links", "2 errors" - the rollup's counted first line, see status-spec.md §2. */
function counted(sorted: LabeledCondition[]): string {
  const errors = sorted.filter((e) => e.condition.level === "error");
  const warnings = sorted.filter((e) => e.condition.level === "warning");
  if (!errors.length && !warnings.length) return "Off everywhere:";
  const one = (list: LabeledCondition[], word: string): string => {
    const phrases = new Set(list.map((e) => e.condition.phrase));
    if (phrases.size === 1) {
      return list.length === 1
        ? `1 ${list[0].condition.phrase}`
        : `${list.length} ${list[0].condition.plural}`;
    }
    return `${list.length} ${word}${list.length > 1 ? "s" : ""}`;
  };
  const summary =
    errors.length && warnings.length
      ? `${errors.length} error${errors.length > 1 ? "s" : ""}, ${warnings.length} warning${warnings.length > 1 ? "s" : ""}`
      : errors.length
        ? one(errors, "error")
        : one(warnings, "warning");
  return `${summary} inside:`;
}

/** Rolls `entries` up into one dot + tooltip for a folder or the whole skill - see status-spec.md §2. */
function rollup(entries: LabeledCondition[], allOff: boolean): RollupResult {
  const sorted = [...entries].sort((a, b) => RANK[b.condition.level] - RANK[a.condition.level]);
  const active = sorted.filter((e) => e.condition.level !== "off");
  if (!active.length && !allOff) return { level: null, tip: "" };
  const level = active.length ? active[0].condition.level : "off";
  const head = sorted.find((e) => e.condition.headline);
  const lines: string[] = [];
  if (head) lines.push(head.condition.what, head.condition.fix ?? "");
  else lines.push(counted(sorted));
  for (const e of sorted) {
    if (e === head) continue;
    lines.push(e.label ? `${e.label}: ${e.condition.what}` : e.condition.what);
  }
  return { level, tip: lines.filter(Boolean).join("\n") };
}

/** The whole card's rollup, for the skill-page header/sidebar dot. Lock-only (no deployments at all) short-circuits to its own fixed condition. */
export function skillRollup(skill: InstalledSkill, groups: ScopeGroup[]): RollupResult {
  if (skill.deployments.length === 0) {
    return {
      level: "warning",
      tip: "Listed in the lock file, but no folder was found.\nInstall again or remove the entry.",
    };
  }
  const entries: LabeledCondition[] = groups.flatMap((g) => {
    const shared: LabeledCondition[] =
      g.shared?.conditions.map((c) => ({ condition: c, label: g.label })) ?? [];
    const rows: LabeledCondition[] = g.rows.flatMap((r) =>
      r.conditions.map((c) => ({ condition: c, label: `${g.label} · ${r.harnessLabel}` })),
    );
    return [...shared, ...rows];
  });
  const allOff =
    groups.length > 0 && groups.every((g) => g.folderLevel === "off" || g.folderLevel === null);
  return rollup(entries, allOff);
}

/** The card title's one right-aligned action link, precedence per status-spec.md §2: unpark > compare > install-again > enable-everywhere > update. */
export function titleLink(
  skill: InstalledSkill,
  hasDrift: boolean,
): "Unpark" | "Compare copies" | "Install again" | "Enable everywhere" | "Update" | null {
  if (liveElsewhere(skill)) return "Unpark";
  if (hasDrift) return "Compare copies";
  if (skill.deployments.length === 0) return "Install again";
  if (skill.parked) return "Enable everywhere";
  if (skill.update_owner_ids.length > 0) return "Update";
  return null;
}

/** `promoteToGlobal`'s result: the project folder to copy into `~/.agents/skills`, and the harnesses that need a link of their own. */
export interface PromoteSource {
  path: string;
  agents: AgentId[];
}

/**
 * The offer behind the card's "Promote to global" link: a skill that lives in
 * two or more projects and nowhere global has one folder worth copying to
 * `~/.agents/skills`, from where every shared-root reader picks it up. Only
 * Claude Code needs a link of its own, so that is the only harness in
 * `agents`.
 */
export function promoteToGlobal(groups: ScopeGroup[]): PromoteSource | null {
  if (groups.some((g) => g.isGlobal)) return null;
  if (groups.length < 2) return null;
  const source = groups
    .map((g) => g.shared ?? g.rows.find((r) => r.kind === "copy"))
    .find((row) => row != null);
  if (!source) return null;
  const agents: AgentId[] = groups.some((g) => g.rows.some((r) => r.harness === "claude-code"))
    ? ["claude-code"]
    : [];
  return { path: source.path, agents };
}

/** The Invocation footer's rows: the Universal folder (if any) plus every copy/plugin - links read the shared file, so they never get one of their own. */
export function buildInvocationFiles(
  groups: ScopeGroup[],
  skill: InstalledSkill,
): InvocationFile[] {
  const files: InvocationFile[] = [];
  for (const group of groups) {
    const shared = group.shared;
    if (shared?.deployment) {
      files.push({
        scopeLabel: group.label,
        kind: "shared",
        harness: "shared",
        name: `${group.label} folder`,
        path: shared.path,
        level: shared.level,
        tip: rowTipLines(shared.conditions).join("\n"),
        chip: null,
        invocation: shared.invocation ?? "both",
        ...fileEditability("shared", group.isGlobal, skill, shared.deployment),
        caption: "",
        deployment: shared.deployment,
      });
    }
    for (const row of group.rows) {
      if (row.kind !== "copy" && row.kind !== "plugin") continue;
      if (!row.deployment) continue;
      const isPlugin = row.kind === "plugin";
      const codexNote =
        row.harness === "codex" && row.invocation !== "both"
          ? `openai.yaml: implicit invocation ${row.invocation === "user-only" ? "off" : "only"}`
          : "";
      files.push({
        scopeLabel: group.label,
        kind: row.kind,
        harness: row.harness,
        name: `${group.label} · ${row.harnessLabel} ${isPlugin ? "plugin" : "copy"}`,
        path: row.path,
        level: row.level,
        tip: rowTipLines(row.conditions).join("\n"),
        chip: isPlugin ? "plugin" : null,
        invocation: row.invocation ?? "both",
        ...fileEditability(row.kind, group.isGlobal, skill, row.deployment),
        caption: codexNote,
        deployment: row.deployment,
      });
    }
  }
  return files;
}

/** The footer's single note line, from status-spec.md §5: one file explains its own value; several files just point at "each file sets its own". */
export function invocationFooterNote(files: InvocationFile[], skillName: string): string {
  if (files.length > 1) return "Each file sets its own. Symlinks follow the folder they point to.";
  if (files.length !== 1) return "";
  switch (files[0].invocation) {
    case "both":
      return `Both: you can call /${skillName} and the model can pick it.`;
    case "user-only":
      return `User only: only /${skillName} starts it.`;
    case "model-only":
      return `Model only: the model picks it; there is no /${skillName} command.`;
  }
}

/** `rowMenu`'s result: the plain entries, the danger entries (rendered after a separator), and an optional hint line. */
export interface RowMenuResult {
  entries: MenuEntry[];
  danger: MenuEntry[];
  hint?: string;
}

/** A row's ⋯ menu: fixes first (highest condition first, its own first item may lead even when destructive), then the row-kind's own actions, danger items after a separator, then a hint line - see prototype.js's `rowMenu`. */
export function rowMenu(
  row: LocationRow,
  scopeLabel: string,
  projectPath: string | null = null,
): RowMenuResult {
  const plain: MenuEntry[] = [];
  const danger: MenuEntry[] = [];
  const seen = new Set<string>();
  const push = (entry: MenuEntry, lead: boolean) => {
    if (seen.has(entry.label)) return;
    seen.add(entry.label);
    (entry.danger && !lead ? danger : plain).push(entry);
  };
  row.conditions.forEach((condition, i) => {
    condition.menu.forEach((entry, j) => push(entry, i === 0 && j === 0));
  });
  const hasOff = row.conditions.some((c) => c.level === "off");

  if (row.kind === "shared") {
    push(
      {
        label: "Reveal in Finder",
        action: { kind: "reveal", path: row.path, label: "the Universal folder" },
      },
      false,
    );
    if (!hasOff) push({ label: "Park (Disable everywhere)", action: { kind: "park" } }, false);
    push(
      {
        label: `Remove from ${scopeLabel}…`,
        action: { kind: "remove-scope", scopeLabel, projectPath },
        danger: true,
      },
      false,
    );
  } else if (row.kind === "reader") {
    if (row.hasSwitch && !hasOff) {
      push(
        {
          label: `Disable for ${row.harnessLabel}`,
          action: {
            kind: "set-reader-enabled",
            target: row.lifecycleTarget,
            agent: row.harness,
            enabled: false,
          },
        },
        false,
      );
    }
    push(
      {
        label: "Reveal Universal folder in Finder",
        action: { kind: "reveal", path: row.path, label: "the Universal folder" },
      },
      false,
    );
  } else if (row.kind === "plugin") {
    push(
      {
        label: "Reveal in Finder",
        action: { kind: "reveal", path: row.path, label: row.harnessLabel },
      },
      false,
    );
    push(
      {
        label: "Open in your editor",
        action: { kind: "open-editor", path: row.path, label: row.harnessLabel },
      },
      false,
    );
  } else {
    if (row.hasSwitch && !hasOff && row.deployment) {
      push(
        {
          label: `Disable for ${row.harnessLabel}`,
          action: { kind: "set-enabled", deployment: row.deployment, enabled: false },
        },
        false,
      );
    }
    push(
      {
        label: "Reveal in Finder",
        action: { kind: "reveal", path: row.path, label: row.harnessLabel },
      },
      false,
    );
    const isBroken = row.deployment?.symlink_is_broken || row.deployment?.symlink_error != null;
    const isRootLink = row.deployment?.shared_via_whole_dir_link ?? false;
    if (isRootLink && row.deployment) {
      push(
        {
          label: "Convert to per-skill links…",
          action: {
            kind: "convert-root",
            target: row.lifecycleTarget,
            harness: row.harness,
            root: parentDirectory(row.deployment.path),
          },
        },
        false,
      );
    }
    if (row.kind === "link" && !isBroken && !isRootLink && row.deployment) {
      push(
        {
          label: "Remove link",
          action: { kind: "remove-link", deployment: row.deployment },
          danger: true,
        },
        false,
      );
    }
    if (row.kind === "copy" && row.deployment?.owner_kind === "copy") {
      push(
        {
          label: `Remove ${row.harnessLabel} copy…`,
          action: { kind: "remove-deployment", scopeLabel, deployment: row.deployment },
          danger: true,
        },
        false,
      );
    }
  }

  let hint = row.conditions.find((c) => c.hint)?.hint;
  if (row.kind === "reader" && !row.hasSwitch)
    hint = `Always on. ${row.harnessLabel} has no per-skill switch.`;
  if (row.kind === "plugin" && row.deployment?.plugin) {
    hint = `Managed by the ${row.deployment.plugin.name} plugin. Disable it in ${row.harnessLabel}.`;
  }
  if (row.kind === "reader" && row.hasSwitch && !hasOff)
    hint = `Sets it off in ${row.harnessLabel}'s own config.`;

  return { entries: plain, danger, hint };
}
