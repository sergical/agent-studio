// ============================================================================
// Skills Module - API Client
// HTTP client for skills.sh API
// ============================================================================

use super::types::{PaginatedSkillsResponse, SkillSearchResponse, SkillSearchResult};

const SKILLS_API_BASE: &str = "https://skills.sh/api";

/// Search for skills on skills.sh
pub async fn search_skills(query: &str, limit: Option<u32>, offset: Option<u32>) -> Result<PaginatedSkillsResponse, String> {
    let encoded_query = urlencoding::encode(query);
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);
    let url = format!("{}/search?q={}&limit={}&offset={}", SKILLS_API_BASE, encoded_query, limit, offset);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch skills: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Skills API returned status: {}", response.status()));
    }

    let data: SkillSearchResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skills response: {}", e))?;

    Ok(PaginatedSkillsResponse {
        skills: data.skills,
        has_more: data.has_more,
    })
}

/// Get skill details from skills.sh
pub async fn get_skill_details(skill_id: &str) -> Result<SkillSearchResult, String> {
    let encoded_id = urlencoding::encode(skill_id);
    let url = format!("{}/skill/{}", SKILLS_API_BASE, encoded_id);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch skill details: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Skills API returned status: {}", response.status()));
    }

    response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skill details: {}", e))
}

/// Get popular skills (sorted by install count)
pub async fn get_popular_skills(limit: Option<u32>, offset: Option<u32>) -> Result<PaginatedSkillsResponse, String> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);
    let url = format!("{}/skills?limit={}&offset={}", SKILLS_API_BASE, limit, offset);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", "AgentStudio/0.1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch popular skills: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Skills API returned status: {}", response.status()));
    }

    let data: SkillSearchResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse skills response: {}", e))?;

    Ok(PaginatedSkillsResponse {
        skills: data.skills,
        has_more: data.has_more,
    })
}
