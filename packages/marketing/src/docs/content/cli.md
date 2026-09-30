# CLI

The Skill Studio CLI does the same jobs as the app, from the terminal. It uses the same history, so you can undo a CLI change in the app.

## Run it

The CLI needs Node.js and macOS. Run it with `npx`. You do not need to install it first.

```sh
npx skill-studio diagnose
```

To see all commands:

```sh
npx skill-studio --help
```

## Check your skills

List every skill, with its agents and an id for each copy:

```sh
npx skill-studio scan
```

Find problems, for example broken links or bad frontmatter:

```sh
npx skill-studio diagnose
```

Find skills with the same name in more than one place:

```sh
npx skill-studio conflicts
```

## Find unused skills

List the skills that no agent used in the last 30 days. Never-used skills come first.

```sh
npx skill-studio usage
```

To use a different number of days:

```sh
npx skill-studio usage --days 7
```

## Fix a skill

```sh
npx skill-studio fix --skill my-skill
```

The CLI repairs what it can. If it cannot fix a problem, it prints the path to the file, so you can fix it yourself.

## Park and unpark

Park a skill to hide it from every agent. The CLI does not delete it.

```sh
npx skill-studio park my-skill
```

To use the skill again:

```sh
npx skill-studio unpark my-skill
```

If more than one copy has the same name, the CLI lists each copy with its path and id. Run the command again with the id:

```sh
npx skill-studio park --id <id>
```

## Turn a skill off for one agent

```sh
npx skill-studio disable my-skill --agent codex
```

To turn it on again:

```sh
npx skill-studio enable my-skill --agent codex
```

The agent can be `claude-code`, `codex` or `opencode`. For other agents, park the skill.

## Undo

Undo the last change:

```sh
npx skill-studio undo
```

To undo an older change, list the history, then restore one event:

```sh
npx skill-studio events
npx skill-studio restore --event-id <id>
```

## Keep skills current

Find skills that have a newer version:

```sh
npx skill-studio outdated
```

Update a skill that you installed from skills.sh:

```sh
npx skill-studio update --skill my-skill --method skills-sh
```

## Add and remove

Install a skill from a GitHub repository:

```sh
npx skill-studio add owner/repo --name my-skill
```

Remove a skill:

```sh
npx skill-studio remove my-skill
```

## Use it in scripts

Add `--json` to get output that a script can read:

```sh
npx skill-studio diagnose --json
```

`scan`, `diagnose` and `fix` exit with code 1 when they find a problem. Other errors use codes 2 and higher.
