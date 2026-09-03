import * as stylex from "@stylexjs/stylex";

export type PaletteId = "mono" | "cobalt" | "ember" | "violet";

export const paletteOptions = [
  { id: "mono", label: "Mono" },
  { id: "cobalt", label: "Cobalt" },
  { id: "ember", label: "Ember" },
  { id: "violet", label: "Violet" },
] satisfies ReadonlyArray<{ id: PaletteId; label: string }>;

export const paletteVars = stylex.defineVars({
  darkBackground: "oklch(0.11 0 0)",
  darkSurface: "oklch(0.15 0 0)",
  darkRaised: "oklch(0.19 0 0)",
  darkText: "oklch(0.97 0 0)",
  darkMuted: "oklch(0.72 0 0)",
  darkBorder: "oklch(0.24 0 0)",
  darkAccent: "oklch(0.97 0 0)",
  darkAccentHover: "oklch(0.88 0 0)",
  darkAccentSoft: "oklch(1 0 0 / 0.1)",
  darkAccentText: "oklch(0.11 0 0)",
  darkSecondary: "oklch(0.72 0 0)",
  lightBackground: "oklch(0.97 0 0)",
  lightSurface: "oklch(1 0 0)",
  lightSubtle: "oklch(0.94 0 0)",
  lightText: "oklch(0.16 0 0)",
  lightMuted: "oklch(0.45 0 0)",
  lightBorder: "oklch(0.16 0 0 / 0.14)",
  lightAccent: "oklch(0.16 0 0)",
  lightAccentHover: "oklch(0.25 0 0)",
  lightAccentSoft: "oklch(0.16 0 0 / 0.08)",
  lightAccentText: "oklch(0.98 0 0)",
  lightSecondary: "oklch(0.45 0 0)",
  logoFilter: "grayscale(1) contrast(1.08)",
});

const monoTheme = stylex.createTheme(paletteVars, {
  darkBackground: "oklch(0.11 0 0)",
  darkSurface: "oklch(0.15 0 0)",
  darkRaised: "oklch(0.19 0 0)",
  darkText: "oklch(0.97 0 0)",
  darkMuted: "oklch(0.72 0 0)",
  darkBorder: "oklch(0.24 0 0)",
  darkAccent: "oklch(0.97 0 0)",
  darkAccentHover: "oklch(0.88 0 0)",
  darkAccentSoft: "oklch(1 0 0 / 0.1)",
  darkAccentText: "oklch(0.11 0 0)",
  darkSecondary: "oklch(0.72 0 0)",
  lightBackground: "oklch(0.97 0 0)",
  lightSurface: "oklch(1 0 0)",
  lightSubtle: "oklch(0.94 0 0)",
  lightText: "oklch(0.16 0 0)",
  lightMuted: "oklch(0.45 0 0)",
  lightBorder: "oklch(0.16 0 0 / 0.14)",
  lightAccent: "oklch(0.16 0 0)",
  lightAccentHover: "oklch(0.25 0 0)",
  lightAccentSoft: "oklch(0.16 0 0 / 0.08)",
  lightAccentText: "oklch(0.98 0 0)",
  lightSecondary: "oklch(0.45 0 0)",
  logoFilter: "grayscale(1) contrast(1.08)",
});

const cobaltTheme = stylex.createTheme(paletteVars, {
  darkBackground: "oklch(0.14 0.025 255)",
  darkSurface: "oklch(0.18 0.03 255)",
  darkRaised: "oklch(0.22 0.035 255)",
  darkText: "oklch(0.97 0.008 255)",
  darkMuted: "oklch(0.73 0.025 255)",
  darkBorder: "oklch(0.27 0.035 255)",
  darkAccent: "oklch(0.68 0.19 255)",
  darkAccentHover: "oklch(0.74 0.16 255)",
  darkAccentSoft: "oklch(0.68 0.19 255 / 0.16)",
  darkAccentText: "oklch(0.99 0.004 255)",
  darkSecondary: "oklch(0.78 0.13 205)",
  lightBackground: "oklch(0.97 0.012 255)",
  lightSurface: "oklch(1 0 0)",
  lightSubtle: "oklch(0.94 0.018 255)",
  lightText: "oklch(0.18 0.025 255)",
  lightMuted: "oklch(0.46 0.035 255)",
  lightBorder: "oklch(0.34 0.07 255 / 0.18)",
  lightAccent: "oklch(0.55 0.22 255)",
  lightAccentHover: "oklch(0.48 0.21 255)",
  lightAccentSoft: "oklch(0.55 0.22 255 / 0.1)",
  lightAccentText: "oklch(0.99 0.004 255)",
  lightSecondary: "oklch(0.58 0.15 205)",
  logoFilter: "hue-rotate(-35deg) saturate(0.92)",
});

const emberTheme = stylex.createTheme(paletteVars, {
  darkBackground: "oklch(0.14 0.02 35)",
  darkSurface: "oklch(0.18 0.025 35)",
  darkRaised: "oklch(0.22 0.03 35)",
  darkText: "oklch(0.97 0.01 65)",
  darkMuted: "oklch(0.73 0.035 55)",
  darkBorder: "oklch(0.27 0.03 35)",
  darkAccent: "oklch(0.68 0.2 35)",
  darkAccentHover: "oklch(0.74 0.16 35)",
  darkAccentSoft: "oklch(0.68 0.2 35 / 0.16)",
  darkAccentText: "oklch(0.99 0.008 70)",
  darkSecondary: "oklch(0.82 0.15 82)",
  lightBackground: "oklch(0.975 0.012 60)",
  lightSurface: "oklch(1 0 0)",
  lightSubtle: "oklch(0.95 0.02 55)",
  lightText: "oklch(0.19 0.025 30)",
  lightMuted: "oklch(0.47 0.04 38)",
  lightBorder: "oklch(0.38 0.075 38 / 0.17)",
  lightAccent: "oklch(0.57 0.19 35)",
  lightAccentHover: "oklch(0.5 0.18 35)",
  lightAccentSoft: "oklch(0.57 0.19 35 / 0.1)",
  lightAccentText: "oklch(0.99 0.008 70)",
  lightSecondary: "oklch(0.62 0.14 82)",
  logoFilter: "hue-rotate(128deg) saturate(0.95)",
});

const violetTheme = stylex.createTheme(paletteVars, {
  darkBackground: "oklch(0.155 0.012 290)",
  darkSurface: "oklch(0.19 0.018 290)",
  darkRaised: "oklch(0.225 0.025 290)",
  darkText: "oklch(0.97 0.008 290)",
  darkMuted: "oklch(0.73 0.03 290)",
  darkBorder: "oklch(0.271 0.009 286)",
  darkAccent: "oklch(0.668 0.176 293)",
  darkAccentHover: "oklch(0.725 0.145 293)",
  darkAccentSoft: "oklch(0.668 0.176 293 / 0.16)",
  darkAccentText: "oklch(0.99 0.004 293)",
  darkSecondary: "oklch(0.75 0.14 235)",
  lightBackground: "oklch(0.97 0.015 293)",
  lightSurface: "oklch(1 0 0)",
  lightSubtle: "oklch(0.94 0.022 293)",
  lightText: "oklch(0.18 0.025 290)",
  lightMuted: "oklch(0.46 0.04 290)",
  lightBorder: "oklch(0.36 0.08 293 / 0.17)",
  lightAccent: "oklch(0.541 0.247 293)",
  lightAccentHover: "oklch(0.48 0.23 293)",
  lightAccentSoft: "oklch(0.541 0.247 293 / 0.1)",
  lightAccentText: "oklch(0.99 0.004 293)",
  lightSecondary: "oklch(0.58 0.16 235)",
  logoFilter: "none",
});

export function isPaletteId(value: string | null): value is PaletteId {
  return value === "mono" || value === "cobalt" || value === "ember" || value === "violet";
}

export function getPaletteTheme(palette: PaletteId) {
  switch (palette) {
    case "mono":
      return monoTheme;
    case "cobalt":
      return cobaltTheme;
    case "ember":
      return emberTheme;
    case "violet":
      return violetTheme;
  }
}
