// ============================================================================
// ProjectFolderAddForm - the inline "Type a path or pattern…" form shown at
// the top of the Settings "Project folders" card's folder list. Validates
// through the same backend check `register_skill_projects` runs, so its
// error message is the one the caller sees.
// ============================================================================

import { useState } from "react";
import { AlertTriangle } from "lucide-react";
import { Button, Input } from "@skill-studio/ui";

const INPUT_ID = "project-folder-path-input";
const ERROR_ID = "project-folder-path-error";

interface ProjectFolderAddFormProps {
  /** Saves `value`; resolves to `null` on success or the backend's error message on failure. */
  onAdd: (value: string) => Promise<string | null>;
  onClose: () => void;
}

export function ProjectFolderAddForm({ onAdd, onClose }: ProjectFolderAddFormProps) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Promise.finally, not async/await's try/finally, so the loading flag always resets even if
  // `onAdd` rejects - the React Compiler doesn't support try/finally in this shape.
  const handleSubmit = () => {
    const trimmed = value.trim();
    if (!trimmed || saving) return;
    setSaving(true);
    onAdd(trimmed)
      .then((message) => {
        if (message) {
          setError(message);
        } else {
          onClose();
        }
      })
      .finally(() => setSaving(false));
  };

  return (
    <div className="flex flex-col gap-1.5 border-b border-border-subtle px-3 py-2.5">
      <label htmlFor={INPUT_ID} className="text-small font-medium text-text-tertiary">
        Path or pattern
      </label>
      <Input
        id={INPUT_ID}
        autoFocus
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        className="font-mono"
        placeholder="~/src/*"
        value={value}
        aria-invalid={!!error}
        aria-describedby={error ? ERROR_ID : undefined}
        onChange={(e) => {
          setValue(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            handleSubmit();
          } else if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
        }}
      />
      <p className="m-0 text-caption text-text-tertiary">
        End with /* to track every project directly inside a folder. Only folders with skills are
        added.
      </p>
      {error && (
        <p id={ERROR_ID} role="alert" className="m-0 flex items-center gap-1 text-small text-error">
          <AlertTriangle size={12} aria-hidden="true" />
          {error}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
        <Button size="sm" disabled={saving || !value.trim()} onClick={handleSubmit}>
          Add
        </Button>
      </div>
    </div>
  );
}
