// ============================================================================
// Skill Studio - frontend-error-report tests
// Guards the privacy promise `report_frontend_error` (Rust) depends on: this
// module must never hand it an error message, a stack, or a URL - only a
// component name and an error kind, both plain identifiers. `mockIPC` stands
// in for the real Tauri bridge without mocking this module itself, matching
// `skill-api-error.test.ts`.
// ============================================================================

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import {
  componentNameFromStack,
  installWindowErrorReporting,
  reportBoundaryError,
  reportFrontendError,
  resetFrontendErrorReportCountForTest,
} from "./frontend-error-report";

// This suite runs under Vitest's plain Node environment (no jsdom in this
// workspace) - `@tauri-apps/api/mocks` only needs a `window` object to hang
// `__TAURI_INTERNALS__` off, so `globalThis` stands in for it rather than
// pulling in a DOM implementation this file doesn't otherwise need.
// SAFETY: no DOM lib is loaded in this Node test environment, so `window`
// is never declared - assigning `globalThis` under that name is additive,
// not a narrowing of an existing value.
(globalThis as { window?: typeof globalThis }).window = globalThis;

beforeEach(() => {
  resetFrontendErrorReportCountForTest();
});

afterEach(() => {
  clearMocks();
});

describe("componentNameFromStack", () => {
  it("component_name_comes_from_the_first_frame_of_the_component_stack", () => {
    const componentStack =
      "\n    at SkillList (http://localhost:1420/src/components/SkillList.tsx:12:3)\n    at App";
    expect(componentNameFromStack(componentStack)).toBe("SkillList");
    expect(componentNameFromStack(null)).toBe("unknown");
    expect(componentNameFromStack(undefined)).toBe("unknown");
  });

  it("component_name_comes_from_a_webkit_frame_without_the_at_keyword", () => {
    // The desktop app runs in WKWebView (JavaScriptCore), which copies
    // native frames as-is: "Name@url:line:col", with no "at " keyword.
    const componentStack =
      "\nSkillList@tauri://localhost/assets/index-abc.js:1:2345\nApp@tauri://localhost/x.js:3:4";
    expect(componentNameFromStack(componentStack)).toBe("SkillList");
    expect(componentNameFromStack("\n@tauri://localhost/x.js:1:1")).toBe("unknown");
  });

  it("an_anonymous_first_frame_is_not_reported_under_its_parent_name", () => {
    // Only the first frame is the failing component. A match that may skip
    // to the next line would tag an anonymous component's error as `App`.
    const componentStack = "\n@tauri://localhost/x.js:1:1\nApp@tauri://localhost/x.js:3:4";
    expect(componentNameFromStack(componentStack)).toBe("unknown");
  });
});

describe("reportFrontendError", () => {
  it("an_error_message_never_reaches_the_command", async () => {
    const calls: { cmd: string; args: unknown }[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
    });

    // Exercises the real ErrorBoundary path (`reportBoundaryError`, which
    // `main.tsx`'s `componentDidCatch` calls) rather than
    // `reportFrontendError` directly, with both a sensitive error message
    // and a sensitive component stack.
    reportBoundaryError(new Error("/Users/alice/.claude/skills/my-skill"), {
      componentStack: "\nSkillList@tauri://localhost/a.js:1:1",
    });

    expect(calls).toHaveLength(1);
    expect(calls[0]?.cmd).toBe("report_frontend_error");
    expect(calls[0]?.args).toEqual({ component: "SkillList", kind: "Error" });
    expect(JSON.stringify(calls[0]?.args)).not.toContain("alice");
  });

  it("reports_stop_after_twenty_in_one_session", () => {
    const calls: unknown[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
    });

    for (let index = 0; index < 25; index += 1) {
      reportFrontendError("SkillList", "Error");
    }

    expect(calls).toHaveLength(20);
  });
});

describe("installWindowErrorReporting", () => {
  it("window_listeners_report_error_and_rejection_kinds", () => {
    const calls: { cmd: string; args: unknown }[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
    });

    // The two event shapes `installWindowErrorReporting` actually reads:
    // an `ErrorEvent`-like `{ error }` and a `PromiseRejectionEvent`-like
    // `{ reason }`. Both fields are always present (never optional) so this
    // one shape satisfies both of `ErrorReportingTarget`'s listener
    // parameter types without a cast.
    interface FakeWindowEvent {
      error: Error | undefined;
      reason: Error | undefined;
    }
    type FakeListener = (event: FakeWindowEvent) => void;
    const listeners = new Map<string, FakeListener[]>();
    // Method shorthand (not arrow-function properties): TypeScript checks a
    // method's parameters bivariantly, which is what lets this narrower
    // `FakeWindowEvent` satisfy `ErrorReportingTarget`'s `WindowErrorEvent` /
    // `WindowRejectionEvent` listener parameters without a cast.
    const fakeWindow = {
      addEventListener(type: string, listener: FakeListener) {
        const forType = listeners.get(type) ?? [];
        forType.push(listener);
        listeners.set(type, forType);
      },
      removeEventListener(type: string, listener: FakeListener) {
        listeners.set(
          type,
          (listeners.get(type) ?? []).filter((l) => l !== listener),
        );
      },
      dispatchEvent(type: string, event: FakeWindowEvent) {
        for (const listener of listeners.get(type) ?? []) listener(event);
      },
    };

    const remove = installWindowErrorReporting(fakeWindow);

    fakeWindow.dispatchEvent("error", {
      error: new TypeError("boom at /Users/alice/x"),
      reason: undefined,
    });
    fakeWindow.dispatchEvent("unhandledrejection", {
      error: undefined,
      reason: new RangeError("also a path"),
    });

    expect(calls).toHaveLength(2);
    expect(calls[0]?.args).toEqual({ component: "window", kind: "TypeError" });
    expect(calls[1]?.args).toEqual({ component: "promise", kind: "RangeError" });

    remove();
    fakeWindow.dispatchEvent("error", { error: new Error("after removal"), reason: undefined });
    expect(calls).toHaveLength(2);
  });
});
