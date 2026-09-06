// ============================================================================
// PROTOTYPE. Throwaway fixture for the installed skill header exploration.
// visual-recap is the sample skill. Location / harness / invocation live in
// the harness below the header, not in this payload's header variants.
// ============================================================================

export interface SkillHeaderFixture {
  name: string;
  source: string;
  sourceKind: string;
  description: string;
  size: string;
  tokens: string;
  uses: string;
  edited: string;
  installed: string;
  updateState: string;
  scope: string;
}

export interface SkillHeaderVariantProps {
  skill: SkillHeaderFixture;
  assistantOpen: boolean;
  feedback: string | null;
  onBack: () => void;
  onToggleAssistant: () => void;
  onAction: (label: string) => void;
}

export const SKILL_HEADER_FIXTURE: SkillHeaderFixture = {
  name: "visual-recap",
  source: "kentcdodds/kcd-skills",
  sourceKind: "skills.sh",
  description:
    "Generate and maintain the system recap block in a PR description — a GitHub-rendered visual summary of which system primitives a change touches, how risky it is, and what changed. Use when planning a non-trivial change (plan mode), when creating or updating a pull request (recap mode), or when the user asks for a visual recap, visual plan, system review, or PR recap.",
  size: "7.8 KB",
  tokens: "1.5k tokens",
  uses: "0 uses in 30 days",
  edited: "just now",
  installed: "Sep 4 2026",
  updateState: "Current",
  scope: "Global",
};
