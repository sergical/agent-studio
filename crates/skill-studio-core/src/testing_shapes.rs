// Test support, gated on `cfg(any(test, feature = "testing"))` in `lib.rs`
// the same way `testing.rs` is, so it is never compiled into a shipping
// binary.

//! Real home-directory shapes, rebuilt from the [`FixtureBuilder`]
//! primitives.
//!
//! Unit 2.7 surveyed real machines instead of checking captured home
//! directories into the repo. Each helper here stands for one layout that
//! survey found, rebuilt with generic names (`skill-a`, `bucket-1`) so no
//! real skill, vendor, or plugin name is carried in. Every helper takes and
//! returns a [`FixtureBuilder`] so a caller can compose the ones it needs;
//! [`largest_real_shape_home`] composes all of them into the one home the
//! scan and diagnose paths are exercised against.

use crate::identity::{PARKED_ROOT_RELATIVE, UNIVERSAL_ROOT_RELATIVE};
use crate::testing::fixtures::{skill, skill_md};
use crate::testing::FixtureBuilder;

/// Claude Code's global skills root, relative to the home.
pub const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";
/// Codex's global skills root, relative to the home.
pub const CODEX_ROOT_RELATIVE: &str = ".codex/skills";
/// pi's global skills root, relative to the home.
pub const PI_ROOT_RELATIVE: &str = ".pi/agent/skills";
/// Folder name reserved by Claude Code inside a skills root
/// (`docs/action-map/harnesses/claude-code.md`, "`synced` under
/// `~/.claude/skills` is reserved; the scanner must skip it").
pub const SYNCED_DIR_NAME: &str = "synced";
/// Stands for the uuid a real synced bucket is named after.
pub const SYNCED_BUCKET_ID: &str = "bucket-1";
/// The two skills every synced bucket in these shapes carries.
pub const SYNCED_BUCKET_SKILLS: [&str; 2] = ["skill-a", "skill-b"];
/// Codex's own bundled skills live behind this dot-prefixed folder.
pub const CODEX_SYSTEM_DIR_NAME: &str = ".system";
/// A dot-prefixed folder that carries a `SKILL.md` of its own, so the
/// hidden-entry rule is the only thing keeping it out of the inventory.
pub const CODEX_HIDDEN_SKILL_DIR_NAME: &str = ".hidden-skill";
/// Frontmatter name inside [`CODEX_HIDDEN_SKILL_DIR_NAME`].
pub const CODEX_HIDDEN_SKILL_NAME: &str = "hidden-skill";
/// The six bundled skills [`with_codex_system_skills`] ships.
pub const CODEX_SYSTEM_SKILLS: [&str; 6] = [
    "system-skill-1",
    "system-skill-2",
    "system-skill-3",
    "system-skill-4",
    "system-skill-5",
    "system-skill-6",
];
/// Skill name a plugin ships under its own `skills/` folder.
pub const PLUGIN_SKILL_NAME: &str = "plugin-skill-a";
/// Skill name buried inside a plugin's `node_modules/` subtree. No
/// documented reader loads a skill from there, so no row may name it.
pub const VENDORED_SKILL_NAME: &str = "vendored-skill";
/// The two cached versions of one plugin that sit side by side on disk.
pub const PLUGIN_VERSIONS: [&str; 2] = ["1.0.0", "2.0.0"];
/// Skill a project keeps in a root-level `skills/` folder, which no
/// harness's documented discovery path names.
pub const PROJECT_ROOT_SKILL_NAME: &str = "root-level-skill";
/// Skill a project keeps in `.cursor/skills`, which is Cursor's own
/// documented project root.
pub const CURSOR_SKILL_NAME: &str = "cursor-skill";

/// Every skills root a real home keeps a `.DS_Store` in.
fn roots_with_ds_store() -> [&'static str; 4] {
    [
        UNIVERSAL_ROOT_RELATIVE,
        CLAUDE_ROOT_RELATIVE,
        CODEX_ROOT_RELATIVE,
        PI_ROOT_RELATIVE,
    ]
}

/// Shape 1: Claude Code's `synced/` bucket root inside its skills root,
/// plus the two skills the bucket feeds - into that root and into the
/// shared root beside it.
///
/// On disk the bucket carries an empty `.bucket-<uuid>` marker file, a
/// sibling `<uuid>/` folder with a `manifest.json`, and the skills
/// themselves three levels down at `synced/<uuid>/<name>/SKILL.md`. The
/// `SKILL.md` at `synced/` itself is the part no real home has: it makes
/// the reserved folder pass the spec's shape test, so only the reserved-name
/// rule can keep it out of the inventory.
pub fn with_synced_bucket(builder: FixtureBuilder) -> FixtureBuilder {
    let synced = format!("{CLAUDE_ROOT_RELATIVE}/{SYNCED_DIR_NAME}");
    let bucket = format!("{synced}/{SYNCED_BUCKET_ID}");
    let mut b = skill(builder, &synced, SYNCED_DIR_NAME)
        .file(&format!("{synced}/.bucket-{SYNCED_BUCKET_ID}"), b"")
        .dir(&bucket)
        .file(
            &format!("{bucket}/manifest.json"),
            br#"{"bucket":"bucket-1","skills":["skill-a","skill-b"]}"#,
        );
    for name in SYNCED_BUCKET_SKILLS {
        b = skill(b, &format!("{bucket}/{name}"), name);
        b = skill(b, &format!("{CLAUDE_ROOT_RELATIVE}/{name}"), name);
        b = skill(b, &format!("{UNIVERSAL_ROOT_RELATIVE}/{name}"), name);
    }
    b
}

/// Shape 2: Codex's own bundled skills, in the dot-prefixed `.system`
/// folder its `.codex-system-skills.marker` file marks, plus a dot-prefixed
/// skill folder at the root's own level. The second one passes the spec's
/// shape test - it holds a `SKILL.md` - so only the hidden-entry rule keeps
/// it out of a reader's list.
pub fn with_codex_system_skills(builder: FixtureBuilder) -> FixtureBuilder {
    let system = format!("{CODEX_ROOT_RELATIVE}/{CODEX_SYSTEM_DIR_NAME}");
    let mut b = builder
        .dir(CODEX_ROOT_RELATIVE)
        .file(
            &format!("{CODEX_ROOT_RELATIVE}/.codex-system-skills.marker"),
            b"",
        )
        .dir(&system);
    for name in CODEX_SYSTEM_SKILLS {
        b = skill(b, &format!("{system}/{name}"), name);
    }
    skill(
        b,
        &format!("{CODEX_ROOT_RELATIVE}/{CODEX_HIDDEN_SKILL_DIR_NAME}"),
        CODEX_HIDDEN_SKILL_NAME,
    )
}

/// Shape 3: every entry in pi's skills root is a relative symlink into the
/// shared root, so one skill is both a shared deployment and a pi one.
pub fn with_pi_links_to_shared(builder: FixtureBuilder, names: &[&str]) -> FixtureBuilder {
    let mut b = builder.dir(PI_ROOT_RELATIVE);
    for name in names {
        b = skill(b, &format!("{UNIVERSAL_ROOT_RELATIVE}/{name}"), name);
        // Three levels up from `.pi/agent/skills/<name>`'s own directory is
        // the home, the same relative form `npx skills` writes in symlink
        // mode.
        b = b.alias(
            &format!("{PI_ROOT_RELATIVE}/{name}"),
            &format!("../../../{UNIVERSAL_ROOT_RELATIVE}/{name}"),
        );
    }
    b
}

/// Shape 4: `OpenCode` set up but with no skills root of either name, next
/// to a stray `~/.opencode` folder that is a node package, not a root.
pub fn with_opencode_installed_without_skill_root(builder: FixtureBuilder) -> FixtureBuilder {
    let config = ".config/opencode";
    let stray = ".opencode";
    builder
        .dir(config)
        .file(
            &format!("{config}/opencode.json"),
            br#"{"$schema":"https://opencode.ai/config.json","theme":"system"}"#,
        )
        .file(&format!("{config}/cli.json"), br#"{"version":1}"#)
        .file(&format!("{config}/service.json"), br#"{"port":0}"#)
        .file(".agents/AGENTS.md", b"# Shared agent instructions\n")
        .alias(&format!("{config}/AGENTS.md"), "../../.agents/AGENTS.md")
        .dir(&format!("{stray}/plan"))
        .file(&format!("{stray}/package.json"), br#"{"name":"opencode"}"#)
        .file(&format!("{stray}/plan/notes.md"), b"# Plan\n")
        .file(
            &format!("{stray}/node_modules/pkg-1/package.json"),
            br#"{"name":"pkg-1"}"#,
        )
        .file(
            &format!("{stray}/node_modules/pkg-1/skills/stray-skill/SKILL.md"),
            skill_md("stray-skill").as_bytes(),
        )
}

/// Shape 5: both plugin caches, each with a manifest, sidecar config files,
/// a `skills/` folder, and a `node_modules/` subtree that hides a
/// `SKILL.md` of its own. The Claude Code cache holds two versions of one
/// plugin side by side, which is what an orphaned version left behind looks
/// like before the vendor prunes it.
pub fn with_plugin_cache_nesting(builder: FixtureBuilder) -> FixtureBuilder {
    let mut b = builder;
    for version in PLUGIN_VERSIONS {
        let root = format!(".claude/plugins/cache/vendor-1/plugin-1/{version}");
        b = with_one_cached_plugin(b, &root, ".claude-plugin", version);
    }
    let codex_root = ".codex/plugins/cache/vendor-1/plugin-1/1.0.0".to_string();
    with_one_cached_plugin(b, &codex_root, ".codex-plugin", "1.0.0")
}

/// One cached plugin: the manifest the agent-plugins.org convention puts in
/// `manifest_dir`, the sidecar files a real plugin ships next to it, its
/// `skills/` folder, and a dependency tree carrying a `SKILL.md` that
/// belongs to the dependency, not to the plugin.
fn with_one_cached_plugin(
    builder: FixtureBuilder,
    root: &str,
    manifest_dir: &str,
    version: &str,
) -> FixtureBuilder {
    let b = builder
        .file(
            &format!("{root}/{manifest_dir}/plugin.json"),
            format!(r#"{{"name":"plugin-1","version":"{version}"}}"#).as_bytes(),
        )
        .file(&format!("{root}/.mcp.json"), br#"{"mcpServers":{}}"#)
        .file(&format!("{root}/server.json"), br#"{"name":"plugin-1"}"#)
        .file(
            &format!("{root}/node_modules/dep-1/package.json"),
            br#"{"name":"dep-1"}"#,
        );
    let b = skill(
        b,
        &format!("{root}/skills/{PLUGIN_SKILL_NAME}"),
        PLUGIN_SKILL_NAME,
    );
    skill(
        b,
        &format!("{root}/node_modules/dep-1/skills/{VENDORED_SKILL_NAME}"),
        VENDORED_SKILL_NAME,
    )
}

/// Shape 6: a version-3 lock file whose entries carry `skillPath`, keys the
/// reader does not model (`dismissed`, `lastSelectedAgents`), agent ids the
/// app has no harness for, and one entry whose folder is not on disk.
/// Written with `builder.file(LOCK_FILE_RELATIVE, LOCK_FILE_V3_UNKNOWN_AGENTS)`.
pub const LOCK_FILE_RELATIVE: &str = ".agents/.skill-lock.json";

/// The bytes shape 6 puts at [`LOCK_FILE_RELATIVE`].
pub const LOCK_FILE_V3_UNKNOWN_AGENTS: &[u8] = br#"{
  "version": 3,
  "skills": {
    "skill-a": {
      "source": "owner/repo",
      "sourceType": "github",
      "sourceUrl": "https://github.com/owner/repo",
      "skillPath": "skills/skill-a",
      "skillFolderHash": "1111111111111111111111111111111111111111",
      "installedAt": "2026-01-01T00:00:00Z",
      "updatedAt": "2026-01-02T00:00:00Z",
      "dismissed": false,
      "lastSelectedAgents": ["claude-code", "codex", "amp", "cline", "warp", "zed", "gemini-cli"]
    },
    "missing-folder-skill": {
      "source": "owner/gone",
      "sourceType": "github",
      "sourceUrl": "https://github.com/owner/gone",
      "skillFolderHash": "2222222222222222222222222222222222222222",
      "installedAt": "2026-01-01T00:00:00Z",
      "updatedAt": "2026-01-01T00:00:00Z",
      "dismissed": true,
      "lastSelectedAgents": ["opencode", "pi", "amp"]
    }
  }
}
"#;

/// Shape 7: a project carrying two roots no discovery path names (a
/// root-level `skills/` folder and a `.cursor/skills` folder) next to the
/// three standard ones.
pub fn with_project_non_standard_roots(
    builder: FixtureBuilder,
    project_relative: &str,
) -> FixtureBuilder {
    let mut b = builder.dir(&format!("{project_relative}/.git"));
    b = skill(
        b,
        &format!("{project_relative}/skills/{PROJECT_ROOT_SKILL_NAME}"),
        PROJECT_ROOT_SKILL_NAME,
    );
    b = skill(
        b,
        &format!("{project_relative}/skills/{PROJECT_ROOT_SKILL_NAME}-2"),
        &format!("{PROJECT_ROOT_SKILL_NAME}-2"),
    );
    b = skill(
        b,
        &format!("{project_relative}/.cursor/skills/{CURSOR_SKILL_NAME}"),
        CURSOR_SKILL_NAME,
    );
    b = skill(
        b,
        &format!("{project_relative}/{UNIVERSAL_ROOT_RELATIVE}/project-shared-skill"),
        "project-shared-skill",
    );
    b = skill(
        b,
        &format!("{project_relative}/{CLAUDE_ROOT_RELATIVE}/project-claude-skill"),
        "project-claude-skill",
    );
    skill(
        b,
        &format!("{project_relative}/{CODEX_ROOT_RELATIVE}/project-codex-skill"),
        "project-codex-skill",
    )
}

/// Shape 8: the config files a set-up home carries, each with the keys and
/// tables the app does not model, plus the `.DS_Store` every browsed folder
/// on macOS ends up with.
pub fn with_config_files(builder: FixtureBuilder) -> FixtureBuilder {
    let mut b = builder
        .file(
            ".claude/settings.json",
            br#"{
  "skillOverrides": {"skill-a": "name-only"},
  "permissions": {"allow": ["Bash(git status)"], "deny": []},
  "enabledPlugins": {"plugin-1@vendor-1": true},
  "extraKnownMarketplaces": {"vendor-1": {"source": {"source": "github"}}},
  "statusLine": {"type": "command"},
  "unknownTopLevelKey": {"kept": true}
}
"#,
        )
        .file(
            ".codex/config.toml",
            br#"notify = ["notify-send", "codex"]

[features]
plugins = true

[projects."/abs/path/project-1"]
trust_level = "trusted"

[projects."/abs/path/project-2"]
trust_level = "untrusted"
"#,
        )
        .file(
            ".pi/agent/settings.json",
            br#"{"theme":"dark","telemetry":false}"#,
        );
    for root in roots_with_ds_store() {
        b = b.dir(root).file(&format!("{root}/.DS_Store"), b"\x00\x01");
    }
    b
}

/// Shape 9: the scale a real home reaches - `count` skills in the shared
/// root and `count` more in Claude Code's - plus the data root Skill Studio
/// keeps its journal, parked, and quarantine folders in.
pub fn with_scale(builder: FixtureBuilder, count: usize) -> FixtureBuilder {
    let mut b = builder;
    for index in 0..count {
        let shared = format!("shared-skill-{index:03}");
        let claude = format!("claude-skill-{index:03}");
        b = skill(b, &format!("{UNIVERSAL_ROOT_RELATIVE}/{shared}"), &shared);
        b = skill(b, &format!("{CLAUDE_ROOT_RELATIVE}/{claude}"), &claude);
    }
    b = skill(
        b,
        &format!("{PARKED_ROOT_RELATIVE}/parked-skill"),
        "parked-skill",
    );
    let quarantine = format!(
        "{UNIVERSAL_ROOT_RELATIVE}/{}/01QUARANTINE00000000000000",
        crate::doctor::QUARANTINE_DIR_NAME
    );
    b = skill(
        b,
        &format!("{quarantine}/quarantined-skill"),
        "quarantined-skill",
    );
    let plan = ".skill-studio/journal/plans/01PLAN0000000000000000000";
    b.file(&format!("{plan}/manifest.json"), br#"{"entries":[]}"#)
        .file(
            &format!("{plan}/plan.json"),
            br#"{"id":"01PLAN0000000000000000000","status":"done"}"#,
        )
}

/// How many skills [`largest_real_shape_home`] puts in each of the two
/// large roots: about 160 per root on a heavy real install, measured
/// 2026-09-18.
pub const LARGEST_HOME_SKILLS_PER_ROOT: usize = 160;

/// Every shape above in one home: the largest real layout the survey found,
/// with the synced bucket, Codex's bundled skills, pi's links, `OpenCode`
/// without a root, both plugin caches, the lock file, one project, the
/// config files, and the full skill count.
pub fn largest_real_shape_home() -> FixtureBuilder {
    let mut b = FixtureBuilder::new();
    b = with_synced_bucket(b);
    b = with_codex_system_skills(b);
    b = with_pi_links_to_shared(b, &["linked-skill-1", "linked-skill-2"]);
    b = with_opencode_installed_without_skill_root(b);
    b = with_plugin_cache_nesting(b);
    b = b.file(LOCK_FILE_RELATIVE, LOCK_FILE_V3_UNKNOWN_AGENTS);
    b = with_project_non_standard_roots(b, "src/project-1");
    b = with_config_files(b);
    with_scale(b, LARGEST_HOME_SKILLS_PER_ROOT)
}
