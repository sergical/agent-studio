// ============================================================================
// Skill Studio - skill-run-target-types
// TS mirrors of the Rust skill_run_target DTOs: the working directory a
// "Test" run executes in (scratch / worktree / in place) and what it takes
// to diff, apply, or discard it afterward.
// ============================================================================

/** Which kind of working directory a "Test" run executes in. */
export type SkillRunTargetKind = "scratch" | "worktree" | "in_place";

/** Request to prepare one run target. */
export interface SkillRunTargetRequest {
  kind: SkillRunTargetKind;
  skill_name: string;
  skill_folder: string;
  /** Other own skills to also install (Scratch only): [name, folder path]. */
  extra_skills: [name: string, folderPath: string][];
  /** `=== path` fixture text (Scratch only) - see the placeholder in `SkillTestForm`. */
  fixture: string | null;
  /** Required for Worktree and InPlace. */
  project_path: string | null;
}

/** A prepared working directory a run can execute in. */
export interface SkillRunTarget {
  kind: SkillRunTargetKind;
  cwd: string;
  /** Set for Scratch (the scratch dir) and Worktree (the worktree path). */
  cleanup_path?: string;
  /** The project HEAD sha the worktree was branched from (Worktree only). */
  git_head?: string;
}
