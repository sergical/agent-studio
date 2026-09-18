// ============================================================================
// StoreInstallFooter - the Install button, or the trust prompt when the
// operation is paused on `needs-trust` (review round 2, B1). Pulled out of
// SkillStoreInstallFlow.tsx to keep that component under react-doctor's
// component size budget.
// ============================================================================

import { Download } from "lucide-react";
import { Button } from "@skill-studio/ui";
import { TrustConfirmFooter } from "../AddSkill/TrustConfirmFooter";
import type { AddSkillOperationEvent } from "@skill-studio/lib";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

export function StoreInstallFooter({
  operation,
  trustBusy,
  isInstalling,
  installDisabled,
  onDeclineTrust,
  onTrustAndRetry,
  onInstall,
}: {
  operation: AddSkillOperationEvent | undefined;
  trustBusy: boolean;
  isInstalling: boolean;
  installDisabled: boolean;
  onDeclineTrust: () => void;
  onTrustAndRetry: () => void;
  onInstall: () => void;
}) {
  if (operation?.phase === "needs-trust") {
    return (
      <TrustConfirmFooter
        packTrust={undefined}
        untrustedIdentity={operation.untrusted_source?.identity}
        trustBusy={trustBusy}
        onCancel={onDeclineTrust}
        onTrustAndRetry={onTrustAndRetry}
      />
    );
  }
  return (
    <div className="mt-auto flex flex-col gap-2 p-5">
      <Button
        className={`${ACTION_BUTTON_CLASS} bg-accent-solid text-text-on-accent hover:bg-accent-solid-hover`}
        onClick={onInstall}
        disabled={installDisabled}
      >
        {isInstalling ? (
          <>
            <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
            Installing…
          </>
        ) : (
          <>
            <Download size={16} />
            Install Skill
          </>
        )}
      </Button>
    </div>
  );
}
