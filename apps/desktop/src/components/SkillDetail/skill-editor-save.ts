// ============================================================================
// Skill Studio - inline SKILL.md editor save
// Keeps the editor-open baseline attached to each compare-and-swap write.
// ============================================================================

/** One inline editor draft and the exact file content from when editing started. */
export interface SkillEditorDraft {
  path: string;
  openedContent: string;
  draftContent: string;
}

/** Saves one editor draft with compare-and-swap and advances its baseline only after success. */
export async function saveSkillEditorDraft(
  draft: SkillEditorDraft,
  writeIfUnchanged: (path: string, expectedContent: string, content: string) => Promise<void>,
): Promise<SkillEditorDraft> {
  await writeIfUnchanged(draft.path, draft.openedContent, draft.draftContent);
  return { ...draft, openedContent: draft.draftContent };
}
