// ============================================================================
// CommandPalette - ⌘K from anywhere: run an action, jump to a place, or open
// a skill by name. A combobox/listbox pair, not a menu - one text input, one
// scrollable results list, `aria-activedescendant` driving the highlight.
// No animation anywhere in this component: it opens and closes instantly,
// and the highlight moves without a transition.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import {
  BookOpen,
  Layers,
  LayoutDashboard,
  Moon,
  Package,
  Plus,
  Puzzle,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Sun,
  Activity as ActivityIcon,
} from "lucide-react";
import { Dialog, DialogContent, Input, Kbd } from "@skill-studio/ui";
import type { SkillSnapshot } from "@skill-studio/lib";
import { keyShortcutsFor, SHORTCUTS } from "../../lib/app-shortcuts";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { useAppStore } from "../../store/appStore";
import { RowGlyph } from "../SkillList/SkillRowCells";
import { rowGroup, rowState } from "../SkillList/skill-row-state";
import type { RowGroup } from "../SkillList/skill-row-state";
import { gotoItems, rankItems, SECTION_LABEL, skillItems } from "./command-palette-items";
import type { PaletteItem, PaletteSection } from "./command-palette-items";

/** Same three labels `SkillListTable` uses for its state groups - kept in sync by hand since
 * the palette's skill rows and the table's group headers should always read the same way. */
const GROUP_LABEL = {
  attention: "Needs attention",
  healthy: "Healthy",
  parked: "Parked",
} satisfies Record<RowGroup, string>;

interface CommandPaletteProps {
  snapshot: SkillSnapshot | undefined;
  requestRescan: () => Promise<void>;
}

/** No transition, no fade, no zoom - only instant open/close and an instant highlight move. */
const NO_ANIMATION =
  "duration-0! animate-none! transition-none! data-open:animate-none! data-closed:animate-none! data-open:duration-0! data-closed:duration-0!";

export function CommandPalette({ snapshot, requestRescan }: CommandPaletteProps) {
  const open = useAppStore((state) => state.commandPaletteOpen);
  const setOpen = useAppStore((state) => state.setCommandPaletteOpen);
  const setActiveView = useAppStore((state) => state.setActiveView);
  const openSkill = useAppStore((state) => state.openSkill);
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);
  const requestSkillSearchFocus = useAppStore((state) => state.requestSkillSearchFocus);
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const setTheme = useAppStore((state) => state.setTheme);

  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  // Tracked so the query/highlight reset below can happen during rendering, on the render where
  // `open` flips, instead of a follow-up effect setting that state after commit.
  const [prevOpen, setPrevOpen] = useState(open);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const packsEnabled = isFeatureEnabled("skill-packs");

  function close() {
    setOpen(false);
  }

  function run(item: PaletteItem) {
    // The item decides where focus goes next (e.g. the filter input); restoring the
    // pre-palette element would take it back.
    restoreFocusRef.current = null;
    close();
    item.run();
  }

  const items: PaletteItem[] = (() => {
    const actions: PaletteItem[] = [
      {
        id: "action-add-skill",
        section: "actions",
        label: SHORTCUTS.addSkill.label,
        icon: <Plus size={14} />,
        shortcut: SHORTCUTS.addSkill,
        run: () => openAddSkillSheet(),
      },
      {
        id: "action-sync",
        section: "actions",
        label: SHORTCUTS.sync.label,
        icon: <RefreshCw size={14} />,
        run: () => void requestRescan(),
      },
      {
        id: "action-filter-skills",
        section: "actions",
        label: "Filter skills",
        icon: <Search size={14} />,
        shortcut: SHORTCUTS.filterSkills,
        run: () => {
          setActiveView({ kind: "skills" });
          requestSkillSearchFocus();
        },
      },
      {
        id: "action-toggle-theme",
        section: "actions",
        label: SHORTCUTS.toggleTheme.label,
        icon: resolvedTheme === "dark" ? <Sun size={14} /> : <Moon size={14} />,
        run: () => setTheme(resolvedTheme === "dark" ? "light" : "dark"),
      },
    ];

    const goto = gotoItems(
      [
        { label: "Home", icon: <LayoutDashboard size={14} />, view: { kind: "home" } },
        { label: "Skills", icon: <Layers size={14} />, view: { kind: "skills" } },
        { label: "Plugins", icon: <Puzzle size={14} />, view: { kind: "plugins" } },
        { label: "Activity", icon: <ActivityIcon size={14} />, view: { kind: "activity" } },
        ...(packsEnabled
          ? [{ label: "Packs", icon: <Package size={14} />, view: { kind: "packs" as const } }]
          : []),
        { label: "Learn", icon: <BookOpen size={14} />, view: { kind: "learn" } },
        { label: "Settings", icon: <SettingsIcon size={14} />, view: { kind: "settings" } },
      ],
      setActiveView,
    );

    const skills = skillItems(
      snapshot?.skills ?? [],
      (skill) => GROUP_LABEL[rowGroup(skill, rowState(skill))],
      (skill) => <RowGlyph state={rowState(skill)} size={14} />,
      (skill) => openSkill(skill.name),
    );

    return [...actions, ...goto, ...skills];
  })();

  const results = rankItems(items, query);

  function onQueryChange(value: string) {
    setQuery(value);
    setHighlight(0);
  }

  // Resetting query/highlight is derived state (it only depends on the render where `open`
  // flips), so it's set here during rendering rather than in the effect below.
  if (open !== prevOpen) {
    setPrevOpen(open);
    if (open) {
      setQuery("");
      setHighlight(0);
    }
  }

  useEffect(() => {
    if (open) {
      restoreFocusRef.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
    } else {
      restoreFocusRef.current?.focus();
      restoreFocusRef.current = null;
    }
  }, [open]);

  useEffect(() => {
    const el = listRef.current?.querySelector(`[data-index="${highlight}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  function onKeyDown(e: ReactKeyboardEvent) {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      setHighlight((h) => Math.min(h + 1, results.length - 1));
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      setHighlight((h) => Math.max(h - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (results[highlight]) run(results[highlight]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }

  const activeId = results[highlight]
    ? `command-palette-option-${results[highlight].id}`
    : undefined;

  // Grouped for rendering, in the same order `rankItems` already produced.
  const sections: { section: PaletteSection; items: PaletteItem[] }[] =
    // SAFETY: these three string literals are exactly the `PaletteSection` union's members.
    (["actions", "goto", "skills"] as PaletteSection[])
      .map((section) => ({ section, items: results.filter((item) => item.section === section) }))
      .filter((group) => group.items.length > 0);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        showCloseButton={false}
        aria-label="Command palette"
        overlayClassName={NO_ANIMATION}
        className={`top-[15vh] left-1/2 w-[560px] max-w-[calc(100%-2rem)] -translate-x-1/2 translate-y-0 gap-0 rounded-lg border border-border bg-bg-secondary p-0 shadow-lg ring-0 ${NO_ANIMATION}`}
      >
        <div className="border-b border-border-subtle px-3 py-2.5">
          <Input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls="command-palette-listbox"
            aria-activedescendant={activeId}
            autoFocus
            placeholder="Type a command or skill"
            className="h-8 border-none bg-transparent px-0 shadow-none focus-visible:border-none"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            onKeyDown={onKeyDown}
          />
        </div>
        <div
          ref={listRef}
          id="command-palette-listbox"
          role="listbox"
          aria-label="Command palette results"
          className="max-h-[420px] overflow-y-auto p-1"
        >
          {sections.map(({ section, items: sectionItems }) => (
            <div key={section} role="group" aria-label={SECTION_LABEL[section]}>
              <div className="px-2 pt-1.5 pb-1 text-caption text-text-tertiary">
                {SECTION_LABEL[section]}
              </div>
              {sectionItems.map((item) => {
                const index = results.indexOf(item);
                const isHighlighted = index === highlight;
                return (
                  <div
                    key={item.id}
                    id={`command-palette-option-${item.id}`}
                    role="option"
                    aria-selected={isHighlighted}
                    data-index={index}
                    className={`flex h-8 cursor-pointer items-center gap-2 rounded-sm px-2 text-body text-text-secondary ${
                      isHighlighted ? "bg-bg-active text-text-primary" : ""
                    }`}
                    onMouseMove={() => setHighlight(index)}
                    onClick={() => run(item)}
                  >
                    {item.icon && (
                      <span className="flex size-3.5 shrink-0 items-center">{item.icon}</span>
                    )}
                    <span className="min-w-0 flex-1 truncate">{item.label}</span>
                    {item.secondary && (
                      <span className="shrink-0 text-caption text-text-tertiary">
                        {item.secondary}
                      </span>
                    )}
                    {item.shortcut && (
                      <span
                        className="flex shrink-0 items-center gap-0.5"
                        aria-keyshortcuts={keyShortcutsFor(item.shortcut)}
                      >
                        {item.shortcut.keys.map((key) => (
                          <Kbd key={key}>{key}</Kbd>
                        ))}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
          {results.length === 0 && (
            <div className="px-2 py-3 text-center text-small text-text-tertiary">No matches</div>
          )}
        </div>
        <div role="status" aria-live="polite" className="sr-only">
          {results.length} result{results.length === 1 ? "" : "s"}
        </div>
      </DialogContent>
    </Dialog>
  );
}
