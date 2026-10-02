// ============================================================================
// Skill Studio - Use it your way
// One window, three ways in. Picking a tab plays that surface's short script;
// nothing moves until the visitor asks.
// ============================================================================
import { useEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import * as stylex from "@stylexjs/stylex";
import { AppWindow, Check, Copy, Plug, RotateCcw, Terminal, type LucideIcon } from "lucide-react";

import { siteTokens } from "../SiteTheme.stylex";
import { CLI_DOCS_URL, DOWNLOAD_URL, MCP_DOCS_URL } from "../site-links";
import { homeSectionStyles } from "./HomeSection.stylex";

type Mode = "app" | "cli" | "mcp";
type Tone = "plain" | "muted" | "warn" | "user" | "tool" | "accent";
interface Line {
  text: string;
  tone?: Tone;
  typed?: boolean;
  pause?: number;
}

const modes: ReadonlyArray<{
  id: Mode;
  label: string;
  icon: LucideIcon;
  blurb: string;
  title: string;
}> = [
  {
    id: "app",
    label: "App",
    icon: AppWindow,
    blurb: "Browse, fix and park on your Mac.",
    title: "Skill Studio",
  },
  {
    id: "cli",
    label: "CLI",
    icon: Terminal,
    blurb: "Check every skill from a terminal.",
    title: "zsh — ~/projects",
  },
  {
    id: "mcp",
    label: "MCP",
    icon: Plug,
    blurb: "Let your agent tidy its own skills.",
    title: "claude — ~/projects",
  },
];

const scripts = {
  cli: [
    { text: "$ npx skill-studio diagnose", typed: true, pause: 380 },
    { text: "Issues:", tone: "muted" },
    { text: "  [error] broken link, pdf-tools: the folder it points at is gone", tone: "warn" },
    { text: "  [warning] header can be fixed, motion: name is missing", tone: "warn" },
    { text: "  [warning] copies differ, writing-skills: 2 copies", tone: "warn", pause: 520 },
    { text: "$ npx skill-studio usage", typed: true, pause: 380 },
    { text: "Not used in the last 30 days:", tone: "muted" },
    { text: "  release-notes      last used never" },
    { text: "  deploy-preview     last used never" },
    { text: "  pdf-tools          last used 2026-08-21" },
    { text: "  …and 9 more", tone: "muted", pause: 260 },
    { text: "12 of 31 skills not used in 30 days", tone: "accent" },
  ],
  mcp: [
    { text: "› Park the skills I haven’t used this month.", tone: "user", typed: true, pause: 420 },
    { text: "⏺ skill-studio · skill_usage (days: 30)", tone: "tool", pause: 300 },
    { text: "  ⎿ 12 of 31 skills not used in 30 days", tone: "muted", pause: 360 },
    { text: "⏺ skill-studio · park × 12", tone: "tool", pause: 300 },
    {
      text: "  ⎿ Parked release-notes, deploy-preview, pdf-tools and 9 more",
      tone: "muted",
      pause: 420,
    },
    { text: "⏺ Done. 12 skills are parked, so your agents load 19.", tone: "plain" },
    { text: "  Say “undo” and I’ll unpark them.", tone: "plain" },
    { text: "  motion also has a broken header. Want me to fix it?", tone: "plain" },
  ],
} satisfies Record<Exclude<Mode, "app">, ReadonlyArray<Line>>;

const commands = {
  cli: { text: "npx skill-studio diagnose", docs: CLI_DOCS_URL },
  mcp: { text: "claude mcp add skill-studio -- npx -y skill-studio mcp", docs: MCP_DOCS_URL },
} satisfies Record<Exclude<Mode, "app">, { text: string; docs: string }>;

function prefersReducedMotion() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

// Plays a script: typed lines appear character by character, output lines arrive staggered.
function useScript(lines: ReadonlyArray<Line>, runId: number) {
  const [state, setState] = useState(() =>
    runId === 0 ? { shown: lines.length, chars: Infinity } : { shown: 0, chars: 0 },
  );

  useEffect(() => {
    if (runId === 0 || prefersReducedMotion()) {
      setState({ shown: lines.length, chars: Infinity });
      return;
    }
    let cancelled = false;
    let timer = 0;
    const wait = (ms: number) =>
      new Promise<void>((resolve) => {
        timer = window.setTimeout(resolve, ms);
      });
    (async () => {
      setState({ shown: 0, chars: 0 });
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        if (line.typed) {
          setState({ shown: i + 1, chars: 0 });
          for (let c = 1; c <= line.text.length; c++) {
            await wait(c < 3 ? 60 : 24);
            if (cancelled) return;
            setState({ shown: i + 1, chars: c });
          }
        } else {
          setState({ shown: i + 1, chars: Infinity });
        }
        await wait(line.pause ?? 70);
        if (cancelled) return;
      }
    })();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [lines, runId]);

  return state;
}

function ScriptView({ lines, runId }: { lines: ReadonlyArray<Line>; runId: number }) {
  const { shown, chars } = useScript(lines, runId);
  return (
    <pre {...stylex.props(styles.screen)}>
      {lines.slice(0, shown).map((line, i) => {
        const current = i === shown - 1;
        const text = current && line.typed ? line.text.slice(0, chars) : line.text;
        const typing = current && line.typed && chars < line.text.length;
        return (
          <div key={i} {...stylex.props(styles.line, styles[line.tone ?? "plain"])}>
            {text}
            {typing && <span {...stylex.props(styles.caret)} />}
          </div>
        );
      })}
      {shown >= lines.length && <span {...stylex.props(styles.caret, styles.caretIdle)} />}
    </pre>
  );
}

const appRows = [
  { name: "agent-browser", used: "used today", unused: false },
  { name: "release-notes", used: "never used", unused: true },
  { name: "tailwind", used: "used 2 days ago", unused: false },
  { name: "deploy-preview", used: "never used", unused: true },
  { name: "commit", used: "used today", unused: false },
  { name: "pdf-tools", used: "41 days ago", unused: true },
];
const unusedCount = appRows.filter((r) => r.unused).length;

interface AppViewProps {
  runId: number;
  announce: (message: string) => void;
  undoFocusRef: RefObject<HTMLButtonElement | null>;
}

function AppView({ runId, announce, undoFocusRef }: AppViewProps) {
  const [parked, setParked] = useState(0);
  const [toast, setToast] = useState(false);

  useEffect(() => announce(toast ? `Parked ${unusedCount} skills` : ""), [toast, announce]);

  useEffect(() => {
    if (runId === 0) return;
    if (prefersReducedMotion()) {
      setParked(unusedCount);
      setToast(true);
      return;
    }
    setParked(0);
    setToast(false);
    const timers = [
      ...Array.from({ length: unusedCount }, (_, i) =>
        window.setTimeout(() => setParked(i + 1), 650 + i * 320),
      ),
      window.setTimeout(() => setToast(true), 650 + unusedCount * 320 + 120),
    ];
    return () => timers.forEach((t) => window.clearTimeout(t));
  }, [runId]);

  let seen = 0;
  return (
    <div {...stylex.props(styles.app)}>
      <div {...stylex.props(styles.appHead)}>
        <span>Not used in 30 days</span>
        <span {...stylex.props(styles.appCount)}>{unusedCount - parked} to review</span>
      </div>
      <ul {...stylex.props(styles.appList)}>
        {appRows.map((row) => {
          const isParked = row.unused && seen++ < parked;
          return (
            <li key={row.name} {...stylex.props(styles.appRow, isParked && styles.appRowParked)}>
              <span
                {...stylex.props(styles.appDot, row.unused ? styles.dotIdle : styles.dotUsed)}
              />
              <span {...stylex.props(styles.appName)}>{row.name}</span>
              <span {...stylex.props(styles.appMeta)}>{row.used}</span>
              <span {...stylex.props(styles.badge, isParked && styles.badgeOn)}>Parked</span>
            </li>
          );
        })}
      </ul>
      <div
        aria-hidden={!toast}
        inert={!toast}
        data-surface="inverse"
        {...stylex.props(styles.toast, toast && styles.toastOn)}
      >
        <span aria-hidden="true">Parked {unusedCount} skills</span>
        <button
          type="button"
          onClick={() => {
            setParked(0);
            setToast(false);
            // The toast turns inert, so focus would otherwise fall back to the page body.
            undoFocusRef.current?.focus();
          }}
          {...stylex.props(styles.toastUndo)}
        >
          Undo
        </button>
      </div>
    </div>
  );
}

function CopyCommand({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef(0);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <div {...stylex.props(styles.command)}>
      <code {...stylex.props(homeSectionStyles.code, styles.commandText)}>{text}</code>
      <button
        type="button"
        aria-label={copied ? "Copied" : "Copy command"}
        onClick={() => {
          void navigator.clipboard?.writeText(text).then(
            () => {
              setCopied(true);
              window.clearTimeout(timer.current);
              timer.current = window.setTimeout(() => setCopied(false), 1500);
            },
            () => setCopied(false),
          );
        }}
        {...stylex.props(styles.copyButton)}
      >
        {copied ? <Check size={16} aria-hidden="true" /> : <Copy size={16} aria-hidden="true" />}
      </button>
    </div>
  );
}

const TAB_STEPS = new Map([
  ["ArrowRight", 1],
  ["ArrowDown", 1],
  ["ArrowLeft", -1],
  ["ArrowUp", -1],
]);

export function UsageSection() {
  const [mode, setMode] = useState<Mode>("cli");
  const [runId, setRunId] = useState(0);
  // Lives outside the per-run app view so screen readers already know the region when
  // its text arrives.
  const [status, setStatus] = useState("");
  const replayRef = useRef<HTMLButtonElement>(null);
  const active = modes.find((m) => m.id === mode)!;

  const pick = (next: Mode) => {
    setMode(next);
    setRunId((r) => r + 1);
    setStatus("");
  };

  const onTabKey = (event: KeyboardEvent) => {
    const step = TAB_STEPS.get(event.key);
    if (!step) return;
    event.preventDefault();
    const index = modes.findIndex((m) => m.id === mode);
    const next = modes[(index + step + modes.length) % modes.length].id;
    pick(next);
    document.getElementById(`use-tab-${next}`)?.focus();
  };

  return (
    <>
      <h2 {...stylex.props(homeSectionStyles.title)}>Use it your way.</h2>
      <div {...stylex.props(styles.layout)}>
        <div
          role="tablist"
          aria-label="Ways to use Skill Studio"
          onKeyDown={onTabKey}
          {...stylex.props(styles.tabs)}
        >
          {modes.map(({ id, label, icon: Icon, blurb }) => (
            <button
              key={id}
              type="button"
              role="tab"
              id={`use-tab-${id}`}
              aria-selected={mode === id}
              aria-controls="use-panel"
              tabIndex={mode === id ? 0 : -1}
              onClick={() => pick(id)}
              {...stylex.props(styles.tab, mode === id && styles.tabOn)}
            >
              <span {...stylex.props(styles.tabIcon, mode === id && styles.tabIconOn)}>
                <Icon size={18} aria-hidden="true" />
              </span>
              <span {...stylex.props(styles.tabText)}>
                <span {...stylex.props(styles.tabLabel)}>{label}</span>
                <span {...stylex.props(styles.tabBlurb)}>{blurb}</span>
              </span>
            </button>
          ))}
        </div>

        <div
          id="use-panel"
          role="tabpanel"
          aria-labelledby={`use-tab-${mode}`}
          {...stylex.props(styles.panel)}
        >
          <div data-surface="dark" {...stylex.props(styles.window)}>
            <div {...stylex.props(styles.titlebar)}>
              <span {...stylex.props(styles.lights)} aria-hidden="true">
                <span {...stylex.props(styles.light)} />
                <span {...stylex.props(styles.light)} />
                <span {...stylex.props(styles.light)} />
              </span>
              <span {...stylex.props(styles.windowTitle)}>{active.title}</span>
              <button
                ref={replayRef}
                type="button"
                onClick={() => setRunId((r) => r + 1)}
                {...stylex.props(styles.replay)}
              >
                <RotateCcw size={13} aria-hidden="true" />
                Replay
              </button>
            </div>
            {mode === "app" ? (
              <AppView
                key={`app-${runId}`}
                runId={runId}
                announce={setStatus}
                undoFocusRef={replayRef}
              />
            ) : (
              <ScriptView key={mode} lines={scripts[mode]} runId={runId} />
            )}
          </div>
          <p role="status" {...stylex.props(styles.visuallyHidden)}>
            {status}
          </p>

          <div {...stylex.props(styles.footer)}>
            {mode === "app" ? (
              <a href={DOWNLOAD_URL} {...stylex.props(homeSectionStyles.textLink)}>
                Download for macOS
                <span aria-hidden="true">→</span>
              </a>
            ) : (
              <>
                <CopyCommand key={mode} text={commands[mode].text} />
                <a href={commands[mode].docs} {...stylex.props(homeSectionStyles.textLink)}>
                  {mode === "cli" ? "CLI docs" : "MCP docs"}
                  <span aria-hidden="true">→</span>
                </a>
              </>
            )}
          </div>
        </div>
      </div>
    </>
  );
}

const settle = "cubic-bezier(0.32, 0.72, 0, 1)";
const blink = stylex.keyframes({ "50%": { opacity: 0 } });

const styles = stylex.create({
  layout: {
    display: "grid",
    gap: 24,
    gridTemplateColumns: "260px minmax(0,1fr)",
    "@media (max-width: 860px)": { gridTemplateColumns: "1fr" },
  },
  tabs: {
    display: "flex",
    flexDirection: "column",
    gap: 6,
    "@media (max-width: 860px)": { display: "grid", gridTemplateColumns: "repeat(3, 1fr)" },
  },
  tab: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: "transparent",
    borderRadius: 12,
    borderStyle: "solid",
    borderWidth: 1,
    color: siteTokens.muted,
    cursor: "pointer",
    display: "flex",
    fontFamily: "inherit",
    gap: 12,
    minHeight: 64,
    padding: "10px 12px",
    textAlign: "left",
    transition:
      "background-color 150ms ease-out, border-color 150ms ease-out, color 150ms ease-out",
    ":hover": { backgroundColor: siteTokens.surface, color: siteTokens.text },
    ":focus-visible": { outline: `2px solid ${siteTokens.text}`, outlineOffset: 2 },
    "@media (max-width: 860px)": { justifyContent: "center", minHeight: 48 },
  },
  tabOn: {
    backgroundColor: siteTokens.surface,
    borderColor: siteTokens.border,
    color: siteTokens.text,
  },
  tabIcon: {
    alignItems: "center",
    borderRadius: 9,
    boxShadow: `inset 0 0 0 1px ${siteTokens.border}`,
    display: "flex",
    flexShrink: 0,
    height: 36,
    justifyContent: "center",
    transition: "background-color 150ms ease-out, color 150ms ease-out",
    width: 36,
    "@media (max-width: 860px)": { height: 28, width: 28 },
  },
  tabIconOn: {
    backgroundColor: siteTokens.accent,
    boxShadow: "none",
    color: siteTokens.accentText,
  },
  tabText: { display: "flex", flexDirection: "column", gap: 2 },
  tabLabel: { fontSize: 15, fontWeight: 640 },
  tabBlurb: { fontSize: 13, lineHeight: 1.35, "@media (max-width: 860px)": { display: "none" } },
  panel: { display: "flex", flexDirection: "column", gap: 16, minWidth: 0 },
  window: {
    backgroundColor: "oklch(0.16 0.02 290)",
    borderColor: "oklch(1 0 0 / .1)",
    borderRadius: 14,
    borderStyle: "solid",
    borderWidth: 1,
    boxShadow: "0 1px 2px oklch(0 0 0 / .2), 0 24px 60px oklch(0 0 0 / .28)",
    color: "oklch(0.93 0.01 290)",
    overflow: "hidden",
  },
  titlebar: {
    alignItems: "center",
    borderBottomColor: "oklch(1 0 0 / .08)",
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    display: "grid",
    gridTemplateColumns: "1fr auto 1fr",
    height: 40,
    paddingInline: 12,
  },
  lights: { display: "flex", gap: 7 },
  light: { backgroundColor: "oklch(1 0 0 / .16)", borderRadius: 999, height: 11, width: 11 },
  windowTitle: { color: "oklch(0.75 0.02 290)", fontSize: 12 },
  replay: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderRadius: 7,
    borderWidth: 0,
    color: "oklch(0.75 0.02 290)",
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: "inherit",
    fontSize: 12,
    gap: 6,
    justifySelf: "end",
    minHeight: 32,
    paddingInline: 10,
    transition: "background-color 150ms ease-out, color 150ms ease-out",
    ":hover": { backgroundColor: "oklch(1 0 0 / .08)", color: "oklch(0.97 0 0)" },
    ":active": { transform: "scale(.96)" },
    ":focus-visible": { outline: "2px solid oklch(0.97 0 0)", outlineOffset: 1 },
  },
  screen: {
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: 13,
    height: 340,
    lineHeight: 1.75,
    margin: 0,
    overflowX: "auto",
    padding: "16px 20px",
    "@media (max-width: 600px)": { fontSize: 11.5, height: 320, padding: "12px 14px" },
  },
  line: { whiteSpace: { default: "pre", "@media (max-width: 600px)": "pre-wrap" } },
  plain: { color: "oklch(0.93 0.01 290)" },
  muted: { color: "oklch(0.68 0.02 290)" },
  warn: { color: "oklch(0.82 0.13 75)" },
  user: { color: "oklch(0.97 0 0)", fontWeight: 600 },
  tool: { color: "oklch(0.78 0.12 293)" },
  accent: { color: "oklch(0.84 0.16 84)", fontWeight: 600 },
  caret: {
    backgroundColor: "oklch(0.93 0.01 290)",
    display: "inline-block",
    height: "1.1em",
    marginLeft: 1,
    verticalAlign: "-.2em",
    width: ".55em",
  },
  caretIdle: {
    animationDuration: "1s",
    animationIterationCount: "infinite",
    animationName: blink,
    animationTimingFunction: "steps(1)",
    "@media (prefers-reduced-motion: reduce)": { animationName: "none" },
  },
  app: {
    backgroundColor: siteTokens.surface,
    color: siteTokens.text,
    height: 340,
    padding: "14px 16px",
    position: "relative",
    "@media (max-width: 600px)": { height: 320 },
  },
  appHead: {
    alignItems: "center",
    display: "flex",
    fontSize: 13,
    fontWeight: 620,
    justifyContent: "space-between",
    marginBottom: 8,
  },
  appCount: { color: siteTokens.muted, fontVariantNumeric: "tabular-nums", fontWeight: 500 },
  appList: { listStyle: "none", margin: 0, padding: 0 },
  appRow: {
    alignItems: "center",
    borderBottomColor: siteTokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    display: "grid",
    fontSize: 13,
    gap: 10,
    gridTemplateColumns: "8px minmax(0,1fr) auto auto",
    height: 42,
    transition: "opacity 300ms ease-out",
  },
  appRowParked: { opacity: 0.45 },
  appDot: { borderRadius: 999, height: 8, width: 8 },
  dotUsed: { backgroundColor: "oklch(0.75 0.17 150)" },
  dotIdle: { backgroundColor: "oklch(0.6 0.02 290)" },
  appName: { fontWeight: 560, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  appMeta: { color: siteTokens.muted, fontSize: 12 },
  badge: {
    backgroundColor: siteTokens.accentSoft,
    borderRadius: 6,
    fontSize: 11,
    fontWeight: 600,
    opacity: 0,
    padding: "3px 7px",
    transform: "scale(.9)",
    transition: `opacity 200ms ease-out, transform 300ms ${settle}`,
  },
  badgeOn: { opacity: 1, transform: "scale(1)" },
  visuallyHidden: {
    clip: "rect(0 0 0 0)",
    clipPath: "inset(50%)",
    height: 1,
    margin: 0,
    overflow: "hidden",
    position: "absolute",
    whiteSpace: "nowrap",
    width: 1,
  },
  toast: {
    alignItems: "center",
    backgroundColor: siteTokens.text,
    borderRadius: 10,
    bottom: 16,
    boxShadow: "0 8px 24px oklch(0 0 0 / .25)",
    color: siteTokens.background,
    display: "flex",
    fontSize: 13,
    gap: 14,
    left: "50%",
    opacity: 0,
    padding: "6px 6px 6px 14px",
    pointerEvents: "none",
    position: "absolute",
    transform: "translate(-50%, 12px)",
    transition: `opacity 200ms ease-out, transform 400ms ${settle}`,
    whiteSpace: "nowrap",
  },
  toastOn: { opacity: 1, pointerEvents: "auto", transform: "translate(-50%, 0)" },
  toastUndo: {
    backgroundColor: "transparent",
    borderRadius: 7,
    borderWidth: 0,
    color: "inherit",
    cursor: "pointer",
    fontFamily: "inherit",
    fontSize: 13,
    fontWeight: 680,
    minHeight: 32,
    paddingInline: 10,
    textDecoration: "underline",
    ":active": { transform: "scale(.96)" },
  },
  footer: { alignItems: "center", display: "flex", flexWrap: "wrap", gap: 16 },
  command: {
    alignItems: "center",
    backgroundColor: siteTokens.surface,
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    gap: 4,
    maxWidth: "100%",
    minHeight: 48,
    minWidth: 0,
    paddingLeft: 14,
  },
  commandText: { overflowX: "auto", whiteSpace: "nowrap" },
  copyButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderRadius: 8,
    borderWidth: 0,
    color: siteTokens.muted,
    cursor: "pointer",
    display: "flex",
    flexShrink: 0,
    height: 44,
    justifyContent: "center",
    transition: "color 150ms ease-out",
    width: 44,
    ":hover": { color: siteTokens.text },
    ":active": { transform: "scale(.96)" },
    ":focus-visible": { outline: `2px solid ${siteTokens.text}`, outlineOffset: -4 },
  },
});
