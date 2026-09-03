// ============================================================================
// SettingsView - the app's own preferences, as opposed to anything read back
// from a harness. Today that is one choice: which application "Open in
// editor" hands a skill folder to. macOS's own default text editor is
// TextEdit, which is never what someone editing a SKILL.md wants.
// ============================================================================

import { useEffect, useState } from "react";
import { Check, KeyRound, SquarePen } from "lucide-react";
import { Button, Input, RadioGroup, RadioGroupItem } from "@skill-studio/ui";
import {
  getPreferredEditor,
  getSkillsShAccess,
  listInstalledEditors,
  setPreferredEditor,
  setSkillsShApiKey,
  type EditorOption,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";

/** The always-available first choice: the first installed editor Skill Studio knows about. */
function automaticOption(editors: EditorOption[]) {
  const first = editors[0];
  return {
    app_name: null,
    label: first ? `Automatic (${first.label})` : "Automatic",
  };
}

function EditorPicker() {
  const addToast = useAppStore((state) => state.addToast);
  const [editors, setEditors] = useState<EditorOption[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    Promise.all([listInstalledEditors(), getPreferredEditor()])
      .then(([installed, preferred]) => {
        if (cancelled) return;
        setEditors(installed);
        setSelected(preferred);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read your editor setting",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  const choose = (appName: string | null) => {
    const previous = selected;
    setSelected(appName);
    setPreferredEditor(appName).catch((err) => {
      setSelected(previous);
      addToast({
        type: "error",
        title: "Couldn't save your editor",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const options = [automaticOption(editors), ...editors];

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border-subtle p-4">
      <div className="flex items-center gap-2 text-body font-semibold text-text-primary">
        <SquarePen size={15} className="text-text-tertiary" />
        Open in editor
      </div>
      <p className="m-0 max-w-prose text-small text-text-tertiary">
        The application a skill folder opens in from the Locations card. Automatic picks the first
        code editor found in your Applications folders.
      </p>
      {isLoading ? (
        <p className="m-0 text-small text-text-tertiary">Looking for installed editors…</p>
      ) : (
        <RadioGroup
          className="flex-col"
          aria-label="Open in editor"
          value={selected ?? "system"}
          onValueChange={(value) => choose(value === "system" ? null : value)}
        >
          {options.map((option) => (
            <label
              key={option.app_name ?? "system"}
              className="flex h-9 cursor-pointer items-center gap-2 rounded-sm px-2 text-left text-body text-text-secondary transition-colors hover:bg-bg-hover has-data-checked:text-text-primary"
            >
              <span className="flex size-4 items-center justify-center text-accent">
                {selected === option.app_name && <Check size={14} />}
              </span>
              <RadioGroupItem value={option.app_name ?? "system"} className="sr-only" />
              {option.label}
            </label>
          ))}
        </RadioGroup>
      )}
      {!isLoading && editors.length === 0 && (
        <p className="m-0 text-small text-text-tertiary">
          No known code editor was found in your Applications folders.
        </p>
      )}
    </div>
  );
}

/**
 * The saved key is never displayed or re-fetched - the input always starts
 * empty, and only the status line reflects what's actually resolved from
 * `~/.agents/skill-studio.json` (see `api::resolve_skills_sh_access`).
 */
function SkillsShKeySetting() {
  const addToast = useAppStore((state) => state.addToast);
  const [access, setAccess] = useState<{ mode: "direct" | "server"; server_url: string | null }>({
    mode: "server",
    server_url: null,
  });
  const [isLoading, setIsLoading] = useState(true);
  const [key, setKey] = useState("");
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getSkillsShAccess()
      .then((status) => {
        if (!cancelled) setAccess(status);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't read your skills.sh access mode",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [addToast]);

  const save = () => {
    setIsSaving(true);
    setSkillsShApiKey(key)
      .then(() => {
        setAccess({ mode: "direct", server_url: null });
        setKey("");
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't save your skills.sh key",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => setIsSaving(false));
  };

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border-subtle p-4">
      <div className="flex items-center gap-2 text-body font-semibold text-text-primary">
        <KeyRound size={15} className="text-text-tertiary" />
        skills.sh
      </div>
      <p className="m-0 max-w-prose text-small text-text-tertiary">
        Browsing normally goes through the Skill Studio server, so no key is required. A skills.sh
        API key (issued by Vercel) pasted here lets this machine browse directly instead. Installing
        by source never needs a key.
      </p>
      <div className="flex items-center gap-2">
        <Input
          type="password"
          placeholder="Developer override: skills.sh API key"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          className="max-w-sm"
        />
        <Button onClick={save} disabled={isSaving || key.trim().length === 0}>
          Save
        </Button>
      </div>
      {!isLoading && (
        <p className="m-0 text-small text-text-tertiary">
          {access.mode === "direct"
            ? "Using a local skills.sh key (developer override)"
            : `Browsing through the Skill Studio server at ${access.server_url}`}
        </p>
      )}
    </div>
  );
}

export function SettingsView() {
  return (
    <div className="flex flex-col gap-4 p-6">
      <h1 className="m-0 text-heading font-semibold text-text-primary">Settings</h1>
      <SkillsShKeySetting />
      <EditorPicker />
    </div>
  );
}
