export const GITHUB_REPOSITORY_URL = "https://github.com/sergical/skill-studio";
// The release workflow uploads the DMGs under these fixed names so the site can link to
// /releases/latest/download/ (the versioned tauri-action names change every release).
export const DOWNLOAD_URL = `${GITHUB_REPOSITORY_URL}/releases/latest/download/Skill-Studio-arm64.dmg`;
export const DOWNLOAD_INTEL_URL = `${GITHUB_REPOSITORY_URL}/releases/latest/download/Skill-Studio-intel.dmg`;
export const DOCS_URL = "/docs/";
export const CLI_DOCS_URL = "/docs/cli/";
export const MCP_DOCS_URL = "/docs/mcp/";

// Keep in step with bundle.macOS.minimumSystemVersion in apps/desktop/src-tauri/tauri.conf.json.
const REQUIREMENTS = "macOS 13 or later, Apple Silicon and Intel";
export const TRUST_LINE = `Free and open source · No account · ${REQUIREMENTS}`;
