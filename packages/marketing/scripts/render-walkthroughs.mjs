import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
const output = fileURLToPath(new URL("../public/walkthrough/current/", import.meta.url));
const entry = "src/remotion/index.ts";
const features = ["Map", "Repair", "Install", "Activity"];
const themes = ["Dark", "Light"];
const formats = ["Desktop", "Mobile"];

mkdirSync(output, { recursive: true });

function run(args) {
  execFileSync("npx", ["remotion", ...args], {
    cwd: packageRoot,
    stdio: ["ignore", "ignore", "pipe"],
  });
}

for (const feature of features) {
  for (const theme of themes) {
    for (const format of formats) {
      const composition = `${feature}${theme}${format}`;
      const basename = `${feature.toLowerCase()}-${theme.toLowerCase()}-${format.toLowerCase()}`;
      run([
        "render",
        entry,
        composition,
        `${output}${basename}.mp4`,
        "--codec=h264",
        "--crf=22",
        "--muted",
        "--log=error",
        "--overwrite",
      ]);
      run([
        "still",
        entry,
        composition,
        `${output}${basename}.jpg`,
        "--frame=0",
        "--image-format=jpeg",
        "--jpeg-quality=90",
        "--log=error",
        "--overwrite",
      ]);
      process.stdout.write(`${basename}\n`);
    }
  }
}
