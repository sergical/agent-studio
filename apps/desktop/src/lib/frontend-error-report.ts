// ============================================================================
// Skill Studio - frontend-error-report
// Reports a WebView error (a React `componentDidCatch`, an uncaught `window`
// error, or an unhandled promise rejection) to the desktop's telemetry
// pipeline through one Tauri command. Never the error message, the stack, or
// a URL - `report_frontend_error` on the Rust side reduces both arguments to
// identifiers before capturing anything.
// Uses `invoke` directly rather than `callCommand` (see skill-api.ts):
// `callCommand` records timing and throws on failure, neither of which this
// fire-and-forget report needs or wants.
// ============================================================================

import { invoke } from "@tauri-apps/api/core";

// The function name of the first frame of React's componentStack, in either
// shape the desktop app can see: V8's "    at Name (url)" / "    at Name",
// and WKWebView/JavaScriptCore's "Name@url:line:col" - WKWebView copies
// native frames as-is, with no "at " keyword, so a V8-only pattern never
// matches there and every ErrorBoundary report would be tagged "unknown" in
// the real app. "unknown" when the stack is absent or has no such frame.
export function componentNameFromStack(componentStack: string | null | undefined): string {
  const match = componentStack?.match(/^\s*(?:at\s+)?([A-Za-z_$][\w$.]*)(?=\s*\(|@|\s*$)/m);
  return match?.[1] ?? "unknown";
}

// `cause.name` for `Error` instances (e.g. "TypeError"), "string" for a
// thrown string, "unknown" for anything else - never `cause.message`, which
// can quote a path or a skill name.
export function errorKind(cause: unknown): string {
  if (cause instanceof Error) return cause.name;
  if (Object.prototype.toString.call(cause) === "[object String]") return "string";
  return "unknown";
}

// Per-session cap: a render loop throwing on every frame must not turn into
// a flood of IPC calls. Mirrors the host's own per-process cap
// (`FRONTEND_ERROR_REPORT_CAP` in telemetry.rs) on the frontend side.
const SESSION_REPORT_CAP = 20;
let reportCount = 0;

// Fire-and-forget: invokes `report_frontend_error` with only `component`
// and `kind`, and swallows any rejection - a failed telemetry report must
// never surface as a second error.
export function reportFrontendError(component: string, kind: string): void {
  if (reportCount >= SESSION_REPORT_CAP) return;
  reportCount += 1;
  void invoke("report_frontend_error", { component, kind }).catch(() => {});
}

// The body of `ErrorBoundary.componentDidCatch` (main.tsx), factored out so
// a test can exercise the real path React calls rather than
// `reportFrontendError` directly.
export function reportBoundaryError(
  cause: unknown,
  info: { componentStack?: string | null },
): void {
  reportFrontendError(componentNameFromStack(info.componentStack), errorKind(cause));
}

// Test-only: resets the per-session counter to zero, so one test's cap
// doesn't leak into the next - `reportCount` is otherwise module state for
// the process's whole lifetime, same as the host's own
// `FRONTEND_ERROR_REPORT_COUNT` (`reset_frontend_error_report_count` in
// telemetry.rs).
export function resetFrontendErrorReportCountForTest(): void {
  reportCount = 0;
}

// The one field each listener below reads off a real `ErrorEvent` /
// `PromiseRejectionEvent` - narrow domain types rather than the full DOM
// event types, so a test's plain-object fake satisfies `ErrorReportingTarget`
// structurally, with no `as Window` assertion needed. A real `ErrorEvent`
// (`error: any`) and `PromiseRejectionEvent` (`reason: any`) both still
// satisfy these, so the production default below is unchanged.
interface WindowErrorEvent {
  error: unknown;
}
interface WindowRejectionEvent {
  reason: unknown;
}

interface ErrorReportingTarget {
  addEventListener(type: "error", listener: (event: WindowErrorEvent) => void): void;
  addEventListener(
    type: "unhandledrejection",
    listener: (event: WindowRejectionEvent) => void,
  ): void;
  removeEventListener(type: "error", listener: (event: WindowErrorEvent) => void): void;
  removeEventListener(
    type: "unhandledrejection",
    listener: (event: WindowRejectionEvent) => void,
  ): void;
}

// Adds `window` "error" and "unhandledrejection" listeners that report
// through `reportFrontendError`, tagged "window" and "promise"
// respectively. Returns a remover so the caller can uninstall them (tests,
// hot reload).
export function installWindowErrorReporting(target: ErrorReportingTarget = window): () => void {
  const onError = (event: WindowErrorEvent) => {
    reportFrontendError("window", errorKind(event.error));
  };
  const onRejection = (event: WindowRejectionEvent) => {
    reportFrontendError("promise", errorKind(event.reason));
  };

  target.addEventListener("error", onError);
  target.addEventListener("unhandledrejection", onRejection);

  return () => {
    target.removeEventListener("error", onError);
    target.removeEventListener("unhandledrejection", onRejection);
  };
}
