// ============================================================================
// Skill Studio - Docs client
// The docs and 404 pages are static HTML. This script adds the copy buttons
// and the theme toggle with delegated listeners, and ships no React.
// ============================================================================

import type { SiteTheme } from "../SiteTheme.stylex";
import {
  applySiteTheme,
  currentSiteTheme,
  onSystemThemeChange,
  rememberSiteTheme,
  rememberedSiteTheme,
  transitionSiteTheme,
} from "../site-theme";
import "../marketing-site.css";
import "./docs-prose.css";
// Imported for its StyleX rules, which the build collects into the shared CSS file.
import "./docs-page-styles";

const COPY_LABEL_RESET_MS = 2000;
const copyResetTimers = new WeakMap<HTMLElement, number>();

function showCopyLabel(button: HTMLElement, text: string, state?: "copied") {
  const label = button.querySelector<HTMLElement>("[data-copy-label]");
  if (label) label.textContent = text;
  if (state) button.dataset.state = state;
  window.clearTimeout(copyResetTimers.get(button));
  copyResetTimers.set(
    button,
    window.setTimeout(() => {
      delete button.dataset.state;
      if (label) label.textContent = "Copy";
    }, COPY_LABEL_RESET_MS),
  );
}

async function copyCode(button: HTMLElement) {
  const code = button.parentElement?.querySelector("code");
  if (!code) return;
  try {
    await navigator.clipboard.writeText(code.textContent ?? "");
    showCopyLabel(button, "Copied", "copied");
  } catch {
    // Clipboard access can be blocked. Selecting the code still lets the reader copy it.
    window.getSelection()?.selectAllChildren(code);
    showCopyLabel(button, "Press Cmd+C");
  }
}

let stopFollowingSystem: (() => void) | undefined =
  rememberedSiteTheme() === null ? onSystemThemeChange(applySiteTheme) : undefined;

function toggleTheme(button: HTMLElement) {
  const nextTheme: SiteTheme = currentSiteTheme() === "dark" ? "light" : "dark";
  const bounds = button.getBoundingClientRect();
  stopFollowingSystem?.();
  stopFollowingSystem = undefined;
  rememberSiteTheme(nextTheme);
  transitionSiteTheme(
    { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 },
    () => applySiteTheme(nextTheme),
  );
}

document.addEventListener("click", (event) => {
  if (!(event.target instanceof Element)) return;
  const copyButton = event.target.closest<HTMLElement>("[data-copy]");
  if (copyButton) {
    void copyCode(copyButton);
    return;
  }
  const themeButton = event.target.closest<HTMLElement>("[data-theme-toggle]");
  if (themeButton) toggleTheme(themeButton);
});
