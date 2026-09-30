#!/usr/bin/env bash
# Rehearses the npm release of the CLI on this Mac, without publishing.
#
# Usage: scripts/rehearse-npm.sh <path to a built skill-studio binary>
#
# Copies the three packages under packages/npm to a temp dir, puts the binary
# into the platform package for this Mac's CPU, packs all three, checks that
# the packed binary keeps its exec bit, installs the platform and main
# tarballs into a temp prefix with a temp HOME, and runs
# `skill-studio --help` and an MCP handshake through the JS launcher. It also
# checks that SIGINT to the launcher stops the native `skill-studio mcp`
# child. Nothing touches the real HOME, ~/.npm or the global npm prefix.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <path to skill-studio binary>" >&2
  exit 1
fi

binary="$1"
if [[ ! -f "$binary" ]]; then
  echo "no file at $binary" >&2
  exit 1
fi

case "$(uname -m)" in
  arm64) platform="cli-darwin-arm64" ;;
  x86_64) platform="cli-darwin-x64" ;;
  *)
    echo "unsupported CPU $(uname -m); the CLI ships for arm64 and x86_64 Macs only" >&2
    exit 1
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cp -R "$repo_root/packages/npm" "$work/src"
mkdir -p "$work/src/$platform/bin"
cp "$binary" "$work/src/$platform/bin/skill-studio"
chmod 755 "$work/src/$platform/bin/skill-studio"

export HOME="$work/home"
export npm_config_cache="$work/npm-cache"
export npm_config_prefix="$work/prefix"
export npm_config_update_notifier=false
mkdir -p "$HOME" "$work/packs"

for package in cli-darwin-arm64 cli-darwin-x64 skill-studio; do
  (cd "$work/packs" && npm pack "$work/src/$package" --silent > /dev/null)
done

platform_tgz="$(ls "$work/packs/skill-studio-$platform-"*.tgz)"
main_tgz="$(ls "$work/packs"/skill-studio-[0-9]*.tgz)"

check_executable() {
  local tgz="$1" path="$2" mode
  mode="$(tar -tvf "$tgz" "package/$path" | awk '{print $1}')"
  if [[ "$mode" != -rwx* ]]; then
    echo "FAIL: $path in $(basename "$tgz") has mode '$mode', expected -rwxr-xr-x" >&2
    exit 1
  fi
  echo "ok: $path in $(basename "$tgz") is $mode"
}
check_executable "$platform_tgz" bin/skill-studio
check_executable "$main_tgz" bin/skill-studio.js

# --omit=optional: the main package pins platform packages that are not on
# npm yet at this version. The platform tarball installs beside it in the
# global prefix, which is where the launcher's require.resolve finds it.
npm install --global --omit=optional --no-audit --no-fund --silent "$platform_tgz" "$main_tgz"

cli="$npm_config_prefix/bin/skill-studio"
if ! "$cli" --help > "$work/help.txt"; then
  echo "FAIL: skill-studio --help exited with an error" >&2
  exit 1
fi
head -n 5 "$work/help.txt"
echo "ok: skill-studio --help ran through the npm launcher"

set +e
"$cli" no-such-command > /dev/null 2>&1
status=$?
set -e
if [[ $status -eq 0 ]]; then
  echo "FAIL: an unknown command exited 0; the launcher is not forwarding the exit code" >&2
  exit 1
fi
echo "ok: an unknown command exits $status through the launcher"

# MCP clients spawn the launcher, not the binary, and stop it with a signal,
# so the stdio pipe and the signal forwarding both need a real round trip.
node - "$cli" "$work" <<'JS'
"use strict";
const { spawn, execFileSync } = require("node:child_process");
const { closeSync, openSync } = require("node:fs");
const { constants } = require("node:os");
const { setTimeout: sleep } = require("node:timers/promises");
const [cli, work] = process.argv.slice(2);

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

function startServer() {
  const proc = spawn(cli, ["mcp"], { stdio: ["pipe", "pipe", "inherit"] });
  const replies = new Map();
  const waiters = new Map();
  let pending = "";
  proc.stdout.on("data", (chunk) => {
    pending += chunk;
    let end;
    while ((end = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, end).trim();
      pending = pending.slice(end + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      replies.set(message.id, message);
      waiters.get(message.id)?.(message);
    }
  });
  const exited = new Promise((resolve) => proc.on("exit", (code, signal) => resolve({ code, signal })));
  return {
    proc,
    exited,
    send: (message) => proc.stdin.write(`${JSON.stringify(message)}\n`),
    reply: (id) =>
      replies.has(id)
        ? Promise.resolve(replies.get(id))
        : new Promise((resolve) => {
            waiters.set(id, resolve);
            setTimeout(() => fail(`no MCP reply to request ${id} within 10 s`), 10000).unref();
          }),
  };
}

const initialize = {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "rehearse-npm", version: "0" } },
};

async function handshake() {
  const server = startServer();
  server.send(initialize);
  server.send({ jsonrpc: "2.0", method: "notifications/initialized" });
  server.send({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  const name = (await server.reply(1)).result?.serverInfo?.name;
  if (name !== "skill-studio") fail(`initialize returned serverInfo.name ${JSON.stringify(name)}`);
  const tools = ((await server.reply(2)).result?.tools ?? []).map((tool) => tool.name);
  for (const expected of ["skill_usage", "park", "unpark"]) {
    if (!tools.includes(expected)) fail(`tools/list has no ${expected}: ${tools.join(", ")}`);
  }
  server.proc.stdin.end();
  const { code } = await server.exited;
  if (code !== 0) fail(`skill-studio mcp exited ${code} after stdin closed`);
  console.log(`ok: MCP handshake through the launcher lists ${tools.length} tools`);
}

async function sigint() {
  // Node destroys a child's stdin pipe when the child exits, so behind a pipe
  // the native server would read EOF and stop by itself even when the
  // launcher forwards nothing. A FIFO held open read-write never hits EOF.
  const fifo = `${work}/mcp-stdin`;
  execFileSync("mkfifo", [fifo]);
  const stdin = openSync(fifo, "r+");
  const launcher = spawn(cli, ["mcp"], { stdio: [stdin, "ignore", "inherit"] });
  const exited = new Promise((resolve) => launcher.on("exit", (code, signal) => resolve({ code, signal })));
  let child = 0;
  for (let attempt = 0; attempt < 50 && !child; attempt += 1) {
    await sleep(100);
    try {
      child = Number(execFileSync("pgrep", ["-P", String(launcher.pid)]).toString().split("\n")[0]);
    } catch {}
  }
  if (!child) fail("the launcher did not start the native skill-studio within 5 s");
  launcher.kill("SIGINT");
  const exit = await Promise.race([exited, sleep(5000).then(() => null)]);
  if (!exit) {
    launcher.kill("SIGKILL");
    fail("the launcher still runs 5 s after SIGINT");
  }
  await sleep(200);
  try {
    process.kill(child, 0);
    process.kill(child, "SIGKILL");
    fail(`the native child ${child} still runs after SIGINT to the launcher`);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
  closeSync(stdin);
  const status = exit.signal ? 128 + constants.signals[exit.signal] : exit.code;
  if (status === 0) fail("the launcher exited 0 after SIGINT");
  console.log(`ok: SIGINT to the launcher stops skill-studio mcp (exit status ${status})`);
}

handshake()
  .then(sigint)
  .catch((error) => fail(error.message));
JS

echo "Rehearsal passed for $platform."
