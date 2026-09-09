// ============================================================================
// Skill Studio - installed-skills-refresh tests
// Verifies a `projects` change refreshes the installed list only - never the
// Browse tab's popular/search results - and that a superseded fetch can't
// overwrite a newer one.
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import { startInstalledSkillsRefresh } from "./installed-skills-refresh";
import type { InstalledSkill } from "@skill-studio/lib";

describe("startInstalledSkillsRefresh", () => {
  it("forwards the fetched installed list to onInstalled verbatim", async () => {
    const fetched: InstalledSkill[] = [];
    let resolveFetch: ((value: InstalledSkill[]) => void) | undefined;
    const fetchInstalledSkills = vi.fn(
      (): Promise<InstalledSkill[]> =>
        new Promise((resolve) => {
          resolveFetch = resolve;
        }),
    );
    const onInstalled = vi.fn();

    startInstalledSkillsRefresh([], fetchInstalledSkills, {
      onInstalled,
      onError: () => {
        throw new Error("onError should not run on a successful fetch");
      },
    });

    resolveFetch?.(fetched);
    await Promise.resolve();
    await Promise.resolve();

    expect(fetchInstalledSkills).toHaveBeenCalledWith([]);
    expect(onInstalled).toHaveBeenCalledTimes(1);
    expect(onInstalled).toHaveBeenCalledWith(fetched);
  });

  it("does not call onInstalled when cancelled before the fetch resolves", async () => {
    let resolveFetch: ((value: InstalledSkill[]) => void) | undefined;
    const onInstalled = vi.fn();
    const cancel = startInstalledSkillsRefresh(
      [],
      () =>
        new Promise((resolve) => {
          resolveFetch = resolve;
        }),
      { onInstalled, onError: () => {} },
    );

    cancel();
    resolveFetch?.([]);
    await Promise.resolve();
    await Promise.resolve();

    expect(onInstalled).not.toHaveBeenCalled();
  });

  it("forwards an Error's message to onError", async () => {
    let rejectFetch: ((error: Error) => void) | undefined;
    const onError = vi.fn();
    startInstalledSkillsRefresh(
      [],
      () =>
        new Promise<InstalledSkill[]>((_resolve, reject) => {
          rejectFetch = reject;
        }),
      { onInstalled: () => {}, onError },
    );

    rejectFetch?.(new Error("boom"));
    await Promise.resolve();
    await Promise.resolve();

    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith("boom");
  });

  it("does not call onError when cancelled before the fetch rejects", async () => {
    let rejectFetch: ((error: Error) => void) | undefined;
    const onError = vi.fn();
    const cancel = startInstalledSkillsRefresh(
      [],
      () =>
        new Promise<InstalledSkill[]>((_resolve, reject) => {
          rejectFetch = reject;
        }),
      { onInstalled: () => {}, onError },
    );

    cancel();
    rejectFetch?.(new Error("boom"));
    await Promise.resolve();
    await Promise.resolve();

    expect(onError).not.toHaveBeenCalled();
  });
});
