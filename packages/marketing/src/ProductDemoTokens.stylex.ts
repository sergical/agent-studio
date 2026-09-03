// ============================================================================
// Skill Studio marketing demo - product UI tokens copied from the desktop app
// ============================================================================

import * as stylex from "@stylexjs/stylex";

export const productDemoTokens = stylex.defineVars({
  background: "oklch(0.155 0.004 285.899)",
  surface: "oklch(0.188 0.005 285.823)",
  raised: "oklch(0.224 0.006 285.885)",
  hover: "oklch(1 0 0 / 0.04)",
  border: "oklch(0.271 0.009 285.805)",
  subtleBorder: "oklch(0.237 0.007 285.877)",
  text: "oklch(0.967 0.001 286.375)",
  muted: "oklch(0.967 0.001 286.375 / 0.68)",
  faint: "oklch(0.967 0.001 286.375 / 0.52)",
  accent: "oklch(0.668 0.176 293)",
  accentHover: "oklch(0.725 0.145 293)",
  accentSoft: "oklch(0.668 0.176 293 / 0.16)",
  danger: "oklch(0.704 0.191 22.216)",
  warning: "oklch(0.769 0.188 70.08)",
});

export const productDemoLightTheme = stylex.createTheme(productDemoTokens, {
  background: "oklch(1 0 0)",
  surface: "oklch(0.967 0.001 286.375)",
  raised: "oklch(0.92 0.004 286.32)",
  hover: "oklch(0 0 0 / 0.04)",
  border: "oklch(0 0 0 / 0.08)",
  subtleBorder: "oklch(0 0 0 / 0.04)",
  text: "oklch(0.141 0.004 285.823)",
  muted: "oklch(0.141 0.004 285.823 / 0.7)",
  faint: "oklch(0.141 0.004 285.823 / 0.58)",
  accent: "oklch(0.541 0.247 293.009)",
  accentHover: "oklch(0.491 0.241 292.581)",
  accentSoft: "oklch(0.541 0.247 293.009 / 0.12)",
  danger: "oklch(0.544 0.215 27.325)",
  warning: "oklch(0.534 0.113 70.08)",
});
