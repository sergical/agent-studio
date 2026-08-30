// ============================================================================
// Skill Studio - skill-source-parse
// Parses the Add-skill sheet's free-text "Source" field into a structured
// shape the backend's `add_skill` command can act on without re-parsing.
// See `AddSkillSheet` and the Rust-side `ParsedSkillSource` it's sent as.
// ============================================================================

export interface ParsedSkillSource {
  kind: "github" | "git" | "local";
  /** "owner/repo", for kind "github". */
  repo?: string;
  /** Path within the repo to the skill's directory, for kind "github". */
  path?: string;
  /** A pinned ref (branch, tag, or commit) from a `/tree/<ref>/...` or `/blob/<ref>/...` URL. */
  ref?: string;
  /** The skill's directory name, when it could be determined from `path`. */
  skillName?: string;
  /** The clone URL, for kind "git". */
  url?: string;
  /** The filesystem path, for kind "local". */
  localPath?: string;
}

const PARSE_ERROR = "Enter owner/repo, a GitHub URL, a skills.sh URL, or a local path";

/**
 * Parses `input`, trimmed, into a `ParsedSkillSource`, or `{ error }` when
 * none of the accepted forms match. Accepted forms: `owner/repo`,
 * `owner/repo/<path>`, a `github.com` URL (bare, `/tree/<ref>/<path>`, or
 * `/blob/<ref>/<path>/SKILL.md`), a `skills.sh/<owner>/<repo>/<skill>` URL,
 * `git:<url>` or a `https://.../*.git` URL, and an absolute or `~/` local
 * path.
 */
export function parseSkillSource(input: string): ParsedSkillSource | { error: string } {
  const trimmed = input.trim();
  if (!trimmed) return { error: PARSE_ERROR };

  if (trimmed.startsWith("git:")) {
    const url = trimmed.slice("git:".length).trim();
    return url ? { kind: "git", url } : { error: PARSE_ERROR };
  }
  if (/^(https?:\/\/|git@)\S+\.git$/.test(trimmed)) {
    return { kind: "git", url: trimmed };
  }
  if (trimmed.startsWith("~/") || trimmed.startsWith("/")) {
    return { kind: "local", localPath: trimmed };
  }

  const githubUrl = trimmed.match(
    /^https:\/\/github\.com\/([^/]+)\/([^/]+?)\/?(?:\/(tree|blob)\/([^/]+)\/(.+))?$/,
  );
  if (githubUrl) {
    const [, owner, repoName, treeOrBlob, ref, rawPath] = githubUrl;
    const repo = `${owner}/${repoName}`;
    if (!treeOrBlob) return { kind: "github", repo };

    let path = rawPath.replace(/\/+$/, "");
    if (treeOrBlob === "blob") {
      if (!path.endsWith("/SKILL.md")) return { error: PARSE_ERROR };
      path = path.slice(0, -"/SKILL.md".length);
    }
    if (!path) return { error: PARSE_ERROR };
    const skillName = path.split("/").filter(Boolean).pop();
    return { kind: "github", repo, path, ref, skillName };
  }

  const skillsShUrl = trimmed.match(/^https:\/\/skills\.sh\/([^/]+)\/([^/]+)\/(.+?)\/?$/);
  if (skillsShUrl) {
    const [, owner, repoName, skill] = skillsShUrl;
    return { kind: "github", repo: `${owner}/${repoName}`, path: skill, skillName: skill };
  }

  if (trimmed.includes("://")) return { error: PARSE_ERROR };

  const bareRepo = trimmed.match(
    /^([A-Za-z0-9._-]+)\/([A-Za-z0-9._-]+)((?:\/[A-Za-z0-9._-]+)*)\/?$/,
  );
  if (bareRepo) {
    const [, owner, repoName, rest] = bareRepo;
    const repo = `${owner}/${repoName}`;
    const path = rest.replace(/^\//, "");
    if (!path) return { kind: "github", repo };
    const skillName = path.split("/").filter(Boolean).pop();
    return { kind: "github", repo, path, skillName };
  }

  return { error: PARSE_ERROR };
}
