import * as stylex from "@stylexjs/stylex";
import { BookOpen } from "lucide-react";

import { AgentIcon, type AgentId } from "../AgentIcon";
import { ProductMock, type ThemeToggleOrigin } from "../ProductMock";
import { Arrow, LogoLockup } from "../MarketingBrand";
import { lightSiteTheme, siteTokens, type SiteTheme } from "../SiteTheme.stylex";
import { SidecarWalkthrough } from "../walkthrough/SidecarWalkthrough";
import { FaqSection } from "../home/FaqSection";
import { homeSectionStyles } from "../home/HomeSection.stylex";
import { ProblemSection } from "../home/ProblemSection";
import { UsageSection } from "../home/UsageSection";
import { DOCS_URL, DOWNLOAD_URL, GITHUB_REPOSITORY_URL, TRUST_LINE } from "../site-links";

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

function DownloadButton({ theme }: { theme: SiteTheme }) {
  return (
    <a href={DOWNLOAD_URL} {...stylex.props(styles.primaryButton)}>
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
      <header {...stylex.props(styles.container, styles.header)}>
        <LogoLockup inverse={theme === "dark"} compact />
        <nav {...stylex.props(styles.nav)} aria-label="Main navigation">
          <a href="#how-it-works" {...stylex.props(styles.navLink)}>
            How it works
          </a>
          <a href={DOCS_URL} {...stylex.props(styles.navLink)}>
            Docs
          </a>
          <a href="#download" {...stylex.props(styles.navLink)}>
            Download
          </a>
        </nav>
      </header>

      <main>
        <section {...stylex.props(styles.container, styles.hero)}>
          <div {...stylex.props(styles.copy)}>
            <h1 {...stylex.props(styles.title)}>
              Tidy up your
              <br />
              <span {...stylex.props(styles.titleAccent)}>agent skills.</span>
            </h1>
            <p {...stylex.props(styles.lede)}>
              Skills pile up across your agents and projects. Skill Studio shows which are broken,
              which are duplicates and which your agents never use, so you can clear them out with
              undo.
            </p>
            <div {...stylex.props(styles.actions)}>
              <DownloadButton theme={theme} />
              <a href={DOCS_URL} {...stylex.props(styles.sourceLink)}>
                <BookOpen aria-hidden="true" size={18} />
                <span>Read the docs</span>
              </a>
            </div>
            <p {...stylex.props(styles.fineprint)}>{TRUST_LINE}</p>
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

        <section id="problem" {...stylex.props(styles.container, styles.problemSection)}>
          <ProblemSection />
        </section>

        <section id="how-it-works" {...stylex.props(styles.container, styles.importSection)}>
          <SidecarWalkthrough theme={theme} />
        </section>

        <section id="use-it" {...stylex.props(styles.container, homeSectionStyles.section)}>
          <UsageSection />
        </section>

        <section id="faq" {...stylex.props(styles.container, homeSectionStyles.section)}>
          <FaqSection />
        </section>

        <section id="download" {...stylex.props(styles.container, styles.closingSection)}>
          <h2 {...stylex.props(styles.closingTitle)}>Clear out your skills.</h2>
          <p {...stylex.props(styles.closingCopy)}>
            Keep the ones your agents use and park the rest.
          </p>
          <DownloadButton theme={theme} />
          <p {...stylex.props(styles.fineprint)}>{TRUST_LINE}</p>
          <p {...stylex.props(styles.trust)}>
            Runs on your Mac. It sends anonymous crash reports, which you can turn off. They never
            include skill names, files or paths.
          </p>
        </section>
      </main>

      <footer {...stylex.props(styles.container, styles.footer)}>
        <LogoLockup inverse={theme === "dark"} compact />
        <nav {...stylex.props(styles.footerLinks)} aria-label="Footer navigation">
          <a href={DOCS_URL} {...stylex.props(styles.footerLink)}>
            Docs
          </a>
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
  container: {
    boxSizing: "border-box",
    marginInline: "auto",
    maxWidth: 1280,
    paddingInline: 28,
    width: "100%",
    "@media (max-width: 600px)": { paddingInline: 20 },
  },
  header: {
    alignItems: "center",
    display: "flex",
    justifyContent: "space-between",
    paddingBlock: 22,
    "@media (max-width: 700px)": { paddingBlock: 16 },
  },
  nav: { display: "flex", gap: 26, "@media (max-width: 700px)": { display: "none" } },
  navLink: {
    color: siteTokens.muted,
    fontSize: 14,
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.text },
  },
  hero: {
    alignItems: "center",
    display: "grid",
    gap: "clamp(38px,5vw,74px)",
    gridTemplateColumns: "minmax(460px,.78fr) minmax(0,1.22fr)",
    paddingBlock: "20px 56px",
    "@media (max-width: 1200px)": { gridTemplateColumns: "1fr", paddingTop: 40 },
    "@media (max-width: 600px)": { gap: 52, paddingBlock: "52px 80px" },
  },
  copy: {
    maxWidth: 560,
    "@media (max-width: 1200px)": {
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
    letterSpacing: "-.035em",
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
    "@media (max-width: 1200px)": { justifyContent: "center", width: "100%" },
  },
  fineprint: {
    color: siteTokens.muted,
    fontSize: 12,
    lineHeight: 1.5,
    margin: "14px 0 0",
    textWrap: "balance",
  },
  trust: {
    color: siteTokens.muted,
    fontSize: 13,
    lineHeight: 1.6,
    margin: "20px 0 0",
    maxWidth: "52ch",
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
    fontSize: 15,
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
    minWidth: 0,
    position: "relative",
    // Wider than its column on purpose: the page clips the overflow at the viewport edge,
    // so the demo keeps its real size instead of shrinking to fit.
    "@media (min-width: 1201px)": { width: "max(100%, 960px)" },
    "@media (max-width: 680px)": { marginInline: "auto", maxWidth: 430, width: "100%" },
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
  problemSection: {
    paddingTop: 112,
    "@media (max-width: 600px)": { paddingTop: 72 },
  },
  importSection: {
    paddingBlock: "124px 24px",
    "@media (max-width: 600px)": { paddingBlock: "86px 24px" },
  },
  closingSection: {
    alignItems: "flex-start",
    display: "flex",
    flexDirection: "column",
    paddingBlock: "124px 142px",
    "@media (max-width: 600px)": {
      alignItems: "center",
      paddingBlock: "86px 100px",
      textAlign: "center",
    },
  },
  closingTitle: {
    fontSize: "clamp(42px,5vw,68px)",
    letterSpacing: "-.035em",
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
    paddingBlock: 28,
    paddingBottom: "max(28px, env(safe-area-inset-bottom))",
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
