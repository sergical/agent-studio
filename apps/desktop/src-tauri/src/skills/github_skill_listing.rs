// ============================================================================
// Skills Module - github_skill_listing
// `list_github_skills`: which skill folders a GitHub repo (or a subpath of
// it) actually contains, so the Add-skill sheet can resolve a pasted URL to
// one skill or offer a picker. One recursive `git/trees` request per
// (repo, ref) answers every shape - a repo root, a `skills/` folder, a
// single skill folder - and the result is cached in-process so re-listing
// and the install that follows don't refetch. The HTTP layer sits behind
// `GithubApi` so the tree-to-listing logic is testable without a network
// call.
// ============================================================================

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use super::gh_cli::run_gh;
use super::skill_update_check;

const GITHUB_API: &str = "https://api.github.com";
const USER_AGENT: &str = "AgentStudio/0.1.0";

// ============================================================================
// DTOs
// ============================================================================

/// One skill folder inside a repo: `path` is repo-relative and `""` for a
/// `SKILL.md` at the repo root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSkillEntry {
    pub name: String,
    pub path: String,
}

/// `list_github_skills`'s result. `commit` is the tree's own sha, which the
/// copy install pins to; `truncated` is GitHub's own flag for a tree too
/// large to return in one response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSkillListing {
    pub repo: String,
    pub git_ref: String,
    pub commit: Option<String>,
    pub skills: Vec<GithubSkillEntry>,
    pub truncated: bool,
}

/// The subset of GitHub's `git/trees/{ref}?recursive=1` payload this module
/// reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GithubTree {
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub tree: Vec<GithubTreeNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubTreeNode {
    pub path: String,
    #[serde(rename = "type")]
    pub node_type: String,
}

// ============================================================================
// HTTP layer - injected so the listing logic runs without a network call.
// ============================================================================

/// A GET against `api.github.com` returning the raw response body.
/// Implementations map HTTP failures to the user-facing strings this
/// module's callers show; `impl Future + Send` rather than `async fn` so the
/// Tauri command's future stays `Send`.
pub trait GithubApi {
    fn get_json(
        &self,
        url: &str,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;
}

/// The real `GithubApi`. `token` is a `gh auth token` value when the CLI is
/// signed in - it raises the rate limit and reaches private repos, and is
/// never logged or returned.
pub struct HttpGithubApi {
    token: Option<String>,
}

impl HttpGithubApi {
    pub fn new() -> Self {
        Self { token: gh_token() }
    }
}

impl Default for HttpGithubApi {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubApi for HttpGithubApi {
    async fn get_json(&self, url: &str) -> Result<String, String> {
        let mut request = reqwest::Client::new()
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let rate_limited = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim() == "0");
            return Err(status_error(status.as_u16(), rate_limited));
        }
        response.text().await.map_err(|e| e.to_string())
    }
}

/// A `gh auth token`, or `None` when `gh` is missing or not signed in - a
/// failure here is never surfaced, since an unauthenticated request still
/// works for public repos.
fn gh_token() -> Option<String> {
    let gh_bin = skill_update_check::resolve_gh_binary()?;
    let stdout = run_gh(&gh_bin, &["auth", "token"], None).ok()?;
    let token = String::from_utf8(stdout).ok()?.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Maps a GitHub response status to an actionable message.
fn status_error(status: u16, rate_limited: bool) -> String {
    match status {
        404 => "Repo or branch not found".to_string(),
        403 | 429 if rate_limited => {
            "GitHub rate limit hit. Sign in to the gh CLI or try later.".to_string()
        }
        401 | 403 => "GitHub refused the request. Sign in to the gh CLI first.".to_string(),
        other => format!("GitHub returned status: {other}"),
    }
}

// ============================================================================
// Listing
// ============================================================================

/// Every skill folder in `tree`, filtered to `path` when one is given.
/// A blob at `<dir>/SKILL.md` makes `<dir>` a skill; a `SKILL.md` at the
/// root makes the repo itself one, named after the repo.
fn build_listing(
    repo: &str,
    git_ref: &str,
    tree: &GithubTree,
    path: Option<&str>,
) -> GithubSkillListing {
    let repo_name = repo.rsplit('/').next().unwrap_or(repo);
    let filter = path
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string());

    let mut skills: Vec<GithubSkillEntry> = tree
        .tree
        .iter()
        .filter(|node| node.node_type == "blob")
        .filter_map(|node| skill_folder_of(&node.path))
        .filter(|folder| matches_filter(folder, filter.as_deref()))
        .map(|folder| GithubSkillEntry {
            name: folder_name(&folder, repo_name),
            path: folder,
        })
        .collect();
    skills.sort_by(|a, b| a.path.cmp(&b.path));
    skills.dedup_by(|a, b| a.path == b.path);

    GithubSkillListing {
        repo: repo.to_string(),
        git_ref: git_ref.to_string(),
        commit: tree.sha.clone(),
        skills,
        truncated: tree.truncated,
    }
}

/// The skill folder a blob path implies, or `None` when the blob isn't a
/// `SKILL.md`.
fn skill_folder_of(blob_path: &str) -> Option<String> {
    if blob_path == "SKILL.md" {
        return Some(String::new());
    }
    blob_path
        .strip_suffix("/SKILL.md")
        .map(|folder| folder.to_string())
}

fn matches_filter(folder: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(prefix) => folder == prefix || folder.starts_with(&format!("{prefix}/")),
    }
}

fn folder_name(folder: &str, repo_name: &str) -> String {
    folder
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(repo_name)
        .to_string()
}

/// `owner/repo`, rejected here rather than in a URL that would send junk to
/// GitHub.
fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    let valid = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    if parts.next().is_some() || !valid(owner) || !valid(name) {
        return Err(format!("`{repo}` is not an owner/repo pair"));
    }
    Ok(())
}

/// The repo's default branch, used when no ref was pasted.
async fn default_branch<A: GithubApi>(api: &A, repo: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct RepoInfo {
        default_branch: Option<String>,
    }
    let body = api.get_json(&format!("{GITHUB_API}/repos/{repo}")).await?;
    let info: RepoInfo = serde_json::from_str(&body)
        .map_err(|e| format!("Could not read GitHub's repo response: {e}"))?;
    info.default_branch
        .filter(|b| !b.is_empty())
        .ok_or_else(|| "Repo or branch not found".to_string())
}

/// Fetches the whole tree for `(repo, git_ref)` once and returns the skill
/// folders under `path`. Callers that want the in-process cache go through
/// `list_github_skills_cached`.
pub async fn list_skills_with<A: GithubApi>(
    api: &A,
    repo: &str,
    path: Option<&str>,
    git_ref: Option<&str>,
) -> Result<GithubSkillListing, String> {
    validate_repo(repo)?;
    let (resolved_ref, tree) = fetch_tree(api, repo, git_ref).await?;
    Ok(build_listing(repo, &resolved_ref, &tree, path))
}

async fn fetch_tree<A: GithubApi>(
    api: &A,
    repo: &str,
    git_ref: Option<&str>,
) -> Result<(String, GithubTree), String> {
    let resolved_ref = match git_ref.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => r.to_string(),
        None => default_branch(api, repo).await?,
    };
    let encoded_ref = urlencoding::encode(&resolved_ref);
    let body = api
        .get_json(&format!(
            "{GITHUB_API}/repos/{repo}/git/trees/{encoded_ref}?recursive=1"
        ))
        .await?;
    let tree: GithubTree = serde_json::from_str(&body)
        .map_err(|e| format!("Could not read GitHub's tree response: {e}"))?;
    Ok((resolved_ref, tree))
}

/// Trees already fetched this session, keyed by `<repo>@<ref-as-asked>` -
/// the whole tree is cached, so a second listing of a different subpath of
/// the same repo is free.
static TREE_CACHE: LazyLock<Mutex<HashMap<String, (String, GithubTree)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_key(repo: &str, git_ref: Option<&str>) -> String {
    format!("{repo}@{}", git_ref.unwrap_or_default())
}

/// `list_skills_with` plus the in-process tree cache. `refresh` bypasses the
/// cached tree and replaces it.
pub async fn list_github_skills_cached<A: GithubApi>(
    api: &A,
    repo: &str,
    path: Option<&str>,
    git_ref: Option<&str>,
    refresh: bool,
) -> Result<GithubSkillListing, String> {
    validate_repo(repo)?;
    let key = cache_key(repo, git_ref);
    if !refresh {
        let cached = TREE_CACHE
            .lock()
            .map_err(|_| "Tree cache was poisoned".to_string())?
            .get(&key)
            .cloned();
        if let Some((resolved_ref, tree)) = cached {
            return Ok(build_listing(repo, &resolved_ref, &tree, path));
        }
    }

    let (resolved_ref, tree) = fetch_tree(api, repo, git_ref).await?;
    if let Ok(mut cache) = TREE_CACHE.lock() {
        cache.insert(key, (resolved_ref.clone(), tree.clone()));
    }
    Ok(build_listing(repo, &resolved_ref, &tree, path))
}

/// Lists the skill folders a GitHub source points at - see the Add-skill
/// sheet, which calls this as the source field settles.
#[tauri::command]
pub async fn list_github_skills(
    repo: String,
    path: Option<String>,
    git_ref: Option<String>,
    refresh: Option<bool>,
) -> Result<GithubSkillListing, String> {
    let api = HttpGithubApi::new();
    list_github_skills_cached(
        &api,
        &repo,
        path.as_deref(),
        git_ref.as_deref(),
        refresh.unwrap_or(false),
    )
    .await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// A `GithubApi` that answers from canned JSON, recording every URL it
    /// was asked for.
    struct FakeApi {
        repo_json: String,
        tree_json: String,
        urls: StdMutex<Vec<String>>,
    }

    impl FakeApi {
        fn new(default_branch: &str, tree_json: &str) -> Self {
            Self {
                repo_json: format!(r#"{{"default_branch":"{default_branch}"}}"#),
                tree_json: tree_json.to_string(),
                urls: StdMutex::new(Vec::new()),
            }
        }
    }

    impl GithubApi for FakeApi {
        async fn get_json(&self, url: &str) -> Result<String, String> {
            self.urls.lock().unwrap().push(url.to_string());
            if url.contains("/git/trees/") {
                Ok(self.tree_json.clone())
            } else {
                Ok(self.repo_json.clone())
            }
        }
    }

    fn tree_json(paths: &[&str], truncated: bool) -> String {
        let nodes: Vec<String> = paths
            .iter()
            .map(|p| format!(r#"{{"path":"{p}","type":"blob"}}"#))
            .collect();
        format!(
            r#"{{"sha":"deadbeef","truncated":{truncated},"tree":[{}]}}"#,
            nodes.join(",")
        )
    }

    async fn list(paths: &[&str], path: Option<&str>) -> GithubSkillListing {
        let api = FakeApi::new("main", &tree_json(paths, false));
        list_skills_with(&api, "kentcdodds/kcd-skills", path, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_single_skill_path_lists_one_skill() {
        let listing = list(
            &[
                "skills/visual-recap/SKILL.md",
                "skills/visual-recap/scripts/run.sh",
                "skills/other/SKILL.md",
            ],
            Some("skills/visual-recap"),
        )
        .await;
        assert_eq!(
            listing.skills,
            vec![GithubSkillEntry {
                name: "visual-recap".to_string(),
                path: "skills/visual-recap".to_string(),
            }]
        );
        assert_eq!(listing.git_ref, "main");
        assert_eq!(listing.commit.as_deref(), Some("deadbeef"));
    }

    #[tokio::test]
    async fn a_folder_of_skills_lists_each_one_sorted() {
        let listing = list(
            &[
                "skills/zed/SKILL.md",
                "skills/alpha/SKILL.md",
                "README.md",
                "skills/alpha/scripts/go.sh",
            ],
            Some("skills"),
        )
        .await;
        let paths: Vec<&str> = listing.skills.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["skills/alpha", "skills/zed"]);
    }

    #[tokio::test]
    async fn nested_skills_are_found_at_any_depth() {
        let listing = list(
            &[
                "packages/a/skills/one/SKILL.md",
                "deep/nested/two/SKILL.md",
                "notes/README.md",
            ],
            None,
        )
        .await;
        let paths: Vec<&str> = listing.skills.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["deep/nested/two", "packages/a/skills/one"]);
        assert_eq!(listing.skills[0].name, "two");
    }

    #[tokio::test]
    async fn a_root_skill_md_is_named_after_the_repo() {
        let listing = list(&["SKILL.md", "skills/one/SKILL.md"], None).await;
        assert_eq!(listing.skills[0].path, "");
        assert_eq!(listing.skills[0].name, "kcd-skills");
    }

    #[tokio::test]
    async fn the_path_filter_excludes_siblings_and_the_root_skill() {
        let listing = list(
            &["SKILL.md", "skills/one/SKILL.md", "other/two/SKILL.md"],
            Some("skills"),
        )
        .await;
        let paths: Vec<&str> = listing.skills.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["skills/one"]);
    }

    #[tokio::test]
    async fn a_truncated_tree_is_surfaced() {
        let api = FakeApi::new("main", &tree_json(&["skills/one/SKILL.md"], true));
        let listing = list_skills_with(&api, "kentcdodds/kcd-skills", None, None)
            .await
            .unwrap();
        assert!(listing.truncated);
    }

    #[tokio::test]
    async fn a_given_ref_skips_the_default_branch_request() {
        let api = FakeApi::new("main", &tree_json(&["skills/one/SKILL.md"], false));
        let listing = list_skills_with(&api, "kentcdodds/kcd-skills", None, Some("v2"))
            .await
            .unwrap();
        assert_eq!(listing.git_ref, "v2");
        let urls = api.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("/git/trees/v2?recursive=1"));
    }

    #[test]
    fn a_repo_that_is_not_an_owner_repo_pair_is_rejected() {
        assert!(validate_repo("kentcdodds/kcd-skills").is_ok());
        assert!(validate_repo("kcd-skills").is_err());
        assert!(validate_repo("a/b/c").is_err());
        assert!(validate_repo("../etc/passwd").is_err());
    }

    #[test]
    fn status_errors_name_the_fix() {
        assert_eq!(status_error(404, false), "Repo or branch not found");
        assert!(status_error(403, true).contains("rate limit"));
        assert!(status_error(500, false).contains("500"));
    }
}
