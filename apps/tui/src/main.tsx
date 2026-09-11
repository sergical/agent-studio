// ============================================================================
// Skill Studio TUI - Entry point
// Boots the OpenTUI renderer and mounts `App`. `bun run src/main.tsx` (the
// `dev` script) or `bun run apps/tui/src/main.tsx` from the repo root.
// ============================================================================

import { createCliRenderer } from "@opentui/core";
import { createRoot } from "@opentui/react";

import { App } from "./App.tsx";
import type { ScopeConfig } from "./cli-transport.ts";

/** `--home`/`--fixture`/`--project` come from the environment here, since
 * this is the one entry point real users run; tests build a `ScopeConfig`
 * directly and pass it to `App`. */
function scopeConfigFromEnv(): ScopeConfig {
  const config: ScopeConfig = {};
  const fixture = process.env["SKILL_STUDIO_FIXTURE"];
  const home = process.env["SKILL_STUDIO_HOME"];
  if (fixture !== undefined) config.fixture = fixture;
  if (home !== undefined) config.home = home;
  return config;
}

async function main(): Promise<void> {
  const renderer = await createCliRenderer({});
  const root = createRoot(renderer);
  root.render(
    <App
      scopeConfig={scopeConfigFromEnv()}
      onQuit={() => {
        root.unmount();
        renderer.stop();
        process.exit(0);
      }}
    />,
  );
}

main().catch((cause: unknown) => {
  process.stderr.write(
    `skill-studio-tui failed to start: ${cause instanceof Error ? cause.message : String(cause)}\n`,
  );
  process.exit(1);
});
