// @vitest-environment happy-dom

// ============================================================================
// Skill Studio - useSkillStoreData hook tests
// Verifies the Browse tab's `rawResults`, `searchQuery`, and `hasMore` survive
// a `projects` change while an active search is committed - the c0078ad
// regression. The skill-discovery API is injected as a faithful fake via the
// hook's `api` parameter (defaulting in production to the real Tauri-backed
// functions), so no module mocking or Tauri-IPC polyfill is needed. Uses
// `renderHook` from @testing-library/react under happy-dom because the bug is a
// React-effect re-firing property that can't be exercised mount-free.
// ============================================================================

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, renderHook, cleanup } from "@testing-library/react";
import { useSkillStoreData } from "./SkillStore";
import type { SkillStoreApi } from "./SkillStore";
import type {
  InstalledSkill,
  PaginatedSkillsResponse,
  SkillSearchResult,
  Toast,
} from "@skill-studio/lib";

function skill(id: string): SkillSearchResult {
  return { id, name: id, installs: 0 };
}

function page(names: string[], hasMore = false): PaginatedSkillsResponse {
  return { skills: names.map(skill), has_more: hasMore };
}

function namesOf(results: { name: string }[]): string[] {
  return results.map((r) => r.name);
}

function flushPromises(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

interface HookProps {
  projects: string[];
  api: SkillStoreApi;
  addToast: (toast: Omit<Toast, "id">) => string;
}

function initialPropsEmpty(api: SkillStoreApi, addToast: HookProps["addToast"]): HookProps {
  return { projects: [], api, addToast };
}

interface FakeApiCounters {
  installed: number;
  popular: number;
  search: number;
}

/** A faithful in-memory `SkillStoreApi` that records call counts for assertions. */
function fakeSkillStoreApi(counters: FakeApiCounters): SkillStoreApi {
  return {
    getInstalledSkills: async () => {
      counters.installed += 1;
      return [] satisfies InstalledSkill[];
    },
    getPopularSkills: async () => {
      counters.popular += 1;
      return page(["popular-a", "popular-b"]);
    },
    searchSkills: async (query: string) => {
      counters.search += 1;
      return page([`${query}-1`, `${query}-2`, `${query}-3`]);
    },
  };
}

describe("useSkillStoreData - active search survives a projects change", () => {
  let toasts: Omit<Toast, "id">[];
  let addToast: (toast: Omit<Toast, "id">) => string;

  beforeEach(() => {
    toasts = [];
    addToast = (toast: Omit<Toast, "id">): string => {
      toasts.push(toast);
      return "test";
    };
  });

  afterEach(() => {
    cleanup();
  });

  it("does not replace an active search's results when projects changes", async () => {
    const counters: FakeApiCounters = { installed: 0, popular: 0, search: 0 };
    // A single stable `api` reference across renders: in production the
    // default `realSkillStoreApi` is a module-level constant, so `api`
    // never changes identity and `loadInitialData` (deps `[api]`) stays
    // stable - mirroring that here keeps the test faithful.
    const api = fakeSkillStoreApi(counters);
    const initialProps: HookProps = initialPropsEmpty(api, addToast);

    const { result, rerender } = renderHook(
      ({ projects, api, addToast }: HookProps) => useSkillStoreData(projects, addToast, api),
      { initialProps },
    );

    await act(async () => {
      await flushPromises();
    });
    // Mount: popular + installed loaded once.
    expect(namesOf(result.current.searchResultsWithStatus)).toEqual(["popular-a", "popular-b"]);
    expect(counters.popular).toBe(1);
    expect(counters.installed).toBe(1);
    const popularCallsAfterMount = counters.popular;

    // Commit an active search.
    await act(async () => {
      await result.current.handleSearch("git");
    });
    expect(result.current.searchQuery).toBe("git");
    expect(namesOf(result.current.searchResultsWithStatus)).toEqual(["git-1", "git-2", "git-3"]);
    expect(result.current.hasMore).toBe(false);
    expect(counters.search).toBe(1);

    // `projects` changes (a new directory is added) while the Browse tab
    // stays mounted and the search bar still reads "git". This is the
    // c0078ad trigger: pre-fix, `loadInitialData` re-ran and overwrote
    // `rawResults` with popular skills here.
    rerender({ projects: ["/work/new-project"], api, addToast });
    await act(async () => {
      await flushPromises();
    });

    // The active search's results, query, and hasMore must survive untouched.
    expect(result.current.searchQuery).toBe("git");
    expect(namesOf(result.current.searchResultsWithStatus)).toEqual(["git-1", "git-2", "git-3"]);
    expect(result.current.hasMore).toBe(false);

    // popular must NOT be re-fetched on a projects change (mount-only).
    expect(counters.popular).toBe(popularCallsAfterMount);

    // The installed list IS refreshed for the new project set - one extra
    // installed call beyond the mount fetch.
    expect(counters.installed).toBe(2);
  });

  it("toasts but does not touch the Browse results when the projects-change installed refresh fails", async () => {
    const counters: FakeApiCounters = { installed: 0, popular: 0, search: 0 };
    const api: SkillStoreApi = {
      // Succeed on the mount fetch (call 1), reject on the projects-change
      // refresh (call 2) so the error path under test fires without also
      // failing the bundled mount fetch.
      getInstalledSkills: async () => {
        counters.installed += 1;
        if (counters.installed > 1) throw new Error("disk on fire");
        return [] satisfies InstalledSkill[];
      },
      getPopularSkills: async () => {
        counters.popular += 1;
        return page(["popular-a", "popular-b"]);
      },
      searchSkills: async (query: string) => {
        counters.search += 1;
        return page([`${query}-1`]);
      },
    };

    const { result, rerender } = renderHook(
      ({ projects, api, addToast }: HookProps) => useSkillStoreData(projects, addToast, api),
      { initialProps: initialPropsEmpty(api, addToast) },
    );

    await act(async () => {
      await flushPromises();
    });
    expect(namesOf(result.current.searchResultsWithStatus)).toEqual(["popular-a", "popular-b"]);

    // Commit a search, then change projects so the installed refresh fires
    // and rejects.
    await act(async () => {
      await result.current.handleSearch("git");
    });
    rerender({ projects: ["/work/new-project"], api, addToast });
    await act(async () => {
      await flushPromises();
    });

    // The error toasts; the Browse search view is untouched.
    expect(toasts.some((t) => t.title === "Failed to Load Installed Skills")).toBe(true);
    expect(result.current.searchQuery).toBe("git");
    expect(namesOf(result.current.searchResultsWithStatus)).toEqual(["git-1"]);
    expect(result.current.browseError).toBeNull();
  });
});
