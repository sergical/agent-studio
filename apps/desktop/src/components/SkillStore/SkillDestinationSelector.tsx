import { universalDestinationPath } from "@skill-studio/lib";
import type { InstallScope } from "@skill-studio/lib";

interface SkillDestinationSelectorProps {
  scope: InstallScope;
}

/**
 * Names the Universal destination every install actually writes to.
 * A "Per harness" mode used to sit here too, but the backend never wrote a
 * separate copy per harness - `ops::install` always writes the Universal
 * root and only ever links Claude Code into it (`skill_install.rs`), so
 * offering that choice showed paths (`~/.codex/skills`, ...) the install
 * never created. Which harnesses actually see the result is decided below,
 * by `UniversalVisibilitySelector`.
 */
export function SkillDestinationSelector({ scope }: SkillDestinationSelectorProps) {
  return (
    <div className="flex flex-col gap-2">
      <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Destination
      </span>
      <div className="flex flex-col gap-1 text-caption text-text-tertiary">
        <p className="m-0">One Universal deployment, visible to every enabled reader.</p>
        <p className="m-0 font-mono">{universalDestinationPath(scope)}</p>
      </div>
    </div>
  );
}
