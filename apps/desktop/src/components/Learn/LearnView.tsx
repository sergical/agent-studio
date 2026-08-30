// ============================================================================
// LearnView - Explainer sections for Home's "why does this matter" links:
// broken installs, invocation policy, prompt cost, unused skills. Content is
// copied verbatim from popover-spec.md's `LEARN` object.
// ============================================================================

import { useEffect, useRef } from "react";
import { HarnessIcon } from "../ui/HarnessIcon";
import { PageShell } from "../Shell/PageShell";
import { useAppStore } from "../../store/appStore";
import type { LearnSection } from "../../store/appStore";

interface LearnViewProps {
  section?: LearnSection;
}

const SECTIONS: { key: LearnSection; title: string }[] = [
  { key: "broken", title: "Broken and warnings" },
  { key: "invoke", title: "Who can invoke a skill" },
  { key: "cost", title: "Prompt cost" },
  { key: "unused", title: "Not used in the last 30 days" },
];

/**
 * The four explainer sections behind Home's "Learn more →" links. When
 * opened at a given `section`, that section scrolls into view and takes
 * focus so a screen reader lands on the content the user asked for, not the
 * page top.
 */
export function LearnView({ section }: LearnViewProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);
  const headingRefs = useRef(new Map<LearnSection, HTMLHeadingElement>());
  const headingRef = (key: LearnSection) => (el: HTMLHeadingElement | null) => {
    if (el) headingRefs.current.set(key, el);
  };

  useEffect(() => {
    if (!section) return;
    const heading = headingRefs.current.get(section);
    heading?.scrollIntoView({ block: "start" });
    heading?.focus();
  }, [section, headingRefs]);

  return (
    <PageShell
      title="Learn"
      actions={
        <button
          className="flex shrink-0 items-center gap-1.5 p-1 text-small text-text-tertiary transition-colors hover:text-text-primary"
          onClick={() => setActiveView({ kind: "home" })}
        >
          ← Home
        </button>
      }
    >
      <div className="grid grid-cols-[180px_minmax(0,1fr)] items-start gap-8">
        <nav className="sticky top-6 flex flex-col gap-0.5">
          {SECTIONS.map(({ key, title }) => (
            <a
              key={key}
              href={`#learn-${key}`}
              className={`rounded-sm px-2.5 py-1.5 text-body text-text-secondary no-underline hover:bg-bg-hover hover:text-text-primary ${
                key === section ? "bg-accent-softer text-accent" : ""
              }`}
              aria-current={key === section ? "true" : undefined}
            >
              {title}
            </a>
          ))}
        </nav>
        <div className="flex max-w-[65ch] flex-col gap-9 text-body leading-[1.55] text-text-secondary">
          <section id="learn-broken" className="flex scroll-mt-6 flex-col gap-2">
            <h3
              className="m-0 mb-1 text-heading font-semibold text-text-primary"
              tabIndex={-1}
              ref={headingRef("broken")}
            >
              Broken and warnings
            </h3>
            <p className="m-0">
              Skill Studio sorts every issue by what an agent experiences, not by where the issue
              comes from.
            </p>
            <h4 className="mt-2 mb-0 text-body font-semibold text-text-primary">Broken</h4>
            <p className="m-0">An agent loads nothing, or something you did not intend.</p>
            <ul className="m-0 flex flex-col gap-1 pl-4.5">
              <li>
                <b className="font-medium text-text-primary">Dead link</b> — a symlink points at a
                folder that no longer exists.
              </li>
              <li>
                <b className="font-medium text-text-primary">Rejected SKILL.md</b> — the name or
                frontmatter breaks the agentskills.io spec, so the loader skips the skill.
              </li>
              <li>
                <b className="font-medium text-text-primary">Parked skill reinstalled</b> — you
                parked it, then an installer put it back. It runs again.
              </li>
            </ul>
            <h4 className="mt-2 mb-0 text-body font-semibold text-text-primary">Warnings</h4>
            <p className="m-0">Everything still loads, but the state drifted.</p>
            <ul className="m-0 flex flex-col gap-1 pl-4.5">
              <li>
                <b className="font-medium text-text-primary">Copies differ</b> — the same skill has
                different content in two places, so two harnesses behave differently.
              </li>
              <li>
                <b className="font-medium text-text-primary">Lock file only</b> —{" "}
                <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                  .skill-lock.json
                </code>{" "}
                lists a skill that has no folder on disk.
              </li>
            </ul>
          </section>

          <section id="learn-invoke" className="flex scroll-mt-6 flex-col gap-2">
            <h3
              className="m-0 mb-1 text-heading font-semibold text-text-primary"
              tabIndex={-1}
              ref={headingRef("invoke")}
            >
              Who can invoke a skill
            </h3>
            <p className="m-0">
              Every harness auto-invokes a skill when its description matches the task, and lets you
              call it by name. The agentskills.io spec has no field to limit either side; the limits
              below are harness extensions. Skill Studio reads the frontmatter ones, so the Home
              numbers reflect what Claude Code sees. <i>Model only</i> exists in Claude Code alone
              and is meant for background knowledge that makes no sense as a command.
            </p>
            <table className="my-1 w-full border-collapse text-small">
              <thead>
                <tr>
                  <th className="border-b border-border-subtle px-2 py-1.5 text-left align-top font-medium text-text-tertiary">
                    Harness
                  </th>
                  <th className="border-b border-border-subtle px-2 py-1.5 text-left align-top font-medium text-text-tertiary">
                    You invoke with
                  </th>
                  <th className="border-b border-border-subtle px-2 py-1.5 text-left align-top font-medium text-text-tertiary">
                    Restrict the model
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top whitespace-nowrap text-text-primary">
                    <span className="inline-flex items-center gap-1">
                      <HarnessIcon harness="claude-code" size={13} /> Claude Code
                    </span>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      /name
                    </code>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      disable-model-invocation: true
                    </code>{" "}
                    (you only) ·{" "}
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      user-invocable: false
                    </code>{" "}
                    (model only)
                  </td>
                </tr>
                <tr>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top whitespace-nowrap text-text-primary">
                    pi
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      /skill:name
                    </code>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      disable-model-invocation: true
                    </code>
                  </td>
                </tr>
                <tr>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top whitespace-nowrap text-text-primary">
                    <span className="inline-flex items-center gap-1">
                      <HarnessIcon harness="codex" size={13} /> Codex
                    </span>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      $name
                    </code>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      agents/openai.yaml
                    </code>{" "}
                    →{" "}
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      policy.allow_implicit_invocation: false
                    </code>
                  </td>
                </tr>
                <tr>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top whitespace-nowrap text-text-primary">
                    <span className="inline-flex items-center gap-1">
                      <HarnessIcon harness="open-code" size={13} /> OpenCode
                    </span>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      skill
                    </code>{" "}
                    tool
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      permission.skill
                    </code>{" "}
                    in{" "}
                    <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                      opencode.json
                    </code>
                  </td>
                </tr>
                <tr>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top whitespace-nowrap text-text-primary">
                    <span className="inline-flex items-center gap-1">
                      <HarnessIcon harness="cursor" size={13} /> Cursor
                    </span>
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    unknown
                  </td>
                  <td className="border-b border-border-subtle px-2 py-1.5 text-left align-top">
                    unknown
                  </td>
                </tr>
              </tbody>
            </table>
            <p className="m-0">
              Frontmatter edits the shared file, so they apply to every harness that reads the same
              folder or symlink. Codex and OpenCode settings apply to that harness only.
            </p>
          </section>

          <section id="learn-cost" className="flex scroll-mt-6 flex-col gap-2">
            <h3
              className="m-0 mb-1 text-heading font-semibold text-text-primary"
              tabIndex={-1}
              ref={headingRef("cost")}
            >
              Prompt cost
            </h3>
            <p className="m-0">
              A harness puts the{" "}
              <b className="font-medium text-text-primary">name and description</b> of every skill
              the model may invoke into the prompt, on every turn. The body of SKILL.md loads only
              when the skill runs.
            </p>
            <p className="m-0">
              Home counts those description tokens for skills marked <i>you or the model</i> and{" "}
              <i>model only</i>. Skills marked <i>you only</i> stay out of the prompt and cost
              nothing until you run them.
            </p>
            <p className="m-0">
              To lower the number: park skills you have not used, or shorten descriptions. Parking
              is the bigger lever.
            </p>
          </section>

          <section id="learn-unused" className="flex scroll-mt-6 flex-col gap-2">
            <h3
              className="m-0 mb-1 text-heading font-semibold text-text-primary"
              tabIndex={-1}
              ref={headingRef("unused")}
            >
              Not used in the last 30 days
            </h3>
            <p className="m-0">
              Use is read from Claude Code transcripts (
              <code className="rounded-[3px] bg-bg-tertiary px-1 py-px font-mono text-small text-text-primary">
                ~/.claude/projects
              </code>
              ). Other harnesses do not record skill invocations yet, so a skill you run only from
              Codex will show as unused.
            </p>
          </section>
        </div>
      </div>
    </PageShell>
  );
}
