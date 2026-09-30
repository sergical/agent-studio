import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "./PaletteThemes.stylex";
import type { ThemeToggleOrigin } from "./ProductMock";
import {
  applySiteTheme,
  currentSiteTheme,
  onSystemThemeChange,
  rememberSiteTheme,
  rememberedSiteTheme,
  transitionSiteTheme,
} from "./site-theme";
import type { SiteTheme } from "./SiteTheme.stylex";
import { CommandCenter } from "./variants/CommandCenter";

interface MarketingSiteProps {
  /** Set only by the prerender, which has no document to read the theme from. */
  prerenderTheme?: SiteTheme;
}

export function MarketingSite({ prerenderTheme }: MarketingSiteProps) {
  const [theme, setTheme] = useState<SiteTheme>(() => prerenderTheme ?? currentSiteTheme());
  const [isSystemTheme, setIsSystemTheme] = useState(
    () => prerenderTheme !== undefined || rememberedSiteTheme() === null,
  );

  useEffect(() => {
    applySiteTheme(theme);
    if (!isSystemTheme) return;
    return onSystemThemeChange(setTheme);
  }, [isSystemTheme, theme]);

  const toggleTheme = (origin: ThemeToggleOrigin) => {
    const nextTheme = theme === "dark" ? "light" : "dark";
    rememberSiteTheme(nextTheme);
    transitionSiteTheme(origin, () =>
      flushSync(() => {
        setIsSystemTheme(false);
        setTheme(nextTheme);
        applySiteTheme(nextTheme);
      }),
    );
  };

  return (
    <div {...stylex.props(getPaletteTheme("violet"))}>
      <CommandCenter theme={theme} onToggleTheme={toggleTheme} />
    </div>
  );
}
