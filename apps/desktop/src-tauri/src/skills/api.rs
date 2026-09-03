// ============================================================================
// Skills Module - API Client
// HTTP client for skills.sh's authenticated /api/v1 surface. skills.sh API
// keys aren't per-account, so the desktop app can't ship one: every request
// either carries `Authorization: Bearer <api_key>` straight to skills.sh (a
// developer-override key from `skill_fork_registry`'s `skills_sh_api_key`) or
// goes unauthenticated to the local Skill Studio server, which holds the real
// key - see `SkillsShAccess`/`resolve_skills_sh_access`.
// ============================================================================

use std::path::Path;

use serde::Deserialize;

use super::skill_dto::{PaginatedSkillsResponse, SkillDetails, SkillSearchResult};
use super::skill_fork_registry;

const SKILLS_API_BASE: &str = "https://skills.sh/api/v1";

/// The local Skill Studio server's default base URL, used when
/// `~/.agents/skill-studio.json` has no `skills_sh_api_key` and no
/// `server_url` override - see `apps/server`.
const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";

/// Which credentials and base URL a discovery request uses - resolved once
/// per call by `resolve_skills_sh_access`. `Direct` is the developer override
/// (a `skills_sh_api_key` configured in `~/.agents/skill-studio.json`), sent
/// straight to skills.sh; `Server` is the default, routed through the local
/// Skill Studio server (which holds the real key) with no Authorization
/// header at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsShAccess {
    Direct { api_key: String },
    Server { base_url: String },
}

impl SkillsShAccess {
    /// The `/api/v1`-suffixed base every request path is built from.
    fn base(&self) -> &str {
        match self {
            SkillsShAccess::Direct { .. } => SKILLS_API_BASE,
            SkillsShAccess::Server { base_url } => base_url,
        }
    }

    /// `base_url` with the `/api/v1` suffix stripped back off, for the
    /// "server not reachable at <url>" message - only meaningful for
    /// `Server`.
    fn server_root(&self) -> &str {
        match self {
            SkillsShAccess::Direct { .. } => "",
            SkillsShAccess::Server { base_url } => {
                base_url.strip_suffix("/api/v1").unwrap_or(base_url)
            }
        }
    }
}

/// Resolves which access mode a discovery request should use: a non-empty
/// `skills_sh_api_key` in `~/.agents/skill-studio.json` wins (`Direct`,
/// straight to skills.sh); otherwise `Server`, routed through the local Skill
/// Studio server at `server_url` (or `DEFAULT_SERVER_URL`).
pub fn resolve_skills_sh_access(home: &Path) -> Result<SkillsShAccess, String> {
    let registry = skill_fork_registry::read_fork_registry(home)?;
    if let Some(api_key) = registry
        .skills_sh_api_key
        .filter(|key| !key.trim().is_empty())
    {
        return Ok(SkillsShAccess::Direct { api_key });
    }
    let server_url = registry
        .server_url
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
    Ok(SkillsShAccess::Server {
        base_url: format!("{}/api/v1", server_url.trim_end_matches('/')),
    })
}

/// One skill as returned by the `/skills` and `/skills/search` list
/// endpoints - there is no `description` field in v1 (there wasn't one in
/// the legacy API either).
#[derive(Debug, Deserialize)]
struct SkillV1 {
    id: String,
    name: String,
    installs: u32,
    source: String,
}

impl From<SkillV1> for SkillSearchResult {
    fn from(skill: SkillV1) -> Self {
        SkillSearchResult {
            id: skill.id,
            name: skill.name,
            description: None,
            installs: skill.installs,
            top_source: Some(skill.source),
            author: None,
            tags: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SkillsListResponse {
    data: Vec<SkillV1>,
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(rename = "hasMore")]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct SkillsSearchResponse {
    data: Vec<SkillV1>,
}

/// One file entry in a `/skills/{source}/{slug}` response's `files` array.
#[derive(Debug, Deserialize)]
pub struct SkillFileEntry {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Deserialize)]
struct SkillDetailsResponse {
    id: String,
    source: String,
    slug: String,
    installs: u32,
    hash: String,
    files: Vec<SkillFileEntry>,
}

/// Picks the skill body out of a `/skills/{source}/{slug}` response's
/// `files`: `SKILL.md` (by final path component, case-insensitive) first,
/// then `AGENTS.md`, then the first `.md` file, else `None`.
pub fn pick_skill_md(files: &[SkillFileEntry]) -> Option<String> {
    fn file_name(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or(path)
    }

    files
        .iter()
        .find(|f| file_name(&f.path).eq_ignore_ascii_case("SKILL.md"))
        .or_else(|| {
            files
                .iter()
                .find(|f| file_name(&f.path).eq_ignore_ascii_case("AGENTS.md"))
        })
        .or_else(|| {
            files
                .iter()
                .find(|f| f.path.to_lowercase().ends_with(".md"))
        })
        .map(|f| f.contents.clone())
}

/// Maps a non-success response status to this module's error strings: 401 is
/// called out specifically since it means the configured key itself is bad,
/// everything else just reports the status code.
fn status_error(status: reqwest::StatusCode) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        "skills.sh API key is invalid or expired".to_string()
    } else {
        format!("Skills API returned status: {}", status)
    }
}

/// Builds the client and headers for `access`: `Direct` carries a bearer
/// `Authorization` header, `Server` carries none - the server it talks to
/// holds the real key.
fn client_for(
    access: &SkillsShAccess,
) -> Result<(reqwest::Client, reqwest::header::HeaderMap), String> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let SkillsShAccess::Direct { api_key } = access {
        let mut auth_value = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|e| format!("Invalid API key: {e}"))?;
        auth_value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
    }
    Ok((reqwest::Client::new(), headers))
}

/// Maps a transport-level failure to reach `access`'s base URL (the request
/// never got a response at all) - distinct from `status_error`, which maps a
/// response that did come back but wasn't a success. `Server` mode names the
/// local server explicitly, since "connection refused" otherwise reads like a
/// skills.sh outage.
fn connection_error(access: &SkillsShAccess, e: reqwest::Error) -> String {
    match access {
        SkillsShAccess::Server { .. } => format!(
            "Skill Studio server not reachable at {}. Start it with `npm run dev:server`.",
            access.server_root()
        ),
        SkillsShAccess::Direct { .. } => format!("Failed to reach skills.sh: {e}"),
    }
}

/// Search for skills on skills.sh. The v1 search endpoint has no pagination:
/// it returns up to `limit` results in one shot, so `has_more` is always
/// `false`.
pub async fn search_skills(
    access: &SkillsShAccess,
    query: &str,
    limit: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let encoded_query = urlencoding::encode(query);
    let limit = limit.unwrap_or(50);
    let url = format!(
        "{}/skills/search?q={}&limit={}",
        access.base(),
        encoded_query,
        limit
    );

    let (client, headers) = client_for(access)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| connection_error(access, e))?;

    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }

    let data: SkillsSearchResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skills response: {}", e))?;

    Ok(PaginatedSkillsResponse {
        skills: data.data.into_iter().map(SkillSearchResult::from).collect(),
        has_more: false,
    })
}

/// Get popular skills (all-time, sorted by install count).
pub async fn get_popular_skills(
    access: &SkillsShAccess,
    page: Option<u32>,
    per_page: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let page = page.unwrap_or(0);
    let per_page = per_page.unwrap_or(50);
    let url = format!(
        "{}/skills?view=all-time&page={}&per_page={}",
        access.base(),
        page,
        per_page
    );

    let (client, headers) = client_for(access)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| connection_error(access, e))?;

    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }

    let data: SkillsListResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skills response: {}", e))?;

    Ok(PaginatedSkillsResponse {
        skills: data.data.into_iter().map(SkillSearchResult::from).collect(),
        has_more: data.pagination.has_more,
    })
}

/// Get skill details, including its `SKILL.md`/`AGENTS.md` body, from
/// skills.sh. `skill_id` is the full `owner/repo/slug` (or `domain.com/slug`)
/// id - appended to the URL path as-is (not url-encoded, since the API wants
/// its slashes literal), after validating it can't be used to escape the
/// intended path.
pub async fn get_skill_details(
    access: &SkillsShAccess,
    skill_id: &str,
) -> Result<SkillDetails, String> {
    if skill_id.is_empty()
        || skill_id.starts_with('/')
        || skill_id.contains("..")
        || skill_id.chars().any(char::is_whitespace)
    {
        return Err(format!("Invalid skill id: {skill_id}"));
    }

    let url = format!("{}/skills/{}", access.base(), skill_id);

    let (client, headers) = client_for(access)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| connection_error(access, e))?;

    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }

    let data: SkillDetailsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skill details: {}", e))?;

    Ok(SkillDetails {
        id: data.id,
        source: data.source,
        slug: data.slug,
        installs: data.installs,
        hash: data.hash,
        skill_md: pick_skill_md(&data.files),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, contents: &str) -> SkillFileEntry {
        SkillFileEntry {
            path: path.to_string(),
            contents: contents.to_string(),
        }
    }

    #[test]
    fn pick_skill_md_prefers_skill_md_over_agents_md() {
        let files = vec![file("AGENTS.md", "agents"), file("SKILL.md", "skill")];
        assert_eq!(pick_skill_md(&files), Some("skill".to_string()));
    }

    #[test]
    fn pick_skill_md_is_case_insensitive() {
        let files = vec![file("skill.md", "skill")];
        assert_eq!(pick_skill_md(&files), Some("skill".to_string()));
    }

    #[test]
    fn pick_skill_md_falls_back_to_agents_md() {
        let files = vec![file("README.md", "readme"), file("AGENTS.md", "agents")];
        assert_eq!(pick_skill_md(&files), Some("agents".to_string()));
    }

    #[test]
    fn pick_skill_md_falls_back_to_first_md_file() {
        let files = vec![file("notes.txt", "notes"), file("README.md", "readme")];
        assert_eq!(pick_skill_md(&files), Some("readme".to_string()));
    }

    #[test]
    fn pick_skill_md_none_when_no_md_file() {
        let files = vec![file("notes.txt", "notes"), file("run.sh", "#!/bin/sh")];
        assert_eq!(pick_skill_md(&files), None);
    }

    #[test]
    fn pick_skill_md_matches_nested_path() {
        let files = vec![file("skills/find-bugs/SKILL.md", "skill")];
        assert_eq!(pick_skill_md(&files), Some("skill".to_string()));
    }

    #[test]
    fn resolve_skills_sh_access_defaults_to_the_local_server() {
        let tmp = tempfile::tempdir().unwrap();
        let access = resolve_skills_sh_access(tmp.path()).unwrap();
        assert_eq!(
            access,
            SkillsShAccess::Server {
                base_url: "http://127.0.0.1:8787/api/v1".to_string()
            }
        );
    }

    #[test]
    fn resolve_skills_sh_access_uses_a_configured_server_url() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = skill_fork_registry::read_fork_registry(tmp.path()).unwrap();
        registry.server_url = Some("http://localhost:9999/".to_string());
        skill_fork_registry::write_fork_registry(tmp.path(), &registry).unwrap();

        let access = resolve_skills_sh_access(tmp.path()).unwrap();
        assert_eq!(
            access,
            SkillsShAccess::Server {
                base_url: "http://localhost:9999/api/v1".to_string()
            }
        );
    }

    #[test]
    fn resolve_skills_sh_access_prefers_a_configured_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = skill_fork_registry::read_fork_registry(tmp.path()).unwrap();
        registry.skills_sh_api_key = Some("sk-test-key".to_string());
        registry.server_url = Some("http://localhost:9999".to_string());
        skill_fork_registry::write_fork_registry(tmp.path(), &registry).unwrap();

        let access = resolve_skills_sh_access(tmp.path()).unwrap();
        assert_eq!(
            access,
            SkillsShAccess::Direct {
                api_key: "sk-test-key".to_string()
            }
        );
    }

    #[test]
    fn resolve_skills_sh_access_ignores_a_blank_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = skill_fork_registry::read_fork_registry(tmp.path()).unwrap();
        registry.skills_sh_api_key = Some("   ".to_string());
        skill_fork_registry::write_fork_registry(tmp.path(), &registry).unwrap();

        let access = resolve_skills_sh_access(tmp.path()).unwrap();
        assert_eq!(
            access,
            SkillsShAccess::Server {
                base_url: "http://127.0.0.1:8787/api/v1".to_string()
            }
        );
    }

    #[test]
    fn server_root_strips_the_api_v1_suffix() {
        let access = SkillsShAccess::Server {
            base_url: "http://127.0.0.1:8787/api/v1".to_string(),
        };
        assert_eq!(access.server_root(), "http://127.0.0.1:8787");
    }
}
