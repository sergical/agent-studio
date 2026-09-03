// ============================================================================
// Skill Studio marketing demo - interactive preview of the desktop product
// ============================================================================

import { useMemo, useState, type MouseEvent } from "react";
import * as stylex from "@stylexjs/stylex";
import {
  Activity,
  ArrowLeft,
  BookOpen,
  ChevronDown,
  ChevronRight,
  Download,
  Folder,
  LayoutDashboard,
  MoreHorizontal,
  Moon,
  PanelRight,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Settings,
  SlidersHorizontal,
  Sun,
  X,
} from "lucide-react";

import { AgentIcon, type AgentId } from "./AgentIcon";
import { productDemoLightTheme, productDemoTokens as tokens } from "./ProductDemoTokens.stylex";
import type { SiteTheme } from "./SiteTheme.stylex";

export interface ThemeToggleOrigin {
  x: number;
  y: number;
}

interface ProductMockProps {
  theme: SiteTheme;
  onToggleTheme: (origin: ThemeToggleOrigin) => void;
  initialView?: RootView;
  initialSkillName?: string;
  initialInstall?: boolean;
  variant?: "hero" | "walkthrough" | "capture";
}

type RootView = "home" | "skills" | "plugins" | "activity";
type Scope = "All" | "Global" | "Project";
type ActivityWindow = "24h" | "7d" | "14d" | "30d";

interface SkillRow {
  name: string;
  description: string;
  location: "Global" | "agent-studio";
  agents: AgentId[];
  uses: number;
  tokens: string;
  status?: "Update";
}

interface DetailState {
  skill: SkillRow;
  from: RootView;
}

const skills = [
  {
    name: "backport-pr",
    description: "Backport a merged PR to a maintenance branch.",
    location: "Global",
    agents: ["claude"],
    uses: 0,
    tokens: "2.7k",
    status: "Update",
  },
  {
    name: "commit",
    description: "Create commits that follow repository conventions.",
    location: "Global",
    agents: ["claude", "codex", "opencode"],
    uses: 6,
    tokens: "1.1k",
  },
  {
    name: "convex-hackathon-skill",
    description: "Create an evidence-based hackathon build log.",
    location: "Global",
    agents: ["codex"],
    uses: 0,
    tokens: "1.1k",
  },
  {
    name: "create-press-release",
    description: "Create a structured press release page.",
    location: "Global",
    agents: ["claude", "opencode"],
    uses: 0,
    tokens: "1.7k",
  },
  {
    name: "design-vocabulary",
    description: "Precise language for interface and product design.",
    location: "Global",
    agents: ["claude", "codex"],
    uses: 0,
    tokens: "1.5k",
  },
  {
    name: "matt-code-review",
    description: "Review changes against standards and the spec.",
    location: "Global",
    agents: ["claude", "codex"],
    uses: 1,
    tokens: "1.5k",
  },
  {
    name: "new-branch",
    description: "Create a branch from the latest default branch.",
    location: "Global",
    agents: ["claude", "codex", "opencode"],
    uses: 0,
    tokens: "792",
  },
  {
    name: "pstack-sequence-verifiable-units",
    description: "Break multi-step work into verifiable units.",
    location: "Global",
    agents: ["claude", "codex"],
    uses: 0,
    tokens: "522",
    status: "Update",
  },
  {
    name: "pstack-technical-writing",
    description: "Layered standards for clear technical writing.",
    location: "agent-studio",
    agents: ["claude", "codex"],
    uses: 0,
    tokens: "2.7k",
  },
] satisfies ReadonlyArray<SkillRow>;

const activitySkills = [
  { ...skills[0], name: "cmux-workspace", uses: 28 },
  { ...skills[1], name: "cmux", uses: 25 },
  { ...skills[2], name: "artifact-design", uses: 21 },
  { ...skills[3], name: "codex:adversarial-review", uses: 20 },
  { ...skills[4], name: "design-foundations", uses: 12 },
  { ...skills[5], name: "ui-polish", uses: 12 },
  { ...skills[6], name: "write-discoverable-code", uses: 12 },
  { ...skills[7], name: "iterate-pr", uses: 10 },
  { ...skills[8], name: "sentry-cli", uses: 10 },
] satisfies ReadonlyArray<SkillRow>;

const pluginGroups = [
  {
    name: "codex",
    version: "1.0.6",
    agent: "codex" as const,
    skills: [
      { ...skills[0], name: "codex-cli-runtime", description: "Call the Codex companion runtime." },
      {
        ...skills[1],
        name: "codex-result-handling",
        description: "Present Codex helper output clearly.",
      },
      {
        ...skills[2],
        name: "gpt-5-4-prompting",
        description: "Compose focused coding and review prompts.",
      },
    ],
  },
  {
    name: "figma",
    version: "2.1.7",
    agent: "claude" as const,
    skills: [
      {
        ...skills[3],
        name: "figma-code-connect",
        description: "Map Figma components to code snippets.",
      },
      {
        ...skills[4],
        name: "figma-create-design-system-rules",
        description: "Generate project design-system rules.",
      },
      {
        ...skills[5],
        name: "figma-create-new-file",
        description: "Create a new Figma or FigJam file.",
      },
      {
        ...skills[6],
        name: "figma-generate-design",
        description: "Translate application views into Figma.",
      },
    ],
  },
];

const navItems: ReadonlyArray<{
  id: RootView;
  label: string;
  Icon: typeof LayoutDashboard;
  count?: number;
}> = [
  { id: "home", label: "Home", Icon: LayoutDashboard },
  { id: "skills", label: "Skills", Icon: Search, count: 290 },
  { id: "plugins", label: "Plugins", Icon: Puzzle, count: 109 },
  { id: "activity", label: "Activity", Icon: Activity },
];

const viewEnter = stylex.keyframes({
  from: { opacity: 0.35, transform: "translateX(7px)" },
  to: { opacity: 1, transform: "translateX(0)" },
});

function toggleThemeFromButton(
  event: MouseEvent<HTMLButtonElement>,
  onToggleTheme: (origin: ThemeToggleOrigin) => void,
) {
  const bounds = event.currentTarget.getBoundingClientRect();
  onToggleTheme({ x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 });
}

function WindowBar({
  theme,
  onToggleTheme,
}: {
  theme: SiteTheme;
  onToggleTheme: (origin: ThemeToggleOrigin) => void;
}) {
  return (
    <div {...stylex.props(styles.windowBar)}>
      <span {...stylex.props(styles.traffic)} aria-hidden="true">
        <i {...stylex.props(styles.trafficDot)} />
        <i {...stylex.props(styles.trafficDot)} />
        <i {...stylex.props(styles.trafficDot)} />
      </span>
      <span>Skill Studio</span>
      <button
        type="button"
        aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
        onClick={(event) => toggleThemeFromButton(event, onToggleTheme)}
        {...stylex.props(styles.windowThemeButton)}
      >
        {theme === "dark" ? (
          <Moon aria-hidden="true" size={15} />
        ) : (
          <Sun aria-hidden="true" size={15} />
        )}
      </button>
    </div>
  );
}

function Sidebar({
  activeView,
  onNavigate,
  onAddSkill,
  theme,
  onToggleTheme,
}: {
  activeView: RootView;
  onNavigate: (view: RootView) => void;
  onAddSkill: () => void;
  theme: SiteTheme;
  onToggleTheme: (origin: ThemeToggleOrigin) => void;
}) {
  return (
    <aside {...stylex.props(styles.sidebar)}>
      <label {...stylex.props(styles.sidebarSearch)}>
        <Search aria-hidden="true" size={15} />
        <input
          aria-label="Search all skills"
          placeholder="Search skills…"
          {...stylex.props(styles.input)}
        />
      </label>
      <button type="button" onClick={onAddSkill} {...stylex.props(styles.addSkill)}>
        <Plus aria-hidden="true" size={15} />
        Add skill
      </button>
      <nav {...stylex.props(styles.productNav)} aria-label="Product navigation">
        {navItems.map(({ id, label, Icon, count }) => (
          <button
            key={id}
            type="button"
            aria-current={activeView === id ? "page" : undefined}
            onClick={() => onNavigate(id)}
            {...stylex.props(styles.navItem, activeView === id && styles.navItemActive)}
          >
            <Icon aria-hidden="true" size={15} />
            <span>{label}</span>
            {count !== undefined && <small {...stylex.props(styles.navCount)}>{count}</small>}
          </button>
        ))}
      </nav>
      <div {...stylex.props(styles.sidebarFooter)}>
        <span {...stylex.props(styles.sidebarFooterStatus)}>
          <RefreshCw aria-hidden="true" size={12} />
          Scanned just now
        </span>
        <button
          type="button"
          aria-label="Documentation"
          {...stylex.props(styles.sidebarFooterButton)}
        >
          <BookOpen aria-hidden="true" size={13} />
        </button>
        <button type="button" aria-label="Settings" {...stylex.props(styles.sidebarFooterButton)}>
          <Settings aria-hidden="true" size={13} />
        </button>
        <button
          type="button"
          aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
          onClick={(event) => toggleThemeFromButton(event, onToggleTheme)}
          {...stylex.props(styles.sidebarFooterButton)}
        >
          {theme === "dark" ? (
            <Moon aria-hidden="true" size={13} />
          ) : (
            <Sun aria-hidden="true" size={13} />
          )}
        </button>
      </div>
    </aside>
  );
}

function PageHeading({
  title,
  subtitle,
  aside,
}: {
  title: string;
  subtitle?: string;
  aside?: string;
}) {
  return (
    <div {...stylex.props(styles.pageHeading)}>
      <div>
        <h2 {...stylex.props(styles.pageTitle)}>{title}</h2>
        {subtitle && <p {...stylex.props(styles.pageSubtitle)}>{subtitle}</p>}
      </div>
      {aside && <span {...stylex.props(styles.pageCount)}>{aside}</span>}
    </div>
  );
}

function AgentStack({ agents }: { agents: readonly AgentId[] }) {
  return (
    <span {...stylex.props(styles.agentStack)}>
      {agents.map((agent) => (
        <AgentIcon key={agent} agent={agent} size={14} />
      ))}
    </span>
  );
}

function HomeScreen({ onOpenSkill }: { onOpenSkill: (skill: SkillRow) => void }) {
  const attention = [
    {
      label: "Broken",
      count: 5,
      tone: "danger" as const,
      rows: [
        ["motion", "Missing required frontmatter fields"],
        ["prepare-videos", "Missing required frontmatter fields"],
        ["tailwind", "Missing required frontmatter fields"],
        ["writing-skills", "Name does not match its directory"],
      ],
    },
    {
      label: "Warnings",
      count: 34,
      tone: "warning" as const,
      rows: [
        ["agent-browser", "Shared folder needs per-skill links"],
        ["activate-homepage-hero", "Three copies differ"],
        ["add-customer-story", "Three copies differ"],
        ["code-simplifier", "Four copies differ"],
      ],
    },
    {
      label: "Updates",
      count: 22,
      tone: "accent" as const,
      rows: [
        ["agent-browser", "716c069 → 021d925"],
        ["bro", "b8b594d → cdba491"],
        ["cmux", "652d0c8 → 1349bc0"],
        ["cmux-customization", "f9b1835 → 8c8c2ba"],
      ],
    },
  ];
  return (
    <section {...stylex.props(styles.page)}>
      <PageHeading title="Home" />
      <div {...stylex.props(styles.statGrid)}>
        {attention.map((item) => (
          <button key={item.label} type="button" {...stylex.props(styles.statCard)}>
            <span {...stylex.props(styles.cardLabel)}>{item.label}</span>
            <strong {...stylex.props(styles.statNumber, styles[item.tone])}>{item.count}</strong>
          </button>
        ))}
      </div>
      <article {...stylex.props(styles.laneCard)}>
        <div {...stylex.props(styles.laneRow)}>
          <span {...stylex.props(styles.laneTitle)}>
            Who can invoke <b>290</b>
          </span>
          <div {...stylex.props(styles.laneSegments)}>
            <button type="button" {...stylex.props(styles.lanePrimary, styles.laneBoth)}>
              <b>202</b>
              <span {...stylex.props(styles.mobileHidden)}>you or the model</span>
              <span {...stylex.props(styles.mobileOnly)}>both</span>
            </button>
            <button type="button" {...stylex.props(styles.laneSecondary, styles.laneModel)}>
              <b>18</b>
              <span {...stylex.props(styles.mobileHidden)}>model only</span>
              <span {...stylex.props(styles.mobileOnly)}>model</span>
            </button>
            <button type="button" {...stylex.props(styles.laneTertiary, styles.laneUser)}>
              <b>87</b>
              <span {...stylex.props(styles.mobileHidden)}>you only</span>
              <span {...stylex.props(styles.mobileOnly)}>you</span>
            </button>
          </div>
        </div>
        <div {...stylex.props(styles.laneRow)}>
          <span {...stylex.props(styles.laneTitle)}>
            Prompt cost <b>12.5k</b>
          </span>
          <div {...stylex.props(styles.laneSegments)}>
            <button type="button" {...stylex.props(styles.lanePrimary, styles.laneActive)}>
              <b>9.3k</b>
              <span {...stylex.props(styles.mobileHidden)}>· 214 skills used in 30 days</span>
              <span {...stylex.props(styles.mobileOnly)}>active</span>
            </button>
            <button type="button" {...stylex.props(styles.laneTertiary, styles.laneIdle)}>
              <b>3.2k</b>
              <span {...stylex.props(styles.mobileHidden)}>· 76 skills not used</span>
              <span {...stylex.props(styles.mobileOnly)}>idle</span>
            </button>
          </div>
        </div>
      </article>
      <div {...stylex.props(styles.inbox)}>
        {attention.map((item, index) => (
          <section key={item.label}>
            <button type="button" {...stylex.props(styles.sectionLabel)}>
              <ChevronDown aria-hidden="true" size={14} />
              {item.label}
              <span>{item.count}</span>
            </button>
            {item.rows.map(([name, detail], rowIndex) => (
              <button
                key={name}
                type="button"
                onClick={() => onOpenSkill(skills[(index * 3 + rowIndex) % skills.length])}
                {...stylex.props(styles.inboxRow)}
              >
                <i {...stylex.props(styles.statusDot, styles[`${item.tone}Background`])} />
                <span {...stylex.props(styles.inboxCopy)}>
                  <span {...stylex.props(styles.inboxName)}>
                    <strong>{name}</strong>
                    <AgentStack agents={skills[(index * 3 + rowIndex) % skills.length].agents} />
                  </span>
                  <small {...stylex.props(styles.inboxDetail)}>{detail}</small>
                </span>
                <span {...stylex.props(styles.rowAction)}>
                  {item.label === "Updates"
                    ? "Pull latest"
                    : item.label === "Warnings"
                      ? "Compare"
                      : "Open"}
                </span>
              </button>
            ))}
          </section>
        ))}
      </div>
    </section>
  );
}

function SkillsScreen({ onOpenSkill }: { onOpenSkill: (skill: SkillRow) => void }) {
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<Scope>("All");
  const visibleSkills = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return skills.filter(
      (skill) =>
        (!normalized ||
          skill.name.includes(normalized) ||
          skill.description.toLowerCase().includes(normalized)) &&
        (scope === "All" ||
          (scope === "Global" ? skill.location === "Global" : skill.location !== "Global")),
    );
  }, [query, scope]);
  return (
    <section {...stylex.props(styles.page)}>
      <PageHeading title="Skills" aside="290 skills" />
      <div {...stylex.props(styles.toolbar)}>
        <label {...stylex.props(styles.filterInput)}>
          <Search aria-hidden="true" size={13} />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label="Filter skills"
            placeholder="Filter skills…"
            {...stylex.props(styles.input)}
          />
        </label>
        <div {...stylex.props(styles.segmented)} role="group" aria-label="Skill scope">
          {(["All", "Global", "Project"] satisfies Scope[]).map((item) => (
            <button
              key={item}
              type="button"
              aria-pressed={scope === item}
              onClick={() => setScope(item)}
              {...stylex.props(styles.segmentButton, scope === item && styles.segmentButtonActive)}
            >
              {item}
            </button>
          ))}
        </div>
        <button type="button" {...stylex.props(styles.filterButton)}>
          <SlidersHorizontal aria-hidden="true" size={12} />
          Filter
        </button>
      </div>
      <div {...stylex.props(styles.tableHeader)}>
        <span>Name</span>
        <span {...stylex.props(styles.mobileHidden)}>Description</span>
        <span {...stylex.props(styles.mobileHidden)}>Location</span>
        <span {...stylex.props(styles.mobileHidden)}>Uses</span>
        <span>Tokens</span>
      </div>
      <div {...stylex.props(styles.rows)}>
        {visibleSkills.map((skill) => (
          <SkillTableRow key={skill.name} skill={skill} onClick={() => onOpenSkill(skill)} />
        ))}
        {visibleSkills.length === 0 && (
          <p {...stylex.props(styles.empty)}>No skills match this filter.</p>
        )}
      </div>
    </section>
  );
}

function SkillTableRow({ skill, onClick }: { skill: SkillRow; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} {...stylex.props(styles.skillRow)}>
      <span {...stylex.props(styles.skillName)}>
        {skill.name}
        {skill.status && <small {...stylex.props(styles.updateBadge)}>{skill.status}</small>}
      </span>
      <span {...stylex.props(styles.truncate, styles.mobileHidden)}>{skill.description}</span>
      <span {...stylex.props(styles.location, styles.mobileHidden)}>
        <AgentStack agents={skill.agents} />
        {skill.location}
      </span>
      <span {...stylex.props(styles.number, styles.mobileHidden)}>{skill.uses}</span>
      <span {...stylex.props(styles.number)}>{skill.tokens}</span>
    </button>
  );
}

function PluginsScreen({ onOpenSkill }: { onOpenSkill: (skill: SkillRow) => void }) {
  return (
    <section {...stylex.props(styles.page)}>
      <PageHeading
        title="Plugin skills"
        subtitle="Shipped inside agent plugins and updated with them — managed by the plugin, not by Skill Studio."
      />
      <div {...stylex.props(styles.pluginGroups)}>
        {pluginGroups.map((group) => (
          <article key={group.name} {...stylex.props(styles.pluginGroup)}>
            <header {...stylex.props(styles.pluginHeader)}>
              <span {...stylex.props(styles.pluginTitle)}>
                <AgentIcon agent={group.agent} size={14} />
                <strong>{group.name}</strong>
              </span>
              <small>v{group.version}</small>
            </header>
            {group.skills.map((skill) => (
              <button
                key={skill.name}
                type="button"
                onClick={() => onOpenSkill(skill)}
                {...stylex.props(styles.pluginRow)}
              >
                <span {...stylex.props(styles.pluginCopy)}>
                  <strong {...stylex.props(styles.pluginName)}>{skill.name}</strong>
                  <small {...stylex.props(styles.truncate, styles.pluginDescription)}>
                    {skill.description}
                  </small>
                </span>
                <span {...stylex.props(styles.number)}>{skill.tokens}</span>
                <ChevronRight aria-hidden="true" size={14} />
              </button>
            ))}
          </article>
        ))}
      </div>
    </section>
  );
}

const heat = Array.from({ length: 364 }, (_, index) => {
  if (index < 310 && index % 47 !== 0) return 0;
  return ((index * 7) % 5) as 0 | 1 | 2 | 3 | 4;
});

function ActivityScreen({ onOpenSkill }: { onOpenSkill: (skill: SkillRow) => void }) {
  const [window, setWindow] = useState<ActivityWindow>("30d");
  return (
    <section {...stylex.props(styles.page)}>
      <PageHeading
        title="Activity"
        subtitle="From Claude Code transcripts. Codex, OpenCode and pi are not tracked yet."
      />
      <article {...stylex.props(styles.activityCard)}>
        <div {...stylex.props(styles.activityTop)}>
          <span {...stylex.props(styles.activityLabel)}>Activity</span>
          <span {...stylex.props(styles.activityTotal)}>293 invocations in the last year</span>
        </div>
        <div {...stylex.props(styles.heatmapMonths)} aria-hidden="true">
          {[
            [1, 4, "Oct"],
            [5, 4, "Nov"],
            [9, 5, "Dec"],
            [14, 4, "Jan"],
            [18, 4, "Feb"],
            [22, 5, "Mar"],
            [27, 4, "Apr"],
            [31, 5, "May"],
            [36, 4, "Jun"],
            [40, 5, "Jul"],
            [45, 4, "Aug"],
            [49, 4, "Sep"],
          ].map(([start, span, label]) => (
            <span key={label} style={{ gridColumn: `${start} / span ${span}` }}>
              {label}
            </span>
          ))}
        </div>
        <div {...stylex.props(styles.heatmapBody)}>
          <div {...stylex.props(styles.weekdays)} aria-hidden="true">
            <span>Mon</span>
            <span />
            <span>Wed</span>
            <span />
            <span>Fri</span>
            <span />
            <span />
          </div>
          <div aria-label="Invocation activity for the last year" {...stylex.props(styles.heatmap)}>
            {heat.map((level, index) => (
              <i
                key={index}
                title={`${level} invocations`}
                {...stylex.props(
                  styles.heatCell,
                  level === 1 && styles.heat1,
                  level === 2 && styles.heat2,
                  level === 3 && styles.heat3,
                  level === 4 && styles.heat4,
                )}
              />
            ))}
          </div>
        </div>
      </article>
      <div {...stylex.props(styles.activityTable)}>
        <div {...stylex.props(styles.activityTableTop)}>
          <span {...stylex.props(styles.activityLabel)}>By skill</span>
          <div {...stylex.props(styles.activitySegmented)}>
            {(["24h", "7d", "14d", "30d"] satisfies ActivityWindow[]).map((item) => (
              <button
                key={item}
                type="button"
                aria-pressed={window === item}
                onClick={() => setWindow(item)}
                {...stylex.props(
                  styles.activitySegmentButton,
                  window === item && styles.segmentButtonActive,
                )}
              >
                {item}
              </button>
            ))}
          </div>
        </div>
        <div {...stylex.props(styles.activityHeader)}>
          <span>Skill</span>
          <span {...stylex.props(styles.mobileHidden)}>Last used</span>
          <span>Invocations</span>
          <span {...stylex.props(styles.mobileHidden)}>Projects</span>
        </div>
        {activitySkills.map((skill, index) => (
          <button
            key={skill.name}
            type="button"
            onClick={() => onOpenSkill(skill)}
            {...stylex.props(styles.activityRow)}
          >
            <span {...stylex.props(styles.activitySkill)}>{skill.name}</span>
            <span {...stylex.props(styles.mobileHidden)}>
              {index < 2 ? "5h ago" : `${index + 1}d ago`}
            </span>
            <span>{skill.uses}</span>
            <span {...stylex.props(styles.mobileHidden)}>{Math.max(3, 19 - index * 2)}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

const installAgents = ["codex", "opencode", "pi", "cursor", "grok"] satisfies AgentId[];
const installScopes = ["Global", "Project"] satisfies ReadonlyArray<"Global" | "Project">;

function AddSkillDrawer({ onClose }: { onClose: () => void }) {
  const [source, setSource] = useState("");
  const [scope, setScope] = useState<"Global" | "Project">("Global");
  const [selectedAgents, setSelectedAgents] = useState<AgentId[]>([
    "claude",
    "codex",
    "opencode",
    "pi",
  ]);

  const toggleAgent = (agent: AgentId) => {
    setSelectedAgents((current) =>
      current.includes(agent) ? current.filter((item) => item !== agent) : [...current, agent],
    );
  };

  return (
    <div role="dialog" aria-label="Add skill" {...stylex.props(styles.addSkillDrawer)}>
      <header {...stylex.props(styles.addSkillHeader)}>
        <h2 {...stylex.props(styles.addSkillTitle)}>Add skill</h2>
        <button
          type="button"
          aria-label="Close add skill"
          onClick={onClose}
          {...stylex.props(styles.drawerClose)}
        >
          <X aria-hidden="true" size={15} />
        </button>
      </header>
      <div {...stylex.props(styles.addSkillTabs)} role="tablist" aria-label="Add skill method">
        <button
          type="button"
          role="tab"
          aria-selected="true"
          {...stylex.props(styles.addSkillTab, styles.addSkillTabActive)}
        >
          Add by source
        </button>
        <button
          type="button"
          role="tab"
          aria-selected="false"
          {...stylex.props(styles.addSkillTab)}
        >
          Browse skills.sh
        </button>
      </div>
      <div {...stylex.props(styles.addSkillBody)}>
        <label {...stylex.props(styles.addSkillField)}>
          <span {...stylex.props(styles.cardLabel)}>Source</span>
          <input
            value={source}
            onChange={(event) => setSource(event.target.value)}
            aria-label="Skill source"
            placeholder="owner/repo, a GitHub URL, a skills.sh URL, or a local path"
            {...stylex.props(styles.addSkillInput)}
          />
          <small {...stylex.props(styles.addSkillHelp)}>
            {source ? `github · ${source}` : "Paste a repo, URL, or path to get started."}
          </small>
        </label>

        <div {...stylex.props(styles.installGroup)}>
          <span {...stylex.props(styles.cardLabel)}>Method</span>
          <div {...stylex.props(styles.methodPicker)} role="group" aria-label="Install method">
            <button
              type="button"
              aria-pressed="true"
              {...stylex.props(styles.methodButton, styles.segmentButtonActive)}
            >
              dotagents
            </button>
            <button type="button" aria-pressed="false" {...stylex.props(styles.methodButton)}>
              skills.sh
            </button>
            <button type="button" aria-pressed="false" {...stylex.props(styles.methodButton)}>
              Copy
            </button>
          </div>
        </div>

        <div {...stylex.props(styles.installControls)}>
          <div {...stylex.props(styles.installGroup)}>
            <span {...stylex.props(styles.cardLabel)}>Scope</span>
            <div {...stylex.props(styles.installScope)} role="group" aria-label="Install scope">
              {installScopes.map((item) => (
                <button
                  key={item}
                  type="button"
                  aria-pressed={scope === item}
                  onClick={() => setScope(item)}
                  {...stylex.props(
                    styles.installScopeButton,
                    scope === item && styles.segmentButtonActive,
                  )}
                >
                  {item}
                </button>
              ))}
            </div>
          </div>

          <div {...stylex.props(styles.installGroup)}>
            <span {...stylex.props(styles.cardLabel)}>Harnesses</span>
            <div {...stylex.props(styles.installTargetRow)}>
              <span {...stylex.props(styles.installTargetIdentity)}>
                <Folder aria-hidden="true" size={16} />
                <span {...stylex.props(styles.installTargetCopy)}>
                  <strong {...stylex.props(styles.installTargetName)}>Shared folder</strong>
                  <small {...stylex.props(styles.installTargetPath)}>
                    {scope === "Global" ? "~/.agents/skills" : ".agents/skills"}
                  </small>
                </span>
              </span>
              <AgentStack
                agents={installAgents.filter((agent) => selectedAgents.includes(agent))}
              />
              <button
                type="button"
                role="switch"
                aria-label="Install to the shared folder"
                aria-checked={selectedAgents.includes("codex")}
                onClick={() => toggleAgent("codex")}
                {...stylex.props(
                  styles.installSwitch,
                  selectedAgents.includes("codex") && styles.installSwitchActive,
                )}
              >
                <i
                  {...stylex.props(
                    styles.installSwitchKnob,
                    selectedAgents.includes("codex") && styles.installSwitchKnobActive,
                  )}
                />
              </button>
            </div>
            <div {...stylex.props(styles.installTargetRow)}>
              <span {...stylex.props(styles.installTargetIdentity)}>
                <AgentIcon agent="claude" size={16} />
                <span {...stylex.props(styles.installTargetCopy)}>
                  <strong {...stylex.props(styles.installTargetName)}>Claude Code</strong>
                  <small {...stylex.props(styles.installTargetPath)}>
                    {scope === "Global" ? "~/.claude/skills" : ".claude/skills"}
                  </small>
                </span>
              </span>
              <button
                type="button"
                role="switch"
                aria-label="Install for Claude Code"
                aria-checked={selectedAgents.includes("claude")}
                onClick={() => toggleAgent("claude")}
                {...stylex.props(
                  styles.installSwitch,
                  selectedAgents.includes("claude") && styles.installSwitchActive,
                )}
              >
                <i
                  {...stylex.props(
                    styles.installSwitchKnob,
                    selectedAgents.includes("claude") && styles.installSwitchKnobActive,
                  )}
                />
              </button>
            </div>
          </div>

          <label {...stylex.props(styles.trialOption)}>
            <input type="checkbox" {...stylex.props(styles.trialCheckbox)} />
            <span {...stylex.props(styles.trialCopy)}>
              Try for 24 hours
              <small {...stylex.props(styles.trialHelp)}>
                Removed automatically after 24 h unless you keep it.
              </small>
            </span>
          </label>
        </div>
      </div>
      <footer {...stylex.props(styles.addSkillFooter)}>
        <button type="button" onClick={onClose} {...stylex.props(styles.cancelAction)}>
          Cancel
        </button>
        <button type="button" disabled={!source} {...stylex.props(styles.installAction)}>
          Add skill
        </button>
      </footer>
    </div>
  );
}

function DetailScreen({ detail, onBack }: { detail: DetailState; onBack: () => void }) {
  const [invocation, setInvocation] = useState("Both");
  const backLabel =
    detail.from === "home" ? "Home" : detail.from === "activity" ? "Activity" : "Skills";
  return (
    <section {...stylex.props(styles.page)}>
      <header {...stylex.props(styles.detailHeader)}>
        <div {...stylex.props(styles.detailHeading)}>
          <button type="button" onClick={onBack} {...stylex.props(styles.backButton)}>
            <ArrowLeft aria-hidden="true" size={16} />
            <span {...stylex.props(styles.backButtonLabel)}>{backLabel}</span>
          </button>
          <h2 {...stylex.props(styles.pageTitle, styles.detailTitle)}>{detail.skill.name}</h2>
          <div {...stylex.props(styles.detailActions)}>
            <button type="button" {...stylex.props(styles.updateButton)}>
              <Download aria-hidden="true" size={14} />
              Update
            </button>
            <button type="button" {...stylex.props(styles.secondaryButton)}>
              <PanelRight aria-hidden="true" size={16} />
              <span {...stylex.props(styles.assistantLabel)}>Assistant</span>
            </button>
            <button type="button" aria-label="More actions" {...stylex.props(styles.iconButton)}>
              <MoreHorizontal aria-hidden="true" size={16} />
            </button>
          </div>
        </div>
        <p {...stylex.props(styles.detailDescription)}>{detail.skill.description}</p>
        <div {...stylex.props(styles.chips)}>
          <span {...stylex.props(styles.chip)}>dotagents</span>
          {detail.skill.status && (
            <span {...stylex.props(styles.chip, styles.accent)}>Update available</span>
          )}
        </div>
        <p {...stylex.props(styles.metadata)}>
          18.4 KB · {detail.skill.tokens} tokens · {detail.skill.uses} uses in 30 days · edited
          today · source dotagents
        </p>
      </header>

      <div {...stylex.props(styles.detailStack)}>
        <article {...stylex.props(styles.detailCard)}>
          <div {...stylex.props(styles.cardHeading)}>
            <h3 {...stylex.props(styles.cardTitle)}>Locations</h3>
          </div>
          <div {...stylex.props(styles.scopeRow)}>
            <button type="button" {...stylex.props(styles.scopeMain)}>
              <span {...stylex.props(styles.scopeChevron)}>
                <ChevronRight aria-hidden="true" size={14} />
              </span>
              <span {...stylex.props(styles.scopeIdentity)}>
                <Folder aria-hidden="true" size={16} />
                <span {...stylex.props(styles.scopeName)}>Global</span>
              </span>
              <span {...stylex.props(styles.readerStack)}>
                {detail.skill.agents.map((agent, index) => (
                  <span
                    key={agent}
                    {...stylex.props(styles.readerBadge, index > 0 && styles.readerBadgeOverlap)}
                  >
                    <AgentIcon agent={agent} size={12} />
                  </span>
                ))}
              </span>
            </button>
            <span {...stylex.props(styles.locationControls)}>
              <span {...stylex.props(styles.switchTrack)}>
                <i {...stylex.props(styles.switchKnob)} />
              </span>
              <button type="button" aria-label="Location actions" {...stylex.props(styles.rowMenu)}>
                <MoreHorizontal aria-hidden="true" size={14} />
              </button>
            </span>
          </div>
          <div {...stylex.props(styles.invocation)}>
            <span {...stylex.props(styles.invocationLabel)}>Invocation</span>
            <div {...stylex.props(styles.invocationRow)}>
              <Folder aria-hidden="true" size={16} />
              <span {...stylex.props(styles.invocationFile)}>
                <b>Global folder</b>
              </span>
              <div
                role="group"
                aria-label="Invocation policy"
                {...stylex.props(styles.invocationSegments)}
              >
                {["Both", "User only", "Model only"].map((label) => (
                  <button
                    key={label}
                    type="button"
                    aria-pressed={invocation === label}
                    onClick={() => setInvocation(label)}
                    {...stylex.props(
                      styles.invocationButton,
                      invocation === label && styles.segmentButtonActive,
                    )}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <p {...stylex.props(styles.invocationNote)}>
              Both: you can call /{detail.skill.name} and the model can pick it.
            </p>
          </div>
        </article>

        <article {...stylex.props(styles.detailCard)}>
          <div {...stylex.props(styles.cardHeading)}>
            <h3 {...stylex.props(styles.cardTitle)}>SKILL.md</h3>
            <span {...stylex.props(styles.markdownActions)}>
              <small>Global</small>
              <button type="button" {...stylex.props(styles.editButton)}>
                Edit
              </button>
            </span>
          </div>
          <div {...stylex.props(styles.markdown)}>
            <h4 {...stylex.props(styles.markdownTitle)}>{detail.skill.name}</h4>
            <p {...stylex.props(styles.markdownParagraph)}>{detail.skill.description}</p>
            <h5 {...stylex.props(styles.markdownHeading)}>When to use this skill</h5>
            <p {...stylex.props(styles.markdownParagraph)}>
              Use it when the work needs a repeatable workflow, clear constraints, and a result that
              every coding agent can understand.
            </p>
            <h5 {...stylex.props(styles.markdownHeading)}>Core principles</h5>
            <ul {...stylex.props(styles.markdownList)}>
              <li>Keep the instructions focused.</li>
              <li>Verify the result in the real workspace.</li>
            </ul>
          </div>
        </article>
      </div>
    </section>
  );
}

export function ProductMock({
  theme,
  onToggleTheme,
  initialView = "skills",
  initialSkillName,
  initialInstall = false,
  variant = "hero",
}: ProductMockProps) {
  const initialSkill = skills.find((skill) => skill.name === initialSkillName);
  const [activeView, setActiveView] = useState<RootView>(initialView);
  const [detail, setDetail] = useState<DetailState | null>(
    initialSkill ? { skill: initialSkill, from: initialView } : null,
  );
  const [installOpen, setInstallOpen] = useState(initialInstall);
  const navigate = (view: RootView) => {
    setActiveView(view);
    setDetail(null);
    setInstallOpen(false);
  };
  const openSkill = (skill: SkillRow) => {
    setDetail({ skill, from: activeView });
    setInstallOpen(false);
  };
  return (
    <div
      role="region"
      aria-label="Interactive Skill Studio product preview"
      {...stylex.props(
        styles.window,
        variant === "walkthrough" && styles.walkthroughWindow,
        variant === "capture" && styles.captureWindow,
        theme === "light" && productDemoLightTheme,
      )}
    >
      <WindowBar theme={theme} onToggleTheme={onToggleTheme} />
      <div {...stylex.props(styles.app)}>
        <Sidebar
          activeView={activeView}
          onNavigate={navigate}
          onAddSkill={() => {
            setInstallOpen(true);
            setDetail(null);
          }}
          theme={theme}
          onToggleTheme={onToggleTheme}
        />
        <div
          key={detail ? `detail-${detail.skill.name}` : activeView}
          {...stylex.props(styles.view)}
        >
          {detail ? (
            <DetailScreen detail={detail} onBack={() => setDetail(null)} />
          ) : activeView === "home" ? (
            <HomeScreen onOpenSkill={openSkill} />
          ) : activeView === "skills" ? (
            <SkillsScreen onOpenSkill={openSkill} />
          ) : activeView === "plugins" ? (
            <PluginsScreen onOpenSkill={openSkill} />
          ) : (
            <ActivityScreen onOpenSkill={openSkill} />
          )}
        </div>
      </div>
      {installOpen && <AddSkillDrawer onClose={() => setInstallOpen(false)} />}
    </div>
  );
}

const tableColumns = "minmax(0,1.2fr) minmax(0,1.8fr) 140px 48px 64px";

const styles = stylex.create({
  window: {
    backgroundColor: tokens.background,
    borderColor: tokens.border,
    borderRadius: 8,
    borderStyle: "solid",
    borderWidth: 1,
    boxShadow: "0 2px 5px oklch(0 0 0 / .22), 0 20px 55px oklch(0 0 0 / .34)",
    color: tokens.text,
    fontFamily: "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif",
    height: 800,
    isolation: "isolate",
    overflow: "hidden",
    position: "relative",
    transform: "scale(.678)",
    transformOrigin: "top left",
    width: "147.493%",
    "@media (max-width: 680px)": {
      borderRadius: "min(3vw, 10px)",
      height: 520,
      transform: "none",
      width: "100%",
    },
  },
  walkthroughWindow: {
    height: 800,
    transform: "scale(.57)",
    width: "175.439%",
    "@media (max-width: 680px)": {
      height: 500,
      transform: "none",
      width: "100%",
    },
  },
  captureWindow: {
    height: 800,
    transform: "scale(1.14)",
    width: "87.719%",
  },
  windowBar: {
    alignItems: "center",
    backgroundColor: tokens.surface,
    borderBottomColor: tokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    color: tokens.muted,
    display: "grid",
    fontSize: 10,
    gridTemplateColumns: "auto 1fr auto",
    gap: 7,
    height: 22,
    paddingInline: 7,
    "@media (max-width: 680px)": { height: 40, paddingInline: 10 },
  },
  windowThemeButton: {
    display: "none",
    "@media (max-width: 680px)": {
      alignItems: "center",
      backgroundColor: "transparent",
      borderColor: tokens.border,
      borderRadius: 5,
      borderStyle: "solid",
      borderWidth: 1,
      color: tokens.muted,
      display: "flex",
      height: 30,
      justifyContent: "center",
      padding: 0,
      width: 30,
    },
    ":active": { transform: "scale(.94)" },
  },
  traffic: { display: "flex", gap: 5 },
  trafficDot: {
    backgroundColor: tokens.border,
    borderRadius: "50%",
    display: "block",
    height: 6,
    width: 6,
  },
  scanned: { fontVariantNumeric: "tabular-nums", justifySelf: "end" },
  app: {
    display: "grid",
    gridTemplateColumns: "240px minmax(0,1fr)",
    height: 778,
    "@media (max-width: 680px)": {
      display: "flex",
      flexDirection: "column",
      height: 480,
    },
  },
  sidebar: {
    backgroundColor: tokens.surface,
    borderRightColor: tokens.border,
    borderRightStyle: "solid",
    borderRightWidth: 1,
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    padding: 10,
    "@media (max-width: 680px)": {
      borderBottomColor: tokens.border,
      borderBottomStyle: "solid",
      borderBottomWidth: 1,
      borderRightWidth: 0,
      gap: 4,
      height: 56,
      padding: 6,
      width: "100%",
    },
  },
  sidebarSearch: {
    alignItems: "center",
    backgroundColor: tokens.background,
    borderColor: tokens.border,
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.faint,
    display: "flex",
    gap: 7,
    height: 32,
    paddingInline: 12,
    ":focus-within": { borderColor: tokens.accent },
    "@media (max-width: 680px)": { display: "none" },
  },
  input: {
    backgroundColor: "transparent",
    borderWidth: 0,
    color: tokens.text,
    fontSize: 13,
    minWidth: 0,
    outline: "none",
    padding: 0,
    width: "100%",
    "@media (max-width: 680px)": { fontSize: 15 },
  },
  addSkill: {
    alignItems: "center",
    backgroundColor: tokens.accentSoft,
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.text,
    display: "flex",
    fontSize: 13,
    fontWeight: 500,
    gap: 6,
    height: 30,
    justifyContent: "center",
    marginBlock: "6px 10px",
    transition: "background-color 150ms ease-out, transform 150ms ease-out",
    ":hover": { backgroundColor: tokens.accentHover },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 680px)": { display: "none" },
  },
  productNav: {
    display: "flex",
    flexDirection: "column",
    gap: 1,
    "@media (max-width: 680px)": { flex: 1, flexDirection: "row", gap: 4, minWidth: 0 },
  },
  navItem: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.muted,
    display: "grid",
    fontSize: 13,
    gap: 8,
    gridTemplateColumns: "15px 1fr auto",
    height: 30,
    paddingInline: 10,
    textAlign: "left",
    transition: "background-color 150ms ease-out, color 150ms ease-out",
    ":hover": { backgroundColor: tokens.hover, color: tokens.text },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 680px)": {
      display: "flex",
      flex: 1,
      fontSize: 12,
      gap: 5,
      height: 44,
      justifyContent: "center",
      paddingInline: 4,
    },
  },
  navCount: { "@media (max-width: 680px)": { display: "none" } },
  navItemActive: { backgroundColor: tokens.accentSoft, color: tokens.text },
  sidebarFooter: {
    alignItems: "center",
    borderTopColor: tokens.subtleBorder,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    color: tokens.faint,
    display: "grid",
    fontSize: 11,
    gap: 4,
    gridTemplateColumns: "1fr auto auto auto",
    marginTop: "auto",
    padding: "8px 2px 0",
    "@media (max-width: 680px)": { display: "none" },
  },
  sidebarFooterStatus: {
    alignItems: "center",
    display: "flex",
    gap: 6,
    minWidth: 0,
    whiteSpace: "nowrap",
  },
  sidebarFooterButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: "transparent",
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 0,
    color: tokens.faint,
    display: "flex",
    height: 24,
    justifyContent: "center",
    padding: 0,
    width: 24,
    ":hover": { backgroundColor: tokens.hover, color: tokens.text },
  },
  view: {
    animationDuration: "190ms",
    animationName: viewEnter,
    animationTimingFunction: "cubic-bezier(.16,1,.3,1)",
    height: 778,
    minWidth: 0,
    overflow: "auto",
    scrollbarWidth: "none",
    "::-webkit-scrollbar": { display: "none" },
    "@media (max-width: 680px)": { height: 424, width: "100%" },
  },
  page: {
    display: "flex",
    flexDirection: "column",
    marginInline: "auto",
    maxWidth: 1200,
    minHeight: "100%",
    minWidth: 0,
    padding: "28px 32px",
    width: "100%",
    "@media (max-width: 680px)": { padding: "18px 16px" },
  },
  pageHeading: {
    alignItems: "flex-start",
    display: "flex",
    justifyContent: "space-between",
    marginBottom: 24,
    "@media (max-width: 680px)": { marginBottom: 18 },
  },
  pageTitle: {
    fontSize: 20,
    fontWeight: 600,
    letterSpacing: "-.01em",
    lineHeight: 1.15,
    margin: 0,
  },
  pageSubtitle: {
    color: tokens.muted,
    fontSize: 13,
    lineHeight: 1.4,
    margin: "6px 0 0",
    maxWidth: 510,
  },
  pageCount: { color: tokens.muted, fontSize: 11, fontVariantNumeric: "tabular-nums" },
  addSkillDrawer: {
    backgroundColor: tokens.surface,
    borderLeftColor: tokens.border,
    borderLeftStyle: "solid",
    borderLeftWidth: 1,
    boxShadow: "-18px 0 54px oklch(0 0 0 / .3)",
    display: "flex",
    flexDirection: "column",
    inset: "22px 0 0 auto",
    position: "absolute",
    width: 420,
    zIndex: 2,
    "@media (max-width: 680px)": { inset: "40px 0 0", width: "100%" },
  },
  addSkillHeader: {
    alignItems: "center",
    borderBottomColor: tokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    display: "flex",
    justifyContent: "space-between",
    minHeight: 56,
    paddingInline: 20,
  },
  addSkillTitle: { fontSize: 15, fontWeight: 600, margin: 0 },
  drawerClose: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: "transparent",
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.muted,
    display: "flex",
    height: 32,
    justifyContent: "center",
    padding: 0,
    width: 32,
    ":hover": { backgroundColor: tokens.hover, color: tokens.text },
    ":active": { transform: "scale(.96)" },
  },
  addSkillTabs: {
    borderBottomColor: tokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
    display: "flex",
    minHeight: 42,
    paddingInline: 20,
  },
  addSkillTab: {
    backgroundColor: "transparent",
    borderColor: "transparent",
    borderStyle: "solid",
    borderWidth: "0 0 2px",
    color: tokens.muted,
    fontSize: 12,
    paddingInline: 12,
  },
  addSkillTabActive: { borderBottomColor: tokens.accent, color: tokens.accent },
  addSkillBody: {
    display: "flex",
    flex: 1,
    flexDirection: "column",
    gap: 22,
    overflow: "hidden",
    padding: "18px 20px",
  },
  addSkillField: { display: "flex", flexDirection: "column", gap: 8 },
  addSkillInput: {
    backgroundColor: tokens.background,
    borderColor: tokens.border,
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.text,
    fontFamily: "inherit",
    fontSize: 12,
    height: 34,
    outline: "none",
    paddingInline: 10,
    width: "100%",
    ":focus": { borderColor: tokens.accent },
  },
  addSkillHelp: { color: tokens.faint, fontSize: 10, lineHeight: 1.4 },
  methodPicker: { display: "flex" },
  methodButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    fontSize: 11,
    height: 28,
    marginLeft: -1,
    paddingInline: 11,
    ":first-child": { borderRadius: "4px 0 0 4px", marginLeft: 0 },
    ":last-child": { borderRadius: "0 4px 4px 0" },
  },
  installControls: {
    display: "flex",
    flexDirection: "column",
    gap: 20,
  },
  installGroup: { display: "flex", flexDirection: "column", gap: 10 },
  installScope: { display: "grid", gridTemplateColumns: "1fr 1fr" },
  installScopeButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    fontSize: 12,
    height: 34,
    marginLeft: -1,
    ":first-child": { borderRadius: "4px 0 0 4px", marginLeft: 0 },
    ":last-child": { borderRadius: "0 4px 4px 0" },
  },
  installTargetRow: {
    alignItems: "center",
    borderColor: tokens.subtleBorder,
    borderStyle: "solid",
    borderWidth: "0 0 1px",
    color: tokens.text,
    display: "grid",
    gap: 10,
    gridTemplateColumns: "minmax(0,1fr) auto auto",
    minHeight: 52,
    padding: "7px 2px",
    textAlign: "left",
    ":last-child": { gridTemplateColumns: "minmax(0,1fr) auto" },
  },
  installTargetIdentity: {
    alignItems: "center",
    display: "flex",
    gap: 9,
    minWidth: 0,
  },
  installTargetCopy: { display: "flex", flexDirection: "column", gap: 3, minWidth: 0 },
  installTargetName: { fontSize: 12 },
  installTargetPath: {
    color: tokens.faint,
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: 10,
  },
  installSwitch: {
    backgroundColor: tokens.raised,
    borderWidth: 0,
    borderRadius: 999,
    display: "block",
    height: 18,
    padding: 2,
    transition: "background-color 150ms ease-out",
    width: 30,
  },
  installSwitchActive: { backgroundColor: tokens.accent },
  installSwitchKnob: {
    backgroundColor: tokens.text,
    borderRadius: "50%",
    display: "block",
    height: 14,
    transform: "translateX(0)",
    transition: "transform 150ms ease-out",
    width: 14,
  },
  installSwitchKnobActive: { transform: "translateX(12px)" },
  trialOption: {
    alignItems: "flex-start",
    color: tokens.text,
    display: "flex",
    fontSize: 12,
    gap: 8,
  },
  trialCheckbox: { margin: "2px 0 0" },
  trialCopy: { display: "flex", flexDirection: "column", gap: 4 },
  trialHelp: { color: tokens.faint, fontSize: 10 },
  addSkillFooter: {
    borderTopColor: tokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
    gap: 8,
    justifyContent: "flex-end",
    padding: "14px 20px",
  },
  cancelAction: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderRadius: 5,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.text,
    fontSize: 12,
    height: 34,
    paddingInline: 14,
    ":hover": { backgroundColor: tokens.hover },
    ":active": { transform: "scale(.96)" },
  },
  installAction: {
    alignItems: "center",
    backgroundColor: tokens.accent,
    borderRadius: 5,
    borderWidth: 0,
    color: tokens.text,
    display: "flex",
    fontSize: 12,
    fontWeight: 600,
    height: 34,
    justifyContent: "center",
    paddingInline: 14,
    transition: "background-color 150ms ease-out, transform 150ms ease-out",
    ":hover": { backgroundColor: tokens.accentHover },
    ":active": { transform: "scale(.96)" },
    ":disabled": { backgroundColor: tokens.raised, color: tokens.faint },
  },
  mobileHidden: { "@media (max-width: 680px)": { display: "none" } },
  mobileOnly: { display: "none", "@media (max-width: 680px)": { display: "inline" } },
  statGrid: {
    display: "grid",
    gap: 12,
    gridTemplateColumns: "repeat(3,1fr)",
    "@media (max-width: 680px)": { gap: 5 },
  },
  statCard: {
    backgroundColor: tokens.surface,
    borderColor: tokens.subtleBorder,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.text,
    display: "flex",
    flexDirection: "column",
    gap: 4,
    padding: "14px 16px",
    textAlign: "left",
    transition: "background-color 150ms ease-out, border-color 150ms ease-out",
    ":hover": { backgroundColor: tokens.hover, borderColor: tokens.border },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 680px)": { minWidth: 0, padding: "10px 11px" },
  },
  statNumber: {
    fontSize: 24,
    fontVariantNumeric: "tabular-nums",
    letterSpacing: "-.02em",
    lineHeight: 1.1,
  },
  danger: { color: tokens.danger },
  warning: { color: tokens.warning },
  accent: { color: tokens.accent },
  dangerBackground: { backgroundColor: tokens.danger },
  warningBackground: { backgroundColor: tokens.warning },
  accentBackground: { backgroundColor: tokens.accent },
  summaryCard: {
    alignItems: "center",
    backgroundColor: tokens.surface,
    borderColor: tokens.border,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    display: "grid",
    gap: 12,
    gridTemplateColumns: "1fr 2fr",
    marginTop: 10,
    padding: "14px 16px",
  },
  cardLabel: {
    color: tokens.faint,
    display: "block",
    fontSize: 11,
    letterSpacing: ".06em",
    textTransform: "uppercase",
  },
  laneCard: {
    backgroundColor: tokens.surface,
    borderColor: tokens.subtleBorder,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    flexDirection: "column",
    gap: 8,
    marginTop: 24,
    overflow: "hidden",
    padding: "14px 16px",
  },
  laneRow: {
    alignItems: "baseline",
    display: "grid",
    gap: 12,
    gridTemplateColumns: "170px minmax(0,1fr)",
    "@media (max-width: 680px)": { gap: 6, gridTemplateColumns: "1fr" },
  },
  laneTitle: { color: tokens.muted, fontSize: 12, whiteSpace: "nowrap" },
  laneSegments: { display: "flex", gap: 2, height: 28, maxWidth: "100%", minWidth: 0 },
  lanePrimary: {
    alignItems: "center",
    backgroundColor: tokens.accentSoft,
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.text,
    display: "flex",
    fontSize: 12,
    gap: 4,
    minWidth: 0,
    overflow: "hidden",
    paddingInline: 10,
    whiteSpace: "nowrap",
  },
  laneSecondary: {
    alignItems: "center",
    backgroundColor: "oklch(0.668 0.176 293 / .08)",
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.muted,
    display: "flex",
    fontSize: 12,
    gap: 4,
    minWidth: 0,
    overflow: "hidden",
    paddingInline: 10,
    whiteSpace: "nowrap",
  },
  laneTertiary: {
    alignItems: "center",
    backgroundColor: tokens.raised,
    borderRadius: 4,
    borderWidth: 0,
    color: tokens.muted,
    display: "flex",
    fontSize: 12,
    gap: 4,
    minWidth: 0,
    overflow: "hidden",
    paddingInline: 10,
    whiteSpace: "nowrap",
  },
  laneBoth: { flex: "2.1 1 0" },
  laneModel: { flex: "1 1 0" },
  laneUser: { flex: "1 1 0" },
  laneActive: { flex: "2.2 1 0" },
  laneIdle: { flex: "1 1 0" },
  summaryMetrics: { display: "grid", gridTemplateColumns: "repeat(4,1fr)" },
  metric: { color: tokens.muted, display: "flex", flexDirection: "column", fontSize: 11, gap: 2 },
  inbox: {
    display: "flex",
    flexDirection: "column",
    marginTop: 24,
  },
  sectionLabel: {
    alignItems: "center",
    borderColor: tokens.subtleBorder,
    borderStyle: "solid",
    borderWidth: "1px 0",
    color: tokens.text,
    display: "flex",
    backgroundColor: tokens.surface,
    fontSize: 12,
    fontWeight: 600,
    gap: 8,
    height: 34,
    margin: 0,
    paddingInline: 12,
    textAlign: "left",
    width: "100%",
  },
  inboxRow: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.subtleBorder,
    borderRadius: 0,
    borderStyle: "solid",
    borderWidth: "0 0 1px",
    color: tokens.text,
    display: "grid",
    gap: 12,
    gridTemplateColumns: "6px minmax(0,260px) minmax(0,1fr) auto",
    height: 36,
    padding: "0 12px 0 16px",
    textAlign: "left",
    transition: "background-color 150ms ease-out",
    width: "100%",
    ":hover": { backgroundColor: tokens.raised },
    ":active": { transform: "scale(.99)" },
    "@media (max-width: 680px)": {
      gridTemplateColumns: "6px minmax(0,1fr) auto",
    },
  },
  inboxCopy: {
    display: "contents",
  },
  inboxName: { alignItems: "center", display: "flex", fontSize: 13, gap: 8, minWidth: 0 },
  inboxDetail: {
    color: tokens.faint,
    fontSize: 12,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    "@media (max-width: 680px)": { display: "none" },
  },
  rowAction: { color: tokens.faint, fontSize: 12, minWidth: 64, textAlign: "right" },
  statusDot: { borderRadius: "50%", height: 6, width: 6 },
  toolbar: {
    alignItems: "center",
    display: "flex",
    gap: 8,
    marginBottom: 18,
    "@media (max-width: 680px)": { flexWrap: "wrap" },
  },
  filterInput: {
    alignItems: "center",
    backgroundColor: tokens.surface,
    borderColor: tokens.border,
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.faint,
    display: "flex",
    flex: 1,
    gap: 7,
    height: 32,
    maxWidth: 260,
    paddingInline: 12,
    ":focus-within": { borderColor: tokens.accent },
    "@media (max-width: 680px)": { flexBasis: "100%", maxWidth: "none" },
  },
  segmented: { display: "flex" },
  segmentButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    fontSize: 12,
    height: 32,
    marginLeft: -1,
    paddingInline: 12,
    transition: "background-color 150ms ease-out, color 150ms ease-out",
    ":first-child": { borderRadius: "4px 0 0 4px", marginLeft: 0 },
    ":last-child": { borderRadius: "0 4px 4px 0" },
    ":hover": { color: tokens.text },
    "@media (max-width: 680px)": { height: 36, paddingInline: 10 },
  },
  segmentButtonActive: { backgroundColor: tokens.raised, color: tokens.text },
  filterButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    display: "flex",
    fontSize: 12,
    gap: 6,
    height: 32,
    paddingInline: 12,
    "@media (max-width: 680px)": { height: 36 },
  },
  tableHeader: {
    color: tokens.faint,
    display: "grid",
    fontSize: 11,
    gap: 12,
    gridTemplateColumns: tableColumns,
    padding: "0 12px 6px",
    "@media (max-width: 680px)": { gridTemplateColumns: "minmax(0,1fr) 64px" },
  },
  rows: { display: "flex", flexDirection: "column", gap: 6 },
  skillRow: {
    alignItems: "center",
    backgroundColor: tokens.surface,
    borderColor: tokens.border,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.text,
    display: "grid",
    fontSize: 13,
    gap: 12,
    gridTemplateColumns: tableColumns,
    minHeight: 44,
    paddingInline: 12,
    textAlign: "left",
    transition: "background-color 150ms ease-out, border-color 150ms ease-out",
    width: "100%",
    ":hover": { backgroundColor: tokens.raised, borderColor: tokens.accent },
    ":active": { transform: "scale(.99)" },
    "@media (max-width: 680px)": { gridTemplateColumns: "minmax(0,1fr) 64px" },
  },
  skillName: {
    alignItems: "center",
    display: "flex",
    fontWeight: 600,
    gap: 6,
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  updateBadge: {
    backgroundColor: tokens.accentSoft,
    borderRadius: 2,
    color: tokens.text,
    fontSize: 11,
    fontWeight: 600,
    padding: "1px 6px",
  },
  truncate: {
    color: tokens.muted,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  location: { alignItems: "center", color: tokens.muted, display: "flex", gap: 6, minWidth: 0 },
  agentStack: { alignItems: "center", display: "flex", gap: 4 },
  number: { color: tokens.muted, fontVariantNumeric: "tabular-nums", textAlign: "right" },
  empty: { color: tokens.muted, fontSize: 10, textAlign: "center" },
  pluginGroups: { display: "flex", flexDirection: "column", gap: 16 },
  pluginGroup: {
    backgroundColor: "transparent",
    borderWidth: 0,
    overflow: "hidden",
  },
  pluginHeader: {
    alignItems: "center",
    color: tokens.muted,
    display: "flex",
    fontSize: 12,
    justifyContent: "space-between",
    minHeight: 32,
    paddingInline: 8,
  },
  pluginTitle: { alignItems: "center", color: tokens.text, display: "flex", gap: 8 },
  pluginCopy: {
    alignItems: "center",
    display: "grid",
    gap: 12,
    gridTemplateColumns: "190px 1fr",
    minWidth: 0,
    "@media (max-width: 680px)": { gridTemplateColumns: "minmax(0,1fr)" },
  },
  pluginDescription: { "@media (max-width: 680px)": { display: "none" } },
  pluginName: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  pluginRow: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.subtleBorder,
    borderStyle: "solid",
    borderRadius: 2,
    borderWidth: 1,
    color: tokens.text,
    display: "grid",
    gap: 10,
    gridTemplateColumns: "1fr auto auto",
    fontSize: 13,
    minHeight: 44,
    marginBottom: 6,
    paddingInline: 12,
    textAlign: "left",
    transition: "background-color 150ms ease-out",
    width: "100%",
    ":hover": { backgroundColor: tokens.hover },
    ":active": { transform: "scale(.99)" },
  },
  activityCard: {
    backgroundColor: "transparent",
    borderWidth: 0,
    padding: 0,
  },
  activityTop: {
    alignItems: "baseline",
    display: "flex",
    gap: 12,
    justifyContent: "space-between",
  },
  activityLabel: {
    color: tokens.faint,
    fontSize: 11,
    fontWeight: 500,
    letterSpacing: ".08em",
    textTransform: "uppercase",
  },
  activityTotal: { color: tokens.faint, fontSize: 12 },
  heatmapMonths: {
    color: tokens.faint,
    display: "grid",
    fontSize: 12,
    gap: 3,
    gridTemplateColumns: "repeat(52,1fr)",
    marginLeft: 34,
    marginTop: 12,
    "@media (max-width: 680px)": { display: "none" },
  },
  heatmapBody: { display: "flex", gap: 6 },
  weekdays: {
    color: tokens.faint,
    display: "grid",
    flexShrink: 0,
    fontSize: 11,
    gridTemplateRows: "repeat(7,1fr)",
    paddingBlock: 2,
    width: 28,
  },
  heatmap: {
    display: "grid",
    flex: 1,
    gap: 3,
    gridAutoFlow: "column",
    gridTemplateColumns: "repeat(52,1fr)",
    gridTemplateRows: "repeat(7,1fr)",
    "@media (max-width: 680px)": {
      gridTemplateColumns: "repeat(26,1fr)",
      overflow: "hidden",
    },
  },
  heatCell: {
    aspectRatio: "1",
    backgroundColor: tokens.raised,
    borderRadius: 2,
    "@media (max-width: 680px)": { ":nth-child(-n+182)": { display: "none" } },
  },
  heat1: { backgroundColor: "oklch(0.668 0.176 293 / .3)" },
  heat2: { backgroundColor: "oklch(0.668 0.176 293 / .5)" },
  heat3: { backgroundColor: "oklch(0.668 0.176 293 / .72)" },
  heat4: { backgroundColor: tokens.accent },
  activityTable: { marginTop: 24 },
  activityTableTop: {
    alignItems: "baseline",
    display: "flex",
    justifyContent: "space-between",
    marginBottom: 12,
  },
  activitySegmented: {
    borderColor: tokens.border,
    borderRadius: 4,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    overflow: "hidden",
  },
  activitySegmentButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderStyle: "solid",
    borderWidth: "0 0 0 1px",
    color: tokens.faint,
    fontSize: 11,
    height: 24,
    paddingInline: 8,
    ":first-child": { borderLeftWidth: 0 },
  },
  activityHeader: {
    color: tokens.faint,
    display: "grid",
    fontSize: 11,
    gap: 12,
    gridTemplateColumns: "minmax(0,1fr) 72px 88px 72px",
    padding: "0 8px 6px",
    "@media (max-width: 680px)": { gridTemplateColumns: "minmax(0,1fr) 72px" },
  },
  activityRow: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.subtleBorder,
    borderStyle: "solid",
    borderWidth: "1px 0 0",
    color: tokens.muted,
    display: "grid",
    fontSize: 13,
    gap: 12,
    gridTemplateColumns: "minmax(0,1fr) 72px 88px 72px",
    minHeight: 32,
    paddingInline: 8,
    textAlign: "left",
    width: "100%",
    ":hover": { backgroundColor: tokens.hover },
    ":active": { transform: "scale(.99)" },
    "@media (max-width: 680px)": {
      gridTemplateColumns: "minmax(0,1fr) 72px",
      minHeight: 40,
    },
  },
  activitySkill: {
    color: tokens.text,
    fontWeight: 400,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  detailHeader: { display: "flex", flexDirection: "column", gap: 8 },
  detailTitle: {
    flex: 1,
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
    "@media (max-width: 680px)": { fontSize: 16 },
  },
  detailHeading: {
    alignItems: "center",
    display: "flex",
    gap: 16,
    "@media (max-width: 680px)": { gap: 6 },
  },
  backButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderWidth: 0,
    color: tokens.faint,
    display: "flex",
    flexShrink: 0,
    fontSize: 12,
    gap: 6,
    padding: 4,
  },
  backButtonLabel: { "@media (max-width: 440px)": { display: "none" } },
  detailActions: {
    alignItems: "center",
    display: "flex",
    flexShrink: 0,
    gap: 8,
    "@media (max-width: 680px)": { gap: 3 },
  },
  updateButton: {
    alignItems: "center",
    backgroundColor: tokens.accent,
    borderRadius: 6,
    borderWidth: 0,
    color: tokens.text,
    display: "flex",
    fontSize: 13,
    fontWeight: 600,
    gap: 6,
    height: 32,
    paddingInline: 12,
    transition: "background-color 150ms ease-out, transform 150ms ease-out",
    ":hover": { backgroundColor: tokens.accentHover },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 680px)": { fontSize: 12, paddingInline: 6 },
  },
  secondaryButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    display: "flex",
    fontSize: 13,
    gap: 6,
    height: 32,
    paddingInline: 12,
    ":hover": { backgroundColor: tokens.hover, color: tokens.text },
    ":active": { transform: "scale(.96)" },
    "@media (max-width: 680px)": { fontSize: 12, paddingInline: 6 },
    "@media (max-width: 440px)": { justifyContent: "center", paddingInline: 0, width: 32 },
  },
  assistantLabel: { "@media (max-width: 440px)": { display: "none" } },
  iconButton: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    display: "flex",
    height: 32,
    justifyContent: "center",
    width: 32,
    ":active": { transform: "scale(.96)" },
  },
  detailDescription: {
    color: tokens.muted,
    fontSize: 13,
    lineHeight: 1.5,
    margin: 0,
    maxWidth: "65ch",
  },
  chips: { display: "flex", gap: 6 },
  chip: {
    backgroundColor: tokens.raised,
    borderRadius: 999,
    color: tokens.muted,
    fontSize: 11,
    padding: "2px 8px",
  },
  metadata: {
    color: tokens.faint,
    fontSize: 12,
    fontVariantNumeric: "tabular-nums",
    margin: 0,
  },
  detailStack: {
    display: "flex",
    flexDirection: "column",
    gap: 24,
    marginTop: 24,
    "@media (max-width: 680px)": { gap: 16, marginTop: 18 },
  },
  detailCard: {
    backgroundColor: "transparent",
    borderColor: tokens.subtleBorder,
    borderRadius: 8,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    flexDirection: "column",
    gap: 4,
    padding: 16,
    "@media (max-width: 680px)": { padding: 12 },
  },
  cardHeading: {
    alignItems: "center",
    display: "flex",
    fontSize: 13,
    justifyContent: "space-between",
  },
  cardTitle: { fontSize: 13, margin: 0 },
  scopeRow: {
    alignItems: "center",
    borderRadius: 6,
    color: tokens.faint,
    display: "grid",
    gap: 12,
    gridTemplateColumns: "minmax(0,1fr) auto",
    height: 36,
    marginInline: -8,
    paddingInline: 8,
    ":hover": { backgroundColor: tokens.hover },
  },
  scopeMain: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderWidth: 0,
    color: tokens.faint,
    display: "grid",
    gap: 12,
    gridTemplateColumns: "20px minmax(0,1fr) auto",
    height: "100%",
    minWidth: 0,
    padding: 0,
    textAlign: "left",
  },
  scopeChevron: {
    alignItems: "center",
    display: "flex",
    height: 20,
    justifyContent: "center",
    width: 20,
  },
  scopeIdentity: {
    alignItems: "center",
    color: tokens.text,
    display: "flex",
    gap: 8,
    minWidth: 0,
  },
  scopeName: { color: tokens.text, fontSize: 13 },
  readerStack: { alignItems: "center", display: "flex", paddingLeft: 4 },
  readerBadge: {
    alignItems: "center",
    backgroundColor: tokens.raised,
    borderColor: tokens.background,
    borderRadius: "50%",
    borderStyle: "solid",
    borderWidth: 2,
    display: "flex",
    height: 18,
    justifyContent: "center",
    width: 18,
  },
  readerBadgeOverlap: { marginLeft: -4 },
  locationControls: { alignItems: "center", display: "flex", gap: 4 },
  switchTrack: {
    backgroundColor: tokens.accent,
    borderRadius: 999,
    display: "flex",
    height: 14,
    justifyContent: "flex-end",
    padding: 1,
    width: 24,
  },
  switchKnob: {
    backgroundColor: tokens.text,
    borderRadius: "50%",
    display: "block",
    height: 12,
    width: 12,
  },
  rowMenu: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderWidth: 0,
    color: tokens.faint,
    display: "flex",
    height: 24,
    justifyContent: "center",
    padding: 0,
    width: 24,
  },
  invocation: {
    borderTopColor: tokens.subtleBorder,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
    flexDirection: "column",
    gap: 6,
    marginTop: 12,
    paddingTop: 12,
  },
  invocationLabel: {
    color: tokens.faint,
    fontSize: 11,
    fontWeight: 500,
    letterSpacing: ".08em",
    textTransform: "uppercase",
  },
  invocationRow: {
    alignItems: "center",
    display: "grid",
    gap: 10,
    gridTemplateColumns: "16px minmax(0,1fr) auto",
    minHeight: 32,
    "@media (max-width: 680px)": { gridTemplateColumns: "16px minmax(0,1fr)" },
  },
  invocationFile: { display: "flex", flexDirection: "column", minWidth: 0 },
  invocationSegments: {
    display: "flex",
    "@media (max-width: 680px)": { gridColumn: "1 / -1", width: "100%" },
  },
  invocationButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderStyle: "solid",
    borderWidth: "1px 1px 1px 0",
    color: tokens.faint,
    fontSize: 12,
    height: 26,
    paddingInline: 12,
    whiteSpace: "nowrap",
    ":first-child": { borderLeftWidth: 1, borderRadius: "6px 0 0 6px" },
    ":last-child": { borderRadius: "0 6px 6px 0" },
    "@media (max-width: 680px)": { flex: 1, paddingInline: 8 },
  },
  invocationNote: { color: tokens.faint, fontSize: 12, lineHeight: 1.4, margin: 0 },
  markdownActions: {
    alignItems: "center",
    color: tokens.faint,
    display: "flex",
    fontSize: 11,
    gap: 8,
  },
  editButton: {
    backgroundColor: "transparent",
    borderColor: tokens.border,
    borderRadius: 6,
    borderStyle: "solid",
    borderWidth: 1,
    color: tokens.muted,
    fontSize: 12,
    height: 28,
    paddingInline: 10,
  },
  markdown: { color: tokens.muted, fontSize: 13, lineHeight: 1.6, padding: "12px 16px" },
  markdownTitle: { color: tokens.text, fontSize: 18, margin: "0 0 8px" },
  markdownHeading: { color: tokens.text, fontSize: 14, margin: "16px 0 8px" },
  markdownParagraph: { margin: "0 0 13px", maxWidth: "72ch" },
  markdownList: { margin: "0 0 13px", paddingLeft: 20 },
});
