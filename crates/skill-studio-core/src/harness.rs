//! Harness capability model.
//!
//! Every fact about a harness is data with evidence. Discovery support (the
//! roots a harness reads) is separate from runner, disable, and link support.
//! Adapters read the catalog; they never hard-code a harness list.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::identity::AgentId;

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
const CLAUDE_PLUGINS_REF: &str = "https://code.claude.com/docs/en/plugins-reference";
const CODEX_SKILLS_DOC: &str = "https://learn.chatgpt.com/docs/build-skills";
const CODEX_PLUGINS_DOC: &str = "https://developers.openai.com/plugins/build/plugins";
const OPENCODE_SKILLS_DOC: &str = "https://opencode.ai/v2/docs/skills/";
const OPENCODE_MIGRATE_DOC: &str = "https://opencode.ai/v2/docs/migrate-v1/";
// Cited in `docs/research/harness-primitives.md` for v1-specific facts
// (the v2 baseline facts below cite `OPENCODE_SKILLS_DOC` instead).
const OPENCODE_STORAGE_DOC: &str = "https://opencode.ai/docs/troubleshooting/";
const PI_SKILLS_DOC: &str =
    "https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md";
const PI_PACKAGES_DOC: &str =
    "https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md";
const PI_SETTINGS_DOC: &str = "https://pi.dev/docs/latest/settings";
const PI_SESSIONS_DOC: &str = "https://pi.dev/docs/latest/sessions";
const CODE_SURVEY: &str = "apps/desktop/src-tauri/src/skills/agents.rs";
const CLAUDE_TRANSCRIPT_READER: &str = "apps/desktop/src-tauri/src/skills/skill_invocations.rs";
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
        roots: vec![
            root(
                ScopeLevel::Global,
                ".claude/skills",
                RootRole::Own,
                false,
                ev(),
            ),
            root(
                ScopeLevel::Project,
                ".claude/skills",
                RootRole::Own,
                false,
                ev(),
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
        follows_per_skill_link: Support::Unknown,
        follows_whole_dir_link: Support::Unknown,
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
            shape: Support::Unknown,
            relative_path: None,
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
            // v1 layout, not re-confirmed for v2.
            shape: Support::Partial(Evidence::inferred(OPENCODE_STORAGE_DOC)),
            relative_path: Some(".local/share/opencode/storage".into()),
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
            // JSONL per working directory; a skill load is not a distinct
            // entry type.
            shape: Support::Partial(Evidence::verified(PI_SESSIONS_DOC)),
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
            shape: Support::Unknown,
            relative_path: None,
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
            shape: Support::Unknown,
            relative_path: None,
        },
        runner: RunnerSpec {
            binary: None,
            support: Support::No(ev()),
        },
    }
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
        let report = CapabilityReport::from_facts(claude, None);
        assert!(report
            .runtime_notes
            .iter()
            .any(|n| n.contains("per-skill link")));
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
}
