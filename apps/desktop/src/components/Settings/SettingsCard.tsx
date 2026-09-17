// ============================================================================
// SettingsCard - the bordered, icon-headed shell every Settings card shares:
// an icon, a title, an optional action at the right of the heading row, one
// sentence describing what the card's setting does, then its content.
// ============================================================================

import type { ReactNode } from "react";

interface SettingsCardProps {
  icon: ReactNode;
  title: string;
  description: string;
  /** Rendered at the right of the heading row, e.g. an "Add folder…" button. */
  action?: ReactNode;
  children: ReactNode;
}

export function SettingsCard({ icon, title, description, action, children }: SettingsCardProps) {
  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border-subtle p-4">
      <div className="flex items-center gap-2">
        <div className="flex flex-1 items-center gap-2 text-body font-semibold text-text-primary">
          {icon}
          {title}
        </div>
        {action}
      </div>
      <p className="m-0 max-w-prose text-small text-text-tertiary">{description}</p>
      {children}
    </div>
  );
}
