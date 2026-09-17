# Current call stack and domain primitives

This document records what the code does today. It does not propose changes.
Every fact points to a file and line. Line numbers come from a code survey
dated 2026-09-09. They drift as the code changes. Items the survey could not
confirm are marked **Unknown**.

Paths are relative to the repository root. `src-tauri` means
`apps/desktop/src-tauri/src/skills` unless the path says otherwise.

## 1. Identity model

One skill has one lexical name. One skill has many deployments. Each
deployment lives under one root. Each root belongs to one scope.

| Concept                       | Identity today                                                                                                                                                       | Rust type that carries it                                       | Location                                                              |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- | --------------------------------------------------------------------- |
| Skill                         | The directory name (lexical). Two dirs with the same name merge into one skill. A symlink alias with a different name is a different skill.                          | `InstalledSkill` (DTO)                                          | `src-tauri/skill_dto.rs:265`                                          |
| Skill (raw fact record)       | One found `SKILL.md` directory                                                                                                                                       | `SkillCandidate`                                                | `src-tauri/skill_candidate.rs:19`                                     |
| Deployment                    | `dep:v1/{scope}/{slot}/{destination}/{name}/{project or -}/{encoded lexical path}`                                                                                   | `Deployment` (DTO); id built by `deployment_id`                 | `src-tauri/skill_dto.rs:127`; `src-tauri/skill_deployment.rs:123`     |
| Deployment shape              | `SkillDestination` (Universal, PerHarness), `BackingRelationship` (Canonical, LinkedTo, Independent), `DeploymentMutability` (Mutable, ReadOnly)                     | Enums                                                           | `src-tauri/skill_deployment.rs:21`                                    |
| Root                          | `SkillRoot { label, project_path: Option, path }`. Label is a harness display name, `shared`, or `parked`.                                                           | `SkillRoot`                                                     | `src-tauri/agents.rs:350`                                             |
| Root list                     | Six first-class harness dirs, OpenCode legacy `skill/`, `.agents/skills` (`shared`), `.agents/skills-parked` (`parked`, global only), then the same per project      | `skill_roots`                                                   | `src-tauri/agents.rs:362`                                             |
| Harness                       | 43-variant enum, serde kebab-case                                                                                                                                    | `AgentId`                                                       | `src-tauri/agents.rs:16`                                              |
| Project                       | An absolute directory path string. A directory is a project when it has a `SKILL_DIR_MARKERS` entry.                                                                 | `SkillSnapshot.projects: Vec<String>`; `SKILL_DIR_MARKERS`      | `src-tauri/skill_refresh.rs:61`; `src-tauri/project_discovery.rs:19`  |
| Scope                         | Wire string: `global`, `project`, `plugin`, `parked`                                                                                                                 | `scope_str`; `InstallScope` (Global, Project)                   | `src-tauri/skill_discovery.rs:865`; `src-tauri/skill_dto.rs:505`      |
| Owner                         | `owner:v1/global/{name}` or `owner:v1/project/{encoded path}/{name}`                                                                                                 | `owner_id_for` / `parse_owner_id`; `LifecycleOwnerKind`         | `src-tauri/skill_ownership.rs:267`; `src-tauri/skill_ownership.rs:27` |
| Mutation target               | Exactly one of `deployment_id` or `owner_id`                                                                                                                         | `LifecycleTarget`                                               | `src-tauri/skill_dto.rs:512`                                          |
| Visibility target             | `deployment_id` + `reader_agent: AgentId`                                                                                                                            | `HarnessVisibilityTarget`                                       | `src-tauri/skill_dto.rs:522`                                          |
| Proposal (frontmatter repair) | `proposal_id` = sha256 over deployment id, path, owner id, owner kind, content fingerprint, proposed text. Bound to `expected_content_fingerprint` (`sha256:<hex>`). | `FrontmatterRepairPreview`; `FrontmatterRepairIntent` (private) | `src-tauri/skill_frontmatter_repair.rs:35`; `:56`                     |
| Event (history)               | ULID id, `kind` string, status `pending`, `done`, `failed`, `interrupted`                                                                                            | `EventDraft`, `EventRow`, `InverseOp`                           | `src-tauri/event_store.rs:1073`; `:1102`; `:1127`                     |
| Event (history DTO)           | Projection for the History view                                                                                                                                      | `SkillEventDto`                                                 | `src-tauri/skill_dto.rs:87`                                           |
| Snapshot                      | Monotonic `revision: u64`                                                                                                                                            | `SkillSnapshot`                                                 | `src-tauri/skill_refresh.rs:61`                                       |

### 1.1 Harness identity facts

`AgentId` has four exhaustive `match` methods: `cli_name` (`agents.rs:64`),
`display_name` (`agents.rs:116`), `project_path` (`agents.rs:165`),
`global_path` (`agents.rs:214`). `AgentId::all` (`agents.rs:263`) is a
hand-maintained list.

| Fact                          | Value                                                                                                                                                                                                     | Location                                                                                 |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| Variant count                 | 43. The header comment says 42.                                                                                                                                                                           | `agents.rs:3`, `agents.rs:16`                                                            |
| First-class harnesses         | ClaudeCode, Codex, OpenCode, Pi, Cursor, GrokBuild                                                                                                                                                        | `FIRST_CLASS_AGENTS`, `agents.rs:337`                                                    |
| Same list, duplicated         | `CHECKED_HARNESSES`                                                                                                                                                                                       | `add_method_defaults.rs:44`                                                              |
| OpenCode wire name            | serde `open-code`, `cli_name` `opencode`. Hand-patched in three places.                                                                                                                                   | `skill_harness_disable.rs:620`, `event_commands.rs:157`, `event_commands.rs:226`         |
| `Deployment.agent`            | Carries `display_name()` or the literal `shared` / `parked`. It is not an `AgentId`. Compared as a string.                                                                                                | `skill_refresh.rs:1112-1155`, `skill_invocation.rs:336,372`, `event_commands.rs:161,230` |
| Universal root label          | Scanner writes `shared`. `is_universal_root_label` accepts `shared` or `universal`.                                                                                                                       | `skill_deployment.rs:68`                                                                 |
| Harness slot in deployment id | `harness_slot` maps display names to `claude-code`, `codex`, `opencode`, `pi`, `cursor`, `grok-build`, `universal`, `other`                                                                               | `skill_deployment.rs:236`                                                                |
| Non-default paths             | OpenCode global `.config/opencode/skills`; Pi global `.pi/agent/skills`; CodyAi `.cody/skills`; PearAi `.pearai/skills`; AmazonQ `.amazonq/skills`; GeminiCode `.gemini/skills`; GrokBuild `.grok/skills` | `agents.rs:165`, `agents.rs:214`                                                         |
| OpenCode legacy `skill/` dir  | Added in `skill_roots`, not in `AgentId` path methods                                                                                                                                                     | `agents.rs:372-376`, `agents.rs:398-402`                                                 |

### 1.2 Provenance has two models

| Model     | Type                 | Values                                                                                                                                                  | Where used                                 |
| --------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| Display   | `SourceKind`         | Dotagents < Plugin < SkillsSh < InRepo < Manual < Fork (derived `Ord`; lower wins)                                                                      | `provenance.rs:27`; assembly picks `min()` |
| Lifecycle | `LifecycleOwnerKind` | SkillsSh, Dotagents, Copy, Fork, Plugin, InRepo, Manual, WildcardDotagents, Ambiguous. `is_mutable()` is true for SkillsSh, Dotagents, Copy, Fork only. | `skill_ownership.rs:27`; `:56`             |

`classify_lifecycle_owner` (`skill_ownership.rs:141`) returns
`(LifecycleOwnerKind, Option<owner_id>, SourceKind)`. Precedence: plugin,
then a matching `CopyDeploymentRecord`, then PerHarness InRepo or Manual,
then ledger lookup, then InRepo or Manual.

### 1.3 Disable and invocation identity

| Concept                                 | Type               | Values                                                          | Location                                    |
| --------------------------------------- | ------------------ | --------------------------------------------------------------- | ------------------------------------------- |
| Disable mechanism                       | `DisabledBy`       | CodexConfig, OpencodePermission, ClaudeLinkRemoved, StudioMoved | `skill_dto.rs:28`                           |
| Disabled readers on a shared deployment | `Vec<String>`      | `codex`, `open-code`                                            | `skill_dto.rs:192`; `skill_refresh.rs:1152` |
| Claude link state                       | `ClaudeLinkState`  | PerSkill, WholeDir, None                                        | `skill_harness_disable.rs:42`               |
| Invocation policy                       | `InvocationPolicy` | Both, UserOnly, ModelOnly                                       | `frontmatter.rs:69`                         |

## 2. Read path: rescan to snapshot DTO

A rescan is a flag flip. The background loop does the work.

```
requestSkillRescan                         apps/desktop/src/lib/skill-api.ts:582
  invoke("request_skill_rescan")
  request_skill_rescan                     skill_refresh.rs:281   sets skills_dirty
  run_refresh_loop                         skill_refresh.rs:720   polls every 200 ms; also notify events
    rebuild_snapshot_now                   skill_refresh.rs:354   also from get_installed_skills (commands.rs:161)
      takes rebuild_lock; dirs::home_dir() skill_refresh.rs:363
      build_snapshot                       skill_refresh.rs:1175
        project_discovery::discover_skill_projects   project_discovery.rs:235
          codex_project_paths :32, claude_transcript_cwds :162, is_studio_scratch_path :227
        is_home_directory filter           skill_refresh.rs:298
        skill_discovery::discover_skill_candidates_cached   skill_discovery.rs:895
          SkillFactsCache::begin_pass / end_pass            skill_discovery.rs:458
          discover_into                                     skill_discovery.rs:1002
            agents::skill_roots                             agents.rs:362
            shared_root_has_lock_entry :706, scope_str :865
            scan_root_entries :743  (root, then root/.skill-studio-disabled)
              scan_root_entry :780
                resolve_symlink :694 / raw_symlink_target :684
                get_or_compute_facts :492
                  walk_for_facts :308 -> walk_folder_capped :90
                  folder_fingerprint :251
                  compute_content_facts_from_walk :378
                    parse_frontmatter, frontmatter_fields, content_hash :197, count_tokens :272, has_spec :54
                build_candidate :533
                  validate_skill (frontmatter.rs:207), plugins::find_plugin_root (plugins.rs:95), in_git_repo :718
                broken_symlink_candidate :627
            plugins::scan_plugin_skills                     plugins.rs:211 -> scope "plugin"
        lock_file::read_lock_file          lock_file.rs:49 -> dirs::home_dir() :44 -> read_lock_file_at :56
        skill_ownership::load_ownership_ledgers   skill_ownership.rs:76 -> read_ledgers :99
          read_lock_file_at; dotagents_ledger::read_dotagents_ledger (dotagents_ledger.rs:93)
        skill_fork_registry::read_fork_registry_or_default
        skill_assembly::assemble_installed_skills   skill_assembly.rs:91
          skill_deployment::id_for_candidate        skill_deployment.rs:192 -> deployment_id :123, harness_slot :236
          skill_ownership::classify_lifecycle_owner skill_ownership.rs:141
            ledger_for_candidate :121, owner_id_for :267, copy_record_matches_candidate :235
          provenance::classify_source_kind          provenance.rs:56  (only when ledgers is empty)
          new_installed_skill                       skill_assembly.rs:25
          frontmatter::invocation_policy            frontmatter.rs:83
          propagate_verified_linked_owners          skill_assembly.rs:220
        skill_update_check::read_update_check_store_at / summarize
        apply_skill_snapshot_overlays      skill_refresh.rs:919
          fork / trial / parked records
          codex_skill_config::read_disabled_skill_md_paths      codex_skill_config.rs:25
          opencode_skill_permission::read_denied_patterns       opencode_skill_permission.rs:50
          skill_update_check::state_for_owner / has_update
          frontmatter::invocation_policy_from
          read_codex_allow_implicit_invocation                  skill_refresh.rs:900
        invocation_index.refresh / save    skill_invocations.rs:210 (home/.claude/projects)
        skill_run_history::read_last_test_index   skill_run_history.rs:276
        opencode_skill_permission::detect_config_kind
        -> SkillSnapshot { revision: 0, ... }
      publish_skill_snapshot               skill_refresh.rs:406
        store_skill_snapshot               skill_refresh.rs:417   revision = prev + 1; replaces RwLock
        app.emit("skills://snapshot")      SNAPSHOT_EVENT skill_refresh.rs:38
  onSkillSnapshot listener                 apps/desktop/src/lib/skill-api.ts:590
  get_skill_snapshot                       skill_refresh.rs:274
```

### 2.1 Read-path facts

| Fact                               | Value                                                                                             | Location                         |
| ---------------------------------- | ------------------------------------------------------------------------------------------------- | -------------------------------- |
| Only filesystem walker             | `discover_skill_candidates_cached`                                                                | `skill_discovery.rs:895`         |
| Facts cache key                    | Canonical skill dir; validated by stat-only `folder_fingerprint`                                  | `skill_discovery.rs:451`, `:251` |
| Walk limits                        | `SKILL.md` read through a 2 MiB bounded reader; folder capped at 2,000 files / 64 MiB             | `skill_discovery.rs:32`, `:308`  |
| Content hash                       | sha256 over sorted (relative path, bytes)                                                         | `skill_discovery.rs:197`         |
| Uncached live hash for guards      | `live_skill_content_hash`                                                                         | `skill_discovery.rs:335`         |
| Holding dir for move-aside disable | `.skill-studio-disabled`                                                                          | `skill_discovery.rs:735`         |
| Aggregate skill facts              | Taken from the first readable deployment in scan order                                            | `skill_assembly.rs:91`           |
| Lock-only skills                   | Kept with empty `deployments`, source kind SkillsSh                                               | `skill_assembly.rs:91`           |
| Get-installed fast path            | Returns cached `snapshot.skills` when not dirty and projects covered; else rebuilds synchronously | `commands.rs:161`                |
| Overlay order                      | Reset, then fork registry, trials, parked, per-harness disable, update check, invocation policy   | `skill_refresh.rs:919`           |

### 2.2 Per-harness overlay inputs

| Harness           | Disable source read                                                                                           | Location                                         |
| ----------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| Codex             | `~/.codex/config.toml` `[[skills.config]]` rows with `enabled=false`; paths canonicalized                     | `codex_skill_config.rs:17`, `:25`                |
| OpenCode          | `~/.config/opencode/opencode.json` `permission.skill.<key>="deny"`; exact or `*` glob                         | `opencode_skill_permission.rs:17`, `:50`, `:74`  |
| OpenCode `.jsonc` | Detected only. Never parsed. Never written.                                                                   | `opencode_skill_permission.rs:23`, `:31`         |
| Claude Code       | Per-skill link `~/.claude/skills/<name>` missing, recorded in registry `harness_disabled`                     | `skill_park.rs:43`, `skill_fork_registry.rs:186` |
| Codex sidecar     | `<skill>/agents/openai.yaml` `policy.allow_implicit_invocation`, read as a note only                          | `skill_refresh.rs:900`                           |
| Invocation usage  | Claude Code transcripts `~/.claude/projects/*/*.jsonl` only. `SkillInvocation.agent` is always `Claude Code`. | `skill_invocations.rs:642`, `:688`               |

## 3. Write path

Every mutation command has the same shape:

1. `ForkMutationLock::try_acquire` (`skill_fork.rs:948`). It uses
   `try_lock`. A busy lock returns `Another fork operation is in progress`.
2. Rebuild a fresh snapshot with `rebuild_fresh_lifecycle_snapshot`
   (`skill_lifecycle.rs:125`), which wraps `rebuild_snapshot_now`.
3. Resolve the target against that snapshot.
4. Call a pure `*_with(home, ...)` function.
5. Publish: `patch_snapshot_and_emit` (surgical) or
   `request_snapshot_rebuild` (mark dirty) or
   `reconcile_skill_names_and_emit` (targeted).

### 3.1 Target resolution

| Step           | Function                               | Check                                                         | Location                      |
| -------------- | -------------------------------------- | ------------------------------------------------------------- | ----------------------------- |
| Lookup         | `find_deployment`                      | Id must be in the snapshot                                    | `skill_lifecycle.rs:39`       |
| Re-parse       | `revalidate_deployment`                | Path leaf, scope, destination match the id                    | `skill_lifecycle.rs:54`       |
| Mutability     | `require_direct_deployment_mutable`    | `owner_kind.is_mutable()` and `Mutable`                       | `skill_lifecycle.rs:201`      |
| Drift          | `revalidate_deployment_fingerprint`    | Live hash equals `content_hash`                               | `skill_lifecycle.rs:87`       |
| Owner target   | `preview_owner_deployments`            | Every deployment with that `owner_id`; picks Canonical        | `skill_lifecycle.rs:228`      |
| Park scope     | `require_global_universal_park_target` | Scope `parked`, or global + Universal + Canonical + no plugin | `skill_lifecycle.rs:180`      |
| Native disable | `resolve_native_harness_target`        | Per-harness rules; OpenCode name-collision refusal            | `skill_harness_disable.rs:48` |
| Fork target    | `resolve_recorded_fork_target`         | Global Universal; ForkRecord id and dir match                 | `skill_fork.rs:1499`          |
| Park target    | `park_target_skill`                    | Custom; `unpark_skill` also checks ParkedRecord               | `skill_park.rs:430`, `:497`   |

`resolve_lifecycle_target` (`skill_lifecycle.rs:135`) is pure. Only
`resolve_fresh_lifecycle_target` (`skill_lifecycle.rs:108`) touches Tauri.

### 3.2 One lifecycle op: park

```
park_skill [#[tauri::command]]                       skill_park.rs:463
  ForkMutationLock::try_acquire                      skill_fork.rs:952
  skill_lifecycle::resolve_fresh_lifecycle_target    skill_lifecycle.rs:108
    rebuild_fresh_lifecycle_snapshot -> skill_refresh::rebuild_snapshot_now   skill_refresh.rs:354
    resolve_lifecycle_target                         skill_lifecycle.rs:135
      find_deployment :39 / revalidate_deployment :54 / require_direct_deployment_mutable :201 / revalidate_deployment_fingerprint :87
  park_target_skill                                  skill_park.rs:430
    require_global_universal_park_target             skill_lifecycle.rs:180
  park_skill_with -> park_skill_impl                 skill_park.rs:198 / :210
    read_fork_registry                               skill_fork_registry.rs:344
    take_claude_link                                 skill_park.rs:53
    fs::rename ~/.agents/skills/<n> -> ~/.agents/skills-parked/<n>
    insert ParkedRecord (deployment id with scope "parked")
    retarget_trial                                   skill_park.rs:160
    write_fork_registry (temp + rename)              skill_fork_registry.rs:372
    [on failure] rollback move and link
  skill_refresh::patch_snapshot_and_emit             skill_refresh.rs:442
```

History for park: a `ParkedRecord` in `~/.agents/skill-studio.json` only.
No `EventStore` row.

Unpark (`skill_park.rs:318`): if the shared dir exists and the trees are
identical, drop the parked copy (Reconciled). If they differ, move the
parked copy to `~/.agents/skills-trash/<n>-<ts>` (ConflictTrashed). Else
rename back (Restored). Restore the Claude link best-effort.

### 3.3 Where history is recorded

| Store                           | Path                                                                 | Written by                                                                                                 | Location                                   |
| ------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| Fork registry (JSON, version 4) | `~/.agents/skill-studio.json`                                        | park, unpark, fork, pull, unfork, Claude link toggle, copy install, trials, trust, editor pick             | `skill_fork_registry.rs:250`; write `:372` |
| Event store (SQLite, WAL)       | `<app_data_dir>/events.sqlite3`                                      | `set_deployment_enabled` (move-aside), independent copy, materialize link ops, frontmatter repair, restore | `event_store.rs:117`; `lib.rs:17`          |
| Backups                         | `<app_data_dir>/backups/<event-id>/<n>-<basename>` + `manifest.json` | `backup_paths`                                                                                             | `event_store.rs:143`                       |

Add operations write no `EventStore` row. `deployments_created` lives only
in the returned `AddSkillResult`.

Five-phase event protocol (`event_store.rs:5`): `allocate_id`,
`backup_paths`, `record` (pending, `post_fingerprint` None), mutate,
`patch_inverse_post_fingerprint` + `finish` (Done or Failed).

| Event kind literal                                                                                                                                              | Written in                                        |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| `explode_shared_dir`, `materialize_then_disable`, `unlink_harness`, `relink_harness`, `reconcile_remove_stale_link`, `repair_remove_link`, `repair_relink_link` | `skill_materialize.rs`                            |
| `move_aside_disable`, `move_aside_restore`                                                                                                                      | `skill_harness_disable.rs:349`                    |
| `make_independent_copy`                                                                                                                                         | `skill_independent_copy.rs:141`                   |
| `repair_skill_frontmatter`                                                                                                                                      | `skill_frontmatter_repair.rs:439`                 |
| `restore`                                                                                                                                                       | `event_store.rs`; `skill_independent_copy.rs:816` |

There is no enum for event kinds.

### 3.4 Frontmatter repair

```
preview_skill_frontmatter_repair [#[tauri::command]]   skill_frontmatter_repair.rs:280
  skill_lifecycle::rebuild_fresh_lifecycle_snapshot
  exact_target :263  (deployment_id only)
  preview_from_deployment :212
    propose_colon_scalar_repair :101
    content_fingerprint :68   ("sha256:<hex>")
    proposal_id :74
    apply_modes :182          (apply-fix | fix-installed-copy | fork-and-fix)

apply_skill_frontmatter_repair [#[tauri::command]]     skill_frontmatter_repair.rs:406
  ForkMutationLock::try_acquire
  rebuild_fresh_lifecycle_snapshot -> exact_target
  EventStoreState lock (guard.as_mut() at :423)
  begin_bound_frontmatter_repair_transaction :249
    begin_skill_md_write_transaction                    skill_md_write.rs:64   (process-wide SKILL_MD_WRITE_LOCK)
    validate_bound_preview :233                         refuses if fingerprint or proposal_id differ
  allocate_id                                           event_store.rs:925
  backup_paths([SKILL.md])                              event_store.rs:143
  record(kind repair_skill_frontmatter, payload FrontmatterRepairIntent,
         inverse RestoreBackup{path, pre_fingerprint})  event_store.rs:195
  [ForkAndFix] skill_fork::fork_resolved_deployment_with_real_services
    app.path().app_data_dir() at :484-487; re-read and re-check fingerprint
  SkillMdWriteTransaction::replace_bytes -> atomic_replace_skill_md_unlocked   skill_md_write.rs:132
    temp .SKILL.md.tmp-<pid>-<counter>-<nanos>, write, fsync, copy permissions, rename, fsync dir
  patch_inverse_post_fingerprint :608 -> finish :229   (finish_repair_write :289)
  skill_refresh::reconcile_skill_names_and_emit -> request_snapshot_rebuild
```

Startup recovery (`reconcile_interrupted_frontmatter_repair`,
`skill_frontmatter_repair.rs:355`): under the SKILL.md lock, compare live
bytes. Equal to proposed fingerprint: finish Done. Not equal to expected
fingerprint: Failed, error `drifted from both sides`. Non-fork mode: Failed.
ForkAndFix with a managed ledger still owning the name:
`roll_back_incomplete_fork`.

### 3.5 Other write paths in one line each

| Op                        | Entry                                                                                       | Notes                                                                                                                           | Location                                                                                        |
| ------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| Foreground add            | `add_skill` -> `add_skill_with(home, request, runner, fetch, lookup)`                       | dotagents / skills.sh / copy dispatch; `maybe_claude_code_symlink`; `apply_disabled_harnesses`; then `request_snapshot_rebuild` | `skill_add.rs:1349`, `:1125`, `:165`, `:1074`                                                   |
| Background add            | `start_add_skill_operation` -> `spawn_operation` (`spawn_blocking`) -> `run_operation_body` | Phases Queued..Completed; `sequence` strictly increases; targeted reconcile via root-entry fingerprint diffs                    | `skill_add_operation.rs:880`, `:793`, `:550`                                                    |
| Fork                      | `fork_skill` -> `fork_skill_with_storage`                                                   | Writes ForkRecord before detaching; `rollback_fork_before_detach` on pre-detach failure                                         | `skill_fork.rs:960`, `:780`, `:683`                                                             |
| Pull                      | `pull_fork_upstream_with` -> `swap_in_pull_result`                                          | Three-way merge in scratch dirs; explicit rollback per step                                                                     | `skill_fork.rs:1236`, `:1157`                                                                   |
| Unfork                    | `unfork_skill_with`                                                                         | `ledger.reinstall`, remove record and trials, remove base snapshot                                                              | `skill_fork.rs:1443`                                                                            |
| Move-aside disable        | `set_deployment_enabled`                                                                    | Holds `EventStoreState` mutex across the move; records `move_aside_*` rows; proceeds when store is `None`                       | `skill_harness_disable.rs:811`, `:349`                                                          |
| Native disable            | `set_harness_enabled` -> `set_harness_enabled_with`                                         | `codex` -> `set_skill_disabled`; `opencode` -> `set_skill_denied`; `claude-code` and `pi`/`cursor`/`grok-build` -> Err          | `skill_harness_disable.rs:601`; `codex_skill_config.rs:176`; `opencode_skill_permission.rs:111` |
| Claude per-skill toggle   | `set_claude_code_enabled_with_registry_writer`                                              | Removes or restores `~/.claude/skills/<n>`; registry key `deployment/<id>`                                                      | `skill_harness_disable.rs:442`                                                                  |
| Shared-root reader toggle | `set_shared_harness_skill_enabled` -> `unlink_harness` / `relink_harness`                   | Refused on whole-dir link until `explode_shared_dir`                                                                            | `event_commands.rs:130`, `:187-195`; `skill_materialize.rs:626`, `:87`                          |
| Independent copy          | `make_skill_independent_copy`                                                               | EventStore row first; staged swap; `InverseOp::RecreateSymlink`                                                                 | `skill_independent_copy.rs:66`                                                                  |
| Invocation policy         | `set_skill_invocation` -> `set_skill_invocation_with`                                       | Byte-preserving frontmatter rewrite; Codex `openai.yaml` sidecar when `agent == "Codex"`                                        | `skill_invocation.rs:354`, `:249`, `:90`, `:193`                                                |
| Restore from History      | `restore_skill_event` -> `EventStore::restore`                                              | CAS claim on `reverted_by`; drift guard; SKILL.md lock only when destination is `SKILL.md`                                      | `event_commands.rs:84`; `event_store.rs:383`, `:444`                                            |

## 4. Runtime

### 4.1 Managed state

`lib.rs::run()` (`lib.rs:114`) registers state in `setup`.

| State                    | Type                                                                                                     | Registered at                                 | Location                            |
| ------------------------ | -------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ----------------------------------- |
| `SkillRefreshState`      | snapshot `RwLock`, `rebuild_lock`, project sets, dirty flags, invocation index, facts cache, cache paths | `skill_refresh::init`                         | `skill_refresh.rs:100`, `:237`      |
| `AddSkillOperationState` | `Arc<Mutex<{records, order}>>`; max 32; 30 min TTL                                                       | `lib.rs:120`                                  | `skill_add_operation.rs:129`        |
| `PackImportTrustState`   | `Mutex<BTreeMap<token, PendingPackTrust>>`; 10 min TTL; max 64                                           | setup                                         | `skill_pack.rs:179`                 |
| `SkillAgentRunnerState`  | `runs` map by `run_id`; `binaries` cache                                                                 | setup                                         | `skill_agent_runner.rs:749`         |
| `SkillRunTargetState`    | `targets: Mutex<HashMap<id, PreparedRunTarget>>`                                                         | setup                                         | `skill_run_target.rs:90`            |
| `ForkMutationLock`       | `Mutex<()>` with `try_acquire`                                                                           | setup                                         | `skill_fork.rs:948`                 |
| `UpdateCheckState`       | `in_progress: Arc<Mutex<bool>>`                                                                          | Inside `spawn_update_check_loop`, not `setup` | `skill_update_check.rs:771`, `:874` |
| `EventStoreState`        | `Mutex<Option<EventStore>>`                                                                              | `lib.rs:136`                                  | `event_commands.rs:23`              |

### 4.2 Background loops

| Thread       | Start                     | Cadence                                                                                                                            | Work                                                                                                                           | Location                                         |
| ------------ | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| Refresh loop | `skill_refresh::init`     | `notify` debounce 750 ms; poll 200 ms; invocations-only rate limit 5 s; forced full after 60 s backlog; hourly invocations rebuild | `classify_watch_event` -> dirty flags -> `rebuild_snapshot_now` or `rebuild_invocations_only`                                  | `skill_refresh.rs:720`, `:1299`, `:845`, `:1324` |
| Update check | `spawn_update_check_loop` | 10 s delay, then every 6 h                                                                                                         | `gh api` on a 4-thread scoped pool; writes `update-check.json`; `request_snapshot_rebuild`                                     | `skill_update_check.rs:872`, `:796`, `:611`      |
| Trial expiry | `spawn_trial_expiry_loop` | 15 s delay, then every 5 min                                                                                                       | `ForkMutationLock.try_acquire` (skips tick if busy); inline `rebuild_snapshot_now`; `expire_one` via `run_npx`; emit per trial | `skill_trial.rs:828`, `:798`, `:243`             |
| Add worker   | `spawn_operation`         | Per request                                                                                                                        | `spawn_blocking`; `try_state::<ForkMutationLock>()`                                                                            | `skill_add_operation.rs:793`                     |
| Agent run    | `start_skill_agent_run`   | Per request                                                                                                                        | `tokio::spawn(run_and_deregister)`                                                                                             | `skill_agent_runner.rs:956`, `:1008`             |

### 4.3 Events emitted

| Event name                     | Payload                                   | Emitter                  | Rust location                          | TS subscriber                                           |
| ------------------------------ | ----------------------------------------- | ------------------------ | -------------------------------------- | ------------------------------------------------------- |
| `skills://snapshot`            | `SkillSnapshot`                           | `publish_skill_snapshot` | `skill_refresh.rs:38`, `:406`          | `skill-api.ts:590`; `hooks/useSkillSnapshot.ts:104`     |
| `skills://trial-expired`       | `{name, trash_path}`                      | `run_and_emit`           | `skill_trial.rs:819`                   | `skill-api.ts:415`; `App.tsx:86`                        |
| `skills://add-skill-operation` | `AddSkillOperationEvent`                  | `emit_status`            | `skill_add_operation.rs:39`, `:328`    | `skill-api.ts:309,361`; `hooks/useAddSkillOperation.ts` |
| `skill-agent://event`          | `SkillAgentEvent {run_id, seq, at, kind}` | `EventSink` closure      | `skill_agent_runner.rs:27`, `:985-989` | `skill-agent-api.ts:13,57`; `hooks/useSkillAgentRun.ts` |

Snapshot revision rule (`skill_refresh.rs:406-434`): `store_skill_snapshot`
sets `revision = prev + 1` inside the write guard. The frontend accepts a
candidate only when `candidate.revision > current.revision`
(`hooks/useSkillSnapshot.ts:16`). The frontend subscribes before it reads
(`hooks/useSkillSnapshot.ts:29-55`).

### 4.4 Process control

| Runner                                  | Style                                                                  | Cancel                                                           | Limits                                           | Location                                       |
| --------------------------------------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------ | ---------------------------------------------- |
| `skill_process::run_controlled_command` | sync `std::process`; `process_group(0)`; stdin null; two drain threads | `AtomicBool` + deadline; SIGTERM group, 2 s grace, SIGKILL, wait | 64 KiB per pipe; 20 ms `try_wait`; default 300 s | `skill_process.rs:187`, `:145`, `:36`          |
| `run_controlled_npx[_with_control]`     | Wrapper over the above                                                 | From `AddOperationControl.remaining()`                           | Same                                             | `skill_process.rs:423`, `:433`                 |
| `skill_agent_runner::run_process`       | `tokio::process`; `kill_on_drop`; `process_group(0)`                   | `Notify`; `tokio::select!`; `terminate_process_group`            | 4 MiB line cap; 4 KiB stderr tail; 2 s grace     | `skill_agent_runner.rs:1053`, `:1236`, `:1270` |
| Harness binary lookup                   | `$SHELL -lc 'command -v <bin>'`; cached per process                    | None                                                             | `pick_executable_line`                           | `skill_agent_runner.rs:808`, `:773`            |
| `gh` lookup                             | Same shell approach; not cached                                        | None                                                             | None                                             | `skill_update_check.rs:377`                    |
| Run-target git                          | plain `Command::output`                                                | None                                                             | No timeout                                       | `skill_run_target.rs:298`                      |
| Reveal                                  | `open -R`                                                              | None                                                             | No timeout                                       | `skill_run_target.rs:445`                      |

`HarnessId::bin_name` (`skill_agent_runner.rs:54`): `claude`, `codex`,
`opencode2`, `pi`.

### 4.5 Runtime file locations

| Data                  | Path                                                                                                     | Location                                                        |
| --------------------- | -------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| Invocation cache      | `<app_data_dir>/skill-invocations.json`                                                                  | `skill_refresh.rs:707`                                          |
| Update check store    | `<app_data_dir>/skill-studio/update-check.json`                                                          | `skill_update_check.rs:121`                                     |
| Run history           | `<app_data_dir>/skill-studio/runs/<skill>/<id>.json`, `<id>.events.jsonl`, `last.json`; max 20 per skill | `skill_run_history.rs:1-10`, `:95-143`                          |
| Fork base snapshot    | `<app_data_dir>/skill-studio/forks/<name>/base`                                                          | `skill_fork_registry.rs:332`                                    |
| Scratch and worktrees | `<app_cache_dir>/skill-studio/{scratch,worktrees}`                                                       | `skill_agent_runner.rs:1364`; `skill_run_target.rs:255`, `:318` |
| Event store           | `<app_data_dir>/events.sqlite3`                                                                          | `lib.rs:17`                                                     |

## 5. Tauri coupling inventory

Every non-command function that takes `AppHandle`, `App`, `State`, or
calls `emit`.

| Function                                                 | Signature (short)                                                                  | What it needs from Tauri                                                                                    | Location                          |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | --------------------------------- |
| `skill_refresh::init`                                    | `(app: &AppHandle) -> SkillRefreshState`                                           | `app.path().app_data_dir()`; spawns the loop with an `AppHandle` clone                                      | `skill_refresh.rs:237`            |
| `skill_refresh::run_refresh_loop`                        | `(app: AppHandle, state: SkillRefreshState)`                                       | Passes `app` to rebuild and publish                                                                         | `skill_refresh.rs:720`            |
| `skill_refresh::rebuild_snapshot_now`                    | `(app: &AppHandle, state: &SkillRefreshState) -> Result<SkillSnapshot, String>`    | Only to call `publish_skill_snapshot`                                                                       | `skill_refresh.rs:354`            |
| `skill_refresh::publish_skill_snapshot`                  | `(app, state, built) -> Result<SkillSnapshot, String>`                             | `app.emit(SNAPSHOT_EVENT)`                                                                                  | `skill_refresh.rs:406`            |
| `skill_refresh::request_snapshot_rebuild`                | `(app: &AppHandle)`                                                                | `app.try_state::<SkillRefreshState>()`                                                                      | `skill_refresh.rs:288`            |
| `skill_refresh::patch_snapshot_and_emit`                 | `(app, state, patch: impl FnOnce(&mut SkillSnapshot)) -> Result<(), String>`       | Publish                                                                                                     | `skill_refresh.rs:442`            |
| `skill_refresh::reconcile_skill_names_and_emit`          | `(app, state, names, affected_projects: &[PathBuf]) -> Result<(), String>`         | Publish                                                                                                     | `skill_refresh.rs:472`            |
| `skill_refresh::rebuild_invocations_only`                | `(app, state) -> Result<(), String>`                                               | Publish                                                                                                     | `skill_refresh.rs:617`            |
| `skill_refresh::invocation_cache_path`                   | `(app: &AppHandle) -> PathBuf`                                                     | `app.path().app_data_dir()`                                                                                 | `skill_refresh.rs:707`            |
| `skill_lifecycle::resolve_fresh_lifecycle_target`        | `(app, refresh_state, target, action) -> Result<FreshLifecycleResolution, String>` | Via `rebuild_snapshot_now`                                                                                  | `skill_lifecycle.rs:108`          |
| `skill_lifecycle::rebuild_fresh_lifecycle_snapshot`      | `(app, refresh_state) -> Result<SkillSnapshot, String>`                            | Via `rebuild_snapshot_now`                                                                                  | `skill_lifecycle.rs:125`          |
| `skill_add::resolve_fetch_and_lookup`                    | `(app: &AppHandle) -> Result<GithubTools, String>`                                 | `app.path().app_data_dir()`                                                                                 | `skill_add.rs:1307`               |
| `skill_add_operation::emit_status`                       | `(app: Option<&AppHandle>, event)`                                                 | `app.emit`                                                                                                  | `skill_add_operation.rs:328`      |
| `skill_add_operation::publish`                           | `(app: Option<&AppHandle>, state, operation_id, phase, message, patch)`            | Via `emit_status`                                                                                           | `skill_add_operation.rs:334`      |
| `skill_add_operation::reconcile_affected`                | `(app: Option<&AppHandle>, names, projects)`                                       | `try_state::<SkillRefreshState>`, rebuild or reconcile                                                      | `skill_add_operation.rs:529`      |
| `skill_add_operation::run_operation_body`                | `(app: Option<&AppHandle>, state, operation_id, home, runner, fetch, lookup)`      | Via `publish` and `reconcile_affected`                                                                      | `skill_add_operation.rs:550`      |
| `skill_add_operation::spawn_operation`                   | `(app: AppHandle, state, operation_id)`                                            | `spawn_blocking`; `try_state::<ForkMutationLock>()`                                                         | `skill_add_operation.rs:793`      |
| `skill_agent_runner` EventSink closure                   | Captures `AppHandle`                                                               | `app.emit(SKILL_AGENT_EVENT)`                                                                               | `skill_agent_runner.rs:985-989`   |
| `skill_agent_runner::scratch_root`                       | `(app: &AppHandle) -> Result<PathBuf, String>`                                     | `app.path().app_cache_dir()`                                                                                | `skill_agent_runner.rs:1364`      |
| `skill_run_target::RunTargetRoots::from_app`             | `(app: &AppHandle)`                                                                | Cache dir                                                                                                   | `skill_run_target.rs:115`         |
| `skill_run_target::scratch_root` / `worktree_root`       | `(app: &AppHandle)`                                                                | Cache dir                                                                                                   | `skill_run_target.rs:255`, `:318` |
| `skill_run_target::prepare_scratch` / `prepare_worktree` | `(app: &AppHandle, ...)`                                                           | Cache dir                                                                                                   | `skill_run_target.rs:263`, `:326` |
| `skill_run_history::runs_root`                           | `(app: &AppHandle) -> Result<PathBuf, String>`                                     | `app.path().app_data_dir()`                                                                                 | `skill_run_history.rs:69`         |
| `skill_trial::run_and_emit`                              | `(app: &AppHandle)`                                                                | `app.state::<ForkMutationLock>()`, `app.state::<SkillRefreshState>()`, `app.emit("skills://trial-expired")` | `skill_trial.rs:798`              |
| `skill_trial::spawn_trial_expiry_loop`                   | `(app: AppHandle)`                                                                 | Thread with handle                                                                                          | `skill_trial.rs:828`              |
| `skill_update_check::check_now`                          | `(app, state: &UpdateCheckState, refresh_state)`                                   | `app.path().app_data_dir()`                                                                                 | `skill_update_check.rs:796`       |
| `skill_update_check::check_now_for_owner`                | `(app, state, owner_id, project_paths)`                                            | Same                                                                                                        | `skill_update_check.rs:833`       |
| `skill_update_check::spawn_update_check_loop`            | `(app: AppHandle)`                                                                 | `app.manage(UpdateCheckState)`, `app.state::<SkillRefreshState>()`                                          | `skill_update_check.rs:872`       |
| `skill_pack::RealPublishConfirm<'a>`                     | `{ app: &'a AppHandle }` impl `PublishConfirm`                                     | `app.dialog().message().blocking_show()`                                                                    | `skill_pack.rs:657`               |
| `lib.rs::open_event_store`                               | `(app: &tauri::App) -> Option<EventStore>`                                         | `app.path().app_data_dir()`; runs startup recovery                                                          | `lib.rs:17`                       |

Coupling inside command bodies only (not separate functions):

| Command                                            | Coupling                                         | Location                              |
| -------------------------------------------------- | ------------------------------------------------ | ------------------------------------- |
| `fork_skill`, `pull_fork_upstream`, `unfork_skill` | `app.path().app_data_dir()`                      | `skill_fork.rs:966`, `:1414`, `:1480` |
| `set_deployment_enabled`                           | `tauri::State<EventStoreState>`                  | `skill_harness_disable.rs:816`        |
| `apply_skill_frontmatter_repair`                   | `app.path().app_data_dir()` for the fork service | `skill_frontmatter_repair.rs:484-487` |

Modules with no Tauri types: `agents.rs`, `codex_skill_config.rs`,
`opencode_skill_permission.rs`, `plugins.rs`, `skill_invocations.rs`,
`event_store.rs`, `skill_md_write.rs`, `skill_discovery.rs`,
`skill_assembly.rs`, `skill_ownership.rs`, `provenance.rs`, all DTOs.
`build_snapshot` takes `home: &Path`.

`dirs::home_dir()` call sites: `skill_refresh.rs:333`, `:363`, `:501`,
`:623`, `:721`; `lock_file.rs:44`; `event_commands.rs:65`. No `HOME` env
reads were found in `src`.

## 6. Policy that lives in TypeScript today

All wire types live in `packages/lib/src/`. IPC wrappers live in
`apps/desktop/src/lib/`. `appStore.ts` holds UI state only and issues no
IPC calls (`apps/desktop/src/store/appStore.ts:62-206`).

| Module                                   | Decision it makes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Duplicates a Rust rule?                                                         | Location                                                                                                                                                                             |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `skill-api.ts`                           | `setHarnessEnabled` throws when `target.deployment_id` is absent                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Rust `exact_target`-style checks                                                | `apps/desktop/src/lib/skill-api.ts:456`                                                                                                                                              |
| `skill-lifecycle-target.ts`              | Resolves an `InstalledSkill` to a `LifecycleTarget`. Prefers one mutable `owner_id`; throws on more than one; falls back to the single canonical deployment, then the single deployment, then synthesizes `owner:v1/global/<name>` for a lock-only skills-sh skill. `skillRemovalPreview`: on dotagents removal keeps only Claude Code per-skill links as dependents. `skillUpdateAvailability`: exactly one mutable owner in scope. `lifecycleTargetForPark`: parked deployment else global universal canonical non-plugin. `lifecycleTargetForTrial`: `trial.deployment_id` else scope fallback.                                                                                                                                                                                                                                                                                                                    | Owner-id format (`skill_ownership.rs:269`)                                      | `apps/desktop/src/lib/skill-lifecycle-target.ts:44`, `:116`, `:241`, `:179`, `:294`, `:313`                                                                                          |
| `skill-page-actions.ts`                  | `sharedFolderDeployment` picks the fork target by path substring `/.agents/skills/` or `/.claude/skills/`. Primary action: `Pull latest` for fork with updates; `Update` for dotagents or skills-sh with updates. Fork action: `Un-fork` for fork, `Fork` when a shared-folder deployment exists. Uses Tauri `ask` for confirms.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | No                                                                              | `apps/desktop/src/components/SkillDetail/skill-page-actions.ts:36`, `:255-270`, `:10`                                                                                                |
| `skill-location-actions.ts`              | `convert-root` -> convert only; `set-enabled false` on `shared_via_whole_dir_link` -> convert-then-disable (root = parent dir). `set-enabled`: `studio-moved` or `!canToggleHarness` -> `set_deployment_enabled`; else resolvable reader not `shared` -> `set_harness_enabled`; else reject. `promote-global` -> `add_skill` copy/universal/global. `install-again` requires GitHub source with repo.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | No                                                                              | `apps/desktop/src/components/SkillDetail/skill-location-actions.ts:79-109`, `:178-196`, `:199-215`, `:254-269`                                                                       |
| `skill-location-helpers.ts`              | `HARNESSES_WITH_PER_SKILL_DISABLE = [codex, open-code, claude-code]`. `canToggleHarness`: those only; a disabled row is always re-enableable; else needs global scope and, for claude-code, `is_symlink`. `sharedFolderSwitchPolicy`: only the Global group may park or unpark.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | `set_harness_enabled_with` support set (`skill_harness_disable.rs:601`)         | `apps/desktop/src/components/SkillDetail/skill-location-helpers.ts:14`, `:30`, `:41`                                                                                                 |
| `skill-location-status.ts` (961 lines)   | Severity `RANK` error > warning > off. `READERS_WITH_A_SWITCH = [codex, open-code]`. `fileEditability`: plugin no; global shared folder yes; dotagents or skills-sh no; else yes. `liveElsewhere`. `specCondition` splits blocking vs non-blocking. `offCondition` keyed on `disabled_by`. `deploymentConditions`: broken link, unreadable link, spec, drift, parked-off, disabled. `buildScopeGroups`: groups by `project_path`; row kind plugin, link, copy; synthesizes reader rows for `AGENTS_READING_SHARED_ROOT_ORDER` (codex, open-code, pi, cursor, grok-build); `allOff` rollup. `titleLink` precedence unpark > compare > install-again > enable-everywhere > update. `promoteToGlobal`: two or more project groups and no global. `rowMenu`: `Make independent copy` only for a healthy linked-to link; `Remove link` only for non-root links; `Remove <harness> copy` only when `owner_kind === "copy"`. | Reader list (no Rust export found)                                              | `apps/desktop/src/components/SkillDetail/skill-location-status.ts:37`, `:40`, `:165`, `:218`, `:223`, `:252`, `:365`, `:496`, `:611`, `:647`, `:701`, `:722`, `:747`, `:761`, `:833` |
| `skill-health.ts`                        | `HEALTH_ISSUE_KIND_ORDER`; `FIRST_CLASS_AGENTS` labels; `SHARED_ROOT_READERS` (Codex, OpenCode, pi, Cursor, Grok Build). `findDuplicateSkills`: more than one distinct `content_hash`; strict-majority reference. `findBrokenSymlinks`. `BLOCKING_SPEC_VIOLATION_PREFIXES` matches Rust message prefixes. `findLinkedRootIssues`: one per (harness, root). `coverageGaps` skips parked and disabled. `findParkedButReinstalled`. `collectDashboardIssues` concatenates and sorts.                                                                                                                                                                                                                                                                                                                                                                                                                                     | Spec message prefixes (`frontmatter.rs:227-246`); reader list; first-class list | `packages/lib/src/skill-health.ts:49`, `:96`, `:110`, `:145`, `:183`, `:208`, `:257`, `:297`, `:338`, `:356`                                                                         |
| `skill-coverage.ts`                      | `AGENTS_READING_SHARED_ROOT` (codex, open-code, pi, cursor, grok-build). `agentIdFromDeploymentLabel` reverse-maps display names to ids. `isOwnDirDeployment`. `isLinkedToSharedRoot` regex `/\.agents/skills/`. `deploymentLinkKind`: broken, shared-root, linked-to-shared, own. Drift = `content_hash` differs from truth. `isSharedRootDeployment` honours `disabled_readers`. `skillVisibleToAgent`: own > shared (readers only) > none.                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | `AgentId::display_name` (`agents.rs:116`); reader list                          | `packages/lib/src/skill-coverage.ts:16`, `:32`, `:59`, `:69`, `:81`, `:91`, `:152`, `:179`, `:321`, `:340`, `:373`, `:512`                                                           |
| `skill-install-destination.ts`           | `PER_HARNESS_DESTINATIONS` hardcodes six harness paths. `normalizeInstallHarnesses`: universal keeps only claude-code. `installDestinationError`: per-harness needs one or more. `installTrialError`: trial only for universal.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | `agents.rs:165-258`; `skill_install_plan.rs:59`; `skill_add.rs:1134`            | `packages/lib/src/skill-install-destination.ts:9`, `:53`, `:67`, `:77`, `:86`                                                                                                        |
| `skill-add-operation-policy.ts`          | `TERMINAL_PHASES`, `CANCELLABLE_PHASES`. `selectNewerAddSkillOperationEvent`: same `operation_id` and greater `sequence`. `shouldConsumeAddSkillOperation`: once per terminal id, never needs-trust. `addSkillFinishAction`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Sequence rule mirrors `advance_locked` (`skill_add_operation.rs:129`)           | `packages/lib/src/skill-add-operation-policy.ts:8`, `:15`, `:38`, `:52`, `:92`                                                                                                       |
| `skill-source-parse.ts`                  | `parseSkillSource`: `git:` or `.git` -> git; `~/` or `/` -> local; github.com URL forms; skills.sh URL; bare `owner/repo[/path]`. Parsing happens only in TS. Rust only validates (`skill_add.rs:242`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | No Rust parser                                                                  | `packages/lib/src/skill-source-parse.ts:35`                                                                                                                                          |
| `skill-plugin-partition.ts`              | `ownDeployments = !plugin`; `editableDeployments = own && !is_symlink && !shared_via_whole_dir_link`; `skillMdPathForDeployment`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No                                                                              | `packages/lib/src/skill-plugin-partition.ts:19`, `:29`, `:40`, `:59`                                                                                                                 |
| `skill-list-filter.ts`                   | `matchesScope`: parked only under `parked`; global includes plugin scope. `matchesHarness` uses coverage semantics. Source `plugin` means has a plugin deployment. Issue filter via `collectDashboardIssues`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | No                                                                              | `packages/lib/src/skill-list-filter.ts:43`, `:54`, `:80`                                                                                                                             |
| `skill-pack-name.ts`                     | `PACK_NAME_PATTERN /^[a-z0-9][a-z0-9-]*$/`, max 64                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `validate_pack_name` (`skill_pack.rs:186-189`)                                  | `packages/lib/src/skill-pack-name.ts:7-17`                                                                                                                                           |
| `skill-path-format.ts`                   | `homeRelativePath` guesses home from `/Users` or `/home`; `parentDirectory` derives a root from a deployment path for `materialize_harness_root`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No                                                                              | `packages/lib/src/skill-path-format.ts:21`, `:29`, `:38`                                                                                                                             |
| `skill-assistant-harness-policy.ts`      | Default harness = claude-code if visible, else first visible                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | No                                                                              | `apps/desktop/src/components/SkillDetail/skill-assistant-harness-policy.ts:17`                                                                                                       |
| `skill-editor-save.ts`                   | Compare-and-swap baseline advance                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Pairs with `write_installed_skill_md_if_unchanged`                              | `apps/desktop/src/components/SkillDetail/skill-editor-save.ts:14`                                                                                                                    |
| `installed-skill-source-ledger-model.ts` | Maps `source_kind` / `owner_kind` to labels                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | No                                                                              | `apps/desktop/src/components/SkillDetail/installed-skill-source-ledger-model.ts:41-85`                                                                                               |
| `skill-frontmatter-repair-policy.ts`     | Picks action labels from `preview.allowed_apply_modes`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | No; Rust decides modes                                                          | `apps/desktop/src/components/SkillDetail/skill-frontmatter-repair-policy.ts:7`, `:17`                                                                                                |

### 6.1 IPC surface

Four wrapper modules call 71 Tauri commands. Command list with wire shapes:
`apps/desktop/src/lib/skill-api.ts:41-590`, `skill-agent-api.ts:24-57`,
`skill-run-target-api.ts:16-39`, `skill-run-history-api.ts:15-28`.
`packages/lib` has zero Tauri imports. One component imports `ask` from
`@tauri-apps/plugin-dialog` directly (`skill-page-actions.ts:10`).

## 7. DTOs in `skill_dto.rs` and their TS mirrors

TS locations are in `packages/lib/src/skill-types.ts` unless stated.

| Rust DTO                                      | Rust line                            | TS mirror                                                               | TS line                                    | Notes                                            |
| --------------------------------------------- | ------------------------------------ | ----------------------------------------------------------------------- | ------------------------------------------ | ------------------------------------------------ |
| `DisabledBy`                                  | `skill_dto.rs:28`                    | `DisabledBy`                                                            | L191                                       | Four variants                                    |
| `SkillSearchResult`                           | `skill_dto.rs:41`                    | `SkillSearchResult`                                                     | L97                                        | skills.sh hit                                    |
| `PaginatedSkillsResponse`                     | after `:41`; exact line **Unknown**  | `PaginatedSkillsResponse`                                               | L110                                       | `has_more`                                       |
| `SkillDetails`                                | after `:41`; exact line **Unknown**  | `SkillDetails`                                                          | L118                                       | `skill_md` body                                  |
| `SkillsShAccessInfo`                          | after `:41`; exact line **Unknown**  | `SkillsShAccessInfo`                                                    | L133                                       | mode direct or server                            |
| `SkillEventDto`                               | `skill_dto.rs:87`                    | `SkillEvent`                                                            | L717                                       | Built by `dto_from_row` (`event_commands.rs:34`) |
| `PluginInfo`                                  | `skill_dto.rs:118`                   | `PluginInfo`                                                            | L179                                       | `harness` is a display string                    |
| `Deployment`                                  | `skill_dto.rs:127`                   | `Deployment`                                                            | L199-260                                   | `agent` is a label, not `AgentId`                |
| `ForkInfo`                                    | `skill_dto.rs:255`                   | `ForkInfo`                                                              | L166                                       |                                                  |
| `InstalledSkill`                              | `skill_dto.rs:265`                   | `InstalledSkill`                                                        | L272-326                                   |                                                  |
| `TrialInfo`                                   | `skill_dto.rs:384`                   | `TrialInfo`                                                             | L332                                       |                                                  |
| `OwnerUpdateInfo`                             | `skill_dto.rs:399`                   | `OwnerUpdateInfo`                                                       | L344                                       |                                                  |
| `ParsedSkillSource` / `ParsedSkillSourceKind` | `skill_dto.rs:417`                   | `ParsedSkillSource`                                                     | `packages/lib/src/skill-source-parse.ts:8` | camelCase; `ref` renamed `git_ref`               |
| `AddSkillRequest`                             | after `:417`; exact line **Unknown** | `AddSkillRequest`                                                       | L448                                       |                                                  |
| `AddSkillsRequest`                            | after `:417`; exact line **Unknown** | `AddSkillsRequest`                                                      | L494                                       |                                                  |
| `AddSkillOutcome`                             | after `:417`; exact line **Unknown** | `AddSkillOutcome`                                                       | L511                                       |                                                  |
| `AddSkillResult`                              | after `:417`; exact line **Unknown** | `AddSkillResult`                                                        | L463                                       |                                                  |
| `InstallScope`                                | `skill_dto.rs:505`                   | `InstallScope`                                                          | L365                                       |                                                  |
| `LifecycleTarget`                             | `skill_dto.rs:512`                   | `LifecycleTarget`                                                       | L383                                       | TS uses `never` to exclude the other key         |
| `HarnessVisibilityTarget`                     | `skill_dto.rs:522`                   | `HarnessVisibilityTarget`                                               | L404                                       |                                                  |
| `InstallResult`                               | after `:522`; exact line **Unknown** | `InstallResult`                                                         | L412                                       |                                                  |
| `InstallProgress`                             | after `:522`; exact line **Unknown** | No direct mirror found. `InstallProgressState` (L631) is frontend-only. | L631                                       | **Unknown** whether the Rust type is still sent  |

Types defined outside `skill_dto.rs` that also cross the wire, with TS
mirrors: `AgentId` (`agents.rs:16`; TS L26), `AgentTarget`
(`agents.rs:324`; TS L74), `SourceKind` (`provenance.rs:27`; TS
`SkillSourceKind` L150), `LifecycleOwnerKind` (`skill_ownership.rs:27`; TS
L371), `SkillDestination` (`skill_deployment.rs:21`; TS L368),
`InvocationPolicy` (`frontmatter.rs:69`; TS L266), `SkillSnapshot`
(`skill_refresh.rs:61`; TS L684), `FrontmatterRepairPreview` and
`FrontmatterRepairApplyMode` (`skill_frontmatter_repair.rs:35`; TS L391,
L388), `AddMethodDefaults` (`add_method_defaults.rs:55`; TS L434),
`ForkRecord` (`skill_fork_registry.rs:136`; TS L521), `AddSkillOperationEvent`
(`skill_add_operation.rs:86`; TS `skill-add-operation-types.ts`),
`SkillAgentEvent` (`skill_agent_runner.rs:99`; TS `skill-agent-types.ts`),
`SkillRunTargetInfo` (`skill_run_target.rs:53`; TS
`skill-run-target-types.ts`), `SkillRunRecord` / `SkillRunSummary`
(`skill_run_history.rs:35`, `:63`; TS `skill-run-history-types.ts`),
`SkillInvocation` / `SkillInvocationStats` / `InvocationHeatmap`
(`skill_invocations.rs:22`, `:32`, `:48`; TS L648, L658, L675),
`UpdateCheckSummary` (TS L705; Rust line **Unknown**). `EditorOption` is
declared inline in `skill-api.ts:185`.

## 8. Open questions

Merged from all areas. Duplicates removed.

### 8.1 Harness identity

1. `AgentId` has 43 variants. `agents.rs:3` says 42. Is GrokBuild a late
   addition? Are the 37 non-first-class variants used anywhere except
   `get_agent_targets` and `cli_name` pass-through to `npx skills`?
2. OpenCode: serde `open-code` vs `cli_name` `opencode`. Hand-patched at
   `skill_harness_disable.rs:620`, `event_commands.rs:157`, `:226`.
   `disabled_readers` emits `open-code` (`skill_refresh.rs:1152`). Which
   spelling is canonical on the wire?
3. `Deployment.agent` is a display string, compared literally in
   `skill_refresh.rs:1105-1143`, `skill_invocation.rs:336,372`,
   `event_commands.rs:161,230`. `shared` and `parked` are labels, not
   variants. Should they become variants or stay labels? The wire carries
   two names for one concept: `agent: "shared"` and
   `destination: "universal"`.
4. Cursor and Grok Build are in `FIRST_CLASS_AGENTS`, `harness_config_dirs`,
   and `PLUGIN_CACHE_ROOTS`, but have no disable or invocation-control
   mechanism. Cursor's documented reading of `.claude` and `.codex` skill
   dirs (`docs/agent-skill-conventions.md:113`) was not found in code.
5. pi, Cursor, and Grok Build are absent from `disabled_readers`. Intended?
6. `skill_invocations.rs` hard-codes `Claude Code` and `~/.claude/projects`.
   Codex, OpenCode, and pi session stores
   (`docs/agent-skill-conventions.md:132-135`) are not read. In scope?
7. OpenCode legacy `skill/` dir is added in `skill_roots` outside `AgentId`
   path methods. `AgentId::global_path` / `project_path` are not the full
   source of truth.
8. `.grok/plugins` layout is marked unknown (`plugins.rs:39`) and scanned
   with the generic depth-3 walk. Verified?
9. The shared-root reader set is defined three times in TS
   (`skill-coverage.ts:16`, `skill-health.ts:110`,
   `skill-location-status.ts:647`) and the per-skill-disable set twice
   (`skill-location-helpers.ts:14`, `skill-location-status.ts:40`). No Rust
   constant exports either list. Which becomes the single source?
10. `PER_HARNESS_DESTINATIONS` (`skill-install-destination.ts:9`) duplicates
    `agents.rs:165-258`. `AddMethodDefaults` carries no paths. Should paths
    come from a command?

### 8.2 Read path

11. `build_snapshot` reads the home lock via `lock_file::read_lock_file()`
    (`dirs::home_dir`) although it has `home`. `load_ownership_ledgers`
    reads the same file again. Two reads per rebuild. The first cannot be
    redirected in tests.
12. `assemble_installed_skills` uses `classify_source_kind` only when
    `ledgers` is empty. Production always has the home ledger. Is
    `provenance::classify_source_kind` dead on the read path?
13. Aggregate facts come from the first readable candidate in scan order.
    The order is implicit (from `skill_roots`), not declared.
14. `SkillCandidate.scope` doc says `global | project | plugin`; `scope_str`
    also emits `parked`. `Deployment.scope` has the same gap.
15. Four places re-implement the `.agents/skills` component test:
    `shared_root_has_lock_entry`, `provenance::resolves_into_dotagents`,
    `skill_deployment::path_is_under_universal_skills`, and the whole-dir
    check in `build_candidate`.
16. `discover_named_skill_candidates_cached` and `discover_into` duplicate
    the plugin-candidate block verbatim (`skill_discovery.rs:948-999` vs
    `:1042-1094`).
17. `isBlockingSpecViolation` (`skill-health.ts:208`) matches Rust message
    prefixes. Rust has no blocking flag on the DTO. Is a structured
    violation kind in scope?

### 8.3 Write path

18. `rebuild_snapshot_now` needs `AppHandle` only to call
    `publish_skill_snapshot`, which calls `app.emit`. (Area 3 raised this
    as unknown; area 2 answers it.)
19. `ForkMutationLock` is `try_lock`. A second command during a background
    Add fails. The Add worker acquires it only when
    `app.try_state::<ForkMutationLock>()` succeeds
    (`skill_add_operation.rs:855`); else it runs unlocked.
20. History is split between the registry and the EventStore. Add records
    nothing in the EventStore.
21. `set_deployment_enabled` proceeds when `EventStoreState` is `None`.
    `make_skill_independent_copy` and `explode_shared_dir` require a store.
22. Owner-target resolution exists in `resolve_lifecycle_target`, but park,
    fork, pull, unfork, and `set_deployment_enabled` refuse `owner_id`. The
    owner-wide consumers are `update_skill` and `remove_skill` in
    `commands.rs`.
23. `park_target_skill` and `fork_skill` re-run checks the fresh resolution
    already did. `unpark_skill` and `set_harness_enabled` skip
    `resolve_lifecycle_target` and have no fingerprint check.
24. Legacy registry records (empty `deployment_id` or `skill_dir`,
    `harness_disabled` keyed by name) are handled in fallback branches at
    `skill_park.rs:513`, `skill_fork.rs:1528`, `:1273`,
    `skill_harness_disable.rs:456`.
25. `skill-lifecycle-target.ts:135` synthesizes `owner:v1/global/<name>` for
    lock-only skills. Should Rust expose an `owner_id` on lock-only skills?

### 8.4 Events and repair

26. Event kinds and statuses are bare strings across five files. No shared
    enum. The spec's kind list (install, remove, park) and the emitted list
    differ.
27. Two fingerprint schemes: `event_store::fingerprint_path` (typed hex,
    `absent` sentinel) and `skill_frontmatter_repair::content_fingerprint`
    (`sha256:<hex>`). The repair event stores both for one `SKILL.md`.
28. Startup recovery is per-kind and hardwired in `lib.rs::open_event_store`.
    Interrupted rows of other kinds rely on manual restore.
29. `restore()` takes the SKILL.md lock only when the destination file name
    is `SKILL.md`. `UndistributeFromShared` and multi-path inverses bypass
    it.
30. `record`, `finish`, and `patch_*` are separate autocommit statements. No
    SQLite transaction wraps claim, record, and finish. `finish` does not
    check the prior status.
31. Lock ordering between `ForkMutationLock`, `EventStoreState`, and
    `SKILL_MD_WRITE_LOCK` is by convention only.
    `apply_skill_frontmatter_repair` holds the store lock across the fork
    service and the write.
32. Schema migration is probe-and-ALTER with no version row.
33. `atomic_replace_skill_md_unlocked` requires the target to exist. It
    cannot create a new `SKILL.md`.

### 8.5 Runtime

34. The trial expiry thread runs a full `rebuild_snapshot_now` every 5 min
    (`skill_trial.rs:812`) even with no trial due. Each tick makes a new
    revision.
35. Run-target git and `open -R` have no timeout, cancel, or process group.
36. `resolve_binary` caches for the process lifetime with no invalidation.
    `resolve_gh_binary` runs a login shell on every call.
37. If the tokio runtime drops a run task, `kill_on_drop` kills the child
    but no `Finished` event is emitted.
38. `UpdateCheckState` is managed inside `spawn_update_check_loop`, so
    command handlers depend on that ordering.
39. The task listed three Tauri events. The code has four. Is
    `skill-agent://event` in scope?

### 8.6 Survey scope notes

40. `apps/desktop/src/lib/skill-types.ts` and
    `apps/desktop/src/components/SkillDetail/skill-lifecycle-target.ts` do
    not exist. Types are in `packages/lib/src/skill-types.ts`; the
    lifecycle module is `apps/desktop/src/lib/skill-lifecycle-target.ts`.
41. `skill_editor.rs` has no relation to events or `SKILL.md` writes. It
    persists the editor choice in the fork registry.
42. Two surveys cite different lines for `set_deployment_enabled`:
    `skill_harness_disable.rs:811` (command) and `:781` (snapshot patch
    site named from the TS call stack). Both are recorded as given.

## Critic findings

Completeness review dated 2026-09-09 against `harness-primitives.md` and the
code under `apps/desktop/src-tauri/src/skills`. Lists below come from `grep`
on that directory. Line numbers drift.

### C.1 `dirs::home_dir()` call sites

Section 5 lists 7 call sites. The code has 53 in 17 files. No file reads
`std::env::var("HOME")`. `SHELL` is read at `skill_update_check.rs:378`,
`skill_agent_runner.rs:819`; `PATH` at `add_method_defaults.rs:113`.

| File                          | Lines                                                                                                           |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `add_method_defaults.rs`      | 112                                                                                                             |
| `commands.rs`                 | 110, 119, 129, 140, 148, 1471, 1487 (`unwrap_or_default`), 1925, 1966, 2027, 2072, 2358, 2378, 2386, 2392, 2410 |
| `event_commands.rs`           | 72, 98, 445                                                                                                     |
| `lock_file.rs`                | 44                                                                                                              |
| `skill_add.rs`                | 1340, 1357                                                                                                      |
| `skill_add_operation.rs`      | 795, 1062                                                                                                       |
| `skill_fork.rs`               | 969, 1413, 1480                                                                                                 |
| `skill_frontmatter_repair.rs` | 452, 483                                                                                                        |
| `skill_harness_disable.rs`    | 749, 866                                                                                                        |
| `skill_pack.rs`               | 1615, 1630, 1646, 1665, 1676, 1706, 1731, 1739                                                                  |
| `skill_park.rs`               | 470, 505                                                                                                        |
| `skill_refresh.rs`            | 333, 363, 501, 623, 721                                                                                         |
| `skill_trial.rs`              | 799, 858, 993                                                                                                   |
| `skill_update_check.rs`       | 801, 844                                                                                                        |

### C.2 Event name strings

Three `skills://` names plus one outside the prefix.

| Event                                   | Location                    |
| --------------------------------------- | --------------------------- |
| `skills://snapshot`                     | `skill_refresh.rs:38`       |
| `skills://trial-expired`                | `skill_trial.rs:820`        |
| `skills://add-skill-operation`          | `skill_add_operation.rs:39` |
| `skill-agent://event` (not `skills://`) | `skill_agent_runner.rs:27`  |

### C.3 `#[tauri::command]` functions (70)

Section 6.1 says 71. The grep finds 70 in `skills/`. The 71st is
unverified.

| File                          | Commands                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `add_method_defaults.rs`      | `get_add_method_defaults` :111                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `commands.rs`                 | `get_skills_sh_access` :109, `set_skills_sh_api_key` :118, `search_skills` :125, `get_popular_skills` :136, `get_skill_details` :147, `get_installed_skills` :161, `list_skill_projects` :1462, `is_skill_installed` :1480, `get_agent_targets` :1486, `remove_skill` :1875, `read_installed_skill_md` :2241, `write_installed_skill_md` :2311, `write_installed_skill_md_if_unchanged` :2329, `open_skill_path` :2346, `list_installed_editors` :2377, `get_preferred_editor` :2385, `set_preferred_editor` :2391, `update_skill` :2402 |
| `event_commands.rs`           | `list_skill_events` :65, `restore_skill_event` :84, `set_shared_harness_skill_enabled` :130, `materialize_harness_root` :273, `materialize_harness_root_then_disable` :314, `make_skill_independent_copy` :390, `repair_skill_link` :529                                                                                                                                                                                                                                                                                                 |
| `github_skill_listing.rs`     | `list_github_skills` :325                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `skill_add.rs`                | `add_skills` :1334, `add_skill` :1349                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `skill_add_operation.rs`      | `start_add_skill_operation` :880, `start_add_skills_operation` :898, `get_add_skill_operation` :916, `cancel_add_skill_operation` :926, `confirm_add_skill_trust` :1054                                                                                                                                                                                                                                                                                                                                                                  |
| `skill_agent_runner.rs`       | `start_skill_agent_run` :956, `cancel_skill_agent_run` :1270, `create_skill_scratch_dir` :1377, `remove_skill_scratch_dir` :1438                                                                                                                                                                                                                                                                                                                                                                                                         |
| `skill_fork.rs`               | `fork_skill` :960, `pull_fork_upstream` :1404, `unfork_skill` :1471                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `skill_frontmatter_repair.rs` | `preview_skill_frontmatter_repair` :280, `apply_skill_frontmatter_repair` :406                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `skill_harness_disable.rs`    | `set_harness_enabled` :741, `set_deployment_enabled` :811                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `skill_invocation.rs`         | `set_skill_invocation` :354                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `skill_pack.rs`               | `create_skill_pack` :1608, `update_skill_pack` :1624, `publish_skill_pack` :1639, `delete_skill_pack` :1660, `import_skill_pack` :1670, `confirm_skill_pack_trust` :1699, `abandon_pack_import_trust` :1727, `list_skill_packs` :1738                                                                                                                                                                                                                                                                                                    |
| `skill_park.rs`               | `park_skill` :463, `unpark_skill` :498                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `skill_refresh.rs`            | `get_skill_snapshot` :274, `request_skill_rescan` :281, `register_skill_projects` :329, `unregister_skill_project` :344                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `skill_run_history.rs`        | `record_skill_run` :84, `list_skill_runs` :214, `read_skill_run_events` :252                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `skill_run_target.rs`         | `prepare_skill_run_target` :173, `reveal_skill_run_target` :445, `skill_run_target_diff` :479, `apply_skill_run_target_diff` :607, `discard_skill_run_target` :737                                                                                                                                                                                                                                                                                                                                                                       |
| `skill_trial.rs`              | `keep_skill_trial` :843, `restore_trashed_skill` :985                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `skill_update_check.rs`       | `check_skill_updates_now` :891                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |

### C.4 Gaps for the shared-core design

Harness facts not covered:

1. Cursor and Grok Build are first-class in code (`FIRST_CLASS_AGENTS`,
   reader lists) but have no section in `harness-primitives.md`. Roots,
   disable, plugin cache, and shared-root reading are all unsourced.
2. pi discovers `SKILL.md` recursively in `.agents/skills`. The move-aside
   holding dir `.agents/skills/.skill-studio-disabled/` may still be read
   by pi. Neither doc checks this. Same question for `skills-parked`.
3. Codex follows symlinks; whether Claude Code, OpenCode, and pi follow
   per-skill or whole-dir symlinks is unknown. The link-based disable and
   the `BackingRelationship` model depend on it.
4. Roots the scanner never visits: Claude Code managed, synced,
   `--add-dir`, nested `<subdir>/.claude/skills`; Codex `/etc/codex/skills`
   and `.agents/skills` in parent dirs; pi settings `skills` array,
   `--skill`, npm-package skills.
5. Lock file: `$XDG_STATE_HOME/skills/.skill-lock.json` and project
   `skills-lock.json` are documented upstream but `lock_file.rs` reads only
   `~/.agents/.skill-lock.json`. `sourceUrl` is required by
   `lock_file.rs:17` but unverified upstream.
6. Codex `agents/openai.yaml`: code writes `policy.allow_implicit_invocation`
   (`skill_invocation.rs:188`); the harness doc cites the key without the
   `policy` nesting. Nesting unverified.
7. Codex project-scope `[[skills.config]]` and pi's on-disk per-skill key
   are unknown; the core cannot offer project-scope native disable for
   either.
8. Usage observation exists only for Claude Code and relies on an
   undocumented transcript shape.

Code areas not mapped in sections 1-7:

9. `skill_pack.rs` (8 commands, `PackImportTrustState`, publish confirm
   dialog): no write-path entry.
10. skills.sh client and key storage (`api.rs`, `commands.rs:109-160`,
    `github_skill_listing.rs`): where the key lives and how the server
    mode is resolved.
11. `remove_skill` / `update_skill` (`commands.rs:1875`, `:2402`): the only
    owner-wide flows, listed by name only.
12. Editor and SKILL.md CAS path (`commands.rs:2241-2340`,
    `skill_editor.rs`), `open_skill_path` shells out to `open`
    (`commands.rs:2346`): macOS-only, no adapter abstraction.
13. `skill_install_plan.rs`, `dotagents_ledger.rs`, `skill_candidate.rs`,
    `skill_update_check.rs` internals are cited but not described.
14. Tauri plugin surface: `tauri_plugin_dialog` in 4 places; frontend
    `ask`. A CLI or MCP adapter needs a confirm port.
15. `lib.rs` `generate_handler!` registration list and the 4 managed-state
    setup order are not enumerated.

Contradictions between the two docs:

16. Codex roots: harness doc says `.agents/skills` only, `.codex/skills`
    closed as not planned; `agents.rs:165/214` and `codex_skill_config.rs`
    use `.codex/skills`.
17. Claude Code disable: harness doc lists native `skillOverrides`; code
    returns `Err` for `claude-code` in `set_harness_enabled_with` and
    removes the link instead. `DisabledBy` has no variant for it.
18. pi disable: harness doc says `pi config` writes a per-skill toggle;
    code returns `Err` for pi.
19. OpenCode legacy `skill/` dir: unknown in harness doc, scanned by
    `agents.rs:372`.
20. Shared-root readers: TS lists Cursor and Grok Build; the harness matrix
    has no row for them. Claude Code "no" matches code.

Claims without a source or with wrong figures:

21. Section 5 home_dir list (7 sites) vs 53 actual; `event_commands.rs:65`
    is the command line, the call is at :72.
22. "71 commands" vs 70 found.
23. DTO lines marked **Unknown** (`PaginatedSkillsResponse`, `AddSkill*`,
    `InstallResult`, `InstallProgress`, `UpdateCheckSummary`).
24. `set_deployment_enabled` cited at both :781 and :811 (item 42).
25. Harness doc `inferred` and `unknown` rows the core would depend on:
    Codex plugin dir layout (`.codex-plugin/`), `.agents/plugins/marketplace.json`,
    `-c` copy alias, `npx skills check` endpoint.
