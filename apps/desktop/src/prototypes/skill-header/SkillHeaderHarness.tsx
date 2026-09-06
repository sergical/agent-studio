// ============================================================================
// PROTOTYPE. Surrounding chrome only: sidebar, Locations + Invocation, assistant
// drawer. Variants own the installed skill header and must not repeat these facts.
// ============================================================================

import { LayoutDashboard, Plus, RefreshCw, Search } from "lucide-react";
import { HarnessIcon } from "../../components/ui/HarnessIcon";
import type { SkillHeaderFixture } from "./fixture";

function SidebarItem({ label, active, count }: { label: string; active: boolean; count?: string }) {
  return (
    <div
      className={`grid h-[30px] w-full grid-cols-[15px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2.5 text-left text-body ${
        active ? "bg-accent-soft text-text-primary" : "text-text-secondary"
      }`}
    >
      {label === "Home" ? <LayoutDashboard size={15} /> : <Search size={15} />}
      <span className="min-w-0 truncate">{label}</span>
      {count ? (
        <span className="text-right text-caption tabular-nums text-text-tertiary">{count}</span>
      ) : null}
    </div>
  );
}

export function PrototypeSidebar() {
  return (
    <nav
      className="flex w-60 shrink-0 flex-col border-r border-border bg-bg-secondary"
      aria-hidden="true"
    >
      <div className="flex flex-col gap-px px-2.5 pt-3 pb-2.5">
        <div className="flex h-8 items-center rounded-sm border border-border bg-bg-primary px-3 text-body text-text-quaternary">
          Search skills…
        </div>
        <div className="mt-1.5 flex h-[30px] items-center justify-center gap-1.5 rounded-sm bg-accent-soft text-body text-text-primary">
          <Plus size={15} />
          Add skill
        </div>
      </div>
      <div className="flex flex-col gap-px px-2.5 pb-2.5">
        <SidebarItem label="Home" active={false} />
        <SidebarItem label="Skills" active count="12" />
        <SidebarItem label="Activity" active={false} />
      </div>
      <div className="mt-auto flex items-center gap-1.5 border-t border-border-subtle px-2.5 py-2 text-caption text-text-tertiary">
        <RefreshCw size={13} />
        Sync
      </div>
    </nav>
  );
}

function InvocationControl({ selected }: { selected: "both" | "user" | "model" }) {
  return (
    <div className="flex items-center rounded-sm border border-border bg-bg-secondary p-0.5 text-caption">
      <span
        className={
          selected === "both"
            ? "rounded-[3px] bg-bg-primary px-2 py-0.5 font-medium text-text-primary shadow-xs"
            : "px-2 py-0.5 text-text-tertiary"
        }
      >
        Both
      </span>
      <span
        className={
          selected === "user"
            ? "rounded-[3px] bg-bg-primary px-2 py-0.5 font-medium text-text-primary shadow-xs"
            : "px-2 py-0.5 text-text-tertiary"
        }
      >
        User only
      </span>
      <span
        className={
          selected === "model"
            ? "rounded-[3px] bg-bg-primary px-2 py-0.5 font-medium text-text-primary shadow-xs"
            : "px-2 py-0.5 text-text-tertiary"
        }
      >
        Model only
      </span>
    </div>
  );
}

export function LocationsStart({ skill }: { skill: SkillHeaderFixture }) {
  const sharedPath = `~/.agents/skills/${skill.name}`;
  const claudePath = `~/.claude/skills/${skill.name}`;

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div className="text-body font-semibold text-text-primary">Locations</div>
      <div className="-mx-2 mt-1 flex flex-col">
        <div className="flex items-center gap-2 px-2 py-2 text-small">
          <HarnessIcon harness="shared" size={16} />
          <span className="font-medium text-text-primary">Shared</span>
          <span className="font-mono text-caption text-text-tertiary">{sharedPath}</span>
          <span className="ml-auto text-caption text-text-tertiary">{skill.scope}</span>
        </div>
        <div className="flex items-center gap-2 px-2 py-2 text-small">
          <HarnessIcon harness="claude-code" size={16} />
          <span className="font-medium text-text-primary">Claude Code</span>
          <span className="font-mono text-caption text-text-tertiary">{claudePath}</span>
          <span className="ml-auto text-caption text-text-tertiary">symlink</span>
        </div>
      </div>

      <div className="mt-3 flex flex-col gap-1.5 border-t border-border-subtle pt-3">
        <div className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
          Invocation
        </div>
        <div className="flex items-center justify-between gap-4">
          <div className="flex min-w-0 items-center gap-2 text-small">
            <HarnessIcon harness="shared" size={16} />
            <span className="min-w-0 truncate text-text-primary">Shared ({sharedPath})</span>
          </div>
          <InvocationControl selected="both" />
        </div>
        <div className="flex items-center justify-between gap-4">
          <div className="flex min-w-0 items-center gap-2 text-small">
            <HarnessIcon harness="claude-code" size={16} />
            <span className="min-w-0 truncate text-text-primary">Claude Code ({claudePath})</span>
          </div>
          <InvocationControl selected="both" />
        </div>
        <p className="m-0 text-caption text-text-tertiary">
          Available to both model tool calling and explicit user invocation.
        </p>
      </div>
    </div>
  );
}

export function PrototypeAssistant({ open }: { open: boolean }) {
  if (!open) return null;
  return (
    <aside
      id="skill-header-prototype-assistant"
      className="flex w-80 shrink-0 flex-col border-l border-border bg-bg-secondary p-5"
    >
      <h2 className="text-heading font-semibold text-text-primary">Assistant</h2>
      <p className="mt-2 text-body leading-[1.55] text-text-secondary">
        Ask about visual-recap, compare copies, or draft a recap block. This panel is harness chrome
        so the header toggle has somewhere to go.
      </p>
    </aside>
  );
}
