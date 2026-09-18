# Release readiness

This file lists what stands between the current checkout and a version people can download, run, and keep updated. Read on 2026-09-16 from the repo and from the Cloudflare account through the executor, read only.

## What exists today

| Piece                                    | State                                                                                                                                                                                                                                                                                                         | Evidence                                                                            |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| App version                              | 0.1.0 in all three places                                                                                                                                                                                                                                                                                     | apps/desktop/src-tauri/tauri.conf.json:5, apps/desktop/package.json:3, Cargo.toml:3 |
| Bundle targets                           | "all"                                                                                                                                                                                                                                                                                                         | tauri.conf.json:32–34                                                               |
| Over-the-air updates                     | none; tauri-plugin-updater is not a dependency and there is no updater block in the config                                                                                                                                                                                                                    | Cargo.toml, tauri.conf.json                                                         |
| macOS signing and notarization           | none configured                                                                                                                                                                                                                                                                                               | no signingIdentity, no notarization keys                                            |
| CI                                       | none; no .github/workflows folder                                                                                                                                                                                                                                                                             | repo root                                                                           |
| Changelog, license                       | none                                                                                                                                                                                                                                                                                                          | repo root                                                                           |
| Hosted backend                           | apps/server is Hono 4.13 on @hono/node-server, bound to 127.0.0.1:8787, reads SKILLS_SH_API_KEY from a repo-root .env                                                                                                                                                                                         | apps/server/src/server.ts:14–16, 207; apps/server/README.md:7–14                    |
| Cloudflare config in repo                | none; no wrangler.toml, no Workers entry                                                                                                                                                                                                                                                                      | grep for wrangler, workers, cloudflare                                              |
| Marketing site                           | Vite + React 19 + StyleX in packages/marketing, version 0.0.0; the download button points at GitHub releases; Remotion walkthroughs are wired in                                                                                                                                                              | packages/marketing/src/CommandCenter.tsx:32, 120                                    |
| Marketing claims                         | "Your skills. Your agents. One place." and "A desktop app for managing agent skills. Find installed copies, compare changes, and choose where to install." and "Get Skill Studio. Find your installed skills and check which agents can use them." Lists Claude Code, Codex, OpenCode, pi, Cursor, Grok Build | CommandCenter.tsx:15–22, 60–68, 124–127                                             |
| Pricing, waitlist, privacy policy, terms | none                                                                                                                                                                                                                                                                                                          | packages/marketing                                                                  |
| CLI                                      | 9 subcommands: scan, diagnose, capabilities, preview-repair, apply-repair, events, restore, schema, watch                                                                                                                                                                                                     | apps/cli/src                                                                        |
| MCP server                               | 7 tools: scan, diagnose, capabilities, preview_frontmatter_repair, apply_frontmatter_repair, list_events, restore_event                                                                                                                                                                                       | apps/mcp/src                                                                        |
| Desktop                                  | 71 commands; every install, remove, update, park, fork, enable, and run operation is desktop only                                                                                                                                                                                                             | docs/action-map/README.md                                                           |
| Telemetry, crash reports, logs           | none in the app; PR #92 in the Codex stack adds bounded Rust telemetry but is not merged                                                                                                                                                                                                                      | grep for sentry, posthog, tracing_subscriber                                        |
| First run                                | no first-run screen, no check for Node or npx or gh, no Full Disk Access prompt                                                                                                                                                                                                                               | grep for first_run, onboarding, which npx                                           |

## What the Cloudflare account has

Account SERG.TECH, read on 2026-09-16 through the Cloudflare MCP, GET requests only.

- Zone `useskillstudio.com` exists, active, free plan, created 2026-09-03, nameservers clark and margaret at Cloudflare. It has zero DNS records. Nothing serves it.
- No Worker, Pages project, KV namespace, R2 bucket, D1 database, or Worker domain is named for Skill Studio. The account holds 16 Workers, 2 Pages projects, 3 KV namespaces, 5 R2 buckets, and 3 D1 databases for other projects.
- Other zones on the account: 1234.sh, 416serg.me, hostk.it.com, repo-architect.com, serg.tech, ventihq.com, weight.coach.

## The path to a first public release

Ordered. Each step names the check that proves it is done.

1. **Sign and notarize the macOS build.** Add an Apple Developer signing identity and notarization credentials to the Tauri bundle config. Done when a fresh Mac opens the .dmg without a Gatekeeper warning.
2. **Add the updater.** Add tauri-plugin-updater, generate the signing key pair, put the public key and the endpoint in tauri.conf.json, and add a "Check for updates" control in Settings. Done when a 0.1.0 build sees a 0.1.1 manifest and installs it.
3. **Host the update manifest and the installers.** Use a Worker on `useskillstudio.com` that serves `latest.json` and redirects to the GitHub release asset, or an R2 bucket behind the same host. Done when `curl https://useskillstudio.com/updates/latest.json` returns the current version.
4. **Build in CI.** A GitHub Actions workflow with tauri-action that runs `npm run check`, builds, signs, notarizes, uploads the release, and writes the update manifest on a tag. Done when tagging `v0.1.1` produces a signed release with no manual step.
5. **Move the skills.sh proxy to a Worker.** apps/server is already Hono, so port the routes to a Worker on `api.useskillstudio.com`, keep the key in a Worker secret, add a rate limit per client, and point the desktop default at it. Done when the desktop store search works on a machine with no .env.
6. **Point the marketing site at the domain.** Deploy packages/marketing to Pages on `useskillstudio.com`, keep the download button on the latest release, and add a privacy policy and terms page. Done when the site loads on the domain and the download link resolves to a signed .dmg.
7. **First-run checks.** On first launch, check for Node and npx, ask for Full Disk Access if a harness folder is unreadable, and show what the app will and will not touch. Done when a clean macOS user account gets through install to the skill list without a blank screen.
8. **Crash reporting and a log file.** Write a rolling log under the app data folder, and send crash reports only with consent. Done when a forced panic shows up in the report sink.
9. **License and changelog.** Pick a license, add a CHANGELOG, and make the release workflow copy the changelog entry into the release notes.

## Parity with the CLI and the MCP server

The job the app does is "keep my skills in good shape". Today only the desktop can install, remove, update, park, fork, enable, and run. The CLI and the MCP server can only scan, diagnose, repair frontmatter, list events, and restore.

Desired state: every operation in docs/action-map is one function in the core crate, and the desktop, the CLI, and the MCP server are three thin adapters over it. Done when the CLI and MCP tool lists contain every write command named in the area files, and a parity test runs the same operation through all three adapters and compares the disk.

## Rollback rehearsal (unit 6.3)

At every release, before announcing it, run the n-1 over n rehearsal by
hand on a Mac that already ran the previous tag - the same checklist
`.github/workflows/release.yml`'s `rehearsal-summary` job prints to the
run's step summary:

- [ ] Install the previous tag's signed DMG and launch it at least once.
- [ ] Install this tag's signed DMG over it (same Applications path).
- [ ] Launch, confirm the app opens to the normal skill list, not the
      newer-data-folder message.
- [ ] Note the app data folder's `schema_version` before and after
      (`~/Library/Application Support/<bundle id>/schema_version`).
- [ ] Reinstall the previous tag's DMG over this one (the rollback),
      launch, and record what the user sees.

Result: (fill in)

This is a manual rehearsal, run once per release by the release author, not
a CI gate - the workflow only prints the checklist, never blocks on it.

## Gaps

- No updater plugin, no signing, no notarization, no CI, no release workflow.
- The backend runs only on localhost and reads its key from a .env file.
- `useskillstudio.com` is registered and idle: no DNS records, no Worker, no Pages project.
- The marketing site has no privacy policy, no terms, and its download link points at releases that do not exist yet.
- The CLI and MCP server expose 9 and 7 read-side operations against 71 desktop commands; no write parity.
- No telemetry, no crash reports, no log file, no first-run checks.
