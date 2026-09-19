#!/bin/sh
# Records nine real `npx skills` runs as before/after fixtures under
# `crates/skill-studio-core/tests/fixtures/cli-traces/`, for
# `crates/skill-studio-core/tests/cli_parity.rs` to replay against
# `skill_studio_core::ops` with no network. Re-run this script whenever the
# pinned CLI version changes to refresh the fixtures.
#
# Safety, enforced by this script itself (never relaxed by an argument):
#   - every `npx` call runs under a fresh `mktemp -d` HOME; the guard below
#     refuses to run npx unless $HOME starts with that trace's temp prefix.
#   - GH_TOKEN/GITHUB_TOKEN are unset so the CLI cannot pick up a real token.
#   - DO_NOT_TRACK/DISABLE_TELEMETRY are set.
#   - the real `~/.npm` and `~/.local/share` are reused read/write-through
#     `npm_config_cache`/`XDG_DATA_HOME` only to avoid re-downloading the
#     npm registry cache and the vite-plus node runtime for every trace -
#     neither holds `~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`, or
#     `~/.config/opencode` state, so reusing them is not a scope violation.
#   - the CLI is pinned to `skills@1.7.0`.
set -eu

CLI_VERSION="skills@1.7.0"
REAL_HOME="$HOME"
REAL_NPM_CACHE="$REAL_HOME/.npm"
REAL_XDG_DATA="$REAL_HOME/.local/share"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TRACES_ROOT="$REPO_ROOT/crates/skill-studio-core/tests/fixtures/cli-traces"
NODE_BIN="$REAL_HOME/.vite-plus/bin/node"
if [ ! -x "$NODE_BIN" ]; then
  NODE_BIN="node"
fi

mkdir -p "$TRACES_ROOT"

# Guard: refuses to let npx run unless $HOME is one of this run's own temp
# directories. Every trace function sets HOME to a fresh `mktemp -d` right
# before calling this.
guard_temp_home() {
  case "$HOME" in
    "$TMP_HOME_PREFIX"*) ;;
    *)
      echo "GUARD: refusing to run npx - HOME ($HOME) is not the trace's own temp directory ($TMP_HOME_PREFIX*)" >&2
      exit 1
      ;;
  esac
}

# Runs one `npx skills@1.7.0 ...` call inside `cwd` (or $HOME when `cwd` is
# empty), with the safety env from the module doc comment. Never reads or
# writes a real `~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`, or
# `~/.config/opencode` - those live under $HOME, which is always this
# trace's own temp directory.
run_cli() {
  cwd="$1"
  shift
  guard_temp_home
  (
    unset GH_TOKEN GITHUB_TOKEN
    export DO_NOT_TRACK=1 DISABLE_TELEMETRY=1
    export npm_config_cache="$REAL_NPM_CACHE"
    export XDG_DATA_HOME="$REAL_XDG_DATA"
    if [ -n "$cwd" ]; then cd "$cwd"; fi
    npx --yes "$CLI_VERSION" "$@"
  )
}

# Strips ANSI escape codes (color, cursor movement, the CLI's spinner
# frames) from stdout, so `stdout.txt` is readable and diff-stable.
strip_ansi() {
  "$NODE_BIN" -e '
    let data = "";
    process.stdin.on("data", c => data += c);
    process.stdin.on("end", () => {
      const stripped = data.replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, "").replace(/\r/g, "");
      process.stdout.write(stripped);
    });
  '
}

# Replaces every occurrence of `$1` with `$2` in stdin, for normalizing a
# temp HOME/PROJECT path (and its `/private` symlink alias on macOS) to the
# placeholder a fixture stores instead.
redact() {
  from="$1"
  to="$2"
  sed "s#$from#$to#g"
}

# Builds `<out>/tree.json` (path, kind, sha256-for-a-file,
# normalised-target-for-a-symlink) for every entry under `root`, and copies
# every regular file's bytes to `<out>/files/<relative path>`. Symlinks are
# never stored as raw links (their targets are the temp HOME/PROJECT, which
# git must not carry) - only their normalised target string, rewritten by
# `home_from`/`home_to` and `proj_from`/`proj_to`.
#
# Only descends into the agent-relevant top-level names (`.agents`,
# `.claude`, `.codex`, `.cursor`, `.config/opencode`, `skills-lock.json`),
# never the whole HOME/PROJECT tree - `npx`'s own vite-plus wrapper
# bootstraps a multi-hundred-MB Node runtime cache under a fresh HOME's
# `.local/share`/`.cache` on first use regardless of `XDG_DATA_HOME`, which
# would blow the fixture folder's ~1MB budget if it were walked too.
ALLOWED_TOP_LEVEL='.agents .claude .codex .cursor .config skills-lock.json'
snapshot_tree() {
  root="$1"
  out="$2"
  home_from="$3"
  home_to="$4"
  proj_from="$5"
  proj_to="$6"
  rm -rf "$out"
  mkdir -p "$out/files"
  "$NODE_BIN" -e '
    const fs = require("fs");
    const path = require("path");
    const crypto = require("crypto");
    const [root, out, homeFrom, homeTo, projFrom, projTo, allowed] = process.argv.slice(1);
    const allowedTop = new Set(allowed.split(" ").filter(Boolean));
    const normalize = (s) => {
      let r = s;
      if (homeFrom) r = r.split(homeFrom).join(homeTo);
      if (projFrom) r = r.split(projFrom).join(projTo);
      return r;
    };
    const entries = [];
    const walk = (rel) => {
      // Below the root, only ever recurse into an allow-listed top-level
      // name (and, under `.config`, only `opencode`) - keeps caches and
      // runtime bootstrap files the CLI itself leaves out of the fixture.
      if (rel === ".config") {
        const abs = path.join(root, rel, "opencode");
        if (fs.existsSync(abs)) walk(path.join(rel, "opencode"));
        return;
      }
      const abs = path.join(root, rel);
      const st = fs.lstatSync(abs);
      if (st.isSymbolicLink()) {
        const target = fs.readlinkSync(abs);
        entries.push({ path: rel || ".", kind: "symlink", target: normalize(target) });
        return;
      }
      if (st.isDirectory()) {
        const names = fs.readdirSync(abs).sort();
        if (names.length === 0 && rel !== "") {
          entries.push({ path: rel, kind: "dir" });
        }
        for (const name of names) {
          if (rel === "" && !allowedTop.has(name)) continue;
          walk(rel ? path.join(rel, name) : name);
        }
        return;
      }
      if (st.isFile()) {
        const bytes = fs.readFileSync(abs);
        const sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
        entries.push({ path: rel, kind: "file", len: bytes.length, sha256 });
        const dest = path.join(out, "files", rel);
        fs.mkdirSync(path.dirname(dest), { recursive: true });
        fs.writeFileSync(dest, bytes);
        return;
      }
      entries.push({ path: rel, kind: "other" });
    };
    if (fs.existsSync(root)) walk("");
    entries.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    fs.writeFileSync(path.join(out, "tree.json"), JSON.stringify(entries, null, 2) + "\n");
  ' "$root" "$out" "$home_from" "$home_to" "$proj_from" "$proj_to" "$ALLOWED_TOP_LEVEL"
}

# Writes `<dir>/command.txt`: the argv this trace's operation ran, one
# token per line, first line the program name, `--`-prefixed CWD_LABEL line
# last (GLOBAL, or the normalised $PROJECT).
write_command() {
  dir="$1"
  cwd_label="$2"
  shift 2
  {
    echo "npx"
    echo "$CLI_VERSION"
    for a in "$@"; do echo "$a"; done
    echo "--cwd--"
    echo "$cwd_label"
  } > "$dir/command.txt"
}

trace_dir() {
  n="$1"
  mkdir -p "$TRACES_ROOT/$n"
  echo "$TRACES_ROOT/$n"
}

# ---------------------------------------------------------------------------
# Trace 01: add from GitHub, global scope, requesting the Claude Code
# harness - `anthropics/skills`'s `academy-guide` skill.
# ---------------------------------------------------------------------------
trace_01() {
  dir=$(trace_dir "01-add-github-global-claude-code")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent universal --agent claude-code --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills add anthropics/skills --yes --global --skill academy-guide --agent universal --agent claude-code
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "source_commit": "$(cat "$dir/../.last-commit-anthropics-skills" 2>/dev/null || echo unknown)",
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [claude-code]."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 02: add from a skills.sh `owner/repo@skill` slug, global scope, no
# extra harness beyond the shared universal root.
# ---------------------------------------------------------------------------
trace_02() {
  dir=$(trace_dir "02-add-skillssh-slug-global")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" add vercel-labs/agent-skills@web-design-guidelines --yes --global --skill web-design-guidelines --agent universal --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills add vercel-labs/agent-skills@web-design-guidelines --yes --global --skill web-design-guidelines --agent universal
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "vercel-labs/agent-skills",
  "source_slug": "web-design-guidelines",
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [] (universal root only)."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 03: add a local folder, project scope. skills@1.7.0's `add` has no
# `--cwd` flag (confirmed against `add --help`); the CLI installs relative
# to its own process cwd, so a project-scope install must run with the
# process cwd already set to the project - unlike `ops_install_cli.rs`'s
# `cli_args_and_cwd`, whose SkillsSh branch never sets `ProcessSpec.cwd` and
# instead pushes a `--cwd` argument the CLI silently ignores. That gap is
# recorded as a KNOWN_DIVERGENCE in `cli_parity.rs`, not fixed here.
# ---------------------------------------------------------------------------
trace_03() {
  dir=$(trace_dir "03-add-local-folder-project")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  project=$(mktemp -d)
  local_skill=$(mktemp -d)/my-skill
  mkdir -p "$local_skill"
  cat > "$local_skill/SKILL.md" <<'EOF'
---
name: my-local-skill
description: A tiny local test skill for CLI trace recording.
---
# My Local Skill
Says hello.
EOF
  snapshot_tree "$project" "$dir/before" "$HOME" '$HOME' "$project" '$PROJECT'
  out=$(run_cli "$project" add "$local_skill" --yes --skill my-local-skill --agent universal --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' | redact "$project" '$PROJECT' > "$dir/stdout.txt"
  snapshot_tree "$project" "$dir/after" "$HOME" '$HOME' "$project" '$PROJECT'
  write_command "$dir" '$PROJECT' skills add "\$LOCAL_SKILL_DIR" --yes --cwd '$PROJECT' --skill my-local-skill --agent universal
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source": "local folder (authored for this fixture, not fetched)",
  "notes": "InstallMethod::SkillsSh, project scope. Recorded by cd-ing into \$PROJECT first, since skills@1.7.0's add has no working --cwd flag; ops_install_cli.rs's SkillsSh builder assumes one exists (see KNOWN_DIVERGENCES). Also: skills@1.7.0 writes this project's lock file at \$PROJECT/skills-lock.json (version 1, computedHash, no timestamps) rather than \$PROJECT/.agents/.skill-lock.json (version 3) - a second KNOWN_DIVERGENCE, since crate::lock_file only reads the latter shape at the latter path."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 04: add requesting two harnesses at once (Claude Code and Cursor).
# `ops_install_cli.rs::cli_args_and_cwd` only ever special-cases Claude
# Code, so a real ops call can never send this exact argv - recorded here
# as the argv a correct two-harness request would need, for `cli_parity.rs`
# to diff against what `ops::install` actually builds.
# ---------------------------------------------------------------------------
trace_04() {
  dir=$(trace_dir "04-add-two-harnesses")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" add anthropics/skills --yes --global --skill brand-guidelines --agent universal --agent claude-code --agent cursor --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills add anthropics/skills --yes --global --skill brand-guidelines --agent universal --agent claude-code --agent cursor
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [claude-code, cursor]. cli_args_and_cwd only ever appends --agent claude-code, silently dropping cursor - a KNOWN_DIVERGENCE."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 05: add with the shared `.agents/skills` root only - no extra
# harness requested at all, distinct from trace 02 by using a GitHub
# source rather than a skills.sh slug.
# ---------------------------------------------------------------------------
trace_05() {
  dir=$(trace_dir "05-add-shared-root-only")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" add anthropics/skills --yes --global --skill web-artifacts-builder --agent universal --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills add anthropics/skills --yes --global --skill web-artifacts-builder --agent universal
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [] (shared universal root only, no per-harness link)."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 06: update, when the source has changed since install - a local
# folder's content is edited between the initial add and the update call,
# so the CLI's own diff/refetch is exercised for real, deterministically
# (no dependency on a remote repo gaining new commits during this run).
# ---------------------------------------------------------------------------
trace_06() {
  dir=$(trace_dir "06-update-newer-source")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  project=$(mktemp -d)
  local_skill=$(mktemp -d)/my-skill
  mkdir -p "$local_skill"
  cat > "$local_skill/SKILL.md" <<'EOF'
---
name: my-update-skill
description: A tiny local test skill, version one.
---
# My Update Skill
Version one body.
EOF
  run_cli "$project" add "$local_skill" --yes --skill my-update-skill --agent universal --json > /dev/null 2>&1
  snapshot_tree "$project" "$dir/before" "$HOME" '$HOME' "$project" '$PROJECT'
  cat > "$local_skill/SKILL.md" <<'EOF'
---
name: my-update-skill
description: A tiny local test skill, version two - the source changed.
---
# My Update Skill
Version two body, now with more content.
EOF
  out=$(run_cli "$project" update my-update-skill --yes --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' | redact "$project" '$PROJECT' > "$dir/stdout.txt"
  snapshot_tree "$project" "$dir/after" "$HOME" '$HOME' "$project" '$PROJECT'
  write_command "$dir" '$PROJECT' skills update my-update-skill
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source": "local folder, edited between add and update",
  "notes": "InstallMethod::SkillsSh, project scope. update_cli_args_and_cwd's SkillsSh branch never passes -y for update - skills@1.7.0's own agent-detection ran non-interactively anyway (see stdout.txt); recorded as-is."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 07: update, when the installed skill is already current - the same
# update call run a second time with no source change in between.
# ---------------------------------------------------------------------------
trace_07() {
  dir=$(trace_dir "07-update-already-current")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  project=$(mktemp -d)
  local_skill=$(mktemp -d)/my-skill
  mkdir -p "$local_skill"
  cat > "$local_skill/SKILL.md" <<'EOF'
---
name: my-current-skill
description: A tiny local test skill that never changes.
---
# My Current Skill
Body.
EOF
  run_cli "$project" add "$local_skill" --yes --skill my-current-skill --agent universal --json > /dev/null 2>&1
  snapshot_tree "$project" "$dir/before" "$HOME" '$HOME' "$project" '$PROJECT'
  out=$(run_cli "$project" update my-current-skill --yes --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' | redact "$project" '$PROJECT' > "$dir/stdout.txt"
  snapshot_tree "$project" "$dir/after" "$HOME" '$HOME' "$project" '$PROJECT'
  write_command "$dir" '$PROJECT' skills update my-current-skill
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source": "local folder, unchanged between add and update",
  "notes": "InstallMethod::SkillsSh, project scope. Proves an update call is a no-op on disk (apart from the lock file's updatedAt) when nothing changed."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 08: remove naming one harness (`--agent claude-code`) out of two the
# skill was installed to (`--copy`, so each harness holds its own physical
# folder rather than a shared universal root + one symlink). skills@1.7.0's
# `remove --agent <name>` removes the whole deployment regardless of which
# single agent is named (confirmed empirically, twice) - recorded as-is;
# `ops::remove`'s own `RemoveRequest` has no harness field either, so this
# is not a divergence, just a real, surprising shared behavior.
# ---------------------------------------------------------------------------
trace_08() {
  dir=$(trace_dir "08-remove-one-harness")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent claude-code --agent cursor --copy --json > /dev/null 2>&1
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" remove --skill academy-guide --global --agent claude-code --yes --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills remove academy-guide --yes --global
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "notes": "Setup installed to two harnesses with --copy (two physical folders). remove_cli_args_and_cwd never sends --agent at all, matching skills@1.7.0's real all-or-nothing remove; naming one harness on the CLI's own remove command made no difference in manual verification, so this is not treated as a divergence."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 09: remove the skill's only deployment - the lock file entry goes
# entirely, from a single-harness install.
# ---------------------------------------------------------------------------
trace_09() {
  dir=$(trace_dir "09-remove-last-deployment")
  TMP_HOME_PREFIX=$(mktemp -d)
  export HOME="$TMP_HOME_PREFIX"
  run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent universal --json > /dev/null 2>&1
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" ""
  out=$(run_cli "" remove --skill academy-guide --global --yes --json 2>&1) || true
  echo "$out" | strip_ansi | redact "$HOME" '$HOME' > "$dir/stdout.txt"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" ""
  write_command "$dir" "GLOBAL" skills remove academy-guide --yes --global
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "notes": "Single-harness setup (universal root only, unlike trace 01's same skill with a claude-code link); removing it drops the .skill-lock.json entry entirely. Uses academy-guide (not canvas-design) to keep the fixture small - canvas-design ships ~5MB of font assets."
}
EOF
}

main() {
  trace_01
  trace_02
  trace_03
  trace_04
  trace_05
  trace_06
  trace_07
  trace_08
  trace_09
  echo "Recorded 9 traces under $TRACES_ROOT"
  du -sh "$TRACES_ROOT"
}

main
