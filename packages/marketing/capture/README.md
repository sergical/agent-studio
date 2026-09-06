# Product video captures

These captures render the current desktop React app with fictional data through Tauri's IPC mocks. They do not run the native backend or change installed skills. The marketing build does not include this entry point.

Use the repository's dev-server workflow to run the desktop Vite server at `https://skill-studio.localhost`, then run from the repository root:

```sh
npm run remotion:capture -w @skill-studio/marketing
npm run remotion:render -w @skill-studio/marketing
```

The capture command requires `agent-browser`. Set `SKILL_STUDIO_CAPTURE_URL` to use another capture page URL. It records real controls at 1040×1000 and 800×1000. Remotion uses separate camera paths for desktop and phone playback.

After an app UI change, capture again and check the actions, states, and camera crops before rendering. `capture/public/remotion/current` contains source frames and is used only by Remotion. `public/walkthrough/current` contains the delivered MP4s and posters.

Run `npx tsc -p packages/marketing/capture/tsconfig.json --noEmit` to check the fixtures against the current app types.
