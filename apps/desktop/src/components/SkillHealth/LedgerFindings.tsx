import type { LedgerFinding } from "@skill-studio/lib";

export function LedgerFindings({ findings }: { findings: LedgerFinding[] }) {
  if (findings.length === 0) return null;
  return (
    <section aria-label="Ledger records" className="border-t border-border-subtle">
      <h3 className="px-3 py-2 text-small text-text-secondary">
        Ledger records · {findings.length}
      </h3>
      {findings.map(({ owner, absence }) => (
        <details
          key={owner.owner_id}
          className="border-b border-border-subtle px-3 py-2 text-small"
        >
          <summary className="cursor-pointer text-text-primary">
            {owner.name} · {owner.scope === "global" ? "Global" : owner.project_path}
          </summary>
          <p className="mt-2 text-text-secondary">
            {absence === "confirmed-absent"
              ? "No deployment was found in the checked scope."
              : "No deployment was observed. Incomplete discovery prevents confirming its absence."}
          </p>
          <dl className="mt-2 text-text-secondary">
            <dt>Owner</dt>
            <dd className="break-all">{owner.owner_id}</dd>
            <dt>Source type</dt>
            <dd>{owner.owner_kind}</dd>
          </dl>
          <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all text-text-tertiary">
            {JSON.stringify(owner.sources, null, 2)}
          </pre>
        </details>
      ))}
    </section>
  );
}
