import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import {
  PER_HARNESS_DESTINATIONS,
  perHarnessDestinationPath,
  universalDestinationPath,
} from "@skill-studio/lib";
import { HarnessIcon } from "../ui/HarnessIcon";
import { SwitchControl } from "../ui/SwitchControl";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";
import type {
  AgentId,
  InstallScope,
  PerHarnessDestinationId,
  SkillDestination,
} from "@skill-studio/lib";

interface SkillDestinationSelectorProps {
  destination: SkillDestination;
  harnesses: AgentId[];
  scope: InstallScope;
  onDestinationChange: (destination: SkillDestination) => void;
  onHarnessChange: (harness: PerHarnessDestinationId, enabled: boolean) => void;
  disabled?: boolean;
  perHarnessDisabledReason?: string;
}

/** Selects the independent install destination and exact Per harness copy targets. */
export function SkillDestinationSelector({
  destination,
  harnesses,
  scope,
  onDestinationChange,
  onHarnessChange,
  disabled = false,
  perHarnessDisabledReason,
}: SkillDestinationSelectorProps) {
  return (
    <div className="flex flex-col gap-2">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Destination
      </span>
      <ToggleGroup
        variant="segmented"
        aria-label="Install destination"
        value={[destination]}
        onValueChange={(next) =>
          singleSelectToggleValue<SkillDestination>(next, onDestinationChange)
        }
      >
        <ToggleGroupItem value="universal" disabled={disabled} className="h-[26px] px-3 text-small">
          Universal
        </ToggleGroupItem>
        <ToggleGroupItem
          value="per-harness"
          disabled={disabled || perHarnessDisabledReason !== undefined}
          className="h-[26px] px-3 text-small"
        >
          Per harness
        </ToggleGroupItem>
      </ToggleGroup>
      {perHarnessDisabledReason && (
        <p className="m-0 text-caption text-text-tertiary">{perHarnessDisabledReason}</p>
      )}
      {destination === "universal" ? (
        <div className="flex flex-col gap-1 text-caption text-text-tertiary">
          <p className="m-0">One canonical deployment for every harness that reads Universal.</p>
          <p className="m-0 font-mono">{universalDestinationPath(scope)}</p>
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <p className="m-0 text-caption text-text-tertiary">
            Independent copies. Available with the Copy method only.
          </p>
          {PER_HARNESS_DESTINATIONS.map((row) => (
            <div
              key={row.id}
              className="grid h-9 grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2"
            >
              <HarnessIcon harness={row.id} size={16} />
              <span className="flex min-w-0 flex-col">
                <span className="text-body text-text-primary">{row.label}</span>
                <span className="truncate font-mono text-caption text-text-tertiary">
                  {perHarnessDestinationPath(row.id, scope)}
                </span>
              </span>
              <SwitchControl
                checked={harnesses.includes(row.id)}
                onCheckedChange={(enabled) => onHarnessChange(row.id, enabled)}
                disabled={disabled}
                ariaLabel={`Install for ${row.label}`}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
