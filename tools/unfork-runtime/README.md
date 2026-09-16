# Packaged Unfork providers

The macOS Tauri build hook materializes the pinned Node and provider packages before
building the frontend. The macOS resource map includes the result as
`unfork-runtime` in the application's resource directory. Ordinary frontend and core
checks do not run the materializer. Generated resources are ignored by Git.

The verified target is native macOS arm64. Other macOS targets fail explicitly;
they need their own Node archive pin and runtime acceptance before support is added.
Non-macOS builds skip this runtime and retain their existing provider route.

From the repository root:

```sh
node tools/unfork-runtime/materialize.mjs
node tools/unfork-runtime/materialize.mjs --verify
node tools/unfork-runtime/materialize.mjs --verify '/path/to/Skill Studio.app/Contents/Resources/unfork-runtime'
```

To reuse a downloaded archive, set `SKILL_STUDIO_NODE_ARCHIVE` to its absolute path.
The same fixed SHA-256 check applies. Otherwise the script downloads the pinned
archive from nodejs.org. It uses that archive's npm, an empty isolated configuration,
a temporary cache, a 512 MiB Node heap, and a two-minute subprocess deadline.
Package installation disables lifecycle scripts and executable links. The committed
lockfile supplies package integrity hashes. Temporary extraction and npm files are
removed on success or failure.

Before replacing generated resources, verification compares the Node executable
digest and the provider tree's bytes, names and permissions with the committed
record. The tree encoding matches the core's `skill-studio-tree-v1` identity.
Links and special files are rejected. The desktop verifies the runtime again before
provider execution. A valid generated runtime is reused without network access.

The record covers Dotagents 3.0.1 and skills 1.5.25 in one dependency tree. Updating
these versions requires new provider-contract acceptance and a new verified record;
do not regenerate the record automatically during builds.

Signing remains a release acceptance requirement. The downloaded Node binary is
signed by the Node.js Foundation. Re-signing that executable changes its digest and
requires an explicit release attestation procedure. Verify the final bundle after
signing; never disable runtime verification to make a signed build pass.
