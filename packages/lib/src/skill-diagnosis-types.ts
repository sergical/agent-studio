import type { AgentId, InstallScope, LifecycleOwnerKind } from "./skill-types";

export interface DiagnosisScope {
  home: string;
  projects: string[];
  backing_roots: string[];
  plugin_ownership_roots: string[];
}

export interface DiagnosisDeployment {
  deployment_id: string | null;
  path: string;
}

export type LedgerSource =
  | {
      kind: "skills-sh";
      entry: {
        source: string;
        sourceType: string;
        sourceUrl: string;
        skillPath: string | null;
        skillFolderHash: string;
        installedAt: string;
        updatedAt: string;
      };
    }
  | {
      kind: "project-skills-sh";
      entry: {
        source: string;
        sourceType: string;
        computedHash: string;
        sourceUrl: string | null;
        skillPath: string | null;
        ref: string | null;
        subagents: string[] | null;
        wellKnownDigest: string | null;
      };
    }
  | {
      kind: "dotagents";
      entry: {
        name: string;
        source: string;
        github_repo: string | null;
        path: string;
        installed_commit: string | null;
        declared_ref: string | null;
        has_manifest_row: boolean;
      };
    };

export interface LedgerOnlySkill {
  owner_id: string;
  name: string;
  scope: InstallScope;
  project_path: string | null;
  owner_kind: LifecycleOwnerKind;
  sources: LedgerSource[];
}

export type SkillDiagnostic =
  | { kind: "parked-but-reinstalled"; skill_name: string; deployments: DiagnosisDeployment[] }
  | { kind: "linked-root"; harness: AgentId; root: string; deployments: DiagnosisDeployment[] }
  | {
      kind: "divergent-copies";
      skill_name: string;
      groups: { content_hash: string; deployments: DiagnosisDeployment[] }[];
    }
  | {
      kind: "broken-symlink";
      skill_name: string;
      deployment: DiagnosisDeployment;
      target: string | null;
    }
  | {
      kind: "blocking-spec-violation";
      skill_name: string;
      deployment: DiagnosisDeployment;
      violations: string[];
    }
  | { kind: "ledger-only"; owner: LedgerOnlySkill; absence: "confirmed-absent" | "not-observed" };

export interface SkillDiagnosis {
  scope: DiagnosisScope;
  completeness: "complete" | "partial";
  extent: "full" | "named";
  issues: SkillDiagnostic[];
}
