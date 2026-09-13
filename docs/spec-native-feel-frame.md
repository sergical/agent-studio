# Spec: Frame (step 2 of docs/spec-native-feel-iteration.md)

Work in `/Users/sergiydybskiy/src/agent-studio/.claude/worktrees/shared-core-primitives` (branch `dev/browser-harness`). Run every command from there. Do not `cd` elsewhere. Do not touch `package-lock.json`. Do not commit. Do not start a dev server (Vite runs on http://localhost:1420; if it does not answer, report that and skip the browser part).

Goal: the window reads as one native frame, like Linear. The sidebar sits on the shell ground with no border and no big controls. The content is an inset, rounded, bordered panel. Every page starts with one slim header bar (breadcrumb left, actions right) and an optional thin toolbar row under it. There is no large page title anywhere. Existing tokens only; do not add CSS variables.

Tokens you will use: `bg-bg-secondary` is the shell ground (sidebar colour today), `bg-bg-primary` the panel, `border-border` the panel edge, `border-border-subtle` inner dividers, `rounded-md` (10px). Type: `text-caption` 11, `text-small` 12, `text-body` 13. `--control-height` is 32px and every form control reads it.

## 1. App frame (`apps/desktop/src/App.tsx` lines 139–148)

Replace the frame with:

```tsx
<div className="flex h-screen overflow-hidden bg-bg-secondary">
  <Sidebar ... />                        {/* unchanged props */}
  <div className="flex min-w-0 flex-1 flex-col pr-2 pb-2">
    <div data-tauri-drag-region className="h-9 shrink-0" />
    <main className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-bg-primary">
      {main}
    </main>
  </div>
</div>
```

The old absolute `h-7` drag band goes. `main` no longer scrolls; each page scrolls inside itself (section 3). Keep everything else in `App.tsx` as is (toasts, sheets, providers).

## 2. Sidebar (`apps/desktop/src/components/Sidebar/Sidebar.tsx`)

- `nav` root: `flex w-60 shrink-0 flex-col overflow-hidden` — remove `border-r border-border bg-bg-secondary` (the shell ground shows through).
- Keep the `h-9` drag region.
- Replace the search `Input` + "Add skill" `Button` block (lines ~142–158) with one switcher row:

```tsx
<div className="flex h-7 shrink-0 items-center justify-between pr-1.5 pl-3.5">
  <span className="text-small font-semibold text-text-primary">Skill Studio</span>
  <div className="flex items-center gap-0.5">
    <TooltipControl content="Search skills">
      <Button variant="ghost" size="icon-xs" className={iconButtonClass} aria-label="Search skills" onClick={goToSearch}>
        <Search size={14} />
      </Button>
    </TooltipControl>
    <TooltipControl content="Add skill">
      <Button variant="ghost" size="icon-xs" className={iconButtonClass} aria-label="Add skill" onClick={() => openAddSkillSheet()}>
        <Plus size={14} />
      </Button>
    </TooltipControl>
  </div>
</div>
```

  `goToSearch`: `if (anchorView.kind !== "skills") setActiveView({ kind: "skills" }); requestSkillSearchFocus();` where `requestSkillSearchFocus` is a new store action (section 4). Delete `searchInputRef`, `handleSearchChange`, `handleSearchKeyDown`, and any effect that only served the removed input (check lines 57–105; keep effects that serve other state). Drop unused imports (`Input`, `KeyboardEvent`). The `Search` icon is currently used on the Skills nav item; change that item's icon to `Layers` (lucide) so the two are not confused.
- Items: `itemClass` becomes `` `grid h-6.5 w-full grid-cols-[14px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 text-left text-body ${active ? "bg-bg-active text-text-primary" : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"}` ``. Icon `size={14}` on every nav item. Counts stay `text-caption tabular-nums text-text-tertiary`.
- Group wrappers: `flex flex-col gap-px px-2 pt-2.5` for the first group and `flex flex-col gap-px px-2 pt-3` for the Parked group (drop `pb-2.5` and `first:pt-3`).
- Footer (line ~240): remove `border-t border-border-subtle`; keep `mt-auto flex items-center justify-between gap-2 px-2 py-1.5`. Sync button and Learn/Settings icons unchanged.
- `iconButtonClass` stays `rounded-sm text-text-tertiary`; add `hover:text-text-primary`.

## 3. PageShell (`apps/desktop/src/components/Shell/PageShell.tsx`)

New shape. It owns the header bar, the optional toolbar, and the scroll area:

```tsx
interface PageShellProps {
  title: string;
  /** Parent crumb, shown before the title as "Skills ›". Clicking it runs `onClick`. */
  parent?: { label: string; onClick: () => void };
  subtitle?: string;
  actions?: ReactNode;
  /** Thin row under the header: filters, tabs. Controls inside are 28px tall. */
  toolbar?: ReactNode;
  children: ReactNode;
  width?: "default" | "narrow";
}
```

Render:

```tsx
<section className="flex min-h-0 flex-1 flex-col">
  <header className="flex h-10 shrink-0 items-center justify-between gap-3 border-b border-border-subtle px-4">
    <div className="flex min-w-0 items-center gap-1.5 text-small">
      {parent && (
        <>
          <Button variant="ghost" size="xs" className="h-6 rounded-sm px-1.5 text-small text-text-tertiary hover:text-text-primary" onClick={parent.onClick}>{parent.label}</Button>
          <ChevronRight size={12} className="shrink-0 text-text-quaternary" />
        </>
      )}
      <h1 className="m-0 truncate text-small font-medium text-text-primary">{title}</h1>
      {subtitle && <span className="min-w-0 truncate text-small text-text-tertiary">· {subtitle}</span>}
    </div>
    {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
  </header>
  {toolbar && (
    <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-4 [--control-height:28px]">
      {toolbar}
    </div>
  )}
  <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
    <div className={`mx-auto flex w-full flex-col gap-5 px-6 pt-5 pb-7 ${width === "narrow" ? "max-w-180" : "max-w-300"}`}>
      {children}
    </div>
  </div>
</section>
```

Update the header comment. Sticky elements inside views (Home `GroupHead` `sticky top-0`, Learn `sticky top-6`) now stick inside the scroll div; confirm in the browser check.

Callers:
- `SkillsView.tsx`: pass `<SkillListFilterBar .../>` as `toolbar` instead of a child. `ScanPartialBanner` stays the first child.
- `SkillActivityView.tsx`, `PluginSkillsView.tsx`: unchanged (subtitle now inline).
- `LearnView.tsx`: remove the `actions` "← Home" button (the sidebar navigates).
- `PacksView.tsx`, `HomeView.tsx`: unchanged.
- `SkillPage.tsx` (lines 369–395 both branches): replace the `mx-auto ... px-8 pt-9 pb-7` wrappers with `PageShell` `title={skill.name}` (or `activeView` name for the "no longer installed" branch: use the `name` the page already receives) and `parent={{ label: backLabel(from), onClick: onBack }}`. Move `backLabel` out of `InstalledSkillHeader.tsx` into a small exported helper in a new file `apps/desktop/src/components/SkillDetail/skill-page-nav.ts` (both files import it). Remove the back `Button` and its wrapper row from the "no longer installed" branch.
- `InstalledSkillHeader.tsx`: the header's top row (lines ~107–116, back button) goes; the actions cluster (primary, assistant toggle, `MenuControl`) stays in this component but the row becomes `flex items-center justify-end gap-2` — this is a step 5 subject, do not restructure further. The `<h1 className="text-title ...">` on line ~188 becomes `<h2 className="text-heading-lg font-semibold ...">` (the page's h1 is now the breadcrumb). Remove the `onBack` prop and the `ArrowLeft` import if nothing else uses them.

## 4. Filter bar in the toolbar (`SkillListFilterBar.tsx`)

The toolbar is one 40px row; make the bar single-line and compact:
- Root `flex flex-col gap-2.5` → `flex w-full items-center gap-2`. The inner `flex flex-wrap items-center gap-2.5` wrapper becomes `flex items-center gap-2` and the trailing group (result count, sort, view toggle) gets `ml-auto`.
- Search wrapper `w-60` → `w-48`; `Search` icon `size={13}` → `12`, `left-3` → `left-2.5`, input `pl-8` → `pl-7`, `pr-3` → `pr-2`.
- Every control already reads `h-(--control-height)`, so they become 28px through the toolbar wrapper. `ToggleGroupItem` `px-3` → `px-2.5`. Result count stays `text-small tabular-nums text-text-tertiary`.
- Store action for search focus: in `apps/desktop/src/store/appStore.ts` add `skillSearchFocusRequest: number` (initial 0) and `requestSkillSearchFocus: () => void` (increments). In `SkillListFilterBar.tsx` hold a `ref` on the search `Input` and `useEffect(() => { if (request > 0) inputRef.current?.focus(); }, [request])` where `request = useAppStore((s) => s.skillSearchFocusRequest)`. Escape in that input clears the query and blurs (port the old `handleSearchKeyDown` behaviour from Sidebar).

## Rules

`anti-slop` lint: no `title=`, no `satisfies`, `// SAFETY:` on every type assertion, no dead exports, no raw form elements outside `packages/ui`. Imports grouped React, external, internal, types. Comments explain what is non-obvious. Do not add new tokens or CSS.

## Acceptance

```
npx tsc --noEmit -p apps/desktop/tsconfig.json
npx oxlint apps/desktop/src packages/ui/src
npx oxfmt --check <every file you changed>
```

Browser check (agent-browser session `frame`), screenshots into `/private/tmp/claude-501/-Users-sergiydybskiy-src-agent-studio/8dbae7bf-5ab7-4416-8005-9bc3e1a49e5a/scratchpad`. Put multi-line JS in a `.sh` script in that dir and run it with `sh`; do not pass inline JS with quotes directly to the Bash tool.

1. `agent-browser --session frame open "http://localhost:1420/?f=1"`, wait 3000, screenshot `frame-home.png`.
2. Click the sidebar "Skills" item, wait 1500, screenshot `frame-skills.png`. Eval: `getComputedStyle(document.querySelector('main')).borderRadius` is `10px`; `document.querySelector('h1').textContent` is `Skills`; the filter bar search input's `offsetHeight` is `28`; `document.querySelectorAll('button button').length` is `0`.
3. Click the sidebar search icon (`aria-label="Search skills"`); eval `document.activeElement.getAttribute('aria-label')` must be `Filter skills`.
4. Click the first skill row (a row whose text includes `commit`), wait 1500, screenshot `frame-detail.png`. Eval: `document.querySelector('h1').textContent` equals the skill name; the crumb button text is `Skills`; click the crumb and confirm the Skills list is back.
5. Open Home, scroll the content div by 600px (eval on the `overflow-y-auto` div inside `main`), screenshot `frame-home-scrolled.png` — group headers must stay stuck under the toolbar-less header.
6. Console: report every error verbatim (ignore `127.0.0.1:8787` network lines).

Report: files changed, check output verbatim on failure, the eval values, screenshot paths, and anything you were unsure about.
