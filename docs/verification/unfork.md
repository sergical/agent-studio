# Restore a Fork to provider management

Review base: `4353bf7` (Fork Pull). Both Dotagents and skills.sh use staged provider
execution followed by the shared core's durable publication and recovery. The
supported desktop target is one mutable Global Universal Fork. This action discards
local Fork edits after confirmation. It does not offer History Undo.

## Product acceptance

| Case | Evidence |
| --- | --- |
| Cancel confirmation | Native dialog No preserved local files and registry; zero new events. |
| Restore Dotagents | Native ownership changed from Fork to dotagents; exact upstream files replaced local additions; sibling preserved. |
| Restore skills.sh | Native restoration produced exact upstream files and selected lock row; unrelated registry/lock fields preserved. |
| Activity | Both operations show provider-specific labels and Reveal only. The corrected app changes pending to done without navigation. |
| Source unavailable | Native missing-path refusal preserved local files and Fork; failed event retained. A separate fetch failure displayed its source-fetch error and exit code. |
| Interrupted completion | SQLite trigger rejected the final done update after publication. On restart without the trigger, the same event completed from saved evidence. A source-fetch trap was not called. |
| Changed evidence | A seeded pending published event plus a new external file stayed interrupted on restart. The file survived. Home warned and View Activity opened the interrupted row. |
| Modern and legacy records | Core and real-provider adapter checks cover current and legacy records without relying on creation history. |
| Publication boundaries | Core lifecycle checks cover before tree exchange, after exchange, after selected lock replacement, and after registry replacement, including changed-evidence refusal. |
| Cancellation and deadlines | Dotagents real executor covers prelaunch cancellation, provider-phase cancellation, timeout, failure and retry. Skills.sh eight-case adapter check passes, including pre-fetch cancellation, post-marker cancellation, fetch deadline and missing ref. |
| Exact target | Frontend target checks cover canonical Global Universal selection and refusal of absent, ambiguous, read-only or project targets. Backend revalidates owner, document and live-tree identity. |
| Runtime | Normal macOS Tauri build includes the verified Node/provider resources. The final bundle passes Node and provider tree digest checks; bundled Node passes strict signature verification. |

Native execution used the production packaged providers with a local GitHub CLI
fixture and isolated HOME/CFFIXED_USER_HOME. The provider's production sandbox
blocked network and confined writes to stage/cache. An initial outer application
sandbox caused macOS to refuse nested sandbox creation; that run established only
failure preservation. Successful native runs did not retain an outer real-home or
network prohibition. These are controlled source fixtures, not live GitHub service
acceptance.

Exact binary hashes and event IDs are in the task's compact result files under
`/tmp/skill-studio-delivery/unfork`: `native-initial-results.json`,
`native-correction-results.json`, and `native-restart-results.json`. The durable
summary is this document; local fixtures are disposable after final review.

## Verification commands

Use the review checkout and the shared Cargo target. Cargo runs with two workers,
locked offline dependencies, and one test thread. The runtime fixture points to
`Skill Studio.app/Contents/Resources/unfork-runtime` in the generated bundle.

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline -j2 \
  native_skills_sh_unfork_restores_and_recovers_with_real_provider -- --ignored --test-threads=1
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline -j2 \
  desktop_native_unfork_executor_records_prelaunch_and_provider_failures -- --ignored --test-threads=1
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline -j2 \
  desktop_native_unfork_executor_recovers_interrupted_publication -- --ignored --test-threads=1
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --locked --offline -j2 -- -D warnings
```

Focused frontend checks use `skill-lifecycle-target.test.ts`,
`SkillHistorySection.test.ts`, and `error-message.test.ts`, followed by desktop
TypeScript and scoped lint/format. Full local suites were not run after each edit.
Fresh simplification removed one forwarding wrapper. Independent review is complete;
its legacy-record and surviving-child-process findings are corrected. The final
PR CI remains a separate delivery gate.

## Resource use and limits

The corrected normal debug app build took 50.16 seconds, maximum process RSS
1,095,155,712 bytes, with zero reported swaps. Runtime materialization took 5.14
seconds, maximum process RSS146,636,800 bytes. These are build measurements;
application and test process-tree peak memory were not measured. Providers have
bounded output, deadlines, cancellation and bounded filesystem traversal. Native
interaction timings were not measured.

The verified packaging target is native macOS arm64. Production signing and
notarization are unverified: the app is an ad-hoc debug build. Re-signing bundled
Node changes its digest and requires explicit release attestation. Other macOS
architectures fail the materializer until their runtime is admitted. Non-macOS
builds retain their existing route; this packet does not claim native acceptance
there.

Native restart acceptance uses an injected completion failure and a seeded changed
live tree. It does not claim process-kill testing at every instruction. Earlier
publication prefixes are covered by focused core lifecycle tests. Recovery never
reruns the provider. Changed evidence remains available for review rather than
being overwritten.

## Final review corrections

The skills.sh adapter derives its name from the resolved live target, including
legacy records without stored deployment IDs or paths. The regression passes an
actually empty legacy record. Prepared provider execution waits for its process
group to exit under the original deadline before returning success. Finite child
writers can finish; cancellation, output failure or timeout terminates the group.
This prevents publication while a child still changes the staged source.

Final strict desktop all-target Clippy passes (7.17 seconds); both Rust manifests
pass formatting checks. All eleven process tests pass (6.46 seconds). After the final correction, the
eight-case skills.sh adapter check passes (31.92 seconds), and Dotagents interrupted
publication recovery passes (14.56 seconds). Native UI evidence above predates
these two review corrections; their affected behavior is covered by these focused
regressions. Final combined native acceptance remains a separate release gate.

The runtime verifier also requires the exact Node license from the pinned archive.
A missing or altered NODE-LICENSE refuses verification; the valid copy passes.
The same verifier runs before generated-resource reuse and in --verify mode.
Both normal reuse and the existing packaged bundle pass this corrected check.
Temporary license-test runtime copies were removed after recording results.
