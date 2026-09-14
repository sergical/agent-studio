// ============================================================================
// useSkillMdEditorState - SkillPage's draft/edit-mode state: whether the
// editor is open, whether its draft is dirty, and the resets a skill or
// deployment switch must apply so no draft carries over to a different copy.
// ============================================================================

import { useState } from "react";
import type { Dispatch, SetStateAction } from "react";

export interface UseSkillMdEditorState {
  editorOpenedContent: string | null;
  setEditorOpenedContent: Dispatch<SetStateAction<string | null>>;
  isEditing: boolean;
  isEditorDirty: boolean;
  setIsEditorDirty: Dispatch<SetStateAction<boolean>>;
}

/**
 * A skill switch, or a deployment/path switch on the same skill, can't carry
 * over a draft or edit mode from a different copy - each reset is adjusted
 * during render (per React's "storing information from previous renders"
 * pattern) against its own previous-value state, since it's independent of
 * the other.
 */
export function useSkillMdEditorState(
  skillName: string | undefined,
  skillMdPath: string | undefined,
): UseSkillMdEditorState {
  const [editorOpenedContent, setEditorOpenedContent] = useState<string | null>(null);
  const isEditing = editorOpenedContent !== null;
  const [isEditorDirty, setIsEditorDirty] = useState(false);

  const [prevSkillName, setPrevSkillName] = useState(skillName);
  if (prevSkillName !== skillName) {
    setPrevSkillName(skillName);
    if (isEditing) setEditorOpenedContent(null);
    if (isEditorDirty) setIsEditorDirty(false);
  }

  const [prevSkillMdPath, setPrevSkillMdPath] = useState(skillMdPath);
  if (prevSkillMdPath !== skillMdPath) {
    setPrevSkillMdPath(skillMdPath);
    if (isEditing) setEditorOpenedContent(null);
    if (isEditorDirty) setIsEditorDirty(false);
  }

  return {
    editorOpenedContent,
    setEditorOpenedContent,
    isEditing,
    isEditorDirty,
    setIsEditorDirty,
  };
}
