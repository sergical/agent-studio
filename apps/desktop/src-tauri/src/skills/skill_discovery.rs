// ============================================================================
// Skills Module - Directory Scanner
// The only module that walks the filesystem looking for installed skills.
// Walks the agent skill roots (agents::skill_roots) and the native plugin
// caches (plugins::scan_plugin_skills), capturing every fact a SkillCandidate
// needs. Classification and merging happen downstream, in provenance.rs and
// skill_assembly.rs, which never touch disk.
// ============================================================================

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tiktoken_rs::CoreBPE;

use super::agents;
use super::frontmatter::{frontmatter_fields, parse_frontmatter, validate_skill, SkillFrontmatter};
use super::plugins;
use super::skill_candidate::SkillCandidate;

/// Folder walk caps: a skill folder is hashed and sized up to this many
/// files or bytes, whichever comes first. Beyond that, `folder_truncated` is
/// set and the remaining entries are skipped rather than read.
const MAX_FOLDER_FILES: usize = 2_000;
const MAX_FOLDER_BYTES: u64 = 64 * 1024 * 1024;

/// SKILL.md is read through a bounded reader capped at this many bytes, so a
/// pathologically large SKILL.md can't be read into memory in full. Counted
/// against the same `MAX_FOLDER_BYTES` budget as the rest of the folder.
const SKILL_MD_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The embedded cl100k_base vocab is loaded once per process, not once per
/// rebuild - rebuilding the BPE from scratch on every snapshot rebuild (which
/// can happen every few seconds from the background watcher) is pure waste
/// since the vocab never changes.
static TOKENIZER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn tokenizer() -> Option<&'static CoreBPE> {
    TOKENIZER
        .get_or_init(|| tiktoken_rs::cl100k_base().ok())
        .as_ref()
}

/// A skill "ships specs" (the getsentry/skillet pattern) when it has a
/// spec.md file or an evals/ subdirectory alongside SKILL.md.
fn has_spec(skill_dir: &Path) -> bool {
    skill_dir.join("spec.md").exists() || skill_dir.join("evals").is_dir()
}

/// A regular file found under a skill folder, queued for hashing once the
/// whole folder has been walked and its entries sorted. Holds only the path,
/// size, and mtime - never the file's bytes - so the walk's memory use
/// doesn't grow with folder size. `len`/`mtime` come straight from the same
/// `fs::Metadata` the walk already reads for `total_bytes`/`newest`, so
/// `folder_fingerprint` costs no extra syscalls.
struct HashableFile {
    rel_path: PathBuf,
    abs_path: PathBuf,
    len: u64,
    mtime: Option<SystemTime>,
}

/// Accumulated facts from walking a skill folder.
struct FolderWalk {
    hashable: Vec<HashableFile>,
    total_bytes: u64,
    file_count: u32,
    newest: Option<SystemTime>,
    truncated: bool,
}

/// Walk `dir` recursively, gathering byte/file counts and the newest mtime,
/// stopping once `max_files`/`max_bytes` is reached. Never follows symlinks:
/// a symlinked directory is not descended into, and a symlinked file counts
/// toward `file_count` but is never opened or hashed. Unreadable entries are
/// skipped rather than failing the whole walk.
#[cfg(test)]
fn walk_folder(dir: &Path) -> FolderWalk {
    walk_folder_capped(dir, MAX_FOLDER_FILES, MAX_FOLDER_BYTES)
}

fn walk_folder_capped(dir: &Path, max_files: usize, max_bytes: u64) -> FolderWalk {
    let mut walk = FolderWalk {
        hashable: Vec::new(),
        total_bytes: 0,
        file_count: 0,
        newest: None,
        truncated: false,
    };
    walk_folder_into(dir, dir, max_files, max_bytes, &mut walk);
    walk
}

fn walk_folder_into(
    root: &Path,
    dir: &Path,
    max_files: usize,
    max_bytes: u64,
    walk: &mut FolderWalk,
) {
    if walk.truncated {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if walk.truncated {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_symlink() {
            // A symlinked file counts toward file_count but is never read; a
            // symlinked dir is skipped entirely rather than descended into.
            if fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                walk.file_count += 1;
                if walk.file_count as usize >= max_files {
                    walk.truncated = true;
                }
            }
            continue;
        }

        if file_type.is_dir() {
            walk_folder_into(root, &path, max_files, max_bytes, walk);
        } else if file_type.is_file() {
            let Ok(meta) = entry.metadata() else { continue };
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
                if walk.newest.is_none_or(|n| mtime > n) {
                    walk.newest = Some(mtime);
                }
            }
            if let Ok(rel_path) = path.strip_prefix(root) {
                walk.hashable.push(HashableFile {
                    rel_path: rel_path.to_path_buf(),
                    abs_path: path.clone(),
                    len: meta.len(),
                    mtime: meta.modified().ok(),
                });
            }
            if walk.file_count as usize >= max_files || walk.total_bytes >= max_bytes {
                walk.truncated = true;
            }
        }
    }
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
fn content_hash(mut files: Vec<HashableFile>, max_bytes: u64) -> String {
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut remaining = max_bytes;
    for file in &files {
        let rel_path_bytes = file.rel_path.to_string_lossy().into_owned().into_bytes();
        hasher.update((rel_path_bytes.len() as u64).to_le_bytes());
        hasher.update(&rel_path_bytes);

        let file_len = fs::metadata(&file.abs_path).map(|m| m.len()).unwrap_or(0);
        hasher.update(file_len.to_le_bytes());

        if remaining == 0 {
            continue;
        }
        if let Ok(handle) = fs::File::open(&file.abs_path) {
            let mut limited = handle.take(remaining);
            loop {
                match limited.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        hasher.update(&buf[..n]);
                        remaining = remaining.saturating_sub(n as u64);
                    }
                    Err(_) => break,
                }
            }
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A cheap stand-in for a folder's content, built from `stat` alone: no file
/// is opened or read. Two walks of the same folder produce the same
/// fingerprint iff every file's relative path, size, and mtime match, and the
/// walk's own summary (`file_count`/`total_bytes`/`truncated`) matches too -
/// which is what `get_or_compute_facts` uses to decide whether a cached
/// `SkillContentFacts` from a previous rebuild is still valid.
fn folder_fingerprint(walk: &FolderWalk) -> u64 {
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
    frontmatter: Option<SkillFrontmatter>,
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
fn walk_for_facts(skill_dir: &Path) -> Option<(FolderWalk, Vec<u8>, bool)> {
    let file = fs::File::open(skill_dir.join("SKILL.md")).ok()?;
    let mut bytes = Vec::new();
    // Read one byte past the cap so we can tell "exactly at the cap" apart
    // from "more data follows", without ever buffering more than that.
    file.take(SKILL_MD_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let skill_md_truncated = bytes.len() as u64 > SKILL_MD_MAX_BYTES;
    if skill_md_truncated {
        bytes.truncate(SKILL_MD_MAX_BYTES as usize);
    }
    let skill_md_len = bytes.len() as u64;

    // SKILL.md's own bytes count against the same folder budget: the walk
    // below re-reads/hashes it as part of the folder, so its remaining
    // budget is reduced by what was just read above.
    let walk = walk_folder_capped(
        skill_dir,
        MAX_FOLDER_FILES,
        MAX_FOLDER_BYTES.saturating_sub(skill_md_len),
    );
    Some((walk, bytes, skill_md_truncated))
}

/// Recompute the bounded strong folder hash that discovery stores on a deployment.
/// This bypasses the facts cache so mutation guards always compare live bytes.
pub(crate) fn live_skill_content_hash(skill_dir: &Path) -> Result<String, String> {
    let (walk, _, _) = walk_for_facts(skill_dir)
        .ok_or_else(|| format!("Could not read {}/SKILL.md", skill_dir.display()))?;
    Ok(content_hash(walk.hashable, MAX_FOLDER_BYTES))
}

/// The expensive half of `compute_content_facts`: hashing every file in
/// `walk` and tokenizing SKILL.md. Only run on a `get_or_compute_facts` cache
/// miss - `walk_for_facts` (stat-only) already ran to produce `walk`.
fn compute_content_facts_from_walk(
    walk: FolderWalk,
    skill_md_bytes: &[u8],
    skill_md_truncated: bool,
    skill_dir: &Path,
    tokenizer: Option<&CoreBPE>,
) -> SkillContentFacts {
    let content = String::from_utf8_lossy(skill_md_bytes).into_owned();
    let frontmatter = parse_frontmatter(&content);
    let name_for_tokens = frontmatter
        .as_ref()
        .and_then(|f| f.name.clone())
        .unwrap_or_default();
    let description_for_tokens = frontmatter
        .as_ref()
        .and_then(|f| f.description.clone())
        .unwrap_or_default();
    let modified_at = walk.newest.map(|t| DateTime::<Utc>::from(t).to_rfc3339());

    SkillContentFacts {
        frontmatter_fields: frontmatter_fields(&content),
        frontmatter,
        has_spec: has_spec(skill_dir),
        folder_bytes: walk.total_bytes,
        file_count: walk.file_count,
        skill_md_tokens: count_tokens(&content, tokenizer),
        description_tokens: count_tokens(
            &format!("{name_for_tokens}: {description_for_tokens}"),
            tokenizer,
        ),
        skill_md_line_count: content.lines().count(),
        content_hash: content_hash(walk.hashable, MAX_FOLDER_BYTES),
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
    let (walk, skill_md_bytes, skill_md_truncated) = walk_for_facts(skill_dir)?;
    Some(compute_content_facts_from_walk(
        walk,
        &skill_md_bytes,
        skill_md_truncated,
        skill_dir,
        tokenizer,
    ))
}

/// One `SkillContentFacts` cache entry: the stat-only fingerprint it was
/// computed for, and the generation (see `SkillFactsCache::begin_pass`) it
/// was last confirmed valid in.
struct CacheEntry {
    fingerprint: u64,
    facts: Arc<SkillContentFacts>,
    last_seen: u64,
}

/// A `SkillContentFacts` cache that survives across rebuilds, keyed by
/// canonical skill directory and validated by `folder_fingerprint` (a
/// stat-only fingerprint of the folder) rather than by never expiring.
/// `begin_pass`/`end_pass` bracket one `discover_skill_candidates_cached`
/// call so an entry not touched during a pass - the skill directory it
/// belonged to was removed - is evicted rather than pinning memory forever.
#[derive(Default)]
pub struct SkillFactsCache {
    entries: HashMap<PathBuf, CacheEntry>,
    generation: u64,
    hits: u64,
    total: u64,
}

impl SkillFactsCache {
    /// Start a new pass: reset the hit/total counters `last_pass_stats`
    /// reports, and advance the generation so `end_pass` can tell which
    /// entries were touched this time.
    fn begin_pass(&mut self) {
        self.generation += 1;
        self.hits = 0;
        self.total = 0;
    }

    /// Drop every entry not looked up during the pass just finished - its
    /// skill directory no longer exists (or was never visited), so keeping
    /// its facts around would only pin memory for a skill that's gone.
    fn end_pass(&mut self) {
        let generation = self.generation;
        self.entries
            .retain(|_, entry| entry.last_seen == generation);
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
fn get_or_compute_facts(
    cache: &mut SkillFactsCache,
    skill_dir: &Path,
    tokenizer: Option<&CoreBPE>,
) -> Option<Arc<SkillContentFacts>> {
    let key = fs::canonicalize(skill_dir).unwrap_or_else(|_| skill_dir.to_path_buf());
    let (walk, skill_md_bytes, skill_md_truncated) = walk_for_facts(skill_dir)?;
    let fingerprint = folder_fingerprint(&walk);
    let generation = cache.generation;

    cache.total += 1;
    if let Some(entry) = cache.entries.get_mut(&key) {
        if entry.fingerprint == fingerprint {
            entry.last_seen = generation;
            cache.hits += 1;
            return Some(Arc::clone(&entry.facts));
        }
    }

    let facts = Arc::new(compute_content_facts_from_walk(
        walk,
        &skill_md_bytes,
        skill_md_truncated,
        skill_dir,
        tokenizer,
    ));
    cache.entries.insert(
        key,
        CacheEntry {
            fingerprint,
            facts: Arc::clone(&facts),
            last_seen: generation,
        },
    );
    Some(facts)
}

/// Build a candidate for a resolved skill directory (the SKILL.md-bearing
/// directory itself - for symlinks, this is the canonicalized target) from
/// its already-computed content facts.
#[allow(clippy::too_many_arguments)]
fn build_candidate(
    entry_path: &Path,
    skill_dir: &Path,
    root: &agents::SkillRoot,
    is_symlink: bool,
    symlink_target: Option<PathBuf>,
    symlink_error: Option<String>,
    shared_root_has_lock_entry: bool,
    scope: &str,
    facts: &SkillContentFacts,
    git_cache: &mut HashMap<PathBuf, bool>,
    studio_disabled: bool,
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
        facts.frontmatter.as_ref(),
        facts.skill_md_line_count,
    );
    // Plugin provenance checks the resolved skill dir first (the canonical
    // target for symlinks), then the entry path itself.
    let plugin = plugins::find_plugin_root(skill_dir, &root.label)
        .or_else(|| plugins::find_plugin_root(entry_path, &root.label));
    // A symlinked entry's canonical target is already carried by
    // `symlink_target`; `resolved_path` only covers the case where
    // `entry_path` itself isn't a symlink but an ancestor is (e.g. a whole
    // `.claude/skills` root linked to `.agents/skills`), where `skill_dir`
    // already holds the canonical directory.
    let resolved_path = if is_symlink {
        Some(skill_dir.to_path_buf())
    } else {
        fs::canonicalize(skill_dir).ok().filter(|c| c != entry_path)
    };
    // Whole-dir link: the entry itself isn't a symlink, `resolved_path` is
    // set (some ancestor is a symlink), and that ancestor's canonicalized
    // root ends in `.agents/skills` - same check as
    // `skill_harness_disable::refuse_shared_root`.
    let shared_via_whole_dir_link = !is_symlink
        && resolved_path.is_some()
        && entry_path.parent().is_some_and(|root| {
            let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let components: Vec<_> = canonical
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            components.windows(2).any(|w| w == [".agents", "skills"])
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
        shared_root_has_lock_entry,
        frontmatter_fields: facts.frontmatter_fields.clone(),
        frontmatter: facts.frontmatter.clone(),
        spec_violations,
        has_spec: facts.has_spec,
        folder_bytes: facts.folder_bytes,
        file_count: facts.file_count,
        skill_md_tokens: facts.skill_md_tokens,
        description_tokens: facts.description_tokens,
        content_hash: facts.content_hash.clone(),
        modified_at: facts.modified_at.clone(),
        folder_truncated: facts.folder_truncated,
        in_git_repo: in_git_repo(skill_dir, git_cache),
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
        plugin: None,
        shared_root_has_lock_entry: false,
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
        in_git_repo: false,
        studio_disabled,
        shared_via_whole_dir_link: false,
    }
}

/// How a symlink's target resolved.
enum SymlinkResolution {
    /// Canonicalized successfully.
    Resolved(PathBuf),
    /// The target doesn't exist (`io::ErrorKind::NotFound`). Carries the raw
    /// `read_link` target, when that itself could be read.
    Broken(Option<PathBuf>),
    /// Canonicalizing failed for some other reason (permission denied, a
    /// symlink loop, etc). Carries the raw `read_link` target and the error.
    Error(Option<PathBuf>, String),
}

/// Raw `fs::read_link` target, resolved relative to the link's parent when
/// the stored target is relative (as most symlinks are).
fn raw_symlink_target(entry_path: &Path) -> Option<PathBuf> {
    let target = fs::read_link(entry_path).ok()?;
    if target.is_absolute() {
        Some(target)
    } else {
        let parent = entry_path.parent().unwrap_or_else(|| Path::new(""));
        Some(parent.join(target))
    }
}

fn resolve_symlink(entry_path: &Path) -> SymlinkResolution {
    match fs::canonicalize(entry_path) {
        Ok(canonical) => SymlinkResolution::Resolved(canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            SymlinkResolution::Broken(raw_symlink_target(entry_path))
        }
        Err(err) => SymlinkResolution::Error(raw_symlink_target(entry_path), err.to_string()),
    }
}

/// Whether `root`'s shared `.agents/skills` directory is dotagents-managed:
/// `agents.toml`/`agents.lock` sitting next to it.
fn shared_root_has_lock_entry(root: &agents::SkillRoot) -> bool {
    if root.label != "shared" {
        return false;
    }
    let agents_root = root.path.parent().unwrap_or(&root.path);
    agents_root.join("agents.toml").exists() || agents_root.join("agents.lock").exists()
}

/// Walk `dir`'s ancestors (including `dir` itself) for a `.git` entry (file
/// or directory - a worktree's `.git` is a file), stopping at the
/// filesystem root. Memoized per directory in `cache` so a root holding many
/// skills doesn't re-walk the same ancestors for each one.
fn in_git_repo(dir: &Path, cache: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(hit) = cache.get(dir) {
        return *hit;
    }
    let result = dir.join(".git").exists()
        || dir
            .parent()
            .is_some_and(|parent| in_git_repo(parent, cache));
    cache.insert(dir.to_path_buf(), result);
    result
}

/// Name of the holding directory `skill_harness_disable`'s universal
/// move-aside disable renames a deployment into, sitting as a sibling of the
/// deployment inside its skills root. Skipped in the normal one-level pass
/// so it's never itself treated as a skill folder, then walked separately
/// (also one level deep) so its contents surface as disabled candidates.
pub(crate) const STUDIO_DISABLED_DIR_NAME: &str = ".skill-studio-disabled";

/// Scan one directory's immediate entries (either an agent skill root itself,
/// or its `.skill-studio-disabled/` holding directory) for skill folders and
/// push a candidate for each into `out`. `studio_disabled` marks every
/// candidate found this way, so `skill_assembly` can surface them as
/// disabled via `DisabledBy::StudioMoved`.
#[allow(clippy::too_many_arguments)]
fn scan_root_entries(
    dir: &Path,
    root: &agents::SkillRoot,
    has_lock_entry: bool,
    scope: &str,
    studio_disabled: bool,
    facts_cache: &mut SkillFactsCache,
    git_cache: &mut HashMap<PathBuf, bool>,
    tokenizer: Option<&CoreBPE>,
    out: &mut Vec<SkillCandidate>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !studio_disabled
            && entry_path.file_name().and_then(|n| n.to_str()) == Some(STUDIO_DISABLED_DIR_NAME)
        {
            continue;
        }
        let sym_meta = fs::symlink_metadata(&entry_path).ok();
        let is_symlink = sym_meta
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        if is_symlink {
            match resolve_symlink(&entry_path) {
                SymlinkResolution::Resolved(target) => {
                    if !target.join("SKILL.md").is_file() {
                        continue;
                    }
                    if let Some(facts) = get_or_compute_facts(facts_cache, &target, tokenizer) {
                        out.push(build_candidate(
                            &entry_path,
                            &target,
                            root,
                            true,
                            Some(target.clone()),
                            None,
                            has_lock_entry,
                            scope,
                            &facts,
                            git_cache,
                            studio_disabled,
                        ));
                    }
                }
                SymlinkResolution::Broken(raw_target) => {
                    out.push(broken_symlink_candidate(
                        &entry_path,
                        root,
                        scope,
                        raw_target,
                        true,
                        None,
                        studio_disabled,
                    ));
                }
                SymlinkResolution::Error(raw_target, message) => {
                    out.push(broken_symlink_candidate(
                        &entry_path,
                        root,
                        scope,
                        raw_target,
                        false,
                        Some(message),
                        studio_disabled,
                    ));
                }
            }
            continue;
        }

        let is_dir = fs::metadata(&entry_path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir || !entry_path.join("SKILL.md").is_file() {
            continue;
        }
        if let Some(facts) = get_or_compute_facts(facts_cache, &entry_path, tokenizer) {
            out.push(build_candidate(
                &entry_path,
                &entry_path,
                root,
                false,
                None,
                None,
                has_lock_entry,
                scope,
                &facts,
                git_cache,
                studio_disabled,
            ));
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

/// Discover every skill directory found by walking the agent skill roots and
/// native plugin caches. Each result is a fact record; no classification or
/// merging happens here. A thin wrapper over `discover_skill_candidates_cached`
/// with a fresh, one-call `SkillFactsCache` - callers that rebuild repeatedly
/// (`skill_refresh`) should keep a `SkillFactsCache` across calls instead, so
/// a folder whose fingerprint hasn't changed skips its expensive facts
/// recomputation.
pub fn discover_skill_candidates(home: &Path, project_paths: &[PathBuf]) -> Vec<SkillCandidate> {
    let mut cache = SkillFactsCache::default();
    discover_skill_candidates_cached(home, project_paths, &mut cache)
}

/// `discover_skill_candidates`, reusing `cache`'s content facts across calls:
/// a skill folder whose `folder_fingerprint` hasn't changed since the last
/// call skips SKILL.md tokenizing and content hashing entirely. Entries for
/// directories not visited this pass (a deleted skill) are evicted at the
/// end, so a long-lived cache never pins memory for skills that are gone.
pub fn discover_skill_candidates_cached(
    home: &Path,
    project_paths: &[PathBuf],
    cache: &mut SkillFactsCache,
) -> Vec<SkillCandidate> {
    cache.begin_pass();
    let out = discover_into(home, project_paths, cache);
    cache.end_pass();
    out
}

fn discover_into(
    home: &Path,
    project_paths: &[PathBuf],
    facts_cache: &mut SkillFactsCache,
) -> Vec<SkillCandidate> {
    let mut out = Vec::new();
    let mut git_cache: HashMap<PathBuf, bool> = HashMap::new();
    let tokenizer = tokenizer();

    for root in agents::skill_roots(home, project_paths) {
        let has_lock_entry = shared_root_has_lock_entry(&root);
        let scope = scope_str(&root);

        scan_root_entries(
            &root.path,
            &root,
            has_lock_entry,
            scope,
            false,
            facts_cache,
            &mut git_cache,
            tokenizer,
            &mut out,
        );
        // The holding directory `skill_harness_disable`'s universal
        // move-aside disable renames deployments into - walked the same way
        // so its entries surface as disabled candidates rather than
        // vanishing from the snapshot entirely.
        scan_root_entries(
            &root.path.join(STUDIO_DISABLED_DIR_NAME),
            &root,
            has_lock_entry,
            scope,
            true,
            facts_cache,
            &mut git_cache,
            tokenizer,
            &mut out,
        );
    }

    // Native plugin enumeration: skills bundled inside a Claude Code or
    // Codex plugin, found by walking their plugin caches for manifests.
    for plugin_skill in plugins::scan_plugin_skills(home) {
        let is_symlink = fs::symlink_metadata(&plugin_skill.skill_dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let Some(facts) = get_or_compute_facts(facts_cache, &plugin_skill.skill_dir, tokenizer)
        else {
            continue;
        };
        let dir_name = plugin_skill
            .skill_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = facts
            .frontmatter
            .as_ref()
            .and_then(|f| f.name.clone())
            .unwrap_or_else(|| dir_name.clone());
        let spec_violations = validate_skill(
            &dir_name,
            facts.frontmatter.as_ref(),
            facts.skill_md_line_count,
        );

        out.push(SkillCandidate {
            name,
            path: plugin_skill.skill_dir.clone(),
            root_label: plugin_skill.plugin.harness.clone(),
            scope: "plugin".to_string(),
            project_path: None,
            is_symlink,
            symlink_target: None,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            plugin: Some(plugin_skill.plugin),
            shared_root_has_lock_entry: false,
            frontmatter_fields: facts.frontmatter_fields.clone(),
            frontmatter: facts.frontmatter.clone(),
            spec_violations,
            has_spec: facts.has_spec,
            folder_bytes: facts.folder_bytes,
            file_count: facts.file_count,
            skill_md_tokens: facts.skill_md_tokens,
            description_tokens: facts.description_tokens,
            content_hash: facts.content_hash.clone(),
            modified_at: facts.modified_at.clone(),
            folder_truncated: facts.folder_truncated,
            in_git_repo: in_git_repo(&plugin_skill.skill_dir, &mut git_cache),
            studio_disabled: false,
            shared_via_whole_dir_link: false,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: does things.\n---\nBody text here.\n"),
        )
        .unwrap();
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("broken symlink candidate found");
        assert!(found.is_symlink);
        assert!(found.symlink_is_broken);
        assert_eq!(found.name, "ghost-skill");
    }

    #[test]
    fn broken_symlink_into_dotagents_still_classifies_dotagents() {
        // The link target doesn't exist, but its path still resolves into
        // .agents/skills, so provenance (which reads symlink_target) can
        // still classify it as dotagents-managed.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let claude_skills = home.join(".claude/skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let link = claude_skills.join("gone-skill");
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join(".agents/skills/gone-skill"), &link).unwrap();

        let candidates = discover_skill_candidates(home, &[]);
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("broken symlink candidate found");
        assert!(found.symlink_is_broken);
        assert_eq!(
            found.symlink_target.as_deref(),
            Some(home.join(".agents/skills/gone-skill").as_path())
        );
        assert_eq!(
            super::super::provenance::classify_source_kind(
                found,
                &super::super::lock_file::SkillLockFile {
                    version: 3,
                    skills: std::collections::HashMap::new(),
                }
            ),
            super::super::provenance::SourceKind::Dotagents
        );
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

        let candidates = discover_skill_candidates(home, &[]);
        let found = candidates
            .iter()
            .find(|c| c.name == "lint-code")
            .expect("plugin skill found");
        let plugin = found.plugin.as_ref().expect("plugin info set");
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

        let candidates = discover_skill_candidates(home, &[]);
        let found = candidates
            .iter()
            .find(|c| c.path == link)
            .expect("symlinked plugin skill found");
        let plugin = found
            .plugin
            .as_ref()
            .expect("plugin info set via canonical target");
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
        assert!(candidates.iter().any(|c| c.name == "legacy-skill"));
    }

    #[test]
    fn tokens_bytes_and_file_count_are_non_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_skill(&home.join(".claude/skills/write-tests"), "write-tests");

        let candidates = discover_skill_candidates(home, &[]);
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
    fn folder_walk_cap_is_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("many-files");
        fs::create_dir_all(&skill_dir).unwrap();
        for i in 0..10 {
            fs::write(skill_dir.join(format!("file-{i}.txt")), b"x").unwrap();
        }

        let walk = walk_folder_capped(&skill_dir, 3, MAX_FOLDER_BYTES);
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
        let walk = walk_folder_capped(&skill_dir, MAX_FOLDER_FILES, 10);
        assert!(walk.truncated);
        assert_eq!(walk.file_count, 0);
        assert_eq!(walk.total_bytes, 0);
        assert!(walk.hashable.is_empty());
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
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
            facts.frontmatter.as_ref().and_then(|f| f.name.clone()),
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

        let candidates = discover_skill_candidates(home, &[]);
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
        let candidates = discover_skill_candidates_cached(home, &[], &mut cache);
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
    fn editing_a_file_and_bumping_its_mtime_invalidates_the_cache_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");

        let mut cache = SkillFactsCache::default();
        let first = discover_skill_candidates_cached(home, &[], &mut cache);
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

        let second = discover_skill_candidates_cached(home, &[], &mut cache);
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
    fn deleted_skill_dir_is_evicted_from_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/find-bugs");
        write_skill(&skill_dir, "find-bugs");

        let mut cache = SkillFactsCache::default();
        discover_skill_candidates_cached(home, &[], &mut cache);
        assert_eq!(cache.entries.len(), 1);

        fs::remove_dir_all(&skill_dir).unwrap();
        let candidates = discover_skill_candidates_cached(home, &[], &mut cache);
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

        let candidates = discover_skill_candidates(home, std::slice::from_ref(&repo_root));
        let found = candidates
            .iter()
            .find(|c| c.name == "tracked-skill")
            .expect("in-repo skill found");
        assert!(found.in_git_repo);
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

        let candidates = discover_skill_candidates(home, &[]);
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

        let candidates = discover_skill_candidates(home, &[]);
        let found = candidates
            .iter()
            .find(|c| c.name == "untracked-skill")
            .expect("skill found");
        assert!(!found.in_git_repo);
    }
}
