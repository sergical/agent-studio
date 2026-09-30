// Runs before `npm run deploy`. The homepage and docs tell visitors to run
// `npx skill-studio ...`, which fails until a release with a CLI binary is on npm.
// A placeholder `skill-studio@0.0.0` with no `bin` is already published, so a plain
// "does the package exist" check is not enough.
import { execFileSync } from "node:child_process";

const PACKAGE = "skill-studio";

let view;
try {
  view = JSON.parse(execFileSync("npm", ["view", PACKAGE, "--json"], { encoding: "utf8" }));
} catch (error) {
  process.stderr.write(`Could not read ${PACKAGE} from npm: ${error.message}\n`);
  process.exit(1);
}

if (!view.bin?.[PACKAGE]) {
  process.stderr.write(
    `${PACKAGE}@${view.version} on npm has no "${PACKAGE}" binary, so the ` +
      `"npx ${PACKAGE}" commands on the site would fail. Publish the CLI release first.\n`,
  );
  process.exit(1);
}

process.stdout.write(`${PACKAGE}@${view.version} on npm has the CLI binary.\n`);
