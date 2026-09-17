//! Harness capability model.
//!
//! Every fact about a harness is data with evidence. Discovery support (the
//! roots a harness reads) is separate from runner, disable, and link support.
//! Adapters read the catalog; they never hard-code a harness list.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::identity::AgentId;
use crate::ports::{NeverCancel, ProcessSpawner, ProcessSpec, ScopeFs, ToolLookup};

/// How sure the catalog is about a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// Read from the vendor's documentation or repository.
    VerifiedFromDocs,
    /// Derived from a secondary source or an issue thread.
    Inferred,
    /// Not found in any primary source.
    Unknown,
}

/// Where a fact comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Evidence {
    /// URL or document reference.
    pub source: String,
    /// Confidence level.
    pub confidence: Confidence,
}

impl Evidence {
    /// A verified fact.
    pub fn verified(source: &str) -> Self {
        Evidence {
            source: source.to_string(),
            confidence: Confidence::VerifiedFromDocs,
        }
    }

    /// An inferred fact.
    pub fn inferred(source: &str) -> Self {
        Evidence {
            source: source.to_string(),
            confidence: Confidence::Inferred,
        }
    }
}

/// Whether a harness supports one capability.
///
/// Invariant: `Unknown` carries no evidence and is never treated as `No`.
/// An operation that needs a `Yes` refuses on `Unknown` with
/// [`crate::error::ErrorCode::Unsupported`] and names the missing evidence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "support", content = "evidence")]
pub enum Support {
    /// Supported.
    Yes(Evidence),
    /// Not supported.
    No(Evidence),
    /// Supported with limits described in the evidence source.
    Partial(Evidence),
    /// No primary source found.
    Unknown,
}

impl Support {
    /// True only for `Yes`.
    pub fn is_yes(&self) -> bool {
        matches!(self, Support::Yes(_))
    }
}

/// Global (`~`) or project scope level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeLevel {
    /// Under the home root.
    Global,
    /// Under a project root.
    Project,
}

/// Role of a root the harness reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RootRole {
    /// The harness's own skills directory.
    Own,
    /// The shared `.agents/skills` root.
    Universal,
    /// A legacy directory still read for compatibility.
    Legacy,
    /// A plugin cache that ships skills.
    PluginCache,
    /// A root another harness owns that this harness also reads.
    CrossHarness,
}

/// One root a harness reads.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct RootSpec {
    /// Global or project.
    pub level: ScopeLevel,
    /// Path relative to the home or the project.
    pub relative_path: String,
    /// Role.
    pub role: RootRole,
    /// True when the harness walks subdirectories for `SKILL.md`.
    pub recursive: bool,
    /// Evidence.
    pub evidence: Evidence,
}

/// Native switch a harness offers to hide one skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisableMechanism {
    /// Claude Code `settings.json` `skillOverrides`.
    ClaudeSkillOverrides,
    /// Codex `config.toml` `[[skills.config]]`.
    CodexSkillsConfig,
    /// OpenCode `opencode.json(c)` permission rule `{ action: "skill",
    /// resource, effect }` (v2) or `permission.skill` map (v1).
    OpencodePermission,
    /// pi `settings.json` `skills` exclusions (`!pattern`, `-path`), also
    /// written by `pi config`.
    PiSettings,
    /// Skill Studio removes the per-skill link under `~/.claude/skills`.
    ClaudeLinkRemoved,
    /// Skill Studio moves the folder into `.skill-studio-disabled`.
    StudioMoved,
}

/// Why a deployment is off, as shown to the user.
///
/// Invariant: the first four names match the desktop wire today; the last
/// two are new and only appear once the matching mechanism is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DisabledBy {
    /// Codex `[[skills.config]] enabled = false`.
    CodexConfig,
    /// OpenCode `permission.skill` deny.
    OpencodePermission,
    /// The Claude Code per-skill link was removed.
    ClaudeLinkRemoved,
    /// Skill Studio moved the folder aside.
    StudioMoved,
    /// Claude Code `skillOverrides` set to `off`.
    ClaudeSkillOverrides,
    /// pi per-skill toggle.
    PiSettings,
}

/// Native disable facts for one harness.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NativeDisableSpec {
    /// The switch.
    pub mechanism: DisableMechanism,
    /// Scope levels the switch works at.
    pub scopes: Vec<ScopeLevel>,
    /// Whether Skill Studio can write the switch safely.
    pub writable: Support,
    /// Value reported on a disabled deployment.
    pub disabled_by: DisabledBy,
    /// Evidence for the mechanism.
    pub evidence: Evidence,
}

/// Invocation control facts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct InvocationControlSpec {
    /// Can the model be stopped from auto-invoking?
    pub model_invocation: Support,
    /// Can the user-facing command be hidden?
    pub user_invocation: Support,
    /// Where the switch lives (frontmatter field or sidecar file).
    pub mechanism: String,
}

/// Plugin cache facts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PluginCacheSpec {
    /// Is the layout known?
    pub layout: Support,
    /// Relative cache path under the home, when known.
    pub relative_path: Option<String>,
}

/// Usage observation facts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct UsageSourceSpec {
    /// Is the record shape documented?
    pub shape: Support,
    /// Relative path of the session store, when known.
    pub relative_path: Option<String>,
}

/// Runner facts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct RunnerSpec {
    /// Binary name to look up on `PATH`, when the harness has a CLI.
    pub binary: Option<String>,
    /// Whether Skill Studio can drive it for a test run.
    pub support: Support,
}

/// All facts about one harness.
///
/// Invariant: discovery facts (`roots`, `reads_universal_root`) say only what
/// the harness reads. Whether Skill Studio may link, disable, or run comes
/// from the other fields, never from `roots`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessFacts {
    /// Id.
    pub id: AgentId,
    /// Display name.
    pub display_name: String,
    /// Roots the harness reads.
    pub roots: Vec<RootSpec>,
    /// Reads `.agents/skills`.
    pub reads_universal_root: Support,
    /// Follows a symlinked skill folder.
    pub follows_per_skill_link: Support,
    /// Follows a skills root that is itself a symlink.
    pub follows_whole_dir_link: Support,
    /// Skips dot-prefixed entries while walking a root.
    ///
    /// Decides whether a skill under
    /// [`crate::identity::MOVE_ASIDE_DIR_NAME`] stays hidden from a reader
    /// whose root is `recursive`. A one-level reader never reaches
    /// `<root>/<holding dir>/<skill>/SKILL.md`, so the fact is `Yes` for it.
    /// `Unknown` means `scan` cannot claim a moved-aside or parked skill is
    /// hidden from this harness.
    pub skips_hidden_entries: Support,
    /// Native per-skill disable, when one exists.
    pub native_disable: Option<NativeDisableSpec>,
    /// A switch that turns every skill (or the skill tool) off at once.
    /// Recorded from the capability matrix; no operation uses it yet.
    pub all_skills_disable: Support,
    /// Invocation control.
    pub invocation_control: InvocationControlSpec,
    /// Plugin cache.
    pub plugin_cache: PluginCacheSpec,
    /// Usage observation.
    pub usage_source: UsageSourceSpec,
    /// Runner.
    pub runner: RunnerSpec,
}

/// The set of harness facts the core runs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessCatalog {
    /// Facts, one row per harness, in scan order.
    pub facts: Vec<HarnessFacts>,
}

impl HarnessCatalog {
    /// Looks up one harness.
    pub fn get(&self, id: &AgentId) -> Option<&HarnessFacts> {
        self.facts.iter().find(|f| &f.id == id)
    }

    /// Harnesses that read the universal root, in scan order.
    pub fn universal_root_readers(&self) -> Vec<&HarnessFacts> {
        self.facts
            .iter()
            .filter(|f| f.reads_universal_root.is_yes())
            .collect()
    }

    /// Plugin cache roots, derived from the facts rather than from the scope.
    pub fn plugin_cache_paths(&self) -> Vec<(AgentId, String)> {
        self.facts
            .iter()
            .filter_map(|f| {
                f.plugin_cache
                    .relative_path
                    .clone()
                    .map(|p| (f.id.clone(), p))
            })
            .collect()
    }

    /// The catalog shipped with this crate. Sources are listed per fact and
    /// mirror `docs/research/harness-primitives.md`.
    pub fn builtin() -> Self {
        HarnessCatalog {
            facts: vec![
                claude_code(),
                codex(),
                open_code(),
                pi(),
                cursor(),
                grok_build(),
            ],
        }
    }
}

/// Facts observed on this machine, as opposed to documented facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessObserved {
    /// The native config file exists.
    pub config_present: bool,
    /// The native config file can be written (parsed and not `.jsonc`).
    pub config_writable: Support,
    /// Resolved runner binary, when found on `PATH`.
    pub runner_binary: Option<PathBuf>,
}

/// Support for one core operation on one harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationSupport {
    /// Operation name, as used in the envelope.
    pub operation: String,
    /// Support.
    pub support: Support,
}

/// Whether one executable was found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolAvailability {
    /// Name as requested (`npx`, `dotagents`, `gh`).
    pub name: String,
    /// Resolved path, `None` when the tool is absent.
    pub path: Option<PathBuf>,
}

/// Result of the `capabilities` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// One report per requested harness, in catalog order.
    pub harnesses: Vec<CapabilityReport>,
    /// Tools the caller asked about, in request order. Empty unless the
    /// request named tools and a [`crate::ports::ToolLookup`] port exists.
    pub tools: Vec<ToolAvailability>,
}

/// Answer to the `capabilities` operation for one harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityReport {
    /// Harness.
    pub harness: AgentId,
    /// Roots the scanner will visit for this harness.
    pub discovery: Vec<RootSpec>,
    /// Per-operation support derived from the facts.
    pub operations: Vec<OperationSupport>,
    /// Native disable facts.
    pub native_disable: Option<NativeDisableSpec>,
    /// Machine facts, when the caller asked for them.
    pub observed: Option<HarnessObserved>,
    /// Notes for a person (unverified rows, open issues).
    pub runtime_notes: Vec<String>,
}

impl CapabilityReport {
    /// Derives the report from facts and optional observations.
    pub fn from_facts(facts: &HarnessFacts, observed: Option<HarnessObserved>) -> Self {
        let unknown_note = |name: &str, support: &Support| match support {
            Support::Unknown => Some(format!("{name}: no primary source found")),
            _ => None,
        };
        let native = facts
            .native_disable
            .as_ref()
            .map(|d| d.writable.clone())
            .unwrap_or(Support::Unknown);
        let operations = vec![
            OperationSupport {
                operation: "set_harness_enabled".into(),
                support: native,
            },
            OperationSupport {
                operation: "set_claude_link".into(),
                support: facts.follows_per_skill_link.clone(),
            },
            OperationSupport {
                operation: "materialize_root".into(),
                support: facts.follows_whole_dir_link.clone(),
            },
            OperationSupport {
                operation: "run_skill_test".into(),
                support: facts.runner.support.clone(),
            },
            OperationSupport {
                operation: "observe_usage".into(),
                support: facts.usage_source.shape.clone(),
            },
            OperationSupport {
                operation: "set_invocation_policy".into(),
                support: facts.invocation_control.model_invocation.clone(),
            },
        ];
        let runtime_notes = [
            unknown_note("per-skill link", &facts.follows_per_skill_link),
            unknown_note("whole-dir link", &facts.follows_whole_dir_link),
            unknown_note("hidden entries", &facts.skips_hidden_entries),
            unknown_note("plugin cache", &facts.plugin_cache.layout),
            unknown_note("usage source", &facts.usage_source.shape),
            unknown_note(
                "model invocation control",
                &facts.invocation_control.model_invocation,
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        CapabilityReport {
            harness: facts.id.clone(),
            discovery: facts.roots.clone(),
            operations,
            native_disable: facts.native_disable.clone(),
            observed,
            runtime_notes,
        }
    }
}

const CLAUDE_SKILLS_DOC: &str = "https://code.claude.com/docs/en/skills";
/// A symlinked skill folder loads once, deduplicated by its target (so a
/// per-skill link and the whole-dir `~/.claude/skills -> ~/.agents/skills`
/// link both work), and nested `<subdir>/.claude/skills` folders are
/// discovered up to the repo root. Resolved by the docs on 2026-09-16
/// (`docs/action-map/harnesses/claude-code.md`, "Resolved by the docs").
const CLAUDE_SKILLS_DOC_2026_09_16: &str = "https://code.claude.com/docs/en/skills (read 2026-09-16: symlink dedup, nested project discovery)";
const CLAUDE_PLUGINS_REF: &str = "https://code.claude.com/docs/en/plugins-reference";
const CODEX_SKILLS_DOC: &str = "https://learn.chatgpt.com/docs/build-skills";
const CODEX_PLUGINS_DOC: &str = "https://developers.openai.com/plugins/build/plugins";
const OPENCODE_SKILLS_DOC: &str = "https://opencode.ai/v2/docs/skills/";
const OPENCODE_MIGRATE_DOC: &str = "https://opencode.ai/v2/docs/migrate-v1/";
const PI_SKILLS_DOC: &str =
    "https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md";
const PI_PACKAGES_DOC: &str =
    "https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md";
const PI_SETTINGS_DOC: &str = "https://pi.dev/docs/latest/settings";
const CODE_SURVEY: &str = "apps/desktop/src-tauri/src/skills/agents.rs";
const CLAUDE_TRANSCRIPT_READER: &str = "crates/skill-studio-host/src/skill_uses.rs";
const CODEX_TRANSCRIPT_READER: &str = "crates/skill-studio-core/src/skill_uses/codex.rs";
const PI_TRANSCRIPT_READER: &str = "crates/skill-studio-core/src/skill_uses/pi.rs";
const CURSOR_TRANSCRIPT_READER: &str = "crates/skill-studio-core/src/skill_uses/cursor.rs";
const GROK_TRANSCRIPT_READER: &str = "crates/skill-studio-core/src/skill_uses/grok.rs";
const OPENCODE_USE_READER: &str = "crates/skill-studio-core/src/skill_uses/opencode.rs";
const ONE_LEVEL_READER: &str =
    "one-level readers never reach <root>/.skill-studio-disabled/<skill>/SKILL.md";

fn root(level: ScopeLevel, path: &str, role: RootRole, recursive: bool, ev: Evidence) -> RootSpec {
    RootSpec {
        level,
        relative_path: path.to_string(),
        role,
        recursive,
        evidence: ev,
    }
}

fn claude_code() -> HarnessFacts {
    let ev = || Evidence::verified(CLAUDE_SKILLS_DOC);
    HarnessFacts {
        id: AgentId::from(AgentId::CLAUDE_CODE),
        display_name: "Claude Code".into(),
        // `CLAUDE_CONFIG_DIR` overrides `~/.claude` for every path below when
        // set; host root resolution honours it (crates/skill-studio-host/src/
        // discovery.rs). `.claude/skills/synced/` is reserved by the vendor
        // and is skipped, not walked as a skill.
        roots: vec![
            root(
                ScopeLevel::Global,
                ".claude/skills",
                RootRole::Own,
                false,
                ev(),
            ),
            // Nested `<subdir>/.claude/skills` folders are discovered up to
            // the repo root (see `CLAUDE_SKILLS_DOC_2026_09_16`), so the
            // project root is recursive, unlike the global one.
            root(
                ScopeLevel::Project,
                ".claude/skills",
                RootRole::Own,
                true,
                Evidence::verified(CLAUDE_SKILLS_DOC_2026_09_16),
            ),
            root(
                ScopeLevel::Global,
                ".claude/plugins/cache",
                RootRole::PluginCache,
                true,
                Evidence::verified(CLAUDE_PLUGINS_REF),
            ),
        ],
        reads_universal_root: Support::No(ev()),
        follows_per_skill_link: Support::Yes(Evidence::verified(CLAUDE_SKILLS_DOC_2026_09_16)),
        follows_whole_dir_link: Support::Yes(Evidence::verified(CLAUDE_SKILLS_DOC_2026_09_16)),
        skips_hidden_entries: Support::Yes(Evidence::inferred(ONE_LEVEL_READER)),
        all_skills_disable: Support::Unknown,
        native_disable: Some(NativeDisableSpec {
            mechanism: DisableMechanism::ClaudeSkillOverrides,
            scopes: vec![ScopeLevel::Global, ScopeLevel::Project],
            writable: Support::Unknown,
            disabled_by: DisabledBy::ClaudeSkillOverrides,
            evidence: ev(),
        }),
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Yes(ev()),
            user_invocation: Support::Yes(ev()),
            mechanism: "SKILL.md frontmatter `disable-model-invocation`, `user-invocable`".into(),
        },
        plugin_cache: PluginCacheSpec {
            layout: Support::Yes(Evidence::verified(CLAUDE_PLUGINS_REF)),
            relative_path: Some(".claude/plugins/cache".into()),
        },
        usage_source: UsageSourceSpec {
            // The record shape is not documented, but the desktop reads
            // `Skill` tool_use blocks from these transcripts today.
            shape: Support::Yes(Evidence::inferred(CLAUDE_TRANSCRIPT_READER)),
            relative_path: Some(".claude/projects".into()),
        },
        runner: RunnerSpec {
            binary: Some("claude".into()),
            support: Support::Yes(Evidence::inferred(CODE_SURVEY)),
        },
    }
}

fn codex() -> HarnessFacts {
    let ev = || Evidence::verified(CODEX_SKILLS_DOC);
    HarnessFacts {
        id: AgentId::from(AgentId::CODEX),
        display_name: "Codex".into(),
        roots: vec![
            root(
                ScopeLevel::Global,
                ".agents/skills",
                RootRole::Universal,
                false,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".agents/skills",
                RootRole::Universal,
                false,
                ev(),
            ),
            root(
                ScopeLevel::Global,
                ".codex/skills",
                RootRole::Own,
                false,
                Evidence::inferred("https://github.com/openai/codex/issues/22590"),
            ),
            root(
                ScopeLevel::Project,
                ".codex/skills",
                RootRole::Own,
                false,
                Evidence::inferred("https://github.com/openai/codex/issues/22590"),
            ),
            root(
                ScopeLevel::Global,
                ".codex/plugins/cache",
                RootRole::PluginCache,
                true,
                Evidence::verified(CODEX_PLUGINS_DOC),
            ),
        ],
        reads_universal_root: Support::Yes(ev()),
        follows_per_skill_link: Support::Yes(ev()),
        follows_whole_dir_link: Support::Unknown,
        skips_hidden_entries: Support::Yes(Evidence::inferred(ONE_LEVEL_READER)),
        all_skills_disable: Support::Unknown,
        native_disable: Some(NativeDisableSpec {
            mechanism: DisableMechanism::CodexSkillsConfig,
            scopes: vec![ScopeLevel::Global],
            writable: Support::Yes(ev()),
            disabled_by: DisabledBy::CodexConfig,
            evidence: ev(),
        }),
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Yes(ev()),
            user_invocation: Support::No(ev()),
            mechanism: "`agents/openai.yaml` `allow_implicit_invocation`".into(),
        },
        plugin_cache: PluginCacheSpec {
            layout: Support::Yes(Evidence::verified(CODEX_PLUGINS_DOC)),
            relative_path: Some(".codex/plugins/cache".into()),
        },
        usage_source: UsageSourceSpec {
            // The record shape is not documented, but the desktop reads
            // `<skill>` blocks and skill-reading tool calls from these
            // rollout transcripts today.
            shape: Support::Yes(Evidence::inferred(CODEX_TRANSCRIPT_READER)),
            relative_path: Some(".codex/sessions".into()),
        },
        runner: RunnerSpec {
            binary: Some("codex".into()),
            support: Support::Yes(Evidence::inferred(CODE_SURVEY)),
        },
    }
}

fn open_code() -> HarnessFacts {
    let ev = || Evidence::verified(OPENCODE_SKILLS_DOC);
    HarnessFacts {
        id: AgentId::from(AgentId::OPEN_CODE),
        display_name: "OpenCode".into(),
        // v2 (beta) reads root-level `*.md` and nested `SKILL.md` at any
        // depth in every source, so every non-legacy root is recursive.
        roots: vec![
            root(
                ScopeLevel::Global,
                ".claude/skills",
                RootRole::CrossHarness,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".claude/skills",
                RootRole::CrossHarness,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Global,
                ".agents/skills",
                RootRole::Universal,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".agents/skills",
                RootRole::Universal,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Global,
                ".config/opencode/skills",
                RootRole::Own,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".opencode/skills",
                RootRole::Own,
                true,
                ev(),
            ),
            // v1 accepted the singular directory name; v2 canonical is
            // plural. Kept for compatibility with a v1 install.
            root(
                ScopeLevel::Global,
                ".config/opencode/skill",
                RootRole::Legacy,
                false,
                Evidence::verified(OPENCODE_MIGRATE_DOC),
            ),
            root(
                ScopeLevel::Project,
                ".opencode/skill",
                RootRole::Legacy,
                false,
                Evidence::verified(OPENCODE_MIGRATE_DOC),
            ),
        ],
        reads_universal_root: Support::Yes(ev()),
        follows_per_skill_link: Support::Unknown,
        follows_whole_dir_link: Support::Unknown,
        skips_hidden_entries: Support::Yes(Evidence::inferred(ONE_LEVEL_READER)),
        all_skills_disable: Support::Unknown,
        native_disable: Some(NativeDisableSpec {
            mechanism: DisableMechanism::OpencodePermission,
            scopes: vec![ScopeLevel::Global, ScopeLevel::Project],
            writable: Support::Partial(Evidence::inferred(
                "opencode.jsonc is detected but never written",
            )),
            disabled_by: DisabledBy::OpencodePermission,
            evidence: ev(),
        }),
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Yes(ev()),
            user_invocation: Support::Yes(ev()),
            mechanism: "frontmatter `opencode/autoinvoke: false` hides from \
                the model; `slash: false` hides the `/skill` command (v2). \
                v1: `permission.skill` `ask` per agent"
                .into(),
        },
        // v2 plugins register skills programmatically through `ctx.skill`;
        // no documented on-disk cache.
        plugin_cache: PluginCacheSpec {
            layout: Support::Unknown,
            relative_path: None,
        },
        usage_source: UsageSourceSpec {
            shape: Support::Yes(Evidence::inferred(OPENCODE_USE_READER)),
            relative_path: Some(".local/share/opencode".into()),
        },
        runner: RunnerSpec {
            binary: Some("opencode".into()),
            // v2 beta installs a separate `opencode2` binary alongside the
            // v1 `opencode` binary during the migration window.
            support: Support::Partial(Evidence::verified(OPENCODE_MIGRATE_DOC)),
        },
    }
}

fn pi() -> HarnessFacts {
    let ev = || Evidence::verified(PI_SKILLS_DOC);
    HarnessFacts {
        id: AgentId::from(AgentId::PI),
        display_name: "pi".into(),
        roots: vec![
            root(
                ScopeLevel::Global,
                ".pi/agent/skills",
                RootRole::Own,
                true,
                ev(),
            ),
            root(ScopeLevel::Project, ".pi/skills", RootRole::Own, true, ev()),
            root(
                ScopeLevel::Global,
                ".agents/skills",
                RootRole::Universal,
                true,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".agents/skills",
                RootRole::Universal,
                true,
                ev(),
            ),
        ],
        reads_universal_root: Support::Yes(ev()),
        follows_per_skill_link: Support::Unknown,
        follows_whole_dir_link: Support::Unknown,
        skips_hidden_entries: Support::Unknown,
        // `--no-skills` is a CLI flag only, no settings key.
        all_skills_disable: Support::Partial(ev()),
        native_disable: Some(NativeDisableSpec {
            mechanism: DisableMechanism::PiSettings,
            scopes: vec![ScopeLevel::Global, ScopeLevel::Project],
            // The `settings.json` `skills` array accepts `!pattern` and
            // `-path` exclusions; the exact entry the interactive
            // `pi config` writes is undocumented.
            writable: Support::Partial(Evidence::verified(PI_SETTINGS_DOC)),
            disabled_by: DisabledBy::PiSettings,
            evidence: Evidence::verified(PI_PACKAGES_DOC),
        }),
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Yes(ev()),
            user_invocation: Support::Yes(ev()),
            mechanism: "frontmatter `disable-model-invocation`; settings `enableSkillCommands`"
                .into(),
        },
        plugin_cache: PluginCacheSpec {
            // Git packages live under `.pi/agent/git/<host>/<path>` and
            // project packages under `.pi/npm` and `.pi/git`.
            layout: Support::Yes(Evidence::verified(PI_PACKAGES_DOC)),
            relative_path: Some(".pi/agent/npm".into()),
        },
        usage_source: UsageSourceSpec {
            shape: Support::Yes(Evidence::inferred(PI_TRANSCRIPT_READER)),
            relative_path: Some(".pi/agent/sessions".into()),
        },
        runner: RunnerSpec {
            binary: Some("pi".into()),
            support: Support::Yes(Evidence::inferred(CODE_SURVEY)),
        },
    }
}

fn cursor() -> HarnessFacts {
    let ev = || Evidence::inferred(CODE_SURVEY);
    HarnessFacts {
        id: AgentId::from(AgentId::CURSOR),
        display_name: "Cursor".into(),
        roots: vec![
            root(
                ScopeLevel::Global,
                ".cursor/skills",
                RootRole::Own,
                false,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".cursor/skills",
                RootRole::Own,
                false,
                ev(),
            ),
        ],
        reads_universal_root: Support::Unknown,
        follows_per_skill_link: Support::Unknown,
        follows_whole_dir_link: Support::Unknown,
        skips_hidden_entries: Support::Unknown,
        all_skills_disable: Support::Unknown,
        native_disable: None,
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Unknown,
            user_invocation: Support::Unknown,
            mechanism: String::new(),
        },
        plugin_cache: PluginCacheSpec {
            layout: Support::Unknown,
            relative_path: None,
        },
        usage_source: UsageSourceSpec {
            shape: Support::Yes(Evidence::inferred(CURSOR_TRANSCRIPT_READER)),
            relative_path: Some(".cursor/projects".into()),
        },
        runner: RunnerSpec {
            binary: None,
            support: Support::No(ev()),
        },
    }
}

fn grok_build() -> HarnessFacts {
    let ev = || Evidence::inferred(CODE_SURVEY);
    HarnessFacts {
        id: AgentId::from(AgentId::GROK_BUILD),
        display_name: "Grok Build".into(),
        roots: vec![
            root(
                ScopeLevel::Global,
                ".grok/skills",
                RootRole::Own,
                false,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".grok/skills",
                RootRole::Own,
                false,
                ev(),
            ),
        ],
        reads_universal_root: Support::Unknown,
        follows_per_skill_link: Support::Unknown,
        follows_whole_dir_link: Support::Unknown,
        skips_hidden_entries: Support::Unknown,
        all_skills_disable: Support::Unknown,
        native_disable: None,
        invocation_control: InvocationControlSpec {
            model_invocation: Support::Unknown,
            user_invocation: Support::Unknown,
            mechanism: String::new(),
        },
        plugin_cache: PluginCacheSpec {
            layout: Support::Unknown,
            relative_path: Some(".grok/plugins".into()),
        },
        usage_source: UsageSourceSpec {
            shape: Support::Yes(Evidence::inferred(GROK_TRANSCRIPT_READER)),
            relative_path: Some(".grok/sessions".into()),
        },
        runner: RunnerSpec {
            binary: None,
            support: Support::No(ev()),
        },
    }
}

// ============================================================================
// Runtime detection: the missing half of the catalog above. `HarnessFacts`
// says what a harness reads and supports, as documented facts; the types and
// trait below say how to prove, on this machine, that the harness exists at
// all. Per `docs/action-map/harnesses/harness-detection.md`.
// ============================================================================

/// A version string or install-method name proven by a probe or a path
/// heuristic, paired with the evidence that produced it.
///
/// Invariant: `value: None` means the fact could not be proven; `evidence`
/// still names why (no binary, a spawn error, an unparseable
/// `--version`), so a caller never has to guess from an empty string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DetectedString {
    /// The value, when proven.
    pub value: Option<String>,
    /// Where the value came from, or why it is `Unknown`.
    pub evidence: Evidence,
}

impl DetectedString {
    fn known(value: impl Into<String>, evidence: Evidence) -> Self {
        DetectedString {
            value: Some(value.into()),
            evidence,
        }
    }

    /// No primary source found; `reason` explains why to a person.
    fn unknown(reason: impl Into<String>) -> Self {
        DetectedString {
            value: None,
            evidence: Evidence {
                source: reason.into(),
                confidence: Confidence::Unknown,
            },
        }
    }
}

/// Runtime detection state for one harness, derived from the signals in
/// `docs/action-map/harnesses/harness-detection.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessState {
    /// No executable, no config, no session data.
    NotFound,
    /// Config or sessions exist but no executable on `PATH`.
    DataOnly,
    /// Executable found; version stays `Unknown` until the probe runs.
    Installed,
    /// Installed and the config file exists.
    Configured,
    /// Configured or Installed, and session evidence exists.
    Used,
}

/// Runtime detection facts for one harness: proven, not guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessDetection {
    /// Catalog id.
    pub id: AgentId,
    /// Display name shown to a person.
    pub display_name: String,
    /// Derived state.
    pub state: HarnessState,
    /// Resolved executable path, when found on `PATH`.
    pub executable: Option<PathBuf>,
    /// Version, proven by `<bin> --version`.
    pub version: DetectedString,
    /// Install method, inferred from the resolved executable path.
    pub install_method: DetectedString,
    /// The vendor config file exists under the home root.
    pub configured: bool,
    /// A session or transcript record exists under the home root.
    pub used: bool,
}

/// Result of the `harnesses` operation: one detection row per first-class
/// harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessReport {
    /// Rows, one per [`builtin_adapters`] harness, in that order.
    pub harnesses: Vec<HarnessDetection>,
}

/// Ports one [`HarnessAdapter`] needs to detect its harness on this machine.
pub struct DetectionPorts<'a> {
    /// Filesystem, for config and session presence checks.
    pub fs: &'a dyn ScopeFs,
    /// The scope's home root; every relative path here is under it.
    pub home: &'a Path,
    /// `PATH` lookup; `None` means every executable reads as absent.
    pub tools: Option<&'a dyn ToolLookup>,
    /// Process runner for the `--version` probe; `None` means version and
    /// install method stay `Unknown` even when the binary is found.
    pub spawner: Option<&'a dyn ProcessSpawner>,
}

/// One first-class harness's detection recipe.
///
/// The static [`HarnessCatalog`] says what a harness reads and supports;
/// this trait says how to prove, at runtime, that it exists at all. One
/// implementation per harness (see [`builtin_adapters`]); every impl shares
/// the same four-signal recipe through [`HarnessAdapter::detect`]'s default
/// body, built from the small set of facts each impl supplies.
pub trait HarnessAdapter: Send + Sync {
    /// Catalog id.
    fn id(&self) -> AgentId;
    /// Display name shown to a person.
    fn display_name(&self) -> &'static str;
    /// Binary name to resolve on `PATH` and to probe with `--version`, when
    /// the harness has one.
    fn binary_name(&self) -> Option<&'static str>;
    /// Config file path relative to the home root, checked at global scope
    /// only, per `docs/action-map/harnesses/harness-detection.md`.
    fn config_relative_path(&self) -> Option<&'static str>;
    /// Session or transcript root relative to the home root; a non-empty
    /// listing is evidence the harness has run at least once.
    fn used_relative_path(&self) -> Option<&'static str>;

    /// Turns `<bin> --version` stdout into a display string. The default
    /// takes the first non-empty line: none of the six vendors' `--version`
    /// formats are pinned yet (`docs/action-map/harnesses/harness-detection.md`,
    /// "Open items").
    fn parse_version(&self, stdout: &str) -> Option<String> {
        stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }

    /// Runs the shared detection recipe against one set of ports.
    fn detect(&self, ports: &DetectionPorts<'_>) -> HarnessDetection {
        let executable = self
            .binary_name()
            .and_then(|bin| ports.tools.and_then(|lookup| lookup.find_binary(bin)));
        let (version, install_method) = probe_version(self, &executable, ports);
        let configured = self
            .config_relative_path()
            .is_some_and(|rel| ports.fs.symlink_metadata(&ports.home.join(rel)).is_ok());
        let used = self.used_relative_path().is_some_and(|rel| {
            ports
                .fs
                .read_dir(&ports.home.join(rel))
                .is_ok_and(|entries| !entries.is_empty())
        });
        let state = match (executable.is_some(), configured, used) {
            (true, _, true) => HarnessState::Used,
            (true, true, false) => HarnessState::Configured,
            (true, false, false) => HarnessState::Installed,
            (false, _, _) if configured || used => HarnessState::DataOnly,
            (false, _, _) => HarnessState::NotFound,
        };
        HarnessDetection {
            id: self.id(),
            display_name: self.display_name().to_string(),
            state,
            executable,
            version,
            install_method,
            configured,
            used,
        }
    }
}

/// Runs `<bin> --version` when both a binary and a spawner exist, and
/// infers the install method from the resolved path. Any missing
/// precondition (no binary, no spawner, a non-zero exit, a spawn error)
/// reports `Unknown` with the reason as evidence rather than guessing. The
/// probe timeout and the by-path/size/mtime cache described in
/// `docs/action-map/harnesses/harness-detection.md` are follow-up work, not
/// this function: a probe that fails to spawn simply reports `Unknown`.
fn probe_version(
    adapter: &(impl HarnessAdapter + ?Sized),
    executable: &Option<PathBuf>,
    ports: &DetectionPorts<'_>,
) -> (DetectedString, DetectedString) {
    let Some(path) = executable else {
        return (
            DetectedString::unknown("no executable on PATH"),
            DetectedString::unknown("no executable on PATH"),
        );
    };
    let Some(spawner) = ports.spawner else {
        return (
            DetectedString::unknown("no ProcessSpawner port"),
            DetectedString::unknown("no ProcessSpawner port"),
        );
    };
    let spec = ProcessSpec {
        program: path.display().to_string(),
        args: vec!["--version".into()],
        cwd: None,
        env: Vec::new(),
        timeout_ms: 2_000,
    };
    let output = match spawner.run(&spec, &NeverCancel) {
        Ok(output) => output,
        Err(err) => {
            let reason = format!(
                "{} --version failed to spawn: {}",
                path.display(),
                err.message
            );
            return (
                DetectedString::unknown(reason.clone()),
                DetectedString::unknown(reason),
            );
        }
    };
    if output.status != Some(0) {
        let reason = format!("{} --version exited {:?}", path.display(), output.status);
        return (
            DetectedString::unknown(reason.clone()),
            DetectedString::unknown(reason),
        );
    }
    let version = match adapter.parse_version(&output.stdout) {
        Some(value) => DetectedString::known(value, Evidence::verified("--version stdout")),
        None => DetectedString::unknown("--version printed nothing usable"),
    };
    (version, infer_install_method(path))
}

/// Infers an install method from the resolved executable's path. No vendor
/// marker is read here - Claude Code's own `~/.claude.json` `installMethod`
/// field is a documented but unread signal, a follow-up - so every row uses
/// the same "inferred from path" heuristic `harness-detection.md` describes
/// for Codex, OpenCode, and pi.
fn infer_install_method(path: &Path) -> DetectedString {
    let text = path.to_string_lossy();
    let method = if text.contains("Cellar") || text.contains("homebrew") {
        "homebrew"
    } else if text.contains("node_modules") || text.contains(".npm") {
        "npm"
    } else if text.contains(".cargo") {
        "cargo"
    } else if text.contains(".volta") {
        "volta"
    } else if text.contains(".bun") {
        "bun"
    } else if text.contains(".nvm") {
        "nvm"
    } else if text.contains("/Applications/") {
        "bundled app"
    } else {
        return DetectedString::unknown("resolved path matches no known install layout");
    };
    DetectedString::known(method, Evidence::inferred("resolved executable path"))
}

/// Claude Code.
pub struct ClaudeCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::CLAUDE_CODE)
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("claude")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        Some(".claude/settings.json")
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".claude/projects")
    }
}

/// Codex.
pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::CODEX)
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("codex")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        Some(".codex/config.toml")
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".codex/sessions")
    }
}

/// OpenCode.
pub struct OpenCodeAdapter;

impl HarnessAdapter for OpenCodeAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::OPEN_CODE)
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("opencode")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        Some(".config/opencode/opencode.json")
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".local/share/opencode")
    }
}

/// pi.
pub struct PiAdapter;

impl HarnessAdapter for PiAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::PI)
    }
    fn display_name(&self) -> &'static str {
        "pi"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("pi")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        Some(".pi/agent/settings.json")
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".pi/agent/sessions")
    }
}

/// Cursor. Detected as the `agent` CLI, not the editor bundle; the two are
/// two separate detections per `docs/action-map/harnesses/harness-detection.md`,
/// and this row is the CLI one.
pub struct CursorAdapter;

impl HarnessAdapter for CursorAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::CURSOR)
    }
    fn display_name(&self) -> &'static str {
        "Cursor"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("agent")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        Some(".cursor/argv.json")
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".cursor/extensions")
    }
}

/// Grok Build. Binary name and config folder are undocumented
/// (`docs/action-map/harnesses/harness-detection.md`, "Open items"); `grok`
/// is a placeholder until one is confirmed.
pub struct GrokBuildAdapter;

impl HarnessAdapter for GrokBuildAdapter {
    fn id(&self) -> AgentId {
        AgentId::from(AgentId::GROK_BUILD)
    }
    fn display_name(&self) -> &'static str {
        "Grok Build"
    }
    fn binary_name(&self) -> Option<&'static str> {
        Some("grok")
    }
    fn config_relative_path(&self) -> Option<&'static str> {
        None
    }
    fn used_relative_path(&self) -> Option<&'static str> {
        Some(".grok/sessions")
    }
}

// ---------------------------------------------------------------------------
// Claude Code adapter operations
//
// Beyond the shared `HarnessAdapter` detection recipe, Claude Code needs its
// own root resolution (honouring `CLAUDE_CONFIG_DIR` and skipping the
// reserved `synced` folder), a per-skill link switch, a `skillOverrides`
// reader/writer for `settings.json`, and a plugin cache reader. The
// transcript reader lives in `skill_uses::claude_code` and the resume/rewrite
// cache lives in `skill-studio-host`; both are shared machinery, not
// Claude-specific code, so they stay where they are.
// ---------------------------------------------------------------------------

/// `synced` under `~/.claude/skills` is reserved by Claude Code itself and
/// must never be treated as a skill folder (docs/action-map/harnesses/
/// claude-code.md, "Resolved by the docs on 2026-09-16").
const CLAUDE_RESERVED_SKILLS_ENTRY: &str = "synced";

/// Claude Code's skills root: `<CLAUDE_CONFIG_DIR>/skills` when the caller
/// supplies the env var's value, else `<home>/.claude/skills`. Reading the
/// env var itself is a host concern (core has no `std::env` access); the
/// caller resolves `CLAUDE_CONFIG_DIR` and passes it through.
pub fn claude_code_skills_root(home: &Path, config_dir_override: Option<&Path>) -> PathBuf {
    match config_dir_override {
        Some(dir) => dir.join("skills"),
        None => home.join(".claude").join("skills"),
    }
}

/// Lists the skill folder names directly under Claude Code's skills root,
/// honouring `CLAUDE_CONFIG_DIR` and skipping the reserved `synced` entry. A
/// missing root reads as no skills rather than an error, matching the rest
/// of the harness readers.
pub fn claude_code_skill_entries(
    fs: &dyn ScopeFs,
    home: &Path,
    config_dir_override: Option<&Path>,
) -> Vec<String> {
    let root = claude_code_skills_root(home, config_dir_override);
    let Ok(entries) = fs.read_dir(&root) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|e| e.name != CLAUDE_RESERVED_SKILLS_ENTRY)
        .map(|e| e.name)
        .collect()
}

/// How `~/.claude/skills/<name>` is deployed, per the desktop's
/// `ClaudeLinkState` (skill_harness_disable.rs:292): a whole-folder link to
/// the universal root, a per-skill link, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeLinkState {
    /// `~/.claude/skills` itself is a symlink (to `~/.agents/skills`).
    WholeDir,
    /// `~/.claude/skills/<name>` is its own symlink.
    PerSkill,
    /// Neither: a plain folder, or nothing at that path.
    None,
}

/// Reads the current link state at `link_path` (expected to be
/// `~/.claude/skills/<name>`), checking the parent first since a whole-dir
/// link makes every per-skill path underneath it meaningless.
pub fn claude_link_state(fs: &dyn ScopeFs, link_path: &Path) -> ClaudeLinkState {
    if let Some(parent) = link_path.parent() {
        if fs
            .symlink_metadata(parent)
            .is_ok_and(|f| f.kind == crate::ports::FileKind::Symlink)
        {
            return ClaudeLinkState::WholeDir;
        }
    }
    match fs.symlink_metadata(link_path) {
        Ok(f) if f.kind == crate::ports::FileKind::Symlink => ClaudeLinkState::PerSkill,
        _ => ClaudeLinkState::None,
    }
}

/// Removes a per-skill link, returning the link's former target so the
/// caller can record it (the desktop's registry calls this field
/// `harness_disabled`) and recreate the link later. Refuses when the
/// deployment is a whole-folder link, since there is no per-skill link to
/// remove without breaking every other skill Claude Code reads through it.
pub fn disable_claude_link(
    fs: &dyn ScopeFs,
    scope: &crate::scope::NormalizedScope,
    guard: &crate::ports::ExclusiveGuard,
    link_path: &Path,
) -> Result<PathBuf, crate::error::CoreError> {
    use crate::error::{CoreError, ErrorCode};
    match claude_link_state(fs, link_path) {
        ClaudeLinkState::WholeDir => Err(CoreError::new(
            ErrorCode::Unsupported,
            "Claude Code reads this skill through a whole-folder link, not a per-skill link - \
             it cannot be disabled without breaking every other skill under the same link",
        )
        .at(link_path)),
        ClaudeLinkState::None => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "not deployed to Claude Code as a per-skill link",
        )
        .at(link_path)),
        ClaudeLinkState::PerSkill => {
            let target = fs
                .read_link(link_path)
                .map_err(|e| CoreError::io(link_path, e))?;
            let scoped = crate::ports::confine(scope, fs, link_path)?;
            fs.remove_file(guard, &scoped)
                .map_err(|e| CoreError::io(link_path, e))?;
            Ok(target)
        }
    }
}

/// Recreates a per-skill link previously removed by [`disable_claude_link`].
/// Idempotent: a link already present at `link_path` is left untouched
/// rather than replaced, matching `unpark`'s recreate-on-restore behaviour.
pub fn enable_claude_link(
    fs: &dyn ScopeFs,
    scope: &crate::scope::NormalizedScope,
    guard: &crate::ports::ExclusiveGuard,
    link_path: &Path,
    target: &Path,
) -> Result<(), crate::error::CoreError> {
    if !matches!(claude_link_state(fs, link_path), ClaudeLinkState::None) {
        return Ok(());
    }
    let scoped_target = crate::ports::confine(scope, fs, target)?;
    let scoped_link = crate::ports::confine(scope, fs, link_path)?;
    fs.symlink(guard, &scoped_target, &scoped_link)
        .map_err(|e| crate::error::CoreError::io(link_path, e))
}

/// Reads `skillOverrides` from Claude Code's `~/.claude/settings.json`. A
/// missing file, an unparsable file, or a missing/malformed key all read as
/// no overrides, matching [`read_claude_enabled_plugins`] in `ops.rs`
/// (kept private there since it only serves `capabilities`).
pub fn read_claude_skill_overrides(
    fs: &dyn ScopeFs,
    home: &Path,
) -> serde_json::Map<String, serde_json::Value> {
    let path = home.join(".claude").join("settings.json");
    let Ok(bytes) = fs.read_capped(&path, 1024 * 1024) else {
        return serde_json::Map::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return serde_json::Map::new();
    };
    value
        .get("skillOverrides")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

/// Writes `skillOverrides` into Claude Code's `~/.claude/settings.json`,
/// preserving every other top-level key byte-for-byte (only the
/// `skillOverrides` value itself is replaced or inserted). Starts from an
/// empty object when the file is missing or unparsable, so a first write
/// still succeeds; a caller that needs to preserve a malformed file's
/// content should read it before calling this.
pub fn write_claude_skill_overrides(
    fs: &dyn ScopeFs,
    scope: &crate::scope::NormalizedScope,
    guard: &crate::ports::ExclusiveGuard,
    home: &Path,
    overrides: serde_json::Map<String, serde_json::Value>,
) -> Result<(), crate::error::CoreError> {
    use crate::error::CoreError;
    let path = home.join(".claude").join("settings.json");
    let mut doc = match fs.read_capped(&path, 1024 * 1024) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
        Err(_) => serde_json::Value::Object(serde_json::Map::new()),
    };
    if !doc.is_object() {
        doc = serde_json::Value::Object(serde_json::Map::new());
    }
    doc.as_object_mut()
        .expect("just normalized to an object")
        .insert(
            "skillOverrides".to_string(),
            serde_json::Value::Object(overrides),
        );
    let bytes = serde_json::to_vec_pretty(&doc)
        .map_err(|e| CoreError::new(crate::error::ErrorCode::Io, e.to_string()).at(&path))?;
    let scoped = crate::ports::confine(scope, fs, &path)?;
    fs.write_atomic(guard, &scoped, &bytes)
        .map_err(|e| CoreError::io(&path, e))
}

/// One [`HarnessAdapter`] per first-class harness, in the same order as
/// [`HarnessCatalog::builtin`].
pub fn builtin_adapters() -> Vec<Box<dyn HarnessAdapter>> {
    vec![
        Box::new(ClaudeCodeAdapter),
        Box::new(CodexAdapter),
        Box::new(OpenCodeAdapter),
        Box::new(PiAdapter),
        Box::new(CursorAdapter),
        Box::new(GrokBuildAdapter),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_separates_discovery_from_operations() {
        let catalog = HarnessCatalog::builtin();
        let claude = catalog.get(&AgentId::from(AgentId::CLAUDE_CODE)).unwrap();
        assert!(!claude.reads_universal_root.is_yes());
        let readers: Vec<_> = catalog
            .universal_root_readers()
            .into_iter()
            .map(|f| f.id.as_str().to_string())
            .collect();
        assert_eq!(readers, ["codex", "open-code", "pi"]);
        let cursor = catalog.get(&AgentId::from(AgentId::CURSOR)).unwrap();
        let cursor_report = CapabilityReport::from_facts(cursor, None);
        assert!(cursor_report
            .runtime_notes
            .iter()
            .any(|n| n.contains("per-skill link")));
        let report = CapabilityReport::from_facts(claude, None);
        let op = |name: &str| {
            report
                .operations
                .iter()
                .find(|o| o.operation == name)
                .map(|o| o.support.clone())
        };
        assert!(op("observe_usage").is_some_and(|s| s.is_yes()));
        assert!(op("set_invocation_policy").is_some_and(|s| s.is_yes()));
        let pi = catalog.get(&AgentId::from(AgentId::PI)).unwrap();
        assert_eq!(pi.skips_hidden_entries, Support::Unknown);
    }

    #[test]
    fn claude_code_symlink_facts_are_verified_or_names_the_unknown_row() {
        let catalog = HarnessCatalog::builtin();
        let claude = catalog.get(&AgentId::from(AgentId::CLAUDE_CODE)).unwrap();
        assert!(
            claude.follows_per_skill_link.is_yes(),
            "per-skill link support is {:?}, not Yes",
            claude.follows_per_skill_link
        );
        assert!(
            claude.follows_whole_dir_link.is_yes(),
            "whole-dir link support is {:?}, not Yes",
            claude.follows_whole_dir_link
        );
        let project_root = claude
            .roots
            .iter()
            .find(|r| r.level == ScopeLevel::Project && r.role == RootRole::Own)
            .expect("Claude Code has a project-level Own root");
        assert!(
            project_root.recursive,
            "nested `.claude/skills` discovery is documented; the project root must be recursive"
        );
        let report = CapabilityReport::from_facts(claude, None);
        assert!(
            !report
                .runtime_notes
                .iter()
                .any(|n| n.contains("per-skill link") || n.contains("whole-dir link")),
            "resolved facts must not still be reported as Unknown: {:?}",
            report.runtime_notes
        );
    }

    #[test]
    fn claude_code_roots_resolve_claude_config_dir_and_skip_the_synced_folder_or_names_the_leaking_path(
    ) {
        // No override: reads under the default `~/.claude/skills`, and
        // `synced` (a reserved folder, never a skill) is skipped.
        let fs = crate::testing::FixtureBuilder::new()
            .dir("/home/.claude/skills/foo")
            .dir("/home/.claude/skills/synced")
            .build_fs();
        let default_entries = claude_code_skill_entries(&fs, Path::new("/home"), None);
        assert_eq!(
            default_entries,
            vec!["foo".to_string()],
            "the reserved `synced` folder leaked into the skill list: {default_entries:?}"
        );

        // `CLAUDE_CONFIG_DIR` override: reads from the override, not the
        // default home path, and still skips `synced` there.
        let fs = crate::testing::FixtureBuilder::new()
            .dir("/home/.claude/skills/should-not-be-read")
            .dir("/custom/config/skills/bar")
            .dir("/custom/config/skills/synced")
            .build_fs();
        let override_entries =
            claude_code_skill_entries(&fs, Path::new("/home"), Some(Path::new("/custom/config")));
        assert_eq!(
            override_entries,
            vec!["bar".to_string()],
            "CLAUDE_CONFIG_DIR override was not honoured, or `synced` leaked: {override_entries:?}"
        );
    }

    #[test]
    fn claude_code_link_state_reads_per_skill_whole_dir_and_none() {
        let fs = crate::testing::FixtureBuilder::new()
            .dir("/home/.claude/skills/plain")
            .alias("/home/.claude/skills/linked", "/home/.agents/skills/linked")
            .dir("/home/.agents/skills/linked")
            .build_fs();
        assert_eq!(
            claude_link_state(&fs, Path::new("/home/.claude/skills/linked")),
            ClaudeLinkState::PerSkill
        );
        assert_eq!(
            claude_link_state(&fs, Path::new("/home/.claude/skills/plain")),
            ClaudeLinkState::None
        );
        assert_eq!(
            claude_link_state(&fs, Path::new("/home/.claude/skills/missing")),
            ClaudeLinkState::None
        );

        let whole_dir_fs = crate::testing::FixtureBuilder::new()
            .alias("/home/.claude/skills", "/home/.agents/skills")
            .dir("/home/.agents/skills/any")
            .build_fs();
        assert_eq!(
            claude_link_state(&whole_dir_fs, Path::new("/home/.claude/skills/any")),
            ClaudeLinkState::WholeDir,
            "a whole-folder link at the parent must be reported for every child path under it"
        );
    }
}
