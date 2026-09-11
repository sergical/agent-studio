// ============================================================================
// Skill Studio TUI - integration test against the real built CLI
// Spawns the actual `skill-studio` binary (not a fake) against a hand-built
// `--home` fixture directory, proving `run()`'s envelope parsing works
// against real CLI output, not just the shape this package assumes.
//
// Fixture materialization: a temp dir with a real on-disk
// `.claude/skills/<name>/SKILL.md`, valid per
// `crates/skill-studio-core/src/frontmatter.rs`'s required `name`/
// `description` frontmatter fields, passed via `--home`. Not `--fixture`:
// that flag expects the Rust `FixtureBuilder`'s in-memory-built shape, which
// isn't reproducible from TypeScript without extra tooling.
// ============================================================================

import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, test } from "bun:test";

import { run } from "../../src/cli-transport.ts";
import type { Inventory } from "../../src/cli-types.ts";

const REPO_ROOT = new URL("../../../..", import.meta.url).pathname;
const CLI_BIN = join(REPO_ROOT, "target/debug/skill-studio");

let homeDir: string;

beforeAll(async () => {
  homeDir = await mkdtemp(join(tmpdir(), "skill-studio-tui-integration-"));
  const skillDir = join(homeDir, ".claude", "skills", "write-tests");
  await mkdir(skillDir, { recursive: true });
  await writeFile(
    join(skillDir, "SKILL.md"),
    "---\nname: write-tests\ndescription: Writes tests for the current change.\n---\n\n# Write Tests\n",
  );
});

afterAll(async () => {
  await rm(homeDir, { recursive: true, force: true });
});

describe("cli-transport against the real skill-studio binary", () => {
  test("scan parses the real envelope and inventory shape", async () => {
    const envelope = await run<Inventory>("scan", [], { bin: CLI_BIN, home: homeDir });
    expect(envelope.status).toBe("ok");
    expect(envelope.operation).toBe("scan");
    expect(envelope.data?.skills).toHaveLength(1);
    expect(envelope.data?.skills[0]?.name).toBe("write-tests");
    expect(envelope.data?.skills[0]?.deployments[0]?.harness).toBe("claude-code");
  });

  test("diagnose parses the real inventory-plus-issues shape", async () => {
    const envelope = await run<{ inventory: Inventory; issues: unknown[] }>("diagnose", [], {
      bin: CLI_BIN,
      home: homeDir,
    });
    expect(envelope.status).toBe("ok");
    expect(envelope.data?.inventory.skills).toHaveLength(1);
    expect(Array.isArray(envelope.data?.issues)).toBe(true);
  });
});
