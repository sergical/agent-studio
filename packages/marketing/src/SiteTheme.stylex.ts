import * as stylex from "@stylexjs/stylex";

import { paletteVars } from "./PaletteThemes.stylex";

export type SiteTheme = "dark" | "light";

export const siteTokens = stylex.defineVars({
  accent: paletteVars.darkAccent,
  accentHover: paletteVars.darkAccentHover,
  accentSoft: paletteVars.darkAccentSoft,
  accentText: paletteVars.darkAccentText,
  background: paletteVars.darkBackground,
  border: paletteVars.darkBorder,
  muted: paletteVars.darkMuted,
  surface: paletteVars.darkSurface,
  text: paletteVars.darkText,
});

export const lightSiteTheme = stylex.createTheme(siteTokens, {
  accent: paletteVars.lightAccent,
  accentHover: paletteVars.lightAccentHover,
  accentSoft: paletteVars.lightAccentSoft,
  accentText: paletteVars.lightAccentText,
  background: paletteVars.lightBackground,
  border: paletteVars.lightBorder,
  muted: paletteVars.lightMuted,
  surface: paletteVars.lightSurface,
  text: paletteVars.lightText,
});
