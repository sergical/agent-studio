# Malformed SKILL.md frontmatter impact

How invalid YAML in `SKILL.md` frontmatter affects discovery and invocation. Complements [agent-skill-conventions.md](./agent-skill-conventions.md) (paths, invocation-control fields). Verified against the linked sources on 2026-09-06. This is loader behavior, not a live timing study.

## What the spec requires

[agentskills.io specification](https://agentskills.io/specification): `SKILL.md` must open with YAML frontmatter; `name` and `description` are required. Progressive disclosure loads **name + description for every skill at startup** (~50–100 tokens each), then the body only after activation.

The reference parser [`skills-ref` `parser.py`](https://github.com/agentskills/agentskills/blob/main/skills-ref/src/skills_ref/parser.py) uses **strictyaml**. Missing `---`, unclosed frontmatter, or `YAMLError` → `ParseError` (`Invalid YAML in frontmatter: …`). [`validate()`](https://github.com/agentskills/agentskills/blob/main/skills-ref/src/skills_ref/validator.py) returns that string; it does not load the skill. Flow collections (`bins: ["gws"]`) fail even though they are valid YAML 1.2 ([googleworkspace/cli#521](https://github.com/googleworkspace/cli/issues/521)).

The client guide ([adding-skills-support](https://agentskills.io/client-implementation/adding-skills-support.md)) treats unquoted colons (`description: Use this skill when: the user asks`) as the common cross-client break. Recommended fallback: quote the scalar or convert to a block scalar, then retry. Completely unparseable YAML → skip and log. Missing/empty `description` → skip (needed for disclosure). Name-format issues → warn and still load.

The spec does **not** define skip-vs-load for a running agent. Each harness chooses.

## Compatibility

| Agent | Parser | Unquoted `key: value: more` | Truly broken YAML | Missing `description` | Explicit invoke | Model invoke | Catalog / tokens |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **agentskills.io / skills-ref** | strictyaml | Fail validate | Fail validate | Fail validate | n/a | n/a | Spec: name+description always in catalog |
| **Claude Code** | Undocumented (lenient) | Load **body, empty metadata** | Same | Body still loads; listing may use first markdown paragraph | `/name` from **directory** still works | No `description` → no auto-match | Listing always includes names; descriptions truncated (1% context budget; 1,536-char cap with `when_to_use`) |
| **Codex** | `serde_yaml` + line repair | **Repaired** (single-quoted) | `SkillParseError::InvalidYaml`; not in loaded set | `missing field \`description\``; not loaded | `$name` only if parse succeeded | Implicit list only loaded skills | Name + description + path; ≤2% context or 8,000 chars; descriptions shortened first |
| **OpenCode** | gray-matter, then `sanitize` → `\|-` | **Repaired** | Session error event + log; **skip** | Current loader: `description` optional; catalog `fmt()` **omits** skills with no description | `skill({ name })` → not found if skipped | Same catalog | `<available_skills>` is name+description |
| **pi** | `yaml` parse, **no** colon repair | **Skip** + warning | Skip + warning | Skip + warning | `/skill:name` only if loaded | Hidden if `disable-model-invocation`; else name+description+location in system prompt | XML catalog of loaded skills only |

Sources: [Claude Code skills](https://code.claude.com/docs/en/skills) (Troubleshooting: malformed YAML); [Codex build skills](https://developers.openai.com/codex/skills); [`codex-rs/skills/src/parser.rs`](https://github.com/openai/codex/blob/main/codex-rs/skills/src/parser.rs) (`aef295cd`); [`parser_tests.rs`](https://github.com/openai/codex/blob/main/codex-rs/skills/src/parser_tests.rs); [OpenCode skills](https://opencode.ai/docs/skills/); `packages/opencode/src/skill/index.ts` on `dev` (`5a04ec21`); [`packages/core/src/config/markdown.ts`](https://github.com/anomalyco/opencode/blob/2a33addd/packages/core/src/config/markdown.ts); [pi skills](https://pi.dev/docs/latest/skills); [`packages/coding-agent/src/core/skills.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/skills.ts) `loadSkillFromFile`; [`frontmatter.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/frontmatter.ts); [pi#4725](https://github.com/earendil-works/pi/issues/4725) / [pi#4532](https://github.com/earendil-works/pi/issues/4532) (maintainers reject non-YAML fallbacks).

Claude Code source is closed. Empty-metadata behavior is **documented**, not measured here. Codex host UI for `LoadedSkills.errors` is not documented; parse failure still means the skill is absent from `skills()` used by `$` selection ([`selection.rs`](https://github.com/openai/codex/blob/main/codex-rs/skills/src/selection.rs)).

## `description: … Triggers on: …`

**YAML:** An unquoted scalar with `: ` is a compact mapping. That is the usual authoring bug, not the words “Triggers on”.

Quoted or block form is valid for every parser above:

```yaml
description: "Does X. Triggers on: review, PR, diff."
# or
description: |-
  Does X. Triggers on: review, PR, diff.
```

**Semantic (selection):** `description` is the always-loaded pointer. Agents match tasks against it ([optimizing-descriptions](https://agentskills.io/skill-creation/optimizing-descriptions.md); Claude and Codex docs). “Triggers on:” is author convention for branches. If YAML parses, those words affect **whether the model picks the skill**, not CPU. If YAML does not parse, Claude keeps `/name` but loses auto-invoke; Codex/OpenCode/pi drop the skill from the catalog (OpenCode/Codex often repair this exact colon case).

**Tokens:** Catalog cost is length of **successfully parsed** descriptions (Claude/Codex also truncate). A skipped skill pays **no** catalog tokens. No harness documents extra runtime from a failed parse beyond one-time load + a log/event.

Do not claim measured latency or energy numbers. Observable effects are skip, empty metadata, catalog omission, and selection quality.

## Safe repair forms

| Form | Compatible with |
| --- | --- |
| Double- or single-quoted scalar | All four agents + skills-ref (escape inner quotes) |
| Literal `\|` / `\|-` | All four; Codex tests keep block bodies while repairing sibling keys |
| Folded `>` / `>-` | YAML 1.2; OpenCode sanitize leaves existing `>`/`\|` alone |
| JSON flow `[]` / `{}` | Claude/Codex/OpenCode/pi likely; **skills-ref fails** |

Prefer quoting the `description` line, or `description: \|-` plus indented text. Do not introduce flow collections in `metadata` if `skills-ref validate` matters.

## Product: preview-diff repair

Treat unquoted `: ` in `name`/`description` (and other top-level scalars) as a **correctness/discoverability** fix, not a performance tune.

1. Show a unified diff of frontmatter only. Do not rewrite the markdown body.
2. Default rewrite: wrap the offending scalar in double quotes, or convert that one key to `\|-`. Keep `name` a single line (directory / slash command).
3. Call out harness split: Claude already slash-invokes; repair restores **auto-invoke** and Codex/OpenCode/pi **load**. pi will not load until YAML is actually valid.
4. If parse still fails after quote/block, stop; do not invent keys. Surface skills-ref / `claude plugin validate` / `--debug` as follow-up.
5. After apply, re-run Skill Studio `spec_violations` and, if available, `skills-ref validate`. Unexpected spec keys (`disable-model-invocation`, `argument-hint`) are a **different** issue from malformed YAML.

Assumption: “malformed” here means YAML that a 1.2 mapping parser rejects, not spec field allowlists or name/directory mismatch.
