// ============================================================================
// Skills Module - Directory Scanner
// The only module that walks the filesystem looking for installed skills.
// Walks the agent skill roots (agents::skill_roots) and the native plugin
// caches (plugins::scan_plugin_skills), capturing every fact a SkillCandidate
// needs. Classification and merging happen downstream, in provenance.rs and
// skill_assembly.rs, which never touch disk.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(test)]
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tiktoken_rs::CoreBPE;

use crate::skill_agents as agents;
use crate::skill_candidate::{GitRepoEvidence, PluginEvidence, SkillCandidate};
use crate::skill_coordination::PreparedContentError;
use crate::skill_document::{
    frontmatter_fields, parse_frontmatter, validate_skill, FrontmatterParseResult,
};
use crate::skill_plugins as plugins;
pub use crate::skill_plugins::SkillDiscoveryReadContext;
use crate::skill_read::{
    DiscoveryReadIssue, DiscoveryReadIssueKind, MembershipSource, SourceCoverage,
};
use crate::skill_scope::{
    ScopedContentFoldError, ScopedContentObservation, ScopedReadError, SkillReadScope,
};

#[derive(Clone, Copy)]
struct SkillContentRead<'a> {
    scope: &'a SkillReadScope,
    guard: Option<&'a crate::skill_coordination::CoordinatedReadGuard>,
}

impl<'a> From<&'a SkillReadScope> for SkillContentRead<'a> {
    fn from(scope: &'a SkillReadScope) -> Self {
        Self { scope, guard: None }
    }
}

impl SkillContentRead<'_> {
    fn prefix(
        self,
        observation: &ScopedContentObservation,
        limit: usize,
    ) -> Result<crate::skill_scope::ScopedPrefixRead, ScopedContentFoldError> {
        if let Some(guard) = self.guard {
            guard.check_content_observation(observation)?;
            guard.read_prefix(self.scope, &observation.requested, limit, &mut || Ok(()))
        } else {
            self.scope
                .read_content_prefix(observation, limit, &mut || Ok(()))
        }
    }

    fn fold(
        self,
        observation: &ScopedContentObservation,
        limit: u64,
        check: &mut dyn FnMut() -> Result<(), String>,
        fold: &mut dyn FnMut(&[u8]),
    ) -> Result<u64, ScopedContentFoldError> {
        if let Some(guard) = self.guard {
            guard.check_content_observation(observation)?;
            guard.fold(self.scope, &observation.requested, limit, check, fold)
        } else {
            self.scope
                .fold_observed_content(observation, limit, check, fold)
        }
    }
}

/// Folder walk caps: a skill folder is hashed and sized up to this many
/// files or bytes, whichever comes first. Beyond that, `folder_truncated` is
/// set and the remaining entries are skipped rather than read.
const MAX_FOLDER_FILES: usize = 2_000;
const MAX_FOLDER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FOLDER_ENTRIES: usize = 8_192;

/// SKILL.md is read through a bounded reader capped at this many bytes, so a
/// pathologically large SKILL.md can't be read into memory in full. Counted
/// against the same `MAX_FOLDER_BYTES` budget as the rest of the folder.
const SKILL_MD_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The embedded cl100k_base vocab is loaded once per process, not once per
/// rebuild - rebuilding the BPE from scratch on every snapshot rebuild (which
/// can happen every few seconds from the background watcher) is pure waste
/// since the vocab never changes.
static TOKENIZER: OnceLock<Option<CoreBPE>> = OnceLock::new();

/// Candidates and read issues from this scanner. `read_issues` covers agent
/// roots, their entries, and the plugin skills supplied by the plugin
/// enumerator. It does not establish complete plugin or ownership coverage.
#[derive(Debug)]
pub struct DiscoveryReport {
    pub candidates: Vec<SkillCandidate>,
    pub read_issues: Vec<DiscoveryReadIssue>,
    #[allow(dead_code)]
    pub(crate) extent: DiscoveryExtent,
    /// Typed observations from the exact full or named operations. These are
    /// not a desktop wire contract; the service will consume them later.
    #[allow(dead_code)]
    pub(crate) source_coverage: Vec<SourceCoverage>,
}

pub(crate) use crate::skill_read::DiscoveryExtent;
pub(crate) use crate::skill_read::SourceReadOutcome as RootReadOutcome;

impl Default for DiscoveryReport {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            read_issues: Vec::new(),
            extent: DiscoveryExtent::Full,
            source_coverage: Vec::new(),
        }
    }
}

fn read_issue(
    issues: &mut Vec<DiscoveryReadIssue>,
    kind: DiscoveryReadIssueKind,
    path: &Path,
    message: impl Into<String>,
) {
    issues.push(DiscoveryReadIssue::new(kind, path, message));
}

fn tokenizer() -> Option<&'static CoreBPE> {
    TOKENIZER
        .get_or_init(|| tiktoken_rs::cl100k_base().ok())
        .as_ref()
}

/// A skill "ships specs" (the getsentry/skillet pattern) when it has a
/// spec.md file or an evals/ subdirectory alongside SKILL.md.
#[cfg(test)]
fn has_spec(
    scope: &SkillReadScope,
    skill_dir: &Path,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> bool {
    PreparedSpecMarkers::enumerate(scope, skill_dir).materialize(scope, skill_dir, issues)
}

struct PreparedSpecMarkers {
    file: Result<ScopedContentObservation, ScopedReadError>,
    directory: Option<Result<PreparedSpecDirectory, ScopedReadError>>,
}

struct PreparedSpecDirectory {
    observation: crate::skill_scope::ScopedEntryObservation,
    target: PathBuf,
}

impl PreparedSpecMarkers {
    fn enumerate(scope: &SkillReadScope, skill_dir: &Path) -> Self {
        let file = scope.observe_content_regular(&skill_dir.join("spec.md"));
        let directory = file.is_err().then(|| {
            let observation = scope.observe_entry(skill_dir, std::ffi::OsStr::new("evals"))?;
            let target = scope.resolve_observed_dir(&observation)?;
            Ok(PreparedSpecDirectory {
                observation,
                target,
            })
        });
        Self { file, directory }
    }

    fn materialize(
        &self,
        scope: &SkillReadScope,
        skill_dir: &Path,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> bool {
        let file_path = skill_dir.join("spec.md");
        match &self.file {
            Ok(observation) => match scope.observe_content_regular(&file_path) {
                Ok(current) if current == *observation => return true,
                _ => read_issue(
                    issues,
                    DiscoveryReadIssueKind::Resource,
                    &file_path,
                    "Spec file changed after enumeration",
                ),
            },
            Err(ScopedReadError::Missing { .. }) => {
                if !matches!(
                    scope.observe_content_regular(&file_path),
                    Err(ScopedReadError::Missing { .. })
                ) {
                    read_issue(
                        issues,
                        DiscoveryReadIssueKind::Resource,
                        &file_path,
                        "Spec file appeared or became unavailable after enumeration",
                    );
                }
            }
            Err(error) => read_issue(
                issues,
                DiscoveryReadIssueKind::Resource,
                &file_path,
                error.to_string(),
            ),
        }
        let directory_path = skill_dir.join("evals");
        match &self.directory {
            None => false,
            Some(Ok(directory)) => match scope.resolve_observed_dir(&directory.observation) {
                Ok(target) if target == directory.target => true,
                _ => {
                    read_issue(
                        issues,
                        DiscoveryReadIssueKind::Resource,
                        &directory_path,
                        "Spec directory changed after enumeration",
                    );
                    false
                }
            },
            Some(Err(ScopedReadError::Missing { .. })) => {
                if !matches!(
                    scope.resolved_dir_path(&directory_path),
                    Err(ScopedReadError::Missing { .. })
                ) {
                    read_issue(
                        issues,
                        DiscoveryReadIssueKind::Resource,
                        &directory_path,
                        "Spec directory appeared or became unavailable after enumeration",
                    );
                }
                false
            }
            Some(Err(error)) => {
                read_issue(
                    issues,
                    DiscoveryReadIssueKind::Resource,
                    &directory_path,
                    error.to_string(),
                );
                false
            }
        }
    }
}

/// A regular file found under a skill folder, queued for hashing once the
/// whole folder has been walked and its entries sorted. Holds only the path,
/// size, and mtime - never the file's bytes - so the walk's memory use
/// doesn't grow with folder size. `len`/`mtime` come straight from the same
/// `fs::Metadata` the walk already reads for `total_bytes`/`newest`, so
/// `folder_fingerprint` costs no extra syscalls.
struct HashableFile {
    rel_path: PathBuf,
    observation: ScopedContentObservation,
    len: u64,
    mtime: Option<SystemTime>,
}

/// Accumulated facts from walking a skill folder.
struct FolderWalk {
    directories: Vec<Arc<crate::skill_scope::ScopedDirectoryObservation>>,
    hashable: Vec<HashableFile>,
    total_bytes: u64,
    file_count: u32,
    newest: Option<SystemTime>,
    truncated: bool,
    incomplete_reason: Option<String>,
    entries_remaining: usize,
}

/// Walk `dir` recursively, gathering byte/file counts and the newest mtime,
/// stopping once `max_files`/`max_bytes` is reached. Never follows symlinks:
/// a symlinked directory is not descended into, and a symlinked file counts
/// toward `file_count` but is never opened or hashed. Unreadable entries are
/// skipped rather than failing the whole walk.
#[cfg(test)]
fn walk_folder(dir: &Path) -> FolderWalk {
    let scope = SkillReadScope::bind(&[dir.to_path_buf()]).expect("test scope binds");
    walk_folder_capped(&scope, dir, MAX_FOLDER_FILES, MAX_FOLDER_BYTES).expect("test walk succeeds")
}

#[cfg(test)]
fn walk_folder_capped(
    scope: &SkillReadScope,
    dir: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<FolderWalk, String> {
    walk_folder_capped_with_check(scope, dir, max_files, max_bytes, &mut || Ok(()))
}

fn walk_folder_capped_with_check<E>(
    scope: &SkillReadScope,
    dir: &Path,
    max_files: usize,
    max_bytes: u64,
    check: &mut dyn FnMut() -> Result<(), E>,
) -> Result<FolderWalk, E> {
    let mut walk = FolderWalk {
        directories: Vec::new(),
        hashable: Vec::new(),
        total_bytes: 0,
        file_count: 0,
        newest: None,
        truncated: false,
        incomplete_reason: None,
        entries_remaining: MAX_FOLDER_ENTRIES,
    };
    walk_folder_into(scope, dir, dir, max_files, max_bytes, &mut walk, check)?;
    Ok(walk)
}

#[cfg(test)]
fn walk_folder_with_entry_budget(
    scope: &SkillReadScope,
    dir: &Path,
    entry_budget: usize,
) -> FolderWalk {
    let mut walk = FolderWalk {
        directories: Vec::new(),
        hashable: Vec::new(),
        total_bytes: 0,
        file_count: 0,
        newest: None,
        truncated: false,
        incomplete_reason: None,
        entries_remaining: entry_budget,
    };
    walk_folder_into(
        scope,
        dir,
        dir,
        MAX_FOLDER_FILES,
        MAX_FOLDER_BYTES,
        &mut walk,
        &mut || Ok::<(), String>(()),
    )
    .unwrap();
    walk
}

fn walk_folder_into<E>(
    scope: &SkillReadScope,
    root: &Path,
    dir: &Path,
    max_files: usize,
    max_bytes: u64,
    walk: &mut FolderWalk,
    check: &mut dyn FnMut() -> Result<(), E>,
) -> Result<(), E> {
    check()?;
    if walk.truncated {
        return Ok(());
    }
    if walk.entries_remaining == 0 {
        walk.truncated = true;
        return Ok(());
    }
    let entries = match scope.read_dir(dir, walk.entries_remaining) {
        Ok(entries) => entries,
        Err(error) => {
            walk.incomplete_reason.get_or_insert_with(|| {
                format!("Could not read directory {}: {error}", dir.display())
            });
            return Ok(());
        }
    };
    walk.directories.push(Arc::new(entries.observation));
    let limit_reached = entries
        .issues
        .iter()
        .any(|issue| matches!(issue, crate::skill_scope::DirectoryReadIssue::LimitReached));
    if !entries.issues.is_empty() {
        walk.incomplete_reason
            .get_or_insert_with(|| format!("Could not fully read directory {}", dir.display()));
        if limit_reached {
            walk.truncated = true;
        }
    }
    let consumed_entries = if limit_reached {
        walk.entries_remaining
    } else {
        entries.entries.len()
            + entries
                .issues
                .iter()
                .filter(|issue| {
                    matches!(issue, crate::skill_scope::DirectoryReadIssue::Entry { .. })
                })
                .count()
    };
    walk.entries_remaining = walk.entries_remaining.saturating_sub(consumed_entries);
    for entry in entries.entries {
        check()?;
        if walk.truncated {
            return Ok(());
        }
        let path = dir.join(&entry.name);
        let file_type = entry.metadata.file_type();

        if file_type.is_symlink() {
            // Resource-file links count as files but their bytes stay out of
            // the digest. Directory links are not descended. Both decisions
            // require scoped resolution so an escaped or broken target stays
            // visible instead of silently disappearing from content facts.
            match scope.observe_content_regular(&path) {
                Ok(_) => {
                    walk.file_count += 1;
                    if walk.file_count as usize >= max_files {
                        walk.truncated = true;
                    }
                }
                Err(_) if scope.read_dir(&path, 0).is_ok() => {}
                Err(error) => {
                    walk.incomplete_reason.get_or_insert_with(|| {
                        format!(
                            "Could not resolve resource link {}: {error}",
                            path.display()
                        )
                    });
                }
            }
            continue;
        }

        if file_type.is_dir() {
            walk_folder_into(scope, root, &path, max_files, max_bytes, walk, check)?;
        } else if file_type.is_file() {
            let meta = entry.metadata;
            // Enforce the remaining byte budget before queuing/reading the
            // file, not after: a single oversized file must never be opened
            // or added to the hash queue, only counted as the reason the
            // walk stopped.
            let remaining = max_bytes.saturating_sub(walk.total_bytes);
            if meta.len() > remaining {
                walk.truncated = true;
                continue;
            }
            walk.total_bytes += meta.len();
            walk.file_count += 1;
            if let Ok(mtime) = meta.modified() {
                let mtime = mtime.into_std();
                if walk.newest.is_none_or(|n| mtime > n) {
                    walk.newest = Some(mtime);
                }
            }
            if let Ok(rel_path) = path.strip_prefix(root) {
                match scope.observe_content_regular(&path) {
                    Ok(observation) if observation.matches_metadata(&meta) => {
                        walk.hashable.push(HashableFile {
                            rel_path: rel_path.to_path_buf(),
                            observation,
                            len: meta.len(),
                            mtime: meta.modified().ok().map(|time| time.into_std()),
                        })
                    }
                    Ok(_) => {
                        walk.incomplete_reason.get_or_insert_with(|| {
                            format!(
                                "Resource changed after directory listing: {}",
                                path.display()
                            )
                        });
                    }
                    Err(error) => {
                        walk.incomplete_reason.get_or_insert_with(|| {
                            format!("Could not observe {}: {error}", path.display())
                        });
                    }
                }
            } else {
                walk.incomplete_reason.get_or_insert_with(|| {
                    format!("Could not map {} under {}", path.display(), root.display())
                });
            }
            if walk.file_count as usize >= max_files || walk.total_bytes >= max_bytes {
                walk.truncated = true;
            }
        } else {
            walk.incomplete_reason
                .get_or_insert_with(|| format!("Cannot hash special file {}", path.display()));
        }
    }
    Ok(())
}

/// sha256 over the sorted (relative path, bytes) pairs of a skill folder.
/// Every record is length-framed - `u64 LE len(rel_path) || rel_path bytes
/// || u64 LE file_len || file bytes` - so that, say, a file "a" containing
/// "bc" hashes differently from a file "ab" containing "c": without framing,
/// the concatenated byte streams for those two folders would be identical.
/// Bytes are streamed straight into the hasher, one file at a time, rather
/// than held in memory all at once. Unreadable files contribute their framed
/// path (and a zero length) to the hash but no bytes, rather than failing
/// the hash. `max_bytes` bounds the total file bytes read across every file:
/// each file is read through `Read::take(remaining)`, so even if a file grew
/// after the walk's own per-file check, no read can push the total past the
/// cap; the length field records the file's real size regardless.
#[cfg(test)]
fn content_hash(mut files: Vec<HashableFile>, max_bytes: u64) -> String {
    let roots = files
        .iter()
        .filter_map(|file| file.observation.requested.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    let scope = SkillReadScope::bind(&roots).expect("test scope binds");
    content_hash_with_check_mode(
        (&scope).into(),
        &mut files,
        max_bytes,
        &mut || Ok(()),
        false,
        None,
    )
    .expect("the no-op content-hash check cannot fail")
}

fn content_hash_with_check_mode(
    reader: SkillContentRead<'_>,
    files: &mut [HashableFile],
    max_bytes: u64,
    check: &mut dyn FnMut() -> Result<(), String>,
    strict: bool,
    mut issues: Option<&mut Vec<DiscoveryReadIssue>>,
) -> Result<String, String> {
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut hasher = Sha256::new();
    let mut remaining = max_bytes;
    for file in files.iter() {
        check()?;
        let rel_path_bytes = file.rel_path.to_string_lossy().into_owned().into_bytes();
        hasher.update((rel_path_bytes.len() as u64).to_le_bytes());
        hasher.update(&rel_path_bytes);

        hasher.update(file.len.to_le_bytes());
        let path = &file.observation.requested;
        if strict && remaining < file.len {
            return Err(format!(
                "Resource could not be read within the discovery limit: {}",
                path.display()
            ));
        }
        let fold_limit = remaining.min(file.len);
        let result = {
            let mut fold = |bytes: &[u8]| {
                hasher.update(bytes);
                remaining = remaining.saturating_sub(bytes.len() as u64);
            };
            reader.fold(&file.observation, fold_limit, check, &mut fold)
        };
        match result {
            Ok(read_total) if read_total == file.len => {}
            Err(ScopedContentFoldError::Cancelled(error)) => return Err(error),
            Err(ScopedContentFoldError::Read(error)) if strict => {
                return Err(format!(
                    "Could not read metadata for {}: {error}",
                    path.display()
                ))
            }
            Err(ScopedContentFoldError::Changed) if strict => {
                return Err(format!(
                    "File size changed while hashing {}",
                    path.display()
                ))
            }
            Ok(_) if strict => return Err(format!("Could not read {}", path.display())),
            Ok(_) | Err(_) => {
                if let Some(issues) = issues.as_deref_mut() {
                    read_issue(
                        issues,
                        DiscoveryReadIssueKind::Resource,
                        path,
                        "Resource changed or could not be read completely while hashing",
                    );
                }
            }
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A bounded cache fingerprint built from each resource's metadata and the
/// freshly read bounded SKILL.md bytes. It detects document-byte changes even
/// when size and mtime are restored. Resource bytes are not reread on a cache
/// hit, so resource changes that preserve metadata remain outside this cache
/// freshness check.
fn folder_fingerprint(walk: &FolderWalk, skill_md_bytes: &[u8], has_spec: bool) -> u64 {
    let mut files: Vec<&HashableFile> = walk.hashable.iter().collect();
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.rel_path.hash(&mut hasher);
        file.len.hash(&mut hasher);
        file.mtime
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .unwrap_or(Duration::ZERO)
            .hash(&mut hasher);
    }
    walk.file_count.hash(&mut hasher);
    walk.total_bytes.hash(&mut hasher);
    walk.truncated.hash(&mut hasher);
    skill_md_bytes.hash(&mut hasher);
    has_spec.hash(&mut hasher);
    hasher.finish()
}

/// Token count of `text`, cl100k_base. `None` tokenizer (the build failed,
/// which should never happen since the vocab is embedded) yields 0.
fn count_tokens(text: &str, tokenizer: Option<&CoreBPE>) -> u32 {
    tokenizer
        .map(|bpe| bpe.encode_with_special_tokens(text).len() as u32)
        .unwrap_or(0)
}

/// Every fact about a skill's content that only depends on its canonical
/// directory, not on which root or symlink found it. Computed once per
/// canonical skill dir per scan and shared across every candidate that
/// resolves to it (see `discover_skill_candidates`'s facts cache). Deliberately
/// holds no per-deployment fact: name-vs-directory spec validation depends on
/// the lexical name a symlink alias was found under, not the canonical
/// target, so it's computed per candidate in `build_candidate` instead.
struct SkillContentFacts {
    incomplete: bool,
    frontmatter_parse_result: FrontmatterParseResult,
    frontmatter_fields: BTreeMap<String, String>,
    has_spec: bool,
    folder_bytes: u64,
    file_count: u32,
    skill_md_tokens: u32,
    /// Token count of just `"{name}: {description}"` - the prompt cost the
    /// model actually pays per turn, as opposed to `skill_md_tokens` which
    /// counts the whole file.
    description_tokens: u32,
    skill_md_line_count: usize,
    content_hash: String,
    modified_at: Option<String>,
    folder_truncated: bool,
}

/// Read SKILL.md through a bounded reader (never buffering more than
/// `SKILL_MD_MAX_BYTES` in memory) and walk `skill_dir`, everything
/// `compute_content_facts_from_walk` needs but stops short of the expensive
/// steps (hashing every file, tokenizing). `None` when SKILL.md can't be
/// opened. The returned `FolderWalk` also feeds `folder_fingerprint`, so a
/// cache hit in `get_or_compute_facts` never has to go further than this.
#[derive(Debug)]
enum FactsWalkError {
    MissingDocument,
    Failed(DiscoveryReadIssue),
}

struct PreparedSkillContent {
    document: ScopedContentObservation,
    walk: FolderWalk,
}

impl PreparedSkillContent {
    #[cfg(test)]
    fn enumerate(scope: &SkillReadScope, skill_dir: &Path) -> Result<Self, FactsWalkError> {
        let document = Self::observe_document(scope, skill_dir)?;
        Self::with_document(scope, skill_dir, document)
    }

    fn observe_document(
        scope: &SkillReadScope,
        skill_dir: &Path,
    ) -> Result<ScopedContentObservation, FactsWalkError> {
        let document_path = skill_dir.join("SKILL.md");
        scope
            .observe_content_regular(&document_path)
            .map_err(|error| {
                if matches!(error, ScopedReadError::Missing { .. }) {
                    FactsWalkError::MissingDocument
                } else {
                    FactsWalkError::Failed(DiscoveryReadIssue::new(
                        DiscoveryReadIssueKind::SkillDocument,
                        &document_path,
                        error.to_string(),
                    ))
                }
            })
    }

    #[cfg(test)]
    fn with_document(
        scope: &SkillReadScope,
        skill_dir: &Path,
        document: ScopedContentObservation,
    ) -> Result<Self, FactsWalkError> {
        // The prefix and the later hash both read document bytes from this budget.
        let prefix_bytes = document.len().min(SKILL_MD_MAX_BYTES);
        let walk = walk_folder_capped(
            scope,
            skill_dir,
            MAX_FOLDER_FILES,
            MAX_FOLDER_BYTES.saturating_sub(prefix_bytes),
        )
        .map_err(|error| {
            FactsWalkError::Failed(DiscoveryReadIssue::new(
                DiscoveryReadIssueKind::Resource,
                skill_dir,
                error,
            ))
        })?;
        Ok(Self { document, walk })
    }

    fn materialize_prefix(
        self,
        reader: SkillContentRead<'_>,
    ) -> Result<(FolderWalk, Vec<u8>, bool), FactsWalkError> {
        let prefix = reader
            .prefix(&self.document, SKILL_MD_MAX_BYTES as usize)
            .map_err(|error| {
                let message = match error {
                    ScopedContentFoldError::Read(error) => error.to_string(),
                    ScopedContentFoldError::Changed => {
                        "Skill document changed after enumeration".to_string()
                    }
                    ScopedContentFoldError::Cancelled(message) => message,
                };
                FactsWalkError::Failed(DiscoveryReadIssue::new(
                    DiscoveryReadIssueKind::SkillDocument,
                    &self.document.requested,
                    message,
                ))
            })?;
        Ok((self.walk, prefix.bytes, prefix.truncated))
    }
}

#[cfg(test)]
fn walk_for_facts(
    scope: &SkillReadScope,
    skill_dir: &Path,
) -> Result<(FolderWalk, Vec<u8>, bool), FactsWalkError> {
    PreparedSkillContent::enumerate(scope, skill_dir)?.materialize_prefix(scope.into())
}

/// Recompute the complete regular-file digest used by mutation guards.
///
/// The digest covers sorted relative regular-file paths and bytes. Resource
/// symlinks remain deliberately excluded and are never followed, matching
/// discovery and independent-copy behavior. This is not a filesystem-wide
/// identity or an atomic snapshot guarantee.
/// The byte budget includes the preliminary document read and the folder
/// pass, so a tree below 64 MiB can still exceed the combined work budget.
pub fn live_skill_content_hash(scope: &SkillReadScope, skill_dir: &Path) -> Result<String, String> {
    live_skill_content_hash_with_check(scope, skill_dir, || Ok(()))
}

/// Recompute the bounded strong hash while checking one Add operation during
/// directory traversal and each streamed file chunk.
pub fn live_skill_content_hash_with_check(
    scope: &SkillReadScope,
    skill_dir: &Path,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<String, String> {
    check()?;
    let prefix = scope
        .read_prefix_checked(
            &skill_dir.join("SKILL.md"),
            SKILL_MD_MAX_BYTES as usize,
            &mut check,
        )
        .map_err(|error| match error {
            ScopedContentFoldError::Cancelled(error) => error,
            ScopedContentFoldError::Read(error) => {
                format!("Could not read {}/SKILL.md: {error}", skill_dir.display())
            }
            ScopedContentFoldError::Changed => {
                format!("Could not read {}/SKILL.md", skill_dir.display())
            }
        })?;
    let skill_md_bytes = prefix.bytes;
    if prefix.truncated {
        return Err(format!(
            "Skill content exceeds the hash limit in {}",
            skill_dir.display()
        ));
    }
    let mut walk = walk_folder_capped_with_check(
        scope,
        skill_dir,
        MAX_FOLDER_FILES,
        MAX_FOLDER_BYTES
            .saturating_sub(skill_md_bytes.len().min(SKILL_MD_MAX_BYTES as usize) as u64),
        &mut check,
    )?;
    if walk.truncated {
        return Err(format!(
            "Skill content exceeds the hash limit in {}",
            skill_dir.display()
        ));
    }
    if let Some(reason) = walk.incomplete_reason {
        return Err(reason);
    }
    content_hash_with_check_mode(
        scope.into(),
        &mut walk.hashable,
        MAX_FOLDER_BYTES,
        &mut check,
        true,
        None,
    )
}

/// Complete regular-file membership used to plan a copy repair before its lease
/// is finalized. Resource symlinks are excluded, as in discovery's folder hash.
pub struct PreparedCopyRepairContent {
    walk: FolderWalk,
    document: PathBuf,
}

impl PreparedCopyRepairContent {
    pub fn enumerate(scope: &SkillReadScope, skill_dir: &Path) -> Result<Self, String> {
        Self::enumerate_cancellable(
            scope,
            skill_dir,
            &crate::skill_coordination::CancellationToken::default(),
        )
    }

    pub fn enumerate_cancellable(
        scope: &SkillReadScope,
        skill_dir: &Path,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<Self, String> {
        Self::enumerate_controlled(scope, skill_dir, cancellation)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn enumerate_controlled(
        scope: &SkillReadScope,
        skill_dir: &Path,
        cancellation: &crate::skill_coordination::CancellationToken,
    ) -> Result<Self, PreparedContentError> {
        let mut check = || {
            if cancellation.is_cancelled() {
                Err(PreparedContentError::from(
                    crate::skill_coordination::CoordinationFailure::Cancelled,
                ))
            } else {
                Ok(())
            }
        };
        check()?;
        let walk = walk_folder_capped_with_check(
            scope,
            skill_dir,
            MAX_FOLDER_FILES,
            MAX_FOLDER_BYTES,
            &mut check,
        )?;
        if walk.truncated || walk.incomplete_reason.is_some() {
            return Err("Copy repair requires complete folder membership".into());
        }
        let document = skill_dir.join("SKILL.md");
        if !walk
            .hashable
            .iter()
            .any(|file| file.observation.requested == document)
        {
            return Err("Copy repair requires a regular SKILL.md".into());
        }
        Ok(Self { walk, document })
    }

    pub fn files(&self) -> Vec<PathBuf> {
        self.walk
            .hashable
            .iter()
            .map(|file| file.observation.requested.clone())
            .collect()
    }

    pub fn hashes(
        &self,
        scope: &SkillReadScope,
        lease: &crate::skill_coordination::FinalizedWriteLease<'_>,
        expected_document: &[u8],
        proposed_document: &[u8],
    ) -> Result<(String, String), String> {
        self.hashes_with_document_limit(
            scope,
            lease,
            expected_document,
            proposed_document,
            crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES,
        )
        .map_err(|error| error.to_string())
    }

    pub(crate) fn hashes_with_document_limit(
        &self,
        scope: &SkillReadScope,
        lease: &crate::skill_coordination::FinalizedWriteLease<'_>,
        expected_document: &[u8],
        proposed_document: &[u8],
        document_limit: usize,
    ) -> Result<(String, String), PreparedContentError> {
        self.hashes_with_document_and_sidecar_limit(
            scope,
            lease,
            (expected_document, proposed_document),
            None,
            document_limit,
        )
    }

    pub(crate) fn hashes_with_document_and_sidecar_limit(
        &self,
        scope: &SkillReadScope,
        lease: &crate::skill_coordination::FinalizedWriteLease<'_>,
        document: (&[u8], &[u8]),
        sidecar: Option<&crate::skill_invocation_edit::CodexInvocationEdit>,
        document_limit: usize,
    ) -> Result<(String, String), PreparedContentError> {
        let (expected_document, proposed_document) = document;
        let revalidate = || -> Result<(), PreparedContentError> {
            lease.revalidate().map_err(PreparedContentError::from)?;
            if !self
                .walk
                .directories
                .iter()
                .all(|directory| directory.revalidate(scope))
            {
                return Err("Copy repair folder membership changed".into());
            }
            if sidecar.is_some_and(|change| change.original().is_none()) {
                let parent = self
                    .document
                    .parent()
                    .ok_or("Copy document has no parent")?;
                match scope.resolved_path_metadata(&parent.join("agents/openai.yaml")) {
                    Err(crate::skill_scope::ScopedReadError::Missing { .. }) => {}
                    Err(error) => return Err(error.to_string().into()),
                    Ok(_) => return Err("Copy invocation sidecar unexpectedly exists".into()),
                }
            }
            Ok(())
        };
        revalidate()?;
        if expected_document.len() > document_limit || proposed_document.len() > document_limit {
            return Err("Copy repair document exceeds its limit".into());
        }
        let current = lease
            .read(&self.document, document_limit)
            .map_err(PreparedContentError::from)?;
        if current != expected_document {
            return Err("Copy repair document changed".into());
        }
        let sidecar_relative = PathBuf::from("agents/openai.yaml");
        let mut files: Vec<_> = self
            .walk
            .hashable
            .iter()
            .map(|file| (file.rel_path.clone(), Some(file)))
            .collect();
        if let Some(change) = sidecar {
            if change
                .original()
                .is_some_and(|bytes| bytes.len() > document_limit)
                || change
                    .proposed()
                    .is_some_and(|bytes| bytes.len() > document_limit)
            {
                return Err("Copy invocation sidecar exceeds its limit".into());
            }
            let present = files.iter().any(|(path, _)| path == &sidecar_relative);
            if present != change.original().is_some() {
                return Err("Copy invocation sidecar presence changed".into());
            }
            if !present {
                if change.proposed().is_some() && files.len() >= MAX_FOLDER_FILES {
                    return Err("Copy invocation exceeds the folder file limit".into());
                }
                files.push((sidecar_relative.clone(), None));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut original = Sha256::new();
        let mut proposed = Sha256::new();
        let mut total = 0u64;
        let hash_file = |hasher: &mut Sha256, path: &Path, bytes: &[u8]| {
            let relative = path.to_string_lossy();
            hasher.update((relative.len() as u64).to_le_bytes());
            hasher.update(relative.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        for (relative_path, file) in files {
            revalidate()?;
            if let Some(change) = sidecar.filter(|_| relative_path == sidecar_relative) {
                let before = change.original().map(str::as_bytes);
                let after = change.proposed().map(str::as_bytes);
                let planned_bytes = before
                    .map_or(0, <[u8]>::len)
                    .max(after.map_or(0, <[u8]>::len)) as u64;
                if planned_bytes > MAX_FOLDER_BYTES.saturating_sub(total) {
                    return Err("Copy repair folder exceeds its limit".into());
                }
                if let Some(file) = file {
                    let expected = before.ok_or("Copy invocation sidecar unexpectedly exists")?;
                    let mut observed = Vec::new();
                    lease.fold_resource(
                        &file.observation.requested,
                        document_limit as u64,
                        &mut |bytes| observed.extend_from_slice(bytes),
                    )?;
                    if observed != expected || file.len != expected.len() as u64 {
                        return Err("Copy invocation sidecar changed".into());
                    }
                }
                if let Some(bytes) = before {
                    hash_file(&mut original, &relative_path, bytes);
                }
                if let Some(bytes) = after {
                    hash_file(&mut proposed, &relative_path, bytes);
                }
                total += planned_bytes;
                continue;
            }
            let file = file.ok_or("Copy folder observation is absent")?;
            let planned_bytes = if file.observation.requested == self.document {
                file.len.max(proposed_document.len() as u64)
            } else {
                file.len
            };
            if planned_bytes > MAX_FOLDER_BYTES.saturating_sub(total) {
                return Err("Copy repair folder exceeds its limit".into());
            }
            let relative = relative_path.to_string_lossy();
            for hasher in [&mut original, &mut proposed] {
                hasher.update((relative.len() as u64).to_le_bytes());
                hasher.update(relative.as_bytes());
            }
            original.update(file.len.to_le_bytes());
            if file.observation.requested == self.document {
                if file.len != expected_document.len() as u64 {
                    return Err("Copy repair document length changed".into());
                }
                original.update(expected_document);
                proposed.update((proposed_document.len() as u64).to_le_bytes());
                proposed.update(proposed_document);
            } else {
                proposed.update(file.len.to_le_bytes());
                let count =
                    lease.fold_resource(&file.observation.requested, file.len, &mut |bytes| {
                        original.update(bytes);
                        proposed.update(bytes);
                    })?;
                if count != file.len {
                    return Err("Copy repair resource changed".into());
                }
            }
            total += planned_bytes;
        }
        revalidate()?;
        let encode = |hash: Sha256| {
            hash.finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        Ok((encode(original), encode(proposed)))
    }
}

/// The expensive half of `compute_content_facts`: hashing every file in
/// `walk` and tokenizing SKILL.md. Only run on a `get_or_compute_facts` cache
/// miss - `walk_for_facts` already read the bounded document and gathered
/// resource metadata to produce `walk`.
fn compute_content_facts_from_walk(
    reader: SkillContentRead<'_>,
    mut walk: FolderWalk,
    skill_md_bytes: &[u8],
    skill_md_truncated: bool,
    has_spec: bool,
    tokenizer: Option<&CoreBPE>,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> SkillContentFacts {
    let content = String::from_utf8_lossy(skill_md_bytes).into_owned();
    let frontmatter_parse_result = parse_frontmatter(&content);
    let name_for_tokens = frontmatter_parse_result
        .as_frontmatter()
        .and_then(|f| f.name.clone())
        .unwrap_or_default();
    let description_for_tokens = frontmatter_parse_result
        .as_frontmatter()
        .and_then(|f| f.description.clone())
        .unwrap_or_default();
    let modified_at = walk.newest.map(|t| DateTime::<Utc>::from(t).to_rfc3339());

    SkillContentFacts {
        incomplete: false,
        frontmatter_fields: frontmatter_fields(&content),
        frontmatter_parse_result,
        has_spec,
        folder_bytes: walk.total_bytes,
        file_count: walk.file_count,
        skill_md_tokens: count_tokens(&content, tokenizer),
        description_tokens: count_tokens(
            &format!("{name_for_tokens}: {description_for_tokens}"),
            tokenizer,
        ),
        skill_md_line_count: content.lines().count(),
        content_hash: content_hash_with_check_mode(
            reader,
            &mut walk.hashable,
            MAX_FOLDER_BYTES,
            &mut || Ok(()),
            false,
            Some(issues),
        )
        .expect("the no-op content-hash check cannot fail"),
        modified_at,
        folder_truncated: walk.truncated || skill_md_truncated,
    }
}

/// Read SKILL.md and walk `skill_dir`, gathering every content fact in one
/// call - the uncached path, kept for the one test that exercises it
/// directly. `discover_skill_candidates_cached` instead goes through
/// `walk_for_facts` + `get_or_compute_facts` so a folder whose fingerprint
/// hasn't changed since the last rebuild skips straight to a cache hit.
#[cfg(test)]
fn compute_content_facts(
    skill_dir: &Path,
    tokenizer: Option<&CoreBPE>,
) -> Option<SkillContentFacts> {
    let scope = SkillReadScope::bind(&[skill_dir.to_path_buf()]).ok()?;
    let (walk, skill_md_bytes, skill_md_truncated) = walk_for_facts(&scope, skill_dir).ok()?;
    let mut issues = Vec::new();
    let has_spec = has_spec(&scope, skill_dir, &mut issues);
    Some(compute_content_facts_from_walk(
        (&scope).into(),
        walk,
        &skill_md_bytes,
        skill_md_truncated,
        has_spec,
        tokenizer,
        &mut issues,
    ))
}

/// One `SkillContentFacts` cache entry: the bounded document and resource
/// metadata fingerprint it was computed for, and the generation (see `SkillFactsCache::begin_pass`) it
/// was last confirmed valid in.
struct CacheEntry {
    fingerprint: u64,
    facts: Arc<SkillContentFacts>,
    last_seen: u64,
}

struct LexicalCacheEntry {
    physical: PathBuf,
    last_seen: u64,
}

/// A `SkillContentFacts` cache that survives across rebuilds, keyed by
/// canonical skill directory and validated by `folder_fingerprint` rather
/// than by never expiring.
/// `begin_pass`/`end_pass` bracket one `discover_skill_candidates_cached`
/// call so an entry not touched during a pass - the skill directory it
/// belonged to was removed - is evicted rather than pinning memory forever.
#[derive(Default)]
pub struct SkillFactsCache {
    entries: HashMap<PathBuf, CacheEntry>,
    lexical_keys: HashMap<PathBuf, LexicalCacheEntry>,
    generation: u64,
    hits: u64,
    total: u64,
}

impl SkillFactsCache {
    /// Start a new pass: reset the hit/total counters `last_pass_stats`
    /// reports, and advance the generation so `end_pass` can tell which
    /// entries were touched this time.
    pub(crate) fn begin_pass(&mut self) {
        self.generation += 1;
        self.hits = 0;
        self.total = 0;
    }

    /// Drop every entry not looked up during the pass just finished - its
    /// skill directory no longer exists (or was never visited), so keeping
    /// its facts around would only pin memory for a skill that's gone.
    pub(crate) fn end_pass(&mut self) {
        let generation = self.generation;
        self.entries
            .retain(|_, entry| entry.last_seen == generation);
        self.lexical_keys.retain(|_, entry| {
            entry.last_seen == generation && self.entries.contains_key(&entry.physical)
        });
    }

    pub(crate) fn end_named_pass(&mut self, names: &std::collections::BTreeSet<String>) {
        let generation = self.generation;
        let stale = self
            .lexical_keys
            .iter()
            .filter(|(lexical, entry)| {
                lexical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| names.contains(name))
                    && entry.last_seen != generation
            })
            .map(|(lexical, entry)| (lexical.clone(), entry.physical.clone()))
            .collect::<Vec<_>>();
        for (lexical, physical) in stale {
            self.lexical_keys.remove(&lexical);
            self.entries.remove(&physical);
        }
    }

    fn evict_lexical(&mut self, lexical: &Path) {
        if let Some(previous) = self.lexical_keys.remove(lexical) {
            self.entries.remove(&previous.physical);
        }
    }

    fn record_lexical(&mut self, lexical: &Path, physical: &Path) {
        if let Some(previous) = self.lexical_keys.insert(
            lexical.to_path_buf(),
            LexicalCacheEntry {
                physical: physical.to_path_buf(),
                last_seen: self.generation,
            },
        ) {
            if previous.physical != physical {
                self.entries.remove(&previous.physical);
            }
        }
    }

    /// `(hits, total)` facts lookups from the most recently completed
    /// `discover_skill_candidates_cached` pass - see `skill_refresh`'s
    /// per-rebuild timing line.
    pub fn last_pass_stats(&self) -> (u64, u64) {
        (self.hits, self.total)
    }
}

/// Look up `skill_dir`'s content facts in `cache`, computing and inserting
/// them on a miss. Cache key is the canonicalized directory, so every root
/// or symlink that resolves to the same directory shares one computation.
/// A hit still walks the folder (via `walk_for_facts`) to get a fresh
/// fingerprint to compare against - cheap, since it only stats files rather
/// than reading or hashing them - but skips `compute_content_facts_from_walk`
/// entirely when the fingerprint hasn't changed since it was cached.
#[cfg(test)]
fn get_or_compute_facts(
    scope: &SkillReadScope,
    cache: &mut SkillFactsCache,
    skill_dir: &Path,
    tokenizer: Option<&CoreBPE>,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> Result<Option<Arc<SkillContentFacts>>, ()> {
    PreparedSkillFacts::enumerate(scope, skill_dir).materialize(
        scope.into(),
        cache,
        skill_dir,
        tokenizer,
        issues,
    )
}

enum PreparedSkillFacts {
    Unavailable(ScopedReadError),
    Ready {
        key: PathBuf,
        content: Box<Result<PreparedSkillContent, FactsWalkError>>,
        spec: Arc<PreparedSpecMarkers>,
    },
}

impl PreparedSkillFacts {
    fn append_regular_files(&self, files: &mut BTreeSet<PathBuf>) {
        if let Self::Ready { content, spec, .. } = self {
            if let Ok(content) = content.as_ref() {
                files.insert(content.document.requested.clone());
                files.extend(
                    content
                        .walk
                        .hashable
                        .iter()
                        .map(|file| file.observation.requested.clone()),
                );
            }
            if let Ok(file) = &spec.file {
                files.insert(file.requested.clone());
            }
        }
    }

    #[cfg(test)]
    fn enumerate(scope: &SkillReadScope, skill_dir: &Path) -> Self {
        Self::enumerate_checked(scope, skill_dir, &mut || Ok(()))
            .expect("unguarded facts preparation cannot be cancelled")
    }

    fn enumerate_checked(
        scope: &SkillReadScope,
        skill_dir: &Path,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let key = match scope.resolved_dir_path(skill_dir) {
            Ok(key) => Ok(key),
            Err(error) => {
                check()?;
                return Ok(Self::Unavailable(error));
            }
        };
        check()?;
        let document = PreparedSkillContent::observe_document(scope, skill_dir);
        Self::from_document_checked(scope, skill_dir, key, document, check)
    }

    fn enumerate_plugin_checked(
        scope: &SkillReadScope,
        plugin: &plugins::PluginSkillDir,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let key = scope.resolve_observed_dir(&plugin.entry_observation);
        Self::from_document_checked(
            scope,
            &plugin.skill_dir,
            key,
            Ok(plugin.document_observation.clone()),
            check,
        )
    }

    fn from_document_checked(
        scope: &SkillReadScope,
        skill_dir: &Path,
        key: Result<PathBuf, ScopedReadError>,
        document: Result<ScopedContentObservation, FactsWalkError>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let key = match key {
            Ok(key) => key,
            Err(error) => return Ok(Self::Unavailable(error)),
        };
        let content = match document {
            Ok(document) => {
                let prefix_bytes = document.len().min(SKILL_MD_MAX_BYTES);
                let walk = walk_folder_capped_with_check(
                    scope,
                    skill_dir,
                    MAX_FOLDER_FILES,
                    MAX_FOLDER_BYTES.saturating_sub(prefix_bytes),
                    check,
                )?;
                Ok(PreparedSkillContent { document, walk })
            }
            Err(error) => Err(error),
        };
        check()?;
        let spec = Arc::new(PreparedSpecMarkers::enumerate(scope, skill_dir));
        check()?;
        Ok(Self::Ready {
            key,
            content: Box::new(content),
            spec,
        })
    }

    fn materialize(
        self,
        reader: SkillContentRead<'_>,
        cache: &mut SkillFactsCache,
        skill_dir: &Path,
        tokenizer: Option<&CoreBPE>,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> Result<Option<Arc<SkillContentFacts>>, ()> {
        let (key, content, spec) = match self {
            Self::Ready { key, content, spec } => (key, content, spec),
            Self::Unavailable(error) => {
                cache.evict_lexical(skill_dir);
                issues.push(DiscoveryReadIssue::new(
                    DiscoveryReadIssueKind::Resource,
                    skill_dir,
                    error.to_string(),
                ));
                return Err(());
            }
        };
        cache.record_lexical(skill_dir, &key);
        let (mut walk, skill_md_bytes, skill_md_truncated) =
            match (*content).and_then(|content| content.materialize_prefix(reader)) {
                Ok(value) => value,
                Err(FactsWalkError::MissingDocument) => {
                    cache.entries.remove(&key);
                    return Ok(None);
                }
                Err(FactsWalkError::Failed(issue)) => {
                    cache.entries.remove(&key);
                    issues.push(issue);
                    return Err(());
                }
            };
        if !walk
            .directories
            .iter()
            .all(|directory| directory.revalidate(reader.scope))
        {
            walk.incomplete_reason
                .get_or_insert_with(|| "Resource directory changed after enumeration".to_string());
        }
        if let Some(guard) = reader.guard {
            for file in &walk.hashable {
                if let Err(error) = guard.check_content_observation(&file.observation) {
                    cache.entries.remove(&key);
                    read_issue(
                        issues,
                        DiscoveryReadIssueKind::Resource,
                        &file.observation.requested,
                        format!("Resource does not match the coordinated read plan: {error:?}"),
                    );
                    return Err(());
                }
            }
        }
        let issue_count_before_spec = issues.len();
        if tokenizer.is_none() {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Tokenizer,
                skill_dir,
                "Tokenizer is unavailable",
            );
        }
        let has_spec = spec.materialize(reader.scope, skill_dir, issues);
        let incomplete = walk.truncated
            || skill_md_truncated
            || walk.incomplete_reason.is_some()
            || tokenizer.is_none()
            || issues.len() != issue_count_before_spec;
        if walk.truncated || skill_md_truncated {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Cap,
                skill_dir,
                "Skill content exceeds the discovery limit",
            );
        }
        if let Some(reason) = &walk.incomplete_reason {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Resource,
                skill_dir,
                reason.clone(),
            );
        }
        let fingerprint = folder_fingerprint(&walk, &skill_md_bytes, has_spec);
        let generation = cache.generation;
        cache.total += 1;
        if !incomplete {
            if let Some(entry) = cache.entries.get_mut(&key) {
                if entry.fingerprint == fingerprint {
                    entry.last_seen = generation;
                    cache.hits += 1;
                    return Ok(Some(Arc::clone(&entry.facts)));
                }
            }
        } else {
            cache.entries.remove(&key);
        }
        let issue_count = issues.len();
        let mut facts = compute_content_facts_from_walk(
            reader,
            walk,
            &skill_md_bytes,
            skill_md_truncated,
            has_spec,
            tokenizer,
            issues,
        );
        let incomplete = incomplete || issues.len() != issue_count;
        facts.incomplete = incomplete;
        let facts = Arc::new(facts);
        if !incomplete {
            cache.entries.insert(
                key.clone(),
                CacheEntry {
                    fingerprint,
                    facts: Arc::clone(&facts),
                    last_seen: generation,
                },
            );
        } else {
            cache.entries.remove(&key);
        }
        Ok(Some(facts))
    }
}

/// Build a candidate for a resolved skill directory (the SKILL.md-bearing
/// directory itself - for symlinks, this is the canonicalized target) from
/// its already-computed content facts.
#[allow(clippy::too_many_arguments)]
fn build_candidate(
    read_context: &plugins::SkillDiscoveryReadContext,
    plugin: PluginEvidence,
    entry_path: &Path,
    skill_dir: &Path,
    root: &agents::SkillRoot,
    is_symlink: bool,
    symlink_target: Option<PathBuf>,
    symlink_error: Option<String>,
    scope: &str,
    facts: &SkillContentFacts,
    git_cache: &mut HashMap<PathBuf, GitRepoEvidence>,
    studio_disabled: bool,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> SkillCandidate {
    // The lexical name this deployment was found under (e.g. a symlink
    // alias's own name), not the canonical target's directory name: it's
    // what a user sees in the agent root, so both the fallback display name
    // and the name-vs-directory spec check use it.
    let dir_name = entry_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let spec_violations = validate_skill(
        &dir_name,
        &facts.frontmatter_parse_result,
        facts.skill_md_line_count,
    );
    // A symlinked entry's canonical target is already carried by
    // `symlink_target`; `resolved_path` only covers the case where
    // `entry_path` itself isn't a symlink but an ancestor is (e.g. a whole
    // `.claude/skills` root linked to `.agents/skills`), where `skill_dir`
    // already holds the canonical directory.
    let resolved_path = if is_symlink {
        Some(skill_dir.to_path_buf())
    } else {
        read_context
            .read_scope()
            .resolved_dir_path(skill_dir)
            .ok()
            .filter(|c| c != entry_path)
    };
    let scope_anchor = root.project_path.as_deref().unwrap_or(read_context.home());
    let shared_via_whole_dir_link = !is_symlink
        && resolved_path.is_some()
        && entry_path.parent().is_some_and(|parent| {
            let canonical = read_context
                .read_scope()
                .resolved_dir_path(parent)
                .unwrap_or_else(|_| parent.to_path_buf());
            // A declared home/project alias does not link one harness's skills
            // to another. Links below that boundary still affect ownership.
            let unchanged_below_anchor =
                parent.strip_prefix(scope_anchor).ok().is_some_and(|rest| {
                    read_context
                        .read_scope()
                        .resolved_dir_path(scope_anchor)
                        .is_ok_and(|anchor| anchor.join(rest) == canonical)
                });
            let components: Vec<_> = canonical
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            !unchanged_below_anchor && components.windows(2).any(|w| w == [".agents", "skills"])
        });

    SkillCandidate {
        // Lifecycle identity is the entry name. Frontmatter name remains in
        // `frontmatter` for display and diagnostics, but cannot claim another
        // folder's ledger or CLI lifecycle operations.
        name: dir_name,
        path: entry_path.to_path_buf(),
        root_label: root.label.clone(),
        scope: scope.to_string(),
        project_path: root.project_path.clone(),
        is_symlink,
        symlink_target,
        resolved_path,
        symlink_is_broken: false,
        symlink_error,
        plugin,
        frontmatter_fields: facts.frontmatter_fields.clone(),
        frontmatter: facts.frontmatter_parse_result.as_frontmatter().cloned(),
        spec_violations,
        has_spec: facts.has_spec,
        folder_bytes: facts.folder_bytes,
        file_count: facts.file_count,
        skill_md_tokens: facts.skill_md_tokens,
        description_tokens: facts.description_tokens,
        content_hash: facts.content_hash.clone(),
        modified_at: facts.modified_at.clone(),
        folder_truncated: facts.folder_truncated,
        git_repo: git_repo_evidence(read_context.read_scope(), skill_dir, git_cache, issues),
        studio_disabled,
        shared_via_whole_dir_link,
    }
}

/// A symlink: known by name but its target doesn't resolve (or resolving it
/// failed), so no content facts can be gathered. `symlink_target` is the raw
/// `fs::read_link` target (resolved relative to the link's parent when
/// relative), kept even though it doesn't canonicalize, so provenance can
/// still pattern-match it (see `provenance::resolves_into_dotagents`).
#[allow(clippy::too_many_arguments)]
fn broken_symlink_candidate(
    plugin: PluginEvidence,
    entry_path: &Path,
    root: &agents::SkillRoot,
    scope: &str,
    symlink_target: Option<PathBuf>,
    symlink_is_broken: bool,
    symlink_error: Option<String>,
    studio_disabled: bool,
) -> SkillCandidate {
    let name = entry_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    SkillCandidate {
        name,
        path: entry_path.to_path_buf(),
        root_label: root.label.clone(),
        scope: scope.to_string(),
        project_path: root.project_path.clone(),
        is_symlink: true,
        symlink_target,
        resolved_path: None,
        symlink_is_broken,
        symlink_error,
        plugin,
        frontmatter: None,
        frontmatter_fields: BTreeMap::new(),
        spec_violations: Vec::new(),
        has_spec: false,
        folder_bytes: 0,
        file_count: 0,
        skill_md_tokens: 0,
        description_tokens: 0,
        content_hash: String::new(),
        modified_at: None,
        folder_truncated: false,
        git_repo: GitRepoEvidence::Unknown,
        studio_disabled,
        shared_via_whole_dir_link: false,
    }
}

/// How a symlink's target resolved.
fn git_repo_evidence(
    scope: &SkillReadScope,
    dir: &Path,
    cache: &mut HashMap<PathBuf, GitRepoEvidence>,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> GitRepoEvidence {
    if let Some(hit) = cache.get(dir) {
        return *hit;
    }
    let marker = dir.join(".git");
    let result = match scope.resolved_path_metadata(&marker) {
        Ok(_) => GitRepoEvidence::Present,
        Err(ScopedReadError::Missing { .. }) => match dir.parent() {
            Some(parent) => match scope.resolved_dir_path(parent) {
                Ok(_) => git_repo_evidence(scope, parent, cache, issues),
                Err(error) => {
                    read_issue(
                        issues,
                        if scope.has_declared_prefix(parent) {
                            DiscoveryReadIssueKind::Metadata
                        } else {
                            DiscoveryReadIssueKind::GitScopeBoundary
                        },
                        parent,
                        format!("Git ancestry leaves the declared read scope: {error}"),
                    );
                    if scope.has_declared_prefix(parent) {
                        GitRepoEvidence::Unknown
                    } else {
                        GitRepoEvidence::Truncated
                    }
                }
            },
            None => GitRepoEvidence::Absent,
        },
        Err(error) => {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Metadata,
                &marker,
                error.to_string(),
            );
            GitRepoEvidence::Unknown
        }
    };
    cache.insert(dir.to_path_buf(), result);
    result
}

/// Name of the holding directory `skill_harness_disable`'s universal
/// move-aside disable renames a deployment into, sitting as a sibling of the
/// deployment inside its skills root. Skipped in the normal one-level pass
/// so it's never itself treated as a skill folder, then walked separately
/// (also one level deep) so its contents surface as disabled candidates.
pub const STUDIO_DISABLED_DIR_NAME: &str = ".skill-studio-disabled";

struct PreparedObservedEntry {
    plugin: Arc<plugins::PreparedPluginAncestry>,
    observation: Arc<crate::skill_scope::ScopedEntryObservation>,
    resolved: Result<(PathBuf, PreparedSkillFacts), ScopedReadError>,
}

impl PreparedObservedEntry {
    fn enumerate(
        context: &SkillDiscoveryReadContext,
        plugin_cache: &mut plugins::PluginObservationCache<'_>,
        harness: &str,
        path: &Path,
        observation: crate::skill_scope::ScopedEntryObservation,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let scope = context.read_scope();
        let resolved = match scope.resolve_observed_dir(&observation) {
            Ok(target) => Ok((
                target,
                PreparedSkillFacts::enumerate_checked(scope, path, check)?,
            )),
            Err(error) => Err(error),
        };
        let plugin = plugins::PreparedPluginAncestry::enumerate_cached(plugin_cache, path, harness);
        check()?;
        Ok(Self {
            plugin: Arc::new(plugin),
            observation: Arc::new(observation),
            resolved,
        })
    }
}

struct PreparedAgentRoot {
    root: agents::SkillRoot,
    path: PathBuf,
    disabled: bool,
    state: RootReadOutcome,
    observation: Option<crate::skill_scope::ScopedEntryObservation>,
    entries: Vec<(PathBuf, Result<PreparedObservedEntry, ScopedReadError>)>,
    issues: Vec<DiscoveryReadIssue>,
}

impl PreparedAgentRoot {
    fn enumerate(
        context: &SkillDiscoveryReadContext,
        plugin_cache: &mut plugins::PluginObservationCache<'_>,
        root: agents::SkillRoot,
        disabled: bool,
        names: Option<&BTreeSet<String>>,
    ) -> Self {
        Self::enumerate_checked(context, plugin_cache, root, disabled, names, &mut || Ok(()))
            .expect("unguarded enumeration cannot be cancelled")
    }

    fn enumerate_checked(
        context: &SkillDiscoveryReadContext,
        plugin_cache: &mut plugins::PluginObservationCache<'_>,
        root: agents::SkillRoot,
        disabled: bool,
        names: Option<&BTreeSet<String>>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let path = if disabled {
            root.path.join(STUDIO_DISABLED_DIR_NAME)
        } else {
            root.path.clone()
        };
        let mut issues = Vec::new();
        let (mut state, observation) = observe_named_root(context.read_scope(), &path, &mut issues);
        let mut entries = Vec::new();
        if state == RootReadOutcome::Read {
            if let Some(names) = names {
                for name in names {
                    check()?;
                    entries.push((
                        path.join(name),
                        context
                            .read_scope()
                            .observe_entry(&path, std::ffi::OsStr::new(name)),
                    ));
                }
            } else {
                match context.read_scope().read_dir(&path, MAX_FOLDER_ENTRIES) {
                    Ok(listing) => {
                        if !listing.issues.is_empty() {
                            state = RootReadOutcome::Incomplete;
                        }
                        for issue in listing.issues {
                            read_issue(
                                &mut issues,
                                DiscoveryReadIssueKind::Entry,
                                &path,
                                format!("{issue:?}"),
                            );
                        }
                        for entry in listing.entries {
                            check()?;
                            if !disabled && entry.name == STUDIO_DISABLED_DIR_NAME {
                                continue;
                            }
                            entries.push((path.join(&entry.name), Ok(entry.into_observation())));
                        }
                    }
                    Err(error) => {
                        state = RootReadOutcome::Incomplete;
                        read_issue(
                            &mut issues,
                            DiscoveryReadIssueKind::Root,
                            &path,
                            error.to_string(),
                        );
                    }
                }
            }
        }
        let entries = entries
            .into_iter()
            .map(|(path, observation)| {
                check()?;
                let prepared = match observation {
                    Ok(observation) => Ok(PreparedObservedEntry::enumerate(
                        context,
                        plugin_cache,
                        &root.label,
                        &path,
                        observation,
                        check,
                    )?),
                    Err(error) => Err(error),
                };
                check()?;
                Ok((path, prepared))
            })
            .collect::<Result<_, crate::skill_coordination::CoordinationFailure>>()?;
        check()?;
        Ok(Self {
            root,
            path,
            disabled,
            state,
            observation,
            entries,
            issues,
        })
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn materialize(
        self,
        context: &SkillDiscoveryReadContext,
        cache: &mut SkillFactsCache,
        git_cache: &mut HashMap<PathBuf, GitRepoEvidence>,
        tokenizer: Option<&CoreBPE>,
        out: &mut Vec<SkillCandidate>,
        issues: &mut Vec<DiscoveryReadIssue>,
    ) -> RootReadOutcome {
        self.materialize_guarded(
            context,
            None,
            cache,
            git_cache,
            tokenizer,
            out,
            issues,
            &mut || Ok(()),
            &mut plugins::ManifestReadPass::new(context.read_scope(), None),
            &mut plugins::ManifestValidationPass::new(context.read_scope()),
        )
        .expect("unguarded root materialization cannot be cancelled")
        .0
    }

    #[allow(clippy::too_many_arguments)]
    fn materialize_guarded(
        self,
        context: &SkillDiscoveryReadContext,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        cache: &mut SkillFactsCache,
        git_cache: &mut HashMap<PathBuf, GitRepoEvidence>,
        tokenizer: Option<&CoreBPE>,
        out: &mut Vec<SkillCandidate>,
        issues: &mut Vec<DiscoveryReadIssue>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
        manifest_reads: &mut plugins::ManifestReadPass<'_>,
        manifest_validation: &mut plugins::ManifestValidationPass<'_>,
    ) -> Result<(RootReadOutcome, AgentRootReadProof), crate::skill_coordination::CoordinationFailure>
    {
        check()?;
        issues.extend(self.issues);
        let mut state = self.state;
        let mut entry_proofs = Vec::new();
        for (path, observation) in self.entries {
            check()?;
            let proof = observation
                .as_ref()
                .ok()
                .map(|entry| AgentEntryReadProof::capture(&path, entry, context.read_scope()));
            if scan_root_entry(
                context,
                SkillContentRead {
                    scope: context.read_scope(),
                    guard,
                },
                &path,
                observation,
                &self.root,
                scope_str(&self.root),
                self.disabled,
                cache,
                git_cache,
                tokenizer,
                out,
                issues,
                manifest_reads,
            ) {
                state = RootReadOutcome::Incomplete;
            }
            check()?;
            if let Some(mut proof) = proof {
                proof.unchanged = proof.current(context.read_scope(), manifest_validation);
                if !proof.unchanged {
                    state = RootReadOutcome::Incomplete;
                }
                entry_proofs.push(proof);
            }
        }
        let mut unchanged = true;
        if self.state == RootReadOutcome::Absent {
            let (current, _) = observe_named_root(context.read_scope(), &self.path, issues);
            if current != RootReadOutcome::Absent {
                unchanged = false;
                state = RootReadOutcome::Incomplete;
                read_issue(
                    issues,
                    DiscoveryReadIssueKind::Root,
                    &self.path,
                    "Root appeared or became unavailable after enumeration",
                );
            }
        } else if !revalidate_named_root(
            context.read_scope(),
            &self.path,
            self.observation.as_ref(),
            issues,
        ) {
            unchanged = false;
            state = RootReadOutcome::Incomplete;
        }
        check()?;
        Ok((
            state,
            AgentRootReadProof {
                path: self.path,
                absent: self.state == RootReadOutcome::Absent,
                observation: self.observation,
                entries: entry_proofs,
                unchanged,
            },
        ))
    }
}

fn prepare_agent_roots(
    context: &SkillDiscoveryReadContext,
    names: Option<&BTreeSet<String>>,
) -> Vec<PreparedAgentRoot> {
    let mut plugin_cache = plugins::PluginObservationCache::new(context);
    agents::skill_roots(context.home(), context.projects())
        .into_iter()
        .flat_map(|root| {
            [
                PreparedAgentRoot::enumerate(
                    context,
                    &mut plugin_cache,
                    root.clone(),
                    false,
                    names,
                ),
                PreparedAgentRoot::enumerate(context, &mut plugin_cache, root, true, names),
            ]
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn scan_root_entry(
    read_context: &plugins::SkillDiscoveryReadContext,
    reader: SkillContentRead<'_>,
    entry_path: &Path,
    observed: Result<PreparedObservedEntry, ScopedReadError>,
    root: &agents::SkillRoot,
    scope: &str,
    studio_disabled: bool,
    facts_cache: &mut SkillFactsCache,
    git_cache: &mut HashMap<PathBuf, GitRepoEvidence>,
    tokenizer: Option<&CoreBPE>,
    out: &mut Vec<SkillCandidate>,
    issues: &mut Vec<DiscoveryReadIssue>,
    manifest_reads: &mut plugins::ManifestReadPass<'_>,
) -> bool {
    let prepared = match observed {
        Ok(value) => value,
        Err(ScopedReadError::Missing { .. }) => {
            facts_cache.evict_lexical(entry_path);
            return false;
        }
        Err(error) => {
            facts_cache.evict_lexical(entry_path);
            read_issue(
                issues,
                DiscoveryReadIssueKind::Metadata,
                entry_path,
                error.to_string(),
            );
            return true;
        }
    };
    let plugin = prepared.plugin;
    let observation = prepared.observation;
    let is_symlink = observation.metadata.file_type().is_symlink();
    let raw_target = match &observation.raw_link_target {
        Ok(target) => target.clone(),
        Err(error) => {
            facts_cache.evict_lexical(entry_path);
            read_issue(
                issues,
                DiscoveryReadIssueKind::Metadata,
                entry_path,
                error.to_string(),
            );
            if is_symlink {
                out.push(broken_symlink_candidate(
                    plugin.materialize_with_pass(manifest_reads, issues),
                    entry_path,
                    root,
                    scope,
                    None,
                    false,
                    Some(error.to_string()),
                    studio_disabled,
                ));
            }
            return true;
        }
    };
    let resolved = prepared.resolved.and_then(|(target, facts)| {
        let current = read_context
            .read_scope()
            .resolve_observed_dir(&observation)?;
        if current != target {
            return Err(ScopedReadError::Io(std::io::Error::other(
                "Skill target changed after enumeration",
            )));
        }
        Ok((target, facts))
    });

    if is_symlink {
        match resolved {
            Ok((target, prepared_facts)) => {
                match prepared_facts.materialize(reader, facts_cache, entry_path, tokenizer, issues)
                {
                    Ok(None) => {
                        out.push(broken_symlink_candidate(
                            plugin.materialize_with_pass(manifest_reads, issues),
                            entry_path,
                            root,
                            scope,
                            Some(target),
                            false,
                            Some("Skill document is missing".into()),
                            studio_disabled,
                        ));
                        return false;
                    }
                    Ok(Some(facts)) => {
                        out.push(build_candidate(
                            read_context,
                            plugin.materialize_with_pass(manifest_reads, issues),
                            entry_path,
                            &target,
                            root,
                            true,
                            Some(target.clone()),
                            None,
                            scope,
                            &facts,
                            git_cache,
                            studio_disabled,
                            issues,
                        ));
                        return false;
                    }
                    Err(()) => {
                        out.push(broken_symlink_candidate(
                            plugin.materialize_with_pass(manifest_reads, issues),
                            entry_path,
                            root,
                            scope,
                            Some(target),
                            false,
                            Some(
                                "Skill content is outside the declared read scope or unavailable"
                                    .to_string(),
                            ),
                            studio_disabled,
                        ));
                    }
                }
            }
            Err(ScopedReadError::Missing { .. }) => {
                facts_cache.evict_lexical(entry_path);
                out.push(broken_symlink_candidate(
                    plugin.materialize_with_pass(manifest_reads, issues),
                    entry_path,
                    root,
                    scope,
                    None,
                    true,
                    None,
                    studio_disabled,
                ));
            }
            Err(error) => {
                let message = error.to_string();
                let missing_target = matches!(
                    &error,
                    ScopedReadError::LinkTarget { source, .. }
                        if source.kind() == std::io::ErrorKind::NotFound
                );
                facts_cache.evict_lexical(entry_path);
                read_issue(
                    issues,
                    DiscoveryReadIssueKind::Metadata,
                    entry_path,
                    message.clone(),
                );
                out.push(broken_symlink_candidate(
                    plugin.materialize_with_pass(manifest_reads, issues),
                    entry_path,
                    root,
                    scope,
                    raw_target.map(|target| {
                        if target.is_absolute() {
                            target
                        } else {
                            entry_path.parent().unwrap_or(Path::new("")).join(target)
                        }
                    }),
                    missing_target,
                    Some(message),
                    studio_disabled,
                ));
            }
        }
        return true;
    }

    let is_dir = observation.metadata.is_dir();
    if !is_dir {
        facts_cache.evict_lexical(entry_path);
        return false;
    }
    let (target, prepared_facts) = match resolved {
        Ok(value) => value,
        Err(error) => {
            facts_cache.evict_lexical(entry_path);
            read_issue(
                issues,
                DiscoveryReadIssueKind::Metadata,
                entry_path,
                error.to_string(),
            );
            let mut candidate = broken_symlink_candidate(
                plugin.materialize_with_pass(manifest_reads, issues),
                entry_path,
                root,
                scope,
                None,
                false,
                Some("Skill directory changed or became unavailable".to_string()),
                studio_disabled,
            );
            candidate.is_symlink = false;
            out.push(candidate);
            return true;
        }
    };
    match prepared_facts.materialize(reader, facts_cache, entry_path, tokenizer, issues) {
        Ok(None) => false,
        Err(()) => true,
        Ok(Some(facts)) => {
            out.push(build_candidate(
                read_context,
                plugin.materialize_with_pass(manifest_reads, issues),
                entry_path,
                &target,
                root,
                false,
                None,
                None,
                scope,
                &facts,
                git_cache,
                studio_disabled,
                issues,
            ));
            false
        }
    }
}

fn scope_str(root: &agents::SkillRoot) -> &'static str {
    if root.label == "parked" {
        // Distinct from "global" so a parked skill's deployment doesn't get
        // counted as a live global deployment in coverage totals - see
        // `skill_park`'s module docs and skill-coverage.ts.
        "parked"
    } else if root.project_path.is_some() {
        "project"
    } else {
        "global"
    }
}

fn candidate_facts_are_incomplete(candidate: &SkillCandidate) -> bool {
    candidate.folder_truncated
        || candidate.symlink_error.is_some()
        || matches!(
            candidate.git_repo,
            GitRepoEvidence::Unknown | GitRepoEvidence::Truncated
        )
        || matches!(candidate.plugin, PluginEvidence::Unknown)
}

fn fact_outcome(
    membership: RootReadOutcome,
    issues_before: usize,
    candidates: &[SkillCandidate],
    issues: &[DiscoveryReadIssue],
) -> RootReadOutcome {
    match membership {
        RootReadOutcome::Absent | RootReadOutcome::Failed => membership,
        RootReadOutcome::Incomplete => RootReadOutcome::Incomplete,
        RootReadOutcome::Read
            if issues.len() != issues_before
                || candidates.iter().any(candidate_facts_are_incomplete) =>
        {
            RootReadOutcome::Incomplete
        }
        RootReadOutcome::Read => RootReadOutcome::Read,
    }
}

fn observe_named_root(
    scope: &SkillReadScope,
    path: &Path,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> (
    RootReadOutcome,
    Option<crate::skill_scope::ScopedEntryObservation>,
) {
    let Some((name, parent)) = path.file_name().zip(path.parent()) else {
        read_issue(
            issues,
            DiscoveryReadIssueKind::Root,
            path,
            "root has no parent or name",
        );
        return (RootReadOutcome::Failed, None);
    };
    let observation = match scope.observe_entry(parent, name) {
        Ok(observation) => observation,
        Err(ScopedReadError::Missing { .. }) => return (RootReadOutcome::Absent, None),
        Err(error) => {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Root,
                path,
                error.to_string(),
            );
            return (RootReadOutcome::Failed, None);
        }
    };
    match scope.resolve_observed_dir(&observation) {
        Ok(_) => (RootReadOutcome::Read, Some(observation)),
        Err(error) => {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Root,
                path,
                error.to_string(),
            );
            (RootReadOutcome::Failed, None)
        }
    }
}

fn revalidate_named_root(
    scope: &SkillReadScope,
    path: &Path,
    observation: Option<&crate::skill_scope::ScopedEntryObservation>,
    issues: &mut Vec<DiscoveryReadIssue>,
) -> bool {
    let Some(observation) = observation else {
        return true;
    };
    match scope.resolve_observed_dir(observation) {
        Ok(_) => true,
        Err(error) => {
            read_issue(
                issues,
                DiscoveryReadIssueKind::Root,
                path,
                format!("root changed after enumeration: {error}"),
            );
            false
        }
    }
}

/// Discover every skill directory found by walking the agent skill roots and
/// native plugin caches. Each result is a fact record; no classification or
/// merging happens here. A thin wrapper over `discover_skill_candidates_cached`
/// with a fresh, one-call `SkillFactsCache` - callers that rebuild repeatedly
/// (`skill_refresh`) should keep a `SkillFactsCache` across calls instead, so
/// a folder whose fingerprint hasn't changed skips its expensive facts
/// recomputation.
pub fn discover_skill_candidates(context: &SkillDiscoveryReadContext) -> DiscoveryReport {
    let mut cache = SkillFactsCache::default();
    discover_skill_candidates_cached(context, &mut cache)
}

/// `discover_skill_candidates`, reusing `cache`'s content facts across calls:
/// a skill folder whose `folder_fingerprint` hasn't changed since the last
/// call skips SKILL.md tokenizing and content hashing entirely. Entries for
/// directories not visited this pass (a deleted skill) are evicted at the
/// end, so a long-lived cache never pins memory for skills that are gone.
pub fn discover_skill_candidates_cached(
    context: &SkillDiscoveryReadContext,
    cache: &mut SkillFactsCache,
) -> DiscoveryReport {
    cache.begin_pass();
    let report = discover_into(context, cache, None);
    cache.end_pass();
    report
}

/// Discover only the requested lexical skill names at every configured agent
/// root. Unlike a full pass, this does not evict untouched facts-cache entries.
pub fn discover_named_skill_candidates_cached(
    context: &SkillDiscoveryReadContext,
    names: &std::collections::BTreeSet<String>,
    cache: &mut SkillFactsCache,
) -> DiscoveryReport {
    cache.begin_pass();
    let report = discover_into(context, cache, Some(names));
    cache.end_named_pass(names);
    report
}

fn discover_into(
    context: &SkillDiscoveryReadContext,
    facts_cache: &mut SkillFactsCache,
    names: Option<&BTreeSet<String>>,
) -> DiscoveryReport {
    PreparedDiscovery::enumerate(context, names).materialize(facts_cache)
}

pub(crate) struct PreparedDiscovery<'a> {
    context: &'a SkillDiscoveryReadContext,
    extent: DiscoveryExtent,
    roots: Vec<PreparedAgentRoot>,
    plugins: PreparedPluginCandidates,
}

impl<'a> PreparedDiscovery<'a> {
    fn enumerate(context: &'a SkillDiscoveryReadContext, names: Option<&BTreeSet<String>>) -> Self {
        let roots = prepare_agent_roots(context, names);
        let plugins = PreparedPluginCandidates::enumerate(context, names);
        Self {
            context,
            extent: if names.is_some() {
                DiscoveryExtent::Named
            } else {
                DiscoveryExtent::Full
            },
            roots,
            plugins,
        }
    }

    pub(crate) fn enumerate_coordinated(
        context: &'a SkillDiscoveryReadContext,
        names: Option<&BTreeSet<String>>,
        guard: crate::skill_coordination::CoordinatedReadGuard,
    ) -> Result<
        (Self, crate::skill_coordination::CoordinatedReadGuard),
        crate::skill_coordination::CoordinationFailure,
    > {
        let mut check = || guard.check_cancelled();
        let mut plugin_cache = plugins::PluginObservationCache::new(context);
        let mut roots = Vec::new();
        for root in agents::skill_roots(context.home(), context.projects()) {
            for disabled in [false, true] {
                roots.push(PreparedAgentRoot::enumerate_checked(
                    context,
                    &mut plugin_cache,
                    root.clone(),
                    disabled,
                    names,
                    &mut check,
                )?);
            }
        }
        guard.check_cancelled()?;
        let (report, guard) = plugins::PreparedPluginScan::enumerate_checked(context, &mut || {
            guard.check_cancelled()
        })?
        .materialize_coordinated(names, guard)?;
        let plugins = PreparedPluginCandidates::from_report_checked(context, report, &mut || {
            guard.check_cancelled()
        })?;
        let mut files = BTreeSet::new();
        for root in &roots {
            guard.check_cancelled()?;
            for (_, entry) in &root.entries {
                guard.check_cancelled()?;
                if let Ok(entry) = entry {
                    entry.plugin.append_regular_files(&mut files);
                    if let Ok((_, facts)) = &entry.resolved {
                        facts.append_regular_files(&mut files);
                    }
                }
            }
        }
        for (_, facts) in &plugins.skills {
            guard.check_cancelled()?;
            facts.append_regular_files(&mut files);
        }
        let guard = guard
            .extend_with_files(context.read_scope(), &files.into_iter().collect::<Vec<_>>())?;
        Ok((
            Self {
                context,
                extent: if names.is_some() {
                    DiscoveryExtent::Named
                } else {
                    DiscoveryExtent::Full
                },
                roots,
                plugins,
            },
            guard,
        ))
    }

    fn materialize(self, facts_cache: &mut SkillFactsCache) -> DiscoveryReport {
        self.materialize_guarded(facts_cache, None)
    }

    pub(crate) fn materialize_guarded(
        self,
        facts_cache: &mut SkillFactsCache,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> DiscoveryReport {
        self.materialize_with_proof(facts_cache, guard).0
    }

    pub(crate) fn materialize_with_proof(
        self,
        facts_cache: &mut SkillFactsCache,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
    ) -> (DiscoveryReport, DiscoveryReadProof) {
        self.materialize_with_proof_checked(facts_cache, guard, &mut || Ok(()))
            .expect("unguarded materialization cannot be cancelled")
    }

    pub(crate) fn materialize_with_proof_checked(
        self,
        facts_cache: &mut SkillFactsCache,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<(DiscoveryReport, DiscoveryReadProof), crate::skill_coordination::CoordinationFailure>
    {
        check()?;
        let context = self.context;
        let home = context.home();
        let extent = self.extent;
        let mut out = Vec::new();
        let mut issues = Vec::new();
        let mut source_coverage = Vec::new();
        let mut agent_roots = Vec::new();
        context.append_bind_issues(&mut issues);
        let mut git_cache: HashMap<PathBuf, GitRepoEvidence> = HashMap::new();
        let tokenizer = tokenizer();
        check()?;
        if tokenizer.is_none() {
            read_issue(
                &mut issues,
                DiscoveryReadIssueKind::Tokenizer,
                home,
                "Tokenizer is unavailable",
            );
        }

        let mut manifest_reads = plugins::ManifestReadPass::new(context.read_scope(), guard);
        let mut manifest_validation = plugins::ManifestValidationPass::new(context.read_scope());
        for root in self.roots {
            check()?;
            let issues_before = issues.len();
            let candidates_before = out.len();
            let path = root.path.clone();
            let source = if root.disabled {
                MembershipSource::DisabledRoot
            } else {
                MembershipSource::AgentRoot
            };
            let (state, root_proof) = root.materialize_guarded(
                context,
                guard,
                facts_cache,
                &mut git_cache,
                tokenizer,
                &mut out,
                &mut issues,
                check,
                &mut manifest_reads,
                &mut manifest_validation,
            )?;
            agent_roots.push(root_proof);
            let facts = fact_outcome(state, issues_before, &out[candidates_before..], &issues);
            source_coverage.push(SourceCoverage {
                path,
                source,
                extent,
                membership: state,
                facts,
            });
        }

        let proof = self.plugins.materialize(
            context,
            guard,
            facts_cache,
            &mut git_cache,
            tokenizer,
            &mut out,
            &mut issues,
            &mut source_coverage,
            check,
        )?;

        check()?;
        Ok((
            DiscoveryReport {
                candidates: out,
                read_issues: issues,
                extent,
                source_coverage,
            },
            DiscoveryReadProof {
                plugins: proof,
                agent_roots,
            },
        ))
    }
}

struct ResourceReadProof {
    unchanged: bool,
    directories: Vec<Arc<crate::skill_scope::ScopedDirectoryObservation>>,
}

impl ResourceReadProof {
    fn capture(facts: &PreparedSkillFacts) -> Self {
        let directories = match facts {
            PreparedSkillFacts::Ready { content, .. } => content
                .as_ref()
                .as_ref()
                .map(|content| content.walk.directories.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        Self {
            directories,
            unchanged: true,
        }
    }

    fn revalidate(&self, scope: &SkillReadScope) -> bool {
        self.current(scope) == self.unchanged
    }

    fn current(&self, scope: &SkillReadScope) -> bool {
        self.directories
            .iter()
            .all(|directory| directory.revalidate(scope))
    }
}

struct SpecReadProof {
    path: PathBuf,
    markers: Arc<PreparedSpecMarkers>,
    state: (bool, bool),
}

impl SpecReadProof {
    fn capture(path: &Path, markers: Arc<PreparedSpecMarkers>, scope: &SkillReadScope) -> Self {
        let mut issues = Vec::new();
        let has_spec = markers.materialize(scope, path, &mut issues);
        Self {
            path: path.to_path_buf(),
            markers,
            state: (has_spec, issues.is_empty()),
        }
    }

    fn revalidate(&self, scope: &SkillReadScope) -> bool {
        let mut issues = Vec::new();
        let has_spec = self.markers.materialize(scope, &self.path, &mut issues);
        (has_spec, issues.is_empty()) == self.state
    }
}

struct DeploymentContentReadProof {
    resources: Option<ResourceReadProof>,
    spec: Option<SpecReadProof>,
}

impl DeploymentContentReadProof {
    fn capture(path: &Path, facts: Option<&PreparedSkillFacts>, scope: &SkillReadScope) -> Self {
        let spec = match facts {
            Some(PreparedSkillFacts::Ready { spec, .. }) => {
                Some(SpecReadProof::capture(path, Arc::clone(spec), scope))
            }
            _ => None,
        };
        let resources = facts.map(ResourceReadProof::capture);
        Self { resources, spec }
    }

    fn resources_unchanged(&self, scope: &SkillReadScope) -> bool {
        self.resources
            .as_ref()
            .is_none_or(|proof| proof.revalidate(scope))
    }

    fn spec_unchanged(&self, scope: &SkillReadScope) -> bool {
        self.spec
            .as_ref()
            .is_none_or(|proof| proof.revalidate(scope))
    }
}

struct AgentEntryReadProof {
    content: DeploymentContentReadProof,
    path: PathBuf,
    observation: Arc<crate::skill_scope::ScopedEntryObservation>,
    ancestry: Arc<plugins::PreparedPluginAncestry>,
    target: Option<PathBuf>,
    missing_target: bool,
    document: Option<Option<ScopedContentObservation>>,
    unchanged: bool,
}

impl AgentEntryReadProof {
    fn capture(path: &Path, entry: &PreparedObservedEntry, scope: &SkillReadScope) -> Self {
        let document = match &entry.resolved {
            Ok((_, PreparedSkillFacts::Ready { content, .. })) => match content.as_ref() {
                Ok(content) => Some(Some(content.document.clone())),
                Err(FactsWalkError::MissingDocument) => Some(None),
                _ => None,
            },
            _ => None,
        };
        let content = DeploymentContentReadProof::capture(
            path,
            entry.resolved.as_ref().ok().map(|(_, facts)| facts),
            scope,
        );
        Self {
            content,
            path: path.to_path_buf(),
            observation: Arc::clone(&entry.observation),
            ancestry: Arc::clone(&entry.plugin),
            target: entry.resolved.as_ref().ok().map(|(path, _)| path.clone()),
            missing_target: match &entry.resolved {
                Err(ScopedReadError::Missing { .. }) => true,
                Err(ScopedReadError::LinkTarget { source, .. }) => {
                    source.kind() == std::io::ErrorKind::NotFound
                }
                _ => false,
            },
            document,
            unchanged: true,
        }
    }

    fn current(
        &self,
        scope: &SkillReadScope,
        manifests: &mut plugins::ManifestValidationPass<'_>,
    ) -> bool {
        if let Some(target) = &self.target {
            if !matches!(scope.resolve_observed_dir(&self.observation), Ok(current) if &current == target)
            {
                return false;
            }
        }
        if self.missing_target {
            let absent = match scope.resolve_observed_dir(&self.observation) {
                Err(ScopedReadError::Missing { .. }) => true,
                Err(ScopedReadError::LinkTarget { source, .. }) => {
                    source.kind() == std::io::ErrorKind::NotFound
                }
                _ => false,
            };
            if !absent {
                return false;
            }
        }
        let document = self.path.join("SKILL.md");
        let unchanged = match &self.document {
            Some(Some(before)) => {
                matches!(scope.observe_content_regular(&document), Ok(after) if before == &after)
            }
            Some(None) => matches!(
                scope.observe_content_regular(&document),
                Err(ScopedReadError::Missing { .. })
            ),
            None => true,
        };
        unchanged
            && self.content.resources_unchanged(scope)
            && self.ancestry.revalidate(manifests)
            && self.content.spec_unchanged(scope)
    }
}

struct AgentRootReadProof {
    entries: Vec<AgentEntryReadProof>,
    path: PathBuf,
    absent: bool,
    observation: Option<crate::skill_scope::ScopedEntryObservation>,
    unchanged: bool,
}

impl AgentRootReadProof {
    fn revalidate(
        &self,
        scope: &SkillReadScope,
        manifests: &mut plugins::ManifestValidationPass<'_>,
    ) -> bool {
        let mut issues = Vec::new();
        let unchanged = if self.absent {
            observe_named_root(scope, &self.path, &mut issues).0 == RootReadOutcome::Absent
        } else {
            revalidate_named_root(scope, &self.path, self.observation.as_ref(), &mut issues)
        };
        unchanged == self.unchanged
            && self
                .entries
                .iter()
                .all(|entry| entry.current(scope, manifests) == entry.unchanged)
    }
}

pub(crate) struct DiscoveryReadProof {
    plugins: PluginReadProof,
    agent_roots: Vec<AgentRootReadProof>,
}

impl DiscoveryReadProof {
    pub(crate) fn into_membership(self) -> DiscoveryMembershipProof {
        let mut agent_roots = self.agent_roots;
        let ancestry = agent_roots
            .iter_mut()
            .flat_map(|root| std::mem::take(&mut root.entries))
            .map(|entry| entry.ancestry)
            .collect();
        DiscoveryMembershipProof {
            plugins: self.plugins,
            ancestry,
            agent_roots,
        }
    }

    pub(crate) fn revalidate(
        &self,
        scope: &SkillReadScope,
    ) -> Result<(), crate::skill_coordination::CoordinationFailure> {
        self.plugins.revalidate(scope)?;
        let mut manifests = plugins::ManifestValidationPass::new(scope);
        if self
            .agent_roots
            .iter()
            .all(|root| root.revalidate(scope, &mut manifests))
        {
            Ok(())
        } else {
            Err(crate::skill_coordination::CoordinationFailure::Changed)
        }
    }
}

pub(crate) struct DiscoveryMembershipProof {
    agent_roots: Vec<AgentRootReadProof>,
    plugins: PluginReadProof,
    ancestry: Vec<Arc<plugins::PreparedPluginAncestry>>,
}

impl DiscoveryMembershipProof {
    pub(crate) fn revalidate(
        &self,
        scope: &SkillReadScope,
        published: impl Fn(&Path) -> bool,
    ) -> Result<(), crate::skill_coordination::CoordinationFailure> {
        self.plugins.revalidate(scope)?;
        let mut manifests = plugins::ManifestValidationPass::new(scope);
        if self.agent_roots.iter().all(|root| {
            if let (Some(observation), Some(parent)) = (&root.observation, root.path.parent()) {
                if root.unchanged && !root.absent {
                    for name in ["skill-studio.json", "agents.lock", "agents.toml"] {
                        let sibling = parent.join(name);
                        if published(&sibling) {
                            return scope
                                .resolve_observed_dir_after_sibling_replace(observation, &sibling)
                                .is_ok();
                        }
                    }
                }
            }
            root.revalidate(scope, &mut manifests)
        }) && self
            .ancestry
            .iter()
            .all(|ancestry| ancestry.revalidate(&mut manifests))
        {
            Ok(())
        } else {
            Err(crate::skill_coordination::CoordinationFailure::Changed)
        }
    }
}

pub(crate) struct PluginReadProof {
    content: Vec<DeploymentContentReadProof>,
    caches: Vec<plugins::PluginCacheProof>,
    invalidated: BTreeSet<PathBuf>,
}

impl PluginReadProof {
    pub(crate) fn revalidate(
        &self,
        scope: &SkillReadScope,
    ) -> Result<(), crate::skill_coordination::CoordinationFailure> {
        let mut issues = Vec::new();
        let current = self
            .caches
            .iter()
            .flat_map(|proof| proof.revalidate(scope, &mut issues))
            .collect::<BTreeSet<_>>();
        if current == self.invalidated
            && self.content.iter().all(|proof| proof.spec_unchanged(scope))
            && self
                .content
                .iter()
                .all(|proof| proof.resources_unchanged(scope))
        {
            Ok(())
        } else {
            Err(crate::skill_coordination::CoordinationFailure::Changed)
        }
    }
}

struct PreparedPluginCandidates {
    cache_proofs: Vec<plugins::PluginCacheProof>,
    skills: Vec<(plugins::PluginSkillDir, PreparedSkillFacts)>,
    read_issues: Vec<DiscoveryReadIssue>,
    coverage: Vec<SourceCoverage>,
}

impl PreparedPluginCandidates {
    fn enumerate(context: &SkillDiscoveryReadContext, names: Option<&BTreeSet<String>>) -> Self {
        Self::from_report(context, plugins::scan_plugin_skills(context, names))
    }

    fn from_report(context: &SkillDiscoveryReadContext, report: plugins::PluginScanReport) -> Self {
        Self::from_report_checked(context, report, &mut || Ok(()))
            .expect("unguarded plugin candidates cannot be cancelled")
    }

    fn from_report_checked(
        context: &SkillDiscoveryReadContext,
        report: plugins::PluginScanReport,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<Self, crate::skill_coordination::CoordinationFailure> {
        check()?;
        Ok(Self {
            skills: report
                .skills
                .into_iter()
                .map(|plugin| {
                    let facts = PreparedSkillFacts::enumerate_plugin_checked(
                        context.read_scope(),
                        &plugin,
                        check,
                    )?;
                    Ok((plugin, facts))
                })
                .collect::<Result<_, crate::skill_coordination::CoordinationFailure>>()?,
            read_issues: report.read_issues,
            coverage: report.coverage,
            cache_proofs: report.cache_proofs,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn materialize(
        self,
        context: &SkillDiscoveryReadContext,
        guard: Option<&crate::skill_coordination::CoordinatedReadGuard>,
        facts_cache: &mut SkillFactsCache,
        git_cache: &mut HashMap<PathBuf, GitRepoEvidence>,
        tokenizer: Option<&CoreBPE>,
        out: &mut Vec<SkillCandidate>,
        issues: &mut Vec<DiscoveryReadIssue>,
        source_coverage: &mut Vec<SourceCoverage>,
        check: &mut dyn FnMut() -> Result<(), crate::skill_coordination::CoordinationFailure>,
    ) -> Result<PluginReadProof, crate::skill_coordination::CoordinationFailure> {
        check()?;
        let candidates_before = out.len();
        let mut content = Vec::new();
        issues.extend(self.read_issues);
        source_coverage.extend(self.coverage);
        for (plugin_skill, prepared_facts) in self.skills {
            check()?;
            let mut resource_proof = ResourceReadProof::capture(&prepared_facts);
            let spec = match &prepared_facts {
                PreparedSkillFacts::Ready { spec, .. } => Some(Arc::clone(spec)),
                _ => None,
            };
            if let Err(error) = context
                .read_scope()
                .resolve_observed_dir(&plugin_skill.entry_observation)
            {
                facts_cache.evict_lexical(&plugin_skill.skill_dir);
                read_issue(
                    issues,
                    DiscoveryReadIssueKind::Entry,
                    &plugin_skill.skill_dir,
                    error.to_string(),
                );
                mark_plugin_coverage_incomplete(source_coverage, &plugin_skill.skill_dir, true);
                continue;
            }
            let dir_name = plugin_skill
                .skill_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let Ok(Some(facts)) = prepared_facts.materialize(
                SkillContentRead {
                    scope: context.read_scope(),
                    guard,
                },
                facts_cache,
                &plugin_skill.skill_dir,
                tokenizer,
                issues,
            ) else {
                mark_plugin_coverage_incomplete(source_coverage, &plugin_skill.skill_dir, true);
                continue;
            };
            if facts.incomplete {
                resource_proof.unchanged = resource_proof.current(context.read_scope());
            }
            let spec = spec.map(|spec| {
                SpecReadProof::capture(&plugin_skill.skill_dir, spec, context.read_scope())
            });
            content.push(DeploymentContentReadProof {
                resources: Some(resource_proof),
                spec,
            });
            check()?;
            let candidate = SkillCandidate {
                name: dir_name.clone(),
                path: plugin_skill.skill_dir.clone(),
                root_label: plugin_skill.plugin.harness.clone(),
                scope: "plugin".to_string(),
                project_path: None,
                is_symlink: false,
                symlink_target: None,
                resolved_path: None,
                symlink_is_broken: false,
                symlink_error: None,
                plugin: PluginEvidence::Confirmed(plugin_skill.plugin),
                frontmatter_fields: facts.frontmatter_fields.clone(),
                frontmatter: facts.frontmatter_parse_result.as_frontmatter().cloned(),
                spec_violations: validate_skill(
                    &dir_name,
                    &facts.frontmatter_parse_result,
                    facts.skill_md_line_count,
                ),
                has_spec: facts.has_spec,
                folder_bytes: facts.folder_bytes,
                file_count: facts.file_count,
                skill_md_tokens: facts.skill_md_tokens,
                description_tokens: facts.description_tokens,
                content_hash: facts.content_hash.clone(),
                modified_at: facts.modified_at.clone(),
                folder_truncated: facts.folder_truncated,
                git_repo: git_repo_evidence(
                    context.read_scope(),
                    &plugin_skill.skill_dir,
                    git_cache,
                    issues,
                ),
                studio_disabled: false,
                shared_via_whole_dir_link: false,
            };
            if facts.incomplete || candidate_facts_are_incomplete(&candidate) {
                mark_plugin_coverage_incomplete(source_coverage, &plugin_skill.skill_dir, false);
            }
            out.push(candidate);
        }
        let mut invalidated_paths = Vec::new();
        for proof in &self.cache_proofs {
            check()?;
            invalidated_paths.extend(proof.revalidate(context.read_scope(), issues));
        }
        check()?;
        for coverage in source_coverage.iter_mut().filter(|coverage| {
            coverage.source == MembershipSource::PluginCache
                && invalidated_paths
                    .iter()
                    .any(|path| path.starts_with(&coverage.path))
        }) {
            if coverage.membership != RootReadOutcome::Failed {
                coverage.membership = RootReadOutcome::Incomplete;
                coverage.facts = RootReadOutcome::Incomplete;
            }
        }
        let mut index = 0;
        out.retain(|candidate| {
            let keep = index < candidates_before
                || !invalidated_paths
                    .iter()
                    .any(|path| candidate.path.starts_with(path));
            index += 1;
            if !keep {
                facts_cache.evict_lexical(&candidate.path);
            }
            keep
        });
        check()?;
        Ok(PluginReadProof {
            content,
            caches: self.cache_proofs,
            invalidated: invalidated_paths.into_iter().collect(),
        })
    }
}

fn mark_plugin_coverage_incomplete(
    source_coverage: &mut [SourceCoverage],
    skill_dir: &Path,
    membership: bool,
) {
    for coverage in source_coverage.iter_mut().filter(|coverage| {
        coverage.source == MembershipSource::PluginCache && skill_dir.starts_with(&coverage.path)
    }) {
        if membership && coverage.membership == RootReadOutcome::Read {
            coverage.membership = RootReadOutcome::Incomplete;
        }
        if coverage.facts == RootReadOutcome::Read {
            coverage.facts = RootReadOutcome::Incomplete;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn read_context(home: &Path, projects: &[PathBuf]) -> SkillDiscoveryReadContext {
        SkillDiscoveryReadContext::bind(
            home.to_path_buf(),
            projects.to_vec(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn discover_skill_candidates(home: &Path, projects: &[PathBuf]) -> DiscoveryReport {
        super::discover_skill_candidates(&read_context(home, projects))
    }

    fn discover_skill_candidates_cached(
        home: &Path,
        projects: &[PathBuf],
        cache: &mut SkillFactsCache,
    ) -> DiscoveryReport {
        super::discover_skill_candidates_cached(&read_context(home, projects), cache)
    }

    fn discover_named_skill_candidates_cached(
        home: &Path,
        projects: &[PathBuf],
        names: &std::collections::BTreeSet<String>,
        cache: &mut SkillFactsCache,
    ) -> DiscoveryReport {
        super::discover_named_skill_candidates_cached(&read_context(home, projects), names, cache)
    }

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: does things.\n---\nBody text here.\n"),
        )
        .unwrap();
    }

    #[test]
    fn facts_processing_returns_cancellation_instead_of_partial_discovery() {
        use crate::skill_coordination::{CancellationToken, CoordinationFailure};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write_skill(&home.join(".agents/skills/alpha"), "alpha");
        let plugin = home.join(".claude/plugins/cache/fixture");
        write_skill(&plugin.join("skills/beta"), "beta");
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let context = read_context(home, &[]);
        let names = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);
        for named in [false, true] {
            let selection = named.then_some(&names);
            let mut cache = SkillFactsCache::default();
            let mut count = 0;
            cache.begin_pass();
            let (report, _) = PreparedDiscovery::enumerate(&context, selection)
                .materialize_with_proof_checked(&mut cache, None, &mut || {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(report.candidates.len(), 2);
            for warm in [false, true] {
                for cancel_at in 1..=count {
                    let mut cache = SkillFactsCache::default();
                    if warm {
                        cache.begin_pass();
                        PreparedDiscovery::enumerate(&context, selection).materialize(&mut cache);
                        cache.end_pass();
                    }
                    cache.begin_pass();
                    let token = CancellationToken::default();
                    let mut checks = 0;
                    let result = PreparedDiscovery::enumerate(&context, selection)
                        .materialize_with_proof_checked(&mut cache, None, &mut || {
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
            }
        }
    }

    #[test]
    fn regular_and_plugin_resource_preparation_preserves_typed_cancellation() {
        use crate::skill_coordination::{CancellationToken, CoordinationFailure};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let plugin_root = home.join(".claude/plugins/cache/fixture");
        let skill = plugin_root.join("skills/alpha");
        write_skill(&skill, "alpha");
        fs::write(plugin_root.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        for relative in ["scripts", "assets/nested", "empty"] {
            fs::create_dir_all(skill.join(relative)).unwrap();
        }
        fs::write(skill.join("scripts/check.sh"), "fixture").unwrap();
        fs::write(skill.join("assets/nested/resource.txt"), "resource").unwrap();
        let context = read_context(home, &[]);
        let report = plugins::scan_plugin_skills(&context, None);
        assert_eq!(report.skills.len(), 1);
        for plugin in [false, true] {
            let prepare = |check: &mut dyn FnMut() -> Result<(), CoordinationFailure>| {
                if plugin {
                    PreparedSkillFacts::enumerate_plugin_checked(
                        context.read_scope(),
                        &report.skills[0],
                        check,
                    )
                } else {
                    PreparedSkillFacts::enumerate_checked(context.read_scope(), &skill, check)
                }
            };
            let mut count = 0;
            let ready = prepare(&mut || {
                count += 1;
                Ok(())
            })
            .unwrap();
            let PreparedSkillFacts::Ready { content, .. } = ready else {
                panic!("expected prepared content")
            };
            assert_eq!(content.unwrap().walk.file_count, 3);
            for cancel_at in 1..=count {
                let token = CancellationToken::default();
                let mut checks = 0;
                let result = prepare(&mut || {
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
            assert!(matches!(
                prepare(&mut || Ok(())),
                Ok(PreparedSkillFacts::Ready { .. })
            ));
        }
    }

    #[test]
    fn agent_root_preparation_returns_cancellation_without_partial_entries() {
        use crate::skill_coordination::{CancellationToken, CoordinationFailure};
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        for disabled in [false, true] {
            let path = if disabled {
                home.join(".agents/skills/.skill-studio-disabled")
            } else {
                home.join(".agents/skills")
            };
            for name in ["alpha", "beta"] {
                write_skill(&path.join(name), name);
            }
        }
        let context = read_context(home, &[]);
        let root = agents::skill_roots(home, &[])
            .into_iter()
            .find(|root| root.path == home.join(".agents/skills"))
            .unwrap();
        let names = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);
        for named in [false, true] {
            for disabled in [false, true] {
                let mut complete_checks = 0;
                let complete = PreparedAgentRoot::enumerate_checked(
                    &context,
                    &mut plugins::PluginObservationCache::new(&context),
                    root.clone(),
                    disabled,
                    named.then_some(&names),
                    &mut || {
                        complete_checks += 1;
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(complete.entries.len(), 2);
                for cancel_at in 1..=complete_checks {
                    let token = CancellationToken::default();
                    let mut checks = 0;
                    let result = PreparedAgentRoot::enumerate_checked(
                        &context,
                        &mut plugins::PluginObservationCache::new(&context),
                        root.clone(),
                        disabled,
                        named.then_some(&names),
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
            }
        }
    }

    fn write_malformed_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Triggers on: requests\n---\nBody.\n"),
        )
        .unwrap();
    }

    fn assert_only_git_edge_issues(issues: &[DiscoveryReadIssue]) {
        assert!(!issues.is_empty());
        assert!(issues.iter().all(|issue| {
            issue.kind == DiscoveryReadIssueKind::GitScopeBoundary
                && issue
                    .message
                    .contains("Git ancestry leaves the declared read scope")
        }));
    }

    #[test]
    fn named_discovery_adds_updates_and_removes_without_evicting_unrelated_facts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".claude/skills");
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        write_skill(&alpha, "alpha");
        write_skill(&beta, "beta");

        let mut cache = SkillFactsCache::default();
        let initial = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        assert!(initial.iter().any(|candidate| candidate.name == "alpha"));
        assert!(initial.iter().any(|candidate| candidate.name == "beta"));
        let beta_key = fs::canonicalize(&beta).unwrap();
        let alpha_key = fs::canonicalize(&alpha).unwrap();
        assert!(cache.entries.contains_key(&beta_key));

        fs::write(
            alpha.join("SKILL.md"),
            "---\nname: alpha\ndescription: changed.\n---\nNew body.\n",
        )
        .unwrap();
        let names = ["alpha".to_string()].into_iter().collect();
        let updated =
            discover_named_skill_candidates_cached(home, &[], &names, &mut cache).candidates;
        assert_eq!(updated.len(), 1);
        assert!(updated[0].description_tokens > 0);
        assert!(cache.entries.contains_key(&beta_key));

        fs::remove_dir_all(&alpha).unwrap();
        let removed =
            discover_named_skill_candidates_cached(home, &[], &names, &mut cache).candidates;
        assert!(removed.is_empty());
        assert!(!cache.entries.contains_key(&alpha_key));
        assert!(!cache.lexical_keys.contains_key(&alpha));
        assert!(cache.entries.contains_key(&beta_key));
    }

    #[test]
    fn named_discovery_uses_lexical_name_and_keeps_frontmatter_mismatch_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write_skill(&home.join(".claude/skills/lexical-name"), "different-name");
        let names = ["lexical-name".to_string()].into_iter().collect();
        let mut cache = SkillFactsCache::default();

        let found =
            discover_named_skill_candidates_cached(home, &[], &names, &mut cache).candidates;

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "lexical-name");
        assert!(found[0]
            .spec_violations
            .iter()
            .any(|violation| violation.contains("does not match its directory name")));
    }

    #[test]
    fn malformed_yaml_diagnostic_is_consistent_across_deployment_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = home.join("project");

        let global = home.join(".claude/skills/global-bad");
        let universal = home.join(".agents/skills/universal-bad");
        let project_skill = project.join(".codex/skills/project-bad");
        let disabled = home
            .join(".pi/agent/skills")
            .join(STUDIO_DISABLED_DIR_NAME)
            .join("disabled-bad");
        for (path, name) in [
            (&global, "global-bad"),
            (&universal, "universal-bad"),
            (&project_skill, "project-bad"),
            (&disabled, "disabled-bad"),
        ] {
            write_malformed_skill(path, name);
        }

        let symlink = home.join(".codex/skills/symlink-bad");
        fs::create_dir_all(symlink.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&universal, &symlink).unwrap();

        let plugin_root = home.join(".claude/plugins/cache/marketplace/bad-plugin/1.0.0");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name": "bad-plugin", "version": "1.0.0"}"#,
        )
        .unwrap();
        let plugin_skill = plugin_root.join("skills/plugin-bad");
        write_malformed_skill(&plugin_skill, "plugin-bad");

        let candidates = discover_skill_candidates(home, std::slice::from_ref(&project)).candidates;
        for path in [
            global,
            universal,
            project_skill,
            disabled,
            symlink,
            plugin_skill,
        ] {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.path == path)
                .unwrap_or_else(|| panic!("candidate not found: {}", path.display()));
            assert_eq!(
                candidate.spec_violations,
                ["invalid YAML frontmatter at line 3, column 25: mapping values are not allowed in this context"]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_alias_does_not_hide_links_below_the_project_boundary() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir(&home).unwrap();
        write_skill(&project.join(".agents/skills/sample"), "sample");
        fs::create_dir(project.join(".claude")).unwrap();
        symlink("../.agents/skills", project.join(".claude/skills")).unwrap();
        let alias = tmp.path().join("project-alias");
        symlink(&project, &alias).unwrap();
        let result = discover_skill_candidates(&home, std::slice::from_ref(&alias));
        for (relative, linked) in [
            (".agents/skills/sample", false),
            (".claude/skills/sample", true),
        ] {
            let path = alias.join(relative);
            let candidate = result.candidates.iter().find(|c| c.path == path).unwrap();
            assert!(!candidate.is_symlink);
            assert_eq!(candidate.shared_via_whole_dir_link, linked);
        }
    }

    #[test]
    fn symlink_into_shared_root_is_captured() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let real_skill = home.join(".agents/skills/find-bugs");
        write_skill(&real_skill, "find-bugs");

        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("find-bugs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_skill, &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("symlinked candidate found");
        assert!(found.is_symlink);
        assert!(!found.symlink_is_broken);
        assert_eq!(
            found.symlink_target.as_deref(),
            Some(fs::canonicalize(&real_skill).unwrap().as_path())
        );
    }

    #[test]
    fn cursor_and_grok_build_global_skill_dirs_are_discovered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".cursor/skills/x"), "x");
        write_skill(&home.join(".grok/skills/y"), "y");

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let cursor = candidates
            .iter()
            .find(|c| c.name == "x")
            .expect("cursor skill found");
        assert_eq!(cursor.root_label, "Cursor");
        let grok = candidates
            .iter()
            .find(|c| c.name == "y")
            .expect("grok build skill found");
        assert_eq!(grok.root_label, "Grok Build");
    }

    #[test]
    fn broken_symlink_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("ghost-skill");
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join("nowhere"), &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("broken symlink candidate found");
        assert!(found.is_symlink);
        assert!(found.symlink_is_broken);
        assert_eq!(found.name, "ghost-skill");
    }

    #[test]
    fn broken_symlink_keeps_lexical_plugin_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join(".claude/skills");
        fs::create_dir_all(&skills).unwrap();
        let link = skills.join("ghost");
        std::os::unix::fs::symlink("missing-target", &link).unwrap();
        let manifest = skills.join("plugin.json");
        fs::write(&manifest, r#"{"name":"lexical-owner"}"#).unwrap();

        let report = discover_skill_candidates(tmp.path(), &[]);
        let candidate = report.candidates.iter().find(|c| c.path == link).unwrap();
        assert!(candidate.symlink_is_broken);
        assert!(
            matches!(&candidate.plugin, PluginEvidence::Confirmed(info) if info.name == "lexical-owner")
        );

        fs::remove_file(&manifest).unwrap();
        fs::create_dir(&manifest).unwrap();
        let report = discover_skill_candidates(tmp.path(), &[]);
        let candidate = report.candidates.iter().find(|c| c.path == link).unwrap();
        assert!(matches!(candidate.plugin, PluginEvidence::Unknown));
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest
                && issue.path == manifest.to_string_lossy()));
    }

    #[test]
    fn plugin_cache_skill_gets_plugin_info() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let plugin_root = home.join(".claude/plugins/cache/marketplace/sentry-toolkit/1.0.0");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name": "sentry-toolkit", "version": "1.0.0"}"#,
        )
        .unwrap();
        write_skill(&plugin_root.join("skills/lint-code"), "lint-code");

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.name == "lint-code")
            .expect("plugin skill found");
        let PluginEvidence::Confirmed(plugin) = &found.plugin else {
            panic!("plugin info set");
        };
        assert_eq!(plugin.name, "sentry-toolkit");
        assert_eq!(plugin.harness, "Claude Code");
    }

    #[test]
    fn symlinked_plugin_skill_root_is_classified_via_canonical_target() {
        // The skill root reached through .agents/skills is a symlink into a
        // fake plugin cache layout; plugin provenance should still fire
        // because build_candidate checks the canonical target first.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let plugin_root = home.join(".claude/plugins/cache/marketplace/sentry-toolkit/1.0.0");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name": "sentry-toolkit", "version": "1.0.0"}"#,
        )
        .unwrap();
        let real_skill = plugin_root.join("skills/lint-code");
        write_skill(&real_skill, "lint-code");

        let shared_skills = home.join(".agents/skills");
        fs::create_dir_all(&shared_skills).unwrap();
        let link = shared_skills.join("lint-code");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_skill, &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("symlinked plugin skill found");
        let PluginEvidence::Confirmed(plugin) = &found.plugin else {
            panic!("plugin info set via canonical target");
        };
        assert_eq!(plugin.name, "sentry-toolkit");
    }

    #[test]
    fn linked_skill_root_yields_resolved_path_not_symlink() {
        // `.claude/skills` itself is a symlink to `.agents/skills` (not one
        // skill inside it), so a skill found by walking through it is a real
        // directory - `is_symlink` is false - but its canonical path (via
        // `.agents/skills`) differs from the path it was found at.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let shared_skill = home.join(".agents/skills/foo");
        write_skill(&shared_skill, "foo");
        fs::create_dir_all(home.join(".claude")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.root_label == "Claude Code" && c.name == "foo")
            .expect("skill reached through the linked root found");
        assert!(!found.is_symlink);
        assert_eq!(
            found.resolved_path.as_deref(),
            Some(fs::canonicalize(&shared_skill).unwrap().as_path())
        );
    }

    #[test]
    fn opencode_legacy_skill_dir_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(
            &home.join(".config/opencode/skill/legacy-skill"),
            "legacy-skill",
        );

        let candidates = discover_skill_candidates(home, &[]).candidates;
        assert!(candidates.iter().any(|c| c.name == "legacy-skill"));
    }

    #[test]
    fn tokens_bytes_and_file_count_are_non_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".claude/skills/write-tests"), "write-tests");

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.name == "write-tests")
            .expect("skill found");
        assert!(found.skill_md_tokens > 0);
        assert!(found.folder_bytes > 0);
        assert!(found.file_count > 0);
        assert!(found.description_tokens > 0);
        assert!(found.description_tokens < found.skill_md_tokens);
        assert!(!found.content_hash.is_empty());
        assert!(!found.folder_truncated);
    }

    #[test]
    fn symlinked_dir_inside_a_skill_folder_is_not_descended() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("write-tests");
        write_skill(&skill_dir, "write-tests");
        // A symlink back to the parent would recurse forever if followed.
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path(), skill_dir.join("loop")).unwrap();

        let walk = walk_folder(&skill_dir);
        // Only SKILL.md itself; the symlinked dir isn't descended into, and
        // (being a dir, not a file) doesn't add to file_count either.
        assert_eq!(walk.file_count, 1);
        assert!(!walk.truncated);
    }

    #[test]
    #[cfg(unix)]
    fn resource_file_links_count_without_digesting_and_escaped_targets_are_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = tmp.path().join("skill");
        write_skill(&skill, "skill");
        fs::write(skill.join("inside"), "inside").unwrap();
        std::os::unix::fs::symlink("inside", skill.join("inside-link")).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill)).unwrap();
        let walk = walk_folder_capped(&scope, &skill, MAX_FOLDER_FILES, MAX_FOLDER_BYTES).unwrap();
        assert_eq!(walk.file_count, 3);
        assert_eq!(walk.hashable.len(), 2);
        let outside = tmp.path().join("outside");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, skill.join("outside-link")).unwrap();
        let walk = walk_folder_capped(&scope, &skill, MAX_FOLDER_FILES, MAX_FOLDER_BYTES).unwrap();
        assert!(walk.incomplete_reason.is_some());
        assert_eq!(walk.hashable.len(), 2);
    }

    #[test]
    fn unreadable_entry_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("write-tests");
        write_skill(&skill_dir, "write-tests");
        let unreadable = skill_dir.join("secret.txt");
        fs::write(&unreadable, b"shh").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        }

        let walk = walk_folder(&skill_dir);
        assert!(!walk.truncated);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    #[test]
    fn growth_between_listing_and_observation_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let resource = tmp.path().join("resource.txt");
        fs::write(&resource, "original").unwrap();
        let scope = SkillReadScope::bind(&[tmp.path().to_path_buf()]).unwrap();
        let mut checks = 0;
        let walk = walk_folder_capped_with_check::<String>(
            &scope,
            tmp.path(),
            MAX_FOLDER_FILES,
            MAX_FOLDER_BYTES,
            &mut || {
                checks += 1;
                if checks == 2 {
                    fs::write(&resource, "original appended").unwrap();
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(checks, 2);
        assert!(walk.hashable.is_empty());
        assert!(walk
            .incomplete_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Resource changed after directory listing")));
    }

    #[test]
    fn strict_digest_rejects_growth_after_directory_listing() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "fixture");
        let resource = tmp.path().join("resource.txt");
        fs::write(&resource, "original").unwrap();
        let scope = SkillReadScope::bind(&[tmp.path().to_path_buf()]).unwrap();
        let mut checks = 0;
        let error = live_skill_content_hash_with_check(&scope, tmp.path(), || {
            checks += 1;
            if checks == 6 {
                fs::write(&resource, "original appended").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
        assert!(
            error.contains("Resource changed after directory listing"),
            "{error}"
        );
    }

    #[test]
    fn folder_walk_cap_is_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("many-files");
        fs::create_dir_all(&skill_dir).unwrap();
        for i in 0..10 {
            fs::write(skill_dir.join(format!("file-{i}.txt")), b"x").unwrap();
        }

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let walk = walk_folder_capped(&scope, &skill_dir, 3, MAX_FOLDER_BYTES).unwrap();
        assert!(walk.truncated);
        assert_eq!(walk.file_count, 3);
    }

    #[test]
    fn folder_walk_byte_cap_skips_oversized_file_before_reading() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("big-file");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("big.bin"), vec![0u8; 100]).unwrap();

        // A tiny cap: the one file present exceeds it, so it must be marked
        // truncated and skipped entirely, never queued or read.
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let walk = walk_folder_capped(&scope, &skill_dir, MAX_FOLDER_FILES, 10).unwrap();
        assert!(walk.truncated);
        assert_eq!(walk.file_count, 0);
        assert_eq!(walk.total_bytes, 0);
        assert!(walk.hashable.is_empty());
    }

    #[test]
    fn folder_walk_entry_budget_caps_empty_directories_across_the_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("a/b/leaf")).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let walk = walk_folder_with_entry_budget(&scope, &skill_dir, 2);
        assert!(walk.truncated);
        assert_eq!(walk.entries_remaining, 0);
    }

    #[test]
    fn symlink_alias_name_mismatch_is_flagged_using_alias_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let target = home.join(".agents/skills/foo");
        write_skill(&target, "foo");

        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("bar");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("aliased candidate found");
        // The alias is named "bar" but frontmatter declares "foo"; the
        // mismatch must be checked against the alias's own lexical name.
        assert!(found
            .spec_violations
            .iter()
            .any(|v| v.contains("does not match its directory name \"bar\"")));
    }

    #[test]
    fn symlink_alias_without_frontmatter_name_falls_back_to_alias_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let target = home.join(".agents/skills/foo");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("SKILL.md"),
            "---\ndescription: does things.\n---\nBody.\n",
        )
        .unwrap();

        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("bar");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("aliased candidate found");
        assert_eq!(found.name, "bar");
    }

    #[test]
    fn symlink_alias_matching_frontmatter_name_has_no_violation() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let target = home.join(".agents/skills/foo");
        write_skill(&target, "foo");

        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("foo");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("aliased candidate found");
        assert!(found.spec_violations.is_empty());
    }

    #[test]
    fn oversized_skill_md_is_truncated_but_still_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("huge-skill-md");
        fs::create_dir_all(&skill_dir).unwrap();
        let mut content = String::from("---\nname: huge-skill\ndescription: does things.\n---\n");
        // Pad the body well past SKILL_MD_MAX_BYTES.
        content.push_str(&"x".repeat((SKILL_MD_MAX_BYTES as usize) + 1024));
        fs::write(skill_dir.join("SKILL.md"), &content).unwrap();

        let facts = compute_content_facts(&skill_dir, None).expect("facts computed");
        assert!(facts.folder_truncated);
        assert_eq!(
            facts
                .frontmatter_parse_result
                .as_frontmatter()
                .and_then(|f| f.name.clone()),
            Some("huge-skill".to_string())
        );
    }

    #[test]
    fn content_hash_length_frames_records_to_avoid_boundary_collisions() {
        let tmp = tempfile::tempdir().unwrap();

        let dir_a = tmp.path().join("a");
        fs::create_dir_all(&dir_a).unwrap();
        fs::write(dir_a.join("a"), b"bc").unwrap();

        let dir_b = tmp.path().join("b");
        fs::create_dir_all(&dir_b).unwrap();
        fs::write(dir_b.join("ab"), b"c").unwrap();

        let hash_a = content_hash(walk_folder(&dir_a).hashable, MAX_FOLDER_BYTES);
        let hash_b = content_hash(walk_folder(&dir_b).hashable, MAX_FOLDER_BYTES);
        assert_ne!(hash_a, hash_b);

        let dir_a2 = tmp.path().join("a2");
        fs::create_dir_all(&dir_a2).unwrap();
        fs::write(dir_a2.join("a"), b"bc").unwrap();
        let hash_a2 = content_hash(walk_folder(&dir_a2).hashable, MAX_FOLDER_BYTES);
        assert_eq!(hash_a, hash_a2);
    }

    #[test]
    fn empty_frontmatter_name_falls_back_to_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for dir_name in ["one", "two"] {
            let skill_dir = home.join(".claude/skills").join(dir_name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: \"\"\ndescription: does things.\n---\nBody.\n",
            )
            .unwrap();
        }

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let mut names: Vec<_> = candidates.iter().map(|c| c.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn content_facts_are_computed_once_across_shared_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let real_skill = home.join(".agents/skills/find-bugs");
        write_skill(&real_skill, "find-bugs");

        for agent_dir in [".claude/skills", ".codex/skills", ".pi/agent/skills"] {
            let dir = home.join(agent_dir);
            fs::create_dir_all(&dir).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&real_skill, dir.join("find-bugs")).unwrap();
        }

        let mut cache = SkillFactsCache::default();
        let candidates = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        let found: Vec<_> = candidates
            .iter()
            .filter(|c| c.name == "find-bugs")
            .collect();
        // One candidate from the shared root itself, plus one per symlinking
        // agent root.
        assert_eq!(found.len(), 4);
        assert!(found
            .iter()
            .all(|c| c.content_hash == found[0].content_hash));
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn cached_discovery_reuses_facts_across_passes_when_nothing_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".claude/skills/find-bugs"), "find-bugs");

        let mut cache = SkillFactsCache::default();
        discover_skill_candidates_cached(home, &[], &mut cache);
        assert_eq!(cache.last_pass_stats(), (0, 1));

        let before = cache.entries.values().next().unwrap().facts.clone();
        discover_skill_candidates_cached(home, &[], &mut cache);
        assert_eq!(cache.last_pass_stats(), (1, 1));
        let after = cache.entries.values().next().unwrap().facts.clone();
        assert!(Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn adding_an_empty_evals_directory_invalidates_cached_has_spec() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");
        let mut cache = SkillFactsCache::default();

        let first = discover_skill_candidates_cached(home, &[], &mut cache);
        assert!(!first.candidates[0].has_spec);

        fs::create_dir(skill_dir.join("evals")).unwrap();
        let second = discover_skill_candidates_cached(home, &[], &mut cache);

        assert!(second.candidates[0].has_spec);
        assert_eq!(cache.last_pass_stats(), (0, 1));
    }

    #[test]
    fn editing_a_file_and_bumping_its_mtime_invalidates_the_cache_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");

        let mut cache = SkillFactsCache::default();
        let first = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        let first_hash = first
            .iter()
            .find(|c| c.name == "find-bugs")
            .unwrap()
            .content_hash
            .clone();

        let skill_md = skill_dir.join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: does things.\n---\nEdited body.\n",
        )
        .unwrap();
        // The write above may land within the same mtime tick as the
        // original file - bump it explicitly so the fingerprint is
        // guaranteed to change, matching what a real edit does on a
        // filesystem with coarser mtime resolution.
        let file = fs::File::open(&skill_md).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();

        let second = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        let second_hash = second
            .iter()
            .find(|c| c.name == "find-bugs")
            .unwrap()
            .content_hash
            .clone();

        assert_eq!(cache.last_pass_stats(), (0, 1));
        assert_ne!(first_hash, second_hash);
    }

    #[test]
    fn editing_skill_md_invalidates_cached_frontmatter_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");

        let mut cache = SkillFactsCache::default();
        let first = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        assert!(first[0].spec_violations.is_empty());

        let skill_md = skill_dir.join("SKILL.md");
        fs::write(
            &skill_md,
            "---\nname: find-bugs\ndescription: Triggers on: bug reports\n---\nBody.\n",
        )
        .unwrap();
        let file = fs::File::open(&skill_md).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();

        let second = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        assert_eq!(cache.last_pass_stats(), (0, 1));
        assert_eq!(second[0].spec_violations.len(), 1);
        assert!(second[0].spec_violations[0].starts_with("invalid YAML frontmatter at line 3"));
    }

    #[test]
    fn deleted_skill_dir_is_evicted_from_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");

        let mut cache = SkillFactsCache::default();
        discover_skill_candidates_cached(home, &[], &mut cache);
        assert_eq!(cache.entries.len(), 1);

        fs::remove_dir_all(&skill_dir).unwrap();
        let candidates = discover_skill_candidates_cached(home, &[], &mut cache).candidates;
        assert!(!candidates.iter().any(|c| c.name == "find-bugs"));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn skill_inside_a_git_repo_is_flagged_from_a_grandparent_dot_git() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        // The .git directory sits two levels above the skill folder, so the
        // flag must come from walking ancestors, not just the immediate
        // parent.
        let repo_root = home.join("projects/my-repo");
        fs::create_dir_all(repo_root.join(".git")).unwrap();
        write_skill(
            &repo_root.join(".claude/skills/tracked-skill"),
            "tracked-skill",
        );

        let candidates =
            discover_skill_candidates(home, std::slice::from_ref(&repo_root)).candidates;
        let found = candidates
            .iter()
            .find(|c| c.name == "tracked-skill")
            .expect("in-repo skill found");
        assert_eq!(found.git_repo, GitRepoEvidence::Present);
    }

    #[test]
    fn skill_moved_into_holding_dir_is_flagged_studio_disabled_without_double_counting() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(
            &home
                .join(".claude/skills")
                .join(STUDIO_DISABLED_DIR_NAME)
                .join("find-bugs"),
            "find-bugs",
        );

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let matching: Vec<_> = candidates
            .iter()
            .filter(|c| c.name == "find-bugs")
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "holding-dir entry must not be double-counted"
        );
        assert!(matching[0].studio_disabled);
    }

    #[test]
    fn skill_outside_any_git_repo_is_not_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(
            &home.join(".claude/skills/untracked-skill"),
            "untracked-skill",
        );

        let candidates = discover_skill_candidates(home, &[]).candidates;
        let found = candidates
            .iter()
            .find(|c| c.name == "untracked-skill")
            .expect("skill found");
        assert_eq!(found.git_repo, GitRepoEvidence::Truncated);
    }

    #[test]
    fn live_hash_check_aborts_before_opening_skill_md() {
        let missing_skill_dir = Path::new("/missing-skill-dir");
        let mut checks = 0;

        let scope = SkillReadScope::bind(&[std::env::temp_dir()]).unwrap();
        let error = live_skill_content_hash_with_check(&scope, missing_skill_dir, || {
            checks += 1;
            Err("cancelled".to_string())
        })
        .unwrap_err();

        assert_eq!(error, "cancelled");
        assert_eq!(checks, 1);
    }

    #[test]
    fn live_hash_check_aborts_during_document_read() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), vec![b'x'; 128 * 1024]).unwrap();
        let mut checks = 0;

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = live_skill_content_hash_with_check(&scope, &skill_dir, || {
            checks += 1;
            if checks == 3 {
                Err("cancelled".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(error, "cancelled");
        assert_eq!(checks, 3);
    }

    #[test]
    fn live_hash_rejects_a_capped_regular_resource() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        std::fs::File::create(skill_dir.join("large-resource"))
            .unwrap()
            .set_len(MAX_FOLDER_BYTES + 1)
            .unwrap();

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = live_skill_content_hash(&scope, &skill_dir).unwrap_err();

        assert!(error.contains("exceeds the hash limit"), "{error}");
    }

    #[test]
    fn live_hash_respects_the_combined_document_and_folder_budget() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        fs::create_dir_all(&skill_dir).unwrap();
        let document_len = 1024 * 1024;
        fs::write(skill_dir.join("SKILL.md"), vec![b'x'; document_len]).unwrap();
        let resource_len = MAX_FOLDER_BYTES - document_len as u64 - 1;
        fs::File::create(skill_dir.join("resource"))
            .unwrap()
            .set_len(resource_len)
            .unwrap();

        assert_eq!(document_len as u64 + resource_len, MAX_FOLDER_BYTES - 1);
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let (walk, _, _) = walk_for_facts(&scope, &skill_dir).unwrap();
        assert!(walk.truncated);
        let error = live_skill_content_hash(&scope, &skill_dir).unwrap_err();
        assert!(error.contains("exceeds the hash limit"), "{error}");
    }

    #[test]
    fn observation_hash_reports_resource_size_drift() {
        let temp = tempfile::tempdir().unwrap();
        let resource = temp.path().join("resource");
        fs::write(&resource, "before").unwrap();
        let mut files = walk_folder(temp.path()).hashable;
        fs::write(&resource, "changed after the walk").unwrap();
        let mut issues = Vec::new();

        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let digest = content_hash_with_check_mode(
            (&scope).into(),
            &mut files,
            MAX_FOLDER_BYTES,
            &mut || Ok(()),
            false,
            Some(&mut issues),
        )
        .unwrap();

        assert!(!digest.is_empty());
        assert!(issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Resource
                && issue.path == resource.to_string_lossy()));
    }

    #[test]
    fn strict_hash_rejects_a_resource_removed_after_the_walk() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        let resource = skill_dir.join("resource");
        fs::write(&resource, "resource").unwrap();
        let mut files = walk_folder(&skill_dir).hashable;
        let mut removed = false;

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = content_hash_with_check_mode(
            (&scope).into(),
            &mut files,
            MAX_FOLDER_BYTES,
            &mut || {
                if !removed {
                    fs::remove_file(&resource).unwrap();
                    removed = true;
                }
                Ok(())
            },
            true,
            None,
        )
        .unwrap_err();

        assert!(error.contains("Could not read metadata"), "{error}");
    }

    #[test]
    fn strict_hash_rejects_a_resource_size_changed_after_the_walk() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        let resource = skill_dir.join("resource");
        fs::write(&resource, "resource").unwrap();
        let mut files = walk_folder(&skill_dir).hashable;
        let mut changed = false;

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = content_hash_with_check_mode(
            (&scope).into(),
            &mut files,
            MAX_FOLDER_BYTES,
            &mut || {
                if !changed {
                    fs::write(&resource, "resource changed").unwrap();
                    changed = true;
                }
                Ok(())
            },
            true,
            None,
        )
        .unwrap_err();

        assert!(error.contains("File size changed"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn strict_hash_rejects_a_resource_that_becomes_unreadable_after_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        let resource = skill_dir.join("resource");
        let replacement = temp.path().join("replacement-directory");
        fs::create_dir_all(&replacement).unwrap();
        std::fs::File::create(&resource)
            .unwrap()
            .set_len(fs::metadata(&replacement).unwrap().len())
            .unwrap();
        let mut files = walk_folder(&skill_dir).hashable;
        let mut checks = 0;

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = content_hash_with_check_mode(
            (&scope).into(),
            &mut files,
            MAX_FOLDER_BYTES,
            &mut || {
                checks += 1;
                if checks == 2 {
                    fs::remove_file(&resource).unwrap();
                    fs::rename(&replacement, &resource).unwrap();
                }
                Ok(())
            },
            true,
            None,
        )
        .unwrap_err();

        assert!(error.contains("Could not read"), "{error}");
    }

    #[test]
    fn copy_repair_hashes_match_discovery_before_and_after_replacement() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        fs::write(skill.join("resource"), vec![b'r'; 2 * 1024 * 1024]).unwrap();
        let original = fs::read(skill.join("SKILL.md")).unwrap();
        let proposed = b"---\nname: alpha\ndescription: repaired\n---\n";
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill)).unwrap();
        let content = PreparedCopyRepairContent::enumerate(&scope, &skill).unwrap();
        let lease = CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(&skill, CoordinationMode::Exclusive)],
            temp.path(),
            None,
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &content.files())
        .unwrap();
        let (before, after) = content.hashes(&scope, &lease, &original, proposed).unwrap();
        assert_eq!(before, live_skill_content_hash(&scope, &skill).unwrap());
        fs::write(skill.join("SKILL.md"), proposed).unwrap();
        assert!(content.hashes(&scope, &lease, &original, proposed).is_err());
        drop(lease);
        assert_eq!(after, live_skill_content_hash(&scope, &skill).unwrap());
    }

    #[test]
    fn copy_repair_enumeration_and_streaming_observe_cancellation() {
        use crate::skill_coordination::{
            CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect,
        };
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        let resource = skill.join("resource");
        fs::write(&resource, vec![b'r'; 2 * 1024 * 1024]).unwrap();
        let scope = SkillReadScope::bind(std::slice::from_ref(&skill)).unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(
            PreparedCopyRepairContent::enumerate_controlled(&scope, &skill, &cancelled)
                .err()
                .is_some_and(|error| error.is_cancelled())
        );
        let cancellation = CancellationToken::default();
        let content =
            PreparedCopyRepairContent::enumerate_cancellable(&scope, &skill, &cancellation)
                .unwrap();
        let lease = CoordinationPlan::new_cancellable(
            vec![DirectoryEffect::tree(&skill, CoordinationMode::Exclusive)],
            Some(std::time::Duration::from_secs(10)),
            cancellation.clone(),
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&scope, &content.files())
        .unwrap();
        let mut bytes_seen = 0;
        let result = lease.fold_resource(&resource, 2 * 1024 * 1024, &mut |bytes| {
            assert!(bytes.len() <= 64 * 1024);
            bytes_seen += bytes.len();
            cancellation.cancel();
        });
        assert!(result.unwrap_err().is_cancelled());
        assert!(bytes_seen > 0 && bytes_seen <= 64 * 1024);
    }

    #[test]
    fn copy_repair_hashes_require_resource_planning_and_stable_membership() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for case in ["unplanned", "added", "changed"] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            fs::write(skill.join("resource"), b"resource").unwrap();
            let original = fs::read(skill.join("SKILL.md")).unwrap();
            let scope = SkillReadScope::bind(std::slice::from_ref(&skill)).unwrap();
            let content = PreparedCopyRepairContent::enumerate(&scope, &skill).unwrap();
            let files = if case == "unplanned" {
                vec![skill.join("SKILL.md")]
            } else {
                content.files()
            };
            let lease = CoordinationPlan::new_fixture(
                vec![DirectoryEffect::tree(&skill, CoordinationMode::Exclusive)],
                temp.path(),
                None,
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &files)
            .unwrap();
            if case == "added" {
                fs::write(skill.join("new"), b"new").unwrap();
            }
            if case == "changed" {
                fs::write(skill.join("resource"), b"modified").unwrap();
            }
            assert!(
                content
                    .hashes(&scope, &lease, &original, b"replacement")
                    .is_err(),
                "{case}"
            );
        }
    }

    #[test]
    fn live_hash_matches_the_existing_digest_for_a_complete_tree() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        fs::write(skill_dir.join("resource"), "resource").unwrap();
        let walk = walk_folder(&skill_dir);

        assert_eq!(
            live_skill_content_hash(
                &SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap(),
                &skill_dir
            )
            .unwrap(),
            content_hash(walk.hashable, MAX_FOLDER_BYTES)
        );
    }

    #[test]
    fn live_hash_rejects_a_capped_skill_document() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            vec![b'x'; SKILL_MD_MAX_BYTES as usize + 1],
        )
        .unwrap();

        let scope = SkillReadScope::bind(std::slice::from_ref(&skill_dir)).unwrap();
        let error = live_skill_content_hash(&scope, &skill_dir).unwrap_err();

        assert!(error.contains("exceeds the hash limit"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn live_hash_keeps_the_existing_resource_symlink_exclusion() {
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join("find-bugs");
        write_skill(&skill_dir, "find-bugs");
        let outside = temp.path().join("outside-resource");
        fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, skill_dir.join("resource-link")).unwrap();
        let walk = walk_folder(&skill_dir);

        assert_eq!(
            live_skill_content_hash(
                &SkillReadScope::bind(&[skill_dir.clone(), temp.path().to_path_buf()]).unwrap(),
                &skill_dir
            )
            .unwrap(),
            content_hash(walk.hashable, MAX_FOLDER_BYTES)
        );
    }
    #[test]
    fn discovery_reports_non_directory_root_but_not_absent_optional_root() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/skills"), "not a directory").unwrap();

        let report = discover_skill_candidates(home, &[]);
        assert!(report.candidates.is_empty());
        assert!(report.read_issues.iter().any(|issue| {
            issue.kind == DiscoveryReadIssueKind::Root && issue.path.ends_with(".claude/skills")
        }));

        fs::remove_file(home.join(".claude/skills")).unwrap();
        let report = discover_skill_candidates(home, &[]);
        assert!(report.read_issues.is_empty());
    }

    #[test]
    fn malformed_candidate_is_retained_without_read_issue() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join(".claude/skills/bad");
        write_malformed_skill(&skill, "bad");
        let report = discover_skill_candidates(temp.path(), &[]);
        assert_eq!(report.candidates.len(), 1);
        assert!(report.candidates[0].frontmatter.is_none());
        assert_only_git_edge_issues(&report.read_issues);
    }
    #[test]
    fn cache_refreshes_when_skill_document_bytes_change_with_restored_mtime() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join(".claude/skills/alpha");
        write_skill(&skill, "alpha");
        let document = skill.join("SKILL.md");
        let original_mtime = fs::metadata(&document).unwrap().modified().unwrap();
        let mut cache = SkillFactsCache::default();
        let first = discover_skill_candidates_cached(temp.path(), &[], &mut cache);
        assert!(first.candidates[0].frontmatter.is_some());

        let original = fs::read(&document).unwrap();
        let mut malformed = original.clone();
        let closing_fence = malformed
            .windows(4)
            .position(|bytes| bytes == b"---\n")
            .unwrap();
        malformed[closing_fence..closing_fence + 3].copy_from_slice(b"xxx");
        fs::write(&document, malformed).unwrap();
        fs::File::open(&document)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original_mtime))
            .unwrap();

        let second = discover_skill_candidates_cached(temp.path(), &[], &mut cache);
        assert!(second.candidates[0].frontmatter.is_none());
        assert_eq!(cache.last_pass_stats().0, 0);
    }

    #[test]
    fn ordinary_candidate_reports_a_failed_manifest_once() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let skill = home.join(".claude/skills/alpha");
        write_skill(&skill, "alpha");
        let manifest = skill.join("plugin.json");
        fs::create_dir(&manifest).unwrap();

        let report = discover_skill_candidates(&home, &[]);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report
                .read_issues
                .iter()
                .filter(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest
                    && issue.path == manifest.to_string_lossy())
                .count(),
            1
        );
    }

    #[test]
    fn named_discovery_does_not_read_unrequested_plugin_content() {
        let temp = tempfile::tempdir().unwrap();
        write_skill(&temp.path().join(".claude/skills/alpha"), "alpha");
        let plugin = temp.path().join(".claude/plugins/cache/market/plugin/v1");
        let unrelated = plugin.join("skills/unrelated");
        write_skill(&unrelated, "unrelated");
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture-plugin"}"#).unwrap();
        fs::File::create(unrelated.join("large-resource"))
            .unwrap()
            .set_len(MAX_FOLDER_BYTES + 1)
            .unwrap();
        let names = std::collections::BTreeSet::from(["alpha".to_string()]);
        let report = discover_named_skill_candidates_cached(
            temp.path(),
            &[],
            &names,
            &mut SkillFactsCache::default(),
        );
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].name, "alpha");
        assert_only_git_edge_issues(&report.read_issues);
        let full = discover_skill_candidates(temp.path(), &[]);
        assert!(full
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Cap));
    }

    #[test]
    #[cfg(unix)]
    fn symlink_resolution_failure_retains_candidate_and_reports_issue() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".claude/skills");
        fs::create_dir_all(&root).unwrap();
        let link = root.join("loop");
        std::os::unix::fs::symlink("loop", &link).unwrap();

        let report = discover_skill_candidates(temp.path(), &[]);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.path == link && candidate.symlink_error.is_some()));
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::Metadata
                && issue.path == link.to_string_lossy()));
    }

    #[test]
    fn failed_named_document_read_evicts_only_affected_facts_and_recovery_is_fresh() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join(".claude/skills/alpha");
        write_skill(&skill, "alpha");
        let unrelated = temp.path().join(".claude/skills/beta");
        write_skill(&unrelated, "beta");
        let unrelated_key = fs::canonicalize(&unrelated).unwrap();
        let names = std::collections::BTreeSet::from(["alpha".to_string()]);
        let document = skill.join("SKILL.md");
        let mut cache = SkillFactsCache::default();
        let first = discover_skill_candidates_cached(temp.path(), &[], &mut cache);
        assert_eq!(first.candidates.len(), 2);

        fs::remove_file(&document).unwrap();
        fs::create_dir(&document).unwrap();
        let failed = discover_named_skill_candidates_cached(temp.path(), &[], &names, &mut cache);
        assert!(failed.candidates.is_empty());
        assert!(failed
            .read_issues
            .iter()
            .any(|issue| issue.kind == DiscoveryReadIssueKind::SkillDocument));
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&unrelated_key));

        fs::remove_dir(&document).unwrap();
        write_skill(&skill, "alpha");
        let recovered =
            discover_named_skill_candidates_cached(temp.path(), &[], &names, &mut cache);
        assert_eq!(recovered.candidates.len(), 1);
        assert_only_git_edge_issues(&recovered.read_issues);
        assert_eq!(cache.last_pass_stats().0, 0);
    }

    #[test]
    #[cfg(unix)]
    fn named_alias_retarget_evicts_affected_facts_and_keeps_unrelated_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        let outside = tmp.path().join("outside");
        write_skill(&first, "alias");
        fs::create_dir_all(&second).unwrap();
        write_skill(&outside, "alias");
        write_skill(&home.join(".codex/skills/other"), "other");
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let alias = home.join(".claude/skills/alias");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let context = SkillDiscoveryReadContext::bind(
            home.clone(),
            Vec::new(),
            vec![first.clone(), second.clone()],
            Vec::new(),
        );
        let warm_names =
            std::collections::BTreeSet::from(["alias".to_string(), "other".to_string()]);
        let names = std::collections::BTreeSet::from(["alias".to_string()]);
        let first_key = fs::canonicalize(&first).unwrap();
        let second_key = fs::canonicalize(&second).unwrap();
        let unrelated_key = fs::canonicalize(home.join(".codex/skills/other")).unwrap();
        let mut cache = SkillFactsCache::default();
        super::discover_named_skill_candidates_cached(&context, &warm_names, &mut cache);
        assert!(cache.entries.contains_key(&first_key));
        assert!(cache.entries.contains_key(&unrelated_key));

        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        super::discover_named_skill_candidates_cached(&context, &names, &mut cache);
        assert!(!cache.entries.contains_key(&first_key));
        assert!(cache.entries.contains_key(&unrelated_key));

        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        super::discover_named_skill_candidates_cached(&context, &names, &mut cache);
        assert!(cache.entries.contains_key(&first_key));

        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        let report = super::discover_named_skill_candidates_cached(&context, &names, &mut cache);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| { candidate.name == "alias" && candidate.content_hash.is_empty() }));
        assert!(!cache.entries.contains_key(&first_key));
        assert!(!cache.entries.contains_key(&second_key));
        assert!(cache.entries.contains_key(&unrelated_key));
        assert_eq!(
            cache.lexical_keys.get(&alias).map(|entry| &entry.physical),
            Some(&second_key)
        );
    }

    #[test]
    #[cfg(unix)]
    fn full_scan_prunes_a_dead_alias_when_its_physical_entry_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backing = tmp.path().join("backing");
        write_skill(&backing, "alias");
        let first_alias = home.join(".claude/skills/alias");
        let surviving_alias = home.join(".codex/skills/alias");
        fs::create_dir_all(first_alias.parent().unwrap()).unwrap();
        fs::create_dir_all(surviving_alias.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&backing, &first_alias).unwrap();
        std::os::unix::fs::symlink(&backing, &surviving_alias).unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home, Vec::new(), vec![backing.clone()], Vec::new());
        let physical_key = fs::canonicalize(&backing).unwrap();
        let mut cache = SkillFactsCache::default();
        super::discover_skill_candidates_cached(&context, &mut cache);
        assert!(cache.lexical_keys.contains_key(&first_alias));
        assert!(cache.lexical_keys.contains_key(&surviving_alias));

        fs::remove_file(&first_alias).unwrap();
        super::discover_skill_candidates_cached(&context, &mut cache);

        assert!(cache.entries.contains_key(&physical_key));
        assert!(!cache.lexical_keys.contains_key(&first_alias));
        assert!(cache.lexical_keys.contains_key(&surviving_alias));
    }

    #[test]
    fn partial_scope_retains_home_plugins_and_reports_missing_and_failed_projects() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let plugin = home.join(".claude/plugins/cache/market/plugin/1");
        write_skill(&plugin.join("skills/alpha"), "alpha");
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let missing = temp.path().join("missing-project");
        let failed = temp.path().join("project-file");
        fs::write(&failed, "not a directory").unwrap();
        let context = SkillDiscoveryReadContext::bind(
            home,
            vec![missing.clone(), failed.clone()],
            Vec::new(),
            Vec::new(),
        );

        assert!(matches!(
            &context.bind_outcomes()[0],
            crate::skill_scope::RootBindOutcome::Bound { .. }
        ));
        assert!(matches!(
            &context.bind_outcomes()[1],
            crate::skill_scope::RootBindOutcome::Missing { requested, .. } if requested == &missing
        ));
        assert!(matches!(
            &context.bind_outcomes()[2],
            crate::skill_scope::RootBindOutcome::Failed { requested, .. } if requested == &failed
        ));
        let report = super::discover_skill_candidates(&context);
        assert!(report.candidates.iter().any(|candidate| {
            candidate.name == "alpha" && matches!(candidate.plugin, PluginEvidence::Confirmed(_))
        }));
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.path == missing.to_string_lossy()));
        assert!(report
            .read_issues
            .iter()
            .any(|issue| issue.path == failed.to_string_lossy()));
    }

    #[test]
    fn full_and_named_plugin_candidates_match_without_evicting_other_plugin_facts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let plugin = home.join(".codex/plugins/cache/market/plugin/1");
        for name in ["alpha", "beta"] {
            write_skill(&plugin.join("skills").join(name), name);
        }
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let context = SkillDiscoveryReadContext::bind(home, Vec::new(), Vec::new(), Vec::new());
        let mut cache = SkillFactsCache::default();
        let full = super::discover_skill_candidates_cached(&context, &mut cache);
        let beta_key = fs::canonicalize(plugin.join("skills/beta")).unwrap();
        assert!(cache.entries.contains_key(&beta_key));
        let names = std::collections::BTreeSet::from(["alpha".to_string()]);
        let named = super::discover_named_skill_candidates_cached(&context, &names, &mut cache);
        let full_alpha = full
            .candidates
            .iter()
            .find(|candidate| candidate.name == "alpha")
            .unwrap();
        assert_eq!(named.candidates.len(), 1);
        assert_eq!(named.candidates[0].path, full_alpha.path);
        assert!(matches!(
            named.candidates[0].plugin,
            PluginEvidence::Confirmed(_)
        ));
        assert!(cache.entries.contains_key(&beta_key));
    }

    fn content_guard(
        scope: &SkillReadScope,
        root: &Path,
        paths: &[PathBuf],
    ) -> crate::skill_coordination::CoordinatedReadGuard {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        CoordinationPlan::new_fixture(
            vec![DirectoryEffect::tree(root, CoordinationMode::Shared)],
            root,
            Some(Duration::from_secs(10)),
        )
        .unwrap()
        .acquire()
        .unwrap()
        .continue_with_files(scope, paths, CoordinationMode::Shared)
        .unwrap()
    }

    #[test]
    fn guarded_prepared_content_matches_scoped_reads_for_regular_and_hard_linked_resources() {
        for hard_link in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            let resource = skill.join("resource.txt");
            fs::write(&resource, "resource content").unwrap();
            if hard_link {
                fs::hard_link(&resource, temp.path().join("outside-alias")).unwrap();
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let (mut expected_walk, expected_bytes, expected_truncated) =
                PreparedSkillContent::enumerate(&scope, &skill)
                    .unwrap()
                    .materialize_prefix((&scope).into())
                    .unwrap();
            let expected_hash = content_hash_with_check_mode(
                (&scope).into(),
                &mut expected_walk.hashable,
                MAX_FOLDER_BYTES,
                &mut || Ok(()),
                true,
                None,
            )
            .unwrap();
            let prepared = PreparedSkillContent::enumerate(&scope, &skill).unwrap();
            let guard = content_guard(&scope, temp.path(), &[skill.join("SKILL.md"), resource]);
            let reader = SkillContentRead {
                scope: &scope,
                guard: Some(&guard),
            };
            let (mut walk, bytes, truncated) = prepared.materialize_prefix(reader).unwrap();
            let hash = content_hash_with_check_mode(
                reader,
                &mut walk.hashable,
                MAX_FOLDER_BYTES,
                &mut || Ok(()),
                true,
                None,
            )
            .unwrap();
            assert_eq!(bytes, expected_bytes);
            assert_eq!(truncated, expected_truncated);
            assert_eq!(hash, expected_hash);
            guard.revalidate(&scope).unwrap();
        }
    }

    #[test]
    fn guarded_materialization_rejects_a_plan_older_than_its_guard() {
        for change_document in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            let document = skill.join("SKILL.md");
            let resource = skill.join("resource.txt");
            fs::write(&resource, "original resource").unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let prepared = PreparedSkillContent::enumerate(&scope, &skill).unwrap();
            fs::write(
                if change_document {
                    &document
                } else {
                    &resource
                },
                "changed",
            )
            .unwrap();
            let guard = content_guard(&scope, temp.path(), &[document, resource]);
            let reader = SkillContentRead {
                scope: &scope,
                guard: Some(&guard),
            };
            let result = prepared.materialize_prefix(reader);
            if change_document {
                assert!(matches!(result, Err(FactsWalkError::Failed(_))));
            } else {
                let (mut walk, _, _) = result.unwrap();
                assert!(content_hash_with_check_mode(
                    reader,
                    &mut walk.hashable,
                    MAX_FOLDER_BYTES,
                    &mut || Ok(()),
                    true,
                    None
                )
                .is_err());
            }
        }
    }

    #[test]
    fn guarded_hash_cannot_read_a_resource_missing_from_the_guard() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        fs::write(skill.join("resource.txt"), "not planned").unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let prepared = PreparedSkillContent::enumerate(&scope, &skill).unwrap();
        let guard = content_guard(&scope, temp.path(), &[skill.join("SKILL.md")]);
        let reader = SkillContentRead {
            scope: &scope,
            guard: Some(&guard),
        };
        let (mut walk, _, _) = prepared.materialize_prefix(reader).unwrap();
        assert!(content_hash_with_check_mode(
            reader,
            &mut walk.hashable,
            MAX_FOLDER_BYTES,
            &mut || Ok(()),
            true,
            None
        )
        .is_err());
    }

    #[test]
    #[ignore = "release discovery benchmark; run with isolated fixture size and layout"]
    fn measure_discovery_scan_scale() {
        fn usage() -> (f64, u64) {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            assert_eq!(
                unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
                0
            );
            let usage = unsafe { usage.assume_init() };
            let cpu = usage.ru_utime.tv_sec as f64
                + usage.ru_utime.tv_usec as f64 / 1_000_000.0
                + usage.ru_stime.tv_sec as f64
                + usage.ru_stime.tv_usec as f64 / 1_000_000.0;
            let peak = usage.ru_maxrss as u64;
            (
                cpu,
                if cfg!(target_os = "macos") {
                    peak
                } else {
                    peak * 1024
                },
            )
        }
        let count: usize = std::env::var("SKILL_STUDIO_BENCH_SKILLS")
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=10_000).contains(&count));
        let layout = std::env::var("SKILL_STUDIO_BENCH_LAYOUT").unwrap();
        assert!(matches!(layout.as_str(), "direct" | "aliases"));
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(if layout == "direct" {
            ".claude/skills"
        } else {
            ".agents/skills"
        });
        for index in 0..count {
            let name = format!("skill{index:05}");
            let skill = root.join(&name);
            fs::create_dir_all(&skill).unwrap();
            let body =
                "Use this fixture to inspect files and report a deterministic result.\n".repeat(64);
            fs::write(
                skill.join("SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: A discovery benchmark fixture.\n---\n{body}"
                ),
            )
            .unwrap();
            for resource in ["one.txt", "two.txt"] {
                fs::write(skill.join(resource), "resource fixture\n".repeat(8)).unwrap();
            }
            if layout == "aliases" {
                for agent in [".claude/skills", ".codex/skills"] {
                    let alias = home.join(agent).join(&name);
                    fs::create_dir_all(alias.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(&skill, &alias).unwrap();
                }
            }
        }
        let context = read_context(home, &[]);
        assert!(tokenizer().is_some());
        let mut cache = SkillFactsCache::default();
        let names = BTreeSet::from(["skill00000".to_string()]);
        let deployments_per_skill = if layout == "aliases" { 3 } else { 1 };
        for (phase, samples) in [("cold_facts_cache", 1), ("warm_full", 3), ("warm_named", 3)] {
            for sample in 0..samples {
                let (cpu_before, _) = usage();
                let started = std::time::Instant::now();
                let report = if phase == "warm_named" {
                    super::discover_named_skill_candidates_cached(&context, &names, &mut cache)
                } else {
                    super::discover_skill_candidates_cached(&context, &mut cache)
                };
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let (cpu_after, peak_rss_bytes) = usage();
                let expected = if phase == "warm_named" {
                    deployments_per_skill
                } else {
                    count * deployments_per_skill
                };
                assert_eq!(report.candidates.len(), expected);
                let (cache_hits, cache_total) = cache.last_pass_stats();
                println!(
                    "SKILL_STUDIO_DISCOVERY_BENCH {}",
                    serde_json::json!({
                        "skills": count, "layout": layout, "phase": phase, "sample": sample,
                        "elapsed_ms": elapsed_ms, "cpu_ms": (cpu_after - cpu_before) * 1000.0,
                        "process_peak_rss_bytes": peak_rss_bytes, "candidates": report.candidates.len(),
                        "issues": report.read_issues.len(), "cache_hits": cache_hits, "cache_total": cache_total,
                    })
                );
            }
        }
    }

    #[test]
    fn shared_manifest_validation_is_fresh_after_materialization_and_each_scan() {
        for named in [false, true] {
            for change in ["appear", "replace", "remove", "outside-link"] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path().join("home");
                for name in ["alpha", "beta"] {
                    let skill = home.join(".agents/skills").join(name);
                    write_skill(&skill, name);
                    let alias = home.join(".claude/skills").join(name);
                    fs::create_dir_all(alias.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(&skill, alias).unwrap();
                }
                let manifest = home.join(".agents/plugin.json");
                if change != "appear" {
                    fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
                }
                let context = read_context(&home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let selection = named.then_some(&names);
                let mut cache = SkillFactsCache::default();
                let (report, proof) = PreparedDiscovery::enumerate(&context, selection)
                    .materialize_with_proof(&mut cache, None);
                assert_eq!(report.candidates.len(), if named { 2 } else { 4 });
                proof.revalidate(context.read_scope()).unwrap();
                match change {
                    "appear" | "replace" => fs::write(&manifest, r#"{"name":"after"}"#).unwrap(),
                    "remove" => fs::remove_file(&manifest).unwrap(),
                    "outside-link" => {
                        let outside = temp.path().join("outside.json");
                        fs::write(&outside, r#"{"name":"outside"}"#).unwrap();
                        fs::remove_file(&manifest).unwrap();
                        std::os::unix::fs::symlink(outside, &manifest).unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(
                    matches!(
                        proof.revalidate(context.read_scope()),
                        Err(crate::skill_coordination::CoordinationFailure::Changed)
                    ),
                    "{change}, named={named}"
                );
                let fresh =
                    PreparedDiscovery::enumerate(&context, selection).materialize(&mut cache);
                assert_eq!(fresh.candidates.len(), if named { 2 } else { 4 });
                for candidate in &fresh.candidates {
                    match change {
                        "appear" | "replace" => assert!(
                            matches!(&candidate.plugin, PluginEvidence::Confirmed(info) if info.name == "after")
                        ),
                        "remove" => {
                            assert!(!matches!(&candidate.plugin, PluginEvidence::Confirmed(_)))
                        }
                        "outside-link" => {
                            assert!(matches!(&candidate.plugin, PluginEvidence::Unknown))
                        }
                        _ => unreachable!(),
                    }
                }
                if change == "outside-link" {
                    assert!(!fresh.read_issues.is_empty());
                }
            }
        }
    }

    #[test]
    fn nested_resource_changes_invalidate_prepared_and_assembled_reads() {
        for plugin_skill in [false, true] {
            for named in [false, true] {
                for before_materialization in [false, true] {
                    for change in ["unchanged", "add", "remove", "replace"] {
                        let temp = tempfile::tempdir().unwrap();
                        let home = temp.path();
                        let skill = if plugin_skill {
                            home.join(".claude/plugins/cache/plugin/skills/alpha")
                        } else {
                            home.join("backing/alpha")
                        };
                        write_skill(&skill, "alpha");
                        if plugin_skill {
                            fs::write(
                                home.join(".claude/plugins/cache/plugin/plugin.json"),
                                r#"{"name":"plugin"}"#,
                            )
                            .unwrap();
                        } else {
                            let alias = home.join(".claude/skills/alpha");
                            fs::create_dir_all(alias.parent().unwrap()).unwrap();
                            std::os::unix::fs::symlink(&skill, &alias).unwrap();
                        }
                        let nested = skill.join("resources/nested");
                        fs::create_dir_all(&nested).unwrap();
                        fs::write(nested.join("existing.txt"), "resource").unwrap();
                        let mutate = || match change {
                            "add" => fs::write(nested.join("new.txt"), "new").unwrap(),
                            "remove" => fs::remove_file(nested.join("existing.txt")).unwrap(),
                            "replace" => {
                                fs::rename(&nested, skill.join("resources/old")).unwrap();
                                fs::create_dir(&nested).unwrap();
                                fs::write(nested.join("existing.txt"), "resource").unwrap();
                            }
                            _ => {}
                        };
                        let context = read_context(home, &[]);
                        let names = BTreeSet::from(["alpha".to_string()]);
                        let mut cache = SkillFactsCache::default();
                        PreparedDiscovery::enumerate(&context, None).materialize(&mut cache);
                        let prepared =
                            PreparedDiscovery::enumerate(&context, named.then_some(&names));
                        if before_materialization {
                            mutate();
                        }
                        let (report, proof) = prepared.materialize_with_proof(&mut cache, None);
                        if before_materialization && change != "unchanged" {
                            assert!(report
                                .read_issues
                                .iter()
                                .any(|issue| issue.kind == DiscoveryReadIssueKind::Resource));
                            assert!(cache.entries.is_empty());
                        } else {
                            let inventory = crate::skill_assembly::assemble_installed_skills(
                                report.candidates,
                                &crate::skill_lock_file::empty_lock_file(),
                                &crate::skill_ownership::OwnershipReadReport::empty(),
                                &Default::default(),
                            );
                            assert_eq!(inventory.len(), 1);
                            assert!(proof.revalidate(context.read_scope()).is_ok());
                            if !before_materialization {
                                mutate();
                            }
                            assert_eq!(
                                proof.revalidate(context.read_scope()).is_ok(),
                                change == "unchanged"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn retained_spec_evidence_rejects_late_changes_for_linked_and_plugin_skills() {
        for plugin_skill in [false, true] {
            for marker in ["absent", "file", "directory"] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let skill = if plugin_skill {
                    home.join(".claude/plugins/cache/plugin/skills/alpha")
                } else {
                    home.join("backing/alpha")
                };
                write_skill(&skill, "alpha");
                if plugin_skill {
                    fs::write(
                        home.join(".claude/plugins/cache/plugin/plugin.json"),
                        r#"{"name":"plugin"}"#,
                    )
                    .unwrap();
                } else {
                    let alias = home.join(".claude/skills/alpha");
                    fs::create_dir_all(alias.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(&skill, &alias).unwrap();
                }
                if marker == "file" {
                    fs::write(skill.join("spec.md"), "before").unwrap();
                }
                if marker == "directory" {
                    fs::create_dir(skill.join("evals")).unwrap();
                }
                let context = read_context(home, &[]);
                let (report, proof) = PreparedDiscovery::enumerate(&context, None)
                    .materialize_with_proof(&mut SkillFactsCache::default(), None);
                let inventory = crate::skill_assembly::assemble_installed_skills(
                    report.candidates,
                    &crate::skill_lock_file::empty_lock_file(),
                    &crate::skill_ownership::OwnershipReadReport::empty(),
                    &Default::default(),
                );
                assert_eq!(inventory.len(), 1);
                assert!(proof.revalidate(context.read_scope()).is_ok());
                match marker {
                    "file" => fs::write(skill.join("spec.md"), "changed after assembly").unwrap(),
                    "directory" => fs::create_dir(skill.join("evals/new-case")).unwrap(),
                    _ => fs::create_dir(skill.join("evals")).unwrap(),
                }
                assert!(proof.revalidate(context.read_scope()).is_err());
            }
        }
    }

    #[test]
    fn agent_entry_proof_rejects_late_document_target_and_ancestry_changes() {
        for named in [false, true] {
            for change in [
                "unchanged",
                "document",
                "missing_document",
                "retarget",
                "repair_target",
                "manifest",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let first = home.join("first/skills/alpha");
                let second = home.join("second/skills/alpha");
                write_skill(&second, "alpha");
                fs::create_dir_all(first.parent().unwrap()).unwrap();
                if change != "repair_target" {
                    write_skill(&first, "alpha");
                }
                if change == "missing_document" {
                    fs::remove_file(first.join("SKILL.md")).unwrap();
                }
                let manifest = home.join("first/plugin.json");
                fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
                let alias = home.join(".claude/skills/alpha");
                fs::create_dir_all(alias.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&first, &alias).unwrap();
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let (report, proof) =
                    PreparedDiscovery::enumerate(&context, named.then_some(&names))
                        .materialize_with_proof(&mut SkillFactsCache::default(), None);
                let _inventory = crate::skill_assembly::assemble_installed_skills(
                    report.candidates,
                    &crate::skill_lock_file::empty_lock_file(),
                    &crate::skill_ownership::OwnershipReadReport::empty(),
                    &Default::default(),
                );
                assert!(proof.revalidate(context.read_scope()).is_ok());
                match change {
                    "document" | "missing_document" => {
                        fs::write(first.join("SKILL.md"), "changed after assembly").unwrap()
                    }
                    "retarget" => {
                        fs::remove_file(&alias).unwrap();
                        std::os::unix::fs::symlink(&second, &alias).unwrap();
                    }
                    "repair_target" => write_skill(&first, "alpha"),
                    "manifest" => fs::write(&manifest, r#"{"name":"changed"}"#).unwrap(),
                    _ => {}
                }
                assert_eq!(
                    proof.revalidate(context.read_scope()).is_ok(),
                    change == "unchanged"
                );
            }
        }
    }

    #[test]
    fn agent_root_proof_survives_assembly_in_full_and_named_scans() {
        for named in [false, true] {
            for change in [
                "unchanged",
                "new_root",
                "new_disabled_root",
                "new_entry",
                "replace_root",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let root = home.join(".claude/skills");
                write_skill(&root.join("alpha"), "alpha");
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let (report, proof) =
                    PreparedDiscovery::enumerate(&context, named.then_some(&names))
                        .materialize_with_proof(&mut SkillFactsCache::default(), None);
                let inventory = crate::skill_assembly::assemble_installed_skills(
                    report.candidates,
                    &crate::skill_lock_file::empty_lock_file(),
                    &crate::skill_ownership::OwnershipReadReport::empty(),
                    &Default::default(),
                );
                assert_eq!(inventory.len(), 1);
                assert!(proof.revalidate(context.read_scope()).is_ok());
                match change {
                    "new_root" => write_skill(&home.join(".codex/skills/alpha"), "alpha"),
                    "new_disabled_root" => {
                        write_skill(&root.join(STUDIO_DISABLED_DIR_NAME).join("alpha"), "alpha")
                    }
                    "new_entry" => write_skill(&root.join("beta"), "beta"),
                    "replace_root" => {
                        fs::rename(&root, home.join("old-skills")).unwrap();
                        write_skill(&root.join("alpha"), "replacement");
                    }
                    _ => {}
                }
                assert_eq!(
                    proof.revalidate(context.read_scope()).is_ok(),
                    change == "unchanged"
                );
            }
        }
    }

    #[test]
    fn plugin_proof_survives_inventory_assembly_and_detects_late_changes() {
        for initial_failure in [false, true] {
            for change in [
                "unchanged",
                "manifest",
                "skill_directory",
                "document",
                "missing_cache",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let plugin = home.join(".claude/plugins/cache/plugin");
                let skill = plugin.join("skills/alpha");
                write_skill(&skill, "alpha");
                fs::write(plugin.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
                if initial_failure {
                    fs::create_dir_all(home.join(".codex/plugins")).unwrap();
                    fs::write(home.join(".codex/plugins/cache"), "not a directory").unwrap();
                }
                let context = read_context(home, &[]);
                let (report, proof) = PreparedDiscovery::enumerate(&context, None)
                    .materialize_with_proof(&mut SkillFactsCache::default(), None);
                let inventory = crate::skill_assembly::assemble_installed_skills(
                    report.candidates,
                    &crate::skill_lock_file::empty_lock_file(),
                    &crate::skill_ownership::OwnershipReadReport::empty(),
                    &Default::default(),
                );
                assert_eq!(inventory.len(), 1);
                assert!(proof.revalidate(context.read_scope()).is_ok());
                match change {
                    "manifest" => {
                        fs::write(plugin.join("plugin.json"), r#"{"name":"changed"}"#).unwrap()
                    }
                    "skill_directory" => fs::create_dir(plugin.join("skills/new-skill")).unwrap(),
                    "document" => {
                        fs::write(skill.join("SKILL.md"), "changed after assembly").unwrap()
                    }
                    "missing_cache" => {
                        fs::create_dir_all(home.join(".cursor/plugins/cache/new-plugin")).unwrap()
                    }
                    _ => {}
                }
                assert_eq!(
                    proof.revalidate(context.read_scope()).is_ok(),
                    change == "unchanged"
                );
            }
        }
    }

    #[test]
    fn coordinated_discovery_matches_scoped_content_for_full_named_and_warm_scans() {
        for named in [false, true] {
            for hard_linked in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let shared = home.join(".agents/skills/alpha");
                let alias = home.join(".claude/skills/alpha");
                let plugin = home.join(".codex/plugins/cache/plugin");
                let plugin_skill = plugin.join("skills/alpha");
                for skill in [&shared, &plugin_skill] {
                    write_skill(skill, "alpha");
                    fs::write(skill.join("spec.md"), "spec").unwrap();
                    let resource = skill.join("resource.txt");
                    fs::write(&resource, "resource").unwrap();
                    if hard_linked {
                        fs::hard_link(&resource, skill.join("resource-alias.txt")).unwrap();
                    }
                }
                fs::create_dir_all(alias.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&shared, &alias).unwrap();
                write_skill(&home.join(".codex/skills/beta"), "beta");
                for root in [home.join(".agents"), plugin] {
                    fs::write(root.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
                }
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let extent = named.then_some(&names);
                let expected = PreparedDiscovery::enumerate(&context, extent)
                    .materialize(&mut SkillFactsCache::default());
                let normalize = |report: &DiscoveryReport| {
                    report
                        .candidates
                        .iter()
                        .map(|candidate| {
                            (
                                candidate.path.clone(),
                                candidate.content_hash.clone(),
                                candidate.has_spec,
                                format!("{:?}", candidate.plugin),
                            )
                        })
                        .collect::<Vec<_>>()
                };
                let mut cache = SkillFactsCache::default();
                for _ in 0..2 {
                    cache.begin_pass();
                    let guard = content_guard(context.read_scope(), home, &[]);
                    let (plan, guard) =
                        PreparedDiscovery::enumerate_coordinated(&context, extent, guard).unwrap();
                    let actual = plan.materialize_guarded(&mut cache, Some(&guard));
                    assert_eq!(normalize(&actual), normalize(&expected));
                    assert_eq!(
                        format!("{:?}", actual.source_coverage),
                        format!("{:?}", expected.source_coverage)
                    );
                    assert_eq!(actual.read_issues.len(), expected.read_issues.len());
                    guard.revalidate(context.read_scope()).unwrap();
                    if named {
                        cache.end_named_pass(&names);
                    } else {
                        cache.end_pass();
                    }
                }
                assert_eq!(cache.last_pass_stats().0, if named { 3 } else { 4 });
            }
        }
    }

    #[test]
    fn plugin_manifest_proof_rejects_identity_changes_after_preparation() {
        for named in [false, true] {
            for change in ["unchanged", "rewrite", "higher_priority", "remove"] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let plugin = home.join(".claude/plugins/cache/plugin");
                let skill = plugin.join("skills/alpha");
                write_skill(&skill, "alpha");
                fs::create_dir(plugin.join(".claude-plugin")).unwrap();
                let manifest = plugin.join("plugin.json");
                fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let plan = PreparedDiscovery::enumerate(&context, named.then_some(&names));
                match change {
                    "rewrite" => fs::write(&manifest, r#"{"name":"after!"}"#).unwrap(),
                    "higher_priority" => fs::write(
                        plugin.join(".claude-plugin/plugin.json"),
                        r#"{"name":"higher"}"#,
                    )
                    .unwrap(),
                    "remove" => fs::remove_file(&manifest).unwrap(),
                    _ => {}
                }
                let mut cache = SkillFactsCache::default();
                let report = plan.materialize(&mut cache);
                if change == "unchanged" {
                    assert_eq!(report.candidates.len(), 1);
                    assert!(
                        matches!(&report.candidates[0].plugin, PluginEvidence::Confirmed(info) if info.name == "before")
                    );
                } else {
                    assert!(report.candidates.is_empty());
                    assert!(cache.entries.is_empty());
                    assert!(report
                        .read_issues
                        .iter()
                        .any(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest));
                    let coverage = report
                        .source_coverage
                        .iter()
                        .find(|coverage| coverage.path == home.join(".claude/plugins/cache"))
                        .unwrap();
                    assert_eq!(coverage.membership, RootReadOutcome::Incomplete);
                }
            }
        }
    }

    #[test]
    fn plugin_tree_proof_survives_the_content_handoff() {
        for named in [false, true] {
            for change in ["unchanged", "sibling", "skill_sibling", "replace", "remove"] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let cache_root = home.join(".claude/plugins/cache");
                let plugin = cache_root.join("plugin");
                let affected = plugin.join("skills/alpha");
                let stable_plugin = home.join(".codex/plugins/cache/plugin");
                let stable = stable_plugin.join("skills/alpha");
                let agent = home.join(".claude/skills/alpha");
                for skill in [&affected, &stable, &agent] {
                    write_skill(skill, "alpha");
                }
                for root in [&plugin, &stable_plugin] {
                    fs::write(root.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
                }
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let plan = PreparedDiscovery::enumerate(&context, named.then_some(&names));
                match change {
                    "sibling" => fs::create_dir(cache_root.join("new-plugin")).unwrap(),
                    "skill_sibling" => fs::create_dir(plugin.join("skills/new-skill")).unwrap(),
                    "replace" => {
                        fs::rename(&plugin, home.join("old-plugin")).unwrap();
                        write_skill(&affected, "replacement");
                        fs::write(plugin.join("plugin.json"), r#"{"name":"replacement"}"#).unwrap();
                    }
                    "remove" => fs::remove_dir_all(&plugin).unwrap(),
                    _ => {}
                }
                let mut cache = SkillFactsCache::default();
                let report = plan.materialize(&mut cache);
                assert!(report
                    .candidates
                    .iter()
                    .any(|candidate| candidate.path == agent));
                assert!(report
                    .candidates
                    .iter()
                    .any(|candidate| candidate.path == stable));
                assert_eq!(
                    report
                        .candidates
                        .iter()
                        .any(|candidate| candidate.path == affected),
                    change == "unchanged"
                );
                let coverage = report
                    .source_coverage
                    .iter()
                    .find(|coverage| coverage.path == cache_root)
                    .unwrap();
                assert_eq!(
                    coverage.membership,
                    if change == "unchanged" {
                        RootReadOutcome::Read
                    } else {
                        RootReadOutcome::Incomplete
                    }
                );
                if change == "sibling" || change == "skill_sibling" {
                    assert!(!cache
                        .entries
                        .contains_key(&fs::canonicalize(&affected).unwrap()));
                }
            }
        }
    }

    #[test]
    fn collected_discovery_rejects_agent_and_plugin_content_drift() {
        for named in [false, true] {
            for change in ["unchanged", "document", "resource"] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let plugin = home.join(".claude/plugins/cache/plugin");
                let selected = [
                    home.join(".claude/skills/alpha"),
                    plugin.join("skills/alpha"),
                ];
                for skill in &selected {
                    write_skill(skill, "alpha");
                    fs::write(skill.join("resource.txt"), "before").unwrap();
                }
                write_skill(&home.join(".claude/skills/other"), "other");
                write_skill(&plugin.join("skills/other"), "other");
                fs::write(plugin.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let plan = PreparedDiscovery::enumerate(&context, named.then_some(&names));
                if change != "unchanged" {
                    for skill in &selected {
                        let file = if change == "document" {
                            "SKILL.md"
                        } else {
                            "resource.txt"
                        };
                        fs::write(skill.join(file), "changed after complete preparation").unwrap();
                    }
                }
                let mut cache = SkillFactsCache::default();
                let report = plan.materialize(&mut cache);
                assert_eq!(
                    report.extent,
                    if named {
                        DiscoveryExtent::Named
                    } else {
                        DiscoveryExtent::Full
                    }
                );
                for skill in &selected {
                    let candidate = report
                        .candidates
                        .iter()
                        .find(|candidate| &candidate.path == skill);
                    if change == "document" {
                        assert!(candidate.is_none());
                    } else {
                        assert!(candidate.is_some());
                    }
                    let coverage = report
                        .source_coverage
                        .iter()
                        .find(|coverage| {
                            skill.starts_with(&coverage.path)
                                && matches!(
                                    coverage.source,
                                    MembershipSource::AgentRoot | MembershipSource::PluginCache
                                )
                        })
                        .unwrap();
                    if change == "unchanged" {
                        assert_eq!(coverage.membership, RootReadOutcome::Read);
                        assert!(cache
                            .entries
                            .contains_key(&fs::canonicalize(skill).unwrap()));
                    } else {
                        assert_eq!(coverage.facts, RootReadOutcome::Incomplete);
                        assert!(!cache
                            .entries
                            .contains_key(&fs::canonicalize(skill).unwrap()));
                        assert!(report
                            .read_issues
                            .iter()
                            .any(|issue| Path::new(&issue.path).starts_with(skill)));
                    }
                }
                assert_eq!(
                    report
                        .candidates
                        .iter()
                        .filter(|candidate| candidate.name == "other")
                        .count(),
                    if named { 0 } else { 2 }
                );
            }
        }
    }

    #[test]
    fn plugin_content_uses_membership_observations_instead_of_adopting_changes() {
        for change in ["document", "entry", "resource", "unchanged"] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path();
            let plugin_root = home.join(".claude/plugins/cache/plugin");
            let skill = plugin_root.join("skills/alpha");
            write_skill(&skill, "alpha");
            let resource = skill.join("resource.txt");
            fs::write(&resource, "before").unwrap();
            fs::write(plugin_root.join("plugin.json"), r#"{"name":"plugin"}"#).unwrap();
            let context = read_context(home, &[]);
            let report = plugins::scan_plugin_skills(&context, None);
            assert_eq!(report.skills.len(), 1);
            let plugin = report.skills.into_iter().next().unwrap();
            match change {
                "document" => {
                    fs::write(skill.join("SKILL.md"), "changed after membership").unwrap()
                }
                "entry" => {
                    fs::rename(&skill, plugin_root.join("old-alpha")).unwrap();
                    write_skill(&skill, "replacement");
                }
                _ => {}
            }
            let prepared = PreparedSkillFacts::enumerate_plugin_checked(
                context.read_scope(),
                &plugin,
                &mut || Ok(()),
            )
            .unwrap();
            if change == "resource" {
                fs::write(&resource, "changed after content preparation").unwrap();
            }
            let mut cache = SkillFactsCache::default();
            let mut issues = Vec::new();
            let result = prepared.materialize(
                context.read_scope().into(),
                &mut cache,
                &skill,
                tokenizer(),
                &mut issues,
            );
            match change {
                "unchanged" => {
                    let facts = result.unwrap().unwrap();
                    assert!(!facts.incomplete);
                    assert!(issues.is_empty());
                    assert_eq!(cache.entries.len(), 1);
                }
                "resource" => {
                    assert!(result.unwrap().unwrap().incomplete);
                    assert!(!issues.is_empty());
                    assert!(cache.entries.is_empty());
                }
                _ => {
                    assert!(result.is_err());
                    assert!(!issues.is_empty());
                    assert!(cache.entries.is_empty());
                }
            }
        }
    }

    #[test]
    fn prepared_spec_markers_preserve_presence_and_file_precedence() {
        for marker in ["absent", "file", "directory"] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            match marker {
                "file" => {
                    fs::write(skill.join("spec.md"), "spec").unwrap();
                    fs::write(skill.join("evals"), "not a directory").unwrap();
                }
                "directory" => fs::create_dir(skill.join("evals")).unwrap(),
                _ => {}
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let plan = PreparedSpecMarkers::enumerate(&scope, &skill);
            let mut issues = Vec::new();
            assert_eq!(
                plan.materialize(&scope, &skill, &mut issues),
                marker != "absent"
            );
            assert!(issues.is_empty());
        }
    }

    #[test]
    fn prepared_spec_markers_reject_appearance_removal_and_replacement() {
        for change in [
            "new-file",
            "new-directory",
            "removed-file",
            "replaced-directory",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            if change == "removed-file" {
                fs::write(skill.join("spec.md"), "spec").unwrap();
            }
            if change == "replaced-directory" {
                fs::create_dir(skill.join("evals")).unwrap();
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let plan = PreparedSpecMarkers::enumerate(&scope, &skill);
            match change {
                "new-file" => fs::write(skill.join("spec.md"), "spec").unwrap(),
                "new-directory" => fs::create_dir(skill.join("evals")).unwrap(),
                "removed-file" => fs::remove_file(skill.join("spec.md")).unwrap(),
                _ => {
                    fs::rename(skill.join("evals"), skill.join("old-evals")).unwrap();
                    fs::create_dir(skill.join("evals")).unwrap();
                }
            }
            let mut issues = Vec::new();
            assert!(!plan.materialize(&scope, &skill, &mut issues));
            assert!(!issues.is_empty());
        }
    }

    #[test]
    fn new_spec_marker_after_preparation_invalidates_warm_cache() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let mut cache = SkillFactsCache::default();
        let mut issues = Vec::new();
        let original = get_or_compute_facts(&scope, &mut cache, &skill, tokenizer(), &mut issues)
            .unwrap()
            .unwrap();
        assert!(!original.has_spec);
        assert!(!original.incomplete);
        let plan = PreparedSkillFacts::enumerate(&scope, &skill);
        fs::create_dir(skill.join("evals")).unwrap();
        let facts = plan
            .materialize(
                (&scope).into(),
                &mut cache,
                &skill,
                tokenizer(),
                &mut issues,
            )
            .unwrap()
            .unwrap();
        assert!(facts.incomplete);
        assert!(!facts.has_spec);
        assert!(!Arc::ptr_eq(&original, &facts));
        assert!(cache.entries.is_empty());
        assert!(!issues.is_empty());
    }

    #[test]
    fn single_entry_ancestry_preserves_precedence_and_external_boundaries() {
        for external in [false, true] {
            for complete in [false, true] {
                for physical_manifest in [false, true] {
                    for lexical_manifest in [false, true] {
                        let temp = tempfile::tempdir().unwrap();
                        let home = temp.path().join("home");
                        let owner = if external {
                            temp.path().join("external")
                        } else {
                            home.join("physical")
                        };
                        let target = owner.join("skills/alpha");
                        write_skill(&target, "alpha");
                        let lexical_owner = home.join(".claude");
                        let entry = lexical_owner.join("skills/alpha");
                        fs::create_dir_all(entry.parent().unwrap()).unwrap();
                        std::os::unix::fs::symlink(&target, &entry).unwrap();
                        if physical_manifest {
                            fs::write(owner.join("plugin.json"), r#"{"name":"physical"}"#).unwrap();
                        }
                        if lexical_manifest {
                            fs::write(lexical_owner.join("plugin.json"), r#"{"name":"lexical"}"#)
                                .unwrap();
                        }
                        let backing = if external { vec![target] } else { Vec::new() };
                        let ownership = if external && complete {
                            vec![owner]
                        } else {
                            Vec::new()
                        };
                        let context =
                            SkillDiscoveryReadContext::bind(home, Vec::new(), backing, ownership);
                        let report = super::discover_skill_candidates(&context);
                        let candidate = report
                            .candidates
                            .iter()
                            .find(|candidate| candidate.path == entry)
                            .unwrap();
                        if physical_manifest && (!external || complete) {
                            assert!(
                                matches!(&candidate.plugin, PluginEvidence::Confirmed(plugin) if plugin.name == "physical")
                            );
                        } else if lexical_manifest {
                            assert!(
                                matches!(&candidate.plugin, PluginEvidence::Confirmed(plugin) if plugin.name == "lexical")
                            );
                        } else if external && !complete {
                            assert!(matches!(candidate.plugin, PluginEvidence::Unknown));
                        } else {
                            assert!(matches!(candidate.plugin, PluginEvidence::Absent));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn retained_agent_ancestry_rejects_changed_or_new_plugin_identity() {
        for linked in [false, true] {
            for initially_present in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let root = home.join(".claude/skills");
                let entry = root.join("alpha");
                let owner = if linked {
                    home.join("package")
                } else {
                    home.join(".claude")
                };
                if linked {
                    let target = owner.join("skills/alpha");
                    write_skill(&target, "alpha");
                    fs::create_dir_all(&root).unwrap();
                    std::os::unix::fs::symlink(&target, &entry).unwrap();
                } else {
                    write_skill(&entry, "alpha");
                }
                let manifest = owner.join("plugin.json");
                if initially_present {
                    fs::write(&manifest, r#"{"name":"before"}"#).unwrap();
                }
                let context = read_context(home, &[]);
                let plan = prepare_agent_roots(&context, None)
                    .into_iter()
                    .find(|plan| plan.path == root)
                    .unwrap();
                fs::write(&manifest, r#"{"name":"after"}"#).unwrap();
                let mut cache = SkillFactsCache::default();
                let mut git = HashMap::new();
                let mut out = Vec::new();
                let mut issues = Vec::new();
                plan.materialize(
                    &context,
                    &mut cache,
                    &mut git,
                    tokenizer(),
                    &mut out,
                    &mut issues,
                );
                let candidate = out
                    .iter()
                    .find(|candidate| candidate.path == entry)
                    .unwrap();
                assert!(matches!(candidate.plugin, PluginEvidence::Unknown));
                assert!(issues
                    .iter()
                    .any(|issue| issue.kind == DiscoveryReadIssueKind::PluginManifest));
            }
        }
    }

    #[test]
    fn agent_content_plans_reject_changes_before_materialization() {
        for named in [false, true] {
            for document in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let root = home.join(".claude/skills");
                let skill = root.join("alpha");
                write_skill(&skill, "alpha");
                let resource = skill.join("resource.txt");
                fs::write(&resource, "before").unwrap();
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string()]);
                let plan = prepare_agent_roots(&context, named.then_some(&names))
                    .into_iter()
                    .find(|plan| plan.path == root)
                    .unwrap();
                let changed = if document {
                    skill.join("SKILL.md")
                } else {
                    resource
                };
                fs::write(&changed, "changed after enumeration").unwrap();
                let mut cache = SkillFactsCache::default();
                let mut git = HashMap::new();
                let mut out = Vec::new();
                let mut issues = Vec::new();
                plan.materialize(
                    &context,
                    &mut cache,
                    &mut git,
                    tokenizer(),
                    &mut out,
                    &mut issues,
                );
                assert!(issues.iter().any(|issue| issue.path == changed));
                assert!(!cache.entries.contains_key(&skill));
                if document {
                    assert!(out.is_empty());
                }
            }
        }
    }

    #[test]
    fn agent_content_plans_cover_all_selected_roots_before_reading() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let claude = home.join(".claude/skills/alpha");
        let codex = home.join(".codex/skills/beta");
        for (skill, name) in [(&claude, "alpha"), (&codex, "beta")] {
            write_skill(skill, name);
            fs::write(skill.join("resource.txt"), "fixture").unwrap();
        }
        let context = read_context(home, &[]);
        let plans = prepare_agent_roots(&context, None);
        let mut observed = BTreeSet::new();
        for plan in &plans {
            for (_, entry) in &plan.entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok((_, PreparedSkillFacts::Ready { content, .. })) = &entry.resolved else {
                    continue;
                };
                let Ok(content) = content.as_ref() else {
                    continue;
                };
                observed.insert(content.document.requested.clone());
                observed.extend(
                    content
                        .walk
                        .hashable
                        .iter()
                        .map(|file| file.observation.requested.clone()),
                );
            }
        }
        assert_eq!(
            observed,
            BTreeSet::from([
                claude.join("SKILL.md"),
                claude.join("resource.txt"),
                codex.join("SKILL.md"),
                codex.join("resource.txt"),
            ])
        );
    }

    #[test]
    fn agent_root_plans_keep_full_named_and_disabled_membership_separate() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".claude/skills");
        write_skill(&root.join("alpha"), "alpha");
        write_skill(&root.join("beta"), "beta");
        write_skill(&root.join(STUDIO_DISABLED_DIR_NAME).join("alpha"), "alpha");
        let context = read_context(home, &[]);
        let names = BTreeSet::from(["alpha".to_string(), "missing".to_string()]);
        for named in [false, true] {
            let plans = prepare_agent_roots(&context, named.then_some(&names));
            let live = plans.iter().find(|plan| plan.path == root).unwrap();
            assert_eq!(live.state, RootReadOutcome::Read);
            let paths: BTreeSet<_> = live.entries.iter().map(|(path, _)| path.clone()).collect();
            let expected = if named { "missing" } else { "beta" };
            assert_eq!(
                paths,
                BTreeSet::from([root.join("alpha"), root.join(expected)])
            );
            if named {
                assert!(live
                    .entries
                    .iter()
                    .any(|(path, entry)| path.ends_with("missing")
                        && matches!(entry, Err(ScopedReadError::Missing { .. }))));
            }
            let disabled = plans
                .iter()
                .find(|plan| plan.path == root.join(STUDIO_DISABLED_DIR_NAME))
                .unwrap();
            assert!(disabled.disabled);
            assert!(disabled
                .entries
                .iter()
                .any(|(path, entry)| path.ends_with("alpha") && entry.is_ok()));
        }
    }

    #[test]
    fn agent_root_plans_reject_membership_changes_before_materialization() {
        for initially_absent in [false, true] {
            for named in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                let root = home.join(".claude/skills");
                if !initially_absent {
                    write_skill(&root.join("alpha"), "alpha");
                }
                let context = read_context(home, &[]);
                let names = BTreeSet::from(["alpha".to_string(), "new".to_string()]);
                let plan = prepare_agent_roots(&context, named.then_some(&names))
                    .into_iter()
                    .find(|plan| plan.path == root)
                    .unwrap();
                write_skill(&root.join("new"), "new");
                let mut cache = SkillFactsCache::default();
                let mut git = HashMap::new();
                let mut out = Vec::new();
                let mut issues = Vec::new();
                let state = plan.materialize(
                    &context,
                    &mut cache,
                    &mut git,
                    tokenizer(),
                    &mut out,
                    &mut issues,
                );
                assert_eq!(state, RootReadOutcome::Incomplete);
                assert!(!out
                    .iter()
                    .any(|candidate| candidate.path == root.join("new")));
                assert!(!issues.is_empty());
            }
        }
    }

    #[test]
    fn guarded_facts_preserve_cold_and_warm_cache_results() {
        for hard_link in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            let resource = skill.join("resource.txt");
            fs::write(&resource, "fixture resource").unwrap();
            if hard_link {
                fs::hard_link(&resource, temp.path().join("alias")).unwrap();
            }
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let mut issues = Vec::new();
            let mut expected_cache = SkillFactsCache::default();
            let expected = get_or_compute_facts(
                &scope,
                &mut expected_cache,
                &skill,
                tokenizer(),
                &mut issues,
            )
            .unwrap()
            .unwrap();
            let mut cache = SkillFactsCache::default();
            cache.begin_pass();
            let prepared = PreparedSkillFacts::enumerate(&scope, &skill);
            let guard = content_guard(&scope, temp.path(), &[skill.join("SKILL.md"), resource]);
            let reader = SkillContentRead {
                scope: &scope,
                guard: Some(&guard),
            };
            let cold = prepared
                .materialize(reader, &mut cache, &skill, tokenizer(), &mut issues)
                .unwrap()
                .unwrap();
            assert_eq!(cold.content_hash, expected.content_hash);
            assert_eq!(cold.skill_md_tokens, expected.skill_md_tokens);
            assert_eq!(cold.file_count, expected.file_count);
            let warm = PreparedSkillFacts::enumerate(&scope, &skill)
                .materialize(reader, &mut cache, &skill, tokenizer(), &mut issues)
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&cold, &warm));
            assert_eq!(cache.last_pass_stats(), (1, 2));
            assert!(issues.is_empty());
            guard.revalidate(&scope).unwrap();
        }
    }

    #[test]
    fn guarded_cache_hits_reject_unplanned_and_stale_resources() {
        for stale in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            let resource = skill.join("resource.txt");
            fs::write(&resource, "before").unwrap();
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let mut cache = SkillFactsCache::default();
            let mut issues = Vec::new();
            cache.begin_pass();
            get_or_compute_facts(&scope, &mut cache, &skill, tokenizer(), &mut issues)
                .unwrap()
                .unwrap();
            let prepared = PreparedSkillFacts::enumerate(&scope, &skill);
            let mut paths = vec![skill.join("SKILL.md")];
            if stale {
                fs::write(&resource, "after!").unwrap();
                paths.push(resource.clone());
            }
            let guard = content_guard(&scope, temp.path(), &paths);
            let reader = SkillContentRead {
                scope: &scope,
                guard: Some(&guard),
            };
            assert!(prepared
                .materialize(reader, &mut cache, &skill, tokenizer(), &mut issues)
                .is_err());
            assert!(cache.entries.is_empty());
            assert_eq!(cache.last_pass_stats().0, 0);
            assert!(issues.iter().any(|issue| issue.path == resource));
        }
    }

    #[test]
    fn prepared_facts_reject_changed_document_before_using_cached_facts() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let mut cache = SkillFactsCache::default();
        let mut issues = Vec::new();
        cache.begin_pass();
        let first = get_or_compute_facts(&scope, &mut cache, &skill, tokenizer(), &mut issues)
            .unwrap()
            .unwrap();
        assert!(issues.is_empty());
        let warm = PreparedSkillFacts::enumerate(&scope, &skill)
            .materialize(
                (&scope).into(),
                &mut cache,
                &skill,
                tokenizer(),
                &mut issues,
            )
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&first, &warm));
        let stale = PreparedSkillFacts::enumerate(&scope, &skill);
        fs::write(skill.join("SKILL.md"), "changed after preparation").unwrap();
        assert!(stale
            .materialize(
                (&scope).into(),
                &mut cache,
                &skill,
                tokenizer(),
                &mut issues
            )
            .is_err());
        assert!(!issues.is_empty());
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn prepared_content_records_document_and_resources_before_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("alpha");
        write_skill(&skill, "alpha");
        fs::create_dir(skill.join("scripts")).unwrap();
        fs::write(skill.join("scripts/check.sh"), "echo fixture").unwrap();
        let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
        let prepared = PreparedSkillContent::enumerate(&scope, &skill).unwrap();
        assert_eq!(prepared.document.requested, skill.join("SKILL.md"));
        let files: BTreeSet<_> = prepared
            .walk
            .hashable
            .iter()
            .map(|file| file.observation.requested.clone())
            .collect();
        assert_eq!(
            files,
            BTreeSet::from([skill.join("SKILL.md"), skill.join("scripts/check.sh")])
        );
        let (_, bytes, truncated) = prepared.materialize_prefix((&scope).into()).unwrap();
        assert_eq!(bytes, fs::read(skill.join("SKILL.md")).unwrap());
        assert!(!truncated);
    }

    #[test]
    fn prepared_content_rejects_document_changes_before_materialization() {
        for remove in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let skill = temp.path().join("alpha");
            write_skill(&skill, "alpha");
            let scope = SkillReadScope::bind(&[temp.path().to_path_buf()]).unwrap();
            let prepared = PreparedSkillContent::enumerate(&scope, &skill).unwrap();
            if remove {
                fs::remove_file(skill.join("SKILL.md")).unwrap();
            } else {
                fs::write(skill.join("SKILL.md"), "replacement content").unwrap();
            }
            assert!(
                matches!(prepared.materialize_prefix((&scope).into()), Err(FactsWalkError::Failed(issue))
                if issue.kind == DiscoveryReadIssueKind::SkillDocument)
            );
        }
    }

    #[test]
    fn missing_document_has_known_membership_but_wrong_type_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = home.join(".claude/skills");
        let skill = root.join("alpha");
        write_skill(&skill, "alpha");
        let context = read_context(&home, &[]);
        let names = BTreeSet::from(["alpha".into()]);
        let mut cache = SkillFactsCache::default();
        assert_eq!(
            super::discover_skill_candidates_cached(&context, &mut cache)
                .candidates
                .len(),
            1
        );
        fs::remove_file(skill.join("SKILL.md")).unwrap();
        for named in [true, false] {
            let report = if named {
                super::discover_named_skill_candidates_cached(&context, &names, &mut cache)
            } else {
                super::discover_skill_candidates_cached(&context, &mut cache)
            };
            assert!(report.candidates.is_empty());
            assert!(cache.entries.is_empty());
            let coverage = report
                .source_coverage
                .iter()
                .find(|c| c.path == root)
                .unwrap();
            assert_eq!(coverage.membership, RootReadOutcome::Read);
            assert_eq!(coverage.facts, RootReadOutcome::Read);
        }
        fs::create_dir(skill.join("SKILL.md")).unwrap();
        for report in [
            super::discover_skill_candidates(&context),
            super::discover_named_skill_candidates_cached(&context, &names, &mut cache),
        ] {
            assert!(report.candidates.is_empty());
            assert_eq!(
                report
                    .source_coverage
                    .iter()
                    .find(|c| c.path == root)
                    .unwrap()
                    .membership,
                RootReadOutcome::Incomplete
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn stable_child_link_has_known_membership_in_both_scan_modes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let backing = home.join("backing/alpha");
        write_skill(&backing, "alpha");
        let root = home.join(".claude/skills");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&backing, root.join("alpha")).unwrap();
        let context = read_context(&home, &[]);
        for report in [
            super::discover_skill_candidates(&context),
            super::discover_named_skill_candidates_cached(
                &context,
                &BTreeSet::from(["alpha".into()]),
                &mut SkillFactsCache::default(),
            ),
        ] {
            assert_eq!(report.candidates.len(), 1);
            assert!(report.candidates[0].is_symlink);
            assert_eq!(report.candidates[0].path, root.join("alpha"));
            assert_eq!(
                report
                    .source_coverage
                    .iter()
                    .find(|c| c.path == root)
                    .unwrap()
                    .membership,
                RootReadOutcome::Read
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn live_and_disabled_fact_coverage_is_source_local() {
        for disabled in [false, true] {
            let tmp = tempfile::tempdir_in("/tmp").unwrap();
            let home = tmp.path().join("home");
            let root = home.join(".claude/skills");
            let holding = root.join(STUDIO_DISABLED_DIR_NAME);
            fs::create_dir_all(&holding).unwrap();
            fs::create_dir(home.join(".git")).unwrap();
            let partial = if disabled { &holding } else { &root };
            write_skill(&partial.join("alpha"), "alpha");
            let _socket =
                std::os::unix::net::UnixListener::bind(partial.join("alpha/resource.sock"))
                    .unwrap();
            let context = read_context(&home, &[]);
            for report in [
                super::discover_skill_candidates(&context),
                super::discover_named_skill_candidates_cached(
                    &context,
                    &BTreeSet::from(["alpha".into()]),
                    &mut SkillFactsCache::default(),
                ),
            ] {
                for path in [&root, &holding] {
                    let coverage = report
                        .source_coverage
                        .iter()
                        .find(|c| &c.path == path)
                        .unwrap();
                    assert_eq!(coverage.membership, RootReadOutcome::Read);
                    assert_eq!(
                        coverage.facts,
                        if path == partial {
                            RootReadOutcome::Incomplete
                        } else {
                            RootReadOutcome::Read
                        }
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn plugin_resource_failure_without_truncation_marks_facts_incomplete() {
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let home = tmp.path().join("home");
        let cache = home.join(".codex/plugins/cache");
        let plugin = cache.join("fixture");
        let skill = plugin.join("skills/alpha");
        write_skill(&skill, "alpha");
        fs::create_dir(home.join(".git")).unwrap();
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(skill.join("resource.sock")).unwrap();
        let context = read_context(&home, &[]);
        for report in [
            super::discover_skill_candidates(&context),
            super::discover_named_skill_candidates_cached(
                &context,
                &BTreeSet::from(["alpha".into()]),
                &mut SkillFactsCache::default(),
            ),
        ] {
            assert_eq!(report.candidates.len(), 1);
            assert!(!report.candidates[0].folder_truncated);
            assert_eq!(report.candidates[0].git_repo, GitRepoEvidence::Present);
            let coverage = report
                .source_coverage
                .iter()
                .find(|c| c.path == cache)
                .unwrap();
            assert_eq!(coverage.membership, RootReadOutcome::Read);
            assert_eq!(coverage.facts, RootReadOutcome::Incomplete);
        }
    }

    #[test]
    fn complete_plugin_membership_can_have_incomplete_content_facts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let plugin = home.join(".codex/plugins/cache/market/plugin/1");
        write_skill(&plugin.join("skills/alpha"), "alpha");
        fs::write(plugin.join("plugin.json"), r#"{"name":"fixture"}"#).unwrap();
        let resource = plugin.join("skills/alpha/large-resource");
        fs::File::create(&resource)
            .unwrap()
            .set_len(MAX_FOLDER_BYTES + 1)
            .unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), Vec::new(), Vec::new());
        let report = super::discover_skill_candidates(&context);
        let coverage = report
            .source_coverage
            .iter()
            .find(|coverage| {
                coverage.source == MembershipSource::PluginCache
                    && coverage.path == home.join(".codex/plugins/cache")
            })
            .unwrap();
        assert_eq!(coverage.membership, RootReadOutcome::Read);
        assert_eq!(coverage.facts, RootReadOutcome::Incomplete);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.folder_truncated));
    }

    #[test]
    fn root_coverage_uses_the_actual_full_and_named_operations() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let wrong_type = home.join(".codex/skills");
        fs::create_dir_all(wrong_type.parent().unwrap()).unwrap();
        fs::write(&wrong_type, "not a directory").unwrap();
        let partial = home.join(".cursor/skills/partial");
        write_skill(&partial, "partial");
        let outside = temp.path().join("outside-resource");
        fs::write(&outside, "outside bytes").unwrap();
        std::os::unix::fs::symlink(&outside, partial.join("resource-link")).unwrap();
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        let capped = home.join(".grok/skills");
        fs::create_dir_all(&capped).unwrap();
        for index in 0..=MAX_FOLDER_ENTRIES {
            fs::write(capped.join(format!("entry-{index:05}")), "").unwrap();
        }
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), Vec::new(), Vec::new());
        let expected_roots = agents::skill_roots(&home, &[]);

        let full = super::discover_skill_candidates(&context);
        assert_eq!(full.extent, DiscoveryExtent::Full);
        assert_eq!(
            full.source_coverage
                .iter()
                .filter(|coverage| coverage.source == MembershipSource::AgentRoot)
                .count(),
            expected_roots.len()
        );
        assert_eq!(
            full.source_coverage
                .iter()
                .filter(|coverage| coverage.source == MembershipSource::DisabledRoot)
                .count(),
            expected_roots.len()
        );
        assert!(full
            .source_coverage
            .iter()
            .all(|coverage| coverage.extent == DiscoveryExtent::Full));
        let full_membership = |path: &Path| {
            full.source_coverage
                .iter()
                .find(|coverage| {
                    coverage.source == MembershipSource::AgentRoot && coverage.path == path
                })
                .unwrap()
                .membership
        };
        assert_eq!(
            full_membership(&home.join(".claude/skills")),
            RootReadOutcome::Absent
        );
        assert_eq!(full_membership(&wrong_type), RootReadOutcome::Failed);
        assert_eq!(
            full.source_coverage
                .iter()
                .find(|coverage| {
                    coverage.source == MembershipSource::DisabledRoot
                        && coverage.path == wrong_type.join(STUDIO_DISABLED_DIR_NAME)
                })
                .unwrap()
                .membership,
            RootReadOutcome::Failed
        );
        assert_eq!(
            full_membership(&home.join(".cursor/skills")),
            RootReadOutcome::Read
        );
        assert_eq!(full_membership(&capped), RootReadOutcome::Incomplete);
        assert_eq!(
            full_membership(&home.join(".agents/skills")),
            RootReadOutcome::Read,
        );

        let names = BTreeSet::from(["partial".to_string()]);
        let named = super::discover_named_skill_candidates_cached(
            &context,
            &names,
            &mut SkillFactsCache::default(),
        );
        assert_eq!(named.extent, DiscoveryExtent::Named);
        assert!(named
            .source_coverage
            .iter()
            .all(|coverage| coverage.extent == DiscoveryExtent::Named));
        let named_membership = |path: &Path| {
            named
                .source_coverage
                .iter()
                .find(|coverage| {
                    coverage.source == MembershipSource::AgentRoot && coverage.path == path
                })
                .unwrap()
                .membership
        };
        assert_eq!(named_membership(&wrong_type), RootReadOutcome::Failed);
        assert_eq!(
            named_membership(&home.join(".cursor/skills")),
            RootReadOutcome::Read
        );
        assert_eq!(named_membership(&capped), RootReadOutcome::Read);
    }

    #[test]
    fn declared_linked_roots_keep_child_identity_and_cached_git_uncertainty() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let backing = temp.path().join("backing");
        write_skill(&backing.join("shared-skill"), "shared-skill");
        for harness in [".claude", ".codex"] {
            fs::create_dir_all(home.join(harness)).unwrap();
            std::os::unix::fs::symlink(&backing, home.join(harness).join("skills")).unwrap();
        }
        let context = SkillDiscoveryReadContext::bind(
            home.clone(),
            Vec::new(),
            vec![backing.clone()],
            Vec::new(),
        );

        for report in [
            super::discover_skill_candidates(&context),
            super::discover_named_skill_candidates_cached(
                &context,
                &BTreeSet::from(["shared-skill".to_string()]),
                &mut SkillFactsCache::default(),
            ),
        ] {
            let deployments: Vec<_> = report
                .candidates
                .iter()
                .filter(|candidate| candidate.name == "shared-skill")
                .collect();
            assert_eq!(deployments.len(), 2);
            assert_ne!(deployments[0].path, deployments[1].path);
            let resolved_backing = fs::canonicalize(backing.join("shared-skill")).unwrap();
            assert!(deployments
                .iter()
                .all(|candidate| candidate.resolved_path.as_deref()
                    == Some(resolved_backing.as_path())));
            for root in [home.join(".claude/skills"), home.join(".codex/skills")] {
                let coverage = report
                    .source_coverage
                    .iter()
                    .find(|coverage| {
                        coverage.source == MembershipSource::AgentRoot && coverage.path == root
                    })
                    .unwrap();
                assert_eq!(coverage.membership, RootReadOutcome::Read);
                assert_eq!(coverage.facts, RootReadOutcome::Incomplete);
            }
        }
    }

    #[test]
    fn git_markers_are_scoped_and_edge_uncertainty_is_visible() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let file_repo = temp.path().join("file-repo");
        let dir_repo = temp.path().join("dir-repo");
        fs::create_dir_all(&file_repo).unwrap();
        fs::write(file_repo.join(".git"), "gitdir: elsewhere").unwrap();
        fs::create_dir_all(dir_repo.join(".git")).unwrap();
        write_skill(&file_repo.join(".claude/skills/file-marker"), "file-marker");
        write_skill(&dir_repo.join(".claude/skills/dir-marker"), "dir-marker");
        let edge = home.join(".claude/skills/edge");
        write_skill(&edge, "edge");
        let outside = temp.path().join("outside");
        fs::create_dir_all(outside.join("secret-git-data")).unwrap();
        let escaped_repo = temp.path().join("escaped-repo");
        fs::create_dir_all(&escaped_repo).unwrap();
        std::os::unix::fs::symlink(&outside, escaped_repo.join(".git")).unwrap();
        write_skill(
            &escaped_repo.join(".claude/skills/escaped-marker"),
            "escaped-marker",
        );
        let context = SkillDiscoveryReadContext::bind(
            home,
            vec![file_repo, dir_repo, escaped_repo],
            Vec::new(),
            Vec::new(),
        );

        let report = super::discover_skill_candidates(&context);
        for name in ["file-marker", "dir-marker"] {
            assert_eq!(
                report
                    .candidates
                    .iter()
                    .find(|candidate| candidate.name == name)
                    .unwrap()
                    .git_repo,
                GitRepoEvidence::Present
            );
        }
        for (name, expected) in [
            ("edge", GitRepoEvidence::Truncated),
            ("escaped-marker", GitRepoEvidence::Unknown),
        ] {
            assert_eq!(
                report
                    .candidates
                    .iter()
                    .find(|candidate| candidate.name == name)
                    .unwrap()
                    .git_repo,
                expected
            );
        }
        assert!(report.read_issues.iter().any(|issue| {
            issue.kind == DiscoveryReadIssueKind::GitScopeBoundary
                && issue
                    .message
                    .contains("Git ancestry leaves the declared read scope")
        }));
        assert!(report.read_issues.iter().any(|issue| {
            issue.kind == DiscoveryReadIssueKind::Metadata
                && issue.path.ends_with("escaped-repo/.git")
        }));
    }

    #[test]
    fn lexical_link_failures_retain_deployment_identity_without_target_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = home.join(".claude/skills");
        let outside = temp.path().join("outside");
        write_skill(&outside.join("secret"), "secret");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), root.join("escaped")).unwrap();
        std::os::unix::fs::symlink("missing-target", root.join("broken")).unwrap();
        std::os::unix::fs::symlink("looped", root.join("looped")).unwrap();
        fs::write(root.join("plain-file"), "not a skill directory").unwrap();
        std::os::unix::fs::symlink(root.join("plain-file"), root.join("file-link")).unwrap();
        let context = SkillDiscoveryReadContext::bind(home, Vec::new(), Vec::new(), Vec::new());

        let report = super::discover_skill_candidates(&context);
        for name in ["escaped", "broken", "looped", "file-link"] {
            let candidate = report
                .candidates
                .iter()
                .find(|candidate| candidate.name == name)
                .unwrap();
            assert!(candidate.is_symlink);
            assert!(candidate.content_hash.is_empty());
        }
        assert!(!report
            .candidates
            .iter()
            .any(|candidate| candidate.name == "secret"));
        assert!(!report
            .candidates
            .iter()
            .any(|candidate| candidate.content_hash.contains("secret")));
    }

    #[test]
    fn listed_entry_retarget_and_raw_link_failure_are_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let backing = temp.path().join("backing");
        write_skill(&backing, "alias");
        let alias = home.join(".claude/skills/alias");
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&backing, &alias).unwrap();
        let context =
            SkillDiscoveryReadContext::bind(home.clone(), Vec::new(), vec![backing], Vec::new());
        let _replacement = SkillReadScope::inject_observed_link_replacement();
        let retargeted = super::discover_skill_candidates(&context);
        let candidate = retargeted
            .candidates
            .iter()
            .find(|candidate| candidate.path == alias)
            .unwrap();
        assert!(candidate.symlink_error.is_some());
        assert_eq!(
            retargeted
                .source_coverage
                .iter()
                .find(|coverage| {
                    coverage.source == MembershipSource::AgentRoot
                        && coverage.path == home.join(".claude/skills")
                })
                .unwrap()
                .membership,
            RootReadOutcome::Incomplete
        );

        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(home.join("missing"), &alias).unwrap();
        let _read_failure = SkillReadScope::inject_entry_link_read_failure(libc::EIO);
        let failed = super::discover_skill_candidates(&context);
        let candidate = failed
            .candidates
            .iter()
            .find(|candidate| candidate.path == alias)
            .unwrap();
        assert!(candidate.symlink_target.is_none());
        assert!(candidate.symlink_error.is_some());
        assert!(failed
            .read_issues
            .iter()
            .any(|issue| issue.path == alias.to_string_lossy()));
    }
}

#[cfg(test)]
pub(crate) mod content_folder_probe {
    use super::*;
    use crate::skill_coordination::{CoordinatedReadGuard, CoordinationFailure};

    pub(crate) enum Failure {
        Coordination(CoordinationFailure),
        Incomplete(Vec<DiscoveryReadIssue>),
    }

    impl From<CoordinationFailure> for Failure {
        fn from(error: CoordinationFailure) -> Self {
            Self::Coordination(error)
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct Facts {
        pub content_hash: String,
        pub file_count: u32,
        pub has_spec: bool,
        pub skill_md_tokens: u32,
    }

    pub(crate) struct Plan {
        path: PathBuf,
        facts: PreparedSkillFacts,
        proof: DeploymentContentReadProof,
    }

    pub(crate) struct Read {
        pub facts: Facts,
        proof: DeploymentContentReadProof,
    }

    impl Read {
        pub(crate) fn revalidate(&self, scope: &SkillReadScope) -> Result<(), CoordinationFailure> {
            if self.proof.resources_unchanged(scope) && self.proof.spec_unchanged(scope) {
                Ok(())
            } else {
                Err(CoordinationFailure::Changed)
            }
        }
    }

    impl Plan {
        pub(crate) fn enumerate(
            scope: &SkillReadScope,
            path: &Path,
            guard: &CoordinatedReadGuard,
        ) -> Result<Self, Failure> {
            guard.check_cancelled()?;
            let facts = PreparedSkillFacts::enumerate_checked(scope, path, &mut || {
                guard.check_cancelled()
            })?;
            let proof = DeploymentContentReadProof::capture(path, Some(&facts), scope);
            guard.check_cancelled()?;
            Ok(Self {
                path: path.to_path_buf(),
                facts,
                proof,
            })
        }

        pub(crate) fn regular_files(&self) -> Vec<PathBuf> {
            let mut files = BTreeSet::new();
            self.facts.append_regular_files(&mut files);
            files.into_iter().collect()
        }

        pub(crate) fn materialize(
            self,
            scope: &SkillReadScope,
            guard: &CoordinatedReadGuard,
        ) -> Result<Read, Failure> {
            guard.check_cancelled()?;
            if !self.proof.resources_unchanged(scope) || !self.proof.spec_unchanged(scope) {
                return Err(CoordinationFailure::Changed.into());
            }
            let mut cache = SkillFactsCache::default();
            cache.begin_pass();
            let mut issues = Vec::new();
            let facts = self.facts.materialize(
                SkillContentRead {
                    scope,
                    guard: Some(guard),
                },
                &mut cache,
                &self.path,
                tokenizer(),
                &mut issues,
            );
            guard.check_cancelled()?;
            let Ok(Some(facts)) = facts else {
                return Err(Failure::Incomplete(issues));
            };
            if facts.incomplete || !issues.is_empty() {
                return Err(Failure::Incomplete(issues));
            }
            let read = Read {
                facts: Facts {
                    content_hash: facts.content_hash.clone(),
                    file_count: facts.file_count,
                    has_spec: facts.has_spec,
                    skill_md_tokens: facts.skill_md_tokens,
                },
                proof: self.proof,
            };
            read.revalidate(scope)?;
            guard.revalidate(scope)?;
            Ok(read)
        }
    }
}
