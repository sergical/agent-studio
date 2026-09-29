// ============================================================================
// InstallHarnessSelector - the Destination section of an install: which
// harnesses get the skill, Link or Copy into their own folders, and a one-line
// preview of the folders it writes. The shared `.agents/skills` folder is
// always written, as in the `skills` CLI's own picker.
// ============================================================================

import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import {
  installFoldersPreview,
  installHarness,
  installHarnessLocked,
  linkModeChoiceVisible,
  toggleInstallHarness,
  universalDestinationPath,
} from "@skill-studio/lib";
import type { AgentId, InstallLinkMode, InstallScope } from "@skill-studio/lib";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import { CheckboxControl } from "../ui/CheckboxControl";
import { HarnessIcon } from "../ui/HarnessIcon";

const ROW_CLASS =
  "grid h-9 grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 hover:bg-bg-hover";

const OPTION_CLASS = "h-[26px] px-3 text-small";

function harnessCaption(
  id: AgentId,
  chosen: boolean,
  linkMode: InstallLinkMode,
  claudeReadsShared: boolean,
  scope: InstallScope,
): string {
  const harness = installHarness(id);
  const folder = harness?.ownFolder?.[scope];
  if (id === "claude-code" && claudeReadsShared) {
    return `Reads the shared folder through ${folder}`;
  }
  if (installHarnessLocked(id, claudeReadsShared, scope)) {
    return `Always reads ${universalDestinationPath(scope)}`;
  }
  if (chosen) {
    if (!folder) return "Reads the shared folder";
    return `${linkMode === "copy" ? "Copy" : "Link"} in ${folder}`;
  }
  if (harness?.hasOffSwitch) return "Turned off for this skill";
  if (harness?.readsSharedFolder) return "Reads the shared folder, no link";
  return "Not included";
}

interface InstallHarnessSelectorProps {
  offered: readonly AgentId[];
  chosen: readonly AgentId[];
  onChosenChange: (chosen: AgentId[]) => void;
  linkMode: InstallLinkMode;
  onLinkModeChange: (mode: InstallLinkMode) => void;
  /** True when `.claude/skills` is a whole-folder link to the shared folder. */
  claudeReadsShared: boolean;
  scope: InstallScope;
  disabled?: boolean;
  /** Set when the install method picks the harnesses itself. */
  lockedReason?: string;
}

export function InstallHarnessSelector({
  offered,
  chosen,
  onChosenChange,
  linkMode,
  onLinkModeChange,
  claudeReadsShared,
  scope,
  disabled = false,
  lockedReason,
}: InstallHarnessSelectorProps) {
  const chosenSet = new Set(chosen);
  const locked = disabled || !!lockedReason;
  return (
    <div className="flex flex-col gap-2">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Destination
      </span>
      {lockedReason && <p className="m-0 text-caption text-text-tertiary">{lockedReason}</p>}

      <div className="-mx-2 flex flex-col">
        {offered.map((id) => {
          const label = installHarness(id)?.label ?? id;
          const isChosen = chosenSet.has(id);
          return (
            <div key={id} className={ROW_CLASS}>
              <HarnessIcon harness={id} size={16} />
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-body text-text-primary">{label}</span>
                <span className="truncate text-caption text-text-tertiary">
                  {harnessCaption(id, isChosen, linkMode, claudeReadsShared, scope)}
                </span>
              </span>
              <CheckboxControl
                checked={isChosen}
                onCheckedChange={(on) =>
                  onChosenChange(toggleInstallHarness(offered, chosen, id, on))
                }
                disabled={locked || installHarnessLocked(id, claudeReadsShared, scope)}
                ariaLabel={`Install for ${label}`}
              />
            </div>
          );
        })}
      </div>

      {!lockedReason && linkModeChoiceVisible(chosen, claudeReadsShared) && (
        <ToggleGroup
          variant="segmented"
          aria-label="Link or copy"
          value={[linkMode]}
          disabled={disabled}
          onValueChange={(next) => singleSelectToggleValue<InstallLinkMode>(next, onLinkModeChange)}
        >
          <ToggleGroupItem value="link" className={OPTION_CLASS}>
            Link
          </ToggleGroupItem>
          <ToggleGroupItem value="copy" className={OPTION_CLASS}>
            Copy
          </ToggleGroupItem>
        </ToggleGroup>
      )}

      <p className="m-0 font-mono text-caption break-words text-text-tertiary">
        {installFoldersPreview(chosen, scope, linkMode, claudeReadsShared)}
      </p>
    </div>
  );
}
