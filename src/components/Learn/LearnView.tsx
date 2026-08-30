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
  }, [section]);

  return (
    <PageShell
      title="Learn"
      actions={
        <button className="skill-page-back" onClick={() => setActiveView({ kind: "home" })}>
          ← Home
        </button>
      }
    >
      <div className="learn-grid">
        <nav className="learn-toc">
          {SECTIONS.map(({ key, title }) => (
            <a
              key={key}
              href={`#learn-${key}`}
              className="learn-nav"
              aria-current={key === section ? "true" : undefined}
            >
              {title}
            </a>
          ))}
        </nav>
        <div className="learn-body">
          <section id="learn-broken" className="learn-section">
            <h3 tabIndex={-1} ref={headingRef("broken")}>
              Broken and warnings
            </h3>
            <p>
              Skill Studio sorts every issue by what an agent experiences, not by where the issue
              comes from.
            </p>
            <h4>Broken</h4>
            <p>An agent loads nothing, or something you did not intend.</p>
            <ul>
              <li>
                <b>Dead link</b> — a symlink points at a folder that no longer exists.
              </li>
              <li>
                <b>Rejected SKILL.md</b> — the name or frontmatter breaks the agentskills.io spec,
                so the loader skips the skill.
              </li>
              <li>
                <b>Parked skill reinstalled</b> — you parked it, then an installer put it back. It
                runs again.
              </li>
            </ul>
            <h4>Warnings</h4>
            <p>Everything still loads, but the state drifted.</p>
            <ul>
              <li>
                <b>Copies differ</b> — the same skill has different content in two places, so two
                harnesses behave differently.
              </li>
              <li>
                <b>Lock file only</b> — <code>.skill-lock.json</code> lists a skill that has no
                folder on disk.
              </li>
            </ul>
          </section>

          <section id="learn-invoke" className="learn-section">
            <h3 tabIndex={-1} ref={headingRef("invoke")}>
              Who can invoke a skill
            </h3>
            <p>
              Every harness auto-invokes a skill when its description matches the task, and lets you
              call it by name. The agentskills.io spec has no field to limit either side; the limits
              below are harness extensions. Skill Studio reads the frontmatter ones, so the Home
              numbers reflect what Claude Code sees. <i>Model only</i> exists in Claude Code alone
              and is meant for background knowledge that makes no sense as a command.
            </p>
            <table>
              <thead>
                <tr>
                  <th>Harness</th>
                  <th>You invoke with</th>
                  <th>Restrict the model</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td>
                    <HarnessIcon harness="claude-code" size={13} /> Claude Code
                  </td>
                  <td>
                    <code>/name</code>
                  </td>
                  <td>
                    <code>disable-model-invocation: true</code> (you only) ·{" "}
                    <code>user-invocable: false</code> (model only)
                  </td>
                </tr>
                <tr>
                  <td>pi</td>
                  <td>
                    <code>/skill:name</code>
                  </td>
                  <td>
                    <code>disable-model-invocation: true</code>
                  </td>
                </tr>
                <tr>
                  <td>
                    <HarnessIcon harness="codex" size={13} /> Codex
                  </td>
                  <td>
                    <code>$name</code>
                  </td>
                  <td>
                    <code>agents/openai.yaml</code> →{" "}
                    <code>policy.allow_implicit_invocation: false</code>
                  </td>
                </tr>
                <tr>
                  <td>
                    <HarnessIcon harness="open-code" size={13} /> OpenCode
                  </td>
                  <td>
                    <code>skill</code> tool
                  </td>
                  <td>
                    <code>permission.skill</code> in <code>opencode.json</code>
                  </td>
                </tr>
                <tr>
                  <td>
                    <HarnessIcon harness="cursor" size={13} /> Cursor
                  </td>
                  <td>unknown</td>
                  <td>unknown</td>
                </tr>
              </tbody>
            </table>
            <p>
              Frontmatter edits the shared file, so they apply to every harness that reads the same
              folder or symlink. Codex and OpenCode settings apply to that harness only.
            </p>
          </section>

          <section id="learn-cost" className="learn-section">
            <h3 tabIndex={-1} ref={headingRef("cost")}>
              Prompt cost
            </h3>
            <p>
              A harness puts the <b>name and description</b> of every skill the model may invoke
              into the prompt, on every turn. The body of SKILL.md loads only when the skill runs.
            </p>
            <p>
              Home counts those description tokens for skills marked <i>you or the model</i> and{" "}
              <i>model only</i>. Skills marked <i>you only</i> stay out of the prompt and cost
              nothing until you run them.
            </p>
            <p>
              To lower the number: park skills you have not used, or shorten descriptions. Parking
              is the bigger lever.
            </p>
          </section>

          <section id="learn-unused" className="learn-section">
            <h3 tabIndex={-1} ref={headingRef("unused")}>
              Not used in the last 30 days
            </h3>
            <p>
              Use is read from Claude Code transcripts (<code>~/.claude/projects</code>). Other
              harnesses do not record skill invocations yet, so a skill you run only from Codex will
              show as unused.
            </p>
          </section>
        </div>
      </div>
    </PageShell>
  );
}
