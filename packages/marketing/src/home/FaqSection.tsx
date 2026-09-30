import type { ReactNode } from "react";
import * as stylex from "@stylexjs/stylex";
import { ChevronDown } from "lucide-react";

import { siteTokens } from "../SiteTheme.stylex";
import { DOCS_URL } from "../site-links";
import { homeSectionStyles as section } from "./HomeSection.stylex";

function Code({ children }: { children: ReactNode }) {
  return <code {...stylex.props(section.code)}>{children}</code>;
}

const questions: ReadonlyArray<{ question: string; answer: ReactNode }> = [
  {
    question: "Which agents does it work with?",
    answer: (
      <>
        Claude Code, Codex, OpenCode, pi, Cursor and Grok Build, plus the shared{" "}
        <Code>.agents</Code> folder. It finds skills globally and in the projects you work in.
      </>
    ),
  },
  {
    question: "Does parking delete a skill?",
    answer: "No. The skill moves to a parked folder, and you can unpark it at any time.",
  },
  {
    question: "Can I undo a change?",
    answer:
      "Yes. Changes go into a history you can undo. Skills you remove go to a quarantine first.",
  },
  {
    question: "Does it work with npx skills?",
    answer: (
      <>
        Yes. It installs, updates and removes skills through <Code>npx skills</Code> and reads the
        lock file. It also works with dotagents.
      </>
    ),
  },
  {
    question: "Can I turn a skill off for one agent only?",
    answer: "Yes, in Claude Code, Codex and OpenCode.",
  },
  {
    question: "Does it send my data anywhere?",
    answer:
      "No account is needed and it runs on your Mac. It sends anonymous crash reports, which you can turn off. They never include skill names, files or paths.",
  },
  {
    question: "Is it free?",
    answer: "Yes. It is open source under the MIT license.",
  },
];

export function FaqSection() {
  return (
    <div {...stylex.props(styles.layout)}>
      <div>
        <h2 {...stylex.props(section.title, styles.title)}>Questions.</h2>
        <a href={DOCS_URL} {...stylex.props(section.textLink)}>
          Read the docs
          <span aria-hidden="true">→</span>
        </a>
      </div>
      <div {...stylex.props(styles.list)}>
        {questions.map(({ question, answer }) => (
          <details key={question} {...stylex.props(stylex.defaultMarker(), styles.item)}>
            <summary {...stylex.props(styles.summary)}>
              {question}
              <ChevronDown aria-hidden="true" size={14} {...stylex.props(styles.chevron)} />
            </summary>
            <p {...stylex.props(styles.answer)}>{answer}</p>
          </details>
        ))}
      </div>
    </div>
  );
}

const styles = stylex.create({
  layout: {
    alignItems: "start",
    display: "grid",
    gap: 64,
    gridTemplateColumns: "300px minmax(0, 1fr)",
    "@media (min-width: 761px) and (max-width: 1000px)": {
      gap: 32,
      gridTemplateColumns: "260px minmax(0, 1fr)",
    },
    "@media (max-width: 760px)": { gap: 0, gridTemplateColumns: "1fr" },
  },
  title: {
    marginBottom: 12,
    "@media (max-width: 760px)": { marginBottom: 8 },
  },
  list: {
    borderTopColor: siteTokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    "@media (max-width: 760px)": { marginTop: 20 },
  },
  item: {
    borderBottomColor: siteTokens.border,
    borderBottomStyle: "solid",
    borderBottomWidth: 1,
  },
  summary: {
    alignItems: "center",
    color: siteTokens.text,
    cursor: "pointer",
    display: "flex",
    fontSize: 16,
    fontWeight: 550,
    gap: 12,
    justifyContent: "space-between",
    letterSpacing: "-.02em",
    listStyle: "none",
    minHeight: 58,
    padding: "14px 0",
    ":focus-visible": {
      outlineColor: siteTokens.muted,
      outlineOffset: 4,
      outlineStyle: "solid",
      outlineWidth: 2,
    },
  },
  chevron: {
    color: siteTokens.muted,
    flexShrink: 0,
    transform: {
      default: "none",
      [stylex.when.ancestor(":is([open])")]: "rotate(180deg)",
    },
    transition: "transform 200ms cubic-bezier(0.25, 1, 0.5, 1)",
  },
  answer: {
    color: siteTokens.muted,
    fontSize: 15,
    lineHeight: 1.6,
    margin: "0 0 22px",
    maxWidth: "62ch",
    textWrap: "pretty",
  },
});
