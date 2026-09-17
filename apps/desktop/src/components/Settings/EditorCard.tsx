// ============================================================================
// EditorCard - Settings' "Open in editor" card: which application a skill
// folder opens in from the Locations card. macOS's own default text editor is
// TextEdit, which is never what someone editing a SKILL.md wants, so the
// choice is explicit - a known app, any other app the user picks, or the
// terminal editor set by `$VISUAL`/`$EDITOR` in their login shell.
// ============================================================================

import { useEffect, useState } from "react";
import { Check, Plus, SquarePen } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, RadioGroup, RadioGroupItem } from "@skill-studio/ui";
import {
  getEditorChoices,
  setPreferredEditor,
  type EditorChoices,
  type EditorOption,
} from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import { SettingsCard } from "./SettingsCard";

const SYSTEM_VALUE = "system";

/** One row's value and label, whichever kind of choice it represents. */
interface EditorRow {
  value: string;
  option: EditorOption;
}

function rowsFor(choices: EditorChoices): EditorRow[] {
  const rows: EditorRow[] = [
    { value: SYSTEM_VALUE, option: { app_name: SYSTEM_VALUE, label: choices.automatic_label } },
    ...choices.apps.map((app) => ({ value: app.app_name, option: app })),
  ];
  if (choices.terminal) {
    rows.push({ value: choices.terminal.app_name, option: choices.terminal });
  }
  return rows;
}

export function EditorCard() {
  const addToast = useAppStore((state) => state.addToast);
  const [choices, setChoices] = useState<EditorChoices | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    getEditorChoices()
      .then((result) => {
        if (!cancelled) setChoices(result);
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

  const save = async (value: string | null) => {
    if (!choices) return;
    const previous = choices.selected;
    setChoices({ ...choices, selected: value });
    try {
      await setPreferredEditor(value);
      // The backend may have normalised a saved `.app` path to a known app
      // name, so the row that ends up checked comes from a fresh read.
      setChoices(await getEditorChoices());
    } catch (err) {
      setChoices({ ...choices, selected: previous });
      addToast({
        type: "error",
        title: "Couldn't save your editor",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const chooseAnother = async () => {
    let picked: string | null;
    try {
      picked = await open({
        directory: false,
        multiple: false,
        defaultPath: "/Applications",
        filters: [{ name: "Applications", extensions: ["app"] }],
      });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't open the app picker",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      return;
    }
    if (!picked) return;
    await save(picked);
  };

  return (
    <SettingsCard
      icon={<SquarePen size={15} className="text-text-tertiary" />}
      title="Open in editor"
      description="The app a skill folder opens in from the Locations card."
    >
      {isLoading || !choices ? (
        <p className="m-0 text-small text-text-tertiary">Looking for installed editors…</p>
      ) : (
        <>
          <RadioGroup
            className="flex-col"
            aria-label="Open in editor"
            value={choices.selected ?? SYSTEM_VALUE}
            onValueChange={(value) => save(value === SYSTEM_VALUE ? null : value)}
          >
            {rowsFor(choices).map(({ value, option }) => (
              <label
                key={value}
                className="flex h-9 cursor-pointer items-center gap-2 rounded-sm px-2 text-left text-body text-text-secondary transition-colors hover:bg-bg-hover has-data-checked:text-text-primary"
              >
                <span className="flex size-4 items-center justify-center text-accent">
                  {(choices.selected ?? SYSTEM_VALUE) === value && (
                    <Check size={14} aria-hidden="true" />
                  )}
                </span>
                <RadioGroupItem value={value} className="sr-only" />
                {option.label}
                {choices.terminal && value === choices.terminal.app_name && (
                  <span className="ml-auto text-small text-text-tertiary">
                    $EDITOR, in a new terminal window
                  </span>
                )}
              </label>
            ))}
          </RadioGroup>
          {choices.apps.length === 0 && (
            <p className="m-0 text-small text-text-tertiary">
              No known code editor was found in your Applications folders.
            </p>
          )}
          <Button
            variant="ghost"
            className="flex h-9 items-center justify-start gap-2 px-2 text-left text-body text-text-tertiary"
            onClick={chooseAnother}
          >
            <span className="flex size-4 items-center justify-center">
              <Plus size={14} aria-hidden="true" />
            </span>
            Choose another app…
          </Button>
        </>
      )}
    </SettingsCard>
  );
}
