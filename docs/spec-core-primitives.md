# Shared core primitives

Status: draft for review. Crate: `crates/skill-studio-core`.
Inputs: `docs/research/current-primitives.md`,
`docs/research/harness-primitives.md`,
`docs/spec-headless-performance-observability.md`,
`docs/spec-workflow-contracts.md`, `docs/spec-event-store.md`.

## 1. Purpose and scope

The core crate holds the rules that every surface of Skill Studio shares:
what a skill is, where a harness reads it, what an operation may change,
and how a result is reported. The desktop app, the future CLI, and the
future TUI call the same functions. They differ only in how they get the
scope, how they show the result, and how they run the process.

In scope for this document:

- The identity model (scope, root, deployment, owner, event).
- The harness capability model.
- The ports the adapters implement.
- Phase 1 operations: `scan`, `diagnose`, `capabilities`.
- Phase 2 operations: `preview_frontmatter_repair`,
  `apply_frontmatter_repair`, `list_events`, `restore_event`.
- The error model, the result envelope, and the transport contracts.
- The migration order and the verification loop.

Out of scope: install through external CLIs, forks, packs, trials, update
checks, the agent runner, and the frontend. Section 6.3 sketches where
they go later.

Content facts are in scope but were not in phase 1. `ops::scan` classifies
a deployment: where it is, which harness reads it, who owns it. It does not
yet derive the content facts the desktop snapshot also carries - content
hash, token counts, frontmatter fields, description, file count, folder
bytes - nor `source_kind`, which needs the lock file and the ownership
ledgers.

This gap contradicted section 10 PR 3, which asked the desktop `rescan` to
call `ops::scan` and adapt the result to `SkillSnapshot`. A snapshot cannot
be built from an inventory that lacks most of its fields, so PR 3 shipped an
overlay instead: the desktop keeps its own classifier and the core overrides
three fields on it. The desktop therefore runs two scanners, which is the
duplication the extraction exists to remove. PR 8 closes the gap.

## 2. Decisions

Each decision names its reason and the alternative that was rejected.

| #   | Decision                                                                                                                                                                                                                                                                                                                                                                                                                                            | Reason                                                                                                                               | Rejected alternative                                                                                                          |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| D1  | Cargo workspace at the repo root with `crates/*` and `apps/desktop/src-tauri` as members. One `Cargo.lock` at the root.                                                                                                                                                                                                                                                                                                                             | One lock file, one `target`, one `cargo test` for the core.                                                                          | Keep the Tauri crate as the only crate and add a module. The core would then depend on `tauri` transitively.                  |
| D2  | The core never reads `HOME` and never calls `dirs::home_dir`. Every operation takes a `RuntimeScope` from the adapter.                                                                                                                                                                                                                                                                                                                              | Tests run against fixture homes; the CLI can run against a temp home; two scopes can run in one process.                             | A global `set_home()` at startup. It leaks between tests and hides the scope from the reader.                                 |
| D3  | Scope identity is `scope:v1/<sha256 of the canonical home path>`. Lease keys are the canonical roots.                                                                                                                                                                                                                                                                                                                                               | Two paths that alias one directory (symlink, case folding, mount) must get one identity and one lease.                               | Identity from the lexical path. Two processes on aliased paths would race.                                                    |
| D4  | Verification first: a fixture builder, golden snapshots, and a parity harness ship with the first PR, before any workflow moves.                                                                                                                                                                                                                                                                                                                    | Each moved workflow must show the same bytes on the wire as the desktop shows today. Without the harness the move cannot be checked. | Move the scanner first and add tests later. Drift would be found by users.                                                    |
| D5  | Harness facts are data (`HarnessCatalog`) with evidence and confidence per fact. Discovery support is a separate field from link, disable, and runner support.                                                                                                                                                                                                                                                                                      | Adapters must not hard-code harness lists. An `Unknown` fact must block a write, not be treated as `No`.                             | A 43-variant enum with four `match` blocks. Adding a fact means editing every match.                                          |
| D6  | `AgentId` is a validated kebab-case newtype (`open-code` is canonical). The catalog carries the six first-class rows.                                                                                                                                                                                                                                                                                                                               | The wire name is the identity; the CLI name is a catalog fact. Removes the three hand patches for OpenCode.                          | Keep the enum. Every new harness is a code change in the core.                                                                |
| D7  | One typed error enum with stable string codes and a fixed map to exit status. The envelope derives the exit status; the adapter never picks it.                                                                                                                                                                                                                                                                                                     | The CLI, the MCP server, and the desktop report one vocabulary.                                                                      | `Result<T, String>` as today. Callers parse messages.                                                                         |
| D8  | DTOs derive `serde` and `schemars::JsonSchema`. TypeScript types and MCP tool schemas are generated from Rust.                                                                                                                                                                                                                                                                                                                                      | One source of truth; a field change breaks the generated types, not the user.                                                        | Hand-written `skill-types.ts` mirrors. They drift silently today.                                                             |
| D9  | Reads take a shared lease; writes take an exclusive lease. Write helpers take an `ExclusiveGuard` and a `ScopedPath`, so a write without the lease or outside the scope does not compile.                                                                                                                                                                                                                                                           | The rule "no write without lock" is enforced by types, not by review.                                                                | A `Mutex<()>` in app state (`ForkMutationLock`). It does not cover the CLI.                                                   |
| D10 | Startup recovery of `pending` history rows lives outside `HistoryStore`, runs only under the exclusive guard, and never from a read.                                                                                                                                                                                                                                                                                                                | A `scan` must never write. Recovery is a mutation.                                                                                   | Recovery inside `open_event_store` at app start, as today. The CLI `scan` would then write.                                   |
| D11 | History rows keep `kind` as a string column. `EventKind` is a typed view; unknown literals load and render as not restorable.                                                                                                                                                                                                                                                                                                                       | Rows written by older versions must load without a migration.                                                                        | Migrate the column to an enum. Old rows would fail to load.                                                                   |
| D12 | `Partial` is a status, not an error. A partial `scan` returns data and exit status 4.                                                                                                                                                                                                                                                                                                                                                               | The user sees what was read and what was skipped.                                                                                    | Fail the whole scan when one root times out.                                                                                  |
| D13 | The MCP server is stateless per MCP 2026-07-28. Each tool call builds a `Runtime`, runs one operation, returns the envelope, and drops everything. No subscriptions, no per-connection cache, no session state. The TUI (TypeScript, OpenTUI) talks to the core through the CLI: one `skill-studio <op> --json` process per action, and `skill-studio watch --json` for a newline-delimited stream of `CoreNotice::Revision` while the TUI is open. | The spec forbids state inferred from earlier requests; a CLI process per call is the same model the desktop uses in-process.         | A long-lived MCP server with a `subscribe_snapshot` tool (violates the spec's statelessness); a separate TUI daemon protocol. |
| D14 | The core owns SHA-256 (in-crate).                                                                                                                                                                                                                                                                                                                                                                                                                   | The dependency list for the core is fixed; `sha2` is not on it. The routine is 80 lines and has a known-vector test.                 | Add `sha2`. Rejected only because the list was fixed; revisit in PR 4 if the list opens.                                      |
| D15 | The `testing` module ships in the core under a feature flag.                                                                                                                                                                                                                                                                                                                                                                                        | Adapters and the parity harness share one fixture builder and one normalizer.                                                        | A separate `skill-studio-testkit` crate. One more crate to version for no gain yet.                                           |

Overrides of the winning design ("verification-first"):

- `AmbiguousTarget` is exit status 2 (invalid request), not 3. The
  request named a target that does not resolve to exactly one deployment;
  the caller must fix the request. Status 3 stays for state that changed
  under the caller.
- Recovery moved out of `HistoryStore` (D10).
- `RootRef` gained `Legacy(AgentId)` and `PluginCache(AgentId)` kinds
  from the domain-first design, so the OpenCode `skill/` root and the
  plugin caches are typed, not labelled.
- `Fingerprint::parse` accepts bare hex (legacy proposals) as well as
  `sha256:<hex>`.

## 3. Identity model

One skill has one lexical name. One skill has many deployments. Each
deployment lives under one root. Each root belongs to one scope.

### 3.1 Scope

```
RuntimeScope {
  kind: Live | Fixture,
  home_root: PathBuf,                        // absolute
  projects: Explicit { paths } | Discover { exclude },
  history_root: PathBuf,                     // adapter supplies it
  history_binding: Default | Override,       // Override only in Fixture
  cache_root: Option<PathBuf>,
  data_root: Option<PathBuf>,                // forks/, runs/, update-check.json, invocation cache
  read_timeout_ms: 2000, write_timeout_ms: 10000
}
```

`NormalizedScope::normalize(scope, fs)` and
`normalize_with_discovery(scope, fs, Some(&discovery))` produce:

- `id: ScopeId` = `scope:v1/` + sha256 of the canonical home path.
- `home: PhysicalRoot { lexical, canonical }`.
- `projects`: sorted by canonical path, duplicates removed. Two aliases
  of one project collapse to one entry. Under `Discover` the
  `ProjectDiscovery` port supplies candidates; candidates that no longer
  exist are dropped and `exclude` is applied by canonical path. The list
  is fixed from here on: lease keys and `contains` never grow later.
- `history_root`, `cache_root`, `data_root`.

Invariants:

- A relative home is `invalid_scope`.
- `HistoryBinding::Override` outside `Fixture` is `invalid_scope`.
- `Discover` without a `ProjectDiscovery` port is `invalid_scope`;
  `Runtime::new` passes `Ports.discovery`.
- The home root cannot also be a project: `invalid_scope`.
- `data_root` is `None` when the adapter has no durable data directory.
  Phase 1 and 2 never read it; the operations that need it (fork, update
  check, run history, invocation cache) fail with `invalid_scope`.
- `lease_keys()` returns the canonical home and canonical projects,
  sorted. Two scopes that alias one directory return equal keys.
- `display_path(p)` renders paths under the home as `~/...`. Error
  messages use it; the core never prints the raw home.

`EffectiveScope` is the DTO copy in every envelope: `id`, `kind`,
`home`, `projects`, `history_root`.

### 3.2 Roots and deployments

```
RootRef { scope: Global | Project(ProjectRef), kind: RootKind }
RootKind = Harness(AgentId) | Universal | Parked | Legacy(AgentId) | PluginCache(AgentId)
ResolvedRoot { root: RootRef, lexical, canonical: Option<PathBuf>, is_link }
```

`Parked` exists only at `Global`: `RootRef::new` refuses
`Project(_) + Parked` with `invalid_request`, `RootRef::parked()` builds
the global one, and `scan` runs `RootRef::validate` on any value that
came in over the wire. `Legacy` covers the OpenCode `skill/` directory.
`PluginCache` covers `~/.claude/plugins/cache` and `~/.codex/plugins/cache`.
The catalog, not the scope, says which roots a harness has.

Layout names the core owns (`identity.rs`): `UNIVERSAL_ROOT_RELATIVE`
(`.agents/skills`), `PARKED_ROOT_RELATIVE` (`.agents/skills-parked`), and
`MOVE_ASIDE_DIR_NAME` (`.skill-studio-disabled`, inside a skills root).

`DeploymentDto.plugin: Option<PluginSourceDto { marketplace, plugin,
version, enabled }>` is `Some` exactly for `PluginCache` roots, per the
agent-plugins.org layout `<cache>/<marketplace>/<plugin>/<version>/skills/`.
`enabled` is Claude Code's `settings.json` `enabledPlugins["<plugin>@
<marketplace>"]` state (`None` for every other harness, or when the harness
records nothing for that id).

Deployment identity stays `dep:v1/...` (opaque newtype `DeploymentId`
with a prefix check). Owner identity stays `owner:v1/...` (`OwnerId`).
The core never parses the tail of either id; it compares them.

`DeploymentDto.harness` is `Option<AgentId>`, never a display string.
`disabled_readers` is `Vec<AgentId>`.

### 3.3 Owner and target

`LifecycleTarget` is exactly one of `Deployment(DeploymentId)` or
`Owner(OwnerId)`. `LifecycleOwnerKind::is_mutable` is true for
`SkillsSh`, `Dotagents`, `Copy`, `Fork` only.

### 3.4 Proposal, event, correlation

- `ProposalId`: sha256 over deployment id, path, owner id, owner kind,
  expected fingerprint, proposed text. Same rule as today.
- `Fingerprint`: `sha256:<hex>`. Parse accepts bare hex.
- `EventId`: ULID string. `IdSource::next_event_id` must sort after every
  earlier id.
- `CorrelationId`: request-scoped, chosen by the adapter, echoed in the
  envelope and in every `CoreNotice`. It is not an event id.

## 4. Harness capability model

`HarnessCatalog::builtin()` holds one `HarnessFacts` row per first-class
harness. Every fact carries `Evidence { source, confidence }` where
confidence is `VerifiedFromDocs`, `Inferred`, or `Unknown`.

`Support` is `Yes(ev) | No(ev) | Partial(ev) | Unknown`. `Unknown` is not
`No`. An operation that needs `Yes` returns `unsupported` on `Unknown`
and names the missing evidence in the message.

Facts per harness:

| Field                                       | Meaning                                                                                                                                                                                                                                                                   |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `roots: Vec<RootSpec>`                      | Level (global or project), relative path, role (`Own`, `Universal`, `Legacy`, `PluginCache`, `CrossHarness`), recursive flag, evidence. This is discovery support only.                                                                                                   |
| `reads_universal_root`                      | Reads `.agents/skills`.                                                                                                                                                                                                                                                   |
| `follows_per_skill_link`                    | A symlinked skill folder is read.                                                                                                                                                                                                                                         |
| `follows_whole_dir_link`                    | A skills root that is itself a symlink is read.                                                                                                                                                                                                                           |
| `skips_hidden_entries`                      | A walk skips dot-prefixed entries, so a skill under `MOVE_ASIDE_DIR_NAME` stays hidden. `Yes` (inferred) for one-level readers; `Unknown` for pi, whose walk is recursive. On `Unknown`, `scan` does not claim a moved-aside or parked skill is hidden from that harness. |
| `all_skills_disable`                        | A switch that turns every skill or the skill tool off at once. From the capability matrix; `Unknown` for every row; no operation uses it.                                                                                                                                 |
| `native_disable: Option<NativeDisableSpec>` | Mechanism, scope levels, `writable: Support`, `disabled_by` value shown to the user.                                                                                                                                                                                      |
| `invocation_control`                        | Model and user invocation switches.                                                                                                                                                                                                                                       |
| `plugin_cache`                              | Layout support and relative path.                                                                                                                                                                                                                                         |
| `usage_source`                              | Session record shape support and relative path.                                                                                                                                                                                                                           |
| `runner`                                    | Binary name and support.                                                                                                                                                                                                                                                  |

Built-in rows (sources in `harness.rs`):

| Harness       | Discovery                                                                                                                       | Universal | Native disable                                                                                                       | Notes                                                                                                                                                                                      |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------- | --------- | -------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `claude-code` | `.claude/skills` global and project; plugin cache                                                                               | No        | `skillOverrides` in `settings.json`; writable Unknown                                                                | Per-skill link support Unknown. `usage_source.shape` is `Yes` with `Inferred` evidence (`skill_invocations.rs` reads `.claude/projects` transcripts today), so `observe_usage` is allowed. |
| `codex`       | `.agents/skills`; `.codex/skills` (inferred, contradiction in issue 22590); plugin cache                                        | Yes       | `[[skills.config]]`, global only, writable Yes                                                                       | Follows per-skill links Yes                                                                                                                                                                |
| `open-code`   | `.claude/skills`, `.agents/skills`, `.config/opencode/skills`, `.opencode/skills` (v2, all recursive); legacy singular `skill/` | Yes       | v2 permission rule `{action:"skill",resource,effect}` (v1 `permission.skill`); writable Partial (`.jsonc` read-only) | Beta v2 baseline; separate `opencode2` binary during migration                                                                                                                             |
| `pi`          | `.pi/agent/skills`, `.pi/skills`, `.agents/skills`; recursive                                                                   | Yes       | `pi config` toggle; writable Unknown                                                                                 |                                                                                                                                                                                            |
| `cursor`      | `.cursor/skills` (inferred)                                                                                                     | Unknown   | none                                                                                                                 | Runner No                                                                                                                                                                                  |
| `grok-build`  | `.grok/skills` (inferred)                                                                                                       | Unknown   | none                                                                                                                 | Runner No                                                                                                                                                                                  |

`CapabilityReport::from_facts(facts, observed)` derives per-operation
support (`set_harness_enabled`, `set_claude_link`, `materialize_root`,
`run_skill_test`, `observe_usage`, `set_invocation_policy` from
`invocation_control.model_invocation`) and lists every `Unknown` in
`runtime_notes`. `HarnessObserved` adds machine facts when the caller
asks: config present, config writable, runner binary. The `capabilities`
result is `Capabilities { harnesses: Vec<CapabilityReport>, tools:
Vec<ToolAvailability { name, path }> }`; `tools` answers the request's
`tools` list through the `ToolLookup` port.

## 5. Ports

Adapters implement these traits. A port moves bytes and reports; it does
not decide policy.

| Port                                     | Methods                                                                                                                                                  | Invariant                                                                                                                                                                                                   |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ScopeFs`                                | `canonicalize`, `symlink_metadata`, `read_link`, `read_dir`, `read_capped`; writes: `write_atomic`, `rename`, `remove_file`, `create_dir_all`, `symlink` | Every write takes `&ExclusiveGuard` and `&ScopedPath`. `symlink` confines the target too; a link out of the scope is never created. `read_capped` errors on a file larger than the cap; it never truncates. |
| `confine(scope, fs, path) -> ScopedPath` | Free function                                                                                                                                            | Absolute, no `..`, inside the home or a project, and the parent's canonical form is inside the scope. Otherwise `invalid_request`.                                                                          |
| `ProjectDiscovery`                       | `discover_projects(home_root) -> Vec<PathBuf>`                                                                                                           | Optional in `Ports`. Runs inside `normalize_with_discovery` only; returns candidates, the scope filters them.                                                                                               |
| `ToolLookup`                             | `find_binary(name) -> Option<PathBuf>`                                                                                                                   | Optional in `Ports`. The core never reads `PATH`.                                                                                                                                                           |
| `Clock`                                  | `now`, `monotonic`                                                                                                                                       | Timings and budgets use `monotonic`.                                                                                                                                                                        |
| `IdSource`                               | `next_event_id`                                                                                                                                          | Sorts after every earlier id.                                                                                                                                                                               |
| `LeaseProvider`                          | `acquire(keys, mode, wait) -> LeaseHandle`                                                                                                               | Keys sorted; `scope_busy` when the wait runs out. Lease files live under the adapter's lease root, never in a project.                                                                                      |
| `acquire_shared` / `acquire_exclusive`   | Free functions over `NormalizedScope::lease_keys`                                                                                                        | Return `SharedGuard` / `ExclusiveGuard`.                                                                                                                                                                    |
| `HistoryOpener`                          | `open(scope, ReadIfExists \| ReadWrite) -> Option<HistoryStore>`                                                                                         | `ReadIfExists` never creates the database.                                                                                                                                                                  |
| `HistoryStore`                           | `list`, `get`, `backup_paths`, `record`, `finish`, `claim_revert`, `pending`                                                                             | `record`, `finish`, `claim_revert` take `&ExclusiveGuard`. `claim_revert` sets `reverted_by` atomically and fails with `already_reverted`.                                                                  |
| `EventSink`                              | `notify(CoreNotice)`                                                                                                                                     | Notices: `OpState`, `Progress`, `Invalidated { skills, projects }`, `Revision`, `Recovered { events }`.                                                                                                     |
| `ProcessSpawner`                         | `run(&ProcessSpec, &dyn CancelToken) -> ProcessOutput`                                                                                                   | Optional in `Ports`. Blocks until exit; one output. Phase 1 and 2 do not use it. A streaming variant is a later-phase port (6.3).                                                                           |
| `CancelToken`                            | `is_cancelled`                                                                                                                                           | `OpContext::checkpoint()` returns `cancelled`.                                                                                                                                                              |

`Ports { fs, clock, ids, leases, history, sink, spawner, discovery, tools, catalog }` and
`Runtime { scope: NormalizedScope, ports }` are what an operation
receives. `MutationSession::begin(rt, ctx)` acquires the exclusive
guard, opens the store `ReadWrite`, runs recovery, and rescans; it exposes
`resolve_exact(id)` (`ambiguous_target` on zero or many) and `finish`.

## 6. Operations

Every operation has the shape
`fn op(rt: &Runtime, ctx: &OpContext, req: &Request) -> Result<T, CoreError>`.
`T` implements `Outcome`, which reports `Partial` and `found_issues`.

### 6.1 Phase 1

| Operation      | Request                                             | Preconditions                                                                                  | Result                                                                                                                                                                     | Error codes                                                                                                         |
| -------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `scan`         | `ScanRequest { skills, timings }`                   | Shared lease inside `read_timeout`. Never creates the history database, a cache, or a watcher. | `Inventory { skills, projects, completeness, observations, timings }`. Skills sorted by name. Roots not read inside the budget appear in `observations` and set `Partial`. | `invalid_scope`, `scope_busy`, `io`, `cancelled`                                                                    |
| `diagnose`     | `ScanRequest`                                       | Same as `scan`. Issue derivation is pure over the inventory.                                   | `Diagnosis { inventory, issues }`. Issues sorted by severity, skill, kind. Each issue names a `NextAction` with a `DeploymentId`.                                          | Same as `scan`                                                                                                      |
| `capabilities` | `CapabilitiesRequest { harnesses, observe, tools }` | None. With `observe = false` and no `tools` nothing is read.                                   | `Capabilities { harnesses, tools }`; harnesses in catalog order, tools in request order.                                                                                   | `invalid_request` (harness without a catalog row), `unsupported` (`tools` without a `ToolLookup` port), `cancelled` |

### 6.2 Phase 2

| Operation                    | Request                                                  | Preconditions                                                                                                                                                                                                                                                                                                                                                                  | Result                                                                                                                                                                                                                                                       | Error codes                                                                            |
| ---------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| `preview_frontmatter_repair` | `RepairPreviewRequest { deployment_id }`                 | Shared lease. Target resolves exactly once. Deployment is mutable.                                                                                                                                                                                                                                                                                                             | `FrontmatterRepairPreview { proposal_id, deployment_id, path, scope, reason, owner_id, owner_kind, expected_fingerprint, proposed_fingerprint, original_content, proposed_content, diff, allowed_apply_modes, managed_update_warning }`. Nothing is written. | `scope_busy`, `ambiguous_target`, `unsupported` (read-only or no supported fix), `io`  |
| `apply_frontmatter_repair`   | `RepairApplyRequest { preview, mode }`                   | Exclusive lease. Fresh rescan. `expected_fingerprint` still matches, else `stale_proposal`. Owner id and kind unchanged, else `ownership_changed`. `mode` in `allowed_apply_modes`, else `invalid_request`. The repair is deterministic: apply recomputes the proposal from the bytes on disk and compares `proposal_id`; `preview.proposed_content` is never written as sent. | `RepairOutcome::Applied { event_id, deployment_id }` or `AlreadyApplied { deployment_id }`. Records `repair_skill_frontmatter` as `pending` before the write, backs up the file, writes atomically, then `finish`.                                           | `scope_busy`, `stale_proposal`, `ownership_changed`, `invalid_request`, `io`           |
| `list_events`                | `ListEventsRequest { skill, limit, after, check_drift }` | None. Opens the store `ReadIfExists`; an absent store returns an empty list. `after` is a cursor: only rows older than that id. `check_drift` reads every touched path.                                                                                                                                                                                                        | `Vec<EventDto>` newest first. `restore` says `Yes`, `Reverted { by }`, `NoInverse`, or `UnknownKind`. `drift` is `Unchecked`, `Clean`, or `Drifted`.                                                                                                         | `io`                                                                                   |
| `restore_event`              | `RestoreRequest { event_id, force }`                     | Exclusive lease. `claim_revert` succeeds, else `already_reverted`. Live fingerprints match the recorded ones, else `drift_conflict` unless `force`; with `force` the drifted bytes are backed up first.                                                                                                                                                                        | `RestoreOutcome { restore_event_id, reverted_event_id, restored_paths }`. The restore is an event with its own backup.                                                                                                                                       | `scope_busy`, `already_reverted`, `drift_conflict`, `unsupported` (unknown kind), `io` |

### 6.3 Later phases (sketch)

Rows are from `docs/spec-workflow-contracts.md`. Each row names the core
operation it becomes, the lease it needs, and the primitive that must
exist first when the crate does not have it yet ("Needs first"; empty
when phase 1 and 2 primitives are enough).

| Workflow row                                            | Core operation                                                         | Lease                      | Phase   | Needs first                                                                                                                                                                   |
| ------------------------------------------------------- | ---------------------------------------------------------------------- | -------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Manual rescan                                           | `scan`                                                                 | shared                     | 1       |                                                                                                                                                                               |
| Home dashboard, skills list, coverage matrix            | `diagnose` + adapter projection                                        | shared                     | 1       |                                                                                                                                                                               |
| Skill details, compare deployments                      | `read_skill`, `compare_deployments`                                    | shared                     | 3       | `SkillContentDto { body, provenance, update_status }`; `DeploymentComparison` DTO                                                                                             |
| Edit installed SKILL.md, accept/reject audit hunks      | `write_skill_content` (guarded by fingerprint)                         | exclusive                  | 3       | `WriteSkillContentRequest { deployment_id, expected_fingerprint, content }` and its outcome DTO                                                                               |
| Native harness visibility                               | `set_harness_enabled`                                                  | exclusive                  | 3       | `NativeConfigStore` port for Codex `config.toml` and OpenCode `opencode.json(c)`; TOML and JSONC crates added to the 11.4 list                                                |
| Disable/restore an eligible independent deployment      | `move_aside`, `move_aside_restore`                                     | exclusive                  | 3       |                                                                                                                                                                               |
| Change invocation policy                                | `set_invocation_policy`                                                | exclusive                  | 3       | `InvocationPolicy` DTO; YAML write for Codex `agents/openai.yaml` through `NativeConfigStore`                                                                                 |
| Park / Unpark                                           | `park`, `unpark`                                                       | exclusive                  | 3       |                                                                                                                                                                               |
| Materialize a linked root                               | `materialize_root`                                                     | exclusive                  | 4       |                                                                                                                                                                               |
| Make independent copy                                   | `make_independent_copy`                                                | exclusive                  | 4       | `copy_tree` helper over `ScopeFs` (composable today; the helper removes repetition)                                                                                           |
| Relink / remove a broken link                           | `repair_link`                                                          | exclusive                  | 4       |                                                                                                                                                                               |
| Remove a deployment, delete pack, delete scratch folder | `remove_deployment`, `remove_pack`                                     | exclusive + spawner        | 5       | `ScopeFs::remove_dir_all(guard, ScopedPath)` for `Copy`, `Fork`, and `Manual` deployments                                                                                     |
| Update managed skill, check for updates                 | `update_skill`, `check_updates`                                        | exclusive + spawner        | 5       | `data_root/update-check.json`; `UpdateCheckSummary` DTO; a `GithubFetch` port over the spawner                                                                                |
| Fork, pull upstream, un-fork                            | `fork`, `pull_fork`, `unfork`                                          | exclusive + spawner        | 5       | `data_root/forks` base snapshots; `GithubFetch` port; `ForkMergeReport` conflict DTO; `remove_dir_all`                                                                        |
| Plugin skills view                                      | `diagnose` + adapter projection                                        | shared                     | 1       | (`DeploymentDto.plugin` carries marketplace, plugin, version)                                                                                                                 |
| Add skill from a pasted or untrusted source             | `add_skill`                                                            | exclusive + spawner        | 5       | `Confirm` port for the trust prompt; per-item `CoreNotice::ItemOutcome`; `CoreNotice::NeedsConfirmation`                                                                      |
| Create, update, publish pack                            | `create_pack`, `update_pack`, `publish_pack`                           | exclusive + spawner        | later   | `copy_tree`; git through the spawner; pack registry under `data_root`; `Confirm` port for publish                                                                             |
| Ask assistant, audit skill, test skill, cancel run      | `run_agent`, `cancel_run`                                              | shared + streaming spawner | later   | `StreamingProcessSpawner` with a line stream and session id; `CoreNotice::AgentEvent { run_id, seq, at, kind }`; terminate, grace, kill sequence in the core                  |
| Choose test target, apply/discard test changes          | `run_agent` targets                                                    | shared + spawner           | later   | `RunTarget` DTO over `cache_root` scratch and worktrees; in-place diff through git                                                                                            |
| Run history, transcripts                                | `list_runs`, `read_run`                                                | none                       | later   | `data_root/runs`; `SkillRunRecord` and `SkillRunSummary` DTOs                                                                                                                 |
| Invocation activity, heatmap, dashboard usage tiles     | `observe_usage`                                                        | shared                     | later   | `SkillInvocation`, `InvocationHeatmap`, `SkillInvocationStats` DTOs; invocation cache under `data_root` (Claude Code evidence is now `Inferred`, so the operation is allowed) |
| Trial expiry (`skills://trial-expired`)                 | adapter only                                                           |                            | adapter | Trials stay outside the core; the desktop keeps emitting the event.                                                                                                           |
| Add a tracked project, stop tracking                    | adapter preference; changes `RuntimeScope.projects`                    | none                       | adapter | A `PreferenceStore` port when the CLI needs shared, versioned project preferences                                                                                             |
| Browse, search, install through skills.sh               | out of scope for the core until the spawner port is used in production |                            | later   |                                                                                                                                                                               |
| Reveal folder, open editor                              | adapter only                                                           |                            | adapter |                                                                                                                                                                               |

## 7. Error codes and exit status

| Code                | Meaning                                                                 | Exit |
| ------------------- | ----------------------------------------------------------------------- | ---- |
| `invalid_request`   | Malformed request, unknown target, path outside scope, mode not allowed | 2    |
| `invalid_scope`     | Relative home, override outside fixture mode, missing root              | 2    |
| `ambiguous_target`  | Target resolves to zero or more than one deployment                     | 2    |
| `unsupported`       | The harness fact is `No` or `Unknown` for this action                   | 2    |
| `execution_failed`  | A child process failed                                                  | 2    |
| `io`                | Filesystem or database failure                                          | 2    |
| `scope_busy`        | Lease wait ran out                                                      | 3    |
| `stale_proposal`    | File changed since the preview                                          | 3    |
| `drift_conflict`    | Live bytes differ from the recorded fingerprint                         | 3    |
| `ownership_changed` | Owner id or kind changed since the preview                              | 3    |
| `already_reverted`  | Another restore claimed the event                                       | 3    |
| `incomplete`        | A root was not read; result is partial                                  | 4    |
| `cancelled`         | The caller cancelled                                                    | 130  |

Exit 0 is a full result. Exit 1 is a full `diagnose` that found at least
one warning or error. `Partial` wins over issues. `ResultEnvelope::exit_status`
is the only place this table is applied.

`CoreError { code, message, path, source }`. `sanitized(scope)` yields
`ErrorEntry { code, message, path }` with the home rendered as `~`.

## 8. Transport contracts

### 8.1 CLI envelope

```json
{
  "schema_version": 1,
  "operation": "scan",
  "scope": { "id": "scope:v1/...", "kind": "live", "home": "...", "projects": [], "history_root": "..." },
  "status": "ok" | "partial" | "error",
  "data": { ... } | null,
  "errors": [ { "code": "scope_busy", "message": "...", "path": "~/.agents/skills" } ],
  "correlation_id": "...",
  "event_id": "01J..." | null
}
```

The CLI is synchronous. It builds a `RuntimeScope` from flags
(`--home`, `--project`, `--fixture`), calls one operation, prints the
envelope, and exits with `exit_status()`. It never prompts in JSON mode.

Confirmed against real `apps/cli` output (PR 4): `skill-studio scan
--fixture <dir> --json` prints exactly this shape and field order,
including `status: "error"`/`data: null` for a scope that fails to
normalize (e.g. `--fixture /nonexistent`, `invalid_scope`, exit 2) and
`status: "error"`/`errors: [{ "code": "scope_busy", ... }]` (exit 3) for a
lease held by another process.

### 8.2 MCP tools

One tool per `Operation`. Input schema is the request DTO's JSON Schema;
output is the envelope. Tool names equal the `Operation` serde names. The
server is built on `rmcp` over stdio and is stateless, per MCP 2026-07-28:
every call builds a `Runtime`, re-reads disk, runs one operation, and
drops the `Runtime`. The server holds no state between calls; a restart
between two calls must give identical results. Each call carries its own
`correlation_id`; progress uses the request-scoped `_meta.progressToken`
only, mapped from `CoreNotice::Progress`.

Confirmed against real `apps/mcp` output (PR 6): tool names are exactly the
`Operation` serde names (`scan`, `diagnose`, `capabilities`,
`preview_frontmatter_repair`, `apply_frontmatter_repair`, `list_events`,
`restore_event`); each tool's input schema is `Parameters<T>` over the
core's own request DTO (`ScanRequest`, `CapabilitiesRequest`,
`RepairPreviewRequest`, `RepairApplyRequest`, `ListEventsRequest`,
`RestoreRequest`) with no redeclaration; each tool's `CallToolResult`
carries the envelope as `structured_content`, with `is_error` set from
`envelope.status == "error"`. The core does not currently call
`EventSink::notify(CoreNotice::Progress)` from inside any op, so the MCP
adapter brackets each call with a start (`0.0`) and done (`1.0`) progress
notification itself, sent only when the caller supplied
`_meta.progressToken` - see `apps/mcp/README.md`.

### 8.3 TUI transport

The TUI spawns the CLI per action (`skill-studio <op> --json`) and parses
the envelope; it holds no long-lived connection to the core. For
liveness it runs `skill-studio watch --json --since <revision>`, which
holds a shared lease, watches the scope roots, and prints one JSON line
per change to stdout. Each line carries both the new revision and the
inventory that revision names:

```json
{"revision": 2, "inventory": { "skills": [ ... ] }}
```

The TUI applies that inventory directly. It does not re-run `scan` on a
watch line: the watcher already holds the inventory it compared against,
and a second scan would race it, reading newer bytes and labelling them
with the older revision it was handed. `--since <revision>` suppresses the
first line when the caller already holds that revision.

The `watch` process is a plain CLI process, not MCP; it does not
participate in the MCP server's request/response cycle.

## 9. Mapping from current types to new types

| Current                                                                                                                                                                                         | New                                                                                                                      | Note                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dirs::home_dir()` in `agents.rs`, `skill_refresh.rs`                                                                                                                                           | `RuntimeScope.home_root`                                                                                                 | Adapter passes it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `SkillRoot { label, project_path, path }`                                                                                                                                                       | `RootRef` + `ResolvedRoot`                                                                                               | Label string becomes `RootKind`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `skill_roots()`                                                                                                                                                                                 | `HarnessCatalog` roots + `RootKind::{Universal, Parked}`                                                                 | Roots derive from facts.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `AgentId` (43-variant enum)                                                                                                                                                                     | `AgentId` newtype + catalog                                                                                              | Non-first-class ids still validate; they have no catalog row.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `FIRST_CLASS_AGENTS`, `CHECKED_HARNESSES`                                                                                                                                                       | `HarnessCatalog::builtin().facts`                                                                                        | One list.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `Deployment.agent: String`                                                                                                                                                                      | `DeploymentDto.harness: Option<AgentId>`                                                                                 | Wire change; generated TS follows.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `PluginSkill { marketplace, plugin, version }`                                                                                                                                                  | `DeploymentDto.plugin: Option<PluginSourceDto>`                                                                          | Plugin-cache deployments join the inventory instead of a separate list.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `app.path().app_data_dir()` for forks, runs, update check, invocation cache                                                                                                                     | `RuntimeScope.data_root`                                                                                                 | Adapter passes it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `project_discovery.rs` transcript walk                                                                                                                                                          | `ProjectDiscovery` port called by `normalize_with_discovery`                                                             | Runs once per runtime, never inside `scan`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `add_method_defaults.rs` `PATH` lookup for `npx`, `dotagents`                                                                                                                                   | `ToolLookup` port + `CapabilitiesRequest.tools`                                                                          |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `disabled_readers: Vec<String>`                                                                                                                                                                 | `Vec<AgentId>`                                                                                                           |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `DisabledBy` (4 values)                                                                                                                                                                         | `DisabledBy` (6 values)                                                                                                  | Adds `claude-skill-overrides`, `pi-settings`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `deployment_id`, `owner_id` strings                                                                                                                                                             | `DeploymentId`, `OwnerId`                                                                                                | Same bytes, prefix-checked.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `LifecycleTarget { deployment_id?, owner_id? }`                                                                                                                                                 | `LifecycleTarget::{Deployment, Owner}`                                                                                   | Exactly-one enforced by the type.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `apply_frontmatter_repair { target: LifecycleTarget, proposal_id, expected_content_fingerprint, mode }`                                                                                         | `RepairApplyRequest { preview, mode }`                                                                                   | Narrowing: the core applies to a deployment only; `Owner` targets are resolved by the adapter until `resolve_target` moves in phase 3.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `FrontmatterRepairPreview { deployment_id, path, scope, reason, expected_content_fingerprint, proposal_id, original_content, proposed_content, allowed_apply_modes }`                           | `FrontmatterRepairPreview` with `owner_id`, `owner_kind`, `proposed_fingerprint`, `diff`, `managed_update_warning` added | Wire change: five added fields; `scope` is a typed `RootScope`; `expected_content_fingerprint` is renamed `expected_fingerprint`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `get_agent_capabilities` list                                                                                                                                                                   | `Capabilities { harnesses, tools }`                                                                                      | Wire change: tool availability is part of the result, not a separate call.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `SkillEventDto`                                                                                                                                                                                 | `EventDto` with `drift`                                                                                                  | Adds `drift: unchecked \| clean \| drifted`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `expected_content_fingerprint: String`                                                                                                                                                          | `Fingerprint`                                                                                                            |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `EventDraft`, `EventRow`, `SkillEventDto`                                                                                                                                                       | `EventDraft`, `EventRecord`, `EventDto`                                                                                  | Same columns.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `kind: &str` literals in 8 files                                                                                                                                                                | `EventKind::as_str`                                                                                                      | Same literals.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `SkillSnapshot.revision: u64`                                                                                                                                                                   | `Revision` + `SnapshotCell`                                                                                              |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `ForkMutationLock`, `rebuild_lock`                                                                                                                                                              | `LeaseProvider` + `ExclusiveGuard`                                                                                       | Cross-process.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `Result<T, String>` commands                                                                                                                                                                    | `Result<T, CoreError>` + `ResultEnvelope`                                                                                | Commands wrap.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `open_event_store` recovery at startup                                                                                                                                                          | `events::recover_interrupted` inside `MutationSession::begin`                                                            | Never on read.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `skill-types.ts` hand mirrors                                                                                                                                                                   | generated from `schemars`                                                                                                | PR 3.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `skill-lifecycle-target.ts` resolution rules                                                                                                                                                    | `MutationSession::resolve_exact` and later `resolve_target`                                                              | Moves to Rust in phase 3.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `Deployment.symlink_target: Option<String>` (canonical resolved path when the link resolves; a raw, lexically-uncollapsed `parent.join(target)` when broken)                                    | `DeploymentDto.link_target: Option<PathBuf>`                                                                             | Core's `process_entries` now derives `link_target` the same way (canonicalize when possible, otherwise a raw un-collapsed join off the entry's own parent) and the parity test compares the exact string on both sides. No exclusion remains for this field.                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `PluginInfo { name, version, harness }`                                                                                                                                                         | `PluginSourceDto { marketplace, plugin, version }`                                                                       | Core adds `marketplace`; no desktop wire equivalent yet. Live exclusion, still open.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `classify_lifecycle_owner` (ledger files: `dotagents_ledger.rs`, `skill_fork_registry.rs`) + `skill_assembly::propagate_verified_linked_owners`                                                 | `ops::classify_owner` + `ops::propagate_verified_linked_owners`                                                          | Core now mirrors the desktop's _reachable_ precedence given the fixture set: `Plugin` > `SkillsSh` (ledger lookup scoped to genuinely universal-rooted entries) > `InRepo` > `Manual`, plus the same post-scan pass that promotes a verified `LinkedTo` universal deployment to its canonical sibling's owner and forces it `ReadOnly`. `Dotagents`, `WildcardDotagents`, `Ambiguous`, and `Copy` stay unimplemented (no dotagents-ledger or copy-record reader in core); `Fork` stays unimplemented per its own row below. None of the 11 fixtures create a dotagents ledger, fork, or copy record, so these are documented gaps, not live exclusions - the parity test passes without them. |
| `id_for_candidate` (`destination`/`backing` follow a per-harness symlink into the universal root or a whole-dir link; `Independent` for untracked copies; `LinkedTo` carries a `deployment_id`) | `ScanTarget`'s per-entry `destination`/`backing` derivation in `ops::process_entries`                                    | Core now derives `destination`/`backing`/`mutability` the same link-aware way as `id_for_candidate` (universal root or `Parked` scope -> `Canonical`; a symlink resolving under `.agents/skills`, or a whole-dir link -> `LinkedTo`; everything else -> `Independent`), including plugin-cache entries. The one remaining difference: core's `BackingRelationship::LinkedTo` carries no `deployment_id` payload (a plain enum, not a struct variant); the parity test compares only the variant name.                                                                                                                                                                                         |
| `skill_discovery::read_skill_md` (truncates a `SKILL.md` over the 2 MiB cap and keeps the skill, `skill_md_truncated: true`)                                                                    | `ops::read_skill_md`                                                                                                     | PR 4 added `ScopeFs::read_prefix` (reads up to a limit and reports whether it truncated, unlike the all-or-nothing `read_capped`), implemented in `RealFs` and `FixtureFs`. `read_skill_md` now uses it: an over-cap `SKILL.md` is truncated and its skill kept, matching the desktop, with the truncation recorded as an `Observation` (not `spec_violations`, so the parity test's row comparison is unaffected) rather than dropping the skill or marking the scan `Partial`. No exclusion remains in the parity test.                                                                                                                                                                     |

## 10. Migration order

Each PR is small enough to review in one sitting and leaves the desktop
green.

| PR  | Content                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Exit check                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Root workspace, lock move, `skill-studio-core` skeleton with all modules, `testing` fixture builder, this spec. No Tauri source change.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | `cargo check -p skill-studio-core`, `cargo test -p skill-studio-core`, `cargo check -p skill-studio` unchanged.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 2   | `scan` implemented in the core over `ScopeFs`. `RealFs`, `SystemClock`, `UlidIds`, `FileLease` adapters in a new `crates/skill-studio-host` (std only). Parity test: desktop scanner output and core `scan` output normalized and compared on the fixture set, including link-aware `destination`/`backing`/`mutability`, `owner_kind`/`owner_id`, and exact `link_target`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Done 2026-09-09. `cargo fmt --all --check`: PASS. `cargo clippy --workspace --all-features --all-targets -- -D warnings`: PASS. `cargo test -p skill-studio-core --all-features`: PASS (includes `tests/scan_golden.rs`, all 11 fixtures, materialized-vs-golden and materialized-vs-in-memory). `cargo test -p skill-studio-host`: PASS. `cargo test -p skill-studio --test core_scan_parity`: PASS (11 of 11 fixtures asserted row-for-row equal except `oversize_skill_md`, whose skill is dropped by design - see section 9 - and `plugin.marketplace`/`backing`'s `deployment_id` payload, both documented wire-only exclusions). `cargo check -p skill-studio`: PASS.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 3   | Tauri crate depends on the core. `rescan` command calls `ops::scan` in-process and adapts to `SkillSnapshot`. TS types generated from `schemars`; `skill-types.ts` replaced by the generated file.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | `npm run check` green; snapshot bytes identical on the manual checklist.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 4   | `diagnose`, `capabilities`; `apps/cli` with `scan`, `diagnose`, `capabilities`, `schema`. Also fixed `oversize_skill_md`: `ScopeFs::read_prefix` truncates and keeps the skill instead of dropping it, marked with an `Observation`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Done 2026-09-09. `cargo fmt --all --check`: PASS (for every file this PR touched; two pre-existing unrelated diffs under `apps/desktop/src-tauri` belong to a concurrent PR). `cargo clippy --workspace --all-features --all-targets -- -D warnings`: PASS. `cargo test -p skill-studio-core --all-features`: PASS (73 unit tests, plus `tests/scan_golden.rs` and the new `tests/diagnosis_golden.rs`, all 11 fixtures). `cargo test -p skill-studio-cli`: PASS (`tests/envelope.rs`: exit 0 clean scan, 1 diagnose-with-warnings, 4 forced-partial scan via `--read-timeout-ms 0`, 2 nonexistent fixture, 3 lease held by another process, plus the schema-snapshot test against the checked-in `crates/skill-studio-core/schema/*.schema.json`). `cargo test -p skill-studio --test core_scan_parity`: PASS (`oversize_skill_md` exclusion removed; 11 of 11 fixtures row-for-row equal). `cargo test --workspace --all-features`: PASS.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 5   | Phase 2 operations; `HistoryStore` adapter over the existing SQLite schema; recovery moved into `MutationSession::begin`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | Restore parity on recorded fixtures.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 6   | MCP server (stateless, `rmcp`) and `skill-studio watch`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Done 2026-09-09. `cargo fmt --all --check`: PASS for every file this PR touched (one pre-existing unrelated diff under `crates/skill-studio-core/src/events.rs` belongs to a concurrent PR, not this one). `cargo clippy --workspace --all-features --all-targets -- -D warnings`: PASS. `cargo test -p skill-studio-mcp`: PASS (`tests/mcp_server.rs`, 8 tests: restart equivalence, statelessness within one process, `invalid_scope` error mapping, progress with and without a token, `SKILL_STUDIO_HOME` never touching the real ambient data root, and `watch` agreeing with a fresh MCP `scan` after a CLI-applied repair). `cargo test -p skill-studio-cli`: PASS (adds no new failures; `watch` itself is exercised by the MCP crate's parity test, since it needs a peer MCP process to compare against). Restart-between-calls proof: `apps/mcp/tests/mcp_server.rs::restart_gives_the_same_envelope_as_the_first_run` spawns the real `skill-studio-mcp` binary, calls `scan`, exits it, spawns a fresh instance, calls `scan` again, and asserts the two envelopes are equal after blanking `correlation_id`. `watch`/MCP parity: since each MCP call is an independent, stateless `Runtime` with no revision authority of its own (only `watch`'s local, per-process `SnapshotCell` has one), "the same revision" is proven as "the same state": `watch --json` and a fresh MCP `scan` are asserted to report the same post-mutation inventory, both differing from the pre-mutation one. |
| 7   | OpenTUI TUI at `apps/tui` (Bun, React, `@opentui/core`/`@opentui/react` 0.5.9), talking to the CLI only: one `skill-studio <op> --json` child per action plus one long-lived `watch` child. Three screens (inventory, detail, issues) and a status line carrying revision, last refresh and watch state.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | `bun test` in `apps/tui` (15 tests, including one that spawns the real built CLI against a temporary home) and `tsc --noEmit` both pass. Verified by hand under a pty against a scratch home: the inventory rendered, the revision reached 1, and the status line moved from `watch: connecting` to `watch: connected`. Two notes for the reader: `@opentui/core` is pinned to 0.5.9, not the 0.5.11 first specified, because every published `@opentui/react` pins its own exact `@opentui/core` version; and `apps/tui` and its dependencies are absent from `package-lock.json`, which the migration was forbidden to touch, so the repo installs with bun and not with `npm ci` until someone runs `npm install`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 8   | Close the phase 1 content gap and delete the duplicate scanner. `ops::scan` derives content hash, token counts, frontmatter fields, description, file count and folder bytes through `ScopeFs`. It also takes over ownership: `classify_owner` today reads only the skills.sh lock file and can return just `Plugin`, `SkillsSh`, `InRepo` and `Manual`, so it must read the dotagents ledger (`agents.toml`, `agents.lock`) and the copy records through ports, with the desktop's precedence, and return `Dotagents`, `WildcardDotagents`, `Copy`, `Fork` and `Ambiguous` where the desktop does. `source_kind` moves the same way. `assemble_installed_skills` then builds `SkillSnapshot` from the core `Inventory` alone, with no locally computed classification and no overlay. `skill_discovery.rs` and `provenance.rs` are deleted; `skill_refresh.rs` keeps only caching, timing and the Tauri event plumbing. | `cargo test --workspace --all-features` green, including the three parity suites unchanged, which is what proves the deletion safe. The ownership fixtures added after the adversarial review are inverted: each one asserted that core reports a weaker owner kind than the desktop, and must now assert both sides agree. Every `LifecycleOwnerKind` variant has at least one fixture; a parity suite proves agreement only over the cases it contains. Snapshot bytes identical on the section 11.5 manual checklist. No `skill_studio_core` symbol appears in the desktop outside the adapter that turns an `Inventory` into a `SkillSnapshot`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |

## 11. Verification loop

### 11.1 Fixture builder

`testing::FixtureBuilder` describes a home in memory: `dir`, `file`,
`alias`. `build_fs()` yields `FixtureFs: ScopeFs`. Aliases resolve on the
lexical prefix and on the resolved prefix, so `~/work/app-link` and
`/vol/homes/alice/work/app` canonicalize to one path. PR 2 adds
`materialize(temp_dir)` for the real-filesystem adapters.

### 11.2 Golden snapshots

Each fixture has one golden `Diagnosis` JSON. `testing::Normalizer`
rewrites the fixture root to `$HOME` and keeps array order. A golden
change is a reviewed diff, never a regenerate-and-commit.

### 11.3 Parity

PR 2 runs the current desktop scanner and `ops::scan` on the same
materialized fixture and compares the normalized JSON. Every difference
is either a bug in the core or a documented wire change listed in
section 9.

### 11.4 Static gates

- `#![deny(missing_docs)]` and `#![forbid(unsafe_code)]` in the core.
- `cargo deny` or a `Cargo.toml` review: the core lists only `serde`,
  `serde_json`, `schemars`, `thiserror`, `chrono`, `ulid`, plus `serde_yaml`
  and `toml` for frontmatter and lock-file parsing (PR 2). Dev-only:
  `skill-studio-host`, `tempfile`, `similar` (golden-snapshot diffing).
- A grep gate: no `dirs::`, `std::env::var("HOME")`, or `home_dir` in
  `crates/skill-studio-core/src`.
- `cargo clippy -D warnings`, `cargo fmt --check`.
- A schema snapshot: `schemars` output for every request and result DTO
  is checked in; a change is a reviewed diff.

### 11.5 Manual fixture-mode checklist

Run the desktop with `--fixture <dir>` (PR 3) against the checked-in
fixtures and confirm:

1. Dashboard counts equal the golden `Diagnosis` counts.
2. A broken link shows `broken_link` with the `repair_link` action.
3. Preview repair on the malformed frontmatter fixture shows the golden
   diff; apply writes the golden bytes and one `repair_skill_frontmatter`
   event.
4. History lists that event; restore puts the bytes back and lists a
   `restore` event.
5. Start a second desktop on the same fixture during apply: it reports
   `scope_busy` and no partial write.
6. Rename the fixture home through a symlink alias: scope id and history
   are unchanged.

## 12. Open questions

1. Claude Code per-skill link support (`follows_per_skill_link`) is
   `Unknown` in the docs. Skill Studio creates these links today. A
   behavioural test against the installed CLI would settle it.
2. Codex reads `.codex/skills` per issue 22590 but the docs name only
   `.agents/skills`. The catalog keeps both with `Inferred` confidence.
3. `pi config` writes the disable toggle; the settings key is not
   documented. `writable` stays `Unknown` until it is.
4. `ScopeId` hashes the canonical home only. Two scopes with the same
   home and different project lists share one id. Is that the intended
   identity for history, or should projects be part of it?
5. Exit status 1 is reserved for `diagnose` with issues. Should `scan`
   with issues also use it, or stay at 0?
6. The `testing` module is feature-gated in the core (D15). If a third
   consumer appears, split it into `skill-studio-testkit`.
7. `rmcp` (https://github.com/modelcontextprotocol/rust-sdk) is the MCP
   SDK to adopt in PR 6; it serves MCP 2026-07-28 statelessly and takes
   `schemars` schemas. The exact crate version is unconfirmed.

## Coverage review

Adversarial review dated 2026-09-09 against `docs/spec-workflow-contracts.md`
("Current user workflow inventory"), `docs/research/current-primitives.md`
(critic findings), `docs/research/harness-primitives.md` (capability
matrix), and the code under `crates/skill-studio-core/src`.

"Expressible" means: the workflow can be written as a call over `Runtime`,
`Ports`, the identity types, and the DTOs in this crate, with no Tauri
type. "Adapter" means the row is UI or OS glue by design and needs no core
primitive. A row marked "no" names the primitive that is absent.

### Workflow inventory

| Workflow row                                       | Expressible   | Missing primitive                                                                                                                                                                                                                                                              |
| -------------------------------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Home dashboard                                     | partial       | `diagnose` covers inventory and issues. No DTO carries usage projections or update status (`UpdateCheckSummary`, `SkillInvocationStats`); no place to store them (`data_root`, see G2).                                                                                        |
| Skills list, query, filters, selection             | yes           | Adapter projection over `Diagnosis`. Display `SourceKind` is derivable from `owner_kind`.                                                                                                                                                                                      |
| Coverage matrix                                    | yes           | Catalog `roots` + `DeploymentDto.disabled_readers`.                                                                                                                                                                                                                            |
| Add a tracked project                              | adapter       | No `PreferenceStore` port; the headless spec asks for shared, versioned project preferences. `ProjectSelection::Discover` has no port that discovers projects (see G1).                                                                                                        |
| Stop tracking a project                            | adapter       | Same as above.                                                                                                                                                                                                                                                                 |
| Manual rescan                                      | yes           | `scan`.                                                                                                                                                                                                                                                                        |
| Skill details                                      | no            | `read_skill` and a `SkillContentDto` (body, provenance source, update status). `InstalledSkillDto` carries only `description`.                                                                                                                                                 |
| Compare deployments                                | no            | `compare_deployments`; sketched in 6.3 only.                                                                                                                                                                                                                                   |
| Edit installed SKILL.md                            | yes (phase 3) | `write_atomic` + `Fingerprint` compare-and-swap exist; `write_skill_content` request and result DTOs are not defined.                                                                                                                                                          |
| Preview/apply frontmatter repair                   | yes           | Binding differs from the desktop preview (see check 4).                                                                                                                                                                                                                        |
| Remove a deployment                                | no            | `ScopeFs` has no `remove_dir_all`; a `Copy`, `Fork`, or `Manual` deployment cannot be removed. `ProcessSpawner` covers the skills.sh and dotagents cases.                                                                                                                      |
| Check for updates                                  | no            | No `data_root` for `update-check.json`; no `UpdateCheckSummary` DTO; `gh api` only through the bare spawner.                                                                                                                                                                   |
| Update managed skill                               | yes (phase 5) | `ProcessSpawner`.                                                                                                                                                                                                                                                              |
| Fork a managed skill                               | no            | Fork base snapshot lives under `app_data_dir/skill-studio/forks`; `RuntimeScope` has no `data_root`. The registry (`~/.agents/skill-studio.json`) is under `home_root`.                                                                                                        |
| Pull upstream into fork                            | no            | `data_root`; a fetch port (`GithubTools` today) beyond a bare spawner; no merge or conflict DTO.                                                                                                                                                                               |
| Un-fork                                            | no            | `data_root` (drop the base snapshot); `remove_dir_all`.                                                                                                                                                                                                                        |
| Park Global Universal skill                        | yes           | `RootKind::Parked`, `rename`, `EventKind::Park`.                                                                                                                                                                                                                               |
| Unpark                                             | yes           | Same.                                                                                                                                                                                                                                                                          |
| Native harness visibility                          | no            | Codex `config.toml` and OpenCode `opencode.json(c)` need TOML and JSONC read and write; the fixed dependency list (11.4) has neither, and there is no `NativeConfigStore` port. `DisabledBy` and `NativeDisableSpec` are ready.                                                |
| Disable/restore an eligible independent deployment | yes           | `rename` + `EventKind::MoveAside*`. The `.skill-studio-disabled` name is not a core constant.                                                                                                                                                                                  |
| Change invocation policy                           | no            | No `InvocationPolicy` DTO; Codex `agents/openai.yaml` needs YAML write, not in the dependency list. `CapabilityReport` has no `set_invocation_policy` row.                                                                                                                     |
| Materialize a linked root                          | yes           | `symlink`, `remove_file`, `EventKind::ExplodeSharedDir`.                                                                                                                                                                                                                       |
| Make independent copy                              | yes           | Composable from `read_dir`, `read_capped`, `create_dir_all`, `write_atomic`; a `copy_tree` helper would remove repetition.                                                                                                                                                     |
| Relink a broken deployment                         | yes           | `remove_file` + `symlink`, `EventKind::RepairRelinkLink`.                                                                                                                                                                                                                      |
| Remove a broken link                               | yes           | `remove_file`, `EventKind::RepairRemoveLink`.                                                                                                                                                                                                                                  |
| Reveal folder/open editor                          | adapter       | Editor preference file is under `home_root`.                                                                                                                                                                                                                                   |
| Plugin skills view                                 | partial       | `RootKind::PluginCache` and `PluginCacheSpec` exist; `DeploymentDto` has no marketplace, plugin, or version fields.                                                                                                                                                            |
| Mutation history                                   | partial       | `list_events`. No pagination cursor and no drift state; the headless spec names both.                                                                                                                                                                                          |
| Restore a supported event                          | yes           | `restore_event`.                                                                                                                                                                                                                                                               |
| Create pack from selected skills                   | no            | Out of scope; needs `copy_tree`, git through the spawner, and a pack registry location.                                                                                                                                                                                        |
| Open/view pack                                     | adapter       | Out of scope.                                                                                                                                                                                                                                                                  |
| Update pack                                        | no            | Out of scope; same as create.                                                                                                                                                                                                                                                  |
| Publish/push pack                                  | no            | No `Confirm` port (critic item 14); publish requires a confirmation.                                                                                                                                                                                                           |
| Delete pack                                        | no            | `remove_dir_all`.                                                                                                                                                                                                                                                              |
| Ask assistant                                      | no            | `ProcessSpawner::run` blocks until exit and returns one `ProcessOutput`. No streaming port, no `SkillAgentEvent` notice, no session id.                                                                                                                                        |
| Audit skill                                        | no            | Same.                                                                                                                                                                                                                                                                          |
| Accept/reject audit hunks                          | yes (phase 3) | Same primitives as "Edit installed SKILL.md".                                                                                                                                                                                                                                  |
| Discard audit proposal                             | adapter       | UI state.                                                                                                                                                                                                                                                                      |
| Test skill                                         | no            | Streaming spawner; run-target and judge DTOs; run history needs `data_root`.                                                                                                                                                                                                   |
| Choose test target                                 | no            | `cache_root` covers scratch and worktrees; no `RunTarget` DTO; in-place diff needs git through the spawner.                                                                                                                                                                    |
| Apply/discard test changes                         | no            | Same.                                                                                                                                                                                                                                                                          |
| Cancel agent run                                   | no            | `CancelToken` exists; the terminate, grace, kill sequence and the final event are adapter-only today.                                                                                                                                                                          |
| Open/delete scratch folder                         | adapter       | Delete needs `remove_dir_all` if it moves into the core.                                                                                                                                                                                                                       |
| Run history/transcripts                            | no            | `data_root` for `runs/`; `SkillRunRecord` and `SkillRunSummary` DTOs.                                                                                                                                                                                                          |
| Invocation activity/heatmap                        | no            | `usage_source.shape` is `Unknown` for every harness, including Claude Code, whose transcripts the app reads today. Under D5 `Unknown` blocks, so `observe_usage` can never run. Needs `Inferred` evidence for Claude Code plus `SkillInvocation` and `InvocationHeatmap` DTOs. |
| Learn                                              | adapter       | Static content.                                                                                                                                                                                                                                                                |
| Settings and theme                                 | adapter       | `~/.agents/skill-studio.json` is under `home_root`.                                                                                                                                                                                                                            |

### Check 1: harness representation

Every column of the capability matrix has a representation:

- Claude Code, Codex, OpenCode, pi: one `HarnessFacts` row each, with
  `Unknown` where the matrix says `unknown`.
- skills.sh CLI / `.agents`: not a harness. Represented by
  `RootKind::Universal`, `RootRole::Universal`, and
  `LifecycleOwnerKind::SkillsSh`.
- Cursor and Grok Build have catalog rows with `Inferred` evidence and
  `reads_universal_root: Unknown`, which correctly contradicts the TS
  `sharedRootReaders` list (critic item 20).

No capability is assumed universal in the types. Two facts the matrix
does not carry are also missing in the catalog:

- Whether a recursive reader (pi) reads `.agents/skills-parked` and
  `.agents/skills/.skill-studio-disabled` (critic C.4.2). `RootSpec` has
  no exclusion list, so the core cannot say a parked skill is hidden from
  pi.
- The matrix row "All-skills or tool-level disable" has no field. No
  workflow needs it yet.

`CapabilityReport::from_facts` reports five operations. It omits
`set_invocation_policy` (from `invocation_control`), so the CLI cannot ask
whether invocation control is supported for a harness.

### Check 2: `dirs::home_dir()` call sites

All 53 sites in critic C.1 derive a path by `home.join(...)`; every one is
replaceable by `RuntimeScope.home_root` (`NormalizedScope.home.lexical`).
`get_agent_targets` (`commands.rs:1487`, `unwrap_or_default`) becomes a
catalog read.

The sites are not the whole story. The same functions also call
`app.path().app_data_dir()` for the fork base snapshot, run history, the
update-check store, and the invocation cache. `RuntimeScope` has
`history_root` and `cache_root` only. A `data_root` (or an explicit list
of those four roots) is missing; without it the fork, update-check, and
run-history workflows cannot leave Tauri.

`add_method_defaults.rs:112` also reads `PATH` to find `npx` and
`dotagents`. `HarnessObserved.runner_binary` covers harness binaries only.
A `ToolLookup` port (or a `tools: Vec<String>` field on
`CapabilitiesRequest`) is missing.

### Check 3: event names and the agent-runner stream

| Emitted today                  | Core port                                    | Verdict                                                                                                                                                                                                                                                                                  |
| ------------------------------ | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `skills://snapshot`            | `CoreNotice::Revision` + `SnapshotPublisher` | Ported.                                                                                                                                                                                                                                                                                  |
| `skills://trial-expired`       | none                                         | Trials are out of scope; the desktop keeps emitting it. Acceptable for phase 1 and 2; record it in 6.3.                                                                                                                                                                                  |
| `skills://add-skill-operation` | `CoreNotice::OpState` + `Progress`           | Partial. The event carries `operation_id`, `sequence`, per-item `outcomes`, `result`, and `untrusted_source` with a `NeedsTrust` phase that waits for `confirm_add_skill_trust`. `CoreNotice` has no per-item payload and no "needs confirmation" notice; `Ports` has no `Confirm` port. |
| `skill-agent://event`          | none                                         | Not ported. `ProcessSpawner::run` returns one `ProcessOutput` at exit. There is no line stream, no `SkillAgentEvent { run_id, seq, at, kind }` notice, and no session id. Ask assistant, Audit, Test skill, and Cancel run cannot be expressed.                                          |

### Check 4: proposal binding

Desktop `FrontmatterRepairPreview` (`skill_frontmatter_repair.rs:35`):
`deployment_id`, `path`, `scope`, `reason`,
`expected_content_fingerprint`, `proposal_id`, `original_content`,
`proposed_content`, `allowed_apply_modes`. Desktop apply request:
`target: LifecycleTarget`, `proposal_id`, `expected_content_fingerprint`,
`mode`.

Core `FrontmatterRepairPreview`: `proposal_id`, `deployment_id`, `path`,
`owner_id`, `owner_kind`, `expected_fingerprint`, `proposed_fingerprint`,
`diff`, `allowed_apply_modes`. Core apply request: `preview`, `mode`.

Differences:

- Added on the wire: `owner_id`, `owner_kind`, `proposed_fingerprint`.
  These are values `proposal_id` already hashes, so exposing them is
  correct and lets `ownership_changed` be checked.
- Dropped: `scope`, `reason`, `original_content`, `proposed_content`.
  The UI renders `original_content` and `proposed_content`
  (`SkillProposedEdits.tsx`); `diff: String` is a wire change that is
  not listed in section 9. `reason` is the user-facing explanation and
  has no replacement.
- Apply must recompute the proposal from disk and compare `proposal_id`,
  since `proposed_content` is not sent. This works only because the
  repair is deterministic; the spec should state that rule.
- Apply takes the whole preview instead of `LifecycleTarget`. The core
  version is stricter (deployment only); the desktop resolves `Owner`
  targets in TS today. Section 9 should note the narrowing.
- `managed_update_warning` from the private intent is not surfaced; the
  UI shows it for `FixInstalledCopy`.

### Check 5: spec versus code

| Spec says                                                             | Code says                                                                                                                     | Fix                                                                                                               |
| --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| §5 `ProcessSpawner::spawn(ProcessSpec, CancelToken) -> ProcessOutput` | `ProcessSpawner::run(&ProcessSpec, &dyn CancelToken)`                                                                         | Rename one side.                                                                                                  |
| §6.1 `capabilities` returns `invalid_request` on an unknown harness   | Unknown ids are silently filtered out                                                                                         | Add the check or drop the error code.                                                                             |
| §6.2 `preview_frontmatter_repair` error list omits `scope_busy`       | Takes a shared lease                                                                                                          | Add `scope_busy`.                                                                                                 |
| §3.2 "`Parked` exists only at `Global`"                               | `RootRef { scope: Project(_), kind: Parked }` constructs and serializes                                                       | Add a constructor or a validation in `normalize` or `scan`.                                                       |
| §3.1 `projects` "Empty under `Discover` until a scan fills them"      | `NormalizedScope` is built once in `Runtime::new` and never updated; `lease_keys()` and `contains()` then cover the home only | See G1.                                                                                                           |
| Headless spec: "The home root cannot also be a project"               | `normalize` does not check                                                                                                    | Add the check.                                                                                                    |
| D9: every write takes a `ScopedPath`                                  | `ScopeFs::symlink(guard, target: &Path, link: &ScopedPath)` accepts an unconfined target                                      | A link to a path outside the scope is a scope escape on the next scan; confine `target` too, or document why not. |
| §11.4 dependency list                                                 | `Cargo.toml` matches (serde, serde_json, schemars, thiserror, chrono, ulid)                                                   | None now; phase 3 native-config workflows need TOML, JSONC, and YAML, so the list must open before phase 3.       |
| Headless spec `list_events`: "filters, pagination", "drift state"     | `ListEventsRequest { skill, limit }`, no cursor; `EventDto` has no drift state                                                | Add `after: Option<EventId>` and a drift field, or amend the headless spec.                                       |
| `docs/spec-event-store.md` lists 15 kinds                             | `EventKind` has 21                                                                                                            | Code is right; update the event-store spec.                                                                       |
| §4 table and §3.2 text                                                | Match `harness.rs` and `identity.rs`                                                                                          | None.                                                                                                             |
| §5 `CoreNotice`, `HistoryStore`, `ScopeFs` method lists               | Match `ports.rs`                                                                                                              | None.                                                                                                             |

### Blocking gaps

G1. `ProjectSelection::Discover` is a dead end: no port discovers
projects, and `NormalizedScope` cannot grow after `Runtime::new`, so the
lease and `confine` never cover discovered projects. Add a
`ProjectDiscovery` port that runs inside `normalize`, or make the adapter
always pass `Explicit`.

G2. No `data_root` in `RuntimeScope`. Blocks fork, update check, run
history, and the invocation cache.

G3. `ScopeFs` has no `remove_dir_all`. Blocks removal of `Copy`, `Fork`,
and `Manual` deployments, pack deletion, and scratch cleanup.

G4. No streaming process port and no agent-event notice. Blocks Ask
assistant, Audit, Test skill, and Cancel run; the `skill-agent://event`
stream has no home.

G5. No `Confirm` port. Blocks the add-skill trust prompt and pack publish.

G6. `usage_source.shape` is `Unknown` for Claude Code while the app reads
its transcripts today; under D5 the heatmap can never run in the core.

G7. Repair preview drops `reason`, `original_content`, and
`proposed_content` without a section 9 entry; the UI depends on them.

G8. Native config formats (TOML, JSONC, YAML) have no port and no
dependency; phase 3 rows "Native harness visibility" and "Change
invocation policy" cannot be implemented as specified.

### Resolution (2026-09-09)

Phase 1 and 2 gaps closed in the crate; later-phase gaps recorded in 6.3
under "Needs first".

| Gap           | Resolution                                                                                                                                                                                                                                                                                                                                            |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| G1            | `ProjectDiscovery` port; `NormalizedScope::normalize_with_discovery`; `Runtime::new` passes `Ports.discovery`. `Discover` without the port is `invalid_scope`.                                                                                                                                                                                        |
| G2            | `RuntimeScope.data_root: Option<PathBuf>`; copied to `NormalizedScope`.                                                                                                                                                                                                                                                                               |
| G3            | Later phase (6.3: `remove_dir_all`).                                                                                                                                                                                                                                                                                                                  |
| G4            | Later phase (6.3: streaming spawner, `AgentEvent` notice).                                                                                                                                                                                                                                                                                            |
| G5            | Later phase (6.3: `Confirm` port, `NeedsConfirmation` notice).                                                                                                                                                                                                                                                                                        |
| G6            | Claude Code `usage_source.shape` is `Yes(Inferred)`; `observe_usage` is allowed.                                                                                                                                                                                                                                                                      |
| G7            | Preview carries `scope`, `reason`, `original_content`, `proposed_content`, `managed_update_warning`; section 9 lists the wire change and the `Deployment`-only narrowing; 6.2 states the recompute-and-compare rule.                                                                                                                                  |
| G8            | Later phase (6.3: `NativeConfigStore`, TOML, JSONC, YAML).                                                                                                                                                                                                                                                                                            |
| Check 1       | `HarnessFacts.skips_hidden_entries` and `all_skills_disable`; `CapabilityReport` reports `set_invocation_policy`.                                                                                                                                                                                                                                     |
| Check 2       | `ToolLookup` port; `CapabilitiesRequest.tools`; `Capabilities { harnesses, tools }`.                                                                                                                                                                                                                                                                  |
| Check 3       | `skills://trial-expired` recorded in 6.3 as adapter-only.                                                                                                                                                                                                                                                                                             |
| Check 5       | Spec says `ProcessSpawner::run`; `capabilities` returns `invalid_request` on an unknown harness; 6.2 lists `scope_busy` for the preview; `RootRef::new` refuses `Parked` outside `Global`; the home is never a project; `symlink` confines its target; `ListEventsRequest.after` and `EventDto.drift`; `docs/spec-event-store.md` lists the 21 kinds. |
| Workflow rows | `DeploymentDto.plugin` for the plugin skills view; `MOVE_ASIDE_DIR_NAME`, `PARKED_ROOT_RELATIVE`, `UNIVERSAL_ROOT_RELATIVE` constants.                                                                                                                                                                                                                |

## 13. PR 8 implementation plan

Written 2026-09-09 from a reconnaissance of the desktop scanner. Section 10's
PR 8 row states the goal; this section states the work.

### 13.1 The thirteen content facts

`SkillCandidate` carries thirteen fields that `DeploymentDto` does not. All
thirteen are computed in `skill_discovery.rs` and funnelled through the
`SkillContentFacts` struct: `frontmatter`, `frontmatter_fields`,
`spec_violations`, `has_spec`, `folder_bytes`, `file_count`,
`skill_md_tokens`, `description_tokens`, `content_hash`, `modified_at`,
`folder_truncated`, `in_git_repo` and `studio_disabled`.

Twelve of them read only inside the skill directory, so `ScopeFs` as it
stands is sufficient: `read_capped` for `SKILL.md`, `read_dir` plus
`symlink_metadata` for the folder walk, and `read_prefix` for the truncation
flags.

`in_git_repo` is the exception. It walks _up_ from the skill directory
looking for a `.git` entry at each ancestor, and `ScopeFs` exposes no
ancestor primitive. PR 8 adds one:

```rust
/// True when `start` or any ancestor of it inside the scope holds an entry
/// named `name`. Walks up from `start` and stops at the scope root, so a
/// repository outside the scope is not visible.
fn ancestor_holds(&self, start: &Path, name: &str) -> std::io::Result<bool>;
```

Stopping at the scope root is deliberate and is a behaviour change from the
desktop, which walks to the filesystem root. The desktop's version reads
directories the scope does not contain, which is the ambient-state leak this
extraction exists to remove. The parity fixtures must therefore place their
`.git` inside the fixture home.

### 13.2 Two hashes, not one

There are two distinct hashing schemes and PR 8 must not merge them.

`content_hash` is a whole-folder digest: SHA256 over the sorted
`(relative path, bytes)` pairs of every file in the skill folder, capped at
`MAX_FOLDER_BYTES`. It answers "did this skill's content change".

`Fingerprint` and the host's `hash_entry` are a per-entry digest with a type
tag (`L` symlink, `D` directory, `F` file). They answer "is this exact path
still the bytes the backup recorded". `tests/fingerprint_parity.rs` pins the
second scheme only; the first has no parity test today and gains one in PR 8.

### 13.3 New core dependencies

`tiktoken-rs` 0.12 for token counting (`cl100k_base`, via
`encode_with_special_tokens`, deterministic and vocabulary-only, so it reads
no ambient state) and `sha2` for `content_hash`. No other dependency is
needed; `ScopeFs` already covers the reads.

### 13.4 Ownership: four ledgers, one precedence

`classify_owner` today reads one file. It must read four per scope, which
`load_ownership_ledgers` in `skill_ownership.rs` already models:

| Ledger              | File                        | Format | Precedence |
| ------------------- | --------------------------- | ------ | ---------- |
| dotagents manifest  | `.agents/agents.toml`       | TOML   | highest    |
| dotagents lock      | `.agents/agents.lock`       | TOML   |            |
| skills.sh lock      | `.agents/.skill-lock.json`  | JSON   |            |
| forks, copies, park | `.agents/skill-studio.json` | JSON   | lowest     |

Core already depends on `toml` and `serde_json`, so both parsers are present.
A deployment claimed by two ledgers is `Ambiguous`; this is the case the
parity fixtures missed before the adversarial review.

### 13.5 Order of work

1. Add `ScopeFs::ancestor_holds` and the two dependencies. Extend
   `DeploymentDto` with the thirteen facts and `source_kind`.
2. Teach `ops::scan` to fill them, with the folder walk in core.
3. Teach `classify_owner` the other three ledgers and the desktop precedence.
4. Rebuild `assemble_installed_skills` from `Inventory` alone; drop the
   overlay and the local classifiers.
5. Delete `skill_discovery.rs` and `provenance.rs`; reduce
   `skill_refresh.rs` to caching, timing and Tauri events.
6. Invert the three ownership fixtures so they assert agreement.

Steps 1 to 3 are additive and leave the desktop untouched, so the parity
suites stay green throughout and keep proving the two implementations agree.
Only step 4 removes the second implementation, and only after the suites
have compared them across every `LifecycleOwnerKind` variant.

### 13.6 What blocks deletion

`skill_discovery.rs` exports five symbols the rest of the desktop calls:
`discover_skill_candidates`, `discover_skill_candidates_cached`,
`discover_named_skill_candidates_cached`, `SkillFactsCache` and
`live_skill_content_hash`. `provenance.rs` exports `SourceKind` and
`classify_source_kind`. `live_skill_content_hash` is the notable one: the
mutation guards in `skill_add_operation.rs` and `skill_independent_copy.rs`
call it outside any scan, so core must expose the same digest as an
operation those guards can call, not only as a field on a scanned
deployment.

### 13.7 Two blockers the call-site map found

**Four link facts are missing from `DeploymentDto`.** The desktop's
`Deployment` carries `is_symlink`, `resolved_path`, `symlink_is_broken` and
`symlink_error`. Core computes a `resolved_path` internally, for owner
propagation, but does not put it on the DTO, and it derives link health at
diagnose time rather than storing it. The two `resolved_path` values are not
the same value: the desktop applies `.filter(|c| c != entry_path)` on the
non-link branch, so an ordinary directory yields `None`, where core's
internal version always yields `Some`. Note also that the desktop's doc
comment on this field is wrong - it claims `resolved_path` is `None` for
symlinked entries, but the code sets `Some(skill_dir)`. Trust the code.

These four land as step 3.5, additively, before step 4, so the parity suite
keeps comparing two live implementations.

**`live_skill_content_hash_controlled` has no core equivalent.** Step 13.6
named `live_skill_content_hash` as the notable blocker and
`ops::skill_content_hash` now answers it, with its own parity test - the
scan-field test proved nothing about the standalone entry point the mutation
guards call. But `skill_add.rs` calls a second, cancellable form that takes a
control handle. Core exposes no cancellable digest, so either
`ops::skill_content_hash` gains an `OpContext` for cancellation or the Add
path accepts an uncancellable digest. This is a decision, not an oversight;
it must be settled before `skill_discovery.rs` can be deleted.

### 13.8 The parity suites are scaffolding

`content_facts_parity.rs` and `core_scan_parity.rs` compare two
implementations. Step 4 deletes one of them, so both suites stop compiling:
`content_facts_parity.rs` imports `discover_skill_candidates` and
`live_skill_content_hash` directly, and `core_scan_parity.rs`'s `run_desktop`
becomes a tautology once the desktop scan is core.

Deleting them with the duplicate would delete the coverage that justified
deleting the duplicate. Convert each fixture instead: keep the setup, drop
the desktop call, and assert the expected owner kind and content facts
against core alone. The fixtures are the asset; the comparison was only ever
the means of establishing what to assert.

### 13.9 What landed, and one classifier the deletion removed

The desktop scanner is gone: `skill_discovery.rs`, `provenance.rs`, and
`skill_candidate.rs` are deleted, and `skill_ownership.rs` keeps only the
ledger loaders and the owner-id codec. `SkillSnapshot` is built from the
core `Inventory` alone, and the two parity suites now assert core against
fixture-derived expectations, as 13.8 asked.

The port exposed a classifier that the core carried from its first draft and
that was wrong in a way the desktop had never been. The core computed
`source_kind` with its own lock-file check: one home lock file tested against
every root's bare skill name. That check is scope-blind. A manual skill under
`.claude/skills` would badge as skills.sh whenever the home lock named a
same-named skill, and it had no dotagents branch at all. The desktop never
did this; its badge was the third return value of `classify_lifecycle_owner`,
a projection of the owner kind.

The core now does the same. `source_kind_from_owner` maps each
`LifecycleOwnerKind` to a `SourceKind`: plugin, fork, skills.sh, and in-repo
map to themselves; dotagents, wildcard-dotagents, and ambiguous all read as
dotagents; copy and manual read as manual. The badge is a projection of the
owner classification, not a second classifier, so it can never disagree with
it. The scanner no longer reads the lock file for the badge; the lock file
is read once, inside the ledger set, for ownership.

One consequence to know. The projection is taken at classification time,
before verified link propagation upgrades a per-harness symlink's
`owner_kind` to its universal root's owner. A per-harness link into
`~/.agents/skills/foo` therefore carries `owner_kind: skills_sh` and
`source_kind: manual`. This matches the desktop at `main`, where the
per-harness early return also badged such links manual, and the skill-level
badge (the minimum over deployments) still resolves from the universal root.
Whether the badge should follow propagation is an open product question, not
a parity bug; it is listed with the other out-of-scope findings.
