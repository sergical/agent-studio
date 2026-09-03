// ============================================================================
// ProjectDirectorySelect - the project-directory picker shared by the Add
// Skill sheet, the store's install controls, and the Test form. Each item's
// label pairs the directory's basename with its full path, same as the
// native `<select>` this replaces.
// ============================================================================

import { SelectControl } from "../ui/SelectControl";

interface ProjectDirectorySelectProps {
  projects: string[];
  value: string | undefined;
  onChange: (path: string) => void;
  ariaLabel?: string;
  disabled?: boolean;
}

export function ProjectDirectorySelect({
  projects,
  value,
  onChange,
  ariaLabel = "Project directory",
  disabled = false,
}: ProjectDirectorySelectProps) {
  const items = projects.map((path) => ({
    value: path,
    label: `${path.split("/").pop()} – ${path}`,
  }));

  return (
    <SelectControl
      value={value ?? ""}
      onValueChange={onChange}
      items={items}
      ariaLabel={ariaLabel}
      disabled={disabled}
    />
  );
}
