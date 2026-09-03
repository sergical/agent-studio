import { useEffect, useRef, useState, type PointerEvent } from "react";
import * as stylex from "@stylexjs/stylex";
import { Code2, Star } from "lucide-react";

import { AgentIcon, type AgentId } from "../AgentIcon";
import { ProductMock, type ThemeToggleOrigin } from "../ProductMock";
import { Arrow, LogoLockup } from "../MarketingBrand";
import { lightSiteTheme, siteTokens, type SiteTheme } from "../SiteTheme.stylex";

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
const GITHUB_REPOSITORY_API_URL = "https://api.github.com/repos/sergical/agent-studio";

function hasStargazerCount(value: unknown): value is { stargazers_count: number } {
  return (
    typeof value === "object" &&
    value !== null &&
    "stargazers_count" in value &&
    typeof value.stargazers_count === "number"
  );
}

function useGitHubStars() {
  const [stars, setStars] = useState<number | null>(null);

  useEffect(() => {
    const controller = new AbortController();

    void fetch(GITHUB_REPOSITORY_API_URL, {
      headers: { Accept: "application/vnd.github+json" },
      signal: controller.signal,
    })
      .then((response) => (response.ok ? response.json() : null))
      .then((repository: unknown) => {
        if (hasStargazerCount(repository)) setStars(repository.stargazers_count);
      })
      .catch(() => undefined);

    return () => controller.abort();
  }, []);

  return stars;
}

function DownloadButton({ theme }: { theme: SiteTheme }) {
  const [downloading, setDownloading] = useState(false);

  const download = () => {
    if (downloading) return;
    setDownloading(true);
    window.setTimeout(() => setDownloading(false), 900);
  };

  return (
    <button
      type="button"
      onClick={download}
      aria-busy={downloading}
      aria-label={downloading ? "Preparing download" : "Download for macOS"}
      {...stylex.props(styles.primaryButton)}
    >
      <span aria-hidden="true" {...stylex.props(styles.primaryButtonLabel)}>
        <span
          {...stylex.props(
            styles.primaryButtonLabelText,
            downloading ? styles.primaryButtonLabelExit : styles.primaryButtonLabelVisible,
          )}
        >
          Download for macOS
        </span>
        <span
          {...stylex.props(
            styles.primaryButtonLabelText,
            styles.primaryButtonLabelEnter,
            downloading && styles.primaryButtonLabelVisible,
          )}
        >
          Preparing download…
        </span>
      </span>
      <span {...stylex.props(styles.primaryButtonArrow)}>
        <span
          {...stylex.props(styles.primaryButtonIcon, downloading && styles.primaryButtonIconExit)}
        >
          <Arrow inverse={theme === "dark"} />
        </span>
        <span
          {...stylex.props(
            styles.primaryButtonIcon,
            styles.primaryButtonIconEnter,
            downloading && styles.primaryButtonIconVisible,
          )}
        >
          <span
            {...stylex.props(styles.downloadSpinner, !downloading && styles.downloadSpinnerPaused)}
            aria-hidden="true"
          />
        </span>
      </span>
    </button>
  );
}

function WalkthroughVideo({
  feature,
  label,
  theme,
  portrait = false,
}: {
  feature: "map" | "invoke" | "drift" | "install";
  label: string;
  theme: SiteTheme;
  portrait?: boolean;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const assetName = feature === "map" ? `map-${theme}-remotion` : `${feature}-${theme}-focused`;

  useEffect(() => {
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const syncPlayback = () => {
      if (preference.matches) {
        video.current?.pause();
        if (video.current) video.current.currentTime = 0;
      } else {
        void video.current?.play().catch(() => undefined);
      }
    };
    syncPlayback();
    preference.addEventListener("change", syncPlayback);
    return () => preference.removeEventListener("change", syncPlayback);
  }, [theme]);

  return (
    <div {...stylex.props(styles.walkthroughMedia, portrait && styles.walkthroughMediaPortrait)}>
      <video
        key={`${feature}-${theme}`}
        ref={video}
        autoPlay
        loop
        muted
        playsInline
        preload="metadata"
        aria-label={label}
        {...stylex.props(styles.walkthroughVideo)}
      >
        <source src={`/walkthrough/${assetName}.mp4`} type="video/mp4" />
        <source src={`/walkthrough/${assetName}.webm`} type="video/webm" />
      </video>
    </div>
  );
}

export function CommandCenter({ theme, onToggleTheme }: CommandCenterProps) {
  const productHalo = useRef<HTMLDivElement>(null);
  const githubStars = useGitHubStars();

  const moveProductHalo = (event: PointerEvent<HTMLDivElement>) => {
    if (
      event.pointerType !== "mouse" ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      return;
    }
    const bounds = event.currentTarget.getBoundingClientRect();
    const x = (event.clientX - bounds.left - bounds.width / 2) * 0.05;
    const y = (event.clientY - bounds.top - bounds.height / 2) * 0.05;
    if (productHalo.current) {
      productHalo.current.style.transform = `translate3d(${x}px, ${y}px, 0) scale(1.04)`;
      productHalo.current.style.opacity = "0.86";
    }
  };

  const resetProductHalo = () => {
    if (productHalo.current) {
      productHalo.current.style.transform = "translate3d(0, 0, 0) scale(1)";
      productHalo.current.style.opacity = "0.62";
    }
  };

  return (
    <div id="top" {...stylex.props(styles.page, theme === "light" && lightSiteTheme)}>
      <header {...stylex.props(styles.header)}>
        <LogoLockup inverse={theme === "dark"} compact />
        <nav {...stylex.props(styles.nav)} aria-label="Main navigation">
          <a href="#product" {...stylex.props(styles.navLink)}>
            Product
          </a>
          <a href="#features" {...stylex.props(styles.navLink)}>
            Features
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
              Every skill.
              <br />
              Every agent.
              <br />
              <span {...stylex.props(styles.titleAccent)}>One place.</span>
            </h1>
            <p {...stylex.props(styles.lede)}>
              Scan one Mac. See every installed skill, which agents can invoke it, and which copies
              have drifted.
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
                <span
                  aria-label={
                    githubStars === null
                      ? "GitHub stars unavailable"
                      : `${githubStars} GitHub stars`
                  }
                  {...stylex.props(styles.sourceStars)}
                >
                  <Star aria-hidden="true" fill="currentColor" size={13} />
                  {githubStars === null ? "–" : githubStars.toLocaleString()}
                </span>
              </a>
            </div>
            <p {...stylex.props(styles.note)}>macOS 10.13 or later</p>
          </div>

          <div
            id="product"
            onPointerMove={moveProductHalo}
            onPointerLeave={resetProductHalo}
            {...stylex.props(styles.productWrap)}
          >
            <div
              ref={productHalo}
              data-product-halo=""
              {...stylex.props(styles.productHalo)}
              aria-hidden="true"
            />
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

        <section id="features" {...stylex.props(styles.importSection)}>
          <div {...stylex.props(styles.importIntro)}>
            <h2 {...stylex.props(styles.importTitle)}>
              Your agents share skills. Their folders don't.
            </h2>
            <p {...stylex.props(styles.importCopy)}>
              Skill Studio scans global, project, shared, and plugin skill directories. It groups
              matching copies and flags broken metadata, drift, and updates.
            </p>
          </div>
          <div {...stylex.props(styles.walkthroughList)}>
            <article {...stylex.props(styles.walkthroughRow)}>
              <div {...stylex.props(styles.walkthroughCopy)}>
                <h3 {...stylex.props(styles.walkthroughTitle)}>Map every copy</h3>
                <p {...stylex.props(styles.walkthroughDescription)}>
                  Global, project, shared, and plugin-installed skills appear in one list. Search,
                  filter, and open any skill to see where every copy lives.
                </p>
              </div>
              <div {...stylex.props(styles.walkthroughPreview)}>
                <WalkthroughVideo
                  feature="map"
                  label="Filtering the Skill Studio skills list"
                  theme={theme}
                />
              </div>
            </article>
            <article {...stylex.props(styles.walkthroughRow)}>
              <div {...stylex.props(styles.walkthroughCopy)}>
                <h3 {...stylex.props(styles.walkthroughTitle)}>Check who can invoke it</h3>
                <p {...stylex.props(styles.walkthroughDescription)}>
                  Open a skill to inspect its locations, agent access, source, and invocation
                  policy. Change whether you, the model, or both can use it.
                </p>
              </div>
              <div {...stylex.props(styles.walkthroughPreview)}>
                <WalkthroughVideo
                  feature="invoke"
                  label="Changing a skill invocation policy"
                  theme={theme}
                />
              </div>
            </article>
            <article {...stylex.props(styles.walkthroughRow)}>
              <div {...stylex.props(styles.walkthroughCopy)}>
                <h3 {...stylex.props(styles.walkthroughTitle)}>Fix drift</h3>
                <p {...stylex.props(styles.walkthroughDescription)}>
                  Broken metadata, mismatched copies, and available updates land in one attention
                  queue. Open the problem, compare it, or pull the latest copy.
                </p>
              </div>
              <div {...stylex.props(styles.walkthroughPreview)}>
                <WalkthroughVideo
                  feature="drift"
                  label="Opening a mismatched skill from the attention queue"
                  theme={theme}
                />
              </div>
            </article>
            <article {...stylex.props(styles.walkthroughRow)}>
              <div {...stylex.props(styles.walkthroughCopy)}>
                <h3 {...stylex.props(styles.walkthroughTitle)}>Choose every target</h3>
                <p {...stylex.props(styles.walkthroughDescription)}>
                  Find a skill on skills.sh or add a repository. Pick the scope and every agent that
                  should receive it before Skill Studio writes a file.
                </p>
              </div>
              <div {...stylex.props(styles.walkthroughPreview)}>
                <WalkthroughVideo
                  feature="install"
                  label="Adding a skill by source and choosing its targets"
                  theme={theme}
                  portrait
                />
              </div>
            </article>
          </div>
        </section>

        <section id="download" {...stylex.props(styles.closingSection)}>
          <h2 {...stylex.props(styles.closingTitle)}>See what every agent is working with.</h2>
          <p {...stylex.props(styles.closingCopy)}>
            Scan your current setup and start with the skills that need attention.
          </p>
          <DownloadButton theme={theme} />
          <span {...stylex.props(styles.closingNote)}>macOS 10.13 or later</span>
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
    overflow: "hidden",
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
        transform: "scale(1.015)",
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
  primaryButtonLabel: {
    display: "grid",
    minWidth: 151,
    textAlign: "left",
  },
  primaryButtonLabelText: {
    gridArea: "1 / 1",
    opacity: 0,
    transform: "translateY(6px)",
    transition: "opacity 140ms ease, transform 220ms cubic-bezier(0.19, 1, 0.22, 1)",
  },
  primaryButtonLabelEnter: { transform: "translateY(6px)" },
  primaryButtonLabelExit: { opacity: 0, transform: "translateY(-6px)" },
  primaryButtonLabelVisible: { opacity: 1, transform: "translateY(0)" },
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
  primaryButtonIcon: {
    alignItems: "center",
    display: "flex",
    inset: 0,
    justifyContent: "center",
    opacity: 1,
    position: "absolute",
    transform: "translateX(0) scale(1)",
    transition: "opacity 140ms ease, transform 220ms cubic-bezier(0.19, 1, 0.22, 1)",
  },
  primaryButtonIconEnter: { opacity: 0, transform: "translateX(-5px) scale(.88)" },
  primaryButtonIconExit: { opacity: 0, transform: "translateX(5px) scale(.88)" },
  primaryButtonIconVisible: { opacity: 1, transform: "translateX(0) scale(1)" },
  downloadSpinner: {
    animationDuration: "700ms",
    animationIterationCount: "infinite",
    animationName: stylex.keyframes({ to: { transform: "rotate(360deg)" } }),
    animationTimingFunction: "linear",
    borderColor: "currentColor transparent currentColor currentColor",
    borderRadius: "50%",
    borderStyle: "solid",
    borderWidth: 1.5,
    height: 14,
    width: 14,
  },
  downloadSpinnerPaused: { animationPlayState: "paused" },
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
  sourceStars: {
    alignItems: "center",
    borderLeftColor: siteTokens.border,
    borderLeftStyle: "solid",
    borderLeftWidth: 1,
    color: siteTokens.muted,
    display: "inline-flex",
    fontSize: 12,
    fontVariantNumeric: "tabular-nums",
    gap: 5,
    marginLeft: 2,
    minWidth: 31,
    paddingLeft: 10,
  },
  note: {
    color: siteTokens.muted,
    fontSize: 11,
    margin: "13px 0 0",
    "@media (max-width: 600px)": { marginTop: 15 },
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
    transition: "opacity 220ms ease, transform 600ms cubic-bezier(0.19, 1, 0.22, 1)",
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
  importIntro: {
    maxWidth: 680,
    "@media (max-width: 600px)": { marginInline: "auto", textAlign: "center" },
  },
  importTitle: {
    fontSize: "clamp(34px,4vw,54px)",
    letterSpacing: "-.045em",
    lineHeight: 1,
    margin: 0,
    textWrap: "balance",
  },
  importCopy: {
    color: siteTokens.muted,
    fontSize: 16,
    lineHeight: 1.6,
    margin: "24px 0 0",
    maxWidth: "52ch",
  },
  walkthroughList: {
    display: "flex",
    flexDirection: "column",
    gap: 120,
    marginTop: 104,
    "@media (max-width: 800px)": { gap: 88, marginTop: 72 },
    "@media (max-width: 600px)": { gap: 76, marginTop: 64 },
  },
  walkthroughRow: {
    alignItems: "center",
    display: "grid",
    gap: "clamp(42px,6vw,82px)",
    gridTemplateColumns: "minmax(250px,.58fr) minmax(560px,1.42fr)",
    "@media (max-width: 920px)": { gap: 30, gridTemplateColumns: "1fr" },
  },
  walkthroughCopy: {
    maxWidth: 360,
    "@media (max-width: 920px)": { maxWidth: 560 },
    "@media (max-width: 600px)": { marginInline: "auto", textAlign: "center" },
  },
  walkthroughTitle: {
    fontSize: "clamp(27px,3vw,38px)",
    letterSpacing: "-.04em",
    lineHeight: 1.04,
    margin: 0,
    textWrap: "balance",
  },
  walkthroughDescription: {
    color: siteTokens.muted,
    fontSize: 15,
    lineHeight: 1.65,
    margin: "20px 0 0",
    maxWidth: "46ch",
  },
  walkthroughPreview: {
    display: "flex",
    justifyContent: "center",
    minWidth: 0,
    position: "relative",
  },
  walkthroughMedia: {
    aspectRatio: "16 / 9",
    backgroundColor: siteTokens.surface,
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    boxShadow: "0 2px 5px oklch(0 0 0 / .18), 0 24px 70px oklch(0 0 0 / .3)",
    isolation: "isolate",
    overflow: "hidden",
    width: "100%",
  },
  walkthroughMediaPortrait: {
    aspectRatio: "3 / 4",
    maxWidth: 430,
  },
  walkthroughVideo: {
    display: "block",
    height: "100%",
    objectFit: "cover",
    pointerEvents: "none",
    userSelect: "none",
    width: "100%",
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
  closingNote: { color: siteTokens.muted, fontSize: 11, marginTop: 15 },
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
