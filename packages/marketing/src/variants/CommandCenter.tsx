import * as stylex from "@stylexjs/stylex";
import { Code2 } from "lucide-react";

import { AgentIcon, type AgentId } from "../AgentIcon";
import { ProductMock, type ThemeToggleOrigin } from "../ProductMock";
import { Arrow, LogoLockup } from "../MarketingBrand";
import { lightSiteTheme, siteTokens, type SiteTheme } from "../SiteTheme.stylex";
import { SidecarWalkthrough } from "../walkthrough/SidecarWalkthrough";

interface CommandCenterProps {
  theme: SiteTheme;
  onToggleTheme: (origin: ThemeToggleOrigin) => void;
}

const supportedAgents = [
  { id: "claude", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "opencode", name: "OpenCode" },
  { id: "pi", name: "pi" },
  { id: "cursor", name: "Cursor" },
  { id: "grok", name: "Grok Build" },
] satisfies ReadonlyArray<{ id: AgentId; name: string }>;

const marqueeMove = stylex.keyframes({
  to: { transform: "translateX(-50%)" },
});

const GITHUB_REPOSITORY_URL = "https://github.com/sergical/agent-studio";

function DownloadButton({ theme }: { theme: SiteTheme }) {
  return (
    <a href={`${GITHUB_REPOSITORY_URL}/releases`} {...stylex.props(styles.primaryButton)}>
      <span>Download for macOS</span>
      <span aria-hidden="true" {...stylex.props(styles.primaryButtonArrow)}>
        <Arrow inverse={theme === "dark"} />
      </span>
    </a>
  );
}

export function CommandCenter({ theme, onToggleTheme }: CommandCenterProps) {
  return (
    <div id="top" {...stylex.props(styles.page, theme === "light" && lightSiteTheme)}>
      <header {...stylex.props(styles.header)}>
        <LogoLockup inverse={theme === "dark"} compact />
        <nav {...stylex.props(styles.nav)} aria-label="Main navigation">
          <a href="#how-it-works" {...stylex.props(styles.navLink)}>
            How it works
          </a>
          <a href="#download" {...stylex.props(styles.navLink)}>
            Download
          </a>
        </nav>
      </header>

      <main>
        <section {...stylex.props(styles.hero)}>
          <div {...stylex.props(styles.copy)}>
            <h1 {...stylex.props(styles.title)}>
              Your skills.
              <br />
              Your agents.
              <br />
              <span {...stylex.props(styles.titleAccent)}>One place.</span>
            </h1>
            <p {...stylex.props(styles.lede)}>
              A desktop app for managing agent skills. Find installed copies, compare changes, and
              choose where to install.
            </p>
            <div {...stylex.props(styles.actions)}>
              <DownloadButton theme={theme} />
              <a
                href={GITHUB_REPOSITORY_URL}
                target="_blank"
                rel="noreferrer"
                {...stylex.props(styles.sourceLink)}
              >
                <Code2 aria-hidden="true" size={18} />
                <span>Browse source</span>
              </a>
            </div>
          </div>

          <div id="product" {...stylex.props(styles.productWrap)}>
            <div {...stylex.props(styles.productHalo)} aria-hidden="true" />
            <ProductMock theme={theme} onToggleTheme={onToggleTheme} />
          </div>
        </section>

        <section {...stylex.props(styles.agentProof)} aria-label="Supported agents">
          <span {...stylex.props(styles.agentProofLabel)}>Works with</span>
          <div {...stylex.props(styles.desktopAgentList)}>
            {supportedAgents.map((agent) => (
              <span key={agent.id} {...stylex.props(styles.agentMark)}>
                <AgentIcon agent={agent.id} size={20} />
                {agent.name}
              </span>
            ))}
          </div>
          <div {...stylex.props(styles.marqueeViewport)}>
            <div {...stylex.props(styles.marqueeTrack)} aria-hidden="true">
              {[0, 1].map((copy) => (
                <div key={copy} {...stylex.props(styles.marqueeSet)}>
                  {supportedAgents.map((agent) => (
                    <span key={`${copy}-${agent.id}`} {...stylex.props(styles.agentMark)}>
                      <AgentIcon agent={agent.id} size={20} />
                      {agent.name}
                    </span>
                  ))}
                </div>
              ))}
            </div>
            <span {...stylex.props(styles.visuallyHidden)}>
              Claude Code, Codex, OpenCode, pi, Cursor, and Grok Build
            </span>
          </div>
        </section>

        <section id="how-it-works" {...stylex.props(styles.importSection)}>
          <SidecarWalkthrough theme={theme} />
        </section>

        <section id="download" {...stylex.props(styles.closingSection)}>
          <h2 {...stylex.props(styles.closingTitle)}>Get Skill Studio.</h2>
          <p {...stylex.props(styles.closingCopy)}>
            Find your installed skills and check which agents can use them.
          </p>
          <DownloadButton theme={theme} />
        </section>
      </main>

      <footer {...stylex.props(styles.footer)}>
        <LogoLockup inverse={theme === "dark"} compact />
        <nav {...stylex.props(styles.footerLinks)} aria-label="Footer navigation">
          <a
            href={GITHUB_REPOSITORY_URL}
            target="_blank"
            rel="noreferrer"
            {...stylex.props(styles.footerLink)}
          >
            Source
          </a>
          <a href="#top" {...stylex.props(styles.footerLink)}>
            Back to top
          </a>
        </nav>
      </footer>
    </div>
  );
}

const styles = stylex.create({
  page: {
    backgroundColor: siteTokens.background,
    color: siteTokens.text,
    minHeight: "100vh",
    overflow: "clip",
  },
  header: {
    alignItems: "center",
    display: "grid",
    gridTemplateColumns: "1fr auto 1fr",
    margin: "0 auto",
    maxWidth: 1280,
    padding: "22px 28px",
    "@media (max-width: 700px)": { display: "flex", padding: "16px 20px" },
  },
  nav: { display: "flex", gap: 26, "@media (max-width: 700px)": { display: "none" } },
  navLink: {
    color: siteTokens.muted,
    fontSize: 12,
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.text },
  },
  hero: {
    alignItems: "center",
    display: "grid",
    gap: "clamp(38px,5vw,74px)",
    gridTemplateColumns: "minmax(460px,.78fr) minmax(0,1.22fr)",
    margin: "0 auto",
    maxWidth: 1380,
    minHeight: 720,
    padding: "72px 28px 62px",
    "@media (max-width: 1120px)": { gridTemplateColumns: "1fr", paddingTop: 54 },
    "@media (max-width: 600px)": { gap: 52, minHeight: 0, padding: "52px 20px 80px" },
  },
  copy: {
    maxWidth: 520,
    "@media (max-width: 600px)": {
      alignItems: "center",
      display: "flex",
      flexDirection: "column",
      marginInline: "auto",
      textAlign: "center",
      width: "100%",
    },
  },
  title: {
    fontSize: "clamp(58px,5.5vw,86px)",
    fontWeight: 720,
    letterSpacing: "-.067em",
    lineHeight: 0.9,
    margin: 0,
  },
  titleAccent: { color: siteTokens.accent },
  lede: {
    color: siteTokens.muted,
    fontSize: 18,
    lineHeight: 1.6,
    margin: "30px 0 28px",
    maxWidth: "54ch",
    textWrap: "pretty",
    "@media (max-width: 600px)": { maxWidth: "34ch" },
  },
  actions: {
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
    gap: 14,
    "@media (max-width: 600px)": { justifyContent: "center", width: "100%" },
  },
  primaryButton: {
    textDecoration: "none",
    alignItems: "center",
    backgroundColor: siteTokens.accent,
    border: 0,
    borderColor: "oklch(1 0 0 / .28)",
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    boxShadow: "0 1px 0 oklch(1 0 0 / .3) inset, 0 10px 30px oklch(0 0 0 / .3)",
    color: siteTokens.accentText,
    display: "inline-flex",
    fontFamily: "inherit",
    fontSize: 15,
    fontWeight: 680,
    gap: 18,
    justifyContent: "space-between",
    minHeight: 54,
    minWidth: 248,
    padding: "7px 8px 7px 18px",
    transition:
      "background-color 150ms ease-out, box-shadow 150ms ease-out, transform 150ms ease-out",
    "@media (hover: hover) and (pointer: fine)": {
      ":hover": {
        backgroundColor: siteTokens.accentHover,
        boxShadow: "0 1px 0 oklch(1 0 0 / .35) inset, 0 14px 38px oklch(0 0 0 / .38)",
      },
      ":hover:active": {
        boxShadow: "0 1px 0 oklch(1 0 0 / .18) inset, 0 4px 12px oklch(0 0 0 / .24)",
        transform: "translateY(2px) scale(.96)",
      },
    },
    ":active": {
      boxShadow: "0 1px 0 oklch(1 0 0 / .18) inset, 0 4px 12px oklch(0 0 0 / .24)",
      transform: "translateY(2px) scale(.96)",
    },
    "@media (max-width: 600px)": { fontSize: 16, minHeight: 56, width: "100%" },
  },
  primaryButtonArrow: {
    alignItems: "center",
    backgroundColor: siteTokens.accentText,
    borderRadius: 7,
    color: siteTokens.text,
    display: "flex",
    height: 38,
    justifyContent: "center",
    overflow: "hidden",
    position: "relative",
    width: 38,
  },
  sourceLink: {
    alignItems: "center",
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    color: siteTokens.text,
    display: "inline-flex",
    fontSize: 14,
    fontWeight: 620,
    gap: 9,
    justifyContent: "center",
    minHeight: 54,
    paddingInline: 16,
    textDecoration: "none",
    transition:
      "background-color 150ms ease-out, border-color 150ms ease-out, transform 150ms ease-out",
    ":hover": { backgroundColor: siteTokens.surface, borderColor: siteTokens.muted },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 600px)": { minHeight: 48 },
  },
  productWrap: {
    height: 542,
    minWidth: 0,
    position: "relative",
    transform: "translateY(58px)",
    "@media (max-width: 1050px)": { transform: "none" },
    "@media (max-width: 680px)": { marginInline: "auto", maxWidth: 430, width: "100%" },
    "@media (max-width: 600px)": { height: 520, overflow: "visible" },
  },
  productHalo: {
    backgroundColor: siteTokens.accentSoft,
    borderRadius: "50%",
    filter: "blur(55px)",
    inset: "8% 4%",
    opacity: 0.62,
    pointerEvents: "none",
    position: "absolute",
  },
  agentProof: {
    alignItems: "center",
    borderBottomColor: siteTokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    borderTopColor: siteTokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    color: siteTokens.muted,
    display: "flex",
    fontSize: 12,
    gap: 28,
    justifyContent: "center",
    minHeight: 76,
    overflow: "hidden",
    padding: "0 28px",
    "@media (max-width: 600px)": { gap: 18, minHeight: 68, paddingInline: 20 },
  },
  agentProofLabel: {
    color: siteTokens.muted,
    flexShrink: 0,
    fontSize: 12,
    whiteSpace: "nowrap",
  },
  desktopAgentList: {
    alignItems: "center",
    display: "flex",
    gap: 34,
    justifyContent: "center",
    "@media (max-width: 860px)": { display: "none" },
  },
  marqueeViewport: {
    display: "none",
    maskImage: "linear-gradient(90deg, transparent, black 5%, black 95%, transparent)",
    minWidth: 0,
    overflow: "hidden",
    position: "relative",
    width: "100%",
    "@media (max-width: 860px)": { display: "block" },
  },
  marqueeTrack: {
    alignItems: "center",
    animationDuration: "20s",
    animationIterationCount: "infinite",
    animationName: marqueeMove,
    animationTimingFunction: "linear",
    display: "flex",
    width: "max-content",
    ":hover": { animationPlayState: "paused" },
    "@media (prefers-reduced-motion: reduce)": { animationName: "none" },
  },
  marqueeSet: {
    alignItems: "center",
    display: "flex",
    gap: 40,
    paddingRight: 40,
  },
  agentMark: {
    alignItems: "center",
    color: siteTokens.text,
    display: "flex",
    fontSize: 13,
    fontWeight: 560,
    gap: 9,
    whiteSpace: "nowrap",
  },
  visuallyHidden: {
    clip: "rect(0 0 0 0)",
    clipPath: "inset(50%)",
    height: 1,
    overflow: "hidden",
    position: "absolute",
    whiteSpace: "nowrap",
    width: 1,
  },
  importSection: {
    margin: "0 auto",
    maxWidth: 1220,
    padding: "124px 28px 48px",
    "@media (max-width: 600px)": { padding: "86px 20px 24px" },
  },
  closingSection: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    padding: "150px 28px 142px",
    textAlign: "center",
    "@media (max-width: 600px)": { padding: "104px 20px 100px" },
  },
  closingTitle: {
    fontSize: "clamp(42px,5vw,68px)",
    letterSpacing: "-.055em",
    lineHeight: 0.98,
    margin: 0,
    maxWidth: "14ch",
    textWrap: "balance",
  },
  closingCopy: {
    color: siteTokens.muted,
    fontSize: 17,
    lineHeight: 1.6,
    margin: "24px 0 30px",
    maxWidth: "48ch",
  },
  footer: {
    alignItems: "center",
    borderTopColor: siteTokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
    justifyContent: "space-between",
    margin: "0 auto",
    maxWidth: 1280,
    padding: "28px",
    paddingBottom: "max(28px, env(safe-area-inset-bottom))",
    "@media (max-width: 600px)": { paddingInline: 20 },
  },
  footerLinks: { alignItems: "center", display: "flex", gap: 22 },
  footerLink: {
    color: siteTokens.muted,
    fontSize: 12,
    minHeight: 44,
    alignItems: "center",
    display: "inline-flex",
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.text },
  },
});
