import { defineConfig } from "vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  // babel() must come after react() so the compiler pass runs on already
  // JSX-transformed output, per @vitejs/plugin-react's documented ordering.
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],

  // Sentry's `component` tag (see frontend-error-report.ts) comes from a
  // function name in React's component stack; the default Oxc minifier
  // mangles those to single letters (e.g. `Kc`), which would make the tag
  // useless in the packaged app. `mangle.keepNames` preserves only function
  // and class declaration names - everything else (locals, params) is still
  // mangled - so this costs far less bundle size than turning mangling off.
  // The compressor has its own `keepNames`: without it a named function
  // expression (`forwardRef(function Name ..)`) loses its name before the
  // mangler ever sees it.
  build: {
    rolldownOptions: {
      output: {
        minify: {
          compress: { keepNames: { function: true, class: true } },
          mangle: { keepNames: { function: true, class: true } },
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
