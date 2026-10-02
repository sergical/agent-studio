// ============================================================================
// Skill Studio - malformed frontmatter repair presentation policy
// ============================================================================

import type {
  Deployment,
  FrontmatterRepairKind,
  FrontmatterRepairPreview,
  InvocationConflictChoice,
} from "@skill-studio/lib";

export function hasMalformedYamlWarning(
  deployment: Pick<Deployment, "spec_violations"> | undefined,
): boolean {
  return Boolean(
    deployment?.spec_violations?.some((violation) =>
      violation.startsWith("invalid YAML frontmatter at line "),
    ),
  );
}

/**
 * The repair a single violation message calls for. The matching mirrors the
 * Rust `violation_is_*` functions in `frontmatter_repair.rs`; a shared
 * fixture of real messages keeps the two sides in step.
 */
export function frontmatterRepairKindForViolation(violation: string): FrontmatterRepairKind | null {
  if (violation.startsWith("invalid YAML frontmatter at line ")) return "colon-scalar";
  if (violation.startsWith('name "')) {
    if (violation.includes("must be 1-64 lowercase")) return "name-format";
    if (violation.includes("does not match its directory name")) return "name-mismatch";
  }
  if (violation === "conflicting invocation keys") return "invocation-conflict";
  return null;
}

/**
 * Every backend repair that targets the deployment's spec violations, one per
 * kind, so a refused name fix never hides the invocation-conflict fix. A broken
 * YAML block hides the other checks, so it stands alone.
 */
export function frontmatterRepairKindsFor(
  deployment: Pick<Deployment, "spec_violations"> | undefined,
): FrontmatterRepairKind[] {
  const kinds = new Set<FrontmatterRepairKind>();
  for (const violation of deployment?.spec_violations ?? []) {
    const kind = frontmatterRepairKindForViolation(violation);
    if (kind !== null) kinds.add(kind);
  }
  return kinds.has("colon-scalar") ? ["colon-scalar"] : [...kinds];
}

/** The header line a repair's Fix button sits on: the one whose violation the repair targets. */
export type FixLine = "error" | "warning" | "note";

/**
 * Which header line carries the Fix for `lineKind` (the repair that is not the invocation
 * conflict). YAML repair belongs to the red line; a name repair belongs to the yellow mismatch line,
 * or to the grey name-format note when no mismatch line is shown. `null` means no line has a Fix.
 */
export function fixLineFor(state: {
  lineKind: FrontmatterRepairKind | undefined;
  hasMismatchLine: boolean;
  hasNameFormatNote: boolean;
}): FixLine | null {
  switch (state.lineKind) {
    case "colon-scalar":
      return "error";
    case "name-mismatch":
      return state.hasMismatchLine ? "warning" : null;
    case "name-format":
      if (state.hasMismatchLine) return "warning";
      return state.hasNameFormatNote ? "note" : null;
    default:
      return null;
  }
}

export const INVOCATION_CONFLICT_OPTIONS: ReadonlyArray<{
  choice: InvocationConflictChoice;
  label: string;
}> = [
  { choice: "user-only", label: "Only you can run it" },
  { choice: "model-only", label: "Only the agent runs it" },
];

const REPAIR_COPY = {
  "colon-scalar": {
    dialogTitle: "Preview YAML fix",
    success: "YAML fixed",
    failure: "Couldn't fix YAML",
  },
  "name-mismatch": {
    dialogTitle: "Preview name fix",
    success: "Name fixed",
    failure: "Couldn't fix name",
  },
  "name-format": {
    dialogTitle: "Preview name fix",
    success: "Name fixed",
    failure: "Couldn't fix name",
  },
  "invocation-conflict": {
    dialogTitle: "Choose who can run this skill",
    success: "Invocation fixed",
    failure: "Couldn't fix invocation",
  },
} satisfies Record<
  FrontmatterRepairKind,
  { dialogTitle: string; success: string; failure: string }
>;

export function frontmatterRepairCopy(kind: FrontmatterRepairKind) {
  return REPAIR_COPY[kind];
}

/** One backend preview per file state: the deployment plus the bytes last scanned for it. */
export function frontmatterPreviewKey(
  deployment: Pick<Deployment, "id" | "content_hash"> | undefined,
): string | null {
  return deployment ? `${deployment.id}\u0000${deployment.content_hash}` : null;
}

/**
 * The local "Quote" button waits for the backend preview to settle and yields
 * to it, so the user never sees "Quote" turn into "Fix".
 */
export function canOfferLocalQuote(state: {
  isPreviewSettled: boolean;
  hasPreview: boolean;
}): boolean {
  return state.isPreviewSettled && !state.hasPreview;
}

export function frontmatterRepairActionLabels(
  preview: Pick<FrontmatterRepairPreview, "allowed_apply_modes">,
): string[] {
  return preview.allowed_apply_modes.map((mode) => {
    if (mode === "fork-and-fix") return "Fork and fix";
    if (mode === "fix-installed-copy") return "Fix installed copy";
    return "Apply fix";
  });
}
