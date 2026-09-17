import { describe, expect, it } from "vitest";
import { applySkillListFilter, deploymentPathForSkillFilter } from "./skill-list-filter";
import type { SkillListFilter } from "./skill-list-filter";
import { filterLedgerFindings, presentDiagnosis } from "./skill-diagnosis-presentation";
import type { LedgerFinding } from "./skill-diagnosis-presentation";
import type { SkillDiagnostic } from "./skill-diagnosis-types";
import type { Deployment, InstalledSkill, SkillSnapshot } from "./skill-types";

function deployment(path: string, overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: `dep:${path}`,
    destination: "per-harness",
    owner_kind: "manual",
    mutability: "mutable",
    backing: { kind: "independent" },
    agent: "Codex",
    scope: "global",
    path,
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "same",
    disabled: false,
    ...overrides,
  };
}
function skill(deployments: Deployment[]): InstalledSkill {
  return {
    name: "alpha",
    source: "fixture/repo",
    source_type: "github",
    installed_at: "before",
    has_update: false,
    update_owner_ids: [],
    source_kind: "manual",
    deployments,
    has_spec: false,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "same",
    content_hashes: ["same"],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
  };
}
function snapshot(skills: InstalledSkill[], issues: SkillDiagnostic[] = []): SkillSnapshot {
  return {
    revision: 1,
    skills,
    projects: [],
    invocations: [],
    heatmap: { days: {} },
    scanned_at: "before",
    last_test_by_skill: {},
    update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
    diagnosis: {
      scope: { home: "/fixture", projects: [], backing_roots: [], plugin_ownership_roots: [] },
      completeness: "complete",
      extent: "full",
      issues,
    },
  };
}
const evidence = (d: Deployment) => ({ deployment_id: d.id, path: d.path });
function ledger(project: string | null): LedgerFinding {
  return {
    kind: "ledger-only",
    absence: "confirmed-absent",
    owner: {
      owner_id: `owner:${project ?? "global"}:alpha`,
      name: "alpha",
      scope: project ? "project" : "global",
      project_path: project,
      owner_kind: "skills-sh",
      sources: [
        {
          kind: "project-skills-sh",
          entry: {
            source: "fixture/repo",
            sourceType: "github",
            computedHash: "hash",
            sourceUrl: null,
            skillPath: null,
            ref: null,
            subagents: null,
            wellKnownDigest: null,
          },
        },
      ],
    },
  };
}

describe("presentDiagnosis", () => {
  it("keeps a parked skill's recreated deployment reachable in parked issue scope", () => {
    const active = deployment("/active/alpha");
    const parked = deployment("/parked/alpha", { scope: "parked" });
    const row = { ...skill([parked, active]), parked: true };
    const input = snapshot(
      [row],
      [
        {
          kind: "parked-but-reinstalled",
          skill_name: "alpha",
          deployments: [evidence(active)],
        },
      ],
    );
    const filter: SkillListFilter = { scope: "parked", query: "", issue: "any" };
    const presented = presentDiagnosis(input, [row], filter);
    expect(presented.issues).toHaveLength(1);
    expect(applySkillListFilter([row], filter, presented.issues)).toEqual([row]);
    expect(deploymentPathForSkillFilter(row, filter, presented.issues)).toBe(active.path);
    expect(deploymentPathForSkillFilter(row, { scope: "parked", query: "" }, [])).toBe(parked.path);
    expect(presentDiagnosis(input, [row], { ...filter, harness: "Claude Code" }).issues).toEqual(
      [],
    );
  });

  it("does not reconstruct missing diagnosis from deployment flags", () => {
    const rows = [skill([deployment("/a", { symlink_is_broken: true })])];
    const input = snapshot(rows);
    delete input.diagnosis;
    expect(presentDiagnosis(input, rows)).toEqual({
      available: false,
      complete: false,
      issues: [],
      ledger: [],
    });
    expect(presentDiagnosis(snapshot(rows), rows).issues).toEqual([]);
  });

  it("keeps exact deployment evidence and the core's violation text", () => {
    const a = deployment("/project-a/alpha");
    const b = deployment("/project-b/alpha");
    const rows = [skill([a, b])];
    rows[0].spec_violations = ["aggregate must not be used"];
    const result = presentDiagnosis(
      snapshot(rows, [
        {
          kind: "blocking-spec-violation",
          skill_name: "alpha",
          deployment: evidence(b),
          violations: ["core finding"],
        },
      ]),
      rows,
    );
    expect(result.complete).toBe(true);
    expect(result.issues[0]).toMatchObject({ deploymentPath: b.path, detail: "core finding" });
  });

  it("does not substitute a same-name target for mismatched ID/path evidence", () => {
    const a = deployment("/project-a/alpha");
    const b = deployment("/project-b/alpha");
    const rows = [skill([a, b])];
    const result = presentDiagnosis(
      snapshot(rows, [
        {
          kind: "broken-symlink",
          skill_name: "alpha",
          target: "/missing",
          deployment: { deployment_id: a.id, path: b.path },
        },
      ]),
      rows,
    );
    expect(result.complete).toBe(false);
    expect(result.issues).toEqual([]);
  });

  it("accepts a unique legacy path but rejects ambiguous legacy evidence", () => {
    const a = deployment("/alpha");
    const finding: SkillDiagnostic = {
      kind: "broken-symlink",
      skill_name: "alpha",
      target: null,
      deployment: { deployment_id: null, path: a.path },
    };
    const rows = [skill([a])];
    expect(presentDiagnosis(snapshot(rows, [finding]), rows).issues[0].deploymentPath).toBe(a.path);
    const ambiguous = [skill([a, { ...a, id: "other" }])];
    expect(presentDiagnosis(snapshot(ambiguous, [finding]), ambiguous)).toMatchObject({
      complete: false,
      issues: [],
    });
  });

  it("omits filtered plugin targets and preserves the visible deployment partition", () => {
    const a = deployment("/own/alpha");
    const plugin = deployment("/plugin/alpha", {
      scope: "plugin",
      plugin: { name: "bundle", harness: "Codex" },
    });
    const rows = [skill([a, plugin])];
    const visible = [skill([a])];
    const issues: SkillDiagnostic[] = [a, plugin].map((d) => ({
      kind: "broken-symlink",
      skill_name: "alpha",
      deployment: evidence(d),
      target: null,
    }));
    const result = presentDiagnosis(snapshot(rows, issues), visible);
    expect(result.complete).toBe(true);
    expect(result.issues).toHaveLength(1);
    expect(result.issues[0].skill.deployments).toEqual([a]);
  });

  it("presents supplied hash groups and one root finding without recomputing hashes", () => {
    const a = deployment("/a", { agent: "Claude Code" });
    const b = deployment("/b", { agent: "Codex" });
    const c = deployment("/c", { agent: "pi" });
    const rows = [skill([a, b, c])];
    const result = presentDiagnosis(
      snapshot(rows, [
        {
          kind: "divergent-copies",
          skill_name: "alpha",
          groups: [
            { content_hash: "core-a", deployments: [evidence(a), evidence(b)] },
            { content_hash: "core-b", deployments: [evidence(c)] },
          ],
        },
        {
          kind: "linked-root",
          harness: "claude-code",
          root: "/fixture/.claude/skills",
          deployments: [evidence(a), evidence(b)],
        },
      ]),
      rows,
    );
    expect(result.issues).toHaveLength(2);
    expect(result.issues[0].detail).toContain("Global · pi differs from Global · Claude Code");
    expect(result.issues[1]).toMatchObject({
      kind: "linked-root",
      deploymentPath: a.path,
      root: "/fixture/.claude/skills",
    });
  });

  it("keeps ledger owners separate from deployments and marks partial reads incomplete", () => {
    const rows = [skill([deployment("/project-c/alpha")])];
    const findings = [ledger(null), ledger("/project-a"), ledger("/project-b")];
    const input = snapshot(rows, findings);
    const result = presentDiagnosis(input, rows);
    expect(result.issues).toEqual([]);
    expect(result.ledger).toEqual(findings);
    expect(result.complete).toBe(true);
    input.diagnosis!.completeness = "partial";
    expect(presentDiagnosis(input, rows).complete).toBe(false);
    input.diagnosis!.completeness = "complete";
    input.diagnosis!.extent = "named";
    expect(presentDiagnosis(input, rows).complete).toBe(false);
  });
});

describe("filterLedgerFindings", () => {
  const findings = [ledger(null), ledger("/project-a"), ledger("/project-b")];
  it("filters the exact scope, source and query without same-name scope leakage", () => {
    expect(filterLedgerFindings(findings, { scope: { project: "/project-a" }, query: "" })).toEqual(
      [findings[1]],
    );
    expect(filterLedgerFindings(findings, { scope: "global", query: "" })).toEqual([findings[0]]);
    expect(
      filterLedgerFindings(findings, { scope: "all", source: "skills-sh", query: "PROJECT-B" }),
    ).toEqual([findings[2]]);
    expect(filterLedgerFindings(findings, { scope: "all", source: "manual", query: "" })).toEqual(
      [],
    );
    expect(
      filterLedgerFindings(findings, { scope: "all", issue: "lock-only", query: "fixture/repo" }),
    ).toEqual(findings);
  });
  it("does not infer deployment-specific attributes for ledger records", () => {
    for (const filter of [
      { scope: "parked" as const },
      { harness: "Codex" },
      { invocation: "both" as const },
      { usage: "unused-30d" as const },
      { issue: "broken-symlink" as const },
    ]) {
      expect(filterLedgerFindings(findings, { scope: "all", query: "", ...filter })).toEqual([]);
    }
  });
});

describe("diagnosis scope and opening target", () => {
  it.each(["project", "harness"] as const)(
    "keeps a healthy %s copy out of issue results",
    (dimension) => {
      const healthy = deployment("/project-a/alpha", {
        scope: "project",
        project_path: "/project-a",
        agent: "Codex",
      });
      const broken = deployment("/project-b/alpha", {
        scope: "project",
        project_path: dimension === "project" ? "/project-b" : "/project-a",
        agent: "Claude Code",
      });
      const rows = [skill([healthy, broken])];
      const input = snapshot(rows, [
        {
          kind: "broken-symlink",
          skill_name: "alpha",
          deployment: evidence(broken),
          target: "/missing",
        },
      ]);
      const healthyFilter: SkillListFilter = {
        scope: { project: "/project-a" },
        query: "",
        issue: "broken-symlink",
      };
      if (dimension === "harness") healthyFilter.harness = "Codex";
      const brokenFilter: SkillListFilter = {
        ...healthyFilter,
        ...(dimension === "project"
          ? { scope: { project: "/project-b" } }
          : { harness: "Claude Code" }),
      };
      const hidden = presentDiagnosis(input, rows, healthyFilter);
      expect(hidden.complete).toBe(true);
      expect(hidden.issues).toEqual([]);
      expect(
        applySkillListFilter(rows, healthyFilter, presentDiagnosis(input, rows).issues),
      ).toEqual([]);
      const shown = presentDiagnosis(input, rows, brokenFilter);
      expect(applySkillListFilter(rows, brokenFilter, shown.issues)).toEqual(rows);
      expect(deploymentPathForSkillFilter(rows[0], brokenFilter, shown.issues)).toBe(broken.path);
      const all: SkillListFilter = { scope: "all", query: "", issue: "broken-symlink" };
      expect(deploymentPathForSkillFilter(rows[0], all, presentDiagnosis(input, rows).issues)).toBe(
        broken.path,
      );
      expect(
        deploymentPathForSkillFilter(rows[0], { scope: "all", query: "" }, shown.issues),
      ).toBeUndefined();
    },
  );

  it("does not report divergent copies when only one hash group is visible", () => {
    const a = deployment("/a/alpha", { scope: "project", project_path: "/a" });
    const b = deployment("/b/alpha", { scope: "project", project_path: "/b" });
    const rows = [skill([a, b])];
    const input = snapshot(rows, [
      {
        kind: "divergent-copies",
        skill_name: "alpha",
        groups: [
          { content_hash: "a", deployments: [evidence(a)] },
          { content_hash: "b", deployments: [evidence(b)] },
        ],
      },
    ]);
    expect(presentDiagnosis(input, rows, { scope: { project: "/a" }, query: "" }).issues).toEqual(
      [],
    );
    const issues = presentDiagnosis(input, rows, { scope: "all", query: "" }).issues;
    expect(issues).toHaveLength(1);
    expect(
      deploymentPathForSkillFilter(
        rows[0],
        { scope: "all", query: "", issue: "duplicate" },
        issues,
      ),
    ).toBe(a.path);
  });
});
