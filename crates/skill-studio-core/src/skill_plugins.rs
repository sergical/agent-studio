// ============================================================================
// Skills Module - Plugin Enumeration
// Discovers skills shipped inside agent plugins (Claude Code, Codex, Cursor,
// Grok Build, and any agent-plugins.org-shaped plugin) by walking plugin
// cache trees for manifests and their bundled skills/ directories, per the
// agent-plugins.org compatible-clients convention:
// https://agent-plugins.org/compatible-clients
//
// OpenCode and pi plugins are JS/TS extension modules, not Agent Skills
// bundles - they have no skills/ subdirectory to scan, so we don't scan them.
// ============================================================================

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::skill_candidate::PluginEvidence;
use crate::skill_read::{
    DiscoveryExtent, DiscoveryReadIssue, DiscoveryReadIssueKind, MembershipSource, SourceCoverage,
    SourceReadOutcome,
};
use crate::skill_scope::{
    DirectoryReadIssue, RootBindOutcome, RootRevalidationError, ScopedDirectoryRead,
    ScopedReadError, SkillReadScope,
};

/// A plugin that shipped a skill, per the agent-plugins.org convention
/// (Claude Code / Codex plugin caches, or any directory with a `plugin.json`
/// manifest and a `skills/` subdirectory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    /// Which agent's plugin system this came from, e.g. "Claude Code", "Codex".
    pub harness: String,
}

/// Plugin manifest filenames to look for, in priority order: each
/// harness-scoped folder (`.claude-plugin/plugin.json`,
/// `.codex-plugin/plugin.json`, `.cursor-plugin/plugin.json`) first, then the
/// generic agent-plugins.org root `plugin.json`.
const MANIFEST_CANDIDATES: &[&str] = &[
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
    "plugin.json",
];

/// Every plugin cache root we scan natively, as (path relative to home,
/// display label) pairs shared by the scanner and its tests. A root that
/// doesn't exist on disk is skipped silently.
pub(crate) const PLUGIN_CACHE_ROOTS: &[(&str, &str)] = &[
    (".claude/plugins/cache", "Claude Code"),
    (".codex/plugins/cache", "Codex"),
    // `<marketplace>/<plugin>/<sha>`.
    (".cursor/plugins/cache", "Cursor"),
    // A git checkout of a single plugin: `<plugin>/.cursor-plugin/plugin.json`.
    (".cursor/plugins/local", "Cursor"),
    // Layout unknown; scanned the same depth-3 way as the others.
    (".grok/plugins", "Grok Build"),
];

/// A plugin root discovered while walking a cache tree, plus the skills it ships.
pub struct EnumeratedPlugin {
    pub info: PluginInfo,
    pub root: PathBuf,
}

/// A single skill directory found inside a plugin's `skills/` subdirectory.
pub struct PluginSkillDir {
    pub skill_dir: PathBuf,
    pub plugin: PluginInfo,
    pub(crate) entry_observation: Arc<crate::skill_scope::ScopedEntryObservation>,
    pub(crate) document_observation: crate::skill_scope::ScopedContentObservation,
}

#[derive(Default)]
pub struct PluginScanReport {
    pub skills: Vec<PluginSkillDir>,
    pub read_issues: Vec<DiscoveryReadIssue>,
    pub(crate) coverage: Vec<SourceCoverage>,
    pub(crate) cache_proofs: Vec<PluginCacheProof>,
}

pub(crate) struct PluginCacheProof {
    pub(crate) path: PathBuf,
    tree: PreparedPluginRoots,
    directories: Vec<PreparedPluginSkillDirectory>,
    skills: Vec<PreparedPluginSkill>,
    state: PluginScanState,
}

impl PluginCacheProof {
    pub(crate) fn revalidate(
        &self,
        scope: &SkillReadScope,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> Vec<PathBuf> {
        let mut state = self.state;
        let mut unchanged = self.tree.revalidate(scope, issues, &mut state);
        for lookup in &self.tree.lookups {
            if !lookup.revalidate(scope, issues) {
                unchanged = false;
            }
        }
        for directory in &self.directories {
            if let Err(message) = directory.revalidate(scope) {
                io_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    &directory.path,
                    message,
                );
                unchanged = false;
            }
        }
        let mut invalidated = BTreeSet::new();
        if !unchanged {
            invalidated.insert(self.path.clone());
        }
        for skill in &self.skills {
            if let Err(error) = scope.resolve_observed_dir(&skill.entry) {
                io_issue(
                    issues,
                    DiscoveryReadIssueKind::Entry,
                    &skill.skill_dir,
                    error.to_string(),
                );
                invalidated.insert(skill.skill_dir.clone());
            }
            if matches!(
                &skill.document,
                Ok(_) | Err(ScopedReadError::Missing { .. })
            ) {
                if let Err(message) = skill.validate_document(scope) {
                    io_issue(
                        issues,
                        DiscoveryReadIssueKind::SkillDocument,
                        &skill.skill_dir.join("SKILL.md"),
                        message,
                    );
                    invalidated.insert(skill.skill_dir.clone());
                }
            }
        }
        invalidated.into_iter().collect()
    }
}

#[derive(Clone, Copy)]
struct PluginScanState {
    membership: SourceReadOutcome,
    facts: SourceReadOutcome,
    work_remaining: usize,
}

impl PluginScanState {
    fn read_directory(
        &mut self,
        scope: &SkillReadScope,
        path: &Path,
        entry_limit: usize,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> Option<Result<ScopedDirectoryRead, ScopedReadError>> {
        if self.work_remaining == 0 {
            self.incomplete();
            io_issue(
                issues,
                DiscoveryReadIssueKind::Cap,
                path,
                "Plugin traversal work budget was reached",
            );
            return None;
        }
        self.work_remaining -= 1;
        let result = scope.read_dir(path, entry_limit.min(self.work_remaining));
        if let Ok(listing) = &result {
            let failed_entries = listing
                .issues
                .iter()
                .filter(|issue| matches!(issue, DirectoryReadIssue::Entry { .. }))
                .count();
            self.work_remaining = self
                .work_remaining
                .saturating_sub(listing.entries.len() + failed_entries);
        }
        Some(result)
    }

    fn read() -> Self {
        Self {
            membership: SourceReadOutcome::Read,
            facts: SourceReadOutcome::Read,
            work_remaining: usize::MAX,
        }
    }

    fn incomplete(&mut self) {
        if self.membership == SourceReadOutcome::Read {
            self.membership = SourceReadOutcome::Incomplete;
        }
        if self.facts == SourceReadOutcome::Read {
            self.facts = SourceReadOutcome::Incomplete;
        }
    }

    fn failed(&mut self) {
        self.membership = SourceReadOutcome::Failed;
        self.facts = SourceReadOutcome::Failed;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryCompleteness {
    Complete,
    Truncated,
}

struct PluginAncestorBoundary {
    lexical: PathBuf,
    physical: PathBuf,
    completeness: BoundaryCompleteness,
}

/// One caller-bound scope for discovery. Core never derives a process home or
/// authorizes a parent of a missing root.
pub struct SkillDiscoveryReadContext {
    home: PathBuf,
    projects: Vec<PathBuf>,
    backing_roots: Vec<PathBuf>,
    plugin_ownership_roots: Vec<PathBuf>,
    scope: SkillReadScope,
    outcomes: Vec<RootBindOutcome>,
    boundaries: Vec<PluginAncestorBoundary>,
}

impl SkillDiscoveryReadContext {
    pub fn bind(
        home: PathBuf,
        projects: Vec<PathBuf>,
        backing_roots: Vec<PathBuf>,
        plugin_ownership_roots: Vec<PathBuf>,
    ) -> Self {
        let mut requested = Vec::with_capacity(
            1 + projects.len() + backing_roots.len() + plugin_ownership_roots.len(),
        );
        requested.push(home.clone());
        requested.extend(projects.iter().cloned());
        requested.extend(backing_roots.iter().cloned());
        requested.extend(plugin_ownership_roots.iter().cloned());
        let partial = SkillReadScope::bind_partial(&requested);
        let mut boundary_specs = Vec::with_capacity(requested.len());
        boundary_specs.push((home.clone(), BoundaryCompleteness::Complete));
        boundary_specs.extend(
            projects
                .iter()
                .chain(&backing_roots)
                .cloned()
                .map(|root| (root, BoundaryCompleteness::Truncated)),
        );
        boundary_specs.extend(
            plugin_ownership_roots
                .iter()
                .cloned()
                .map(|root| (root, BoundaryCompleteness::Complete)),
        );
        let boundaries = boundary_specs
            .into_iter()
            .zip(&partial.outcomes)
            .filter_map(|((lexical, completeness), outcome)| match outcome {
                RootBindOutcome::Bound { physical, .. } => Some(PluginAncestorBoundary {
                    lexical,
                    physical: physical.clone(),
                    completeness,
                }),
                RootBindOutcome::Missing { .. } | RootBindOutcome::Failed { .. } => None,
            })
            .collect();
        Self {
            home,
            projects,
            backing_roots,
            plugin_ownership_roots,
            scope: partial.scope,
            outcomes: partial.outcomes,
            boundaries,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn projects(&self) -> &[PathBuf] {
        &self.projects
    }

    pub fn backing_roots(&self) -> &[PathBuf] {
        &self.backing_roots
    }

    pub fn plugin_ownership_roots(&self) -> &[PathBuf] {
        &self.plugin_ownership_roots
    }

    pub fn bind_outcomes(&self) -> &[RootBindOutcome] {
        &self.outcomes
    }

    pub fn revalidate_roots(&self) -> Result<(), RootRevalidationError> {
        self.scope.revalidate_roots()
    }

    pub(crate) fn read_scope(&self) -> &SkillReadScope {
        &self.scope
    }

    pub(crate) fn append_bind_issues(&self, issues: &mut Vec<DiscoveryReadIssue>) {
        for outcome in &self.outcomes {
            match outcome {
                RootBindOutcome::Bound { .. } => {}
                RootBindOutcome::Missing { requested, source } => io_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    requested,
                    format!("Requested root is missing: {source}"),
                ),
                RootBindOutcome::Failed { requested, source } => io_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    requested,
                    format!("Requested root could not be bound: {source}"),
                ),
            }
        }
    }
}

const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4096;

#[derive(Clone, Copy)]
struct PluginScanLimits {
    max_depth: u8,
    directory_entries: usize,
    // One unit per directory read and per listed entry, including failed entries.
    // Manifest probes and skill-document observations have fixed cost per entry.
    max_work: usize,
}

const DEFAULT_PLUGIN_SCAN_LIMITS: PluginScanLimits = PluginScanLimits {
    max_depth: 3,
    directory_entries: MAX_DIRECTORY_ENTRIES,
    max_work: 16_384,
};

fn fallback_plugin_identity(dir: &Path) -> (String, Option<String>) {
    (
        dir.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        None,
    )
}

fn issue(issues: &mut Vec<DiscoveryReadIssue>, path: &Path, message: impl Into<String>) {
    issues.push(DiscoveryReadIssue::new(
        DiscoveryReadIssueKind::PluginManifest,
        path,
        message,
    ));
}

fn io_issue(
    issues: &mut Vec<DiscoveryReadIssue>,
    kind: DiscoveryReadIssueKind,
    path: &Path,
    message: impl Into<String>,
) {
    issues.push(DiscoveryReadIssue::new(kind, path, message));
}

#[derive(Clone)]
enum ManifestEvidence {
    Confirmed(String, Option<String>),
    Absent,
    Unknown,
}

struct PreparedManifest {
    root: PathBuf,
    observation: crate::skill_scope::ScopedContentObservation,
    absent_before: Vec<PathBuf>,
    portable: Option<crate::skill_scope::ScopedContentObservation>,
}

impl PreparedManifest {
    fn enumerate(scope: &SkillReadScope, dir: &Path) -> Result<Option<Self>, DiscoveryReadIssue> {
        let mut absent_before = Vec::new();
        for candidate in MANIFEST_CANDIDATES {
            let path = dir.join(candidate);
            match scope.observe_content_regular(&path) {
                Ok(observation) => {
                    let portable_path = dir.join("plugin.json");
                    let portable = if observation.requested == portable_path {
                        None
                    } else {
                        match scope.observe_content_regular(&portable_path) {
                            Ok(observation) => Some(observation),
                            Err(ScopedReadError::Missing { .. }) => {
                                absent_before.push(portable_path);
                                None
                            }
                            Err(error) => {
                                return Err(DiscoveryReadIssue::new(
                                    DiscoveryReadIssueKind::PluginManifest,
                                    &portable_path,
                                    error.to_string(),
                                ))
                            }
                        }
                    };
                    return Ok(Some(Self {
                        root: dir.to_path_buf(),
                        observation,
                        absent_before,
                        portable,
                    }));
                }
                Err(ScopedReadError::Missing { .. }) => absent_before.push(path),
                Err(error) => {
                    return Err(DiscoveryReadIssue::new(
                        DiscoveryReadIssueKind::PluginManifest,
                        &path,
                        error.to_string(),
                    ))
                }
            }
        }
        Ok(None)
    }

    fn priority_unchanged(
        &self,
        scope: &SkillReadScope,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> bool {
        if let Some(portable) = &self.portable {
            if !matches!(scope.observe_content_regular(&portable.requested), Ok(current) if current == *portable)
            {
                issue(
                    issues,
                    &portable.requested,
                    "Portable plugin manifest changed after enumeration",
                );
                state.incomplete();
                return false;
            }
        }
        for path in &self.absent_before {
            if !matches!(
                scope.observe_content_regular(path),
                Err(ScopedReadError::Missing { .. })
            ) {
                issue(
                    issues,
                    path,
                    "Higher-priority plugin manifest changed after enumeration",
                );
                state.incomplete();
                return false;
            }
        }
        true
    }

    fn paths(&self) -> impl Iterator<Item = PathBuf> + '_ {
        std::iter::once(self.observation.requested.clone()).chain(
            self.portable
                .iter()
                .map(|observation| observation.requested.clone()),
        )
    }

    fn read_observation(
        scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        observation: &crate::skill_scope::ScopedContentObservation,
    ) -> Result<Vec<u8>, String> {
        use crate::skill_scope::ScopedContentFoldError;
        let content = match guard {
            Some(guard) => guard.read_content(scope, observation, MAX_MANIFEST_BYTES),
            None => scope
                .read_content_observed(observation, MAX_MANIFEST_BYTES)
                .map_err(ScopedContentFoldError::Read),
        };
        content.map_err(|error| match error {
            ScopedContentFoldError::Read(error) => error.to_string(),
            ScopedContentFoldError::Changed => "Plugin manifest changed after enumeration".into(),
            ScopedContentFoldError::Cancelled(message) => message,
        })
    }

    fn materialize(
        &self,
        scope: &SkillReadScope,
        harness: &str,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> ManifestEvidence {
        if !self.priority_unchanged(scope, issues, state) {
            return ManifestEvidence::Unknown;
        }
        let mut selected = &self.observation;
        let mut portable_content = None;
        if harness == "Codex" {
            if let Some(portable) = &self.portable {
                match Self::read_observation(scope, guard, portable) {
                    Ok(content) => {
                        let recognized = serde_json::from_slice::<serde_json::Value>(&content)
                            .ok().is_some_and(|value| value.get("$schema").and_then(|schema| schema.as_str())
                                == Some("https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"));
                        if recognized {
                            selected = portable;
                            portable_content = Some(content);
                        }
                    }
                    Err(error) => {
                        issue(issues, &portable.requested, error);
                        state.incomplete();
                        return ManifestEvidence::Unknown;
                    }
                }
            }
        }
        let content = match portable_content
            .map(Ok)
            .unwrap_or_else(|| Self::read_observation(scope, guard, selected))
        {
            Ok(content) => content,
            Err(error) => {
                issue(issues, &selected.requested, error);
                state.incomplete();
                return ManifestEvidence::Unknown;
            }
        };
        if !self.priority_unchanged(scope, issues, state) {
            return ManifestEvidence::Unknown;
        }
        let value: serde_json::Value = match serde_json::from_slice(&content) {
            Ok(value) => value,
            Err(error) => {
                issue(issues, &selected.requested, error.to_string());
                state.incomplete();
                let fallback = fallback_plugin_identity(&self.root);
                return ManifestEvidence::Confirmed(fallback.0, fallback.1);
            }
        };
        let Some(name) = value.get("name").and_then(|value| value.as_str()) else {
            issue(
                issues,
                &selected.requested,
                "Plugin manifest has no string name",
            );
            state.incomplete();
            let fallback = fallback_plugin_identity(&self.root);
            return ManifestEvidence::Confirmed(fallback.0, fallback.1);
        };
        ManifestEvidence::Confirmed(
            name.to_string(),
            value
                .get("version")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        )
    }
}

struct PreparedManifestLookup {
    root: PathBuf,
    observation: Result<Option<PreparedManifest>, DiscoveryReadIssue>,
}

impl PreparedManifestLookup {
    fn enumerate(scope: &SkillReadScope, root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            observation: PreparedManifest::enumerate(scope, root),
        }
    }

    fn revalidate(&self, scope: &SkillReadScope, issues: &mut Vec<DiscoveryReadIssue>) -> bool {
        let unchanged = match &self.observation {
            Ok(Some(manifest)) => {
                matches!(scope.observe_content_regular(&manifest.observation.requested), Ok(current) if current == manifest.observation)
                    && manifest.portable.iter().all(|observation| {
                        matches!(scope.observe_content_regular(&observation.requested), Ok(current) if current == *observation)
                    })
                    && manifest.absent_before.iter().all(|path| {
                        matches!(
                            scope.observe_content_regular(path),
                            Err(ScopedReadError::Missing { .. })
                        )
                    })
            }
            Ok(None) => matches!(PreparedManifest::enumerate(scope, &self.root), Ok(None)),
            Err(_) => return true,
        };
        if !unchanged {
            issue(
                issues,
                &self.root,
                "Plugin manifest evidence changed after membership preparation",
            );
        }
        unchanged
    }

    fn materialize(
        &self,
        scope: &SkillReadScope,
        harness: &str,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> ManifestEvidence {
        match &self.observation {
            Ok(Some(manifest)) => manifest.materialize(scope, harness, guard, issues, state),
            Ok(None) => match PreparedManifest::enumerate(scope, &self.root) {
                Ok(None) => ManifestEvidence::Absent,
                Ok(Some(_)) => {
                    issue(
                        issues,
                        &self.root,
                        "Plugin manifest appeared after enumeration",
                    );
                    state.incomplete();
                    ManifestEvidence::Unknown
                }
                Err(error) => {
                    issues.push(error.clone());
                    state.incomplete();
                    ManifestEvidence::Unknown
                }
            },
            Err(error) => {
                issues.push(error.clone());
                state.incomplete();
                ManifestEvidence::Unknown
            }
        }
    }
}

/// Walk up from `path` looking for a plugin root. Used to identify
/// plugin-shipped skills found via a generic directory scan (e.g. a skill
/// dir reached through `.agents/skills` or an agent's own skills root),
/// not just the dedicated plugin cache walk below.
pub fn find_plugin_root(
    context: &SkillDiscoveryReadContext,
    path: &Path,
    harness: &str,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> PluginEvidence {
    PreparedPluginAncestry::enumerate(context, path, harness).materialize(
        &context.scope,
        None,
        issues,
    )
}

#[derive(Clone)]
struct ManifestObservationKey(Arc<PreparedManifestLookup>);

impl PartialEq for ManifestObservationKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ManifestObservationKey {}
impl std::hash::Hash for ManifestObservationKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.0), state);
    }
}

pub(crate) struct ManifestReadPass<'a> {
    scope: &'a SkillReadScope,
    guard: Option<&'a crate::skill_coordination::CoordinatedReadGuard>,
    results: HashMap<(ManifestObservationKey, bool), (ManifestEvidence, Vec<DiscoveryReadIssue>)>,
}

impl<'a> ManifestReadPass<'a> {
    pub(crate) fn new(
        scope: &'a SkillReadScope,
        guard: Option<&'a crate::skill_coordination::CoordinatedReadGuard>,
    ) -> Self {
        Self {
            scope,
            guard,
            results: HashMap::new(),
        }
    }

    fn materialize(
        &mut self,
        lookup: &Arc<PreparedManifestLookup>,
        harness: &str,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> ManifestEvidence {
        let (evidence, diagnostics) = self
            .results
            .entry((
                ManifestObservationKey(Arc::clone(lookup)),
                harness == "Codex",
            ))
            .or_insert_with(|| {
                let mut diagnostics = Vec::new();
                let evidence = lookup.materialize(
                    self.scope,
                    harness,
                    self.guard,
                    &mut diagnostics,
                    &mut PluginScanState::read(),
                );
                (evidence, diagnostics)
            });
        issues.extend(diagnostics.iter().cloned());
        evidence.clone()
    }
}

pub(crate) struct ManifestValidationPass<'a> {
    scope: &'a SkillReadScope,
    results: HashMap<ManifestObservationKey, bool>,
}

impl<'a> ManifestValidationPass<'a> {
    pub(crate) fn new(scope: &'a SkillReadScope) -> Self {
        Self {
            scope,
            results: HashMap::new(),
        }
    }

    fn unchanged(&mut self, lookup: &Arc<PreparedManifestLookup>) -> bool {
        *self
            .results
            .entry(ManifestObservationKey(Arc::clone(lookup)))
            .or_insert_with(|| lookup.revalidate(self.scope, &mut Vec::new()))
    }
}

pub(crate) struct PluginObservationCache<'a> {
    context: &'a SkillDiscoveryReadContext,
    lookups: HashMap<PathBuf, Arc<PreparedManifestLookup>>,
}

impl<'a> PluginObservationCache<'a> {
    pub(crate) fn new(context: &'a SkillDiscoveryReadContext) -> Self {
        Self {
            context,
            lookups: HashMap::new(),
        }
    }

    fn lookup(&mut self, path: &Path) -> Arc<PreparedManifestLookup> {
        Arc::clone(self.lookups.entry(path.to_path_buf()).or_insert_with(|| {
            Arc::new(PreparedManifestLookup::enumerate(&self.context.scope, path))
        }))
    }
}

pub(crate) struct PreparedPluginAncestry {
    harness: String,
    resolved: PreparedAncestorBranch,
    lexical: Option<PreparedAncestorBranch>,
    issues: Vec<DiscoveryReadIssue>,
}

impl PreparedPluginAncestry {
    pub(crate) fn enumerate(
        context: &SkillDiscoveryReadContext,
        path: &Path,
        harness: &str,
    ) -> Self {
        Self::enumerate_cached(&mut PluginObservationCache::new(context), path, harness)
    }

    pub(crate) fn enumerate_cached(
        cache: &mut PluginObservationCache<'_>,
        path: &Path,
        harness: &str,
    ) -> Self {
        let context = cache.context;
        let mut issues = Vec::new();
        let (resolved, skip_start) = match context.scope.resolved_dir_path(path) {
            Ok(resolved) => (
                PreparedAncestorBranch::enumerate(cache, &resolved, true, false),
                false,
            ),
            Err(error) => {
                issue(
                    &mut issues,
                    path,
                    format!("Could not resolve plugin ancestry: {error}"),
                );
                let missing_target = matches!(&error, ScopedReadError::LinkTarget { source, .. } if source.kind() == std::io::ErrorKind::NotFound);
                (
                    PreparedAncestorBranch {
                        lookups: Vec::new(),
                        unknown: !missing_target,
                    },
                    missing_target,
                )
            }
        };
        let lexical = context
            .boundaries
            .iter()
            .any(|boundary| path.starts_with(&boundary.lexical))
            .then(|| PreparedAncestorBranch::enumerate(cache, path, false, skip_start));
        Self {
            harness: harness.to_string(),
            resolved,
            lexical,
            issues,
        }
    }

    pub(crate) fn append_regular_files(&self, files: &mut BTreeSet<PathBuf>) {
        for lookup in self
            .resolved
            .lookups
            .iter()
            .chain(self.lexical.iter().flat_map(|branch| &branch.lookups))
        {
            if let Ok(Some(manifest)) = &lookup.observation {
                files.extend(manifest.paths());
            }
        }
    }

    pub(crate) fn revalidate(&self, pass: &mut ManifestValidationPass<'_>) -> bool {
        self.resolved
            .lookups
            .iter()
            .chain(self.lexical.iter().flat_map(|branch| &branch.lookups))
            .all(|lookup| pass.unchanged(lookup))
    }

    pub(crate) fn materialize(
        &self,
        scope: &SkillReadScope,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> PluginEvidence {
        self.materialize_with_pass(&mut ManifestReadPass::new(scope, guard), issues)
    }

    pub(crate) fn materialize_with_pass(
        &self,
        pass: &mut ManifestReadPass<'_>,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> PluginEvidence {
        issues.extend(self.issues.iter().cloned());
        let resolved = self.resolved.materialize(pass, &self.harness, issues);
        if matches!(resolved, PluginEvidence::Confirmed(_)) {
            return resolved;
        }
        let Some(lexical) = &self.lexical else {
            return resolved;
        };
        let lexical = lexical.materialize(pass, &self.harness, issues);
        if matches!(lexical, PluginEvidence::Confirmed(_)) {
            lexical
        } else if matches!(resolved, PluginEvidence::Unknown)
            || matches!(lexical, PluginEvidence::Unknown)
        {
            PluginEvidence::Unknown
        } else {
            PluginEvidence::Absent
        }
    }
}

struct PreparedAncestorBranch {
    lookups: Vec<Arc<PreparedManifestLookup>>,
    unknown: bool,
}

impl PreparedAncestorBranch {
    fn enumerate(
        cache: &mut PluginObservationCache<'_>,
        path: &Path,
        physical: bool,
        skip_start: bool,
    ) -> Self {
        let context = cache.context;
        let containing = |boundary: &&PluginAncestorBoundary| {
            path.starts_with(if physical {
                &boundary.physical
            } else {
                &boundary.lexical
            })
        };
        let complete = context
            .boundaries
            .iter()
            .filter(containing)
            .filter(|boundary| boundary.completeness == BoundaryCompleteness::Complete)
            .min_by_key(|boundary| {
                if physical {
                    boundary.physical.components().count()
                } else {
                    boundary.lexical.components().count()
                }
            });
        let boundary = complete.or_else(|| {
            context
                .boundaries
                .iter()
                .filter(containing)
                .filter(|boundary| boundary.completeness == BoundaryCompleteness::Truncated)
                .min_by_key(|boundary| {
                    if physical {
                        boundary.physical.components().count()
                    } else {
                        boundary.lexical.components().count()
                    }
                })
        });
        let Some(boundary) = boundary else {
            return Self {
                lookups: Vec::new(),
                unknown: true,
            };
        };
        let limit = if physical {
            &boundary.physical
        } else {
            &boundary.lexical
        };
        let mut current = if skip_start {
            path.parent()
        } else {
            Some(path)
        };
        let mut lookups = Vec::new();
        while let Some(dir) = current {
            lookups.push(cache.lookup(dir));
            if dir == limit {
                break;
            }
            current = dir.parent();
        }
        Self {
            lookups,
            unknown: boundary.completeness == BoundaryCompleteness::Truncated,
        }
    }

    fn materialize(
        &self,
        pass: &mut ManifestReadPass<'_>,
        harness: &str,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> PluginEvidence {
        let mut unknown = self.unknown;
        for lookup in &self.lookups {
            match pass.materialize(lookup, harness, issues) {
                ManifestEvidence::Confirmed(name, version) => {
                    return PluginEvidence::Confirmed(PluginInfo {
                        name,
                        version,
                        harness: harness.to_string(),
                    })
                }
                ManifestEvidence::Absent => {}
                ManifestEvidence::Unknown => unknown = true,
            }
        }
        if unknown {
            PluginEvidence::Unknown
        } else {
            PluginEvidence::Absent
        }
    }
}

/// Walk a plugin cache tree up to `max_depth` levels looking for plugin
/// roots. Handles the documented Claude Code layout
/// (`cache/{marketplace}/{plugin}/{version}/`) as well as shallower ones,
/// since real caches vary. Stops descending once a plugin root is found.
///
/// The cache root can be an alias that resolves through a declared root.
/// Descendant link directories are not followed.
#[cfg(test)]
fn find_plugin_roots(
    context: &SkillDiscoveryReadContext,
    cache_dir: &Path,
    harness: &str,
    limits: PluginScanLimits,
    issues: &mut Vec<DiscoveryReadIssue>,
    state: &mut PluginScanState,
) -> Vec<EnumeratedPlugin> {
    PreparedPluginRoots::enumerate(context, cache_dir, limits, issues, state).materialize(
        &context.scope,
        harness,
        None,
        issues,
        state,
    )
}

struct PreparedPluginRoots {
    lookups: Vec<PreparedManifestLookup>,
    directories: Vec<PreparedPluginDirectory>,
}

struct PreparedPluginDirectory {
    path: PathBuf,
    observation: Result<(PathBuf, cap_std::fs::Metadata), ScopedReadError>,
}

impl PreparedPluginDirectory {
    fn unchanged(&self, scope: &SkillReadScope) -> bool {
        use cap_std::fs::MetadataExt;
        match (&self.observation, scope.resolved_path_metadata(&self.path)) {
            (Ok((path, before)), Ok((current, after))) => {
                path == &current
                    && before.is_dir()
                    && after.is_dir()
                    && before.dev() == after.dev()
                    && before.ino() == after.ino()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec()
            }
            (Err(ScopedReadError::Missing { .. }), Err(ScopedReadError::Missing { .. })) => true,
            _ => false,
        }
    }
}

impl PreparedPluginRoots {
    #[cfg(test)]
    fn enumerate(
        context: &SkillDiscoveryReadContext,
        cache_dir: &Path,
        limits: PluginScanLimits,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> Self {
        Self::enumerate_checked(context, cache_dir, limits, issues, state, &mut || Ok(()))
            .expect("unguarded plugin enumeration cannot be cancelled")
    }

    fn enumerate_checked(
        context: &SkillDiscoveryReadContext,
        cache_dir: &Path,
        limits: PluginScanLimits,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let mut plan = Self {
            lookups: Vec::new(),
            directories: vec![PreparedPluginDirectory {
                path: cache_dir.to_path_buf(),
                observation: context.scope.resolved_path_metadata(cache_dir),
            }],
        };
        match context.scope.resolved_dir_path(cache_dir) {
            Ok(_) => {}
            Err(ScopedReadError::Missing { .. }) => {
                state.membership = SourceReadOutcome::Absent;
                state.facts = SourceReadOutcome::Absent;
                return Ok(plan);
            }
            Err(error) => {
                io_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    cache_dir,
                    error.to_string(),
                );
                state.failed();
                return Ok(plan);
            }
        }
        walk_for_plugin_roots(context, cache_dir, limits, &mut plan, issues, state, check)?;
        check()?;
        Ok(plan)
    }

    #[cfg(test)]
    fn materialize(
        &self,
        scope: &SkillReadScope,
        harness: &str,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> Vec<EnumeratedPlugin> {
        self.materialize_checked(scope, harness, guard, issues, state, &mut || Ok(()))
            .expect("unguarded manifest materialization cannot be cancelled")
    }

    fn materialize_checked(
        &self,
        scope: &SkillReadScope,
        harness: &str,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Vec<EnumeratedPlugin>, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let mut found = Vec::new();
        let mut blocked: Option<PathBuf> = None;
        for lookup in &self.lookups {
            check()?;
            let root = lookup.root.clone();
            if blocked
                .as_ref()
                .is_some_and(|parent| root.starts_with(parent))
            {
                continue;
            }
            blocked = None;
            match lookup.materialize(scope, harness, guard, issues, state) {
                ManifestEvidence::Confirmed(name, version) => found.push(EnumeratedPlugin {
                    info: PluginInfo {
                        name,
                        version,
                        harness: harness.to_string(),
                    },
                    root,
                }),
                ManifestEvidence::Unknown => blocked = Some(root),
                ManifestEvidence::Absent => {}
            }
        }
        check()?;
        if !self.revalidate(scope, issues, state) {
            found.clear();
        }
        check()?;
        Ok(found)
    }

    fn revalidate(
        &self,
        scope: &SkillReadScope,
        issues: &mut Vec<DiscoveryReadIssue>,
        state: &mut PluginScanState,
    ) -> bool {
        if state.membership == SourceReadOutcome::Failed {
            return false;
        }
        let mut unchanged = true;
        for directory in &self.directories {
            if !directory.unchanged(scope) {
                io_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    &directory.path,
                    "Plugin cache directory changed or is unavailable after enumeration",
                );
                state.membership = SourceReadOutcome::Incomplete;
                state.facts = SourceReadOutcome::Incomplete;
                unchanged = false;
            }
        }
        unchanged
    }
}

fn walk_for_plugin_roots(
    context: &SkillDiscoveryReadContext,
    dir: &Path,
    limits: PluginScanLimits,
    plan: &mut PreparedPluginRoots,
    issues: &mut Vec<DiscoveryReadIssue>,
    state: &mut PluginScanState,
    check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
) -> Result<(), crate::skill_coordination::CoordinationFailure> {
    check()?;
    let Some(listing) = state.read_directory(&context.scope, dir, limits.directory_entries, issues)
    else {
        return Ok(());
    };
    let listing = match listing {
        Ok(entries) => entries,
        Err(ScopedReadError::Missing { .. }) => {
            state.incomplete();
            return Ok(());
        }
        Err(error) => {
            io_issue(issues, DiscoveryReadIssueKind::Root, dir, error.to_string());
            state.incomplete();
            return Ok(());
        }
    };
    check()?;
    append_directory_issues(issues, dir, listing.issues, state);
    for entry in listing.entries {
        check()?;
        let path = dir.join(&entry.name);
        let meta = entry.metadata;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        plan.directories.push(PreparedPluginDirectory {
            path: path.clone(),
            observation: context
                .scope
                .resolved_dir_path(&path)
                .map(|resolved| (resolved, meta)),
        });
        let lookup = PreparedManifestLookup::enumerate(&context.scope, &path);
        check()?;
        let descend = matches!(&lookup.observation, Ok(None));
        plan.lookups.push(lookup);
        if !descend {
            continue;
        }
        if limits.max_depth > 0 {
            walk_for_plugin_roots(
                context,
                &path,
                PluginScanLimits {
                    max_depth: limits.max_depth - 1,
                    ..limits
                },
                plan,
                issues,
                state,
                check,
            )?;
        } else {
            state.incomplete();
            io_issue(
                issues,
                DiscoveryReadIssueKind::Cap,
                &path,
                "Plugin traversal depth limit was reached",
            );
        }
    }
    check()
}

fn append_directory_issues(
    issues: &mut Vec<DiscoveryReadIssue>,
    dir: &Path,
    directory_issues: Vec<DirectoryReadIssue>,
    state: &mut PluginScanState,
) {
    for directory_issue in directory_issues {
        state.incomplete();
        let (kind, path, message) = match directory_issue {
            DirectoryReadIssue::Entry { name, source } => (
                DiscoveryReadIssueKind::Entry,
                name.map_or_else(|| dir.to_path_buf(), |name| dir.join(name)),
                source.to_string(),
            ),
            DirectoryReadIssue::LimitReached => (
                DiscoveryReadIssueKind::Root,
                dir.to_path_buf(),
                format!("Directory entry limit of {MAX_DIRECTORY_ENTRIES} was reached"),
            ),
            DirectoryReadIssue::Changed => (
                DiscoveryReadIssueKind::Root,
                dir.to_path_buf(),
                "Directory changed during plugin scan".to_string(),
            ),
            DirectoryReadIssue::DirectoryMetadata(source) => (
                DiscoveryReadIssueKind::Metadata,
                dir.to_path_buf(),
                source.to_string(),
            ),
        };
        io_issue(issues, kind, &path, message);
    }
}

/// Enumerate skills shipped by plugins in a plugin cache root
/// (`~/.claude/plugins/cache` or `~/.codex/plugins/cache`). Each plugin's
/// `skills/<name>` entry is required to be a real directory (not a symlink)
/// that is not a link. Its document must pass a scoped regular-file observation.
pub fn enumerate_plugin_skills(
    context: &SkillDiscoveryReadContext,
    cache_dir: &Path,
    harness: &str,
    requested_names: Option<&BTreeSet<String>>,
) -> PluginScanReport {
    let extent = if requested_names.is_some() {
        DiscoveryExtent::Named
    } else {
        DiscoveryExtent::Full
    };
    enumerate_plugin_skills_with_limits(
        context,
        cache_dir,
        harness,
        requested_names,
        DEFAULT_PLUGIN_SCAN_LIMITS,
        extent,
    )
}

fn enumerate_plugin_skills_with_limits(
    context: &SkillDiscoveryReadContext,
    cache_dir: &Path,
    harness: &str,
    requested_names: Option<&BTreeSet<String>>,
    limits: PluginScanLimits,
    extent: DiscoveryExtent,
) -> PluginScanReport {
    PreparedPluginCache::enumerate(context, cache_dir, harness, limits).materialize(
        context,
        requested_names,
        extent,
        None,
    )
}

struct PreparedPluginCache {
    path: PathBuf,
    harness: String,
    limits: PluginScanLimits,
    roots: PreparedPluginRoots,
    state: PluginScanState,
    issues: Vec<DiscoveryReadIssue>,
}

impl PreparedPluginCache {
    fn enumerate(
        context: &SkillDiscoveryReadContext,
        path: &Path,
        harness: &str,
        limits: PluginScanLimits,
    ) -> Self {
        Self::enumerate_checked(context, path, harness, limits, &mut || Ok(()))
            .expect("unguarded plugin cache enumeration cannot be cancelled")
    }

    fn enumerate_checked(
        context: &SkillDiscoveryReadContext,
        path: &Path,
        harness: &str,
        limits: PluginScanLimits,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let mut state = PluginScanState::read();
        state.work_remaining = limits.max_work;
        let mut issues = Vec::new();
        let roots = PreparedPluginRoots::enumerate_checked(
            context,
            path,
            limits,
            &mut issues,
            &mut state,
            check,
        )?;
        Ok(Self {
            path: path.to_path_buf(),
            harness: harness.to_string(),
            limits,
            roots,
            state,
            issues,
        })
    }

    fn materialize(
        self,
        context: &SkillDiscoveryReadContext,
        requested_names: Option<&BTreeSet<String>>,
        extent: DiscoveryExtent,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> PluginScanReport {
        self.prepare_membership(context, requested_names, extent, guard)
            .materialize(&context.scope)
    }

    fn prepare_membership(
        self,
        context: &SkillDiscoveryReadContext,
        requested_names: Option<&BTreeSet<String>>,
        extent: DiscoveryExtent,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> PreparedPluginMembership {
        self.prepare_membership_checked(context, requested_names, extent, guard, &mut || Ok(()))
            .expect("unguarded membership preparation cannot be cancelled")
    }

    fn prepare_membership_checked(
        self,
        context: &SkillDiscoveryReadContext,
        requested_names: Option<&BTreeSet<String>>,
        extent: DiscoveryExtent,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<PreparedPluginMembership, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let mut report = PluginScanReport {
            read_issues: self.issues,
            ..PluginScanReport::default()
        };
        let mut state = self.state;
        let plugins = self.roots.materialize_checked(
            &context.scope,
            &self.harness,
            guard,
            &mut report.read_issues,
            &mut state,
            check,
        )?;
        let mut directories = Vec::new();
        let mut skills = Vec::new();
        for plugin in plugins {
            check()?;
            let skills_dir = plugin.root.join("skills");
            let observation = context
                .scope
                .observe_entry(&plugin.root, std::ffi::OsStr::new("skills"));
            let Some(listing) = state.read_directory(
                &context.scope,
                &skills_dir,
                self.limits.directory_entries,
                &mut report.read_issues,
            ) else {
                break;
            };
            check()?;
            let listing = match listing {
                Ok(entries) => entries,
                Err(ScopedReadError::Missing { .. }) => {
                    directories.push(PreparedPluginSkillDirectory {
                        path: skills_dir,
                        observation,
                    });
                    continue;
                }
                Err(error) => {
                    io_issue(
                        &mut report.read_issues,
                        DiscoveryReadIssueKind::Root,
                        &skills_dir,
                        error.to_string(),
                    );
                    state.incomplete();
                    continue;
                }
            };
            directories.push(PreparedPluginSkillDirectory {
                path: skills_dir.clone(),
                observation,
            });
            append_directory_issues(
                &mut report.read_issues,
                &skills_dir,
                listing.issues,
                &mut state,
            );
            for entry in listing.entries {
                check()?;
                let skill_dir = skills_dir.join(&entry.name);
                let name = skill_dir
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if requested_names.is_some_and(|names| !names.contains(&name)) {
                    continue;
                }
                let meta = &entry.metadata;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    continue;
                }
                let document = skill_dir.join("SKILL.md");
                skills.push(PreparedPluginSkill {
                    skill_dir,
                    plugin: plugin.info.clone(),
                    entry: Arc::new(entry.into_observation()),
                    document: context.scope.observe_content_regular(&document),
                });
            }
        }
        check()?;
        Ok(PreparedPluginMembership {
            tree: self.roots,
            path: self.path,
            extent,
            state,
            report,
            directories,
            skills,
        })
    }
}

struct PreparedPluginSkill {
    skill_dir: PathBuf,
    plugin: PluginInfo,
    entry: Arc<crate::skill_scope::ScopedEntryObservation>,
    document: Result<crate::skill_scope::ScopedContentObservation, ScopedReadError>,
}

impl PreparedPluginSkill {
    fn validate_document(
        &self,
        scope: &SkillReadScope,
    ) -> Result<Option<crate::skill_scope::ScopedContentObservation>, String> {
        let path = self.skill_dir.join("SKILL.md");
        match &self.document {
            Ok(observation) => match scope.observe_content_regular(&path) {
                Ok(current) if current == *observation => Ok(Some(observation.clone())),
                _ => Err("Plugin skill document changed after enumeration".into()),
            },
            Err(ScopedReadError::Missing { .. }) => match scope.observe_content_regular(&path) {
                Err(ScopedReadError::Missing { .. }) => Ok(None),
                _ => Err(
                    "Plugin skill document appeared or became unavailable after enumeration".into(),
                ),
            },
            Err(error) => Err(error.to_string()),
        }
    }
}

struct PreparedPluginMembership {
    tree: PreparedPluginRoots,
    path: PathBuf,
    extent: DiscoveryExtent,
    state: PluginScanState,
    report: PluginScanReport,
    directories: Vec<PreparedPluginSkillDirectory>,
    skills: Vec<PreparedPluginSkill>,
}

struct PreparedPluginSkillDirectory {
    path: PathBuf,
    observation: Result<crate::skill_scope::ScopedEntryObservation, ScopedReadError>,
}

impl PreparedPluginSkillDirectory {
    fn revalidate(&self, scope: &SkillReadScope) -> Result<(), String> {
        match &self.observation {
            Ok(observation) => scope
                .resolve_observed_dir(observation)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            Err(ScopedReadError::Missing { .. }) => match scope.resolved_dir_path(&self.path) {
                Err(ScopedReadError::Missing { .. }) => Ok(()),
                _ => Err(
                    "Plugin skills directory appeared or became unavailable after enumeration"
                        .into(),
                ),
            },
            Err(error) => Err(error.to_string()),
        }
    }
}

impl PreparedPluginMembership {
    fn materialize(self, scope: &SkillReadScope) -> PluginScanReport {
        self.materialize_checked(scope, &mut || Ok(()))
            .expect("unguarded membership validation cannot be cancelled")
    }

    fn materialize_checked(
        mut self,
        scope: &SkillReadScope,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<PluginScanReport, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let mut failed_directories = BTreeSet::new();
        for directory in &self.directories {
            check()?;
            if let Err(message) = directory.revalidate(scope) {
                io_issue(
                    &mut self.report.read_issues,
                    DiscoveryReadIssueKind::Root,
                    &directory.path,
                    message,
                );
                self.state.incomplete();
                failed_directories.insert(directory.path.clone());
            }
        }
        for skill in &self.skills {
            check()?;
            if skill
                .skill_dir
                .parent()
                .is_some_and(|path| failed_directories.contains(path))
            {
                continue;
            }
            if let Err(error) = scope.resolve_observed_dir(&skill.entry) {
                io_issue(
                    &mut self.report.read_issues,
                    DiscoveryReadIssueKind::Entry,
                    &skill.skill_dir,
                    error.to_string(),
                );
                self.state.incomplete();
                continue;
            }
            let path = skill.skill_dir.join("SKILL.md");
            let result = skill.validate_document(scope);
            match result {
                Ok(Some(document_observation)) => self.report.skills.push(PluginSkillDir {
                    skill_dir: skill.skill_dir.clone(),
                    plugin: skill.plugin.clone(),
                    entry_observation: Arc::clone(&skill.entry),
                    document_observation,
                }),
                Ok(None) => {}
                Err(error) => {
                    io_issue(
                        &mut self.report.read_issues,
                        DiscoveryReadIssueKind::SkillDocument,
                        &path,
                        error.to_string(),
                    );
                    self.state.incomplete();
                }
            }
        }
        check()?;
        if !self
            .tree
            .revalidate(scope, &mut self.report.read_issues, &mut self.state)
        {
            self.report.skills.clear();
        }
        self.report.cache_proofs.push(PluginCacheProof {
            path: self.path.clone(),
            tree: self.tree,
            directories: self.directories,
            skills: self.skills,
            state: self.state,
        });
        self.report.coverage.push(SourceCoverage {
            path: self.path,
            source: MembershipSource::PluginCache,
            extent: self.extent,
            membership: self.state.membership,
            facts: self.state.facts,
        });
        check()?;
        Ok(self.report)
    }
}

/// Enumerate every plugin-shipped skill across the plugin harnesses we know
/// how to scan natively: Claude Code, Codex, Cursor, and Grok Build.
pub fn scan_plugin_skills(
    context: &SkillDiscoveryReadContext,
    requested_names: Option<&BTreeSet<String>>,
) -> PluginScanReport {
    PreparedPluginScan::enumerate(context).materialize(requested_names, None)
}

pub(crate) struct PreparedPluginScan<'a> {
    context: &'a SkillDiscoveryReadContext,
    caches: Vec<PreparedPluginCache>,
}

impl<'a> PreparedPluginScan<'a> {
    pub(crate) fn enumerate(context: &'a SkillDiscoveryReadContext) -> Self {
        Self {
            context,
            caches: prepare_plugin_caches(context),
        }
    }

    pub(crate) fn enumerate_checked(
        context: &'a SkillDiscoveryReadContext,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        let mut caches = Vec::new();
        for (path, harness) in PLUGIN_CACHE_ROOTS {
            check()?;
            caches.push(PreparedPluginCache::enumerate_checked(
                context,
                &context.home.join(path),
                harness,
                DEFAULT_PLUGIN_SCAN_LIMITS,
                check,
            )?);
        }
        check()?;
        Ok(Self { context, caches })
    }

    #[allow(dead_code)]
    pub(crate) fn materialize_coordinated(
        self,
        requested_names: Option<&BTreeSet<String>>,
        guard: crate::skill_coordination::CoordinatedReadGuard,
    ) -> Result<
        (
            PluginScanReport,
            crate::skill_coordination::CoordinatedReadGuard,
        ),
        crate::skill_coordination::CoordinationFailure,
    > {
        let files = self
            .caches
            .iter()
            .flat_map(|cache| &cache.roots.lookups)
            .flat_map(|lookup| {
                lookup
                    .observation
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .into_iter()
                    .flat_map(PreparedManifest::paths)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let guard = guard.extend_with_files(&self.context.scope, &files)?;
        let context = self.context;
        let report = self.materialize_checked(requested_names, Some(&guard), &mut || {
            guard.check_cancelled()
        })?;
        guard.revalidate(&context.scope)?;
        Ok((report, guard))
    }

    fn materialize(
        self,
        requested_names: Option<&BTreeSet<String>>,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> PluginScanReport {
        self.materialize_checked(requested_names, guard, &mut || Ok(()))
            .expect("unguarded plugin scan cannot be cancelled")
    }

    fn materialize_checked(
        self,
        requested_names: Option<&BTreeSet<String>>,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<PluginScanReport, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let context = self.context;
        let mut report = PluginScanReport::default();
        let extent = if requested_names.is_some() {
            DiscoveryExtent::Named
        } else {
            DiscoveryExtent::Full
        };
        let memberships = self
            .caches
            .into_iter()
            .map(|cache| {
                cache.prepare_membership_checked(context, requested_names, extent, guard, check)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for membership in memberships {
            let nested = membership.materialize_checked(&context.scope, check)?;
            report.skills.extend(nested.skills);
            report.read_issues.extend(nested.read_issues);
            report.coverage.extend(nested.coverage);
            report.cache_proofs.extend(nested.cache_proofs);
        }
        check()?;
        Ok(report)
    }
}

fn prepare_plugin_caches(context: &SkillDiscoveryReadContext) -> Vec<PreparedPluginCache> {
    PLUGIN_CACHE_ROOTS
        .iter()
        .map(|(path, harness)| {
            PreparedPluginCache::enumerate(
                context,
                &context.home.join(path),
                harness,
                DEFAULT_PLUGIN_SCAN_LIMITS,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plugin_membership_cancellation_never_returns_a_partial_report() {
        use crate::skill_coordination::{CancellationToken, CoordinationFailure};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let plugin = home.join(PLUGIN_CACHE_ROOTS[0].0).join("plugin");
        for name in ["alpha", "beta", "gamma"] {
            fs::create_dir_all(plugin.join("skills").join(name)).unwrap();
            fs::write(plugin.join("skills").join(name).join("SKILL.md"), "fixture").unwrap();
        }
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(home.to_path_buf(), vec![], vec![], vec![]);
        let names = BTreeSet::from(["beta".to_string()]);
        for named in [false, true] {
            let selection = named.then_some(&names);
            let mut complete_checks = 0;
            let report = PreparedPluginScan::enumerate(&context)
                .materialize_checked(selection, None, &mut || {
                    complete_checks += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(report.skills.len(), if named { 1 } else { 3 });
            assert_eq!(report.coverage.len(), PLUGIN_CACHE_ROOTS.len());
            for cancel_at in 1..=complete_checks {
                let token = CancellationToken::default();
                let mut checks = 0;
                let result = PreparedPluginScan::enumerate(&context).materialize_checked(
                    selection,
                    None,
                    &mut || {
                        checks += 1;
                        if checks == cancel_at {
                            token.cancel();
                        }
                        if token.is_cancelled() {
                            Err(CoordinationFailure::Cancelled)
                        } else {
                            Ok(())
                        }
                    },
                );
                assert!(matches!(result, Err(CoordinationFailure::Cancelled)));
                assert_eq!(checks, cancel_at);
            }
            assert_eq!(
                PreparedPluginScan::enumerate(&context)
                    .materialize_checked(selection, None, &mut || Ok(()))
                    .unwrap()
                    .skills
                    .len(),
                report.skills.len()
            );
        }
    }

    #[test]
    fn plugin_tree_cancellation_discards_each_partial_traversal() {
        use crate::skill_coordination::{CancellationToken, CoordinationFailure};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        for (cache, _) in PLUGIN_CACHE_ROOTS {
            for name in ["alpha", "beta"] {
                let plugin = home.join(cache).join("market").join(name).join("version");
                fs::create_dir_all(plugin.join("skills/example")).unwrap();
                fs::write(
                    plugin.join("plugin.json"),
                    format!(r#"{{"name":"{name}"}}"#),
                )
                .unwrap();
                fs::write(plugin.join("skills/example/SKILL.md"), "fixture").unwrap();
            }
        }
        let context = SkillDiscoveryReadContext::bind(home.to_path_buf(), vec![], vec![], vec![]);
        let mut complete_checks = 0;
        let complete = PreparedPluginScan::enumerate_checked(&context, &mut || {
            complete_checks += 1;
            Ok(())
        })
        .unwrap();
        let report = complete.materialize(None, None);
        assert_eq!(report.skills.len(), PLUGIN_CACHE_ROOTS.len() * 2);
        for cancel_at in 1..=complete_checks {
            let token = CancellationToken::default();
            let mut checks = 0;
            let result = PreparedPluginScan::enumerate_checked(&context, &mut || {
                checks += 1;
                if checks == cancel_at {
                    token.cancel();
                }
                if token.is_cancelled() {
                    Err(CoordinationFailure::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(CoordinationFailure::Cancelled)));
            assert_eq!(checks, cancel_at);
        }
        let fresh = PreparedPluginScan::enumerate_checked(&context, &mut || Ok(())).unwrap();
        assert_eq!(
            fresh.materialize(None, None).skills.len(),
            PLUGIN_CACHE_ROOTS.len() * 2
        );
    }

    #[test]
    fn prepared_plugin_scan_extends_guard_with_observed_manifest_paths() {
        for named in [false, true] {
            for changed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let mut manifests = Vec::new();
                for (cache, _) in PLUGIN_CACHE_ROOTS {
                    let plugin = home.join(cache).join("plugin");
                    fs::create_dir_all(plugin.join("skills/alpha")).unwrap();
                    fs::write(plugin.join("skills/alpha/SKILL.md"), "fixture").unwrap();
                    let manifest = plugin.join("plugin.json");
                    fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
                    manifests.push(manifest);
                }
                fs::hard_link(&manifests[0], home.join("manifest-alias")).unwrap();
                let context = SkillDiscoveryReadContext::bind(
                    home.to_path_buf(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                );
                let guard = manifest_guard(&context.scope, home, &[]);
                let plan = PreparedPluginScan::enumerate(&context);
                if changed {
                    fs::write(&manifests[0], r#"{"name":"changed"}"#).unwrap();
                }
                let names = BTreeSet::from(["alpha".to_string()]);
                let (report, guard) = plan
                    .materialize_coordinated(named.then_some(&names), guard)
                    .unwrap();
                assert_eq!(
                    report.skills.len(),
                    PLUGIN_CACHE_ROOTS.len() - usize::from(changed)
                );
                assert_eq!(report.coverage.len(), PLUGIN_CACHE_ROOTS.len());
                assert_eq!(
                    report.coverage[0].membership,
                    if changed {
                        SourceReadOutcome::Incomplete
                    } else {
                        SourceReadOutcome::Read
                    }
                );
                for path in &manifests {
                    assert!(!guard.read(&context.scope, path, 256).unwrap().is_empty());
                }
                let document = home
                    .join(PLUGIN_CACHE_ROOTS[1].0)
                    .join("plugin/skills/alpha/SKILL.md");
                assert!(guard.read(&context.scope, &document, 256).is_err());
                let guard = guard
                    .extend_with_files(&context.scope, std::slice::from_ref(&document))
                    .unwrap();
                assert_eq!(
                    guard.read(&context.scope, &document, 256).unwrap(),
                    b"fixture"
                );
                guard.revalidate(&context.scope).unwrap();
            }
        }
    }

    #[test]
    fn retained_plugin_documents_reject_late_changes_with_named_scope() {
        for named in [false, true] {
            for selected in [false, true] {
                for change in ["rewrite", "remove", "appear"] {
                    let temp = tempfile::tempdir().unwrap();
                    let plugin = temp.path().join("cache/plugin");
                    for name in ["alpha", "beta"] {
                        let skill = plugin.join("skills").join(name);
                        fs::create_dir_all(&skill).unwrap();
                        fs::write(skill.join("SKILL.md"), "before").unwrap();
                    }
                    let document = plugin
                        .join("skills")
                        .join(if selected { "alpha" } else { "beta" })
                        .join("SKILL.md");
                    if change == "appear" {
                        fs::remove_file(&document).unwrap();
                    }
                    fs::write(plugin.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
                    let context = SkillDiscoveryReadContext::bind(
                        temp.path().to_path_buf(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    );
                    let names = BTreeSet::from(["alpha".to_string()]);
                    let report = PreparedPluginCache::enumerate(
                        &context,
                        &temp.path().join("cache"),
                        "Fixture",
                        DEFAULT_PLUGIN_SCAN_LIMITS,
                    )
                    .materialize(
                        &context,
                        named.then_some(&names),
                        if named {
                            DiscoveryExtent::Named
                        } else {
                            DiscoveryExtent::Full
                        },
                        None,
                    );
                    assert_eq!(report.cache_proofs.len(), 1);
                    let proof = &report.cache_proofs[0];
                    assert!(proof.revalidate(&context.scope, &mut Vec::new()).is_empty());
                    if change == "remove" {
                        fs::remove_file(&document).unwrap();
                    } else {
                        fs::write(&document, "changed").unwrap();
                    }
                    let mut issues = Vec::new();
                    assert_eq!(
                        proof.revalidate(&context.scope, &mut issues).is_empty(),
                        named && !selected
                    );
                    if !named || selected {
                        assert!(issues
                            .iter()
                            .any(|issue| issue.kind == DiscoveryReadIssueKind::SkillDocument));
                    }
                }
            }
        }
    }

    #[test]
    fn plugin_cache_tree_changes_invalidate_retained_membership() {
        for after_membership in [false, true] {
            for named in [false, true] {
                for change in [
                    "unchanged",
                    "new_cache",
                    "new_sibling",
                    "replace_group",
                    "remove_group",
                ] {
                    let temp = tempfile::tempdir().unwrap();
                    let cache = temp.path().join("cache");
                    let group = cache.join("group");
                    let plugin = group.join("plugin");
                    let make_plugin = |path: &Path| {
                        fs::create_dir_all(path.join("skills/alpha")).unwrap();
                        fs::write(path.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
                        fs::write(
                            path.join("skills/alpha/SKILL.md"),
                            "---\nname: alpha\ndescription: fixture\n---\n",
                        )
                        .unwrap();
                    };
                    if change != "new_cache" {
                        make_plugin(&plugin);
                    }
                    let context = SkillDiscoveryReadContext::bind(
                        temp.path().to_path_buf(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    );
                    let plan = PreparedPluginCache::enumerate(
                        &context,
                        &cache,
                        "Fixture",
                        DEFAULT_PLUGIN_SCAN_LIMITS,
                    );
                    let names = BTreeSet::from(["alpha".to_string()]);
                    let extent = if named {
                        DiscoveryExtent::Named
                    } else {
                        DiscoveryExtent::Full
                    };
                    let change_tree = || match change {
                        "new_cache" => make_plugin(&plugin),
                        "new_sibling" => make_plugin(&group.join("sibling")),
                        "replace_group" => {
                            fs::rename(&group, temp.path().join("old-group")).unwrap();
                            make_plugin(&plugin);
                        }
                        "remove_group" => fs::remove_dir_all(&group).unwrap(),
                        _ => {}
                    };
                    if !after_membership {
                        change_tree();
                    }
                    let membership =
                        plan.prepare_membership(&context, named.then_some(&names), extent, None);
                    if after_membership {
                        change_tree();
                    }
                    let report = membership.materialize(&context.scope);
                    assert_eq!(report.coverage.len(), 1);
                    assert_eq!(report.coverage[0].extent, extent);
                    if change == "unchanged" {
                        assert_eq!(report.skills.len(), 1);
                        assert_eq!(report.coverage[0].membership, SourceReadOutcome::Read);
                        assert!(report.read_issues.is_empty());
                    } else {
                        assert!(report.skills.is_empty());
                        assert_eq!(report.coverage[0].membership, SourceReadOutcome::Incomplete);
                        assert_eq!(report.coverage[0].facts, SourceReadOutcome::Incomplete);
                        assert!(report
                            .read_issues
                            .iter()
                            .any(|issue| issue.kind == DiscoveryReadIssueKind::Root));
                    }
                }
            }
        }
    }

    #[test]
    fn manifest_pass_keeps_observation_identity_and_replays_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("plugin");
        fs::create_dir(&owner).unwrap();
        let context =
            SkillDiscoveryReadContext::bind(temp.path().to_path_buf(), vec![], vec![], vec![]);
        let absent = Arc::new(PreparedManifestLookup::enumerate(&context.scope, &owner));
        fs::write(owner.join("plugin.json"), r#"{"name":"now-present"}"#).unwrap();
        let present = Arc::new(PreparedManifestLookup::enumerate(&context.scope, &owner));
        let mut pass = ManifestReadPass::new(&context.scope, None);
        let mut first_issues = Vec::new();
        assert!(matches!(
            pass.materialize(&absent, "Codex", &mut first_issues),
            ManifestEvidence::Unknown
        ));
        assert!(!first_issues.is_empty());
        let mut repeated_issues = Vec::new();
        assert!(matches!(
            pass.materialize(&absent, "Codex", &mut repeated_issues),
            ManifestEvidence::Unknown
        ));
        assert_eq!(
            serde_json::to_value(&first_issues).unwrap(),
            serde_json::to_value(&repeated_issues).unwrap()
        );
        let mut present_issues = Vec::new();
        assert!(
            matches!(pass.materialize(&present, "Codex", &mut present_issues), ManifestEvidence::Confirmed(name, _) if name == "now-present")
        );
        assert!(present_issues.is_empty());
        let mut validation = ManifestValidationPass::new(&context.scope);
        assert!(!validation.unchanged(&absent));
        assert!(validation.unchanged(&present));
    }

    #[test]
    fn portable_codex_identity_does_not_replace_other_harness_identities() {
        for recognized in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("package");
            let skill = root.join("skills/alpha");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir(root.join(".codex-plugin")).unwrap();
            fs::write(
                root.join(".codex-plugin/plugin.json"),
                r#"{"name":"compatibility","version":"old"}"#,
            )
            .unwrap();
            let schema = if recognized {
                "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
            } else {
                "https://example.invalid/plugin.schema.json"
            };
            fs::write(
                root.join("plugin.json"),
                serde_json::to_vec(&serde_json::json!({
                    "$schema": schema, "name": "portable", "version": "new",
                    "extensions": {"com.openai": {"name": "not-package-identity"}}
                }))
                .unwrap(),
            )
            .unwrap();
            let context =
                SkillDiscoveryReadContext::bind(temp.path().to_path_buf(), vec![], vec![], vec![]);
            let mut observations = PluginObservationCache::new(&context);
            let claude =
                PreparedPluginAncestry::enumerate_cached(&mut observations, &skill, "Claude Code");
            let codex =
                PreparedPluginAncestry::enumerate_cached(&mut observations, &skill, "Codex");
            let mut files = BTreeSet::new();
            codex.append_regular_files(&mut files);
            let guard = manifest_guard(
                &context.scope,
                temp.path(),
                &files.into_iter().collect::<Vec<_>>(),
            );
            let mut reads = ManifestReadPass::new(&context.scope, Some(&guard));
            for (plan, expected) in [
                (&claude, "compatibility"),
                (
                    &codex,
                    if recognized {
                        "portable"
                    } else {
                        "compatibility"
                    },
                ),
                (&claude, "compatibility"),
            ] {
                let mut issues = Vec::new();
                let evidence = plan.materialize_with_pass(&mut reads, &mut issues);
                assert!(
                    matches!(evidence, PluginEvidence::Confirmed(PluginInfo {name, version, ..})
                    if name == expected && version.as_deref() == Some(if expected == "portable" {"new"} else {"old"}))
                );
                assert!(issues.is_empty(), "{issues:?}");
            }
            guard.revalidate(&context.scope).unwrap();
        }
    }

    #[test]
    fn portable_identity_requires_a_guard_for_both_observed_manifests() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(".codex-plugin")).unwrap();
        let compatibility = temp.path().join(".codex-plugin/plugin.json");
        fs::write(&compatibility, r#"{"name":"compatibility"}"#).unwrap();
        fs::write(temp.path().join("plugin.json"), r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"portable"}"#).unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let prepared = PreparedManifest::enumerate(&scope, temp.path())
            .unwrap()
            .unwrap();
        let guard = manifest_guard(&scope, temp.path(), &[compatibility]);
        let mut issues = Vec::new();
        let mut state = PluginScanState::read();
        assert!(matches!(
            prepared.materialize(&scope, "Codex", Some(&guard), &mut issues, &mut state),
            ManifestEvidence::Unknown
        ));
        assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        assert!(!issues.is_empty());
    }

    #[test]
    fn portable_manifest_changes_invalidate_prepared_identity() {
        for existed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir(temp.path().join(".codex-plugin")).unwrap();
            fs::write(
                temp.path().join(".codex-plugin/plugin.json"),
                r#"{"name":"compatibility"}"#,
            )
            .unwrap();
            let portable = temp.path().join("plugin.json");
            if existed {
                fs::write(&portable, r#"{"name":"before"}"#).unwrap();
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let prepared = PreparedManifest::enumerate(&scope, temp.path())
                .unwrap()
                .unwrap();
            fs::write(&portable, r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"after"}"#).unwrap();
            let mut issues = Vec::new();
            let mut state = PluginScanState::read();
            assert!(matches!(
                prepared.materialize(&scope, "Codex", None, &mut issues, &mut state),
                ManifestEvidence::Unknown
            ));
            assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        }
    }

    #[test]
    fn shared_manifest_results_keep_each_harness_identity() {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("plugin");
        let skill = owner.join("skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            owner.join("plugin.json"),
            r#"{"name":"shared","version":"1"}"#,
        )
        .unwrap();
        let context =
            SkillDiscoveryReadContext::bind(temp.path().to_path_buf(), vec![], vec![], vec![]);
        let mut observations = PluginObservationCache::new(&context);
        let claude =
            PreparedPluginAncestry::enumerate_cached(&mut observations, &skill, "Claude Code");
        let codex = PreparedPluginAncestry::enumerate_cached(&mut observations, &skill, "Codex");
        let mut reads = ManifestReadPass::new(&context.scope, None);
        for (ancestry, expected_harness) in [(&claude, "Claude Code"), (&codex, "Codex")] {
            let mut issues = Vec::new();
            assert!(
                matches!(ancestry.materialize_with_pass(&mut reads, &mut issues), PluginEvidence::Confirmed(PluginInfo { name, version, harness }) if name == "shared" && version.as_deref() == Some("1") && harness == expected_harness)
            );
            assert!(issues.is_empty());
        }
    }

    #[test]
    fn ancestry_observations_are_shared_without_adopting_mid_pass_changes() {
        for initially_present in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let owner = temp.path().join("plugin");
            let alpha = owner.join("skills/alpha");
            let beta = owner.join("skills/beta");
            fs::create_dir_all(&alpha).unwrap();
            fs::create_dir_all(&beta).unwrap();
            let manifest = owner.join("plugin.json");
            if initially_present {
                fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
            }
            let context = SkillDiscoveryReadContext::bind(
                temp.path().to_path_buf(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            let mut cache = PluginObservationCache::new(&context);
            let first = PreparedPluginAncestry::enumerate_cached(&mut cache, &alpha, "Fixture");
            fs::write(&manifest, r#"{"name":"after"}"#).unwrap();
            let second = PreparedPluginAncestry::enumerate_cached(&mut cache, &beta, "Fixture");
            let physical_owner = context.scope.resolved_dir_path(&owner).unwrap();
            let first_owner = first
                .resolved
                .lookups
                .iter()
                .find(|lookup| lookup.root == physical_owner)
                .unwrap();
            let second_owner = second
                .resolved
                .lookups
                .iter()
                .find(|lookup| lookup.root == physical_owner)
                .unwrap();
            assert!(Arc::ptr_eq(first_owner, second_owner));
            for plan in [first, second] {
                let mut issues = Vec::new();
                assert!(matches!(
                    plan.materialize(&context.scope, None, &mut issues),
                    PluginEvidence::Unknown
                ));
                assert!(!issues.is_empty());
            }
            let fresh = PreparedPluginAncestry::enumerate(&context, &beta, "Fixture");
            let mut issues = Vec::new();
            assert!(
                matches!(fresh.materialize(&context.scope, None, &mut issues), PluginEvidence::Confirmed(PluginInfo { name, .. }) if name == "after")
            );
            assert!(issues.is_empty());
        }
    }

    #[test]
    fn prepared_plugin_membership_rejects_changes_and_ignores_unrequested_documents() {
        for change in [
            "document",
            "missing-document",
            "new-skill",
            "missing-skills-root",
            "unrequested-document",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let cache = temp.path().join("cache");
            let plugin = cache.join("plugin");
            fs::create_dir_all(&plugin).unwrap();
            fs::write(plugin.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
            let skills = plugin.join("skills");
            let selected = skills.join("selected");
            let unrequested = skills.join("unrequested");
            if change != "missing-skills-root" {
                for skill in [&selected, &unrequested] {
                    fs::create_dir_all(skill).unwrap();
                    fs::write(skill.join("SKILL.md"), "before").unwrap();
                }
                if change == "missing-document" {
                    fs::remove_file(selected.join("SKILL.md")).unwrap();
                }
            }
            let context = SkillDiscoveryReadContext::bind(
                temp.path().to_path_buf(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            let names = BTreeSet::from(["selected".to_string()]);
            let plan = PreparedPluginCache::enumerate(
                &context,
                &cache,
                "Fixture",
                DEFAULT_PLUGIN_SCAN_LIMITS,
            )
            .prepare_membership(&context, Some(&names), DiscoveryExtent::Named, None);
            match change {
                "document" | "missing-document" => {
                    fs::write(selected.join("SKILL.md"), "after").unwrap()
                }
                "unrequested-document" => fs::remove_file(unrequested.join("SKILL.md")).unwrap(),
                "new-skill" => fs::create_dir(skills.join("new")).unwrap(),
                _ => {
                    fs::create_dir_all(&selected).unwrap();
                    fs::write(selected.join("SKILL.md"), "new").unwrap();
                }
            }
            let report = plan.materialize(&context.scope);
            assert_eq!(report.coverage.len(), 1);
            if change == "unrequested-document" {
                assert_eq!(report.skills.len(), 1);
                assert_eq!(report.skills[0].skill_dir, selected);
                assert_eq!(report.coverage[0].membership, SourceReadOutcome::Read);
                assert!(report.read_issues.is_empty());
            } else {
                assert!(report.skills.is_empty());
                assert_eq!(report.coverage[0].membership, SourceReadOutcome::Incomplete);
                assert!(!report.read_issues.is_empty());
            }
        }
    }

    #[test]
    fn prepared_ancestry_rejects_stale_or_new_manifests() {
        for initially_present in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("plugin");
            let skill = root.join("skills/alpha");
            fs::create_dir_all(&skill).unwrap();
            let manifest = root.join("plugin.json");
            if initially_present {
                fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
            }
            let context = SkillDiscoveryReadContext::bind(
                temp.path().to_path_buf(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            let plan = PreparedPluginAncestry::enumerate(&context, &skill, "Fixture");
            fs::write(&manifest, r#"{"name":"after"}"#).unwrap();
            let physical_manifest = context
                .scope
                .resolved_dir_path(&root)
                .unwrap()
                .join("plugin.json");
            let guard = manifest_guard(&context.scope, temp.path(), &[physical_manifest, manifest]);
            let mut issues = Vec::new();
            let result = plan.materialize(&context.scope, Some(&guard), &mut issues);
            assert!(matches!(result, PluginEvidence::Unknown));
            assert!(!issues.is_empty());
        }
    }

    #[test]
    fn guarded_ancestry_keeps_physical_precedence_without_reading_lexical_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let physical = temp.path().join("physical");
        let lexical = temp.path().join("lexical");
        let skill = physical.join("skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::create_dir_all(lexical.join("skills")).unwrap();
        fs::write(physical.join("plugin.json"), r#"{"name":"physical"}"#).unwrap();
        fs::write(lexical.join("plugin.json"), r#"{"name":"lexical"}"#).unwrap();
        let alias = lexical.join("skills/alpha");
        std::os::unix::fs::symlink(&skill, &alias).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            temp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let plan = PreparedPluginAncestry::enumerate(&context, &alias, "Fixture");
        let paths = plan
            .resolved
            .lookups
            .iter()
            .flat_map(|lookup| {
                lookup
                    .observation
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .into_iter()
                    .flat_map(PreparedManifest::paths)
            })
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let guard = manifest_guard(&context.scope, temp.path(), &paths);
        let mut issues = Vec::new();
        let result = plan.materialize(&context.scope, Some(&guard), &mut issues);
        assert!(
            matches!(result, PluginEvidence::Confirmed(PluginInfo { name, harness, .. }) if name == "physical" && harness == "Fixture")
        );
        assert!(issues.is_empty());
        guard.revalidate(&context.scope).unwrap();
    }

    #[test]
    fn prepared_cache_set_keeps_named_coverage_and_isolates_stale_manifests() {
        for stale in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path();
            let mut manifests = Vec::new();
            for (cache, _) in PLUGIN_CACHE_ROOTS {
                let root = home.join(cache).join("fixture-plugin");
                for name in ["selected", "unrequested"] {
                    let skill = root.join("skills").join(name);
                    fs::create_dir_all(&skill).unwrap();
                    fs::write(skill.join("SKILL.md"), "fixture skill").unwrap();
                }
                let manifest = root.join("plugin.json");
                fs::write(&manifest, r#"{"name":"fixture-plugin"}"#).unwrap();
                manifests.push(manifest);
            }
            let context = SkillDiscoveryReadContext::bind(
                home.to_path_buf(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            let caches = prepare_plugin_caches(&context);
            assert_eq!(caches.len(), PLUGIN_CACHE_ROOTS.len());
            let observed: BTreeSet<_> = caches
                .iter()
                .flat_map(|cache| cache.roots.lookups.iter())
                .filter_map(|lookup| match &lookup.observation {
                    Ok(Some(manifest)) => Some(manifest.observation.requested.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(observed, manifests.iter().cloned().collect());
            if stale {
                fs::write(manifests.last().unwrap(), r#"{"name":"changed"}"#).unwrap();
            }
            let guard = manifest_guard(&context.scope, home, &manifests);
            let names = BTreeSet::from(["selected".to_string()]);
            for (index, cache) in caches.into_iter().enumerate() {
                let report =
                    cache.materialize(&context, Some(&names), DiscoveryExtent::Named, Some(&guard));
                assert_eq!(report.coverage.len(), 1);
                let coverage = &report.coverage[0];
                assert_eq!(coverage.path, home.join(PLUGIN_CACHE_ROOTS[index].0));
                assert_eq!(coverage.extent, DiscoveryExtent::Named);
                if stale && index == PLUGIN_CACHE_ROOTS.len() - 1 {
                    assert!(report.skills.is_empty());
                    assert_eq!(coverage.membership, SourceReadOutcome::Incomplete);
                    assert!(!report.read_issues.is_empty());
                } else {
                    assert_eq!(report.skills.len(), 1);
                    assert!(report.skills[0].skill_dir.ends_with("selected"));
                    assert_eq!(report.skills[0].plugin.harness, PLUGIN_CACHE_ROOTS[index].1);
                    assert_eq!(coverage.membership, SourceReadOutcome::Read);
                    assert!(report.read_issues.is_empty());
                }
            }
            guard.revalidate(&context.scope).unwrap();
        }
    }

    #[test]
    fn plugin_root_plan_collects_manifests_and_stops_at_present_roots() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let group = cache.join("group");
        let alpha = group.join("alpha");
        let beta = group.join("beta");
        let hidden = alpha.join("hidden");
        for (root, name) in [(&alpha, "alpha"), (&beta, "beta"), (&hidden, "hidden")] {
            fs::create_dir_all(root).unwrap();
            fs::write(root.join("plugin.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
        }
        let context = SkillDiscoveryReadContext::bind(
            temp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut issues = Vec::new();
        let mut state = PluginScanState::read();
        state.work_remaining = DEFAULT_PLUGIN_SCAN_LIMITS.max_work;
        let plan = PreparedPluginRoots::enumerate(
            &context,
            &cache,
            DEFAULT_PLUGIN_SCAN_LIMITS,
            &mut issues,
            &mut state,
        );
        assert_eq!(
            plan.lookups
                .iter()
                .map(|lookup| lookup.root.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([group, alpha.clone(), beta.clone()])
        );
        let guard = manifest_guard(
            &context.scope,
            temp.path(),
            &[alpha.join("plugin.json"), beta.join("plugin.json")],
        );
        let roots = plan.materialize(
            &context.scope,
            "Fixture",
            Some(&guard),
            &mut issues,
            &mut state,
        );
        assert_eq!(
            roots
                .iter()
                .map(|root| root.info.name.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["alpha".to_string(), "beta".to_string()])
        );
        assert!(issues.is_empty());
        assert_eq!(state.membership, SourceReadOutcome::Read);
        guard.revalidate(&context.scope).unwrap();
    }

    #[test]
    fn plugin_root_plan_rejects_new_ancestor_manifest_and_skips_its_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let group = cache.join("group");
        let child = group.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("plugin.json"), r#"{"name":"child"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            temp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut issues = Vec::new();
        let mut state = PluginScanState::read();
        let plan = PreparedPluginRoots::enumerate(
            &context,
            &cache,
            DEFAULT_PLUGIN_SCAN_LIMITS,
            &mut issues,
            &mut state,
        );
        fs::write(group.join("plugin.json"), r#"{"name":"new-parent"}"#).unwrap();
        let roots = plan.materialize(&context.scope, "Fixture", None, &mut issues, &mut state);
        assert!(roots.is_empty());
        assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        assert!(issues.iter().any(|issue| issue.path == group));
    }

    #[test]
    fn plugin_root_plan_rejects_manifest_changes_before_guard_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let root = cache.join("plugin");
        fs::create_dir_all(&root).unwrap();
        let manifest = root.join("plugin.json");
        fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            temp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut issues = Vec::new();
        let mut state = PluginScanState::read();
        let plan = PreparedPluginRoots::enumerate(
            &context,
            &cache,
            DEFAULT_PLUGIN_SCAN_LIMITS,
            &mut issues,
            &mut state,
        );
        fs::write(&manifest, r#"{"name":"after"}"#).unwrap();
        let guard = manifest_guard(&context.scope, temp.path(), &[manifest]);
        let roots = plan.materialize(
            &context.scope,
            "Fixture",
            Some(&guard),
            &mut issues,
            &mut state,
        );
        assert!(roots.is_empty());
        assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        assert!(!issues.is_empty());
    }

    fn manifest_guard(
        scope: &SkillReadScope,
        root: &Path,
        paths: &[PathBuf],
    ) -> crate::skill_coordination::CoordinatedReadGuard {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(root, CoordinationMode::Shared)],
            root,
            Some(std::time::Duration::from_secs(10)),
        )
        .unwrap()
        .acquire()
        .unwrap()
        .continue_with_files(scope, paths, CoordinationMode::Shared)
        .unwrap()
    }

    #[test]
    fn prepared_manifest_preserves_valid_identity_and_malformed_fallback_under_a_guard() {
        for malformed in [false, true] {
            for hard_link in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("fallback-name");
                fs::create_dir(&root).unwrap();
                let path = root.join("plugin.json");
                fs::write(
                    &path,
                    if malformed {
                        "{"
                    } else {
                        r#"{"name":"declared","version":"1.2"}"#
                    },
                )
                .unwrap();
                if hard_link {
                    fs::hard_link(&path, temp.path().join("alias")).unwrap();
                }
                let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
                let prepared = PreparedManifest::enumerate(&scope, &root).unwrap().unwrap();
                let guard = manifest_guard(&scope, temp.path(), &[path]);
                let mut issues = Vec::new();
                let mut state = PluginScanState::read();
                let result =
                    prepared.materialize(&scope, "Codex", Some(&guard), &mut issues, &mut state);
                assert!(matches!(result, ManifestEvidence::Confirmed(name, version)
                    if name == if malformed { "fallback-name" } else { "declared" }
                    && version == if malformed { None } else { Some("1.2".into()) }));
                assert_eq!(issues.is_empty(), !malformed);
                guard.revalidate(&scope).unwrap();
            }
        }
    }

    #[test]
    fn prepared_manifest_rejects_a_newer_guard_or_an_unplanned_file() {
        for planned in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("plugin.json");
            fs::write(&path, r#"{"name":"original"}"#).unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let prepared = PreparedManifest::enumerate(&scope, temp.path())
                .unwrap()
                .unwrap();
            let paths = if planned {
                fs::write(&path, r#"{"name":"changed"}"#).unwrap();
                vec![path]
            } else {
                Vec::new()
            };
            let guard = manifest_guard(&scope, temp.path(), &paths);
            let mut issues = Vec::new();
            let mut state = PluginScanState::read();
            assert!(matches!(
                prepared.materialize(&scope, "Codex", Some(&guard), &mut issues, &mut state),
                ManifestEvidence::Unknown
            ));
            assert!(!issues.is_empty());
            assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        }
    }

    #[test]
    fn prepared_manifest_rejects_a_new_higher_priority_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugin.json");
        fs::write(&path, r#"{"name":"generic"}"#).unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let prepared = PreparedManifest::enumerate(&scope, temp.path())
            .unwrap()
            .unwrap();
        let preferred = temp.path().join(".claude-plugin/plugin.json");
        fs::create_dir(preferred.parent().unwrap()).unwrap();
        fs::write(&preferred, r#"{"name":"preferred"}"#).unwrap();
        let guard = manifest_guard(&scope, temp.path(), &[path]);
        let mut issues = Vec::new();
        let mut state = PluginScanState::read();
        assert!(matches!(
            prepared.materialize(&scope, "Codex", Some(&guard), &mut issues, &mut state),
            ManifestEvidence::Unknown
        ));
        assert_eq!(issues[0].path, preferred.to_string_lossy());
    }

    fn enumerate_plugin_skills(
        cache_dir: &Path,
        harness: &str,
        requested_names: Option<&BTreeSet<String>>,
    ) -> PluginScanReport {
        let bound = if cache_dir.is_dir() {
            cache_dir
        } else {
            cache_dir.parent().unwrap()
        };
        let context = SkillDiscoveryReadContext::bind(
            bound.to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        super::enumerate_plugin_skills(&context, cache_dir, harness, requested_names)
    }

    fn scan_plugin_skills(
        home: &Path,
        requested_names: Option<&BTreeSet<String>>,
    ) -> PluginScanReport {
        let context =
            SkillDiscoveryReadContext::bind(home.to_path_buf(), Vec::new(), Vec::new(), Vec::new());
        super::scan_plugin_skills(&context, requested_names)
    }

    fn lookup(
        root: &Path,
        path: &Path,
        harness: &str,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> PluginEvidence {
        let context =
            SkillDiscoveryReadContext::bind(root.to_path_buf(), Vec::new(), Vec::new(), Vec::new());
        find_plugin_root(&context, path, harness, issues)
    }

    fn write_plugin_skill(home: &Path, cache_path: &str, name: &str) -> PathBuf {
        let plugin_root = home.join(cache_path);
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            format!(r#"{{"name":"{name}"}}"#),
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\n").unwrap();
        skill_dir
    }

    #[test]
    fn plugin_document_size_does_not_use_the_manifest_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = write_plugin_skill(
            tmp.path(),
            ".claude/plugins/cache/market/plugin/v1",
            "large",
        );
        fs::write(skill.join("SKILL.md"), "x".repeat(MAX_MANIFEST_BYTES + 1)).unwrap();
        let report = scan_plugin_skills(tmp.path(), None);
        assert!(report.skills.iter().any(|entry| entry.skill_dir == skill));
        assert!(report.read_issues.is_empty());
    }

    #[test]
    fn claude_plugin_cache_tree_is_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp
            .path()
            .join("cache/some-marketplace/sentry-toolkit/1.0.0");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name": "sentry-toolkit", "version": "1.0.0"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/lint-code");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: lint-code\n---\n").unwrap();

        let found = enumerate_plugin_skills(&tmp.path().join("cache"), "Claude Code", None).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin.name, "sentry-toolkit");
        assert_eq!(found[0].plugin.version.as_deref(), Some("1.0.0"));
        assert_eq!(found[0].plugin.harness, "Claude Code");
        assert_eq!(found[0].skill_dir, skill_dir);
    }

    #[test]
    fn scan_plugin_skills_uses_only_the_supplied_home() {
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let skill_a = write_plugin_skill(
            home_a.path(),
            ".claude/plugins/cache/marketplace/alpha/1.0.0",
            "alpha",
        );
        let skill_b = write_plugin_skill(
            home_b.path(),
            ".codex/plugins/cache/marketplace/beta/1.0.0",
            "beta",
        );

        let found_a = scan_plugin_skills(home_a.path(), None).skills;
        let found_b = scan_plugin_skills(home_b.path(), None).skills;

        assert_eq!(found_a.len(), 1);
        assert_eq!(found_a[0].skill_dir, skill_a);
        assert_eq!(found_a[0].plugin.name, "alpha");
        assert_eq!(found_a[0].plugin.harness, "Claude Code");
        assert_eq!(found_b.len(), 1);
        assert_eq!(found_b[0].skill_dir, skill_b);
        assert_eq!(found_b[0].plugin.name, "beta");
        assert_eq!(found_b[0].plugin.harness, "Codex");
        assert!(!home_a.path().join(".codex/plugins/cache").exists());
        assert!(!home_b.path().join(".claude/plugins/cache").exists());
    }

    #[test]
    fn generic_plugin_json_is_found_by_find_plugin_root() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("my-plugin");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{"$schema": "https://agent-plugins.org/schema.json", "name": "my-plugin", "description": "does things"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/do-things");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: do-things\n---\n").unwrap();

        let info = match lookup(tmp.path(), &skill_dir, "Codex", &mut Vec::new()) {
            PluginEvidence::Confirmed(info) => info,
            other => panic!("unexpected evidence: {other:?}"),
        };
        assert_eq!(info.name, "my-plugin");
        assert_eq!(info.harness, "Codex");
        assert!(info.version.is_none());
    }

    #[test]
    fn cursor_cache_plugin_tree_is_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("cache/cursor-public/linear/abc123");
        fs::create_dir_all(plugin_root.join(".cursor-plugin")).unwrap();
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name": "linear"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/linear-issues");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: linear-issues\n---\n",
        )
        .unwrap();

        let found = enumerate_plugin_skills(&tmp.path().join("cache"), "Cursor", None).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin.name, "linear");
        assert_eq!(found[0].plugin.harness, "Cursor");
    }

    #[test]
    fn cursor_local_plugin_checkout_is_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("local/sentry");
        fs::create_dir_all(plugin_root.join(".cursor-plugin")).unwrap();
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name": "sentry"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/triage");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: triage\n---\n").unwrap();

        let found = enumerate_plugin_skills(&tmp.path().join("local"), "Cursor", None).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin.name, "sentry");
    }

    #[test]
    fn grok_plugin_tree_is_enumerated_with_grok_build_harness() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("foo");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(plugin_root.join("plugin.json"), r#"{"name": "foo"}"#).unwrap();
        let skill_dir = plugin_root.join("skills/do-things");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: do-things\n---\n").unwrap();

        let found = enumerate_plugin_skills(tmp.path(), "Grok Build", None).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin.harness, "Grok Build");
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_skill_dir_escaping_the_plugin_root_is_not_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside-evil");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("SKILL.md"), "---\nname: evil\n---\n").unwrap();

        let plugin_root = tmp.path().join("local/evil");
        fs::create_dir_all(plugin_root.join(".cursor-plugin")).unwrap();
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name": "evil"}"#,
        )
        .unwrap();
        let skills_dir = plugin_root.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        std::os::unix::fs::symlink(&outside, skills_dir.join("x")).unwrap();

        let found = enumerate_plugin_skills(&tmp.path().join("local"), "Cursor", None).skills;
        assert!(found.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_plugin_dir_is_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside-plugin");
        fs::create_dir_all(outside.join(".cursor-plugin")).unwrap();
        fs::write(
            outside.join(".cursor-plugin/plugin.json"),
            r#"{"name": "outside-plugin"}"#,
        )
        .unwrap();
        let skill_dir = outside.join("skills/do-things");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: do-things\n---\n").unwrap();

        let cache_dir = tmp.path().join("cache");
        fs::create_dir_all(&cache_dir).unwrap();
        std::os::unix::fs::symlink(&outside, cache_dir.join("linked-plugin")).unwrap();

        let found = enumerate_plugin_skills(&cache_dir, "Cursor", None);
        assert!(found.skills.is_empty());
    }

    #[test]
    fn no_manifest_yields_no_plugin_root() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("just-a-dir/skills/notes");
        fs::create_dir_all(&skill_dir).unwrap();

        assert!(matches!(
            lookup(tmp.path(), &skill_dir, "Claude Code", &mut Vec::new()),
            PluginEvidence::Absent
        ));
    }
    #[test]
    fn absent_cache_is_silent_but_non_directory_cache_reports_an_issue() {
        let tmp = tempfile::tempdir().unwrap();
        let absent = enumerate_plugin_skills(&tmp.path().join("absent"), "Claude Code", None);
        assert!(absent.skills.is_empty());
        assert!(absent.read_issues.is_empty());
        let root = tmp.path().join("cache");
        fs::write(&root, "not a directory").unwrap();
        let report = enumerate_plugin_skills(&root, "Claude Code", None);
        assert!(report.skills.is_empty());
        assert_eq!(report.read_issues.len(), 1);
    }

    #[test]
    fn malformed_manifest_keeps_readable_skill_and_reports_issue() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache/plugin/1");
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        fs::write(root.join(".claude-plugin/plugin.json"), "{").unwrap();
        let skill = root.join("skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: alpha\n---\n").unwrap();
        let report = enumerate_plugin_skills(&tmp.path().join("cache"), "Claude Code", None);
        assert_eq!(report.skills.len(), 1);
        assert_eq!(report.skills[0].plugin.name, "1");
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest));
    }

    #[test]
    fn named_scan_does_not_read_unrequested_skill_document() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache/plugin/1");
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name":"plugin"}"#,
        )
        .unwrap();
        for name in ["wanted", "other"] {
            let skill = root.join("skills").join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        }
        let bad_document = root.join("skills/other/SKILL.md");
        fs::remove_file(&bad_document).unwrap();
        fs::create_dir(&bad_document).unwrap();
        let names = ["wanted".to_string()].into_iter().collect();
        let report =
            enumerate_plugin_skills(&tmp.path().join("cache"), "Claude Code", Some(&names));
        assert_eq!(report.skills.len(), 1);
        assert_eq!(report.skills[0].skill_dir.file_name().unwrap(), "wanted");
        assert!(report.read_issues.is_empty());
        let full = enumerate_plugin_skills(&tmp.path().join("cache"), "Claude Code", None);
        assert!(full
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::SkillDocument
                && issue.path == bad_document.to_string_lossy()));
    }

    #[test]
    fn wrong_type_manifest_reports_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugin");
        let manifest = root.join("plugin.json");
        fs::create_dir_all(&manifest).unwrap();
        let mut issues = Vec::new();
        assert!(matches!(
            lookup(tmp.path(), &root, "Codex", &mut issues),
            PluginEvidence::Unknown
        ));
        assert!(issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest
                && issue.path == manifest.to_string_lossy()));
    }

    #[test]
    fn oversized_manifest_read_is_bounded_and_keeps_fallback_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugin");
        fs::create_dir(&root).unwrap();
        let manifest = root.join("plugin.json");
        fs::write(&manifest, " ".repeat(MAX_MANIFEST_BYTES + 1)).unwrap();
        let scope = SkillReadScope::bind(&[tmp.path().to_path_buf()]).unwrap();
        assert!(scope
            .read(&manifest, MAX_MANIFEST_BYTES)
            .unwrap_err()
            .to_string()
            .contains("limit"));
        let mut issues = Vec::new();
        assert!(matches!(
            lookup(tmp.path(), &root, "Codex", &mut issues),
            PluginEvidence::Unknown
        ));
        assert!(issues.iter().any(
            |issue| issue.path == manifest.to_string_lossy() && issue.message.contains("limit")
        ));
    }

    #[test]
    fn generic_lookup_reports_bad_manifest_and_uses_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugin");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("plugin.json"), "not json").unwrap();
        let skill = root.join("skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        let mut issues = Vec::new();
        let info = match lookup(tmp.path(), &skill, "Codex", &mut issues) {
            PluginEvidence::Confirmed(info) => info,
            other => panic!("unexpected evidence: {other:?}"),
        };
        assert_eq!(info.name, "plugin");
        assert_eq!(issues.len(), 1);
    }
    #[test]
    #[cfg(unix)]
    fn dangling_manifest_link_is_unknown_and_readable_link_retains_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let root = cache.join("plugin/1");
        let manifest_dir = root.join(".claude-plugin");
        fs::create_dir_all(&manifest_dir).unwrap();
        let manifest = manifest_dir.join("plugin.json");
        std::os::unix::fs::symlink("actual.json", &manifest).unwrap();
        let skill = root.join("skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: alpha\n---\n").unwrap();

        let failed = enumerate_plugin_skills(&cache, "Claude Code", None);
        assert!(failed.skills.is_empty());
        assert_eq!(failed.read_issues.len(), 1);
        assert_eq!(
            failed.read_issues[0].kind,
            DiscoveryReadIssueKind::PluginManifest
        );
        assert_eq!(failed.read_issues[0].path, manifest.to_string_lossy());

        fs::write(
            manifest_dir.join("actual.json"),
            r#"{"name":"linked-plugin"}"#,
        )
        .unwrap();
        let readable = enumerate_plugin_skills(&cache, "Claude Code", None);
        assert_eq!(readable.skills.len(), 1);
        assert_eq!(readable.skills[0].plugin.name, "linked-plugin");
        assert!(readable.read_issues.is_empty());
    }

    #[test]
    fn outer_complete_home_boundary_wins_over_nested_truncated_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("project");
        let skill = project.join(".claude/skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(home.join("plugin.json"), r#"{"name":"home-plugin"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(home, vec![project], Vec::new(), Vec::new());

        let evidence = find_plugin_root(&context, &skill, "Claude Code", &mut Vec::new());
        assert!(matches!(
            evidence,
            PluginEvidence::Confirmed(PluginInfo { ref name, .. }) if name == "home-plugin"
        ));
    }

    #[test]
    fn external_project_edge_is_unknown_until_exact_ownership_root_is_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let skill = project.join(".codex/skills/alpha");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&skill).unwrap();
        let truncated = SkillDiscoveryReadContext::bind(
            home.clone(),
            vec![project.clone()],
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            find_plugin_root(&truncated, &skill, "Codex", &mut Vec::new()),
            PluginEvidence::Unknown
        ));

        let complete =
            SkillDiscoveryReadContext::bind(home, vec![project.clone()], Vec::new(), vec![project]);
        assert!(matches!(
            find_plugin_root(&complete, &skill, "Codex", &mut Vec::new()),
            PluginEvidence::Absent
        ));
    }

    #[test]
    #[cfg(unix)]
    fn resolved_plugin_precedes_lexical_plugin_and_lexical_marker_is_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let resolved_root = home.join("resolved");
        let resolved_skill = resolved_root.join("skills/alpha");
        fs::create_dir_all(&resolved_skill).unwrap();
        fs::write(
            resolved_root.join("plugin.json"),
            r#"{"name":"resolved-plugin"}"#,
        )
        .unwrap();
        let lexical_root = home.join("lexical");
        fs::create_dir_all(lexical_root.join("skills")).unwrap();
        fs::write(
            lexical_root.join("plugin.json"),
            r#"{"name":"lexical-plugin"}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&resolved_skill, lexical_root.join("skills/alpha")).unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), Vec::new(), Vec::new());
        let evidence = find_plugin_root(
            &context,
            &lexical_root.join("skills/alpha"),
            "Codex",
            &mut Vec::new(),
        );
        assert!(matches!(
            evidence,
            PluginEvidence::Confirmed(PluginInfo { ref name, .. }) if name == "resolved-plugin"
        ));

        fs::remove_file(resolved_root.join("plugin.json")).unwrap();
        let lexical = find_plugin_root(
            &context,
            &lexical_root.join("skills/alpha"),
            "Codex",
            &mut Vec::new(),
        );
        assert!(matches!(
            lexical,
            PluginEvidence::Confirmed(PluginInfo { ref name, .. }) if name == "lexical-plugin"
        ));
    }

    #[test]
    #[cfg(unix)]
    fn declared_cross_root_link_is_read_and_undeclared_target_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backing = tmp.path().join("backing");
        let outside = tmp.path().join("outside");
        for root in [&home, &backing, &outside] {
            fs::create_dir_all(root.join("skills/alpha")).unwrap();
        }
        fs::write(backing.join("plugin.json"), r#"{"name":"declared-plugin"}"#).unwrap();
        fs::write(outside.join("plugin.json"), r#"{"name":"outside-plugin"}"#).unwrap();
        std::os::unix::fs::symlink(backing.join("skills/alpha"), home.join("declared")).unwrap();
        std::os::unix::fs::symlink(outside.join("skills/alpha"), home.join("outside")).unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), vec![backing], Vec::new());
        assert!(matches!(
            find_plugin_root(&context, &home.join("declared"), "Codex", &mut Vec::new()),
            PluginEvidence::Confirmed(PluginInfo { ref name, .. }) if name == "declared-plugin"
        ));
        let mut issues = Vec::new();
        assert!(matches!(
            find_plugin_root(&context, &home.join("outside"), "Codex", &mut issues),
            PluginEvidence::Unknown
        ));
        assert!(!issues.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn cache_root_alias_is_allowed_but_escaped_alias_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backing = tmp.path().join("backing");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(home.join(".claude/plugins")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        write_plugin_skill(&backing, "cache/market/plugin/1", "alpha");
        std::os::unix::fs::symlink(backing.join("cache"), home.join(".claude/plugins/cache"))
            .unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), vec![backing], Vec::new());
        let report = super::scan_plugin_skills(&context, None);
        assert!(report.skills.iter().any(|found| {
            found.plugin.name == "alpha"
                && found
                    .skill_dir
                    .starts_with(home.join(".claude/plugins/cache"))
        }));

        fs::remove_file(home.join(".claude/plugins/cache")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join(".claude/plugins/cache")).unwrap();
        let escaped = super::scan_plugin_skills(&context, None);
        assert!(escaped.skills.is_empty());
        assert!(escaped
            .read_issues
            .iter()
            .any(|issue| issue.path.ends_with(".claude/plugins/cache")));
    }

    #[test]
    fn capped_cache_listing_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        for index in 0..=MAX_DIRECTORY_ENTRIES {
            fs::create_dir(cache.join(format!("entry-{index:04}"))).unwrap();
        }
        let report = enumerate_plugin_skills(&cache, "Claude Code", None);
        assert!(report.skills.is_empty());
        assert!(report.read_issues.iter().any(|issue| {
            issue.kind == DiscoveryReadIssueKind::Root && issue.message.contains("entry limit")
        }));
    }

    #[test]
    fn cache_coverage_records_absence_and_named_scans_keep_every_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let names = BTreeSet::from(["missing".to_string()]);
        let report = super::scan_plugin_skills(&context, Some(&names));
        assert_eq!(report.coverage.len(), PLUGIN_CACHE_ROOTS.len());
        assert!(report.coverage.iter().all(|coverage| {
            coverage.extent == DiscoveryExtent::Named
                && coverage.membership == SourceReadOutcome::Absent
        }));
    }

    #[test]
    fn empty_cache_is_read_while_a_missing_cache_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let empty_report = super::enumerate_plugin_skills(&context, &empty, "Fixture", None);
        let absent_report =
            super::enumerate_plugin_skills(&context, &tmp.path().join("missing"), "Fixture", None);
        assert_eq!(empty_report.coverage[0].membership, SourceReadOutcome::Read);
        assert_eq!(
            absent_report.coverage[0].membership,
            SourceReadOutcome::Absent
        );
    }

    #[test]
    fn manifest_bearing_siblings_consume_the_total_work_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        for i in 0..20 {
            let plugin = cache.join(format!("plugin-{i}"));
            fs::create_dir_all(plugin.join("skills/alpha")).unwrap();
            fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
            fs::write(plugin.join("skills/alpha/SKILL.md"), "skill").unwrap();
        }
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let limits = PluginScanLimits {
            max_depth: 3,
            directory_entries: 100,
            max_work: 10,
        };
        let mut state = PluginScanState::read();
        state.work_remaining = limits.max_work;
        let mut issues = Vec::new();
        let roots = find_plugin_roots(&context, &cache, "Fixture", limits, &mut issues, &mut state);
        assert_eq!(state.work_remaining, 0);
        assert_eq!(roots.len(), 9);
        assert!(state
            .read_directory(
                &context.scope,
                &roots[0].root.join("skills"),
                100,
                &mut issues
            )
            .is_none());
        assert_eq!(state.membership, SourceReadOutcome::Incomplete);
        assert!(issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Cap));
    }

    #[test]
    fn total_directory_budget_keeps_readable_siblings_and_marks_coverage_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        for (plugin, skill) in [("a", "alpha"), ("b", "beta")] {
            let root = cache.join(plugin).join("release");
            fs::create_dir_all(root.join("skills").join(skill)).unwrap();
            fs::write(
                root.join("plugin.json"),
                format!(r#"{{"name":"{plugin}"}}"#),
            )
            .unwrap();
            fs::write(
                root.join("skills").join(skill).join("SKILL.md"),
                "---\nname: x\n---\n",
            )
            .unwrap();
        }
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let report = super::enumerate_plugin_skills_with_limits(
            &context,
            &cache,
            "Fixture",
            None,
            PluginScanLimits {
                max_depth: 3,
                directory_entries: 8,
                max_work: 9,
            },
            DiscoveryExtent::Full,
        );
        assert_eq!(report.skills.len(), 1);
        assert_eq!(report.coverage[0].membership, SourceReadOutcome::Incomplete);
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Cap));
    }

    #[test]
    fn failed_plugin_document_omits_the_candidate_and_marks_membership_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache/plugin");
        fs::create_dir_all(root.join("skills/alpha/SKILL.md")).unwrap();
        fs::write(root.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let report =
            super::enumerate_plugin_skills(&context, &tmp.path().join("cache"), "Fixture", None);
        assert!(report.skills.is_empty());
        assert_eq!(report.coverage[0].membership, SourceReadOutcome::Incomplete);
    }

    #[test]
    fn named_deletion_is_authoritative_but_a_failed_cache_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let root = cache.join("plugin");
        fs::create_dir_all(root.join("skills/deleted")).unwrap();
        fs::write(root.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
        fs::write(
            root.join("skills/deleted/SKILL.md"),
            "---\nname: deleted\n---\n",
        )
        .unwrap();
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        fs::remove_dir_all(root.join("skills/deleted")).unwrap();
        let names = BTreeSet::from(["deleted".to_string()]);
        let deleted = super::enumerate_plugin_skills(&context, &cache, "Fixture", Some(&names));
        assert!(deleted.skills.is_empty());
        assert_eq!(deleted.coverage[0].membership, SourceReadOutcome::Read);
        fs::remove_dir_all(&cache).unwrap();
        fs::write(&cache, "wrong type").unwrap();
        let failed = super::enumerate_plugin_skills(&context, &cache, "Fixture", Some(&names));
        assert_eq!(failed.coverage[0].membership, SourceReadOutcome::Failed);
    }

    #[test]
    fn malformed_manifest_and_depth_limit_keep_membership_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let deep = cache.join("market/plugin/version");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("plugin.json"), "not json").unwrap();
        let context = SkillDiscoveryReadContext::bind(
            tmp.path().to_path_buf(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let malformed = super::enumerate_plugin_skills_with_limits(
            &context,
            &cache,
            "Fixture",
            None,
            PluginScanLimits {
                max_depth: 3,
                directory_entries: 8,
                max_work: 8,
            },
            DiscoveryExtent::Full,
        );
        assert_eq!(
            malformed.coverage[0].membership,
            SourceReadOutcome::Incomplete
        );

        let limited = super::enumerate_plugin_skills_with_limits(
            &context,
            &cache,
            "Fixture",
            None,
            PluginScanLimits {
                max_depth: 0,
                directory_entries: 8,
                max_work: 8,
            },
            DiscoveryExtent::Full,
        );
        assert_eq!(
            limited.coverage[0].membership,
            SourceReadOutcome::Incomplete
        );
        assert!(limited
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Cap));
    }

    #[test]
    #[cfg(unix)]
    fn repeated_physical_aliases_keep_requested_outcomes_without_expanding_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let alias = tmp.path().join("project-alias");
        let skill = project.join("skills/alpha");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&skill).unwrap();
        fs::write(project.join("plugin.json"), r#"{"name":"project-plugin"}"#).unwrap();
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            home,
            vec![project.clone(), project.clone()],
            vec![alias],
            Vec::new(),
        );

        assert_eq!(context.bind_outcomes().len(), 4);
        assert!(context
            .bind_outcomes()
            .iter()
            .all(|outcome| matches!(outcome, RootBindOutcome::Bound { .. })));
        assert!(matches!(
            find_plugin_root(&context, &skill, "Codex", &mut Vec::new()),
            PluginEvidence::Confirmed(PluginInfo { ref name, .. }) if name == "project-plugin"
        ));
        assert!(matches!(
            find_plugin_root(
                &context,
                &tmp.path().join("outside"),
                "Codex",
                &mut Vec::new()
            ),
            PluginEvidence::Unknown
        ));
    }
}
