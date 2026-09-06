// ============================================================================
// PROTOTYPE. Shared header controls so every variant has working Back,
// Assistant, and overflow — not a shared layout.
// ============================================================================

import { useEffect, useState } from "react";
import { ArrowLeft, MoreHorizontal, PanelRight } from "lucide-react";
import { MenuControl, MenuItem, MenuSeparator } from "../../components/ui/MenuControl";

export function useHeaderFeedback() {
  const [feedback, setFeedback] = useState<string | null>(null);

  useEffect(() => {
    if (!feedback) return;
    const id = window.setTimeout(() => setFeedback(null), 1400);
    return () => window.clearTimeout(id);
  }, [feedback]);

  return { feedback, setFeedback };
}

export function BackButton({ flashed, onBack }: { flashed: boolean; onBack: () => void }) {
  return (
    <button
      type="button"
      className={`flex shrink-0 items-center gap-1.5 border-0 bg-transparent p-1 text-small transition-colors duration-150 ease-out ${
        flashed ? "text-text-primary" : "text-text-tertiary hover:text-text-primary"
      }`}
      onClick={onBack}
      aria-label="Back to Skills"
    >
      <ArrowLeft size={16} />
      <span>{flashed ? "Back to Skills" : "Skills"}</span>
    </button>
  );
}

export function AssistantButton({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      className="flex h-(--control-height) items-center gap-1.5 rounded-sm border border-border px-3 text-body text-text-secondary transition-colors duration-150 ease-out hover:bg-bg-tertiary hover:text-text-primary aria-expanded:border-border-focus aria-expanded:text-accent"
      onClick={onToggle}
      aria-expanded={open}
      aria-controls={open ? "skill-header-prototype-assistant" : undefined}
    >
      <PanelRight size={16} />
      <span>{open ? "Hide assistant" : "Assistant"}</span>
    </button>
  );
}

export function OverflowMenu({ onAction }: { onAction: (label: string) => void }) {
  return (
    <MenuControl
      triggerClassName="flex h-(--control-height) w-(--control-height) cursor-pointer items-center justify-center rounded-sm border border-border text-text-secondary transition-colors duration-150 ease-out hover:bg-bg-tertiary hover:text-text-primary"
      triggerAriaLabel="More actions"
      trigger={<MoreHorizontal size={16} />}
      align="end"
    >
      <MenuItem closeOnClick onClick={() => onAction("Revealed in Finder")}>
        Reveal in Finder
      </MenuItem>
      <MenuItem closeOnClick onClick={() => onAction("Opened in editor")}>
        Open in editor
      </MenuItem>
      <MenuItem closeOnClick onClick={() => onAction("Copied path")}>
        Copy path
      </MenuItem>
      <MenuSeparator />
      <MenuItem closeOnClick onClick={() => onAction("Parked skill")}>
        Park skill
      </MenuItem>
      <MenuItem closeOnClick onClick={() => onAction("Forked to local copy")}>
        Fork to local copy
      </MenuItem>
      <MenuSeparator />
      <MenuItem closeOnClick variant="destructive" onClick={() => onAction("Remove queued")}>
        Remove
      </MenuItem>
    </MenuControl>
  );
}

export function HeaderActionCluster({
  assistantOpen,
  onToggleAssistant,
  onAction,
}: {
  assistantOpen: boolean;
  onToggleAssistant: () => void;
  onAction: (label: string) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2">
      <AssistantButton open={assistantOpen} onToggle={onToggleAssistant} />
      <OverflowMenu onAction={onAction} />
    </div>
  );
}

export function HeaderStatus({ message }: { message: string | null }) {
  return (
    <p className="m-0 min-h-[1.25rem] text-caption text-accent" role="status" aria-live="polite">
      {message ?? ""}
    </p>
  );
}
