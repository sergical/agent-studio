export const chapters = [
  {
    id: "map",
    title: "One skill, two copies.",
    copy: "Your project has its own copy of a skill. See it next to the global copy, see which agents read each one, and compare what changed.",
  },
  {
    id: "repair",
    title: "Fix a broken skill.",
    copy: "A project skill points to a folder that no longer exists. Link it to a healthy copy in one click.",
  },
  {
    id: "install",
    title: "Park a skill you don't need yet.",
    copy: "Hide it from all your agents without deleting it. Unpark it to bring it back.",
  },
  {
    id: "activity",
    title: "See which skills run.",
    copy: "See which skills your agents used, in which projects, and how recently.",
  },
] as const;

export interface WalkthroughProps {
  theme: "dark" | "light";
}
