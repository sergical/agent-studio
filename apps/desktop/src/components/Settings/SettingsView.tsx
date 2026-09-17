// ============================================================================
// SettingsView - the app's own preferences, as opposed to anything read back
// from a harness: which application "Open in editor" hands a skill folder to,
// and the project folders discovery searches.
// ============================================================================

import type { SkillSnapshot } from "@skill-studio/lib";
import { PageShell } from "../Shell/PageShell";
import { EditorCard } from "./EditorCard";
import { ErrorReportingCard } from "./ErrorReportingCard";
import { ProjectFoldersCard } from "./ProjectFoldersCard";

interface SettingsViewProps {
  snapshot: SkillSnapshot | undefined;
}

export function SettingsView({ snapshot }: SettingsViewProps) {
  return (
    <PageShell title="Settings" width="narrow">
      <EditorCard />
      <ProjectFoldersCard snapshot={snapshot} />
      <ErrorReportingCard />
    </PageShell>
  );
}
