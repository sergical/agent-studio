import * as stylex from "@stylexjs/stylex";

import { paletteVars } from "./PaletteThemes.stylex";

interface LogoLockupProps {
  inverse?: boolean;
  compact?: boolean;
}

export function LogoLockup({ inverse = false, compact = false }: LogoLockupProps) {
  return (
    <a href="#top" {...stylex.props(brandStyles.lockup, inverse && brandStyles.inverse)}>
      <img
        src="/skill-studio-logo.png"
        alt=""
        width={compact ? 32 : 38}
        height={compact ? 32 : 38}
        {...stylex.props(brandStyles.logo)}
      />
      <span {...stylex.props(brandStyles.wordmark, compact && brandStyles.compactWordmark)}>
        Skill Studio
      </span>
    </a>
  );
}

interface ArrowProps {
  inverse?: boolean;
}

export function Arrow({ inverse = false }: ArrowProps) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      width="16"
      height="16"
      {...stylex.props(brandStyles.arrow, inverse && brandStyles.inverse)}
    >
      <path
        d="M3 8h9M8.5 4.5 12 8l-3.5 3.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

const brandStyles = stylex.create({
  lockup: {
    alignItems: "center",
    color: paletteVars.lightText,
    display: "inline-flex",
    gap: 9,
    textDecoration: "none",
  },
  inverse: {
    color: paletteVars.darkText,
  },
  logo: {
    display: "block",
    filter: paletteVars.logoFilter,
    height: "auto",
    objectFit: "contain",
    transition: "transform 300ms cubic-bezier(0.19, 1, 0.22, 1)",
    "@media (hover: hover) and (pointer: fine)": {
      ":hover": { transform: "rotate(-6deg) scale(1.06)" },
    },
  },
  wordmark: {
    fontSize: 15,
    fontWeight: 680,
    letterSpacing: "-0.025em",
  },
  compactWordmark: {
    fontSize: 14,
  },
  arrow: {
    flexShrink: 0,
  },
});
