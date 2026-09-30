# Get started

Skill Studio shows every agent skill on your Mac in one list. Use it to find broken, duplicate and unused skills, then fix or park them. You can undo each change.

## Install

1. Download the latest release from [GitHub](https://github.com/sergical/skill-studio/releases).
2. Move Skill Studio to your Applications folder.
3. Open Skill Studio.

Skill Studio needs macOS 13 or later. It runs on Apple Silicon and Intel Macs.

## First look

The app reads the skill folders of Claude Code, Codex, OpenCode, Cursor, pi and Grok Build, and the shared `.agents/skills` folder. It shows each skill once, with:

- The agents that load it.
- Its scope: global, or a project.
- When an agent last used it, or "Never used".
- Any problem, for example a broken link or bad frontmatter.

## Fix a broken skill

A skill with a problem shows a warning. Select the skill, then select **Fix**. Skill Studio repairs what it can, for example a broken link or bad frontmatter. If it cannot fix a problem, it shows the path to the file, so you can fix it yourself.

## Park and unpark

Park a skill to hide it from every agent. Skill Studio does not delete it.

1. Select the skill.
2. Select **Park**.

To use the skill again, select **Unpark**.

## Undo

Each change goes into a history.

1. Open **Activity**.
2. Find the change in **History**.
3. Select **Restore**.

To undo a park, select **Unpark**.

## Install from skills.sh

You can browse and install thousands of skills from skills.sh.

1. Select **Add skill**.
2. Select **Browse skills.sh**.
3. Find a skill, then install it.

Skill Studio installs skills with the `npx skills` command, so the skills stay in step with the `skills` lock file.

## Privacy

- You do not need an account.
- Skill Studio runs on your Mac.
- It goes online only to browse and install skills, check for updates and send crash reports. Skill browsing goes through the Skill Studio server. Update checks go to GitHub.
- It sends anonymous crash reports. They do not include skill names, files or paths.
- To turn crash reports off, open **Settings**, then **Telemetry**. You can also turn them off on the first screen.

## Next steps

- [Use the CLI](/docs/cli/) to check and fix skills from the terminal.
- [Connect the MCP server](/docs/mcp/) so your agent can tidy its own skills.
