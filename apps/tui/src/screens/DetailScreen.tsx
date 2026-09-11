// ============================================================================
// Skill Studio TUI - Detail screen
// One skill's deployments: root, path, link target, and spec violations.
// ============================================================================

import { useKeyboard } from "@opentui/react";

import type { InstalledSkillDto } from "../cli-types.ts";

export interface DetailScreenProps {
  skill: InstalledSkillDto;
  onBack: () => void;
}

export function DetailScreen({ skill, onBack }: DetailScreenProps) {
  useKeyboard((key) => {
    if (key.name === "escape" || key.name === "backspace") onBack();
  });

  return (
    <box style={{ flexDirection: "column" }}>
      <text>{`${skill.name} — ${String(skill.deployments.length)} deployment(s)`}</text>
      {skill.description !== null && <text>{skill.description}</text>}
      <box style={{ flexDirection: "column", marginTop: 1 }}>
        {skill.deployments.map((deployment) => (
          <box key={deployment.id} style={{ flexDirection: "column", marginBottom: 1 }}>
            <text>{`${deployment.harness ?? deployment.root.kind.kind} · ${deployment.root.scope.scope} · ${deployment.mutability}`}</text>
            <text>{`path: ${deployment.path}`}</text>
            <text>{`link target: ${deployment.link_target ?? "(none)"}`}</text>
            {deployment.spec_violations.length > 0 && (
              <text>{`spec violations: ${deployment.spec_violations.join("; ")}`}</text>
            )}
          </box>
        ))}
      </box>
      <text>Press Escape or Backspace to go back.</text>
    </box>
  );
}
