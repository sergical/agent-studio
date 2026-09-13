import React from "react";
import ReactDOM from "react-dom/client";
import { KitPreview } from "@skill-studio/ui";
import App from "./App";
import { stampInitialTheme } from "./lib/theme";

// Stamped before React renders, so the app never flashes the wrong palette
// on first paint.
stampInitialTheme();

// Simple error boundary for debugging
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean; error: Error | null }
> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error) {
    return { hasError: true, error };
  }

  componentDidCatch(_error: Error, _errorInfo: React.ErrorInfo) {
    // No toast store is reachable here: a crash this deep may mean the
    // Zustand provider itself failed to mount. The fallback UI below is
    // the only surface left to report the error on.
  }

  render() {
    if (this.state.hasError) {
      return (
        <div
          style={{
            padding: 20,
            color: "#ff6b6b",
            backgroundColor: "#1a1a1a",
            minHeight: "100vh",
            fontFamily: "monospace",
          }}
        >
          <h1>Something went wrong</h1>
          <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word", userSelect: "text" }}>
            {this.state.error?.message}
          </pre>
          <pre
            style={{
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
              fontSize: 12,
              opacity: 0.7,
              userSelect: "text",
            }}
          >
            {this.state.error?.stack}
          </pre>
        </div>
      );
    }

    return this.props.children;
  }
}

// Dev-only proof that @skill-studio/ui renders themed with the app's tokens.
// Not a real route: visit with `#kit` while running `npm run dev`.
const showKitPreview = import.meta.env.DEV && location.hash === "#kit";

async function boot(): Promise<void> {
  const rootElement = document.getElementById("root");
  if (!rootElement) {
    throw new Error("Skill Studio root element #root is missing from index.html");
  }

  // No Tauri runtime behind this tab (a plain browser dev session): install
  // the mock IPC layer so an agent-browser session can drive the app from a
  // realistic in-memory skill estate. See `dev/harness/mock-tauri.ts`.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const [{ buildHarnessSnapshot }, { installMockTauri }] = await Promise.all([
      import("./dev/harness/skill-fixture"),
      import("./dev/harness/mock-tauri"),
    ]);
    window.__harness = installMockTauri(buildHarnessSnapshot());
    // eslint-disable-next-line no-console
    console.info("[harness] mock Tauri IPC installed");
  }

  ReactDOM.createRoot(rootElement).render(
    <React.StrictMode>
      <ErrorBoundary>{showKitPreview ? <KitPreview /> : <App />}</ErrorBoundary>
    </React.StrictMode>,
  );
}

void boot();
