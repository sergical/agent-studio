//! Deterministic bench estate: a generated home matching the measured shape
//! of a real skill collection (skill-estate-content-facts: 63% global, name
//! lengths p90 33 chars, description lengths median 249 chars, spread across
//! the four harnesses plus the shared `.agents` root). Same `(n, seed)` in,
//! same [`FixtureBuilder`] tree out, so `benches/scan.rs` and any test that
//! generates it agree.
//!
//! Behind the same `cfg` as [`crate::testing`], since it builds on
//! [`FixtureBuilder`] and exists only for benches and tests.

use std::path::PathBuf;

use crate::identity::UNIVERSAL_ROOT_RELATIVE;
use crate::testing::FixtureBuilder;

/// One of the four first-class harnesses, or the shared universal root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Universal,
    Claude,
    Codex,
    OpenCode,
    Pi,
}

const HARNESSES: [Category; 4] = [
    Category::Claude,
    Category::Codex,
    Category::OpenCode,
    Category::Pi,
];

/// A generated bench estate: the fixture tree, plus the relative project
/// directories a [`crate::scope::ProjectSelection::Explicit`] scope needs to
/// see the project-scoped skills, and the per-skill numbers the shape test
/// checks.
#[derive(Debug, Clone)]
pub struct GeneratedEstate {
    /// The fixture tree, home-relative like every [`crate::testing::fixtures`] builder.
    pub builder: FixtureBuilder,
    /// Home-relative project directories, each with a `.git` marker.
    pub project_dirs: Vec<PathBuf>,
    /// Per-skill numbers the shape test measures against skill-estate-content-facts.
    pub stats: EstateStats,
}

/// The measured shape a bench estate is checked against.
#[derive(Debug, Clone)]
pub struct EstateStats {
    /// Total skills generated.
    pub skill_count: usize,
    /// Skills placed in a global (home-level) root, not a project root.
    pub global_count: usize,
    /// Skills placed under the shared `.agents/skills` root and symlinked
    /// into one harness.
    pub universal_count: usize,
    /// Every skill's frontmatter `name` length, in the order generated.
    pub name_lengths: Vec<usize>,
    /// Every skill's frontmatter `description` length, in the order generated.
    pub description_lengths: Vec<usize>,
}

/// A splitmix64 generator: small, dependency-free, and fully determined by
/// its seed, so two calls with the same `(n, seed)` to [`estate`] walk the
/// exact same sequence of decisions.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// A value in `[lo, hi)`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next_u64() as usize) % (hi - lo)
    }
}

/// In-place Fisher-Yates, so a count-based assignment (e.g. "the first 63%
/// are global") doesn't cluster by index.
fn shuffle<T>(rng: &mut Rng, v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = rng.range(0, i + 1);
        v.swap(i, j);
    }
}

const NAME_WORDS: &[&str] = &[
    "lint", "format", "test", "build", "deploy", "review", "audit", "scan", "fetch", "parse",
    "merge", "sync", "validate", "render", "index", "cache", "queue", "trace", "debug", "profile",
    "migrate", "backup", "restore", "notify", "schedule", "filter", "sort", "compress", "encrypt",
    "decrypt",
];

const DESCRIPTION_WORDS: &[&str] = &[
    "automates",
    "reviews",
    "generates",
    "summarizes",
    "checks",
    "extracts",
    "transforms",
    "monitors",
    "coordinates",
    "documents",
    "analyzes",
    "prepares",
    "verifies",
    "schedules",
    "tracks",
    "compares",
    "annotates",
    "packages",
    "migrates",
    "tests",
];

/// Builds a spec-valid kebab-case name of exactly (or one under, when the
/// last word is trimmed at a hyphen) `target_len` characters, prefixed with
/// `s{i:04}` so it is unique across the estate regardless of length.
fn build_name(i: usize, target_len: usize) -> String {
    let prefix = format!("s{i:04}");
    if target_len <= prefix.len() {
        return prefix;
    }
    let filler_target = target_len - prefix.len() - 1;
    let mut filler = String::new();
    let mut w = 0usize;
    while filler.chars().count() < filler_target {
        if !filler.is_empty() {
            filler.push('-');
        }
        filler.push_str(NAME_WORDS[w % NAME_WORDS.len()]);
        w += 1;
    }
    let filler: String = filler.chars().take(filler_target).collect();
    let filler = filler.trim_end_matches('-');
    if filler.is_empty() {
        prefix
    } else {
        format!("{prefix}-{filler}")
    }
}

/// Builds a description of exactly `target_len` characters (spec only
/// requires 1 to 1024 non-empty chars; it need not be a full sentence).
fn build_description(i: usize, target_len: usize) -> String {
    let mut desc = format!("Skill {i:04} helps with");
    let mut w = 0usize;
    while desc.chars().count() < target_len {
        desc.push(' ');
        desc.push_str(DESCRIPTION_WORDS[(i + w) % DESCRIPTION_WORDS.len()]);
        w += 1;
    }
    desc.chars().take(target_len.max(1)).collect()
}

fn harness_root_path(category: Category, is_global: bool) -> &'static str {
    match (category, is_global) {
        (Category::Claude, _) => ".claude/skills",
        (Category::Codex, _) => ".codex/skills",
        (Category::OpenCode, true) => ".config/opencode/skills",
        (Category::OpenCode, false) => ".opencode/skills",
        (Category::Pi, true) => ".pi/agent/skills",
        (Category::Pi, false) => ".pi/skills",
        (Category::Universal, _) => unreachable!("universal skills have no harness root"),
    }
}

/// Places one skill: an ordinary harness skill is a directory with its own
/// `SKILL.md`; a universal skill is a real directory under
/// `.agents/skills` symlinked into one harness root, the same shape
/// `fixtures::basic`'s `gamma` uses.
fn place_skill(
    mut b: FixtureBuilder,
    root_prefix: &str,
    category: Category,
    is_global: bool,
    i: usize,
    name: &str,
    description: &str,
) -> FixtureBuilder {
    let skill_md =
        format!("---\nname: {name}\ndescription: {description}\n---\nBody text for {name}.\n");
    match category {
        Category::Universal => {
            let skill_dir = format!("{root_prefix}{UNIVERSAL_ROOT_RELATIVE}/{name}");
            b = b
                .dir(&skill_dir)
                .file(&format!("{skill_dir}/SKILL.md"), skill_md.as_bytes());
            let harness_root = harness_root_path(HARNESSES[i % HARNESSES.len()], is_global);
            let link = format!("{root_prefix}{harness_root}/{name}");
            // The symlink sits in a directory `harness_root.split('/').count()`
            // levels below `root_prefix` (one per path segment of the root,
            // not counting the skill's own name segment), so climbing back to
            // `root_prefix` takes exactly that many `../`s regardless of
            // whether the harness root is two segments (e.g. `.claude/skills`)
            // or three (e.g. `.config/opencode/skills`).
            let depth = harness_root.split('/').count();
            let ups = "../".repeat(depth);
            let target = format!("{ups}{UNIVERSAL_ROOT_RELATIVE}/{name}");
            b.alias(&link, &target)
        }
        other => {
            let harness_root = harness_root_path(other, is_global);
            let skill_dir = format!("{root_prefix}{harness_root}/{name}");
            b.dir(&skill_dir)
                .file(&format!("{skill_dir}/SKILL.md"), skill_md.as_bytes())
        }
    }
}

/// Generates an `n`-skill home matching skill-estate-content-facts's
/// measured shape, deterministically from `seed`: the same `(n, seed)`
/// always builds the same tree in the same order.
pub fn estate(n: usize, seed: u64) -> GeneratedEstate {
    let mut rng = Rng::new(seed);

    let global_count = ((n as f64) * 0.63).round() as usize;
    let universal_count = ((n as f64) * 0.20).round() as usize;
    let per_harness = (n - universal_count) / HARNESSES.len();

    let mut categories: Vec<Category> = Vec::with_capacity(n);
    categories.extend(std::iter::repeat_n(Category::Universal, universal_count));
    for harness in HARNESSES {
        categories.extend(std::iter::repeat_n(harness, per_harness));
    }
    while categories.len() < n {
        categories.push(Category::Claude);
    }
    shuffle(&mut rng, &mut categories);

    let mut is_global = vec![true; global_count.min(n)];
    is_global.resize(n, false);
    shuffle(&mut rng, &mut is_global);

    // A bulk/tail split, not a smooth distribution: nearest-rank p90 lands
    // squarely inside the tail only when the tail is both past the 90th
    // rank and narrow enough that any rank within it reads close to 33.
    let bulk_count = ((n as f64) * 0.80).round() as usize;
    let tail_count = n - bulk_count;
    let mut name_lengths = Vec::with_capacity(n);
    for _ in 0..bulk_count {
        name_lengths.push(rng.range(8, 28));
    }
    for _ in 0..tail_count {
        name_lengths.push(rng.range(32, 35));
    }
    shuffle(&mut rng, &mut name_lengths);

    let mut description_lengths = Vec::with_capacity(n);
    for i in 0..n {
        let sign: isize = if i % 2 == 0 { 1 } else { -1 };
        let delta = rng.range(0, 50) as isize;
        description_lengths.push((249 + sign * delta).max(10) as usize);
    }
    shuffle(&mut rng, &mut description_lengths);

    let project_count = n - global_count.min(n);
    let num_projects = project_count.div_ceil(8);

    let mut builder = FixtureBuilder::new();
    let mut project_dirs = Vec::with_capacity(num_projects);
    for k in 0..num_projects {
        let dir = format!("proj{k}");
        builder = builder.dir(&format!("{dir}/.git"));
        project_dirs.push(PathBuf::from(dir));
    }

    let mut project_cursor = 0usize;
    for i in 0..n {
        let root_prefix = if is_global[i] {
            String::new()
        } else {
            let k = project_cursor % num_projects.max(1);
            project_cursor += 1;
            format!("proj{k}/")
        };
        let name = build_name(i, name_lengths[i]);
        let description = build_description(i, description_lengths[i]);
        builder = place_skill(
            builder,
            &root_prefix,
            categories[i],
            is_global[i],
            i,
            &name,
            &description,
        );
    }

    GeneratedEstate {
        builder,
        project_dirs,
        stats: EstateStats {
            skill_count: n,
            global_count: global_count.min(n),
            universal_count,
            name_lengths,
            description_lengths,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::frontmatter::{parse_frontmatter, FrontmatterParseResult};
    use crate::testing::golden::unique_temp_dir;

    fn percentile(sorted: &[usize], p: f64) -> f64 {
        let rank = ((p * sorted.len() as f64).ceil() as usize)
            .max(1)
            .min(sorted.len());
        sorted[rank - 1] as f64
    }

    fn median(sorted: &[usize]) -> f64 {
        let n = sorted.len();
        if n % 2 == 1 {
            sorted[n / 2] as f64
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
        }
    }

    fn within_5_percent(actual: f64, target: f64) -> bool {
        (actual - target).abs() <= target * 0.05
    }

    /// Guards: a bench that silently regenerated a different tree between
    /// runs would make `bench/baseline.json` compare noise, not signal.
    #[test]
    fn estate_same_seed_generates_an_identical_tree() {
        let a = estate(400, 1);
        let b = estate(400, 1);
        assert_eq!(format!("{:?}", a.builder), format!("{:?}", b.builder));
        assert_eq!(a.project_dirs, b.project_dirs);
    }

    /// Walks a materialized fixture tree, collecting the home-relative
    /// directory of every real (non-symlink) `SKILL.md` and the home-relative
    /// path of every symlink, without descending into a symlinked directory
    /// (which would otherwise revisit a universal skill's real `SKILL.md` a
    /// second time through its harness alias).
    fn walk_real(
        dir: &Path,
        rel_prefix: &Path,
        skill_dirs: &mut Vec<PathBuf>,
        links: &mut Vec<PathBuf>,
    ) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let rel = rel_prefix.join(entry.file_name());
            let file_type = entry.file_type().expect("file_type");
            if file_type.is_symlink() {
                links.push(rel);
            } else if file_type.is_dir() {
                walk_real(&entry.path(), &rel, skill_dirs, links);
            } else if entry.file_name() == "SKILL.md" {
                skill_dirs.push(rel_prefix.to_path_buf());
            }
        }
    }

    /// Guards: a generator that drifts from skill-estate-content-facts would
    /// make the scan bench measure a home nothing like a real one. Measures
    /// the tree the generator actually built on disk (paths, symlinks,
    /// frontmatter), not the intent vectors it built the tree from, so a bug
    /// in `place_skill` (e.g. a dangling universal symlink) would fail this
    /// test even though `EstateStats` looks fine.
    #[test]
    fn estate_400_matches_the_measured_shape_within_5_percent() {
        let generated = estate(400, 1);
        let dir = unique_temp_dir("bench-estate-shape");
        std::fs::create_dir_all(&dir).expect("create shape test home");
        generated
            .builder
            .materialize(&dir)
            .expect("materialize bench estate");

        let mut skill_dirs = Vec::new();
        let mut links = Vec::new();
        walk_real(&dir, Path::new(""), &mut skill_dirs, &mut links);

        for link in &links {
            assert!(
                dir.join(link).exists(),
                "symlink {} is dangling",
                link.display()
            );
        }

        let mut names = Vec::with_capacity(skill_dirs.len());
        let mut descriptions = Vec::with_capacity(skill_dirs.len());
        let mut global_count = 0usize;
        for skill_dir in &skill_dirs {
            let is_project = skill_dir
                .components()
                .next()
                .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with("proj"));
            if !is_project {
                global_count += 1;
            }
            let content = std::fs::read_to_string(dir.join(skill_dir).join("SKILL.md"))
                .expect("read SKILL.md");
            let fm = match parse_frontmatter(&content) {
                FrontmatterParseResult::Valid(fm) => fm,
                other => panic!(
                    "{}: expected valid frontmatter, got {other:?}",
                    skill_dir.display()
                ),
            };
            names.push(fm.name.expect("name").chars().count());
            descriptions.push(fm.description.expect("description").chars().count());
        }

        std::fs::remove_dir_all(&dir).ok();

        let skill_count = skill_dirs.len();
        assert_eq!(skill_count, 400, "expected 400 real skill directories");

        names.sort_unstable();
        let name_p90 = percentile(&names, 0.9);
        assert!(
            within_5_percent(name_p90, 33.0),
            "measured name length p90 {name_p90} not within 5% of 33"
        );

        descriptions.sort_unstable();
        let description_median = median(&descriptions);
        assert!(
            within_5_percent(description_median, 249.0),
            "measured description length median {description_median} not within 5% of 249"
        );

        let global_fraction = global_count as f64 / skill_count as f64;
        assert!(
            within_5_percent(global_fraction, 0.63),
            "measured global fraction {global_fraction} not within 5% of 0.63"
        );

        let universal_fraction = links.len() as f64 / skill_count as f64;
        assert!(
            universal_fraction >= 0.19,
            "measured universal fraction {universal_fraction} below the 19% floor"
        );
    }
}
