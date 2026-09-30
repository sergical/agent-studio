import * as stylex from "@stylexjs/stylex";
import { CircleDashed, Copy, Link2Off, type LucideIcon } from "lucide-react";

import { siteTokens } from "../SiteTheme.stylex";
import { homeSectionStyles as section } from "./HomeSection.stylex";

const problems: ReadonlyArray<{ icon: LucideIcon; text: string }> = [
  { icon: Link2Off, text: "Some point to folders that no longer exist." },
  { icon: Copy, text: "Some are copies of each other that have drifted apart." },
  {
    icon: CircleDashed,
    text: "Most never run, yet each one adds its name and description to every session.",
  },
];

export function ProblemSection() {
  return (
    <>
      <h2 {...stylex.props(section.title)}>Skills pile up.</h2>
      <ul {...stylex.props(styles.list)}>
        {problems.map(({ icon: Icon, text }) => (
          <li key={text} {...stylex.props(styles.item)}>
            <span aria-hidden="true" {...stylex.props(styles.icon)}>
              <Icon size={18} />
            </span>
            <p {...stylex.props(styles.text)}>{text}</p>
          </li>
        ))}
      </ul>
    </>
  );
}

const styles = stylex.create({
  list: {
    display: "grid",
    gap: 32,
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    listStyle: "none",
    margin: 0,
    padding: 0,
    "@media (max-width: 900px)": { gap: 0, gridTemplateColumns: "1fr" },
  },
  item: {
    alignItems: "flex-start",
    borderTopColor: siteTokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
    flexDirection: "column",
    gap: 16,
    paddingTop: 22,
    "@media (max-width: 900px)": {
      flexDirection: "row",
      gap: 16,
      paddingBlock: 18,
    },
  },
  icon: {
    alignItems: "center",
    backgroundColor: siteTokens.accentSoft,
    borderRadius: 9,
    color: siteTokens.accent,
    display: "flex",
    flexShrink: 0,
    height: 36,
    justifyContent: "center",
    width: 36,
  },
  text: {
    color: siteTokens.text,
    fontSize: 17,
    lineHeight: 1.5,
    margin: 0,
    maxWidth: "34ch",
    textWrap: "pretty",
    "@media (max-width: 900px)": { alignSelf: "center", maxWidth: "none" },
  },
});
