// ============================================================================
// UniversalVisibilitySelector - which harnesses can see a new Universal
// deployment. The Universal folder (~/.agents/skills) is one canonical
// deployment; the installed harnesses that read it hang under it, each
// with its own off switch (pi, Cursor and Grok Build have no per-skill
// disable, so theirs is checked and inert). Claude Code sits beside the
// Universal block because it needs its own per-skill symlink.
// ============================================================================

import { Folder } from "lucide-react";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { TooltipControl } from "../ui/TooltipControl";
import {
  harnessHasPerSkillDisable,
  UNIVERSAL_VISIBILITY_HARNESSES,
} from "./universal-install-visibility";
import type { AgentId, InstallScope } from "@skill-studio/lib";

const HARNESS_LABEL = new Map<AgentId, string>(UNIVERSAL_VISIBILITY_HARNESSES);

const NO_SWITCH_CAPTION = "Always reads the Universal folder; no per-skill off switch";

const ROW_CLASS = "grid h-9 items-center gap-2 rounded-sm px-2 hover:bg-bg-hover";

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
          {hasSwitch ? "Reads the Universal folder" : NO_SWITCH_CAPTION}
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

interface UniversalVisibilitySelectorProps {
  /** Installed harnesses that read the Universal folder (Claude Code excluded). */
  readers: readonly AgentId[];
  /** The subset of `readers` still switched on. */
  enabledReaders: readonly AgentId[];
  onReaderEnabledChange: (agent: AgentId, enabled: boolean) => void;
  /** True when ~/.claude/skills is the whole-directory symlink to the Universal folder. */
  claudeReadsShared: boolean;
  claudeLink: boolean;
  onClaudeLinkChange: (on: boolean) => void;
  /** Only changes the folder paths shown in the captions. */
  scope?: InstallScope;
  disabled?: boolean;
}

/** Selects visibility for one canonical Universal deployment. */
export function UniversalVisibilitySelector({
  readers,
  enabledReaders,
  onReaderEnabledChange,
  claudeReadsShared,
  claudeLink,
  onClaudeLinkChange,
  scope = "global",
  disabled = false,
}: UniversalVisibilitySelectorProps) {
  const enabledReaderSet = new Set(enabledReaders);
  const folderPath = scope === "global" ? "~/.agents/skills" : ".agents/skills";
  const claudePath = scope === "global" ? "~/.claude/skills" : ".claude/skills";
  // The whole-dir symlink covers every skill at once, so the backend refuses
  // to remove Claude Code's link for one skill - see `set_claude_code_enabled`.
  const claudeSwitchDisabled = disabled || claudeReadsShared;
  const claudeCaption = claudeReadsShared
    ? `${claudePath} already links to the Universal folder`
    : claudeLink
      ? `Symlink in ${claudePath}`
      : "Not linked, Claude Code will not see it";

  return (
    <div className="flex flex-col gap-2">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Visibility
      </span>

      <div className="-mx-2 flex flex-col">
        <div className={`${ROW_CLASS} grid-cols-[16px_minmax(0,1fr)]`}>
          <Folder size={16} className="text-text-tertiary" />
          <span className="flex min-w-0 flex-col">
            <span className="truncate text-body text-text-primary">Universal folder</span>
            <span className="truncate font-mono text-caption text-text-tertiary">{folderPath}</span>
          </span>
        </div>

        {readers.map((id) => (
          <ReaderRow
            key={id}
            agent={id}
            enabled={enabledReaderSet.has(id)}
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
