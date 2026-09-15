import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@skill-studio/ui";
import {
  getSkillScopeSettings,
  setSkillScopeSettings,
  type SkillScopeSettings,
} from "../../lib/skill-api";

const FOLDERS = [
  {
    key: "backing_roots",
    label: "Linked skill folders",
    description:
      "Folders outside your home that contain the targets of linked skills. Adding one lets Skill Studio read those targets; it does not install skills or change their files.",
  },
  {
    key: "plugin_ownership_roots",
    label: "Plugin ownership folders",
    description:
      "Choose the outer folder that contains a project and any plugin that owns it. Skill Studio checks for plugin ownership up to this folder. Only add it if no plugin above it owns those skills; an incomplete choice can make plugin skills appear locally editable.",
  },
] as const;

export function SkillFolderSettings() {
  const [settings, setSettings] = useState<SkillScopeSettings | null>(null);
  const [draft, setDraft] = useState({ backing_roots: "", plugin_ownership_roots: "" });
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getSkillScopeSettings()
      .then((value) => {
        if (cancelled) return;
        setSettings(value);
        setDraft({
          backing_roots: value.config.backing_roots.join("\n"),
          plugin_ownership_roots: value.config.plugin_ownership_roots.join("\n"),
        });
      })
      .catch((failure) => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : String(failure));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const disabled = !settings || settings.environment_override || saving;
  const save = async () => {
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const lines = (value: string) =>
        value
          .split("\n")
          .map((line) => line.trim())
          .filter(Boolean);
      await setSkillScopeSettings({
        backing_roots: lines(draft.backing_roots),
        plugin_ownership_roots: lines(draft.plugin_ownership_roots),
      });
      setSaved(true);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section
      className="flex flex-col gap-3 rounded-lg border border-border-subtle p-4"
      aria-label="Additional skill folders"
    >
      <h2 className="m-0 text-body font-semibold text-text-primary">Additional skill folders</h2>
      <p className="m-0 max-w-prose text-small text-text-tertiary">
        Home and known projects are included automatically. Add folders below to resolve linked
        skills or plugin ownership outside those locations. Remove a line and save to stop including
        that folder. This does not delete files.
      </p>
      {!settings && !error && <p role="status">Loading folder settings…</p>}
      {settings?.environment_override && (
        <p role="status">
          These folders are set by SKILL_STUDIO_SCOPE. Remove that launch override and restart the
          app to edit them here.
        </p>
      )}
      {FOLDERS.map(({ key, label, description }) => (
        <div key={key} className="flex flex-col gap-2">
          <label htmlFor={key} className="text-small font-semibold text-text-primary">
            {label}
          </label>
          <p id={`${key}-help`} className="m-0 max-w-prose text-small text-text-tertiary">
            {description} Enter one absolute folder path per line.
          </p>
          <textarea
            id={key}
            aria-describedby={`${key}-help`}
            rows={3}
            disabled={disabled}
            className="w-full resize-y rounded-md border border-border-subtle bg-bg-primary p-2 text-small text-text-primary disabled:opacity-50"
            value={draft[key]}
            onChange={(event) => {
              setDraft({ ...draft, [key]: event.target.value });
              setSaved(false);
            }}
          />
          <Button
            disabled={disabled}
            onClick={async () => {
              try {
                const selected = await open({ directory: true, multiple: false, title: label });
                if (selected) {
                  setDraft((current) => ({
                    ...current,
                    [key]: [current[key], selected].filter(Boolean).join("\n"),
                  }));
                  setSaved(false);
                }
              } catch (failure) {
                setError(failure instanceof Error ? failure.message : String(failure));
              }
            }}
          >
            Choose {label.toLowerCase()}
          </Button>
        </div>
      ))}
      {error && (
        <p role="alert" className="m-0 text-small text-text-primary">
          {error}
        </p>
      )}
      <Button disabled={disabled} onClick={save}>
        {saving ? "Saving…" : "Save folders"}
      </Button>
      {saved && (
        <p role="status" className="m-0 text-small text-text-tertiary">
          Folders saved. The inventory will refresh in the background.
        </p>
      )}
    </section>
  );
}
