// ============================================================================
// useSkillCompareDialog - SkillPage's compare dialog identity tracking:
// closes a dialog carried over from a previous skill on a plain skill
// switch, so it never reopens for a different skill's copy.
// ============================================================================

import { useState } from "react";
import type { Dispatch, SetStateAction } from "react";

export interface UseSkillCompareDialog {
  isCompareOpen: boolean;
  setIsCompareOpen: Dispatch<SetStateAction<boolean>>;
}

export function useSkillCompareDialog(skillName: string | undefined): UseSkillCompareDialog {
  const [isCompareOpen, setIsCompareOpen] = useState(false);

  /** The skill the compare dialog was last shown for, so a plain skill switch (no fresh compare
   * request) closes it instead of carrying it over. */
  const [prevSkillName, setPrevSkillName] = useState(skillName);

  // A plain skill switch (no fresh compare request) closes a dialog carried
  // over from the previous skill - adjusted during render, per React's
  // "storing information from previous renders" pattern, since it's a reset
  // keyed off an identity change rather than something to synchronize.
  if (prevSkillName !== skillName) {
    setPrevSkillName(skillName);
    if (isCompareOpen) setIsCompareOpen(false);
  }

  return { isCompareOpen, setIsCompareOpen };
}
