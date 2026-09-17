# Primitives and the call stack

The eight primitives, how each user job is a short sequence of them, and where each harness's own facts enter. The rule that shapes this file: use the tools the user already has. The app never re-implements a fetch, a merge, or a plugin manager; it calls `npx skills`, `git`, `gh`, `claude plugin`, and the user's editor, and owns only the safe placement of files and the record of what it did.

## The primitives

| Primitive     | Guarantee                                                                                                       | Owns                                                       | Uses on disk                                                                         |
| ------------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Root          | a path handle confined to one root; nothing above it can be touched                                             | path checks, canonical parent, `.agents/skills` validation | none                                                                                 |
| Snapshot      | a read of a root at one instant, with a completeness flag and a budget                                          | scan, TreeHash per skill, the inventory DTO                | reads only                                                                           |
| Stage         | a folder built next to its final place, fsynced, not yet visible                                                | temp folder under the root                                 | `<root>/.skill-studio-stage/<id>`                                                    |
| Swap          | one folder replaces another; the old one is quarantined, never deleted in place; a crash leaves before or after | rename pair                                                | `<root>/.skill-studio-quarantine/<id>`                                               |
| Link          | a symlink created under a temp name and renamed into place; removal recorded before it happens                  | per-skill and whole-folder links                           | `<harness root>/<name>`                                                              |
| WriteFile     | temp, fsync, rename; refuses when the file changed since it was read                                            | every config and ledger write                              | registry, `config.toml`, `opencode.json`, `settings.json`, `openai.yaml`, `SKILL.md` |
| Journal       | the plan is on disk before step one; each step marks done; startup finishes or reverses any open plan           | the event store                                            | app data `events.sqlite`                                                             |
| Lease         | one writer per root at a time; a second gets Busy with pid and age; stale leases expire                         | `FileLease`                                                | `<root>/.skill-studio.lock`                                                          |
| TreeHash      | the git tree SHA of a folder, equal to GitHub's and the skills CLI's                                            | currency checks                                            | reads only                                                                           |
| Source, Fetch | where a skill comes from and how bytes arrive; always through a tool the user has                               | `npx skills`, `gh`, `git`                                  | the tool's own cache                                                                 |

Every write is: plan (pure, from a Snapshot) → Lease → Journal(plan) → steps → Journal(done) → event. The plan is a value the tests can assert on without touching disk.

## How the user jobs compose them

| Job                            | Sequence                                                                                                                                          | Harness facts that enter                                                                                |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Detect harnesses (U1)          | Snapshot of PATH and config files, no write                                                                                                       | harness-detection.md signals per harness                                                                |
| Inventory (U2)                 | Snapshot over every root, TreeHash per skill                                                                                                      | roots and depth per harness; `synced` skipped; `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, XDG honoured          |
| Activity (U3)                  | Snapshot over transcripts and databases, resumed by offset                                                                                        | each adapter's usage reader                                                                             |
| Outdated (U4)                  | TreeHash against the lock file, ledger commit against `gh`, plugin version against the marketplace                                                | shared-root.md ledgers; Claude plugin cache layout                                                      |
| Install (U5)                   | Lease → Journal → Fetch through `npx skills add` into Stage → Swap into `~/.agents/skills` → Link per chosen harness → WriteFile registry → done  | Claude needs a link, Codex and pi read the root, OpenCode reads `.agents/skills` in its precedence list |
| Update (U6)                    | Lease → Journal → Fetch through `npx skills update` into Stage → Swap (old tree to quarantine) → done; TreeHash before and after                  | same                                                                                                    |
| Park (U7)                      | Lease → Journal → Link remove per harness → Swap out of the root into `skills-parked` → WriteFile registry and the harness rows → done            | Codex `[[skills.config]]` path row, OpenCode deny key, kept consistent                                  |
| Unpark                         | the reverse, from the journal entry                                                                                                               | same                                                                                                    |
| Fix (U8)                       | Snapshot → doctor invariants → for each: WriteFile (frontmatter), Link (dangling link), WriteFile (stale row or key)                              | the name rule from agentskills.io; each harness's switch file                                           |
| Conflicts (U9)                 | Snapshot → TreeHash per copy → no write; editor launch with both paths                                                                            | none; the editor is the user's                                                                          |
| Fork                           | Snapshot → Lease → Journal → Swap (copy the tree into Stage, Swap it into place under the new owner) → Link → WriteFile registry `forks`          | fork is Stage plus Swap plus Link plus a registry row; no new primitive                                 |
| Independent copy               | Stage (copy) → Swap the link for the folder → WriteFile registry `copies`                                                                         | same pieces as fork with a different registry row                                                       |
| Turn off for one harness (U10) | Lease → Journal → one of: Link remove (Claude), WriteFile `config.toml` (Codex), WriteFile `opencode.json` (OpenCode) → WriteFile registry → done | each harness file, "How the app turns a skill off"                                                      |
| Undo (U11)                     | Journal entry → reverse each step: Swap back from quarantine, Link restore, WriteFile the previous content                                        | none                                                                                                    |
| Remove (U12)                   | Lease → Journal → Link remove per harness → `npx skills remove` → Swap the tree into quarantine → WriteFile registry → done                       | same as install                                                                                         |

Fork, independent copy, park, and remove all reduce to Swap and Link with a registry row. That is the sharing the user asked for: one implementation of "move a folder safely" serves every job.

## The call stack, top to bottom

```
click "Park" in the desktop            keydown in the CLI            tool call in MCP
        |                                     |                             |
skill-api.ts parkSkill(req) ---- IPC ---> park command        main.rs park      mcp park tool
                                              |                    |               |
                                              +--------- ops::park(req, ctx) ------+
                                                              |
                                              plan = park_plan(snapshot, req)       pure
                                              lease = ctx.lease.acquire(root)       host
                                              ctx.journal.begin(plan)               host
                                              for step in plan: step.run(ctx.fs)    host
                                              ctx.journal.done(plan.id)             host
                                              ctx.events.emit(SkillParked{..})      host
                                                              |
                          desktop: skills://event ---> store applies the event ---> one row re-renders
```

The three surfaces share one `ops` function and one request type. The frontend applies the event to the row it already has; it does not wait for a full snapshot. The full snapshot follows from the background refresh when the watcher sees the change.

## Where each harness enters

A harness is a data table plus five small functions behind one trait:

```
trait HarnessAdapter {
    fn facts(&self) -> &HarnessFacts;          // roots, depth, links, switches, with Evidence per row
    fn detect(&self, probe: &dyn Probe) -> Detection;
    fn roots(&self, home: &Path, project: Option<&Path>) -> Vec<RootSpec>;
    fn read_switch(&self, fs: &dyn Fs, skill: &SkillRef) -> SwitchState;
    fn write_switch(&self, fs: &dyn Fs, skill: &SkillRef, on: bool) -> Plan;
    fn usage_reader(&self) -> Box<dyn UsageReader>;
}
```

`ops` never names a harness. It asks the catalog for every adapter, and each adapter answers from its table. Adding a harness is one file with one table, one fixture home, and five functions; nothing in `ops` changes. Cursor and Grok Build are adapters with `detect` and `roots` only until someone captures their usage format.

| Harness     | detect                                              | roots                                                                           | switch                                 | usage reader         |
| ----------- | --------------------------------------------------- | ------------------------------------------------------------------------------- | -------------------------------------- | -------------------- |
| Claude Code | PATH, `~/.claude.json` fields, `numStartups`        | `~/.claude/skills` one level, project `.claude/skills` and nested, plugin cache | per-skill Link; later `skillOverrides` | transcripts `.jsonl` |
| Codex       | PATH or ChatGPT.app bundle, `config.toml`, rollouts | `~/.agents/skills` and ancestors, `/etc/codex/skills`; `.codex/skills` inferred | `[[skills.config]]` row; `openai.yaml` | rollouts `.jsonl`    |
| OpenCode    | PATH, `opencode.json(c)`, database rows             | the v2 precedence list including `.claude/skills` and the `skills` array        | permission deny; `autoinvoke`          | SQLite read-only     |
| pi          | PATH, `settings.json`, sessions or `trust.json`     | `~/.pi/agent/skills` and `~/.agents/skills`, project roots recursive            | frontmatter only                       | sessions `.jsonl`    |
| Shared root | n/a                                                 | `~/.agents/skills`, `skills-parked`                                             | n/a; park is the switch                | n/a                  |

## What this replaces

Today each desktop module carries its own write path: skill_park.rs, skill_fork.rs, skill_independent_copy.rs, skill_harness_disable.rs, skill_materialize.rs, event_commands.rs, and the legacy paths in commands.rs and skill_add.rs. Each lands its own rename, its own registry write, and its own idea of undo. The map in README.md counts 46 writes with 8 journaled. Under this file, every one of them is a plan over the same eight primitives, and the journal is not optional.
