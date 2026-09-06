import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const output = fileURLToPath(new URL("../capture/public/remotion/current/", import.meta.url));
const capturePage =
  process.env.SKILL_STUDIO_CAPTURE_URL ??
  new URL(
    `/@fs${fileURLToPath(new URL("../capture/index.html", import.meta.url))}`,
    "https://skill-studio.localhost",
  ).href;
const coordinates = {};
mkdirSync(output, { recursive: true });

function browser(...args) {
  return execFileSync("agent-browser", ["--session", "current-skill-studio-capture", ...args], {
    encoding: "utf8",
  });
}

function evaluate(source) {
  return browser("eval", source);
}

function clickExact(text, scope = "document") {
  evaluate(`(() => {
    const button = [...${scope}.querySelectorAll("button")].find((item) => item.textContent?.trim() === ${JSON.stringify(text)});
    if (!button) throw new Error(${JSON.stringify(`Missing button: ${text}`)});
    button.click();
  })()`);
}

function open(scene, theme, width = 1040) {
  browser("open", `${capturePage}?scene=${scene}&theme=${theme}`);
  browser("set", "viewport", String(width), "1000");
  browser("wait", "--fn", "window.__captureReady === true");
}

function settle() {
  evaluate(
    "(async () => { await document.fonts.ready; await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))); })()",
  );
}

function capture(name) {
  settle();
  browser("mouse", "move", "1", "1");
  browser("screenshot", `${output}${name}.png`);
  coordinates[name] = JSON.parse(
    evaluate(`([...document.querySelectorAll('button, input, [role="switch"], [role="dialog"], h1, h2, h3, [aria-label]')]
      .map((element) => {
        const rect = element.getBoundingClientRect();
        return {
          text: (element.getAttribute("aria-label") || element.textContent || "").trim().replace(/\\s+/g, " ").slice(0, 120),
          x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height)
        };
      })
      .filter((rect) => rect.width > 0 && rect.height > 0))`),
  );
  process.stdout.write(`Captured ${name}.png\n`);
}

function captureMap(theme, width, suffix = "") {
  open("map", theme, width);
  clickExact("Global folder");
  capture(`map-${theme}-global${suffix}`);
  clickExact("Project folder");
  capture(`map-${theme}-expanded${suffix}`);
  clickExact("Compare copies");
  browser(
    "wait",
    "--fn",
    'document.querySelector("diffs-container")?.shadowRoot?.textContent?.includes("Run the scoped checks") === true',
  );
  capture(`map-${theme}-compare${suffix}`);
}

function captureRepair(theme, width, suffix = "") {
  open("repair", theme, width);
  browser("wait", "--text", "Repair this location");
  capture(`repair-${theme}-broken${suffix}`);
  clickExact("Fix it");
  browser(
    "wait",
    "--fn",
    'document.querySelector("main")?.textContent?.includes("Summarize the shipped behavior") === true',
  );
  capture(`repair-${theme}-resolved${suffix}`);
}

function captureInstall(theme, width, suffix = "") {
  open("install", theme, width);
  browser("wait", "--text", "frontend-design");
  clickExact("Project", "document.querySelector('[role=\"dialog\"]')");
  capture(`install-${theme}-project${suffix}`);
  evaluate(`document.querySelector('[role="dialog"] [data-slot="checkbox"]').click()`);
  browser(
    "wait",
    "--fn",
    `document.querySelector('[role="dialog"] [data-slot="checkbox"]')?.getAttribute("aria-checked") === "true"`,
  );
  capture(`install-${theme}-trial${suffix}`);
  clickExact("Add skill", "document.querySelector('[role=\"dialog\"]')");
  browser("wait", "--text", "frontend-design");
  capture(`install-${theme}-installed${suffix}`);
}

function captureActivity(theme, width, suffix = "") {
  open("activity", theme, width);
  capture(`activity-${theme}-30d${suffix}`);
  clickExact("7d");
  capture(`activity-${theme}-7d${suffix}`);
}

for (const theme of ["dark", "light"]) {
  for (const [width, suffix] of [
    [1040, ""],
    [800, "-mobile"],
  ]) {
    captureMap(theme, width, suffix);
    captureRepair(theme, width, suffix);
    captureInstall(theme, width, suffix);
    captureActivity(theme, width, suffix);
  }
}

writeFileSync("/tmp/current-shot-coordinates.json", JSON.stringify(coordinates, null, 2));
browser("close");
