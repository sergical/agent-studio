// ============================================================================
// Skills Module - API Client
// HTTP client for skills.sh's authenticated /api/v1 surface. Every request
// carries `Authorization: Bearer <api_key>` - see `skill_fork_registry`'s
// `skills_sh_api_key` field for where the key comes from.
// ============================================================================

use serde::Deserialize;

use super::skill_dto::{PaginatedSkillsResponse, SkillDetails, SkillSearchResult};

const SKILLS_API_BASE: &str = "https://skills.sh/api/v1";

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

fn client_with_auth(
    api_key: &str,
) -> Result<(reqwest::Client, reqwest::header::HeaderMap), String> {
    let mut headers = reqwest::header::HeaderMap::new();
    let mut auth_value = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|e| format!("Invalid API key: {e}"))?;
    auth_value.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);
    Ok((reqwest::Client::new(), headers))
}

/// Search for skills on skills.sh. The v1 search endpoint has no pagination:
/// it returns up to `limit` results in one shot, so `has_more` is always
/// `false`.
pub async fn search_skills(
    api_key: &str,
    query: &str,
    limit: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let encoded_query = urlencoding::encode(query);
    let limit = limit.unwrap_or(50);
    let url = format!(
        "{}/skills/search?q={}&limit={}",
        SKILLS_API_BASE, encoded_query, limit
    );

    let (client, headers) = client_with_auth(api_key)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch skills: {}", e))?;

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
    api_key: &str,
    page: Option<u32>,
    per_page: Option<u32>,
) -> Result<PaginatedSkillsResponse, String> {
    let page = page.unwrap_or(0);
    let per_page = per_page.unwrap_or(50);
    let url = format!(
        "{}/skills?view=all-time&page={}&per_page={}",
        SKILLS_API_BASE, page, per_page
    );

    let (client, headers) = client_with_auth(api_key)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch popular skills: {}", e))?;

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
pub async fn get_skill_details(api_key: &str, skill_id: &str) -> Result<SkillDetails, String> {
    if skill_id.is_empty()
        || skill_id.starts_with('/')
        || skill_id.contains("..")
        || skill_id.chars().any(char::is_whitespace)
    {
        return Err(format!("Invalid skill id: {skill_id}"));
    }

    let url = format!("{}/skills/{}", SKILLS_API_BASE, skill_id);

    let (client, headers) = client_with_auth(api_key)?;
    let response = client
        .get(&url)
        .headers(headers)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch skill details: {}", e))?;

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
}
