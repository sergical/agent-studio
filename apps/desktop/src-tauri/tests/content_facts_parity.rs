//! Pins `skill-studio-core`'s `ops::scan` content facts (whole-folder digest,
//! token counts, byte/file counters, link facts) to concrete expected values
//! derived from the fixture bytes themselves and from the same primitives
//! (`cl100k_base` tokenizer, length-framed sha256 folder digest) the core
//! implementation uses. Desktop's own duplicate scanner is gone; only
//! `core_content_hash`'s thin wrapper over `ops::skill_content_hash` remains
//! on the desktop side, and the standalone-hash test below pins that
//! wrapper too.

use std::fs;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops::scan;
use skill_studio_core::ports::{OpContext, Runtime};
use skill_studio_core::scope::RuntimeScope;

/// Materializes a `.claude/skills/widget` fixture under `home` with nested
/// directories, an empty file, and a file whose name sorts before every
/// other entry - the same shape the required parity test calls for.
fn write_fixture(home: &std::path::Path) {
    let skill_dir = home.join(".claude/skills/widget");
    fs::create_dir_all(skill_dir.join("nested/deeper")).expect("create nested dirs");
    fs::write(
        skill_dir.join("SKILL.md"),
        b"---\nname: widget\ndescription: A small test widget.\n---\nBody text.\n",
    )
    .expect("write SKILL.md");
    fs::write(skill_dir.join("empty.txt"), b"").expect("write empty file");
    fs::write(
        skill_dir.join("0-first.txt"),
        b"sorts before everything else",
    )
    .expect("write 0-first.txt");
    fs::write(skill_dir.join("nested/note.md"), b"nested note").expect("write nested/note.md");
    fs::write(skill_dir.join("nested/deeper/leaf.md"), b"leaf content")
        .expect("write nested/deeper/leaf.md");
}

/// The fixture's own files, `(rel_path, bytes)`, in the same shape
/// `content_hash_from_walk` hashes. Kept as a single source of truth so the
/// expected digest/byte-count/file-count below are all derived from these
/// bytes rather than hand-copied literals.
fn fixture_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        (
            "SKILL.md",
            b"---\nname: widget\ndescription: A small test widget.\n---\nBody text.\n" as &[u8],
        ),
        ("empty.txt", b""),
        ("0-first.txt", b"sorts before everything else"),
        ("nested/note.md", b"nested note"),
        ("nested/deeper/leaf.md", b"leaf content"),
    ]
}

/// Reimplements `ops::content_hash_from_walk`'s length-framed sha256 digest
/// over the fixture's own known bytes, sorted by rel_path exactly as core
/// sorts them, so the expected digest is derived rather than copied from a
/// prior run.
fn expected_content_hash(files: &[(&'static str, &'static [u8])]) -> String {
    let mut sorted = files.to_vec();
    sorted.sort_by_key(|(rel, _)| *rel);
    let mut hasher = Sha256::new();
    for (rel, bytes) in sorted {
        let rel_bytes = rel.as_bytes();
        hasher.update((rel_bytes.len() as u64).to_le_bytes());
        hasher.update(rel_bytes);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// cl100k_base token count, matching core's `count_tokens`
/// (`ops.rs::tokenizer`).
fn expected_token_count(text: &str) -> u32 {
    let bpe = tiktoken_rs::cl100k_base().expect("embedded cl100k_base vocab");
    bpe.encode_with_special_tokens(text).len() as u32
}

fn core_deployment(home: &std::path::Path) -> skill_studio_core::dto::DeploymentDto {
    core_deployment_named(home, "widget")
}

/// As [`core_deployment`], but for a skill name other than `widget`.
fn core_deployment_named(
    home: &std::path::Path,
    name: &str,
) -> skill_studio_core::dto::DeploymentDto {
    let scope = RuntimeScope::fixture(home);
    let catalog = Arc::new(HarnessCatalog::builtin());
    let ports = skill_studio_host::default_ports(home.join("core-leases"), catalog);
    let rt = Runtime::new(&scope, ports).expect("runtime");
    let ctx = OpContext::uncancellable(CorrelationId("content-facts-parity".into()));
    let inventory = scan(&rt, &ctx, &ScanRequest::default()).expect("scan");
    inventory
        .skills
        .into_iter()
        .find(|s| s.name.0 == name)
        .unwrap_or_else(|| panic!("core discovered the {name} skill"))
        .deployments
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("core discovered a deployment for {name}"))
}

/// Materializes a skill deployed as a symlink: the real skill directory sits
/// under `.agents/skills/` under a name the symlink itself does not reuse, so
/// scanning the universal root does not also surface a second
/// `linked-widget` deployment and `deployments.into_iter().next()` above
/// stays unambiguous. `.claude/skills/linked-widget` is the symlink to it -
/// the shape needed to exercise `is_symlink`/`resolved_path` for real.
fn write_symlink_fixture(home: &std::path::Path) {
    let real_dir = home.join(".agents/skills/linked-widget-target");
    fs::create_dir_all(&real_dir).expect("create real skill dir");
    fs::write(
        real_dir.join("SKILL.md"),
        b"---\nname: linked-widget\ndescription: A symlinked test widget.\n---\nBody text.\n",
    )
    .expect("write SKILL.md");

    let claude_skills = home.join(".claude/skills");
    fs::create_dir_all(&claude_skills).expect("create .claude/skills");
    std::os::unix::fs::symlink(&real_dir, claude_skills.join("linked-widget"))
        .expect("create symlink");
}

#[test]
fn core_content_hash_matches_desktop_live_content_hash() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().canonicalize().expect("canonicalize temp dir");
    write_fixture(&home);

    let core = core_deployment(&home);
    let expected = expected_content_hash(&fixture_files());

    assert_eq!(
        core.content_hash, expected,
        "core content_hash must match the digest hand-derived from the fixture's own bytes"
    );
}

#[test]
fn core_token_and_folder_counters_match_desktop() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().canonicalize().expect("canonicalize temp dir");
    write_fixture(&home);

    let core = core_deployment(&home);
    let files = fixture_files();

    let expected_folder_bytes: u64 = files.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    let expected_file_count = files.len() as u32;
    let skill_md_text =
        std::str::from_utf8(files.iter().find(|(rel, _)| *rel == "SKILL.md").unwrap().1).unwrap();
    let expected_skill_md_tokens = expected_token_count(skill_md_text);
    let expected_description_tokens = expected_token_count("widget: A small test widget.");

    assert_eq!(core.folder_bytes, expected_folder_bytes, "folder_bytes");
    assert_eq!(core.file_count, expected_file_count, "file_count");
    assert_eq!(
        core.skill_md_tokens, expected_skill_md_tokens,
        "skill_md_tokens"
    );
    assert_eq!(
        core.description_tokens, expected_description_tokens,
        "description_tokens"
    );
}

/// Pins the standalone digest (`ops::skill_content_hash`, the entry point
/// mutation guards in `skill_add.rs`/`skill_independent_copy.rs` call with no
/// scan in hand) to both the scanned `content_hash` of the same folder and
/// the desktop's own thin wrapper (`core_content_hash::live_skill_content_hash`),
/// so all three ways of getting this folder's hash agree.
#[test]
fn core_standalone_content_hash_matches_desktop_live_content_hash() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().canonicalize().expect("canonicalize temp dir");
    write_fixture(&home);
    let skill_dir = home.join(".claude/skills/widget");

    let scope = RuntimeScope::fixture(&home);
    let catalog = Arc::new(HarnessCatalog::builtin());
    let ports = skill_studio_host::default_ports(home.join("core-leases"), catalog);
    let rt = Runtime::new(&scope, ports).expect("runtime");

    let ctx = OpContext::uncancellable(CorrelationId("content-facts-parity".into()));
    let core_hash =
        skill_studio_core::ops::skill_content_hash(rt.ports.fs.as_ref(), &ctx, &skill_dir)
            .expect("core standalone hash");
    let desktop_hash =
        skill_studio_lib::skills::core_content_hash::live_skill_content_hash(&skill_dir)
            .expect("desktop live content hash");

    assert!(
        !desktop_hash.is_empty(),
        "the fixture must produce a real hash for the comparison to mean anything"
    );
    assert_eq!(
        core_hash, desktop_hash,
        "the standalone digests must agree, not only the scanned field"
    );

    // And the standalone digest must equal the one the scan reports, so the
    // guards and the scanned deployment never disagree about one folder.
    assert_eq!(
        core_hash,
        core_deployment(&home).content_hash,
        "standalone and scanned digests must agree"
    );
}

/// Pins core's `is_symlink`/`resolved_path`/`symlink_is_broken`/
/// `symlink_error` to the concrete facts each fixture implies, for both an
/// ordinary directory and a real symlinked deployment.
#[test]
fn core_link_facts_match_desktop() {
    let plain_dir = tempfile::tempdir().expect("temp dir");
    let plain_home = plain_dir
        .path()
        .canonicalize()
        .expect("canonicalize temp dir");
    write_fixture(&plain_home);

    let plain_core = core_deployment(&plain_home);

    // The plain fixture is a genuine directory, not a symlink.
    assert!(!plain_core.is_symlink, "is_symlink");
    assert_eq!(plain_core.resolved_path, None, "resolved_path");
    assert!(!plain_core.symlink_is_broken, "symlink_is_broken");
    assert_eq!(plain_core.symlink_error, None, "symlink_error");

    let link_dir = tempfile::tempdir().expect("temp dir");
    let link_home = link_dir
        .path()
        .canonicalize()
        .expect("canonicalize temp dir");
    write_symlink_fixture(&link_home);

    let link_core = core_deployment_named(&link_home, "linked-widget");
    let expected_target = link_home
        .join(".agents/skills/linked-widget-target")
        .canonicalize()
        .expect("canonicalize symlink target");

    assert!(link_core.is_symlink, "is_symlink");
    assert_eq!(
        link_core.resolved_path,
        Some(expected_target),
        "resolved_path"
    );
    assert!(!link_core.symlink_is_broken, "symlink_is_broken");
    assert_eq!(link_core.symlink_error, None, "symlink_error");
}
