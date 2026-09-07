// ============================================================================
// Skill Studio - inline SKILL.md editor save tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import { saveSkillEditorDraft } from "./skill-editor-save";

describe("saveSkillEditorDraft", () => {
  it("does not overwrite a concurrent write when the editor baseline is stale", async () => {
    let diskContent = "assistant update";
    const writeIfUnchanged = vi.fn(
      async (_path: string, expectedContent: string, content: string): Promise<void> => {
        if (diskContent !== expectedContent) throw new Error("SKILL.md changed on disk");
        diskContent = content;
      },
    );

    await expect(
      saveSkillEditorDraft(
        {
          path: "/skills/example/SKILL.md",
          openedContent: "content when editor opened",
          draftContent: "user draft",
        },
        writeIfUnchanged,
      ),
    ).rejects.toThrow("SKILL.md changed on disk");

    expect(writeIfUnchanged).toHaveBeenCalledWith(
      "/skills/example/SKILL.md",
      "content when editor opened",
      "user draft",
    );
    expect(diskContent).toBe("assistant update");
  });

  it("advances the baseline to the saved draft after a successful save", async () => {
    const writeIfUnchanged = vi.fn(async (): Promise<void> => undefined);

    const saved = await saveSkillEditorDraft(
      {
        path: "/skills/example/SKILL.md",
        openedContent: "content when editor opened",
        draftContent: "saved draft",
      },
      writeIfUnchanged,
    );

    expect(saved).toEqual({
      path: "/skills/example/SKILL.md",
      openedContent: "saved draft",
      draftContent: "saved draft",
    });
  });
});
