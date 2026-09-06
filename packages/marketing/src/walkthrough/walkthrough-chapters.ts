export const chapters = [
  {
    id: "map",
    title: "One skill. Different copies.",
    copy: "Your project has its own commit instructions. See them beside the global copy, check which agents can read each, and compare what changed.",
  },
  {
    id: "repair",
    title: "Fix a broken skill link.",
    copy: "A project skill points to a folder that no longer exists. Re-link it to a healthy copy and read its instructions again.",
  },
  {
    id: "install",
    title: "Try a skill for a day.",
    copy: "Install a skill in your project for 24 hours. Keep it if it helps. Skill Studio removes the trial if you don't.",
  },
  {
    id: "activity",
    title: "See which skills run.",
    copy: "See which skills Claude Code used, in which projects, and how recently.",
  },
] as const;

export interface WalkthroughProps {
  theme: "dark" | "light";
}
