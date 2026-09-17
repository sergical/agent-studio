use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::skill_assembly::assemble_installed_skills;
use crate::skill_coordination::{
    CoordinatedReadGuard, CoordinationMode, CoordinationPlan, DirectoryEffect,
};
use crate::skill_discovery::{PreparedDiscovery, SkillFactsCache};
use crate::skill_inventory::InstalledSkill;
use crate::skill_ownership::{OwnershipInput, OwnershipReadReport, PreparedOwnershipRead};
use crate::skill_plugins::SkillDiscoveryReadContext;
use crate::skill_read::{
    DiscoveryExtent, DiscoveryReadIssue, MembershipSource, SourceCoverage, SourceReadOutcome,
};

pub use crate::skill_coordination::{CancellationToken, CoordinationFailure, PreparedContentError};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillScope {
    pub home: PathBuf,
    pub projects: Vec<PathBuf>,
    pub backing_roots: Vec<PathBuf>,
    pub plugin_ownership_roots: Vec<PathBuf>,
}

pub const MAX_REPAIR_DOCUMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub enum WritePreparationError {
    Scan(ScanError),
    IncompleteInventory,
    InvalidRepairSelection(String),
}

impl WritePreparationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Scan(error) => error.code(),
            Self::IncompleteInventory => "incomplete_write_inventory",
            Self::InvalidRepairSelection(_) => "invalid_repair_selection",
        }
    }
}
impl std::fmt::Display for WritePreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scan(error) => error.fmt(formatter),
            Self::InvalidRepairSelection(message) => formatter.write_str(message),
            Self::IncompleteInventory => formatter.write_str(
                "Write preparation requires complete discovery membership and ownership evidence",
            ),
        }
    }
}
impl std::error::Error for WritePreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scan(error) => Some(error),
            Self::IncompleteInventory | Self::InvalidRepairSelection(_) => None,
        }
    }
}
impl From<ScanError> for WritePreparationError {
    fn from(error: ScanError) -> Self {
        Self::Scan(error)
    }
}
impl From<CoordinationFailure> for WritePreparationError {
    fn from(error: CoordinationFailure) -> Self {
        Self::Scan(error.into())
    }
}

impl SkillScope {
    fn from_context(context: &SkillDiscoveryReadContext) -> Self {
        Self {
            home: context.home().to_path_buf(),
            projects: context.projects().to_vec(),
            backing_roots: context.backing_roots().to_vec(),
            plugin_ownership_roots: context.plugin_ownership_roots().to_vec(),
        }
    }
}

#[derive(Debug)]
pub enum ScanError {
    InvalidScope { path: PathBuf },
    InvalidNames,
    Coordination(CoordinationFailure),
}

impl ScanError {
    pub fn code(&self) -> &'static str {
        match self {
            ScanError::InvalidScope { .. } => "invalid_scope",
            ScanError::InvalidNames => "invalid_names",
            ScanError::Coordination(failure) => coordination_error_code(failure),
        }
    }
}

fn coordination_error_code(failure: &CoordinationFailure) -> &'static str {
    match failure {
        CoordinationFailure::Busy => "scope_busy",
        CoordinationFailure::DeadlineExceeded => "scope_deadline_exceeded",
        CoordinationFailure::Cancelled => "cancelled",
        CoordinationFailure::InvalidTimeout => "invalid_timeout",
        CoordinationFailure::FilesystemRootRequired => "filesystem_root_required",
        CoordinationFailure::Unavailable { .. } => "scope_unavailable",
        CoordinationFailure::Changed => "scope_changed",
        CoordinationFailure::CapacityExceeded { .. } => "coordination_capacity",
        CoordinationFailure::ExclusiveEffectsRequired => "coordination_exclusive_required",
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScope { path } => write!(
                formatter,
                "skill scope requires an absolute directory below the filesystem root: {}",
                path.display()
            ),
            Self::InvalidNames => write!(
                formatter,
                "named scans require a nonempty set of plain directory names"
            ),
            Self::Coordination(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Coordination(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoordinationFailure> for ScanError {
    fn from(error: CoordinationFailure) -> Self {
        Self::Coordination(error)
    }
}

pub struct ScopedSkillService {
    context: SkillDiscoveryReadContext,
    cache: SkillFactsCache,
}

impl ScopedSkillService {
    /// Bind only explicit roots. Missing roots produce partial coverage without
    /// granting read access to their parents. No projects are discovered or files changed.
    pub fn bind(mut scope: SkillScope) -> Result<Self, ScanError> {
        for path in std::iter::once(&scope.home)
            .chain(&scope.projects)
            .chain(&scope.backing_roots)
            .chain(&scope.plugin_ownership_roots)
        {
            if !path.is_absolute() || path.parent().is_none() {
                return Err(ScanError::InvalidScope { path: path.clone() });
            }
        }
        for roots in [
            &mut scope.projects,
            &mut scope.backing_roots,
            &mut scope.plugin_ownership_roots,
        ] {
            roots.sort();
            roots.dedup();
        }
        let context = SkillDiscoveryReadContext::bind(
            scope.home,
            scope.projects,
            scope.backing_roots,
            scope.plugin_ownership_roots,
        );
        Ok(Self {
            context,
            cache: SkillFactsCache::default(),
        })
    }

    pub fn scope(&self) -> SkillScope {
        SkillScope::from_context(&self.context)
    }

    #[cfg(test)]
    pub(crate) fn new(context: SkillDiscoveryReadContext) -> Self {
        Self {
            context,
            cache: SkillFactsCache::default(),
        }
    }

    /// None selects a full scan. A missing timeout uses the finite default acquisition budget.
    pub fn scan(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
    ) -> Result<InventoryRead, ScanError> {
        self.scan_controlled(names, timeout, None)
    }

    /// Cancellation is cooperative. No inventory is returned after a checkpoint
    /// observes cancellation; this does not interrupt an in-flight filesystem syscall.
    pub fn scan_cancellable(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<InventoryRead, ScanError> {
        self.scan_controlled(names, timeout, Some(cancellation))
    }

    fn scan_controlled(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
        cancellation: Option<CancellationToken>,
    ) -> Result<InventoryRead, ScanError> {
        let span = tracing::info_span!(
            "skill.scan",
            extent = if names.is_some() { "named" } else { "full" },
            selected_count = names.map_or(0, BTreeSet::len),
            project_count = self.context.projects().len(),
            outcome = "running",
            error_code = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
            skill_count = tracing::field::Empty,
            ledger_only_count = tracing::field::Empty,
            discovery_issue_count = tracing::field::Empty,
        );
        let _entered = span.enter();
        let started = Instant::now();
        let result = self.scan_read(names, timeout, cancellation);
        let (outcome, error_code) = match &result {
            Ok(inventory) => {
                span.record("skill_count", inventory.skills.len());
                span.record("ledger_only_count", inventory.ledger_only.len());
                span.record("discovery_issue_count", inventory.discovery_issues.len());
                (
                    match inventory.completeness {
                        InventoryCompleteness::Complete => "complete",
                        InventoryCompleteness::Partial => "partial",
                    },
                    "none",
                )
            }
            Err(ScanError::Coordination(CoordinationFailure::Cancelled)) => {
                ("cancelled", "cancelled")
            }
            Err(error) => ("failed", error.code()),
        };
        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        span.record("outcome", outcome);
        span.record("error_code", error_code);
        span.record("duration_ms", duration_ms);
        tracing::info!(outcome, error_code, duration_ms, "skill.scan.finished");
        result
    }

    fn scan_read(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
        cancellation: Option<CancellationToken>,
    ) -> Result<InventoryRead, ScanError> {
        self.scan_read_guarded(names, timeout, cancellation)
            .map(|(inventory, _guard)| inventory)
    }

    fn scan_read_guarded(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
        cancellation: Option<CancellationToken>,
    ) -> Result<(InventoryRead, CoordinatedReadGuard), ScanError> {
        self.scan_read_guarded_mode(
            names,
            timeout,
            cancellation,
            CoordinationMode::Shared,
            &[],
            &[],
        )
        .map(|read| (read.inventory, read.guard))
    }

    /// Verified membership and ownership with the same frozen exclusive lease.
    /// Callers validate required operation facts and derive every effect before intent.
    /// Additional trees are trusted, authorized state roots.
    pub fn prepare_write_inventory(
        &mut self,
        names: Option<&BTreeSet<String>>,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<
        (
            InventoryRead,
            crate::skill_coordination::FinalizedWriteLease<'_>,
        ),
        WritePreparationError,
    > {
        self.prepare_write_inventory_with_entries(
            names,
            additional_trees,
            &[],
            timeout,
            cancellation,
        )
    }

    /// Additional entries are trusted, authorized mutation targets. They add
    /// coordination effects without expanding the service's readable roots.
    pub fn prepare_write_inventory_with_entries(
        &mut self,
        names: Option<&BTreeSet<String>>,
        additional_trees: &[PathBuf],
        additional_entries: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<
        (
            InventoryRead,
            crate::skill_coordination::FinalizedWriteLease<'_>,
        ),
        WritePreparationError,
    > {
        let read = self.scan_read_guarded_mode(
            names,
            timeout,
            Some(cancellation),
            CoordinationMode::Exclusive,
            additional_trees,
            additional_entries,
        )?;
        if !inventory_read_blockers(
            &read.inventory.scope,
            read.inventory.extent,
            &read.inventory.source_coverage,
            &read.inventory.ownership,
        )
        .is_empty()
        {
            return Err(WritePreparationError::IncompleteInventory);
        }
        let lease = read
            .guard
            .finalize_write(self.context.read_scope())?
            .retain_ownership(read.ownership)?
            .retain_membership(read.discovery.into_membership())?;
        Ok((read.inventory, lease))
    }

    fn scan_read_guarded_mode(
        &mut self,
        names: Option<&BTreeSet<String>>,
        timeout: Option<Duration>,
        cancellation: Option<CancellationToken>,
        mode: CoordinationMode,
        additional_trees: &[PathBuf],
        additional_entries: &[PathBuf],
    ) -> Result<GuardedInventoryRead, ScanError> {
        if names.is_some_and(|names| {
            names.is_empty()
                || names.iter().any(|name| {
                    name.is_empty()
                        || name == "."
                        || name == ".."
                        || name.contains('/')
                        || name.contains('\\')
                        || name.contains('\0')
                })
        }) {
            return Err(ScanError::InvalidNames);
        }
        let mut effects: Vec<_> = std::iter::once(self.context.home().to_path_buf())
            .chain(self.context.projects().iter().cloned())
            .chain(self.context.backing_roots().iter().cloned())
            .chain(self.context.plugin_ownership_roots().iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|path| {
                if self.context.bind_outcomes().iter().any(|outcome| matches!(outcome,
                    crate::skill_scope::RootBindOutcome::Missing { requested, .. } if requested == &path)) {
                    DirectoryEffect::entry(path, mode)
                } else {
                    DirectoryEffect::tree(path, mode)
                }
            })
            .collect();
        effects.extend(
            additional_trees
                .iter()
                .cloned()
                .map(|path| DirectoryEffect::tree(path, mode)),
        );
        effects.extend(
            additional_entries
                .iter()
                .cloned()
                .map(|path| DirectoryEffect::entry(path, mode)),
        );
        let guard = scan_phase("initial_coordination", || {
            let plan = match cancellation {
                Some(token) => CoordinationPlan::new_cancellable(effects, timeout, token),
                None => CoordinationPlan::new(effects, timeout),
            }?;
            let guard =
                plan.acquire()?
                    .continue_with_files(self.context.read_scope(), &[], mode)?;
            Ok(guard)
        })?;
        read_inventory_guarded(&self.context, &mut self.cache, names, guard)
            .map_err(ScanError::from)
    }

    pub fn last_pass_stats(&self) -> (u64, u64) {
        self.cache.last_pass_stats()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InventoryCompleteness {
    Complete,
    Partial,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "evidence", rename_all = "kebab-case")]
pub enum ReplacementBlocker {
    MissingDiscoverySource {
        path: PathBuf,
        source: MembershipSource,
    },
    DiscoverySource(SourceCoverage),
    MissingOwnershipScope(PathBuf),
    Ownership(crate::skill_ownership::OwnershipReadIssue),
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReplacementSafety {
    NotApplicable,
    Safe {
        names: BTreeSet<String>,
    },
    Preserve {
        names: BTreeSet<String>,
        causes: Vec<ReplacementBlocker>,
    },
}

fn inventory_status(
    context: &SkillDiscoveryReadContext,
    names: Option<&BTreeSet<String>>,
    coverage: &[SourceCoverage],
    issues: &[DiscoveryReadIssue],
    ownership: &OwnershipReadReport,
) -> (InventoryCompleteness, ReplacementSafety) {
    let extent = if names.is_some() {
        DiscoveryExtent::Named
    } else {
        DiscoveryExtent::Full
    };
    let causes = inventory_read_blockers(
        &SkillScope::from_context(context),
        extent,
        coverage,
        ownership,
    );
    let complete = causes.is_empty()
        && issues.is_empty()
        && coverage.iter().all(|item| {
            matches!(
                item.facts,
                SourceReadOutcome::Read | SourceReadOutcome::Absent
            )
        });
    let completeness = if complete {
        InventoryCompleteness::Complete
    } else {
        InventoryCompleteness::Partial
    };
    let safety = match names {
        None => ReplacementSafety::NotApplicable,
        Some(names) if causes.is_empty() => ReplacementSafety::Safe {
            names: names.clone(),
        },
        Some(names) => ReplacementSafety::Preserve {
            names: names.clone(),
            causes,
        },
    };
    (completeness, safety)
}

pub(crate) fn inventory_read_blockers(
    scope: &SkillScope,
    extent: DiscoveryExtent,
    coverage: &[SourceCoverage],
    ownership: &OwnershipReadReport,
) -> Vec<ReplacementBlocker> {
    let mut causes = Vec::new();
    let expected = crate::skill_agents::skill_roots(&scope.home, &scope.projects)
        .into_iter()
        .flat_map(|root| {
            [
                (root.path.clone(), MembershipSource::AgentRoot),
                (
                    root.path
                        .join(crate::skill_discovery::STUDIO_DISABLED_DIR_NAME),
                    MembershipSource::DisabledRoot,
                ),
            ]
        })
        .chain(
            crate::skill_plugins::PLUGIN_CACHE_ROOTS
                .iter()
                .map(|(path, _)| (scope.home.join(path), MembershipSource::PluginCache)),
        );
    for (path, source) in expected {
        if !coverage
            .iter()
            .any(|item| item.path == path && item.source == source)
        {
            causes.push(ReplacementBlocker::MissingDiscoverySource { path, source });
        }
    }
    for item in coverage {
        if item.extent != extent
            || !matches!(
                item.membership,
                SourceReadOutcome::Read | SourceReadOutcome::Absent
            )
        {
            causes.push(ReplacementBlocker::DiscoverySource(item.clone()));
        }
    }
    let expected_owners = std::iter::once((
        scope.home.as_path(),
        crate::skill_deployment::InstallScope::Global,
    ))
    .chain(scope.projects.iter().map(|project| {
        (
            project.as_path(),
            crate::skill_deployment::InstallScope::Project,
        )
    }));
    for (root, scope) in expected_owners {
        if !ownership.scopes.iter().any(|item| {
            item.agents_dir == root.join(".agents")
                && item.scope == scope
                && (scope == crate::skill_deployment::InstallScope::Global
                    || item.project_skills_sh.is_some())
                && item.project_path.as_deref()
                    == if scope == crate::skill_deployment::InstallScope::Global {
                        None
                    } else {
                        Some(root)
                    }
        }) {
            causes.push(ReplacementBlocker::MissingOwnershipScope(
                root.to_path_buf(),
            ));
        }
    }
    causes.extend(
        ownership
            .failures()
            .into_iter()
            .map(ReplacementBlocker::Ownership),
    );
    causes
}

#[derive(Debug)]
pub struct InventoryRead {
    pub scope: SkillScope,
    pub ledger_only: Vec<crate::skill_ledger_inventory::LedgerOnlySkill>,
    pub completeness: InventoryCompleteness,
    pub replacement_safety: ReplacementSafety,
    pub skills: Vec<InstalledSkill>,
    pub discovery_issues: Vec<DiscoveryReadIssue>,
    pub source_coverage: Vec<SourceCoverage>,
    pub extent: DiscoveryExtent,
    pub ownership: OwnershipReadReport,
}

#[cfg(test)]
pub(crate) fn read_inventory(
    context: &SkillDiscoveryReadContext,
    cache: &mut SkillFactsCache,
    names: Option<&BTreeSet<String>>,
    guard: CoordinatedReadGuard,
) -> Result<InventoryRead, CoordinationFailure> {
    read_inventory_guarded(context, cache, names, guard).map(|read| read.inventory)
}

struct GuardedInventoryRead {
    inventory: InventoryRead,
    guard: CoordinatedReadGuard,
    ownership: PreparedOwnershipRead,
    discovery: crate::skill_discovery::DiscoveryReadProof,
}

fn read_inventory_guarded(
    context: &SkillDiscoveryReadContext,
    cache: &mut SkillFactsCache,
    names: Option<&BTreeSet<String>>,
    guard: CoordinatedReadGuard,
) -> Result<GuardedInventoryRead, CoordinationFailure> {
    guard.check_cancelled()?;
    let scope = context.read_scope();
    let ownership_plan = scan_phase("ownership_enumeration", || {
        Ok(PreparedOwnershipRead::enumerate(
            scope,
            context.home(),
            context.projects(),
        ))
    })?;
    let guard = scan_phase("ownership_coordination", || {
        guard.extend_with_files(scope, &ownership_plan.regular_files())
    })?;
    let (discovery_plan, guard) = scan_phase("discovery_enumeration", || {
        PreparedDiscovery::enumerate_coordinated(context, names, guard)
    })?;
    guard.check_cancelled()?;
    let (inventory, discovery) = with_cache_pass(cache, |cache| {
        guard.check_cancelled()?;
        let (discovery, discovery_proof) = scan_phase("discovery_materialization", || {
            discovery_plan.materialize_with_proof_checked(cache, Some(&guard), &mut || {
                guard.check_cancelled()
            })
        })?;
        guard.check_cancelled()?;
        let ownership = scan_phase("ownership_materialization", || {
            Ok(ownership_plan.materialize(scope, Some(&guard)))
        })?;
        guard.check_cancelled()?;
        let skills = scan_phase("assembly", || {
            let mut lock = ownership.global_lock();
            if let Some(names) = names {
                lock.skills.retain(|name, _| names.contains(name));
            }
            let mut skills = assemble_installed_skills(
                discovery.candidates,
                &lock,
                &ownership,
                &ownership.copy_records(),
            );
            guard.check_cancelled()?;
            skills.retain(|skill| !skill.deployments.is_empty());
            if let OwnershipInput::Loaded(registry) = &ownership.registry {
                crate::skill_registry_projection::apply_registry_facts(
                    context.home(),
                    &mut skills,
                    registry,
                );
            }
            guard.check_cancelled()?;
            Ok(skills)
        })?;
        scan_phase("final_validation", || {
            discovery_proof.revalidate(scope)?;
            ownership_plan
                .revalidate(scope)
                .map_err(|_| CoordinationFailure::Changed)?;
            guard.revalidate(scope)?;
            context
                .revalidate_roots()
                .map_err(|_| CoordinationFailure::Changed)?;
            for outcome in context.bind_outcomes() {
                if let crate::skill_scope::RootBindOutcome::Missing { requested, .. } = outcome {
                    match crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(requested))
                    {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        _ => return Err(CoordinationFailure::Changed),
                    }
                }
            }
            Ok(())
        })?;
        if let Some(names) = names {
            cache.end_named_pass(names);
        } else {
            cache.end_pass();
        }
        let (completeness, replacement_safety) = inventory_status(
            context,
            names,
            &discovery.source_coverage,
            &discovery.read_issues,
            &ownership,
        );
        let ledger_only = scan_phase("ledger_collection", || {
            let rows =
                crate::skill_ledger_inventory::ledger_only_skills(&ownership, &skills, names);
            guard.check_cancelled()?;
            Ok(rows)
        })?;
        Ok((
            InventoryRead {
                scope: SkillScope::from_context(context),
                ledger_only,
                completeness,
                replacement_safety,
                skills,
                discovery_issues: discovery.read_issues,
                source_coverage: discovery.source_coverage,
                extent: discovery.extent,
                ownership,
            },
            discovery_proof,
        ))
    })?;
    Ok(GuardedInventoryRead {
        inventory,
        guard,
        ownership: ownership_plan,
        discovery,
    })
}

fn scan_phase<T>(
    phase: &'static str,
    run: impl FnOnce() -> Result<T, CoordinationFailure>,
) -> Result<T, CoordinationFailure> {
    let span = tracing::info_span!(
        "skill.scan.phase",
        phase,
        outcome = "running",
        error_code = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
    );
    let _entered = span.enter();
    let started = Instant::now();
    let result = run();
    let (outcome, error_code) = match &result {
        Ok(_) => ("complete", "none"),
        Err(CoordinationFailure::Cancelled) => ("cancelled", "cancelled"),
        Err(error) => ("failed", coordination_error_code(error)),
    };
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    span.record("outcome", outcome);
    span.record("error_code", error_code);
    span.record("duration_ms", duration_ms);
    tracing::info!(
        phase,
        outcome,
        error_code,
        duration_ms,
        "skill.scan.phase.finished"
    );
    result
}

fn with_cache_pass<T>(
    cache: &mut SkillFactsCache,
    read: impl FnOnce(&mut SkillFactsCache) -> Result<T, CoordinationFailure>,
) -> Result<T, CoordinationFailure> {
    let mut pass = std::mem::take(cache);
    pass.begin_pass();
    let result = read(&mut pass);
    if result.is_ok() {
        *cache = pass;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    use std::fs;
    #[test]
    fn entry_aware_inventory_prepares_move_domains_and_rejects_stale_parent_creation() {
        for parent_exists in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let agents = home.join(".agents");
            let source = agents.join("skills/alpha");
            let holding = agents.join("skills/.skill-studio-disabled");
            let destination = holding.join("alpha");
            fs::create_dir_all(&source).unwrap();
            fs::write(
                source.join("SKILL.md"),
                "---\nname: alpha\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            fs::write(
                agents.join(".skill-lock.json"),
                r#"{"version":3,"skills":{}}"#,
            )
            .unwrap();
            fs::write(agents.join("skill-studio.json"), r#"{"version":4}"#).unwrap();
            if parent_exists {
                fs::create_dir(&holding).unwrap();
            }
            let mut service = ScopedSkillService::bind(SkillScope {
                home,
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            })
            .unwrap();
            let names = BTreeSet::from(["alpha".into()]);
            {
                let (_, lease) = service
                    .prepare_write_inventory(
                        Some(&names),
                        std::slice::from_ref(&source),
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                assert!(lease.validate_tree_move(&source, &destination).is_err());
            }
            {
                let (inventory, lease) = service
                    .prepare_write_inventory_with_entries(
                        Some(&names),
                        std::slice::from_ref(&source),
                        &[source.clone(), destination.clone()],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                assert!(inventory.skills.iter().any(|skill| skill.name == "alpha"));
                lease.validate_tree_move(&source, &destination).unwrap();
                if !parent_exists {
                    fs::create_dir(&holding).unwrap();
                    assert!(lease.revalidate().is_err());
                }
            }
            let (_, fresh) = service
                .prepare_write_inventory_with_entries(
                    Some(&names),
                    std::slice::from_ref(&source),
                    &[source.clone(), destination.clone()],
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            fresh.validate_tree_move(&source, &destination).unwrap();
            assert_eq!(
                fs::read(source.join("SKILL.md")).unwrap(),
                b"---\nname: alpha\ndescription: fixture\n---\nbody\n"
            );
            assert!(!destination.exists());
        }
    }

    #[test]
    fn write_inventory_retains_missing_ownership_inputs_through_execution() {
        for name in ["agents.lock", "agents.toml", "skill-studio.json"] {
            for after_write in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let skill = home.join(".agents/skills/alpha");
                fs::create_dir_all(&skill).unwrap();
                let path = skill.join("SKILL.md");
                fs::write(&path, "---\nname: alpha\ndescription: fixture\n---\nbody\n").unwrap();
                let mut service = ScopedSkillService::bind(SkillScope {
                    home: home.clone(),
                    projects: vec![],
                    backing_roots: vec![],
                    plugin_ownership_roots: vec![],
                })
                .unwrap();
                let (_, mut lease) = service
                    .prepare_write_inventory(
                        Some(&BTreeSet::from(["alpha".into()])),
                        &[],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let before = lease.read(&path, MAX_REPAIR_DOCUMENT_BYTES).unwrap();
                let target =
                    crate::skill_document_target::SkillDocumentTarget::bind(&skill).unwrap();
                if after_write {
                    target.replace(&mut lease, &before, b"replacement").unwrap();
                    lease.revalidate().unwrap();
                }
                fs::write(home.join(".agents").join(name), "{}").unwrap();
                assert!(matches!(
                    lease.revalidate(),
                    Err(CoordinationFailure::Changed)
                ));
                if !after_write {
                    assert!(target.replace(&mut lease, &before, b"replacement").is_err());
                    assert_eq!(fs::read(&path).unwrap(), before);
                } else {
                    assert_eq!(fs::read(&path).unwrap(), b"replacement");
                }
            }
        }
    }

    #[test]
    fn write_inventory_retains_plugin_membership_before_and_after_replacement() {
        for cache in [false, true] {
            for after_write in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let skill = home.join(".agents/skills/alpha");
                fs::create_dir_all(&skill).unwrap();
                let path = skill.join("SKILL.md");
                fs::write(&path, "---\nname: alpha\ndescription: fixture\n---\nbody\n").unwrap();
                let mut service = ScopedSkillService::bind(SkillScope {
                    home: home.clone(),
                    projects: vec![],
                    backing_roots: vec![],
                    plugin_ownership_roots: vec![],
                })
                .unwrap();
                let (_, mut lease) = service
                    .prepare_write_inventory(
                        Some(&BTreeSet::from(["alpha".into()])),
                        &[],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let before = lease.read(&path, MAX_REPAIR_DOCUMENT_BYTES).unwrap();
                let target =
                    crate::skill_document_target::SkillDocumentTarget::bind(&skill).unwrap();
                if after_write {
                    target.replace(&mut lease, &before, b"replacement").unwrap();
                    lease.revalidate().unwrap();
                }
                let manifest = if cache {
                    home.join(".claude/plugins/cache/market/plugin/v1/.claude-plugin/plugin.json")
                } else {
                    skill.join(".claude-plugin/plugin.json")
                };
                fs::create_dir_all(manifest.parent().unwrap()).unwrap();
                fs::write(manifest, r#"{"name":"new-plugin"}"#).unwrap();
                assert!(
                    matches!(lease.revalidate(), Err(CoordinationFailure::Changed)),
                    "cache={cache}, after_write={after_write}"
                );
                if !after_write {
                    assert!(target.replace(&mut lease, &before, b"replacement").is_err());
                    assert_eq!(fs::read(&path).unwrap(), before);
                } else {
                    assert_eq!(fs::read(&path).unwrap(), b"replacement");
                }
            }
        }
    }

    #[test]
    fn write_inventory_retains_agent_root_membership_during_execution() {
        for initially_present in [false, true] {
            for after_write in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let skill = home.join(".agents/skills/alpha");
                let harness_root = home.join(".claude/skills");
                fs::create_dir_all(&skill).unwrap();
                if initially_present {
                    fs::create_dir_all(&harness_root).unwrap();
                }
                let path = skill.join("SKILL.md");
                fs::write(&path, "---\nname: alpha\ndescription: fixture\n---\nbody\n").unwrap();
                let mut service = ScopedSkillService::bind(SkillScope {
                    home: home.clone(),
                    projects: vec![],
                    backing_roots: vec![],
                    plugin_ownership_roots: vec![],
                })
                .unwrap();
                let (_, mut lease) = service
                    .prepare_write_inventory(
                        Some(&BTreeSet::from(["alpha".into()])),
                        &[],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let before = lease.read(&path, MAX_REPAIR_DOCUMENT_BYTES).unwrap();
                let target =
                    crate::skill_document_target::SkillDocumentTarget::bind(&skill).unwrap();
                if after_write {
                    target.replace(&mut lease, &before, b"replacement").unwrap();
                    lease.revalidate().unwrap();
                }
                fs::create_dir_all(harness_root.join("alpha")).unwrap();
                fs::write(harness_root.join("alpha/SKILL.md"), &before).unwrap();
                assert!(
                    matches!(lease.revalidate(), Err(CoordinationFailure::Changed)),
                    "present={initially_present}, after_write={after_write}"
                );
                if !after_write {
                    assert!(target.replace(&mut lease, &before, b"replacement").is_err());
                    assert_eq!(fs::read(&path).unwrap(), before);
                } else {
                    assert_eq!(fs::read(&path).unwrap(), b"replacement");
                }
            }
        }
    }

    #[test]
    fn write_preparation_refuses_partial_evidence_and_releases_its_guard() {
        for missing_project in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let skill = home.join(".agents/skills/alpha");
            fs::create_dir_all(&skill).unwrap();
            let document = skill.join("SKILL.md");
            fs::write(
                &document,
                "---\nname: alpha\ndescription: fixture\n---\nbody\n",
            )
            .unwrap();
            let project = temp.path().join("project");
            let lock = home.join(".agents/.skill-lock.json");
            if !missing_project {
                fs::write(&lock, "invalid ownership data").unwrap();
            }
            let mut service = ScopedSkillService::bind(SkillScope {
                home,
                projects: if missing_project {
                    vec![project.clone()]
                } else {
                    vec![]
                },
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            })
            .unwrap();
            let timeout = Some(Duration::from_secs(10));
            let names = BTreeSet::from(["alpha".into()]);
            let read = service.scan(Some(&names), timeout).unwrap();
            assert_eq!(read.completeness, InventoryCompleteness::Partial);
            assert!(matches!(
                service.prepare_write_inventory(
                    Some(&names),
                    &[],
                    timeout,
                    CancellationToken::default()
                ),
                Err(WritePreparationError::IncompleteInventory)
            ));
            assert_eq!(
                service.scan(Some(&names), timeout).unwrap().completeness,
                InventoryCompleteness::Partial
            );
            if !missing_project {
                fs::remove_file(lock).unwrap();
                let refreshed = service.scan(Some(&names), timeout).unwrap();
                assert!(inventory_read_blockers(
                    &refreshed.scope,
                    refreshed.extent,
                    &refreshed.source_coverage,
                    &refreshed.ownership
                )
                .is_empty());
                let (_, lease) = service
                    .prepare_write_inventory(
                        Some(&names),
                        &[],
                        timeout,
                        CancellationToken::default(),
                    )
                    .unwrap();
                lease.revalidate().unwrap();
            }
            assert!(fs::read_to_string(document)
                .unwrap()
                .contains("description: fixture"));
        }
    }

    #[test]
    fn failed_materialization_discards_cache_and_next_scan_starts_cold() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let skill = home.join(".agents/skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: alpha\ndescription: fixture\n---\nbody\n",
        )
        .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home,
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        for panic in [false, true] {
            service.scan(None, Some(Duration::from_secs(10))).unwrap();
            service.scan(None, Some(Duration::from_secs(10))).unwrap();
            assert_eq!(service.last_pass_stats(), (1, 1));
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_cache_pass(&mut service.cache, |_| -> Result<(), CoordinationFailure> {
                    if panic {
                        std::panic::panic_any("fixture materialization failure");
                    }
                    Err(CoordinationFailure::Cancelled)
                })
            }));
            if panic {
                assert_eq!(
                    outcome.unwrap_err().downcast_ref::<&str>(),
                    Some(&"fixture materialization failure")
                );
            } else {
                assert!(matches!(
                    outcome.unwrap(),
                    Err(CoordinationFailure::Cancelled)
                ));
            }
            assert_eq!(service.last_pass_stats(), (0, 0));
            assert_eq!(
                service
                    .scan(None, Some(Duration::from_secs(10)))
                    .unwrap()
                    .skills
                    .len(),
                1
            );
            assert_eq!(service.last_pass_stats(), (0, 1));
        }
    }

    #[test]
    fn replacement_policy_requires_membership_and_ownership_but_retains_partial_facts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        let skill = project.join(".agents/skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: alpha\ndescription: fixture\n---\nbody\n",
        )
        .unwrap();
        let context = SkillDiscoveryReadContext::bind(home, vec![project.clone()], vec![], vec![]);
        let mut service = ScopedSkillService::new(context);
        let names = BTreeSet::from(["alpha".to_string()]);
        let result = service
            .scan(Some(&names), Some(Duration::from_secs(10)))
            .unwrap();
        assert!(
            matches!(&result.replacement_safety, ReplacementSafety::Safe { names: selected } if selected == &names)
        );
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.completeness, InventoryCompleteness::Partial);
        assert!(!result.discovery_issues.is_empty());
        for index in 0..result.source_coverage.len() {
            let mut coverage = result.source_coverage.clone();
            coverage[index].facts = SourceReadOutcome::Incomplete;
            let (complete, safety) = inventory_status(
                &service.context,
                Some(&names),
                &coverage,
                &[],
                &result.ownership,
            );
            assert_eq!(complete, InventoryCompleteness::Partial);
            assert!(matches!(safety, ReplacementSafety::Safe { .. }));
            for outcome in [SourceReadOutcome::Incomplete, SourceReadOutcome::Failed] {
                coverage[index].membership = outcome;
                let (_, safety) = inventory_status(
                    &service.context,
                    Some(&names),
                    &coverage,
                    &[],
                    &result.ownership,
                );
                assert!(
                    matches!(safety, ReplacementSafety::Preserve { names: selected, causes }
                    if selected == names && causes.iter().any(|cause| matches!(cause, ReplacementBlocker::DiscoverySource(item) if item.path == coverage[index].path)))
                );
            }
            coverage.remove(index);
            let (_, safety) = inventory_status(
                &service.context,
                Some(&names),
                &coverage,
                &[],
                &result.ownership,
            );
            assert!(matches!(safety, ReplacementSafety::Preserve { causes, .. }
                if causes.iter().any(|cause| matches!(cause, ReplacementBlocker::MissingDiscoverySource { path, source } if path == &result.source_coverage[index].path && source == &result.source_coverage[index].source))));
        }
        let mut ownership = result.ownership.clone();
        ownership
            .scopes
            .retain(|scope| scope.project_path.is_none());
        let (_, safety) = inventory_status(
            &service.context,
            Some(&names),
            &result.source_coverage,
            &[],
            &ownership,
        );
        assert!(matches!(safety, ReplacementSafety::Preserve { causes, .. }
            if causes.iter().any(|cause| matches!(cause, ReplacementBlocker::MissingOwnershipScope(path) if path == &project))));
        let mut ownership = result.ownership.clone();
        ownership
            .scopes
            .iter_mut()
            .find(|scope| scope.project_path.as_ref() == Some(&project))
            .unwrap()
            .project_skills_sh = None;
        assert!(matches!(
            inventory_status(
                &service.context,
                Some(&names),
                &result.source_coverage,
                &[],
                &ownership
            )
            .1,
            ReplacementSafety::Preserve { .. }
        ));
        let mut ownership = result.ownership.clone();
        ownership.registry = OwnershipInput::Failed(crate::skill_ownership::OwnershipReadIssue {
            kind: crate::skill_ownership::OwnershipReadIssueKind::LifecycleRegistry,
            path: "registry".to_string(),
            message: "unreadable".to_string(),
        });
        let (_, safety) = inventory_status(
            &service.context,
            Some(&names),
            &result.source_coverage,
            &[],
            &ownership,
        );
        assert!(matches!(safety, ReplacementSafety::Preserve { causes, .. }
            if causes.iter().any(|cause| matches!(cause, ReplacementBlocker::Ownership(issue) if issue.path == "registry"))));
        let (complete, safety) = inventory_status(
            &service.context,
            None,
            &[],
            &[],
            &OwnershipReadReport::empty(),
        );
        assert_eq!(complete, InventoryCompleteness::Partial);
        assert!(matches!(safety, ReplacementSafety::NotApplicable));
        let mut coverage = result.source_coverage.clone();
        coverage[0].extent = DiscoveryExtent::Full;
        assert!(matches!(
            inventory_status(
                &service.context,
                Some(&names),
                &coverage,
                &[],
                &result.ownership
            )
            .1,
            ReplacementSafety::Preserve { .. }
        ));
    }

    #[test]
    fn service_coordinates_each_declared_root_and_releases_after_contention() {
        let temp = tempfile::tempdir().unwrap();
        let roots = ["home", "project", "backing", "plugin"].map(|name| temp.path().join(name));
        for root in &roots {
            fs::create_dir(root).unwrap();
        }
        let context = SkillDiscoveryReadContext::bind(
            roots[0].clone(),
            vec![roots[1].clone()],
            vec![roots[2].clone()],
            vec![roots[3].clone()],
        );
        let mut service = ScopedSkillService::new(context);
        for root in &roots {
            let writer = CoordinationPlan::new(
                vec![DirectoryEffect::tree(root, CoordinationMode::Exclusive)],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap();
            assert!(matches!(
                service.scan(None, Some(Duration::from_millis(20))),
                Err(ScanError::Coordination(CoordinationFailure::Busy))
            ));
            drop(writer);
            assert!(service
                .scan(None, Some(Duration::from_secs(10)))
                .unwrap()
                .skills
                .is_empty());
        }
    }

    #[test]
    fn service_owns_full_named_and_warm_scans_without_a_caller_guard() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let backing = temp.path().join("backing");
        let plugin_root = temp.path().join("plugin-root");
        for root in [&backing, &plugin_root] {
            fs::create_dir_all(root).unwrap();
        }
        for (root, name) in [(&home, "alpha"), (&project, "beta")] {
            let skill = root.join(".agents/skills").join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: fixture\n---\nbody\n"),
            )
            .unwrap();
        }
        let context =
            SkillDiscoveryReadContext::bind(home, vec![project], vec![backing], vec![plugin_root]);
        let mut service = ScopedSkillService::new(context);
        let timeout = Some(Duration::from_secs(10));
        let full = service.scan(None, timeout).unwrap();
        assert_eq!(full.skills.len(), 2);
        assert_eq!(service.last_pass_stats(), (0, 2));
        let full = service.scan(None, timeout).unwrap();
        assert_eq!(full.skills.len(), 2);
        assert_eq!(service.last_pass_stats(), (2, 2));
        let names = BTreeSet::from(["alpha".to_string()]);
        let named = service.scan(Some(&names), timeout).unwrap();
        assert_eq!(named.skills.len(), 1);
        assert_eq!(named.skills[0].name, "alpha");
        assert_eq!(service.last_pass_stats(), (1, 1));
        assert!(matches!(
            service.scan(None, Some(Duration::ZERO)),
            Err(ScanError::Coordination(CoordinationFailure::InvalidTimeout))
        ));
        service.scan(None, timeout).unwrap();
        assert_eq!(service.last_pass_stats(), (2, 2));
    }

    #[test]
    fn rejected_scope_validation_clears_materialized_cache() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let backing = temp.path().join("backing");
        fs::create_dir(&backing).unwrap();
        let skill = home.join(".agents/skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: alpha\ndescription: fixture\n---\nbody\n",
        )
        .unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), vec![], vec![backing.clone()], vec![]);
        let mut service = ScopedSkillService::new(context);
        let timeout = Some(Duration::from_secs(10));
        service.scan(None, timeout).unwrap();
        service.scan(None, timeout).unwrap();
        assert_eq!(service.last_pass_stats(), (1, 1));
        fs::rename(&backing, temp.path().join("old-backing")).unwrap();
        fs::create_dir(&backing).unwrap();
        assert!(matches!(
            service.scan(None, timeout),
            Err(ScanError::Coordination(CoordinationFailure::Changed))
        ));
        assert_eq!(service.last_pass_stats(), (0, 0));
        service.context = SkillDiscoveryReadContext::bind(home, vec![], vec![backing], vec![]);
        service.scan(None, timeout).unwrap();
        assert_eq!(service.last_pass_stats(), (0, 1));
        service.scan(None, timeout).unwrap();
        assert_eq!(service.last_pass_stats(), (1, 1));
    }

    #[test]
    fn inventory_composition_shares_scope_and_preserves_named_selection() {
        for named in [false, true] {
            for invalid_ledger in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                let project = temp.path().join("project");
                for (root, name) in [(&home, "alpha"), (&project, "beta")] {
                    let skill = root.join(".agents/skills").join(name);
                    fs::create_dir_all(&skill).unwrap();
                    fs::write(
                        skill.join("SKILL.md"),
                        format!("---\nname: {name}\ndescription: fixture\n---\nbody\n"),
                    )
                    .unwrap();
                }
                let lock_path = home.join(".agents/.skill-lock.json");
                fs::write(&lock_path, if invalid_ledger { "invalid" } else { r#"{"version":3,"skills":{"ledger-only":{"source":"owner/repo","sourceType":"github","sourceUrl":"https://example.test/owner/repo","skillFolderHash":"hash","installedAt":"2026-01-01","updatedAt":"2026-01-01"}}}"# }).unwrap();
                let context = SkillDiscoveryReadContext::bind(
                    home.clone(),
                    vec![project.clone()],
                    Vec::new(),
                    Vec::new(),
                );
                let names = BTreeSet::from(["alpha".to_string()]);
                let mut cache = SkillFactsCache::default();
                for _ in 0..2 {
                    let scope = context.read_scope();
                    let guard = CoordinationPlan::new_fixture(
                        vec![
                            DirectoryEffect::tree(&home, CoordinationMode::Shared),
                            DirectoryEffect::tree(&project, CoordinationMode::Shared),
                        ],
                        temp.path(),
                        Some(std::time::Duration::from_secs(10)),
                    )
                    .unwrap()
                    .acquire()
                    .unwrap()
                    .continue_with_files(scope, &[], CoordinationMode::Shared)
                    .unwrap();
                    let result =
                        read_inventory(&context, &mut cache, named.then_some(&names), guard)
                            .unwrap();
                    let actual = result
                        .skills
                        .iter()
                        .map(|skill| skill.name.as_str())
                        .collect::<BTreeSet<_>>();
                    let expected = if named {
                        BTreeSet::from(["alpha"])
                    } else {
                        BTreeSet::from(["alpha", "beta"])
                    };
                    assert_eq!(actual, expected);
                    assert_eq!(
                        result
                            .ledger_only
                            .iter()
                            .map(|record| record.name.as_str())
                            .collect::<Vec<_>>(),
                        if named || invalid_ledger {
                            vec![]
                        } else {
                            vec!["ledger-only"]
                        }
                    );
                    assert!(result
                        .skills
                        .iter()
                        .all(|skill| !skill.deployments.is_empty()));
                    assert_eq!(
                        result.extent,
                        if named {
                            DiscoveryExtent::Named
                        } else {
                            DiscoveryExtent::Full
                        }
                    );
                    assert_eq!(result.ownership.failures().is_empty(), !invalid_ledger);
                    assert!(!result.source_coverage.is_empty());
                }
                assert_eq!(
                    fs::read_to_string(home.join(".agents/skills/alpha/SKILL.md")).unwrap(),
                    "---\nname: alpha\ndescription: fixture\n---\nbody\n"
                );
            }
        }
    }
}

#[derive(Debug)]
pub enum RepairPreviewError {
    Scan(ScanError),
    IncompleteInventory,
    TargetNotFound,
    AmbiguousTarget,
    DocumentUnavailable,
    UnsupportedRepair,
}
impl RepairPreviewError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Scan(error) => error.code(),
            Self::IncompleteInventory => "incomplete_inventory",
            Self::TargetNotFound => "target_not_found",
            Self::AmbiguousTarget => "ambiguous_target",
            Self::DocumentUnavailable => "document_unavailable",
            Self::UnsupportedRepair => "unsupported_repair",
        }
    }
}
impl std::fmt::Display for RepairPreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}
impl std::error::Error for RepairPreviewError {}

pub struct PreparedRepairSelection<'scope> {
    owner_kind: crate::skill_ownership::LifecycleOwnerKind,
    owner_revision: Option<String>,
    preview: crate::skill_frontmatter_repair::FrontmatterRepairPreview,
    mode: crate::skill_frontmatter_repair::FrontmatterRepairApplyMode,
    lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
}

impl<'scope> PreparedRepairSelection<'scope> {
    pub fn owner_kind(&self) -> crate::skill_ownership::LifecycleOwnerKind {
        self.owner_kind
    }

    pub fn owner_revision(&self) -> Option<&str> {
        self.owner_revision.as_deref()
    }

    pub fn preview(&self) -> &crate::skill_frontmatter_repair::FrontmatterRepairPreview {
        &self.preview
    }

    pub fn revalidate(&self) -> Result<(), crate::skill_coordination::CoordinationFailure> {
        self.lease.revalidate()
    }

    pub fn into_parts(
        self,
    ) -> (
        crate::skill_frontmatter_repair::FrontmatterRepairPreview,
        crate::skill_frontmatter_repair::FrontmatterRepairApplyMode,
        crate::skill_coordination::FinalizedWriteLease<'scope>,
    ) {
        (self.preview, self.mode, self.lease)
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyRepairSelection<'scope> {
    selection: PreparedRepairSelection<'scope>,
    transition: crate::skill_copy_repair::CopyRepairTransition,
    registry_path: PathBuf,
    registry_original: Vec<u8>,
    registry_proposed: Vec<u8>,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
    folder_hashes: (String, String),
}

#[cfg(all(unix, feature = "event-store"))]
impl<'scope> PreparedCopyRepairSelection<'scope> {
    pub fn preview(&self) -> &crate::skill_frontmatter_repair::FrontmatterRepairPreview {
        self.selection.preview()
    }

    #[cfg(feature = "event-store")]
    pub(crate) fn into_execution_parts(
        self,
    ) -> (PreparedRepairSelection<'scope>, PathBuf, Vec<u8>, Vec<u8>) {
        (
            self.selection,
            self.registry_path,
            self.registry_original,
            self.registry_proposed,
        )
    }

    pub fn transition(&self) -> &crate::skill_copy_repair::CopyRepairTransition {
        &self.transition
    }

    pub fn registry_change(&self) -> (&std::path::Path, &[u8], &[u8]) {
        (
            &self.registry_path,
            &self.registry_original,
            &self.registry_proposed,
        )
    }

    pub fn revalidate(&self) -> Result<(), String> {
        let hashes = self.content.hashes(
            &self.content_scope,
            &self.selection.lease,
            self.preview().original_content.as_bytes(),
            self.preview().proposed_content.as_bytes(),
        )?;
        if hashes != self.folder_hashes {
            return Err("Prepared copy repair folder changed".into());
        }
        self.selection
            .lease
            .revalidate()
            .map_err(|error| error.to_string())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyRepairRecovery<'scope> {
    pub(crate) event: crate::skill_repair_recovery_event::CopyRepairRecoveryEvent,
    pub(crate) state: crate::skill_copy_repair::CopyRepairObservedState,
    pub(crate) registry_original: Vec<u8>,
    pub(crate) lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    backup: crate::skill_repair_backup::VerifiedCopyRepairBackup,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
    document: Vec<u8>,
}

#[cfg(all(unix, feature = "event-store"))]
impl PreparedCopyRepairRecovery<'_> {
    pub fn state(&self) -> crate::skill_copy_repair::CopyRepairObservedState {
        self.state
    }
    pub fn revalidate(&self) -> Result<(), String> {
        self.backup.revalidate(&self.lease)?;
        let (hash, _) = self.content.hashes(
            &self.content_scope,
            &self.lease,
            &self.document,
            &self.document,
        )?;
        let state = self.event.intent().classify_observed(
            &self.document,
            &self.registry_original,
            &hash,
        )?;
        if state != self.state {
            return Err("Copy recovery state changed".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyRedoRecovery<'scope> {
    pub(crate) event: crate::skill_repair_recovery_event::CopyRedoRecoveryEvent,
    pub(crate) state: crate::skill_copy_repair::CopyRepairObservedState,
    pub(crate) registry_original: Vec<u8>,
    pub(crate) lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    backup: crate::skill_repair_backup::VerifiedCopyRedoBackups,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
    document: Vec<u8>,
}

#[cfg(all(unix, feature = "event-store"))]
impl PreparedCopyRedoRecovery<'_> {
    pub fn state(&self) -> crate::skill_copy_repair::CopyRepairObservedState {
        self.state
    }
    pub fn revalidate(&self) -> Result<(), String> {
        self.backup.revalidate(&self.lease)?;
        let (hash, _) = self.content.hashes(
            &self.content_scope,
            &self.lease,
            &self.document,
            &self.document,
        )?;
        let state = self.event.intent().repair.classify_observed(
            &self.document,
            &self.registry_original,
            &hash,
        )?;
        if state != self.state {
            return Err("Copy redo recovery state changed".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyUndoRecovery<'scope> {
    pub(crate) event: crate::skill_repair_recovery_event::CopyUndoRecoveryEvent,
    pub(crate) state: crate::skill_copy_repair::CopyUndoObservedState,
    pub(crate) registry_original: Vec<u8>,
    pub(crate) lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    backup: crate::skill_repair_backup::VerifiedCopyUndoBackups,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
    document: Vec<u8>,
}

#[cfg(all(unix, feature = "event-store"))]
impl PreparedCopyUndoRecovery<'_> {
    pub fn state(&self) -> crate::skill_copy_repair::CopyUndoObservedState {
        self.state
    }
    pub fn revalidate(&self) -> Result<(), String> {
        self.backup.revalidate(&self.lease)?;
        let (hash, _) = self.content.hashes(
            &self.content_scope,
            &self.lease,
            &self.document,
            &self.document,
        )?;
        let state = self.event.intent().classify_undo_observed(
            &self.document,
            &self.registry_original,
            &hash,
        )?;
        if state != self.state {
            return Err("Copy undo recovery state changed".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyRepairUndo<'scope> {
    pub(crate) source: crate::skill_repair_recovery_event::CopyRepairUndoSource,
    pub(crate) plan: crate::skill_copy_repair::CopyRepairUndoPlan,
    pub(crate) lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    backup: crate::skill_repair_backup::VerifiedCopyRepairBackup,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
}

#[cfg(all(unix, feature = "event-store"))]
impl PreparedCopyRepairUndo<'_> {
    pub fn plan(&self) -> &crate::skill_copy_repair::CopyRepairUndoPlan {
        &self.plan
    }
    pub fn revalidate(&self) -> Result<(), String> {
        self.backup.revalidate(&self.lease)?;
        let document = self.plan.document_change().0;
        let (hash, _) =
            self.content
                .hashes(&self.content_scope, &self.lease, document, document)?;
        if self
            .source
            .intent()
            .classify_observed(document, self.plan.registry_change().0, &hash)?
            != crate::skill_copy_repair::CopyRepairObservedState::Applied
        {
            return Err("Copy undo target no longer matches the repaired state".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedCopyRepairRedo<'scope> {
    pub(crate) source: crate::skill_repair_recovery_event::CopyRepairRedoSource,
    pub(crate) plan: crate::skill_copy_repair::CopyRepairRedoPlan,
    pub(crate) lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
    backup: crate::skill_repair_backup::VerifiedCopyUndoBackups,
    content: crate::skill_discovery::PreparedCopyRepairContent,
    content_scope: crate::skill_scope::SkillReadScope,
}

#[cfg(all(unix, feature = "event-store"))]
impl PreparedCopyRepairRedo<'_> {
    pub fn plan(&self) -> &crate::skill_copy_repair::CopyRepairRedoPlan {
        &self.plan
    }
    pub fn revalidate(&self) -> Result<(), String> {
        self.backup.revalidate(&self.lease)?;
        let document = self.plan.document_change().0;
        let (hash, _) =
            self.content
                .hashes(&self.content_scope, &self.lease, document, document)?;
        if self.source.intent().classify_undo_observed(
            document,
            self.plan.registry_change().0,
            &hash,
        )? != crate::skill_copy_repair::CopyUndoObservedState::Restored
        {
            return Err("Copy redo target no longer matches the undone state".into());
        }
        Ok(())
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub(crate) fn exact_repair_deployment<'a>(
    inventory: &'a InventoryRead,
    id: &str,
) -> Result<&'a crate::skill_inventory::Deployment, WritePreparationError> {
    let mut matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == id);
    let deployment = matches.next().ok_or_else(|| {
        WritePreparationError::InvalidRepairSelection("Selected deployment is absent".into())
    })?;
    if matches.next().is_some() {
        return Err(WritePreparationError::InvalidRepairSelection(
            "Selected deployment is ambiguous".into(),
        ));
    }
    Ok(deployment)
}

#[cfg(all(unix, feature = "event-store"))]
impl ScopedSkillService {
    pub fn prepare_repair_selection(
        &mut self,
        request: &crate::skill_frontmatter_repair::BoundFrontmatterRepairRequest,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedRepairSelection<'_>, WritePreparationError> {
        let selected = crate::skill_deployment::parse_deployment_id(&request.deployment_id)
            .ok_or_else(|| {
                WritePreparationError::InvalidRepairSelection("Invalid deployment ID".into())
            })?;
        let names = BTreeSet::from([selected.name]);
        let (inventory, lease) =
            self.prepare_write_inventory(Some(&names), additional_trees, timeout, cancellation)?;
        let deployment = exact_repair_deployment(&inventory, &request.deployment_id)?;
        let path = PathBuf::from(&deployment.path).join("SKILL.md");
        let bytes = lease
            .read(&path, MAX_REPAIR_DOCUMENT_BYTES)
            .map_err(|error| WritePreparationError::InvalidRepairSelection(error.to_string()))?;
        let preview = request
            .validate(deployment, &bytes)
            .map_err(WritePreparationError::InvalidRepairSelection)?;
        lease.revalidate()?;
        Ok(PreparedRepairSelection {
            owner_kind: deployment.owner_kind,
            owner_revision: deployment.owner_revision.clone(),
            preview,
            mode: request.mode,
            lease,
        })
    }

    pub fn prepare_copy_repair_selection(
        &mut self,
        request: &crate::skill_frontmatter_repair::BoundFrontmatterRepairRequest,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRepairSelection<'_>, WritePreparationError> {
        let registry_path = self.context.home().join(".agents/skill-studio.json");
        let selection = self.prepare_repair_selection(
            request,
            additional_trees,
            timeout,
            cancellation.clone(),
        )?;
        let invalid = WritePreparationError::InvalidRepairSelection;
        if selection.owner_kind() != crate::skill_ownership::LifecycleOwnerKind::Copy {
            return Err(invalid(
                "Selected repair is not owned by a copy record".into(),
            ));
        }
        let skill_dir = PathBuf::from(&selection.preview().path);
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&skill_dir))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &skill_dir,
            &cancellation,
        )
        .map_err(invalid)?;
        let (original_hash, proposed_hash) = content
            .hashes(
                &content_scope,
                &selection.lease,
                selection.preview().original_content.as_bytes(),
                selection.preview().proposed_content.as_bytes(),
            )
            .map_err(invalid)?;
        let registry_original = selection
            .lease
            .read(&registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let registry: crate::skill_fork_registry::ForkRegistry =
            serde_json::from_slice(&registry_original)
                .map_err(|error| invalid(error.to_string()))?;
        let record = registry
            .copies
            .get(&request.deployment_id)
            .ok_or_else(|| invalid("Copy registry record is missing".into()))?;
        if record.content_hash != original_hash {
            return Err(invalid(
                "Copy folder does not match its registry hash".into(),
            ));
        }
        let folder_hashes = (original_hash, proposed_hash.clone());
        let transition =
            crate::skill_copy_repair::CopyRepairTransition::new(record.clone(), proposed_hash)
                .map_err(invalid)?;
        let registry_proposed = transition
            .apply_document(&registry_original)
            .map_err(invalid)?;
        selection.lease.revalidate()?;
        Ok(PreparedCopyRepairSelection {
            selection,
            transition,
            registry_path,
            registry_original,
            registry_proposed,
            content,
            content_scope,
            folder_hashes,
        })
    }

    pub fn prepare_copy_repair_recovery(
        &mut self,
        row: &crate::skill_event::EventRow,
        store: &crate::skill_event_store::EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRepairRecovery<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let event = crate::skill_repair_recovery_event::CopyRepairRecoveryEvent::from_row(row)
            .map_err(invalid)?;
        let intent = event.intent();
        if intent.registry_path != self.context.home().join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy recovery registry is outside the selected home".into(),
            ));
        }
        let names = BTreeSet::from([intent.document.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &intent.document.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.document.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != intent.transition.is_disabled()
            || !matches!(
                deployment.owner_kind,
                crate::skill_ownership::LifecycleOwnerKind::Copy
                    | crate::skill_ownership::LifecycleOwnerKind::Unknown
                    | crate::skill_ownership::LifecycleOwnerKind::Manual
                    | crate::skill_ownership::LifecycleOwnerKind::InRepo
            )
        {
            return Err(invalid(format!(
                "Copy recovery target ownership or location changed: {:?}",
                deployment.owner_kind
            )));
        }
        let backup = crate::skill_repair_backup::VerifiedCopyRepairBackup::read(
            &store.app_data,
            &event,
            &lease,
        )
        .map_err(invalid)?;
        let document = lease
            .read(
                &intent.document.path.join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let registry_original = lease
            .read(&intent.registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&intent.document.path))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &intent.document.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let (hash, _) = content
            .hashes(&content_scope, &lease, &document, &document)
            .map_err(invalid)?;
        let state = intent
            .classify_observed(&document, &registry_original, &hash)
            .map_err(invalid)?;
        let prepared = PreparedCopyRepairRecovery {
            event,
            state,
            registry_original,
            lease,
            backup,
            content,
            content_scope,
            document,
        };
        prepared.revalidate().map_err(invalid)?;
        Ok(prepared)
    }

    pub fn prepare_copy_redo_recovery(
        &mut self,
        source_row: &crate::skill_event::EventRow,
        undo_row: &crate::skill_event::EventRow,
        redo_row: &crate::skill_event::EventRow,
        store: &crate::skill_event_store::EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRedoRecovery<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let event = crate::skill_repair_recovery_event::CopyRedoRecoveryEvent::from_rows(
            source_row, undo_row, redo_row,
        )
        .map_err(invalid)?;
        let intent = &event.intent().repair;
        if intent.registry_path != self.context.home().join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy redo recovery registry is outside the selected home".into(),
            ));
        }
        let names = BTreeSet::from([intent.document.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &intent.document.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.document.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != intent.transition.is_disabled()
            || !matches!(
                deployment.owner_kind,
                crate::skill_ownership::LifecycleOwnerKind::Copy
                    | crate::skill_ownership::LifecycleOwnerKind::Unknown
                    | crate::skill_ownership::LifecycleOwnerKind::Manual
                    | crate::skill_ownership::LifecycleOwnerKind::InRepo
            )
        {
            return Err(invalid(format!(
                "Copy redo recovery target ownership or location changed: {:?}",
                deployment.owner_kind
            )));
        }
        let backup = crate::skill_repair_backup::VerifiedCopyRedoBackups::read(
            &store.app_data,
            &event,
            &lease,
        )
        .map_err(invalid)?;
        let document = lease
            .read(
                &intent.document.path.join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let registry_original = lease
            .read(&intent.registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&intent.document.path))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &intent.document.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let (hash, _) = content
            .hashes(&content_scope, &lease, &document, &document)
            .map_err(invalid)?;
        let state = intent
            .classify_observed(&document, &registry_original, &hash)
            .map_err(invalid)?;
        let prepared = PreparedCopyRedoRecovery {
            event,
            state,
            registry_original,
            lease,
            backup,
            content,
            content_scope,
            document,
        };
        prepared.revalidate().map_err(invalid)?;
        Ok(prepared)
    }

    pub fn prepare_copy_undo_recovery(
        &mut self,
        source_row: &crate::skill_event::EventRow,
        undo_row: &crate::skill_event::EventRow,
        store: &crate::skill_event_store::EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyUndoRecovery<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let event = crate::skill_repair_recovery_event::CopyUndoRecoveryEvent::from_rows(
            source_row, undo_row,
        )
        .map_err(invalid)?;
        let intent = event.intent();
        if intent.registry_path != self.context.home().join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy undo recovery registry is outside the selected home".into(),
            ));
        }
        let names = BTreeSet::from([intent.document.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &intent.document.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.document.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != intent.transition.is_disabled()
            || !matches!(
                deployment.owner_kind,
                crate::skill_ownership::LifecycleOwnerKind::Copy
                    | crate::skill_ownership::LifecycleOwnerKind::Unknown
                    | crate::skill_ownership::LifecycleOwnerKind::Manual
                    | crate::skill_ownership::LifecycleOwnerKind::InRepo
            )
        {
            return Err(invalid(format!(
                "Copy undo recovery target ownership or location changed: {:?}",
                deployment.owner_kind
            )));
        }
        let backup = crate::skill_repair_backup::VerifiedCopyUndoBackups::read(
            &store.app_data,
            &event,
            &lease,
        )
        .map_err(invalid)?;
        let document = lease
            .read(
                &intent.document.path.join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let registry_original = lease
            .read(&intent.registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&intent.document.path))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &intent.document.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let (hash, _) = content
            .hashes(&content_scope, &lease, &document, &document)
            .map_err(invalid)?;
        let state = intent
            .classify_undo_observed(&document, &registry_original, &hash)
            .map_err(invalid)?;
        let prepared = PreparedCopyUndoRecovery {
            event,
            state,
            registry_original,
            lease,
            backup,
            content,
            content_scope,
            document,
        };
        prepared.revalidate().map_err(invalid)?;
        Ok(prepared)
    }

    pub fn prepare_copy_repair_undo(
        &mut self,
        row: &crate::skill_event::EventRow,
        store: &crate::skill_event_store::EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRepairUndo<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let source = crate::skill_repair_recovery_event::CopyRepairUndoSource::from_row(row)
            .map_err(invalid)?;
        let intent = source.intent();
        if intent.registry_path != self.context.home().join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy undo registry is outside the selected home".into(),
            ));
        }
        let names = BTreeSet::from([intent.document.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &intent.document.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.document.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != intent.transition.is_disabled()
            || deployment.owner_kind != crate::skill_ownership::LifecycleOwnerKind::Copy
        {
            return Err(invalid(format!(
                "Copy undo target ownership or location changed: {:?}",
                deployment.owner_kind
            )));
        }
        let backup = crate::skill_repair_backup::VerifiedCopyRepairBackup::read_undo_source(
            &store.app_data,
            &source,
            &lease,
        )
        .map_err(invalid)?;
        let document = lease
            .read(
                &intent.document.path.join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let registry_original = lease
            .read(&intent.registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&intent.document.path))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &intent.document.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let (hash, _) = content
            .hashes(&content_scope, &lease, &document, &document)
            .map_err(invalid)?;
        let plan = crate::skill_copy_repair::CopyRepairUndoPlan::from_observed(
            &source,
            &backup,
            &document,
            &registry_original,
            &hash,
        )
        .map_err(invalid)?;
        let prepared = PreparedCopyRepairUndo {
            source,
            plan,
            lease,
            backup,
            content,
            content_scope,
        };
        prepared.revalidate().map_err(invalid)?;
        Ok(prepared)
    }

    pub fn prepare_copy_repair_redo(
        &mut self,
        source_row: &crate::skill_event::EventRow,
        undo_row: &crate::skill_event::EventRow,
        store: &crate::skill_event_store::EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyRepairRedo<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let source = crate::skill_repair_recovery_event::CopyRepairRedoSource::from_rows(
            source_row, undo_row,
        )
        .map_err(invalid)?;
        let intent = source.intent();
        if intent.registry_path != self.context.home().join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy redo registry is outside the selected home".into(),
            ));
        }
        let names = BTreeSet::from([intent.document.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &intent.document.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.document.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != intent.transition.is_disabled()
            || deployment.owner_kind != crate::skill_ownership::LifecycleOwnerKind::Copy
        {
            return Err(invalid(format!(
                "Copy redo target ownership or location changed: {:?}",
                deployment.owner_kind
            )));
        }
        let backup = crate::skill_repair_backup::VerifiedCopyUndoBackups::read_redo_source(
            &store.app_data,
            &source,
            &lease,
        )
        .map_err(invalid)?;
        let document = lease
            .read(
                &intent.document.path.join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let registry_original = lease
            .read(&intent.registry_path, 8 * 1024 * 1024)
            .map_err(|error| invalid(error.to_string()))?;
        let content_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&intent.document.path))
                .map_err(|error| invalid(error.to_string()))?;
        let content = crate::skill_discovery::PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &intent.document.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let (hash, _) = content
            .hashes(&content_scope, &lease, &document, &document)
            .map_err(invalid)?;
        let plan = crate::skill_copy_repair::CopyRepairRedoPlan::from_observed(
            &source,
            &backup,
            &document,
            &registry_original,
            &hash,
        )
        .map_err(invalid)?;
        let prepared = PreparedCopyRepairRedo {
            source,
            plan,
            lease,
            backup,
            content,
            content_scope,
        };
        prepared.revalidate().map_err(invalid)?;
        Ok(prepared)
    }

    pub fn preview_frontmatter_repair(
        &mut self,
        deployment_id: &str,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<crate::skill_frontmatter_repair::FrontmatterRepairPreview, RepairPreviewError> {
        let span = tracing::info_span!(
            "skill.repair.preview",
            outcome = "running",
            error_code = tracing::field::Empty,
            duration_ms = tracing::field::Empty
        );
        let _entered = span.enter();
        let started = Instant::now();
        let result = self.preview_frontmatter_repair_read(deployment_id, timeout, cancellation);
        let (outcome, code) = match &result {
            Ok(_) => ("complete", "none"),
            Err(RepairPreviewError::IncompleteInventory) => ("partial", "incomplete_inventory"),
            Err(RepairPreviewError::Scan(ScanError::Coordination(
                CoordinationFailure::Cancelled,
            ))) => ("cancelled", "cancelled"),
            Err(error) => ("failed", error.code()),
        };
        let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        span.record("outcome", outcome);
        span.record("error_code", code);
        span.record("duration_ms", duration_ms);
        tracing::info!(
            outcome,
            error_code = code,
            duration_ms,
            "skill.repair.preview.finished"
        );
        result
    }

    fn preview_frontmatter_repair_read(
        &mut self,
        deployment_id: &str,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<crate::skill_frontmatter_repair::FrontmatterRepairPreview, RepairPreviewError> {
        let selected = crate::skill_deployment::parse_deployment_id(deployment_id)
            .ok_or(RepairPreviewError::TargetNotFound)?;
        let names = BTreeSet::from([selected.name]);
        let (inventory, guard) = self
            .scan_read_guarded(Some(&names), timeout, Some(cancellation))
            .map_err(RepairPreviewError::Scan)?;
        if !inventory_read_blockers(
            &inventory.scope,
            inventory.extent,
            &inventory.source_coverage,
            &inventory.ownership,
        )
        .is_empty()
        {
            return Err(RepairPreviewError::IncompleteInventory);
        }
        let mut matches = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .filter(|deployment| deployment.id == deployment_id);
        let deployment = matches.next().ok_or(RepairPreviewError::TargetNotFound)?;
        if matches.next().is_some() {
            return Err(RepairPreviewError::AmbiguousTarget);
        }
        if matches!(
            deployment.owner_kind,
            crate::skill_ownership::LifecycleOwnerKind::Unknown
                | crate::skill_ownership::LifecycleOwnerKind::Ambiguous
        ) {
            return Err(RepairPreviewError::IncompleteInventory);
        }
        let path = PathBuf::from(&deployment.path).join("SKILL.md");
        let scope = self.context.read_scope();
        let guard = guard
            .extend_with_files(scope, std::slice::from_ref(&path))
            .and_then(|guard| guard.finalize(scope))
            .map_err(|error| RepairPreviewError::Scan(error.into()))?;
        let bytes = guard.read(&path, MAX_REPAIR_DOCUMENT_BYTES);
        guard
            .check_cancelled()
            .map_err(|error| RepairPreviewError::Scan(error.into()))?;
        guard
            .revalidate()
            .map_err(|error| RepairPreviewError::Scan(error.into()))?;
        let bytes = bytes.map_err(|_| RepairPreviewError::DocumentUnavailable)?;
        let preview =
            crate::skill_frontmatter_repair::preview_frontmatter_repair(deployment, &bytes);
        guard
            .revalidate()
            .map_err(|error| RepairPreviewError::Scan(error.into()))?;
        self.context
            .revalidate_roots()
            .map_err(|_| RepairPreviewError::Scan(CoordinationFailure::Changed.into()))?;
        guard
            .check_cancelled()
            .map_err(|error| RepairPreviewError::Scan(error.into()))?;
        preview.map_err(|_| RepairPreviewError::UnsupportedRepair)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairRecoveryDocumentState {
    Original,
    Proposed,
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedRepairRecovery<'scope> {
    deployment: crate::skill_inventory::Deployment,
    document_state: RepairRecoveryDocumentState,
    lease: crate::skill_coordination::FinalizedWriteLease<'scope>,
}

#[cfg(all(unix, feature = "event-store"))]
impl<'scope> PreparedRepairRecovery<'scope> {
    pub fn into_parts(
        self,
    ) -> (
        crate::skill_inventory::Deployment,
        RepairRecoveryDocumentState,
        crate::skill_coordination::FinalizedWriteLease<'scope>,
    ) {
        (self.deployment, self.document_state, self.lease)
    }
}

#[cfg(all(unix, feature = "event-store"))]
pub struct PreparedRepairEventRecovery<'scope> {
    event: crate::skill_repair_recovery_event::RepairRecoveryEvent,
    recovery: PreparedRepairRecovery<'scope>,
}

#[cfg(all(unix, feature = "event-store"))]
impl<'scope> PreparedRepairEventRecovery<'scope> {
    pub fn into_parts(
        self,
    ) -> (
        crate::skill_repair_recovery_event::RepairRecoveryEvent,
        PreparedRepairRecovery<'scope>,
    ) {
        (self.event, self.recovery)
    }
}

#[cfg(all(unix, feature = "event-store"))]
impl ScopedSkillService {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn prepare_direct_restore(
        &mut self,
        row: &crate::skill_event::EventRow,
        state_root: &std::path::Path,
        force: bool,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<crate::skill_direct_restore::PreparedDirectRestore<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let source = crate::skill_direct_restore::DirectRestoreSource::from_row(row, None)
            .map_err(invalid)?;
        let prepared =
            self.prepare_direct_restore_target(source, state_root, timeout, cancellation)?;
        if !force
            && crate::skill_event_store::fingerprint_regular_bytes(&prepared.current)
                != prepared.source.after
        {
            return Err(invalid("Document changed since the event; force must preserve current bytes before restore".into()));
        }
        Ok(prepared)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(crate) fn prepare_direct_restore_target(
        &mut self,
        source: crate::skill_direct_restore::DirectRestoreSource,
        state_root: &std::path::Path,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<crate::skill_direct_restore::PreparedDirectRestore<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let names = BTreeSet::from([source.repair.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            &[state_root.to_path_buf()],
            timeout,
            cancellation,
        )?;
        let deployment = exact_repair_deployment(&inventory, &source.repair.deployment_id)?;
        source
            .repair
            .validate_direct_recovery_deployment(deployment)
            .map_err(invalid)?;
        if deployment.owner_kind == crate::skill_ownership::LifecycleOwnerKind::Copy {
            return Err(invalid(
                "Copy restore requires its registry transaction".into(),
            ));
        }
        let document = source.repair.path.join("SKILL.md");
        let current = lease
            .read(&document, MAX_REPAIR_DOCUMENT_BYTES)
            .map_err(|error| invalid(error.to_string()))?;
        let backup = crate::skill_repair_backup::VerifiedRepairBackup::read_document(
            state_root,
            &source.row.id,
            &document,
            &source.before,
            &lease,
        )
        .map_err(invalid)?;
        if source.row.kind == "repair_skill_frontmatter" {
            source
                .repair
                .validate_original(backup.original())
                .map_err(invalid)?;
        }
        lease.revalidate()?;
        Ok(crate::skill_direct_restore::PreparedDirectRestore {
            source,
            current,
            backup,
            lease,
        })
    }

    pub fn prepare_repair_event_recovery(
        &mut self,
        row: &crate::skill_event::EventRow,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedRepairEventRecovery<'_>, WritePreparationError> {
        let event = crate::skill_repair_recovery_event::RepairRecoveryEvent::from_row(row)
            .map_err(WritePreparationError::InvalidRepairSelection)?;
        let recovery =
            self.prepare_repair_recovery(event.intent(), additional_trees, timeout, cancellation)?;
        Ok(PreparedRepairEventRecovery { event, recovery })
    }
    pub fn prepare_repair_recovery(
        &mut self,
        intent: &crate::skill_repair_intent::FrontmatterRepairIntent,
        additional_trees: &[PathBuf],
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedRepairRecovery<'_>, WritePreparationError> {
        intent
            .validate_record()
            .map_err(WritePreparationError::InvalidRepairSelection)?;
        let names = BTreeSet::from([intent.name.clone()]);
        let (inventory, lease) =
            self.prepare_write_inventory(Some(&names), additional_trees, timeout, cancellation)?;
        let deployment = exact_repair_deployment(&inventory, &intent.deployment_id)?;
        if std::path::Path::new(&deployment.path) != intent.path {
            return Err(WritePreparationError::InvalidRepairSelection(
                "Recovery target path changed".into(),
            ));
        }
        if intent.mode != crate::skill_frontmatter_repair::FrontmatterRepairApplyMode::ForkAndFix {
            intent
                .validate_direct_recovery_deployment(deployment)
                .map_err(WritePreparationError::InvalidRepairSelection)?;
        }
        let bytes = lease
            .read(
                &PathBuf::from(&deployment.path).join("SKILL.md"),
                MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| WritePreparationError::InvalidRepairSelection(error.to_string()))?;
        let fingerprint = crate::skill_frontmatter_repair::content_fingerprint(&bytes);
        let document_state = if fingerprint == intent.expected_content_fingerprint {
            intent
                .validate_original(&bytes)
                .map_err(WritePreparationError::InvalidRepairSelection)?;
            RepairRecoveryDocumentState::Original
        } else if fingerprint == intent.proposed_content_fingerprint {
            RepairRecoveryDocumentState::Proposed
        } else {
            return Err(WritePreparationError::InvalidRepairSelection(
                "Recovery document differs from both sides of the intent".into(),
            ));
        };
        lease.revalidate()?;
        Ok(PreparedRepairRecovery {
            deployment: deployment.clone(),
            document_state,
            lease,
        })
    }
}
