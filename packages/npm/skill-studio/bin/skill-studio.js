#!/usr/bin/env node
// Runs the native skill-studio binary from the platform package that npm
// installed through optionalDependencies (@skill-studio/cli-<os>-<arch>).
"use strict";

const { spawn } = require("node:child_process");
const { constants } = require("node:os");

const platformPackage = `@skill-studio/cli-${process.platform}-${process.arch}`;

let binaryPath;
try {
  binaryPath = require.resolve(`${platformPackage}/bin/skill-studio`);
} catch {
  process.stderr.write(
    `skill-studio: could not find ${platformPackage}.\n` +
      "skill-studio runs on macOS only, on Apple silicon (arm64) and Intel (x64) Macs.\n" +
      "On a Mac, install it again without skipping optional packages:\n" +
      "  npm install -g skill-studio --include=optional\n",
  );
  process.exit(1);
}

// Async spawn, not spawnSync: `skill-studio mcp` is a long-lived stdio server,
// and the launcher must stay responsive to forward signals to it.
const child = spawn(binaryPath, process.argv.slice(2), { stdio: "inherit" });

const forwardedSignals = ["SIGINT", "SIGTERM", "SIGHUP"];
for (const signal of forwardedSignals) {
  process.on(signal, () => child.kill(signal));
}

child.on("error", (error) => {
  process.stderr.write(`skill-studio: could not start ${binaryPath}: ${error.message}\n`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Die from the same signal so the caller sees the real cause. The
    // forwarding handlers must go first, or they would swallow it. Node
    // ignores some signals (SIGPIPE), so also set the shell's 128+N code.
    for (const forwarded of forwardedSignals) process.removeAllListeners(forwarded);
    process.exitCode = 128 + (constants.signals[signal] ?? 0);
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
