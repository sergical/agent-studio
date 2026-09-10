// ============================================================================
// Skill Studio TUI - Issues screen
// The `diagnose` output, in the order the core sorts it (severity, skill,
// kind), with the next action for each issue.
// ============================================================================

import type { SelectOption } from "@opentui/core";
import { useMemo } from "react";

import type { Diagnosis, NextAction } from "../cli-types.ts";
import { CliTransportError } from "../cli-transport.ts";

export interface IssuesScreenProps {
  diagnosis: Diagnosis | null;
  loading: boolean;
  error: CliTransportError | null;
  onOpenSkill: (skillName: string) => void;
}

function nextActionText(action: NextAction): string {
  switch (action.action) {
    case "preview_repair":
      return "next: preview a frontmatter repair";
    case "repair_link":
      return "next: remove or relink the broken link";
    case "restore":
      return "next: restore the parked or disabled deployment";
    case "rescan":
      return "next: rescan with a longer read budget";
    case "none":
      return "next: no automatic action";
  }
}

export function IssuesScreen({ diagnosis, loading, error, onOpenSkill }: IssuesScreenProps) {
  const options: SelectOption[] = useMemo(() => {
    if (diagnosis === null) return [];
    return diagnosis.issues.map((issue) => ({
      name: `[${issue.severity}] ${issue.kind}: ${issue.skill}`,
      description: `${issue.message} — ${nextActionText(issue.next_action)}`,
      value: issue.skill,
    }));
  }, [diagnosis]);

  if (error !== null) {
    return <text>{`error (${error.code}): ${error.message}`}</text>;
  }
  if (loading || diagnosis === null) {
    return <text>Loading issues…</text>;
  }
  if (options.length === 0) {
    return <text>No issues found.</text>;
  }

  return (
    <select
      focused
      options={options}
      style={{ flexGrow: 1 }}
      onSelect={(_index, option) => {
        if (option === null) return;
        // SAFETY: `options` above always sets `value` to `issue.skill`, a
        // string we constructed; `SelectOption.value` is typed `any` by
        // the library, not narrowed from external input.
        onOpenSkill(option.value as string);
      }}
    />
  );
}
