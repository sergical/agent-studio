# Release preparation and rollback

Status on September 17, 2026: preparation only. No final combined candidate,
signed installer, deployed API/site, production telemetry receipt, or exercised
rollback is established by this document. Merge and deployment need explicit
authorization after the candidate and evidence are ready to review.

## Observed release dependencies

| Surface               | Observed configuration                                                                                                                                                                          | Required before release                                                                                                                                          |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop               | Tauri identifies `com.skillstudio.app`, version `0.1.0`, bundles all targets, and embeds `apps/desktop/dist`. Signing identity and updater settings are absent from the inspected Tauri config. | Select supported macOS architectures and distribution channel; configure signing/notarization and matching symbols; verify the packaged candidate.               |
| Desktop API access    | `api.rs` defaults to `http://127.0.0.1:8787`; registry `server_url` can override it.                                                                                                            | Define how a clean installation obtains the production endpoint. An installed desktop app must not depend on the developer's local server.                       |
| Hono API              | PR #89 builds a Node service, requires `SKILLS_SH_API_KEY`, and binds to `127.0.0.1`, using `PORT` or 8787.                                                                                     | Select host, public origin, proxy/TLS configuration, supervisor, and secret injection. A Workers deployment is not implemented by this Node build.               |
| Marketing             | PR #90 builds static Vite assets, with private source maps in a separate output directory.                                                                                                      | Select host/domain and deployment mechanism; verify API and download links against the approved release.                                                         |
| Monitoring            | Four real projects exist; see [project mapping](sentry-project-mapping.md).                                                                                                                     | Bind the exact release, verify received and redacted signals, upload matching maps/symbols, and verify source locations and alert delivery.                      |
| Repository automation | Release and remote tag queries returned no entries; the repository Actions-secret query returned no entries.                                                                                    | Establish release automation and credential sources. These queries do not establish whether organization/environment secrets or local signing credentials exist. |

The observations come from main `c489f78`, the unmerged integration worktree,
API PR #89 (`1989004`), and marketing PR #90 (`88f369a`). Both surface CI
checks and their security checks pass at these revisions. The desktop review
chain now reaches PR #180 (`bcdb864`), whose core, frontend and security checks
pass. These separate checks do not establish an integrated release. Assemble the candidate only from exact reviewed commit heads. The dirty
integration worktree is not a release source. Any required remaining change must
first receive its own reviewed commit. Preserve excluded CLI/MCP source separately
with a path/hash manifest; do not include it by copying the integration tree.

Rust telemetry PR #92 (`dda0eac`) has passing telemetry CI but a failed
GitGuardian check. A synthetic authenticated DSN uses `example.invalid` in its
transport rejection test. The finding still needs explicit resolution; a fixture
explanation alone does not turn the security check green.

### Hosting and monitoring dependencies

A September 17 read-only inventory of the connected personal Cloudflare account
SERG.TECH returned 16 Workers and two Pages projects. No names or IDs matched
`skill|agent.studio`. This does not rule out a deployment under another name or
on another host. The available Vercel connection is a work account; no personal
Vercel connection was available. No deployment or routing was changed.

Personal Sentry access confirms the four `sergtech` projects in the mapping.
The sampled receipt queries found API verification telemetry, but did not
establish production receipt for any surface or desktop/marketing receipt.
Release acceptance still requires exact build identities, received signals,
redaction and matching source maps or debug symbols. Do not label verification
events as production evidence.

The unresolved release inputs are a confirmed API origin/host, marketing
host/domain, desktop distribution targets and signing credentials, plus an
approved deployment candidate. Record these inputs before requesting deployment
approval; do not substitute the development localhost endpoint.

## Candidate and artifacts

1. Select the reviewed PR set and resolve conflicts in a clean integration
   checkout. Preserve excluded CLI/MCP/cloud work separately. Exclude the
   unverified retained-directory performance experiment.
2. Record the source commit, clean-tree status, lockfile hashes, toolchain versions,
   enabled features, target architecture and non-secret build configuration.
   Retain the desktop `native-fork-repair` default feature and packaged runtime
   wiring already present in PR #180. Verify that wiring in the exact combined
   artifact; previous packaged acceptance does not cover new signing or architectures.
3. Run the final integration checks and agreed native lifecycle cases on this
   candidate. Use the isolated test application and fixture for native tests.
   The current dirty worktree and separate PR checks are insufficient evidence.
4. Build each surface from that identity and retain the artifacts below. Record
   file hashes after signing and packaging, not only before them.

| Artifact                                                 | Required evidence                                                                                                                                                      |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop app and installer for each selected architecture | Exact version/revision, bundle ID, feature set, signature/notarization result, file hashes, native acceptance and packaged resources.                                  |
| Desktop JavaScript maps and Rust debug symbols           | Match the shipped binary/assets; received errors resolve to the correct source. Keep private maps out of the public download.                                          |
| API Node bundle and production dependencies              | Matching runtime dependencies, instrumentation preload, release/environment values, health and upstream smoke checks, shutdown behavior under the selected supervisor. |
| Marketing assets and private maps                        | Matching HTML/assets/release, valid download destinations, normal navigation, expected telemetry configuration, maps uploaded privately.                               |

The integration telemetry code supports matching desktop release labels using
the package version plus `SKILL_STUDIO_DESKTOP_BUILD_REVISION`; React receives
the corresponding `VITE_DESKTOP_SENTRY_RELEASE`. Derive these from the same
clean build. API and marketing use their configured release variables. Preserve
the build identity even if an artifact is promoted between environments.

## Signing and publication sequence

For direct macOS distribution, provision a Developer ID Application identity and
notarization credentials. Tauri accepts `APPLE_SIGNING_IDENTITY` or its macOS
signing configuration; follow its [official signing guide](https://v2.tauri.app/distribute/sign/macos/).
Local test-app ad-hoc signatures are not distribution acceptance.

Prepare a review packet with artifact hashes, checks, known limits, credential
source names, rollout target, monitoring evidence and rollback target. Obtain
explicit deployment authorization before publishing or changing live routing.

After authorization, deploy the API candidate behind its approved ingress and
verify health, a real authorized upstream request, and received telemetry. Deploy
the matching marketing assets and verify download links. Publish only the signed,
verified desktop artifacts. Check a clean installation against the production
endpoint. Record the actual deployment/release IDs and first health observations.

Before any remote test export, configure a designated test destination and
bounded retention. Normal suites keep export disabled or use fixture transports.
Record event IDs and concise redaction/symbolication results rather than archives
of raw telemetry. Production receipt remains a separate acceptance step.

## Rollback procedure to rehearse before publication

Retain the previous working artifact, configuration and deployment IDs. Define
the observed failure that triggers rollback and the person who authorizes it.
No previous GitHub release was found, so a first-release fallback still needs to
be selected and tested; do not invent a known-good version.

- **API:** restore the previous bundle, dependencies and configuration together,
  then restore routing and verify health/upstream access. Preserve diagnostic
  event IDs and verify the old client remains compatible with the restored API.
- **Marketing:** restore the previous HTML and asset set together. Keep referenced
  assets available to existing clients and verify restored download destinations.
- **Desktop:** stop further distribution first. Before a binary downgrade,
  verify event-store/schema and filesystem-state compatibility in an isolated
  fixture. Preserve current skill files and recovery state. Restoring an old
  database alone can desynchronize it from skill files; it is not a rollback.
  If downgrade compatibility is unproven, prepare a reviewed forward fix.
- **Telemetry configuration:** restore the recorded configuration with the
  artifact it belongs to. Preserve release and diagnostic IDs for investigation.

The rollback rehearsal is complete only after the restored candidate passes
the affected user workflow and its state is consistent. No rehearsal was run
during this read-only release audit.

## Audit evidence

Read-only commands: `gh release list --limit 5`, repository tags API, repository
`gh secret list` (names only), and source/config inspection. The September 17
refresh also inspected PR head/check metadata and the personal
Cloudflare Workers/Pages inventory through Executor. No secret values or signing
keys were read, and no build jobs, servers, deployments, or test captures were
started by this documentation audit. Resource peaks were not measured because
this was a metadata and documentation task. Hosting-specific commands must be added after the targets
are selected; this packet is not an executable deployment script.
