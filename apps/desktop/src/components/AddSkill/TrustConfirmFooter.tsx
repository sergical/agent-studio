// ============================================================================
// TrustConfirmFooter - the "trust this repository?" footer, shared by the Add
// Skill sheet's own needs-trust phase and the Skill Store install flow
// (review round 2, B1): both pause on the same `ops::install` trust gate, so
// both show the same copy rather than each inventing their own.
// ============================================================================

import { Button } from "@skill-studio/ui";

type PackTrustState = { identities: string[]; confirmationToken: string; requestKey: string };

export function TrustConfirmFooter({
  packTrust,
  untrustedIdentity,
  trustBusy,
  onCancel,
  onTrustAndRetry,
}: {
  packTrust: PackTrustState | undefined;
  untrustedIdentity: string | undefined;
  trustBusy: boolean;
  onCancel: () => void;
  onTrustAndRetry: () => void;
}) {
  const identities = packTrust ? packTrust.identities : [untrustedIdentity ?? "this repository"];
  const isPackTrust = !!packTrust;
  return (
    <div className="flex flex-col gap-3 border-t border-border px-5 py-4">
      <div>
        <p className="m-0 text-body font-medium text-text-primary">
          Trust {identities.length === 1 ? "this repository" : "these repositories"}?
        </p>
        <ul className="m-0 mt-1 list-inside list-disc text-small text-text-secondary">
          {identities.map((identity) => (
            <li key={identity}>{identity}</li>
          ))}
        </ul>
        <p className="m-0 mt-2 text-caption text-text-tertiary">
          Skills from this source can run on your machine. Confirm only if you trust it.
        </p>
      </div>
      <div className="flex justify-end gap-2">
        <Button
          variant="outline"
          className="h-(--control-height) rounded-md px-3.5 text-body font-medium"
          onClick={onCancel}
          disabled={trustBusy}
        >
          Close
        </Button>
        <Button
          className="h-(--control-height) rounded-md bg-accent-solid px-3.5 text-body font-medium text-text-on-accent hover:bg-accent-solid-hover"
          onClick={onTrustAndRetry}
          disabled={trustBusy}
        >
          {isPackTrust
            ? `Trust ${identities.length === 1 ? "repository" : "repositories"} and import`
            : "Trust repository and retry"}
        </Button>
      </div>
    </div>
  );
}
