// ============================================================================
// Skill Studio - marketing capture
// Boots the desktop app against the shared harness fixture/mock, then drives
// it into one of a fixed set of scenes for the marketing site's screenshots
// and Remotion walkthroughs. See `apps/desktop/src/dev/harness/` for the
// fixture and mock IPC layer this now shares with the browser dev harness.
// ============================================================================

import { createRoot } from "react-dom/client";
import type { SkillSnapshot } from "../../lib/src/index.ts";
import {
  buildHarnessSnapshot,
  HARNESS_PROJECT,
} from "../../../apps/desktop/src/dev/harness/skill-fixture.ts";
import { installMockTauri } from "../../../apps/desktop/src/dev/harness/mock-tauri.ts";

type Scene = "map" | "coverage" | "invoke" | "drift" | "install" | "activity" | "repair";

const SCENES: readonly Scene[] = [
  "map",
  "coverage",
  "invoke",
  "drift",
  "install",
  "activity",
  "repair",
];

interface CaptureControl {
  setScene: (scene: Scene) => void;
  snapshot: () => SkillSnapshot;
}

declare global {
  interface Window {
    __captureReady?: boolean;
    __capture?: CaptureControl;
  }
}

const control = installMockTauri(buildHarnessSnapshot());

const params = new URLSearchParams(location.search);
const sceneParam = params.get("scene");
const initialScene = SCENES.find((scene) => scene === sceneParam) ?? "map";
const theme = params.get("theme") === "light" ? "light" : "dark";
localStorage.setItem("theme", theme);
localStorage.setItem("project-paths", HARNESS_PROJECT);
document.documentElement.setAttribute("data-theme", theme);

const [{ default: App }, { useAppStore }] = await Promise.all([
  import("../../../apps/desktop/src/App.tsx"),
  import("../../../apps/desktop/src/store/appStore.ts"),
]);

function setScene(scene: Scene): void {
  const common = {
    userAddedProjects: [HARNESS_PROJECT],
    excludedProjects: [],
    addSkillSheet: { open: false },
  };
  if (scene === "coverage") {
    useAppStore.setState({
      ...common,
      activeView: { kind: "skills" },
      showCoverage: true,
      skillListFilter: { ...useAppStore.getState().skillListFilter, query: "", scope: "all" },
    });
  } else if (scene === "activity") {
    useAppStore.setState({ ...common, activeView: { kind: "activity" }, usageWindow: "30d" });
  } else if (scene === "install") {
    useAppStore.setState({
      ...common,
      activeView: { kind: "home" },
      addSkillSheet: { open: true, prefill: "anthropics/skills/tree/main/frontend-design" },
    });
  } else if (scene === "repair") {
    useAppStore.setState({
      ...common,
      activeView: {
        kind: "skill",
        name: "release-notes",
        deploymentPath: `${HARNESS_PROJECT}/.claude/skills/release-notes`,
        from: { kind: "home" },
      },
    });
  } else {
    useAppStore.setState({
      ...common,
      activeView: {
        kind: "skill",
        name: "commit",
        from: { kind: "skills" },
        intent: scene === "drift" ? "compare" : undefined,
      },
    });
  }
}

setScene(initialScene);
window.__capture = { setScene, snapshot: control.snapshot };
createRoot(document.getElementById("root")!).render(<App />);
requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    window.__captureReady = true;
  }),
);
