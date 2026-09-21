# Contributing to Skill Studio

Thank you for your interest in this project.

## Prerequisites

- Node.js `^22.18.0` or `>=24.11.0` (see `engines` in `package.json`)
- Rust `1.92.0` with `rustfmt` and `clippy` (see `.github/workflows/rust.yml`)
- Tauri CLI prerequisites: https://tauri.app/start/prerequisites/

## Setup

```bash
npm install
npm run tauri dev
```

## Local gate

Before you open a pull request, run the full gate:

```bash
npm run check
```

For a small change, the scoped commands below are faster. Run the ones that
match what you touched:

```bash
# TypeScript / frontend
npm run typecheck
npm run lint
npm run format:check

# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks (job `test` from `rust.yml`, job `check` from
`web.yml`) on every pull request.

## How a change lands

1. Fork the repository and create a branch off `main`.
2. Make your change and commit it.
3. Open a pull request against `main`.
4. CI must go green on both required checks, `test` and `check`.
5. If you are an outside contributor, every workflow run from your fork
   waits for a maintainer's approval; CI starts only after they approve it.
   This applies to each push, not just the first one.
6. A maintainer squash-merges the pull request. There is no required review
   count, but a maintainer may still ask for changes.

## Commit and pull request titles

Use [Conventional Commits](https://www.conventionalcommits.org/), for
example `feat(cli): add the outdated command` or `fix(desktop): repair the
park toggle`. Run `git log` to see recent examples from this repository.

## Test rule

Each test name states the flow, the expectation it checks, and the failure
it catches. A new test must fail without its matching production fix — a
test that never goes red proves nothing. Do not assert on wall-clock time.

## UI rule

Form controls and buttons come from `@skill-studio/ui`
(`packages/ui/src/index.ts`). Raw `<input>`, `<textarea>`, `<select>`, and
`<button>` elements are only allowed inside that package; the
`anti-slop/no-raw-form-elements` lint rule enforces this.

## Test safety

Tests must never read or write a real home folder
(`~/.claude`, `~/.codex`, `~/.config/opencode`, and similar). Use a temporary
`HOME` directory or the project's fixture builder.

## Code style

See `CLAUDE.md` (also read by AI agents as `AGENTS.md`) for naming
conventions, file layout, and language-specific style rules. This file does
not repeat them.

## AI-assisted pull requests

AI-assisted pull requests are welcome, as long as the author has read and
run the change themselves before submitting it.

## Where to ask

Open a [GitHub issue](https://github.com/sergical/agent-studio/issues) for
questions, bugs, and feature ideas.
