import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "./PaletteThemes.stylex";
import type { ThemeToggleOrigin } from "./ProductMock";
import type { SiteTheme } from "./SiteTheme.stylex";
import { CommandCenter } from "./variants/CommandCenter";

function systemTheme(): SiteTheme {
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function MarketingSite() {
  const [theme, setTheme] = useState<SiteTheme>(systemTheme);
  const [isSystemTheme, setIsSystemTheme] = useState(true);

  useEffect(() => {
    const preference = window.matchMedia("(prefers-color-scheme: light)");
    const followPreference = () => {
      if (isSystemTheme) setTheme(preference.matches ? "light" : "dark");
    };
    preference.addEventListener("change", followPreference);
    document.documentElement.dataset.siteTheme = theme;
    return () => preference.removeEventListener("change", followPreference);
  }, [isSystemTheme, theme]);

  const toggleTheme = (origin: ThemeToggleOrigin) => {
    const nextTheme = theme === "dark" ? "light" : "dark";
    const commitTheme = () => {
      setIsSystemTheme(false);
      setTheme(nextTheme);
      document.documentElement.dataset.siteTheme = nextTheme;
    };

    const shouldReduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (shouldReduceMotion || !("startViewTransition" in document)) {
      commitTheme();
      return;
    }

    const transition = document.startViewTransition(() => flushSync(commitTheme));
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
          duration: 520,
          easing: "cubic-bezier(0.19, 1, 0.22, 1)",
          pseudoElement: "::view-transition-new(root)",
        },
      );
    });
  };

  return (
    <div {...stylex.props(getPaletteTheme("mono"))}>
      <CommandCenter theme={theme} onToggleTheme={toggleTheme} />
    </div>
  );
}
