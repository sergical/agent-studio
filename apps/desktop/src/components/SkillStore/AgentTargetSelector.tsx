// ============================================================================
// AgentTargetSelector - where a new skill goes, drawn the way the skill
// page's Locations card draws where an installed skill lives. Every install
// method writes the shared folder (~/.agents/skills), so that row is a fact
// with no switch; the installed harnesses that read it hang under it, each
// with its own off switch (pi, Cursor and Grok Build have no per-skill
// disable, so theirs is checked and inert). Claude Code sits beside the
// shared block because it needs its own per-skill symlink.
// Shared by AddSkillSheet and the store's InstallControls.
// ============================================================================

import { Folder } from "lucide-react";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { TooltipControl } from "../ui/TooltipControl";
import type { AgentId, InstallScope } from "@skill-studio/lib";

export const INSTALL_TARGET_HARNESSES = [
  ["claude-code", "Claude Code"],
  ["codex", "Codex"],
  ["open-code", "OpenCode"],
  ["pi", "pi"],
  ["cursor", "Cursor"],
  ["grok-build", "Grok Build"],
] as const satisfies readonly (readonly [AgentId, string])[];

const HARNESS_LABEL = new Map<AgentId, string>(INSTALL_TARGET_HARNESSES);

/** Harnesses that read the shared folder and have no per-skill off switch
 * in any of their own config files - see `set_harness_enabled_with`. */
const NO_PER_SKILL_DISABLE: readonly AgentId[] = ["pi", "cursor", "grok-build"];

const NO_SWITCH_CAPTION = "Reads the shared folder directly, no per-skill off switch";

const ROW_CLASS = "grid h-9 items-center gap-2 rounded-sm px-2 hover:bg-bg-hover";

export function harnessHasPerSkillDisable(agent: AgentId): boolean {
  return !NO_PER_SKILL_DISABLE.includes(agent);
}

/**
 * The agents the backend receives for an install: the readers left switched
 * on, in the order they were given (`AgentId`'s declaration order), plus
 * Claude Code when its per-skill link is wanted. An empty result is passed
 * through untouched - the backend decides what an install with no target
 * means.
 */
export function installTargetAgents(
  enabledReaders: readonly AgentId[],
  claudeLink: boolean,
): AgentId[] {
  return claudeLink ? [...enabledReaders, "claude-code"] : [...enabledReaders];
}

/**
 * The harnesses the backend must switch off after the install, because the
 * install itself cannot avoid reaching them: a reader that was toggled off
 * still sees the shared folder, and Claude Code still sees it through a
 * whole-directory symlink no per-skill choice can undo.
 */
export function installDisabledHarnesses(
  readers: readonly AgentId[],
  enabledReaders: readonly AgentId[],
  claudeReadsShared: boolean,
  claudeLink: boolean,
): AgentId[] {
  const disabled = readers.filter((id) => !enabledReaders.includes(id));
  if (claudeReadsShared && !claudeLink) disabled.push("claude-code");
  return disabled;
}

/** One installed reader under the shared-folder row. */
function ReaderRow({
  agent,
  enabled,
  onEnabledChange,
  disabled,
}: {
  agent: AgentId;
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  disabled: boolean;
}) {
  const label = HARNESS_LABEL.get(agent) ?? agent;
  const hasSwitch = harnessHasPerSkillDisable(agent);
  const row = (
    <div className={`${ROW_CLASS} grid-cols-[20px_16px_minmax(0,1fr)_auto]`}>
      <span aria-hidden="true" />
      <HarnessIcon harness={agent} size={16} />
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-body text-text-primary">{label}</span>
        <span className="truncate text-caption text-text-tertiary">
          {hasSwitch ? "Reads the shared folder" : NO_SWITCH_CAPTION}
        </span>
      </span>
      <SwitchControl
        checked={!hasSwitch || enabled}
        onCheckedChange={onEnabledChange}
        disabled={disabled || !hasSwitch}
        ariaLabel={`Enable for ${label}`}
      />
    </div>
  );
  return hasSwitch ? row : <TooltipControl content={NO_SWITCH_CAPTION}>{row}</TooltipControl>;
}

interface AgentTargetSelectorProps {
  /** Installed harnesses that read the shared folder (Claude Code excluded). */
  readers: readonly AgentId[];
  /** The subset of `readers` still switched on. */
  enabledReaders: readonly AgentId[];
  onReaderEnabledChange: (agent: AgentId, enabled: boolean) => void;
  /** True when ~/.claude/skills is the whole-directory symlink to the shared folder. */
  claudeReadsShared: boolean;
  claudeLink: boolean;
  onClaudeLinkChange: (on: boolean) => void;
  /** Only changes the folder paths shown in the captions. */
  scope?: InstallScope;
  disabled?: boolean;
}

export function AgentTargetSelector({
  readers,
  enabledReaders,
  onReaderEnabledChange,
  claudeReadsShared,
  claudeLink,
  onClaudeLinkChange,
  scope = "global",
  disabled = false,
}: AgentTargetSelectorProps) {
  const folderPath = scope === "global" ? "~/.agents/skills" : ".agents/skills";
  const claudePath = scope === "global" ? "~/.claude/skills" : ".claude/skills";
  // The whole-dir symlink covers every skill at once, so the backend refuses
  // to remove Claude Code's link for one skill - see `set_claude_code_enabled`.
  const claudeSwitchDisabled = disabled || claudeReadsShared;
  const claudeCaption = claudeReadsShared
    ? `${claudePath} already links to the shared folder`
    : claudeLink
      ? `Symlink in ${claudePath}`
      : "Not linked, Claude Code will not see it";

  return (
    <div className="flex flex-col gap-2">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Location
      </span>

      <div className="-mx-2 flex flex-col">
        <div className={`${ROW_CLASS} grid-cols-[16px_minmax(0,1fr)]`}>
          <Folder size={16} className="text-text-tertiary" />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-body text-text-primary">Shared folder</span>
            <span className="truncate font-mono text-caption text-text-tertiary">{folderPath}</span>
          </span>
        </div>

        {readers.map((id) => (
          <ReaderRow
            key={id}
            agent={id}
            enabled={enabledReaders.includes(id)}
            onEnabledChange={(next) => onReaderEnabledChange(id, next)}
            disabled={disabled}
          />
        ))}

        <div className={`${ROW_CLASS} grid-cols-[16px_minmax(0,1fr)_auto]`}>
          <HarnessIcon harness="claude-code" size={16} />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-body text-text-primary">Claude Code</span>
            <span className="truncate text-caption text-text-tertiary">{claudeCaption}</span>
          </span>
          <SwitchControl
            checked={claudeReadsShared ? true : claudeLink}
            onCheckedChange={onClaudeLinkChange}
            disabled={claudeSwitchDisabled}
            ariaLabel="Symlink for Claude Code"
          />
        </div>
      </div>
    </div>
  );
}
