// ============================================================================
// Skill Studio - Installed skill source ledger model tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { buildInstalledSkillSourceLedgerModel } from "./installed-skill-source-ledger-model";

function ledgerDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "deployment-1",
    destination: "universal",
    owner_kind: "skills-sh",
    owner_id: "owner-1",
    mutability: "mutable",
    backing: { kind: "canonical" },
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/example",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    ...overrides,
  };
}

function ledgerSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "example",
    source: "acme/example",
    source_type: "github",
    installed_at: "2026-01-02T00:00:00Z",
    has_update: false,
    update_owner_ids: [],
    source_kind: "skills-sh",
    deployments: [ledgerDeployment()],
    has_spec: true,
    spec_violations: [],
    skill_md_tokens: 1200,
    description_tokens: 12,
    folder_bytes: 2048,
    file_count: 2,
    content_hash: "abc",
    content_hashes: ["abc"],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    ...overrides,
  };
}

describe("buildInstalledSkillSourceLedgerModel", () => {
  it("deduplicates a verified dependent link under its skills.sh owner", () => {
    const canonical = ledgerDeployment();
    const dependent = ledgerDeployment({
      id: "deployment-2",
      owner_kind: "ambiguous",
      owner_id: undefined,
      mutability: "read-only",
      backing: { kind: "linked-to", deployment_id: canonical.id },
      is_symlink: true,
    });

    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({ deployments: [canonical, dependent] }),
    );

    expect(model.lifecycleOwner).toBe("skills.sh");
    expect(model.lifecycleManagement).toBe("Managed");
  });

  it("labels a mutable copy as managed by Skill Studio Copy", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({ deployments: [ledgerDeployment({ owner_kind: "copy" })] }),
    );

    expect(model.lifecycleOwner).toBe("Skill Studio Copy");
    expect(model.lifecycleManagement).toBe("Managed");
  });

  it("reports mixed ownership for distinct mutable owners", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({
        deployments: [
          ledgerDeployment(),
          ledgerDeployment({ id: "deployment-2", owner_kind: "dotagents", owner_id: "owner-2" }),
        ],
      }),
    );

    expect(model.lifecycleOwner).toBe("Mixed ownership");
    expect(model.lifecycleManagement).toBe("Managed");
  });

  it("uses deployment identity for independent ownerless copies", () => {
    const canonicalCopy = ledgerDeployment({
      owner_kind: "copy",
      owner_id: undefined,
    });
    const linkedCopy = ledgerDeployment({
      id: "deployment-2",
      owner_kind: "ambiguous",
      owner_id: undefined,
      mutability: "read-only",
      backing: { kind: "linked-to", deployment_id: canonicalCopy.id },
    });
    const independentCopy = ledgerDeployment({
      id: "deployment-3",
      owner_kind: "copy",
      owner_id: undefined,
      backing: { kind: "independent" },
    });

    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ deployments: [canonicalCopy, linkedCopy] }),
      ).lifecycleOwner,
    ).toBe("Skill Studio Copy");
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ deployments: [canonicalCopy, linkedCopy, independentCopy] }),
      ).lifecycleOwner,
    ).toBe("Mixed ownership");
  });

  it("reports mixed ownership and lifecycle management for mutable and read-only owners", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({
        deployments: [
          ledgerDeployment(),
          ledgerDeployment({
            id: "deployment-2",
            owner_kind: "plugin",
            owner_id: "plugin-1",
            mutability: "read-only",
          }),
        ],
      }),
    );

    expect(model.lifecycleOwner).toBe("Mixed ownership");
    expect(model.lifecycleManagement).toBe("Mixed");
  });

  it("distinguishes manual and ambiguous all-read-only ownership", () => {
    const manual = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({
        deployments: [
          ledgerDeployment({ owner_kind: "manual", owner_id: undefined, mutability: "read-only" }),
        ],
      }),
    );
    const ambiguous = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({
        deployments: [
          ledgerDeployment({
            owner_kind: "ambiguous",
            owner_id: undefined,
            mutability: "read-only",
          }),
        ],
      }),
    );

    expect(manual.lifecycleOwner).toBe("Manual");
    expect(manual.lifecycleManagement).toBe("Read-only");
    expect(ambiguous.lifecycleOwner).toBe("Ambiguous");
    expect(ambiguous.lifecycleManagement).toBe("Read-only");
  });

  it("uses source URLs and local wording when repository data is absent", () => {
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ source: "", source_url: "https://github.com/acme/example" }),
      ).source,
    ).toBe("https://github.com/acme/example");
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ source: "local", source_url: undefined, source_kind: "plugin" }),
      ).source,
    ).toBe("Agent plugin");
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ source: "", source_url: undefined, source_kind: "manual", deployments: [] }),
      ).source,
    ).toBe("Local skill");
  });

  it("uses dotagents as the source fallback for a local ledger source", () => {
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ source: "local", source_url: undefined, source_kind: "dotagents" }),
      ).source,
    ).toBe("dotagents");
  });

  it("retains the plugin name in the source identity", () => {
    const pluginDeployment = ledgerDeployment({
      owner_kind: "plugin",
      mutability: "read-only",
      plugin: { name: "openai-templates", harness: "Codex" },
    });

    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ source: "plugin", source_kind: "plugin", deployments: [pluginDeployment] }),
      ).source,
    ).toBe("Plugin · openai-templates");
  });

  it("prefers a tracked repository over an independent copy", () => {
    const copy = ledgerDeployment({
      id: "deployment-2",
      owner_kind: "copy",
      owner_id: undefined,
      backing: { kind: "independent" },
    });

    expect(
      buildInstalledSkillSourceLedgerModel(ledgerSkill({ deployments: [ledgerDeployment(), copy] }))
        .source,
    ).toBe("acme/example");
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({
          source: "local",
          source_url: undefined,
          source_kind: "manual",
          deployments: [copy],
        }),
      ).source,
    ).toBe("Local copy");
  });

  it("counts distinct owner updates without naming their locations", () => {
    expect(buildInstalledSkillSourceLedgerModel(ledgerSkill()).updateState).toBe("Up to date");
    expect(
      buildInstalledSkillSourceLedgerModel(ledgerSkill({ update_owner_ids: ["owner-1"] }))
        .updateState,
    ).toBe("1 update available");
    expect(
      buildInstalledSkillSourceLedgerModel(
        ledgerSkill({ update_owner_ids: ["owner-1", "owner-1", "owner-2"] }),
      ).updateState,
    ).toBe("2 owner updates available");
  });

  it("shows an unknown install date and omits a missing modification date", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({ installed_at: "", modified_at: undefined }),
    );

    expect(model.installed).toBe("Unknown");
    expect(model.lastModified).toBeUndefined();
  });

  it("shows unknown lifecycle and omits metrics when no deployment content is readable", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({ deployments: [], content_hashes: [], folder_bytes: 0, skill_md_tokens: 0 }),
    );

    expect(model.lifecycleManagement).toBe("Unknown");
    expect(model.size).toBeUndefined();
    expect(model.tokens).toBeUndefined();
  });

  it("keeps size and token metrics for one-content skills", () => {
    const model = buildInstalledSkillSourceLedgerModel(ledgerSkill());

    expect(model.size).toBe("2.0 KB");
    expect(model.tokens).toBe("1.2k tokens");
  });

  it("omits first-deployment metrics when content hashes diverge", () => {
    const model = buildInstalledSkillSourceLedgerModel(
      ledgerSkill({ content_hashes: ["abc", "abc", "def"] }),
    );

    expect(model.size).toBeUndefined();
    expect(model.tokens).toBeUndefined();
  });
});
