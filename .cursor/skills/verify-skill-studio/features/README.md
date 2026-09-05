# Skill Studio Feature Map

This directory documents Skill Studio's core user-facing features for verification testing. Each feature file describes what it is, how to access it, how to drive it with Playwright, and observable outcomes that prove it works.

## Features Mapped

1. **[Home Dashboard](./home-dashboard.md)** - Central hub showing skill health stats, inbox of issues, and recent activity
2. **[Skills Management](./skills-management.md)** - Browse, filter, search, and manage installed skills across all agents
3. **[Skill Detail View](./skill-detail.md)** - View, edit, test, and manage individual skill deployments
4. **[Activity Tracking](./activity-tracking.md)** - View invocation history, usage heatmap, and cost analytics
5. **[Settings](./settings.md)** - Configure app preferences, theme, and skill directories

## Coverage

These 5 features represent the primary user workflows in Skill Studio. Additional features (Packs, Learn, Plugins) are secondary views that follow similar patterns.

## Testing Strategy

Each feature should be verified by:
1. Navigating to it via the sidebar
2. Checking for key UI elements (headings, buttons, data)
3. Performing a representative action (filter, click, edit)
4. Observing the result (UI update, modal, navigation)
5. Capturing screenshots as evidence

## Feature Dependencies

- **Skills Management** depends on having at least one skill installed
- **Activity Tracking** depends on having invocation history (may be empty on fresh install)
- **Home Dashboard** aggregates data from Skills + Activity

For testing on a fresh environment, consider seeding test skills in `~/.agents/skills/` or using the app's install flow.
