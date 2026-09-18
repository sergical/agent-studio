// ============================================================================
// ProjectDirectoryField - the project-directory picker row shown under
// SkillStoreInstallFlow's Scope section when installing into a project.
// Pulled out of that component to keep it under react-doctor's component
// size budget.
// ============================================================================

import { FolderPlus } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { ProjectDirectorySelect } from "./ProjectDirectorySelect";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

export function ProjectDirectoryField({
  availableProjects,
  selectedProject,
  onSelectProject,
  onBrowse,
}: {
  availableProjects: string[];
  selectedProject: string | null;
  onSelectProject: (path: string) => void;
  onBrowse: () => void;
}) {
  return (
    <div className="mt-3">
      <span className="mb-1.5 block text-caption font-medium tracking-[0.04em] text-text-tertiary uppercase">
        Project directory
      </span>
      <div className="flex gap-2">
        {availableProjects.length > 0 && (
          <div className="flex-1">
            <ProjectDirectorySelect
              projects={availableProjects}
              value={selectedProject ?? undefined}
              onChange={onSelectProject}
            />
          </div>
        )}
        <Button variant="outline" className={ACTION_BUTTON_CLASS} onClick={onBrowse}>
          <FolderPlus size={14} />
          {availableProjects.length === 0 ? "Choose directory" : "Add"}
        </Button>
      </div>
    </div>
  );
}
