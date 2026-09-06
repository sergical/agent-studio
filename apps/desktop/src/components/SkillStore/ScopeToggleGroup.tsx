// ============================================================================
// ScopeToggleGroup - Global/Project single-select toggle, shared by
// AddSkillSheet's Scope picker and InstallControls' Install scope picker.
// ============================================================================

import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import type { InstallScope } from "@skill-studio/lib";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";

/** Same segmented look as the sheet's Method control and the skill page's Invocation control. */
const SCOPE_OPTION_CLASS = "h-[26px] px-3 text-small";

interface ScopeToggleGroupProps {
  scope: InstallScope;
  onScopeChange: (scope: InstallScope) => void;
  ariaLabel?: string;
}

export function ScopeToggleGroup({
  scope,
  onScopeChange,
  ariaLabel = "Install scope",
}: ScopeToggleGroupProps) {
  return (
    <ToggleGroup
      variant="segmented"
      aria-label={ariaLabel}
      value={[scope]}
      onValueChange={(next) => singleSelectToggleValue<InstallScope>(next, onScopeChange)}
    >
      <ToggleGroupItem value="global" className={SCOPE_OPTION_CLASS}>
        Global
      </ToggleGroupItem>
      <ToggleGroupItem value="project" className={SCOPE_OPTION_CLASS}>
        Project
      </ToggleGroupItem>
    </ToggleGroup>
  );
}
