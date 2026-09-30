// ============================================================================
// Skill Studio - Site theme
// Reads, applies and remembers the light/dark theme on <html> for every page
// ============================================================================

import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "./PaletteThemes.stylex";
import type { ThemeToggleOrigin } from "./ProductMock";
import { lightSiteTheme, type SiteTheme } from "./SiteTheme.stylex";

const STORAGE_KEY = "skill-studio-site-theme";
const LIGHT_QUERY = "(prefers-color-scheme: light)";

// The site tokens default to the dark values on :root. The light theme points them at the
// violet palette's light values, and custom properties resolve where they are declared, so
// the palette has to sit on <html> together with the light theme.
const LIGHT_THEME_CLASSES = (
  stylex.props(getPaletteTheme("violet"), lightSiteTheme).className ?? ""
)
  .split(" ")
  .filter(Boolean);

/**
 * The inline <head> script that sets the theme before first paint. Prerendered pages carry no
 * theme in their markup, so without it a light-mode visitor sees the dark page first.
 */
export function themeBootScript(): string {
  return `(()=>{let t;try{t=localStorage.getItem(${JSON.stringify(STORAGE_KEY)})}catch{}if(t!=="light"&&t!=="dark")t=matchMedia(${JSON.stringify(LIGHT_QUERY)}).matches?"light":"dark";const r=document.documentElement;r.dataset.siteTheme=t;if(t==="light")r.classList.add(...${JSON.stringify(LIGHT_THEME_CLASSES)})})()`;
}

function parseTheme(value: string | null | undefined): SiteTheme | null {
  return value === "light" || value === "dark" ? value : null;
}

function systemSiteTheme(): SiteTheme {
  return window.matchMedia(LIGHT_QUERY).matches ? "light" : "dark";
}

export function currentSiteTheme(): SiteTheme {
  return parseTheme(document.documentElement.dataset.siteTheme) ?? systemSiteTheme();
}

export function rememberedSiteTheme(): SiteTheme | null {
  try {
    return parseTheme(localStorage.getItem(STORAGE_KEY));
  } catch {
    return null;
  }
}

export function rememberSiteTheme(theme: SiteTheme) {
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Private windows and blocked storage still switch the theme; it just is not kept.
  }
}

export function applySiteTheme(theme: SiteTheme) {
  const root = document.documentElement;
  root.dataset.siteTheme = theme;
  for (const className of LIGHT_THEME_CLASSES) {
    root.classList.toggle(className, theme === "light");
  }
}

export function onSystemThemeChange(listener: (theme: SiteTheme) => void) {
  const preference = window.matchMedia(LIGHT_QUERY);
  const handleChange = () => listener(preference.matches ? "light" : "dark");
  preference.addEventListener("change", handleChange);
  return () => preference.removeEventListener("change", handleChange);
}

/**
 * Runs `commit` inside a view transition that grows a circle out of the toggle button.
 * A React caller wraps its state update in `flushSync`, so the new frame is ready in time.
 */
export function transitionSiteTheme(origin: ThemeToggleOrigin, commit: () => void) {
  const shouldReduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (shouldReduceMotion || !("startViewTransition" in document)) {
    commit();
    return;
  }

  const transition = document.startViewTransition(commit);
  void transition.ready.then(() => {
    const radius = Math.hypot(
      Math.max(origin.x, window.innerWidth - origin.x),
      Math.max(origin.y, window.innerHeight - origin.y),
    );
    document.documentElement.animate(
      {
        clipPath: [
          `circle(0px at ${origin.x}px ${origin.y}px)`,
          `circle(${radius}px at ${origin.x}px ${origin.y}px)`,
        ],
      },
      {
        // A near-linear start keeps the first frames inside the icon, so the reveal reads as
        // growing out of the button. An expo-out curve reaches ~90% radius in 150ms and then
        // crawls, which reads as a mid-way freeze.
        duration: 560,
        easing: "cubic-bezier(0.4, 0, 0.2, 1)",
        pseudoElement: "::view-transition-new(root)",
      },
    );
  });
}
