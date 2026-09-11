//! Emits the JSON Schema for the desktop's Tauri wire types to stdout - the
//! source `npm run types:generate` (apps/desktop/package.json) feeds into
//! `json-schema-to-typescript` to produce `packages/lib/src/skill-types.generated.ts`.
//! See that file's header, and `docs/spec-core-primitives.md` section 10's
//! PR 3 row, for why generation replaces the old hand-written skill-types.ts.
//!
//! `WireTypes` exists only to give `schemars` one root whose fields pull in
//! every DTO an IPC command returns or accepts, via schemars' own reachable-type
//! walk - it is never constructed and is filtered out of the generated output.
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use skill_studio_lib::skills::add_method_defaults::AddMethodDefaults;
use skill_studio_lib::skills::agents::AgentTarget;
use skill_studio_lib::skills::github_skill_listing::GithubSkillListing;
use skill_studio_lib::skills::skill_dto::{
    AddSkillOutcome, AddSkillRequest, AddSkillResult, AddSkillsRequest, HarnessVisibilityTarget,
    InstallResult, LifecycleTarget, PaginatedSkillsResponse, SkillDetails, SkillEventDto,
    SkillsShAccessInfo,
};
use skill_studio_lib::skills::skill_fork::PullResult;
use skill_studio_lib::skills::skill_fork_registry::{ForkRecord, PackMember};
use skill_studio_lib::skills::skill_frontmatter_repair::FrontmatterRepairPreview;
use skill_studio_lib::skills::skill_invocations::{InvocationHeatmap, SkillInvocation};
use skill_studio_lib::skills::skill_pack::{
    ImportResult, PackImportPreflightResult, PackImportRequest, PackInfo, UpdatePackResult,
};
use skill_studio_lib::skills::skill_refresh::SkillSnapshot;

#[derive(JsonSchema)]
#[allow(dead_code)]
struct WireTypes {
    skill_snapshot: SkillSnapshot,
    agent_target: AgentTarget,
    add_method_defaults: AddMethodDefaults,
    paginated_skills_response: PaginatedSkillsResponse,
    skill_details: SkillDetails,
    skills_sh_access_info: SkillsShAccessInfo,
    install_result: InstallResult,
    harness_visibility_target: HarnessVisibilityTarget,
    add_skill_request: AddSkillRequest,
    add_skills_request: AddSkillsRequest,
    add_skill_outcome: AddSkillOutcome,
    add_skill_result: AddSkillResult,
    github_skill_listing: GithubSkillListing,
    fork_record: ForkRecord,
    pull_result: PullResult,
    frontmatter_repair_preview: FrontmatterRepairPreview,
    invocation_heatmap: InvocationHeatmap,
    pack_info: PackInfo,
    update_pack_result: UpdatePackResult,
    import_result: ImportResult,
    pack_import_preflight_result: PackImportPreflightResult,
    skill_event: SkillEventDto,
    pack_member: PackMember,
    pack_import_request: PackImportRequest,
    skill_invocation: SkillInvocation,
    lifecycle_target: LifecycleTarget,
}

fn main() {
    // `for_serialize()` makes `required` reflect what the Rust side actually
    // writes to the wire (every field lacking `skip_serializing_if` is always
    // present), not what `#[serde(default)]` would tolerate on the way back
    // in - schemars' default `Contract::Deserialize` schema marks any
    // `#[serde(default)]` field optional even though it's never omitted by
    // `serde_json::to_string`, which produced a stream of spurious `field?:`
    // types (and `T | undefined` for `Option<T>` fields) in the generated
    // TypeScript.
    let settings = SchemaSettings::default().for_serialize();
    let schema = settings
        .into_generator()
        .into_root_schema_for::<WireTypes>();
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}
