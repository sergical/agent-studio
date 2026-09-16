import { createHash } from "node:crypto";
import * as fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const inputs = path.dirname(fileURLToPath(import.meta.url));
const output = path.resolve(inputs, "../../apps/desktop/src-tauri/resources/unfork-runtime");
const record = JSON.parse(fs.readFileSync(path.join(inputs, "verified-record.json"), "utf8"));
const archiveName = "node-v24.19.0-darwin-arm64.tar.gz";
const archiveDigest = "8294b7aa9b03997481c06babf1e8b270c859358f27da57a11509afe537ac381d";

function numberBytes(value, size) {
  const buffer = Buffer.alloc(size);
  if (size === 8) buffer.writeBigUInt64LE(BigInt(value));
  else buffer.writeUInt32LE(value);
  return buffer;
}

function fileDigest(filename, prefix = []) {
  const hash = createHash("sha256");
  for (const value of prefix) hash.update(value);
  const file = fs.openSync(filename, "r");
  try {
    const buffer = Buffer.alloc(65536);
    let count;
    while ((count = fs.readSync(file, buffer)) > 0) hash.update(buffer.subarray(0, count));
  } finally {
    fs.closeSync(file);
  }
  return hash.digest("hex");
}

function treeIdentity(root) {
  let entries = 0;
  let bytes = 0;
  function visit(filename, depth) {
    if (++entries > 20000 || depth > 64) throw new Error("Runtime tree exceeds its limits");
    const metadata = fs.lstatSync(filename);
    const hash = createHash("sha256").update("skill-studio-tree-v1\0");
    if (metadata.isDirectory()) {
      hash.update("D").update(numberBytes(metadata.mode & 0o7777, 4));
      const names = fs.readdirSync(filename, { encoding: "buffer" }).sort(Buffer.compare);
      for (const name of names) {
        const child = Buffer.concat([Buffer.from(filename), Buffer.from("/"), name]);
        hash
          .update(numberBytes(name.length, 8))
          .update(name)
          .update(visit(child, depth + 1));
      }
    } else if (metadata.isFile()) {
      bytes += metadata.size;
      if (bytes > 256 * 1024 * 1024) throw new Error("Runtime bytes exceed their limit");
      const digest = fileDigest(filename, [Buffer.from("F"), numberBytes(metadata.size, 8)]);
      hash
        .update("F")
        .update(numberBytes(metadata.mode & 0o7777, 4))
        .update(digest);
    } else {
      throw new Error("Runtime must contain only regular files and directories");
    }
    return `tree-v1:${hash.digest("hex")}`;
  }
  return visit(root, 0);
}

function verify(root) {
  const node = path.join(root, "bin/node");
  const metadata = fs.lstatSync(node);
  if (!metadata.isFile() || !(metadata.mode & 0o111))
    throw new Error("Invalid runtime Node executable");
  if (`sha256:${fileDigest(node)}` !== record.node_content_digest)
    throw new Error("Node digest mismatch");
  if (treeIdentity(path.join(root, "node_modules")) !== record.provider_tree_identity) {
    throw new Error("Provider tree digest mismatch");
  }
  const saved = JSON.parse(fs.readFileSync(path.join(root, "verified-record.json"), "utf8"));
  if (JSON.stringify(saved) !== JSON.stringify(record)) throw new Error("Runtime record mismatch");
}

function run(command, args, cwd, env) {
  const result = spawnSync(command, args, {
    cwd,
    env,
    encoding: "utf8",
    timeout: 120000,
    maxBuffer: 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    throw new Error(`${path.basename(command)} failed: ${result.error?.message ?? result.stderr}`);
  }
}

const platform = process.env.TAURI_ENV_PLATFORM ?? process.platform;
const arch = process.env.TAURI_ENV_ARCH ?? process.arch;
if (!["macos", "darwin"].includes(platform)) {
  process.stdout.write("Unfork runtime packaging is macOS-only.\n");
} else {
  if (
    !["aarch64", "arm64"].includes(arch) ||
    process.platform !== "darwin" ||
    process.arch !== "arm64"
  ) {
    throw new Error("Unfork runtime is verified only for native macOS arm64 builds");
  }
  if (process.argv.includes("--verify")) {
    verify(process.argv[process.argv.indexOf("--verify") + 1] ?? output);
    process.stdout.write("Packaged Unfork runtime verified.\n");
  } else {
    let reusable = false;
    try {
      verify(output);
      reusable = true;
    } catch {
      /* Rebuild incomplete or stale generated resources. */
    }
    if (reusable) {
      process.stdout.write("Reusing verified Unfork runtime.\n");
    } else {
      fs.mkdirSync(path.dirname(output), { recursive: true });
      const work = fs.mkdtempSync(path.join(os.tmpdir(), "skill-studio-runtime-"));
      const stage = fs.mkdtempSync(path.join(path.dirname(output), ".unfork-runtime-"));
      try {
        const env = {
          PATH: "/usr/bin:/bin",
          HOME: path.join(work, "home"),
          TMPDIR: work,
          LANG: "en_US.UTF-8",
          NODE_OPTIONS: "--max-old-space-size=512",
        };
        fs.mkdirSync(env.HOME);
        const archive = process.env.SKILL_STUDIO_NODE_ARCHIVE ?? path.join(work, archiveName);
        if (!process.env.SKILL_STUDIO_NODE_ARCHIVE) {
          run(
            "/usr/bin/curl",
            [
              "--fail",
              "--location",
              "--proto",
              "=https",
              "--max-time",
              "90",
              "--max-filesize",
              "60000000",
              "--output",
              archive,
              `https://nodejs.org/dist/v24.19.0/${archiveName}`,
            ],
            work,
            env,
          );
        }
        if (fileDigest(archive) !== archiveDigest) throw new Error("Node archive digest mismatch");
        run("/usr/bin/tar", ["-xzf", archive, "-C", work], work, env);
        const nodeRoot = path.join(work, "node-v24.19.0-darwin-arm64");
        const node = path.join(nodeRoot, "bin/node");
        fs.mkdirSync(path.join(stage, "bin"));
        fs.copyFileSync(node, path.join(stage, "bin/node"));
        fs.chmodSync(path.join(stage, "bin/node"), 0o755);
        fs.copyFileSync(path.join(nodeRoot, "LICENSE"), path.join(stage, "NODE-LICENSE"));
        for (const name of ["package.json", "package-lock.json", "verified-record.json"]) {
          fs.copyFileSync(path.join(inputs, name), path.join(stage, name));
        }
        for (const name of ["user.npmrc", "global.npmrc"])
          fs.writeFileSync(path.join(work, name), "");
        Object.assign(env, {
          npm_config_userconfig: path.join(work, "user.npmrc"),
          npm_config_globalconfig: path.join(work, "global.npmrc"),
          npm_config_cache: path.join(work, "cache"),
          npm_config_registry: "https://registry.npmjs.org/",
          npm_config_update_notifier: "false",
        });
        run(
          node,
          [
            path.join(nodeRoot, "lib/node_modules/npm/bin/npm-cli.js"),
            "ci",
            "--ignore-scripts",
            "--bin-links=false",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
          ],
          stage,
          env,
        );
        verify(stage);
        // Keep the previous valid resources until the replacement has passed verification.
        const previous = `${output}.previous`;
        if (fs.existsSync(previous))
          throw new Error(`Resolve interrupted runtime replacement at ${previous}`);
        if (fs.existsSync(output)) fs.renameSync(output, previous);
        try {
          fs.renameSync(stage, output);
        } catch (error) {
          if (fs.existsSync(previous)) fs.renameSync(previous, output);
          throw error;
        }
        fs.rmSync(previous, { recursive: true, force: true });
        process.stdout.write("Materialized and verified Unfork runtime.\n");
      } finally {
        fs.rmSync(work, { recursive: true, force: true });
        fs.rmSync(stage, { recursive: true, force: true });
      }
    }
  }
}
