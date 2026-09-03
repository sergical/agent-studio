import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "./PaletteThemes.stylex";
import { ProductMock, type ThemeToggleOrigin } from "./ProductMock";
import type { SiteTheme } from "./SiteTheme.stylex";
import { CommandCenter } from "./variants/CommandCenter";

function systemTheme(): SiteTheme {
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

type WalkthroughCapture = "map" | "invoke" | "drift" | "install";

function isWalkthroughCapture(value: string | null): value is WalkthroughCapture {
  return value === "map" || value === "invoke" || value === "drift" || value === "install";
}

export function MarketingSite() {
  const searchParams = new URLSearchParams(window.location.search);
  const capture = searchParams.get("capture");
  const requestedCaptureTheme = searchParams.get("captureTheme");
  const captureTheme: SiteTheme = requestedCaptureTheme === "light" ? "light" : "dark";
  const [theme, setTheme] = useState<SiteTheme>(systemTheme);
  const [isSystemTheme, setIsSystemTheme] = useState(true);

  useEffect(() => {
    const preference = window.matchMedia("(prefers-color-scheme: light)");
    const followPreference = () => {
      if (isSystemTheme) setTheme(preference.matches ? "light" : "dark");
    };
    preference.addEventListener("change", followPreference);
    document.documentElement.dataset.siteTheme = isWalkthroughCapture(capture)
      ? captureTheme
      : theme;
    return () => preference.removeEventListener("change", followPreference);
  }, [capture, captureTheme, isSystemTheme, theme]);

  const toggleTheme = (origin: ThemeToggleOrigin) => {
    const nextTheme = theme === "dark" ? "light" : "dark";
    const commitTheme = () => {
      setIsSystemTheme(false);
      setTheme(nextTheme);
      document.documentElement.dataset.siteTheme = nextTheme;
    };

    const shouldReduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (shouldReduceMotion || typeof document.startViewTransition !== "function") {
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

  if (isWalkthroughCapture(capture)) {
    return (
      <div {...stylex.props(styles.captureStage)}>
        <ProductMock
          theme={captureTheme}
          onToggleTheme={toggleTheme}
          initialView={capture === "drift" ? "home" : "skills"}
          initialSkillName={capture === "invoke" ? "commit" : undefined}
          initialInstall={capture === "install"}
          variant="capture"
        />
      </div>
    );
  }

  return (
    <div {...stylex.props(getPaletteTheme("mono"))}>
      <CommandCenter theme={theme} onToggleTheme={toggleTheme} />
    </div>
  );
}

const styles = stylex.create({
  captureStage: {
    height: 912,
    overflow: "hidden",
    width: 1600,
  },
});
