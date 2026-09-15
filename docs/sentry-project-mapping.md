# Skill Studio Sentry projects

Confirmed on September 15, 2026 through Executor and the personal
`sentry_mcp.org.localSentryMcp` connection. Organization and team: `sergtech`.
Region: `https://us.sentry.io`.

| Surface | Project slug | Project ID | DSN configuration |
| --- | --- | --- | --- |
| Desktop React | `skill-studio-desktop-react` | `4512090766901248` | `VITE_DESKTOP_SENTRY_DSN` |
| Desktop Rust | `skill-studio-desktop-rust` | `4512090767622144` | `SKILL_STUDIO_DESKTOP_SENTRY_DSN` |
| Hono API (Node) | `skill-studio-api` | `4512090768474112` | `SENTRY_DSN` |
| Marketing React | `skill-studio-marketing` | `4512090769391616` | `VITE_SENTRY_DSN` |

The configuration names describe the instrumentation in the
`codex/shared-core-design` integration worktree. That implementation still needs
its own reviewed PRs; this document does not add runtime instrumentation to main.

Each project was created with a DSN. Retrieve its client key from that project's
Sentry settings when configuring the release build or runtime. Do not substitute
a project from the work account or an unrelated existing application.

Separate projects keep JavaScript source maps, Rust debug files, and service
errors associated with their own runtime. Cross-runtime trace continuity still
needs an end-to-end check. Project separation alone does not prove correlation.

## Verified state

All four create calls succeeded. A subsequent `find_projects` query for
`skill-studio` returned exactly these four slugs with `hasMore: false`.
No application configuration, deployment, event upload, or artifact upload was
performed. No build, test suite, or local server was started for this check.
Peak process memory was not measured; no test artifacts were generated.

## Remaining acceptance

1. Extract and review the existing instrumentation in bounded PRs. Check each
   outgoing signal's redaction before enabling export.
2. Bind each build/runtime to the project above and an explicit environment.
   Verify release identity: Rust uses the package version plus an optional
   `SKILL_STUDIO_DESKTOP_BUILD_REVISION` build value; JavaScript release labels
   are configurable. Establish an unambiguous mapping
   between the delivered binary, assets, and uploaded artifacts.
3. Configure a designated test destination with bounded retention before a
   bounded remote telemetry check. Normal test suites must keep export disabled.
4. Upload matching private source maps and Rust debug files, then verify readable
   errors and source locations from the exact candidate.
5. Verify applicable errors, traces, logs, and metrics for all four surfaces,
   including correlation and redaction in received data. Record event IDs,
   candidate identity, and concise results rather than raw payload archives.
6. Verify production receipt after an explicitly authorized deployment. Confirm
   alert routing, release configuration, and rollback instructions.

Project creation closes the missing-destination setup step. It does not establish
production monitoring acceptance.
