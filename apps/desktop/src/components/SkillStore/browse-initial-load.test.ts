// ============================================================================
// Skill Studio - Browse initial load tests (#66)
// ============================================================================

import { describe, expect, it } from "vitest";
import { loadBrowseInitialData } from "./browse-initial-load";
import type { InstalledSkill, PaginatedSkillsResponse, SkillSearchResult } from "@skill-studio/lib";

// SAFETY: `loadBrowseInitialData` passes these through untouched; no field is read.
const INSTALLED = [{ name: "installed-one" }] as InstalledSkill[];
// SAFETY: as above, only the array identity is compared.
const POPULAR_SKILLS = [{ name: "popular-one" }] as SkillSearchResult[];
const POPULAR: PaginatedSkillsResponse = { skills: POPULAR_SKILLS, has_more: true };

const sources = {
  getInstalled: async () => INSTALLED,
  getPopular: async () => POPULAR,
};

describe("browse initial load", () => {
  it("project_list_reload_during_an_active_search_keeps_the_search_results_or_names_the_popular_overwrite", async () => {
    // The user searched "git", then added a project folder in the install drawer: the project
    // list changed, so the Browse tab reloads installed and popular skills.
    const load = await loadBrowseInitialData(sources, () => "git");

    expect(load.installed, "the installed list must still refresh for the new project").toBe(
      INSTALLED,
    );
    expect(
      load.popular,
      "popular skills came back for the Browse tab while the search box still shows 'git'",
    ).toBeNull();
  });

  it("search_typed_while_the_reload_is_in_flight_keeps_its_results_or_names_the_late_overwrite", async () => {
    let query = "";
    let releasePopular: (value: PaginatedSkillsResponse) => void = () => {};
    const pending = loadBrowseInitialData(
      {
        getInstalled: async () => INSTALLED,
        getPopular: () =>
          new Promise<PaginatedSkillsResponse>((resolve) => {
            releasePopular = resolve;
          }),
      },
      () => query,
    );
    query = "git";
    releasePopular(POPULAR);

    expect(
      (await pending).popular,
      "the reload that started before the search replaced the search results when it landed",
    ).toBeNull();
  });

  it("reload_with_no_active_search_shows_popular_skills_or_names_the_empty_browse_tab", async () => {
    for (const query of ["", " ", "g"]) {
      const load = await loadBrowseInitialData(sources, () => query);
      expect(load.popular, `query ${JSON.stringify(query)} shows popular skills`).toBe(POPULAR);
    }
  });
});
