// ============================================================================
// ActivityDayDetails - Everything about one day: how it compares, when it
// happened, the harness and trigger split, every skill, and every project.
// One column in a narrow panel; two columns once its container is 40rem wide.
// ============================================================================

import type { ReactNode } from "react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import {
  basename,
  dayDetail,
  deploymentLabelFromAgentId,
  formatCount,
  formatDay,
  ranked,
  shiftDay,
  usualDay,
  uses,
} from "@skill-studio/lib";
import type { ActivityFilter, AgentId, SkillInvocationStats } from "@skill-studio/lib";
import { Button } from "@skill-studio/ui";
import { HarnessIcon } from "../ui/HarnessIcon";
import { TooltipControl } from "../ui/TooltipControl";
import { CappedList, Muted } from "./ActivityParts";
import { HourStrip } from "./ActivityHourStrip";
import { PanelLabel, Splits } from "./ActivitySplits";

function compared(total: number, usual: number): string | null {
  if (total === 0 || usual === 0) return null;
  const ratio = total / usual;
  if (ratio >= 1.5) return `${Number(ratio.toFixed(1))}× a usual day`;
  if (ratio <= 0.6) return "Quieter than a usual day";
  return "About a usual day";
}

function IconButton({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <TooltipControl content={label}>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={label}
        disabled={disabled}
        className="text-text-tertiary"
        onClick={onClick}
      >
        {children}
      </Button>
    </TooltipControl>
  );
}

interface ActivityDayDetailsProps {
  stats: SkillInvocationStats[];
  dayKey: string;
  filter: ActivityFilter;
  /** Per-day totals under the same filter, for the "usual day" line. */
  days: Record<string, number>;
  dates: string[];
  onChangeDay: (dayKey: string | null) => void;
  onOpenSkill: (name: string) => void;
}

export function ActivityDayDetails({
  stats,
  dayKey,
  filter,
  days,
  dates,
  onChangeDay,
  onOpenSkill,
}: ActivityDayDetailsProps) {
  const day = dayDetail(stats, dayKey, filter);
  const usual = usualDay(days);
  const skills = ranked(day.bySkill);
  const projects = ranked(day.byProject);
  const prev = shiftDay(dates, dayKey, -1);
  const next = shiftDay(dates, dayKey, 1);
  const comparison = compared(day.total, usual);

  const summary =
    day.total === 0
      ? "No uses"
      : `${uses(day.total)} · ${skills.length} ${skills.length === 1 ? "skill" : "skills"}`;

  return (
    <div className="@container flex flex-col gap-5">
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h2 className="text-body font-medium text-text-primary">{formatDay(dayKey)}</h2>
          <Muted>{summary}</Muted>
          {comparison && (
            <span className="text-small tabular-nums text-text-quaternary">{comparison}</span>
          )}
        </div>
        <div className="-mr-1.5 flex shrink-0 items-center">
          <IconButton label="Previous day" disabled={!prev} onClick={() => onChangeDay(prev)}>
            <ChevronLeft size={14} aria-hidden="true" />
          </IconButton>
          <IconButton label="Next day" disabled={!next} onClick={() => onChangeDay(next)}>
            <ChevronRight size={14} aria-hidden="true" />
          </IconButton>
          <IconButton label="Close details" onClick={() => onChangeDay(null)}>
            <X size={14} aria-hidden="true" />
          </IconButton>
        </div>
      </div>

      {day.total > 0 && (
        <div className="grid gap-x-10 gap-y-5 @[40rem]:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)]">
          <div className="flex min-w-0 flex-col gap-5">
            <HourStrip byHour={day.byHour} />
            <Splits b={day} filter={filter} />
          </div>

          <div className="flex min-w-0 flex-col gap-5">
            <div className="flex flex-col gap-1.5">
              <PanelLabel>Skills</PanelLabel>
              <CappedList
                items={skills}
                moreClassName="-ml-2 px-2"
                render={([skill, n]) => {
                  const harnesses = day.skillHarnesses[skill] ?? [];
                  return (
                    <Button
                      key={skill}
                      variant="ghost"
                      size="sm"
                      className="-mx-2 justify-between gap-3 px-2 font-normal"
                      onClick={() => onOpenSkill(skill)}
                    >
                      <span className="truncate text-text-secondary">{skill}</span>
                      <span className="flex shrink-0 items-center gap-2.5">
                        {filter.harness === "all" && (
                          <span
                            role="img"
                            aria-label={harnesses
                              .map((id) => deploymentLabelFromAgentId(id))
                              .join(", ")}
                            className="flex items-center gap-1"
                          >
                            {harnesses.map((id) => (
                              // SAFETY: harness ids come off the wire as plain strings; every one
                              // this page can show is a known AgentId.
                              <HarnessIcon key={id} harness={id as AgentId} size={12} />
                            ))}
                          </span>
                        )}
                        <span className="min-w-6 text-right">
                          <Muted>{formatCount(n)}</Muted>
                        </span>
                      </span>
                    </Button>
                  );
                }}
              />
            </div>

            {projects.length > 0 && (
              <div className="flex flex-col gap-1.5">
                <PanelLabel>Projects</PanelLabel>
                <div className="flex flex-col">
                  {projects.map(([project, n]) => (
                    <TooltipControl key={project} content={[{ text: project, mono: true }]}>
                      <div className="flex h-7 items-center justify-between gap-3">
                        <span className="truncate text-body text-text-secondary">
                          {basename(project)}
                        </span>
                        <Muted>{formatCount(n)}</Muted>
                      </div>
                    </TooltipControl>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
