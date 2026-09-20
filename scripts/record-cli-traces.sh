#!/bin/sh
# Records nine real `npx skills` runs as before/after fixtures under
# `crates/skill-studio-core/tests/fixtures/cli-traces/`, for
# `crates/skill-studio-core/tests/cli_parity.rs` to replay against
# `skill_studio_core::ops` with no network. Re-run this script whenever the
# pinned CLI version changes to refresh the fixtures.
#
# Recorder truth: `run_cli` is the only place `command.txt` and each trace's
# `exit_status` are written, and it writes exactly the argv/cwd it is about
# to execute - never a hand-typed duplicate of what `ops` is expected to
# build. `RECORD_TO` must be set only around the one call a trace replays;
# every setup call (installing a fixture's starting state before the traced
# operation) runs with `RECORD_TO` unset.
#
# Safety, enforced by this script itself (never relaxed by an argument):
#   - every `npx` call runs under a fresh `mktemp -d` HOME; the guard below
#     refuses to run npx unless $HOME starts with that trace's temp prefix.
#   - GH_TOKEN/GITHUB_TOKEN are unset so the CLI cannot pick up a real token.
#   - DO_NOT_TRACK/DISABLE_TELEMETRY are set.
#   - the real `~/.npm` is reused read/write-through `npm_config_cache` only,
#     to avoid re-downloading the npm registry cache for every trace - it
#     never holds `~/.agents`, `~/.claude`, `~/.codex`, `~/.cursor`, or
#     `~/.config/opencode` state. `XDG_DATA_HOME` points at a dedicated cache
#     directory under the real home's `.cache`, never the real
#     `~/.local/share` (OpenCode keeps its own real data there) - it exists
#     only to let `npx`'s vite-plus wrapper reuse its downloaded Node
#     runtime across traces instead of re-fetching hundreds of MB each time.
#   - a `trap` removes every `mktemp -d` directory this run created, even on
#     failure or interrupt.
#   - the CLI is pinned to `skills@1.7.0`.
set -eu

CLI_VERSION="skills@1.7.0"
REAL_HOME="$HOME"
REAL_NPM_CACHE="$REAL_HOME/.npm"
XDG_DATA_DIR="$REAL_HOME/.cache/skill-studio-cli-trace-recorder/xdg-data"
mkdir -p "$XDG_DATA_DIR"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TRACES_ROOT="$REPO_ROOT/crates/skill-studio-core/tests/fixtures/cli-traces"
NODE_BIN="$REAL_HOME/.vite-plus/bin/node"
if [ ! -x "$NODE_BIN" ]; then
  NODE_BIN="node"
fi

mkdir -p "$TRACES_ROOT"

# Every `mktemp -d` this run creates is appended here (space-separated), and
# removed by the EXIT/INT/TERM trap below - so an interrupted or failed run
# never leaves a temp HOME/PROJECT/local-skill-source directory behind.
CLEANUP_DIRS=""
track_tmp() {
  d=$(mktemp -d)
  CLEANUP_DIRS="$CLEANUP_DIRS $d"
  echo "$d"
}
cleanup() {
  # shellcheck disable=SC2086
  [ -n "$CLEANUP_DIRS" ] && rm -rf $CLEANUP_DIRS
}
trap cleanup EXIT INT TERM

# Guard: refuses to let npx run unless $HOME is one of this run's own temp
# directories. Every trace function sets HOME to a fresh tracked temp dir
# right before calling this.
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
#
# Sets `CLI_STDOUT` and `CLI_EXIT_STATUS` for the caller to read - the real
# exit status is never discarded or replaced with a fixed 0.
#
# When `RECORD_TO` is set (only around the single call a trace replays),
# also writes `$RECORD_TO/command.txt`: the exact argv passed to this `npx`
# invocation, one token per line, then a `--cwd--` marker and the raw `cwd`
# argument (empty for none) - the only source of truth `cli_parity.rs` reads
# for what really ran. Never hand-typed elsewhere.
run_cli() {
  cwd="$1"
  shift
  guard_temp_home
  set +e
  CLI_STDOUT=$(
    unset GH_TOKEN GITHUB_TOKEN
    export DO_NOT_TRACK=1 DISABLE_TELEMETRY=1
    export npm_config_cache="$REAL_NPM_CACHE"
    export XDG_DATA_HOME="$XDG_DATA_DIR"
    if [ -n "$cwd" ]; then cd "$cwd"; fi
    npx --yes "$CLI_VERSION" "$@" 2>&1
  )
  CLI_EXIT_STATUS=$?
  set -e
  if [ -n "${RECORD_TO:-}" ]; then
    {
      # `ops` calls the spawner as `npx skills <subcommand-args...>` -
      # "skills" is one of its own argv tokens, not an npx-level flag (see
      # `cli_args_and_cwd`/`remove_cli_args_and_cwd`/`update_cli_args_and_cwd`
      # in `ops_install_cli.rs`/`ops_remove.rs`/`ops_update.rs`). This
      # recorder instead runs `npx --yes skills@1.7.0 <subcommand-args...>`
      # for reproducibility, so "skills" is prepended here to make the
      # *recorded* argv match the shape `ops` actually builds, without
      # changing what real command executes above.
      echo "skills"
      for a in "$@"; do printf '%s\n' "$a"; done
      echo "--cwd--"
      printf '%s\n' "$cwd"
    } > "$RECORD_TO/command.txt"
  fi
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

# Reads stdin, applies every "from" "to" pair given as arguments (an even
# number of args), and writes the result to stdout. For each pair, the
# macOS `/private` alias of `from` (`/private$from`) is always redacted
# *before* the plain `from`, so a path the OS reports through its
# `/private/var/...` alias never survives as a half-redacted leak.
redact_stream() {
  script=""
  while [ "$#" -ge 2 ]; do
    from="$1"
    to="$2"
    shift 2
    [ -z "$from" ] && continue
    script="${script}s#/private${from}#${to}#g; s#${from}#${to}#g; "
  done
  if [ -z "$script" ]; then cat; else sed "$script"; fi
}

# Builds `<out>/tree.json` (path, kind, sha256-for-a-file,
# normalised-target-for-a-symlink) for every entry under `root`, and copies
# every regular file's bytes to `<out>/files/<relative path>` - with every
# occurrence of a temp HOME/PROJECT/local-skill-source path redacted from
# both the copied file's *content* and any symlink target, not just paths,
# so no fixture can leak a `/var/folders/...` (or its `/private` alias) id
# through a file a real CLI wrote (e.g. a lock file's own absolute `source`
# field for a local-folder install).
#
# Only descends into the agent-relevant top-level names (`.agents`,
# `.claude`, `.codex`, `.cursor`, `.config/opencode`, `skills-lock.json`),
# never the whole HOME/PROJECT tree - `npx`'s own vite-plus wrapper
# bootstraps a multi-hundred-MB Node runtime cache under a fresh HOME's
# `.local/share`/`.cache` on first use regardless of `XDG_DATA_HOME`, which
# would blow the fixture folder's ~1MB budget if it were walked too.
#
# Symlink targets: the real CLI's own raw `readlink` value is neither a
# clean relative nor a clean absolute path (it walks `../` up to the
# filesystem root, then back down the *entire* absolute target) - resolving
# it to a clean absolute path first, then re-expressing it relative to the
# link's own directory (when the target shares this call's own walked
# root), avoids both that shape and the missing-separator bug a naive
# substring splice on the raw value produced.
ALLOWED_TOP_LEVEL='.agents .claude .codex .cursor .config skills-lock.json'
snapshot_tree() {
  root="$1"
  out="$2"
  home_from="$3"
  home_to="$4"
  proj_from="$5"
  proj_to="$6"
  local_skill_from="$7"
  local_skill_to="$8"
  walk_root_token="$9"
  rm -rf "$out"
  mkdir -p "$out/files"
  "$NODE_BIN" -e '
    const fs = require("fs");
    const path = require("path");
    const crypto = require("crypto");
    const [root, out, homeFrom, homeTo, projFrom, projTo, localFrom, localTo, walkRootToken, allowed] = process.argv.slice(1);
    const allowedTop = new Set(allowed.split(" ").filter(Boolean));
    // Longest/most specific prefix first (local skill source can sit inside
    // a directory that also matches home/project), and each pair redacts
    // its `/private` alias before the plain form.
    const pairs = [[localFrom, localTo], [projFrom, projTo], [homeFrom, homeTo]].filter(([f]) => f);
    const normalize = (s) => {
      let r = s;
      for (const [from, to] of pairs) {
        r = r.split("/private" + from).join(to);
        r = r.split(from).join(to);
      }
      return r;
    };
    // Byte-safe (latin1 round-trip) so binary file content is never
    // corrupted by a redaction pass that assumed UTF-8 text.
    const normalizeBytes = (buf) => Buffer.from(normalize(buf.toString("latin1")), "latin1");
    const rootFor = (placeholderAbs) => pairs.find(([, to]) => to === walkRootToken && (placeholderAbs === to || placeholderAbs.startsWith(to + "/")));
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
        const raw = fs.readlinkSync(abs);
        const resolved = path.isAbsolute(raw) ? raw : path.resolve(path.dirname(abs), raw);
        const placeholderAbs = normalize(resolved);
        let target = placeholderAbs;
        const match = rootFor(placeholderAbs);
        if (match) {
          const [, to] = match;
          const targetRootRel = placeholderAbs === to ? "" : placeholderAbs.slice(to.length + 1);
          target = path.relative(path.dirname(rel), targetRootRel) || ".";
        }
        entries.push({ path: rel || ".", kind: "symlink", target });
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
        const bytes = normalizeBytes(fs.readFileSync(abs));
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
  ' "$root" "$out" "$home_from" "$home_to" "$proj_from" "$proj_to" "$local_skill_from" "$local_skill_to" "$walk_root_token" "$ALLOWED_TOP_LEVEL"
}

# Rewrites `command.txt`'s raw cwd line (written by `run_cli`) to the
# `GLOBAL`/`$PROJECT` label `cli_parity.rs` expects, and redacts any
# temp-path argv token (e.g. trace 03's local-folder source path) in place.
finish_command() {
  dir="$1"
  file="$dir/command.txt"
  raw_cwd=$(tail -1 "$file")
  if [ -z "$raw_cwd" ]; then
    label="GLOBAL"
  else
    label='$PROJECT'
  fi
  sed '$d' "$file" > "$file.body"
  {
    redact_stream "${LOCAL_SKILL_DIR:-}" '$LOCAL_SKILL_DIR' "${PROJECT_DIR:-}" '$PROJECT' "$HOME" '$HOME' < "$file.body"
    echo "$label"
  } > "$file"
  rm -f "$file.body"
}

trace_dir() {
  n="$1"
  mkdir -p "$TRACES_ROOT/$n"
  echo "$TRACES_ROOT/$n"
}

write_stdout() {
  dir="$1"
  printf '%s\n' "$CLI_STDOUT" | strip_ansi | redact_stream "${LOCAL_SKILL_DIR:-}" '$LOCAL_SKILL_DIR' "${PROJECT_DIR:-}" '$PROJECT' "$HOME" '$HOME' > "$dir/stdout.txt"
}


# Resolves the current HEAD commit of a public GitHub repo without cloning
# it, for `meta.json`'s `source_commit` field - dropped from `meta.json`
# entirely (see each trace below) if this cannot be resolved honestly.
resolve_commit() {
  git ls-remote "https://github.com/$1" HEAD 2>/dev/null | cut -f1
}

# ---------------------------------------------------------------------------
# Trace 01: add from GitHub, global scope, requesting the Claude Code
# harness - `anthropics/skills`'s `academy-guide` skill.
# ---------------------------------------------------------------------------
trace_01() {
  dir=$(trace_dir "01-add-github-global-claude-code")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent universal --agent claude-code
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  commit=$(resolve_commit anthropics/skills)
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  $( [ -n "$commit" ] && echo "\"source_commit\": \"$commit\"," )
  "exit_status": $CLI_EXIT_STATUS,
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
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" add vercel-labs/agent-skills@web-design-guidelines --yes --global --skill web-design-guidelines --agent universal
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  commit=$(resolve_commit vercel-labs/agent-skills)
  curl -fsSL "https://raw.githubusercontent.com/vercel-labs/agent-skills/main/LICENSE" -o "$dir/LICENSE" 2>/dev/null || true
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "vercel-labs/agent-skills",
  "source_slug": "web-design-guidelines",
  $( [ -n "$commit" ] && echo "\"source_commit\": \"$commit\"," )
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [] (universal root only). LICENSE next to this trace is vercel-labs/agent-skills's own upstream license, covering the vendored SKILL.md content under files/."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 03: add a local folder, project scope. skills@1.7.0's `add` has no
# `--cwd` flag (confirmed against `add --help`, and against a real run: the
# CLI writes wherever the process's own cwd is, ignoring any `--cwd` value)
# - recorded here as the real invocation, cd-ing into $PROJECT first and
# never passing `--cwd`. See KNOWN_DIVERGENCES in cli_parity.rs for whether
# `ops_install_cli.rs` matches this yet.
# ---------------------------------------------------------------------------
trace_03() {
  dir=$(trace_dir "03-add-local-folder-project")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  PROJECT_DIR=$(track_tmp)
  LOCAL_SKILL_DIR="$(track_tmp)/my-skill"
  mkdir -p "$LOCAL_SKILL_DIR"
  cat > "$LOCAL_SKILL_DIR/SKILL.md" <<'EOF'
---
name: my-local-skill
description: A tiny local test skill for CLI trace recording.
---
# My Local Skill
Says hello.
EOF
  snapshot_tree "$PROJECT_DIR" "$dir/before" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "$LOCAL_SKILL_DIR" '$LOCAL_SKILL_DIR' '$PROJECT'
  RECORD_TO="$dir" run_cli "$PROJECT_DIR" add "$LOCAL_SKILL_DIR" --yes --skill my-local-skill --agent universal
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$PROJECT_DIR" "$dir/after" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "$LOCAL_SKILL_DIR" '$LOCAL_SKILL_DIR' '$PROJECT'
  finish_command "$dir"
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source": "local folder (authored for this fixture, not fetched)",
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, project scope. Recorded by cd-ing into \$PROJECT first and never passing --cwd, since skills@1.7.0's add has no working --cwd flag. Whether this still diverges from ops_install_cli.rs's SkillsSh builder is decided in cli_parity.rs's KNOWN_DIVERGENCES, not here."
}
EOF
  unset PROJECT_DIR LOCAL_SKILL_DIR
}

# ---------------------------------------------------------------------------
# Trace 04: add requesting two harnesses at once (Claude Code and an
# unsupported/unrecognized harness id, `cursor`). `ops_install_cli.rs`'s
# `cli_args_and_cwd` only ever special-cases Claude Code - a request naming
# any other harness never reaches the CLI's argv, yet `ops::install` still
# reports the overall install as `Installed`, as if `cursor` had been set up
# too. That silent over-reporting, not the missing `--agent` token itself,
# is the real gap this trace documents (see KNOWN_DIVERGENCES). Sending no
# `--agent` token for a harness that reads the shared universal root by
# design (Codex, OpenCode, pi - see `harness.rs`'s `reads_universal_root`)
# would be correct, not a bug; `cursor` here stands in for a harness id the
# catalog does not recognize at all.
# ---------------------------------------------------------------------------
trace_04() {
  dir=$(trace_dir "04-add-two-harnesses")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" add anthropics/skills --yes --global --skill brand-guidelines --agent universal --agent claude-code
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  commit=$(resolve_commit anthropics/skills)
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  $( [ -n "$commit" ] && echo "\"source_commit\": \"$commit\"," )
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [claude-code, cursor]. cursor is an id the harness catalog does not recognize; ops::install still reports Installed even though nothing was done for it - the real KNOWN_DIVERGENCE."
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
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" add anthropics/skills --yes --global --skill web-artifacts-builder --agent universal
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  commit=$(resolve_commit anthropics/skills)
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  $( [ -n "$commit" ] && echo "\"source_commit\": \"$commit\"," )
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, global scope, requested harnesses = [] (shared universal root only, no per-harness link)."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 06: update, when the source has genuinely changed since install.
# `skills update` cannot find/match a `sourceType: "local"` skill at all
# (confirmed empirically - always "No installed skills found matching"), so
# this must use a GitHub source; the CLI's own `update` stdout always says
# "Updated" regardless of whether bytes actually changed, so the real
# "source changed" case is forced deterministically by corrupting the
# installed file's bytes after `add`, then calling `update` - the CLI
# refetches and restores the canonical upstream content.
# ---------------------------------------------------------------------------
trace_06() {
  dir=$(trace_dir "06-update-newer-source")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  PROJECT_DIR=$(track_tmp)
  run_cli "$PROJECT_DIR" add anthropics/skills --yes --skill academy-guide --agent universal > /dev/null
  printf '\nCORRUPTED FOR TRACE 06\n' >> "$PROJECT_DIR/.agents/skills/academy-guide/SKILL.md"
  snapshot_tree "$PROJECT_DIR" "$dir/before" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "" "" '$PROJECT'
  RECORD_TO="$dir" run_cli "$PROJECT_DIR" update academy-guide
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$PROJECT_DIR" "$dir/after" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "" "" '$PROJECT'
  finish_command "$dir"
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, project scope. academy-guide is installed, then its SKILL.md is corrupted by this script (not the CLI) so update has a real, deterministic content change to refetch and restore - verified by before/after sha256, not stdout wording (skills update always prints Updated regardless of whether content changed)."
}
EOF
  unset PROJECT_DIR
}

# ---------------------------------------------------------------------------
# Trace 07: update, when the installed skill is already current - the same
# update call run a second time with no source change in between.
# ---------------------------------------------------------------------------
trace_07() {
  dir=$(trace_dir "07-update-already-current")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  PROJECT_DIR=$(track_tmp)
  run_cli "$PROJECT_DIR" add anthropics/skills --yes --skill academy-guide --agent universal > /dev/null
  snapshot_tree "$PROJECT_DIR" "$dir/before" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "" "" '$PROJECT'
  RECORD_TO="$dir" run_cli "$PROJECT_DIR" update academy-guide
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$PROJECT_DIR" "$dir/after" "$HOME" '$HOME' "$PROJECT_DIR" '$PROJECT' "" "" '$PROJECT'
  finish_command "$dir"
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "InstallMethod::SkillsSh, project scope. Proves an update call is a no-op on disk (apart from the lock file's updatedAt) when nothing changed - verified by before/after sha256 being identical."
}
EOF
  unset PROJECT_DIR
}

# ---------------------------------------------------------------------------
# Trace 08: remove one of two harnesses installed with `--copy` (each
# harness holds its own physical folder, not a shared universal root +
# symlink). Recorded with the exact positional-name argv `ops_remove.rs`
# actually builds (`remove <name> --yes --global`, no `--skill`/`--agent`
# flag - `RemoveRequest` has no harness selector either). Confirmed
# empirically that this removes the whole multi-harness deployment
# regardless of which single harness a name might otherwise suggest - not a
# CLI rejection, and not a divergence, since core never claims to remove
# only one harness.
# ---------------------------------------------------------------------------
trace_08() {
  dir=$(trace_dir "08-remove-one-harness")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent claude-code --agent cursor --copy > /dev/null
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" remove academy-guide --yes --global
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "exit_status": $CLI_EXIT_STATUS,
  "notes": "Setup installed to two harnesses with --copy (two physical folders). remove is called with the exact positional-name argv ops_remove.rs builds; it removes the whole deployment - matching RemoveRequest's own lack of a harness field, so this is not a divergence."
}
EOF
}

# ---------------------------------------------------------------------------
# Trace 09: remove the skill's only deployment - the lock file entry goes
# entirely, from a single-harness install.
# ---------------------------------------------------------------------------
trace_09() {
  dir=$(trace_dir "09-remove-last-deployment")
  TMP_HOME_PREFIX=$(track_tmp)
  export HOME="$TMP_HOME_PREFIX"
  run_cli "" add anthropics/skills --yes --global --skill academy-guide --agent universal > /dev/null
  snapshot_tree "$HOME" "$dir/before" "$HOME" '$HOME' "" "" "" "" '$HOME'
  RECORD_TO="$dir" run_cli "" remove academy-guide --yes --global
  unset RECORD_TO
  write_stdout "$dir"
  snapshot_tree "$HOME" "$dir/after" "$HOME" '$HOME' "" "" "" "" '$HOME'
  finish_command "$dir"
  cat > "$dir/meta.json" <<EOF
{
  "cli_version": "$CLI_VERSION",
  "source_repo": "anthropics/skills",
  "exit_status": $CLI_EXIT_STATUS,
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
