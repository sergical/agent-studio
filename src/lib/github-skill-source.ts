// ============================================================================
// Agent Studio - github-skill-source
// Resolves the raw SKILL.md content for a skill from either a well-known
// source URL, an installed GitHub skill_path, or by walking a repo's tree.
// ============================================================================

import { readInstalledSkillMd } from "./skill-api";
import type { InstalledSkill } from "./skill-types";

// Cache for GitHub repo trees - stores SKILL.md paths per repo
const repoTreeCache = new Map<string, { paths: string[]; timestamp: number }>();
const CACHE_TTL = 5 * 60 * 1000; // 5 minutes

/**
 * Find the path to a SKILL.md file in a GitHub repo using the Tree API.
 * This avoids hardcoding repo structures.
 */
export async function findSkillPath(
  owner: string,
  repo: string,
  skillName: string,
): Promise<string | null> {
  const cacheKey = `${owner}/${repo}`;
  const cached = repoTreeCache.get(cacheKey);

  let paths: string[];

  // Check cache first
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    paths = cached.paths;
  } else {
    // Fetch from GitHub Tree API - try both main and master branches
    let fetchedPaths: string[] | null = null;
    for (const branch of ["main", "master"]) {
      try {
        const response = await fetch(
          `https://api.github.com/repos/${owner}/${repo}/git/trees/${branch}?recursive=1`,
        );
        if (!response.ok) {
          continue;
        }

        const data = await response.json();
        fetchedPaths = data.tree
          .filter(
            (f: { type: string; path: string }) => f.type === "blob" && f.path.endsWith("SKILL.md"),
          )
          .map((f: { path: string }) => f.path);
        break;
      } catch {
        // Continue to next branch
      }
    }
    if (!fetchedPaths) {
      return null;
    }
    paths = fetchedPaths;
    repoTreeCache.set(cacheKey, { paths, timestamp: Date.now() });
  }

  // 1. Exact match: vercel-react-best-practices/SKILL.md
  let match = paths.find((p) => p.endsWith(`${skillName}/SKILL.md`));
  if (match) {
    return match;
  }

  // 2. Strip common prefixes (vercel-, remotion-, getsentry-, anthropic-, openai-)
  const prefixes = ["vercel-", "remotion-", "getsentry-", "anthropic-", "openai-"];
  for (const prefix of prefixes) {
    if (skillName.startsWith(prefix)) {
      const stripped = skillName.slice(prefix.length);
      match = paths.find((p) => p.endsWith(`${stripped}/SKILL.md`));
      if (match) {
        return match;
      }
    }
  }

  // 3. Partial match - folder name is part of skill name or vice versa
  match = paths.find((p) => {
    const parts = p.split("/");
    const folder = parts[parts.length - 2]; // Folder containing SKILL.md
    if (!folder) return false;
    // Check if skill name contains folder or folder contains normalized skill name
    return skillName.includes(folder) || folder.includes(skillName.replace(/-/g, ""));
  });

  return match || null;
}

/** Minimal shape of a skill needed to resolve its SKILL.md content. */
export interface SkillContentSource {
  name: string;
  topSource: string | null;
  installedInfo?: InstalledSkill;
}

/**
 * Resolve the raw SKILL.md content for a skill, trying (in order):
 * 1. Reading it straight off disk, from the skill's first deployment path.
 * 2. A well-known skill's source_url (which IS the SKILL.md URL).
 * 3. An installed GitHub skill's recorded skill_path.
 * 4. The top_source treated as a direct URL.
 * 5. The top_source treated as a GitHub `owner/repo`, resolved via the Tree API.
 */
export async function fetchSkillMdContent(source: SkillContentSource): Promise<string | null> {
  const { name, topSource, installedInfo } = source;

  // 1. Read from disk when we have a local deployment path (works offline,
  // and for manual/plugin skills with no remote source at all).
  const localPath = installedInfo?.deployments[0]?.path;
  if (localPath) {
    try {
      const content = await readInstalledSkillMd(`${localPath}/SKILL.md`);
      if (content) {
        return content;
      }
    } catch {
      // Fall through to the remote methods below.
    }
  }

  // 2. For well-known skills, source_url IS the SKILL.md URL - fetch directly
  if (installedInfo?.source_type === "well-known" && installedInfo.source_url) {
    try {
      const response = await fetch(installedInfo.source_url);
      if (response.ok) {
        return await response.text();
      }
    } catch {
      // Fall through to other methods
    }
  }

  // 3. For GitHub skills with skill_path, construct the raw URL
  if (
    installedInfo?.source_type === "github" &&
    installedInfo.source_url &&
    installedInfo.skill_path
  ) {
    // source_url is like "https://github.com/vercel-labs/skills.git"
    // skill_path is like "skills/find-skills/SKILL.md"
    const repoUrl = installedInfo.source_url.replace(/\.git$/, "");
    const match = repoUrl.match(/github\.com\/([^/]+\/[^/]+)/);
    if (match) {
      const ownerRepo = match[1];
      for (const branch of ["main", "master"]) {
        const url = `https://raw.githubusercontent.com/${ownerRepo}/${branch}/${installedInfo.skill_path}`;
        try {
          const response = await fetch(url);
          if (response.ok) {
            return await response.text();
          }
        } catch {
          // Continue to next branch
        }
      }
    }
  }

  // 4. Fall back to top_source for browse tab skills (not installed)
  if (!topSource) {
    return null;
  }

  // 4b. If source is a full URL, try fetching directly
  if (topSource.startsWith("http://") || topSource.startsWith("https://")) {
    try {
      const response = await fetch(topSource);
      if (response.ok) {
        return await response.text();
      }
    } catch {
      // Fall through to GitHub method
    }
  }

  // 5. GitHub owner/repo format - use Tree API to find SKILL.md path
  if (topSource.includes("/")) {
    const [owner, repo] = topSource.split("/");
    const skillPath = await findSkillPath(owner, repo, name);

    if (skillPath) {
      for (const branch of ["main", "master"]) {
        const url = `https://raw.githubusercontent.com/${topSource}/${branch}/${skillPath}`;
        try {
          const response = await fetch(url);
          if (response.ok) {
            return await response.text();
          }
        } catch {
          // Continue to next branch
        }
      }
    }
  }

  return null;
}
