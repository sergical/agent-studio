import { deploymentLabel } from "./skill-health";
import type { HealthIssue } from "./skill-health";
import type { DiagnosisDeployment, SkillDiagnostic } from "./skill-diagnosis-types";
import type { Deployment, InstalledSkill, SkillSnapshot } from "./skill-types";
import { isProjectScope, deploymentMatchesSkillFilter } from "./skill-list-filter";
import type { SkillListFilter } from "./skill-list-filter";

export type LedgerFinding = Extract<SkillDiagnostic, { kind: "ledger-only" }>;

export interface PresentedDiagnosis {
  available: boolean;
  complete: boolean;
  issues: HealthIssue[];
  ledger: LedgerFinding[];
}

interface ResolvedDeployment {
  skill: InstalledSkill;
  deployment: Deployment;
}
const targetKey = (id: string | null, path: string) => JSON.stringify([id || null, path]);

export function presentDiagnosis(
  snapshot: SkillSnapshot | null | undefined,
  visibleSkills: InstalledSkill[],
  filter?: SkillListFilter,
): PresentedDiagnosis {
  const diagnosis = snapshot?.diagnosis;
  const result: PresentedDiagnosis = {
    available: Boolean(diagnosis),
    complete: diagnosis?.completeness === "complete" && diagnosis.extent === "full",
    issues: [],
    ledger: [],
  };
  if (!snapshot || !diagnosis) return result;
  const all = new Map<string, ResolvedDeployment>();
  const paths = new Map<string, ResolvedDeployment[]>();
  for (const skill of snapshot.skills) {
    for (const deployment of skill.deployments) {
      const resolved = { skill, deployment };
      all.set(targetKey(deployment.id, deployment.path), resolved);
      const matches = paths.get(deployment.path) ?? [];
      matches.push(resolved);
      paths.set(deployment.path, matches);
    }
  }
  const visible = new Map<string, ResolvedDeployment>();
  for (const skill of visibleSkills) {
    for (const deployment of skill.deployments) {
      if (filter && !deploymentMatchesSkillFilter(deployment, filter, skill.parked)) continue;
      visible.set(targetKey(deployment.id, deployment.path), { skill, deployment });
    }
  }
  const resolve = (evidence: DiagnosisDeployment): ResolvedDeployment | undefined => {
    const candidates = paths.get(evidence.path);
    const resolved = evidence.deployment_id
      ? all.get(targetKey(evidence.deployment_id, evidence.path))
      : candidates?.length === 1
        ? candidates[0]
        : undefined;
    if (!resolved) {
      result.complete = false;
      return undefined;
    }
    return visible.get(targetKey(resolved.deployment.id, resolved.deployment.path));
  };
  const resolveMany = (evidence: DiagnosisDeployment[]) =>
    evidence.flatMap((item) => {
      const resolved = resolve(item);
      return resolved ? [resolved] : [];
    });
  for (const finding of diagnosis.issues) {
    if (finding.kind === "ledger-only") {
      result.ledger.push(finding);
      continue;
    }
    if (finding.kind === "linked-root") {
      const targets = resolveMany(finding.deployments);
      const first = targets[0];
      if (first)
        result.issues.push({
          kind: "linked-root",
          skill: first.skill,
          deploymentPath: first.deployment.path,
          harness: finding.harness,
          harnessLabel: first.deployment.agent,
          root: finding.root,
          detail: `${first.deployment.agent} reads the Universal folder through a root link at ${finding.root}. Convert it to per-skill links for individual controls.`,
        });
      continue;
    }
    if (finding.kind === "divergent-copies") {
      const groups = finding.groups.map((group) => resolveMany(group.deployments));
      if (groups.filter((group) => group.length > 0).length < 2) continue;
      const targets = groups.flat();
      const first = targets[0];
      if (!first) continue;
      const majority = groups.find((group) => group.length * 2 > targets.length);
      const reference = majority?.[0];
      const differing = groups.filter((group) => group !== majority).flat();
      const detail = reference
        ? `${differing.map((item) => deploymentLabel(item.deployment)).join("; ")} ${differing.length === 1 ? "differs" : "differ"} from ${deploymentLabel(reference.deployment)}`
        : `${targets.length} copies differ: ${targets.map((item) => deploymentLabel(item.deployment)).join("; ")}`;
      result.issues.push({
        kind: "duplicate",
        skill: first.skill,
        deploymentPath: first.deployment.path,
        detail,
      });
      continue;
    }
    if (finding.kind === "parked-but-reinstalled") {
      const first = resolveMany(finding.deployments)[0];
      if (first)
        result.issues.push({
          kind: "parked-but-reinstalled",
          skill: first.skill,
          deploymentPath: first.deployment.path,
          detail: "Parked, but an install or sync recreated a deployment",
        });
      continue;
    }
    const resolved = resolve(finding.deployment);
    if (!resolved) continue;
    result.issues.push({
      kind: finding.kind === "broken-symlink" ? "broken-symlink" : "spec-violation",
      skill: resolved.skill,
      deploymentPath: resolved.deployment.path,
      detail:
        finding.kind === "broken-symlink"
          ? `${deploymentLabel(resolved.deployment)}: missing target ${finding.target ?? ""}`
          : finding.violations.join("; "),
    });
  }
  return result;
}

export function filterLedgerFindings(
  findings: LedgerFinding[],
  filter: SkillListFilter,
): LedgerFinding[] {
  if (filter.scope === "parked" || filter.harness || filter.invocation || filter.usage) return [];
  if (filter.issue && filter.issue !== "any" && filter.issue !== "lock-only") return [];
  const query = filter.query.trim().toLowerCase();
  return findings.filter(({ owner }) => {
    if (filter.scope === "global" && owner.scope !== "global") return false;
    if (isProjectScope(filter.scope) && owner.project_path !== filter.scope.project) return false;
    if (
      filter.source &&
      !owner.sources.some(
        (source) =>
          (source.kind === "project-skills-sh" ? "skills-sh" : source.kind) === filter.source,
      )
    )
      return false;
    return (
      !query ||
      [
        owner.name,
        owner.project_path ?? "",
        ...owner.sources.map((source) => source.entry.source),
      ].some((text) => text.toLowerCase().includes(query))
    );
  });
}
