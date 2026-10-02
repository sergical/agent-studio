import * as stylex from "@stylexjs/stylex";
import { BookOpen } from "lucide-react";

import { AgentIcon, type AgentId } from "../AgentIcon";
import { ProductMock, type ThemeToggleOrigin } from "../ProductMock";
import { Arrow } from "../MarketingBrand";
import { SiteFooter, SiteHeader, siteLayout } from "../SiteChrome";
import { lightSiteTheme, siteTokens, type SiteTheme } from "../SiteTheme.stylex";
import { SidecarWalkthrough } from "../walkthrough/SidecarWalkthrough";
import { ClosingFinale } from "../home/ClosingFinale";
import { finaleCtaMarker } from "../home/ClosingFinale.stylex";
import { FaqSection } from "../home/FaqSection";
import { homeSectionStyles } from "../home/HomeSection.stylex";
import { MascotStage } from "../home/MascotStage";
import { UsageSection } from "../home/UsageSection";
import { DOCS_URL, DOWNLOAD_INTEL_URL, DOWNLOAD_URL, TRUST_LINE } from "../site-links";

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

function DownloadButton({ theme, inFinale = false }: { theme: SiteTheme; inFinale?: boolean }) {
  return (
    <a href={DOWNLOAD_URL} {...stylex.props(inFinale && finaleCtaMarker, styles.primaryButton)}>
      <span>Download for macOS</span>
      <span aria-hidden="true" {...stylex.props(styles.primaryButtonArrow)}>
        <Arrow inverse={theme === "dark"} />
      </span>
    </a>
  );
}

function TrustLine() {
  return (
    <p {...stylex.props(styles.fineprint)}>
      {TRUST_LINE} ·{" "}
      <a href={DOWNLOAD_INTEL_URL} {...stylex.props(homeSectionStyles.textLink, styles.intelLink)}>
        Intel build
      </a>
    </p>
  );
}

export function CommandCenter({ theme, onToggleTheme }: CommandCenterProps) {
  return (
    <div id="top" {...stylex.props(siteLayout.page, theme === "light" && lightSiteTheme)}>
      <SiteHeader page="home" />

      <main>
        <section {...stylex.props(siteLayout.container, styles.hero)}>
          <div {...stylex.props(styles.copy)}>
            <h1 {...stylex.props(styles.title)}>Tidy up your agent skills.</h1>
            <p {...stylex.props(styles.lede)}>
              Find the broken, duplicate and unused skills across all your agents. Park them, with
              undo.
            </p>
            <div {...stylex.props(styles.actions)}>
              <DownloadButton theme={theme} />
              <a href={DOCS_URL} {...stylex.props(styles.sourceLink)}>
                <BookOpen aria-hidden="true" size={18} />
                <span>Read the docs</span>
              </a>
            </div>
            <TrustLine />
          </div>

          <MascotStage />
        </section>

        <section id="product" {...stylex.props(siteLayout.container, styles.productSection)}>
          <div {...stylex.props(styles.productWrap)}>
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

        <section id="how-it-works" {...stylex.props(siteLayout.container, styles.importSection)}>
          <SidecarWalkthrough theme={theme} />
        </section>

        <section id="use-it" {...stylex.props(siteLayout.container, homeSectionStyles.section)}>
          <UsageSection />
        </section>

        <section id="faq" {...stylex.props(siteLayout.container, homeSectionStyles.section)}>
          <FaqSection />
        </section>
      </main>

      <ClosingFinale theme={theme}>
        <section
          id="download"
          aria-labelledby="download-title"
          {...stylex.props(siteLayout.container, styles.closingSection)}
        >
          <img
            src="/skill-studio-logo.png"
            alt=""
            width={96}
            height={96}
            {...stylex.props(styles.closingMascot)}
          />
          <h2 id="download-title" {...stylex.props(styles.closingTitle)}>
            Clear out your skills.
          </h2>
          <p {...stylex.props(styles.closingCopy)}>
            Keep the ones your agents use and park the rest.
          </p>
          <DownloadButton theme={theme} inFinale />
          <TrustLine />
        </section>
        <SiteFooter page="home" />
      </ClosingFinale>
    </div>
  );
}

const styles = stylex.create({
  hero: {
    alignItems: "center",
    display: "grid",
    gap: "clamp(32px,4vw,64px)",
    gridTemplateColumns: "minmax(0,.9fr) minmax(0,1.1fr)",
    paddingBlock: "40px 88px",
    "@media (max-width: 1080px)": { gridTemplateColumns: "1fr", justifyItems: "center" },
    "@media (max-width: 600px)": { gap: 20, paddingBlock: "28px 56px" },
  },
  copy: {
    maxWidth: 560,
    "@media (max-width: 1080px)": {
      alignItems: "center",
      display: "flex",
      flexDirection: "column",
      textAlign: "center",
      width: "100%",
    },
  },
  title: {
    fontSize: "clamp(48px,6vw,88px)",
    fontWeight: 720,
    letterSpacing: "-.04em",
    lineHeight: 0.95,
    margin: 0,
    textWrap: "balance",
    "@media (max-width: 1080px)": { maxWidth: "12ch" },
  },
  lede: {
    color: siteTokens.muted,
    fontSize: 18,
    lineHeight: 1.6,
    margin: "26px 0 30px",
    maxWidth: "46ch",
    textWrap: "pretty",
    "@media (max-width: 600px)": { maxWidth: "34ch" },
  },
  actions: {
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
    gap: 14,
    "@media (max-width: 1080px)": { justifyContent: "center", width: "100%" },
  },
  fineprint: {
    color: siteTokens.muted,
    fontSize: 12,
    lineHeight: 1.5,
    margin: "14px 0 0",
    textWrap: "balance",
  },
  intelLink: { fontSize: "inherit", minHeight: "auto" },
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
        transform: "translateY(-1px)",
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
  productSection: { paddingBottom: 96, "@media (max-width: 600px)": { paddingBottom: 64 } },
  productWrap: { marginInline: "auto", maxWidth: 1100, position: "relative" },
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
    paddingBlock: "124px 24px",
    "@media (max-width: 600px)": { paddingBlock: "86px 24px" },
  },
  closingSection: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    paddingBlock: "124px 96px",
    textAlign: "center",
    "@media (max-width: 600px)": { paddingBlock: "86px 64px" },
  },
  closingMascot: {
    filter: "drop-shadow(0 18px 32px oklch(0.45 0.2 293 / .4))",
    height: "auto",
    marginBottom: 28,
    // The mascot perks up while the download button below it is hovered.
    transform: {
      default: "none",
      "@media (hover: hover) and (pointer: fine)": {
        [stylex.when.anySibling(":hover", finaleCtaMarker)]: "translateY(-6px) rotate(-6deg)",
      },
    },
    transition: "transform 300ms cubic-bezier(0.34, 1.56, 0.64, 1)",
    width: 96,
    "@media (max-width: 600px)": { width: 80 },
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
    textWrap: "balance",
  },
});
