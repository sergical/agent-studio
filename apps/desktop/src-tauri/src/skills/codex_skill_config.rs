// ============================================================================
// Skills Module - codex_skill_config
// Reads and writes Codex's own per-skill disable switch:
// `~/.codex/config.toml` `[[skills.config]]` rows with `path = "<abs SKILL.md
// path>"` and `enabled = false`. Uses `toml_edit` (format-preserving) rather
// than `toml`/`serde` so a hand-edited config.toml's comments, key order, and
// unrelated tables survive a write untouched.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{value, Decor, DocumentMut, Item, Table};

/// `~/.codex/config.toml`.
pub fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

/// Every canonical `SKILL.md` path Codex's own config disables, read from
/// `[[skills.config]]` rows with `enabled = false`. A missing file yields an
/// empty set; a file that fails to parse also yields an empty set (read-only
/// callers, e.g. the scanner, must not fail an entire snapshot rebuild over a
/// malformed config Codex itself would presumably also reject).
pub fn read_disabled_skill_md_paths(home: &Path) -> Vec<PathBuf> {
    let path = codex_config_path(home);
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(doc) = content.parse::<DocumentMut>() else {
        return Vec::new();
    };
    skills_config_rows(&doc)
        .filter(|row| row.get("enabled").and_then(Item::as_bool) == Some(false))
        .filter_map(|row| row.get("path").and_then(Item::as_str))
        .map(|p| fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p)))
        .collect()
}

/// Iterates `[[skills.config]]` rows, tolerating a document with no `skills`
/// table, no `config` array, or a `config` that isn't an array of tables.
fn skills_config_rows(doc: &DocumentMut) -> impl Iterator<Item = &Table> {
    doc.get("skills")
        .and_then(Item::as_table)
        .and_then(|t| t.get("config"))
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flatten()
}

/// Index of the `[[skills.config]]` row whose `path` matches `skill_md_path`,
/// if any.
fn find_row_index(doc: &DocumentMut, skill_md_path: &Path) -> Option<usize> {
    let target = skill_md_path.to_string_lossy();
    skills_config_rows(doc)
        .position(|row| row.get("path").and_then(Item::as_str) == Some(target.as_ref()))
}

/// Adds (or removes) a `[[skills.config]] path = "<skill_md_path>" enabled =
/// false` row so Codex disables (or stops disabling) that skill, preserving
/// every other byte of the file - other tables, comments, and formatting are
/// untouched because this edits the parsed `DocumentMut` in place rather than
/// re-serializing a plain `toml::Value`. Written atomically (temp file +
/// rename). Refuses if the existing file fails to parse, rather than
/// silently discarding whatever's in it.
pub fn set_skill_disabled(home: &Path, skill_md_path: &Path, disabled: bool) -> Result<(), String> {
    let path = codex_config_path(home);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;

    let existing = find_row_index(&doc, skill_md_path);

    if !disabled {
        if let Some(idx) = existing {
            // `toml_edit` 0.23 stores `#` comments as *prefix decor on the next
            // item*, never as standalone nodes. Removing a `[[skills.config]]`
            // row - or the whole `[skills]` table once it empties - would drop
            // whatever comment physically attached to that carrier. To honor
            // this module's promise to preserve hand-edited comments, capture
            // the carrier's prefix first and re-home it onto a surviving
            // successor, so enabling a skill never silently deletes a user
            // annotation near the managed block.
            let array = doc["skills"]["config"]
                .as_array_of_tables_mut()
                .expect("find_row_index only returns Some when this is an array of tables");
            // Only a prefix that actually carries a comment is re-homed; a
            // whitespace-only prefix is left untouched so comment-free configs
            // keep the original byte-for-byte teardown.
            let removed_row_prefix = array
                .get(idx)
                .and_then(|t| t.decor().prefix())
                .and_then(|p| p.as_str())
                .filter(|s| s.contains('#'))
                .map(|s| s.to_string());
            array.remove(idx);
            let array_is_empty = array.is_empty();
            // The row that shifts into `idx` (if any) inherits the removed
            // row's comment. Prepend rather than replace so a comment on the
            // successor survives too.
            let mut carried_prefix: Option<String> = None;
            if let Some(prefix) = removed_row_prefix {
                if idx < array.len() {
                    if let Some(succ) = array.get_mut(idx) {
                        let succ_prefix = succ
                            .decor()
                            .prefix()
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        succ.decor_mut()
                            .set_prefix(format!("{prefix}{succ_prefix}"));
                    }
                } else {
                    // No successor row (the removed row was last). Re-home the
                    // comment out of the block below.
                    carried_prefix = Some(prefix);
                }
            }
            let skills_table = doc["skills"]
                .as_table_mut()
                .expect("skills is a table when config was");
            if array_is_empty {
                skills_table.remove("config");
            }
            let skills_is_empty = skills_table.is_empty();
            if skills_is_empty {
                // Capture the table's prefix (may carry a comment physically
                // just above `[skills]`) before removing it - this is the last
                // use of `skills_table`, after which `doc` is free to borrow.
                let skills_prefix = skills_table
                    .decor()
                    .prefix()
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                let orphan = match (skills_prefix.contains('#'), carried_prefix.take()) {
                    (false, None) => None,
                    (true, None) => Some(skills_prefix),
                    (false, Some(comment)) => Some(comment),
                    (true, Some(comment)) => Some(format!("{skills_prefix}{comment}")),
                };
                let successor_key = if orphan.is_some() {
                    find_successor_key(&doc, "skills")
                } else {
                    None
                };
                doc.as_table_mut().remove("skills");
                if let Some(orphan) = orphan {
                    rehome_orphan_comment(&mut doc, successor_key, &orphan);
                }
            } else if let Some(comment) = carried_prefix.take() {
                // The block survives but the removed row was its last (no
                // successor row): re-home the orphaned comment out of the block
                // onto the next top-level item after `[skills]` (or trailing).
                let successor_key = find_successor_key(&doc, "skills");
                rehome_orphan_comment(&mut doc, successor_key, &comment);
            }
        }
    } else if existing.is_none() {
        let skills_table = doc
            .entry("skills")
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("{} has a non-table top-level `skills` key", path.display()))?;
        let config_array = skills_table
            .entry("config")
            .or_insert_with(|| Item::ArrayOfTables(Default::default()))
            .as_array_of_tables_mut()
            .ok_or_else(|| format!("{} has a non-array `skills.config` key", path.display()))?;
        let mut row = Table::new();
        row["path"] = value(skill_md_path.to_string_lossy().to_string());
        row["enabled"] = value(false);
        config_array.push(row);
    }
    // `existing.is_some() && disabled`: already disabled, nothing to do -
    // idempotent by construction.

    let parent = path.parent().ok_or("config.toml has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    let tmp_path = path.with_extension("toml.tmp");
    fs::write(&tmp_path, doc.to_string())
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to save {}: {e}", path.display())
    })
}

/// Finds the top-level key that immediately follows `target` in document
/// order, or `None` when `target` is absent or is the last key. `DocumentMut`
/// preserves insertion order, so iteration reflects the file's key order.
fn find_successor_key(doc: &DocumentMut, target: &str) -> Option<String> {
    let mut found = false;
    for (key, _) in doc.iter() {
        if found {
            return Some(key.to_string());
        }
        if key == target {
            found = true;
        }
    }
    None
}

/// Re-homes an orphaned comment (prefix decor) onto the surviving top-level
/// item named `successor_key`, or appends it to the document's trailing decor
/// when there is none, so a comment that sat on a removed `[skills]` block (or
/// a removed last row) is never silently lost. The comment is *prepended* to
/// the successor's existing prefix so a comment already on the successor is
/// preserved too.
fn rehome_orphan_comment(doc: &mut DocumentMut, successor_key: Option<String>, comment: &str) {
    if let Some(successor) = successor_key {
        if let Some(item) = doc.get_mut(&successor) {
            rehome_prefix_onto_item(item, comment);
            return;
        }
    }
    let trailing = doc.trailing().as_str().unwrap_or("").to_string();
    doc.set_trailing(format!("{comment}{trailing}"));
}

fn rehome_prefix_onto_item(item: &mut Item, comment: &str) {
    let prepend = |decor: &mut Decor| {
        let existing = decor
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string();
        decor.set_prefix(format!("{comment}{existing}"));
    };
    match item {
        Item::Table(t) => prepend(t.decor_mut()),
        Item::Value(v) => prepend(v.decor_mut()),
        // An array-of-tables has no decor of its own; attach to its first row,
        // which is the carrier for a `#` comment before the first `[[succ]]`.
        Item::ArrayOfTables(aot) => {
            if let Some(first) = aot.get_mut(0) {
                prepend(first.decor_mut());
            }
        }
        Item::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_has_no_disabled_paths() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_disabled_skill_md_paths(tmp.path()).is_empty());
    }

    #[test]
    fn add_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "---\nname: find-bugs\n---\n").unwrap();

        set_skill_disabled(home, &skill_md, true).unwrap();
        let disabled = read_disabled_skill_md_paths(home);
        assert_eq!(disabled, vec![std::fs::canonicalize(&skill_md).unwrap()]);
    }

    #[test]
    fn add_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        set_skill_disabled(home, &skill_md, true).unwrap();
        set_skill_disabled(home, &skill_md, true).unwrap();
        assert_eq!(read_disabled_skill_md_paths(home).len(), 1);
    }

    #[test]
    fn remove_is_idempotent_and_drops_empty_config_array() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        set_skill_disabled(home, &skill_md, true).unwrap();
        set_skill_disabled(home, &skill_md, false).unwrap();
        set_skill_disabled(home, &skill_md, false).unwrap();
        assert!(read_disabled_skill_md_paths(home).is_empty());
        let content = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(!content.contains("[[skills.config]]"));
    }

    #[test]
    fn preserves_unrelated_content_and_comments_byte_for_byte_outside_the_added_table() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        let original = "# a comment\nmodel = \"gpt-5\"\n\n[other]\nfoo = 1\n";
        std::fs::write(codex_config_path(home), original).unwrap();

        set_skill_disabled(home, &skill_md, true).unwrap();
        let updated = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(updated.starts_with(original));
        assert!(updated.contains("[[skills.config]]"));

        set_skill_disabled(home, &skill_md, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn refuses_when_the_file_fails_to_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(codex_config_path(home), "not = [valid").unwrap();

        let err = set_skill_disabled(home, Path::new("/tmp/x/SKILL.md"), true).unwrap_err();
        assert!(err.contains("not valid TOML"));
    }

    // Regression guards for the comment-preservation contract: a `#` comment
    // placed on the managed `[skills]` block (or its rows) must survive an
    // enable. toml_edit stores such comments as prefix decor on the next item,
    // so removing the carrier would silently drop them without re-homing.

    #[test]
    fn preserves_comment_between_skills_header_and_first_config_row_when_enabling_the_first_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_a = home.join("skills/find-bugs/SKILL.md");
        let skill_b = home.join("skills/other/SKILL.md");
        std::fs::create_dir_all(skill_a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(skill_b.parent().unwrap()).unwrap();
        std::fs::write(&skill_a, "x").unwrap();
        std::fs::write(&skill_b, "x").unwrap();

        let original = format!(
            "# top\nmodel = \"gpt-5\"\n\n\
             [skills]\n# my important note\n\
             [[skills.config]]\npath = \"{}\"\nenabled = false\n\
             [[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_a.display(),
            skill_b.display(),
        );
        std::fs::write(codex_config_path(home), &original).unwrap();

        // Enable (remove) the FIRST row: the block survives with row `b`, and
        // the comment between `[skills]` and that first row must not be lost.
        set_skill_disabled(home, &skill_a, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(
            restored.contains("# my important note"),
            "comment was lost:\n{restored}"
        );
        assert!(
            restored.contains(&format!("path = \"{}\"", skill_b.display())),
            "successor row b was lost:\n{restored}"
        );
        assert!(
            restored.contains("[[skills.config]]"),
            "skills block was lost:\n{restored}"
        );
        assert!(
            !restored.contains(&format!("path = \"{}\"", skill_a.display())),
            "row a was not removed:\n{restored}"
        );
    }

    #[test]
    fn preserves_comment_between_skills_header_and_first_config_row_when_enabling_empties_the_block(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        let original = format!(
            "# top\nmodel = \"gpt-5\"\n\n\
             [skills]\n# my important note\n\
             [[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_md.display(),
        );
        std::fs::write(codex_config_path(home), &original).unwrap();

        // Enabling removes the only row, tearing the whole `[skills]` block
        // down; the comment that lived inside the block must survive it.
        set_skill_disabled(home, &skill_md, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(
            restored.contains("# my important note"),
            "comment was lost:\n{restored}"
        );
        assert!(
            restored.contains("# top"),
            "unrelated top comment was lost:\n{restored}"
        );
        assert!(
            !restored.contains("[[skills.config]]"),
            "config row was not removed:\n{restored}"
        );
        assert!(
            !restored.contains("[skills]"),
            "skills table was not removed:\n{restored}"
        );
    }

    #[test]
    fn preserves_comment_above_skills_header_when_enabling_empties_the_block() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        let original = format!(
            "model = \"gpt-5\"\n\n\
             # important preamble above skills\n\
             [skills]\n[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_md.display(),
        );
        std::fs::write(codex_config_path(home), &original).unwrap();

        // The comment sits physically *above* `[skills]` (top-level), bound as
        // the table's prefix decor; removing the now-empty table must re-home
        // it rather than discard it.
        set_skill_disabled(home, &skill_md, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(
            restored.contains("# important preamble above skills"),
            "comment was lost:\n{restored}"
        );
        assert!(
            !restored.contains("[[skills.config]]"),
            "config row was not removed:\n{restored}"
        );
        assert!(
            !restored.contains("[skills]"),
            "skills table was not removed:\n{restored}"
        );
    }

    #[test]
    fn preserves_comment_above_skills_header_when_a_following_table_takes_over() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_md = home.join("skills/find-bugs/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        std::fs::write(&skill_md, "x").unwrap();

        let original = format!(
            "model = \"gpt-5\"\n\n\
             # important preamble above skills\n\
             [skills]\n[[skills.config]]\npath = \"{}\"\nenabled = false\n\n\
             [other]\nfoo = 1\n",
            skill_md.display(),
        );
        std::fs::write(codex_config_path(home), &original).unwrap();

        // `[skills]` is followed by `[other]`; the orphaned comment must be
        // re-homed onto `[other]`'s prefix, and `[other]` stays intact.
        set_skill_disabled(home, &skill_md, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(
            restored.contains("# important preamble above skills"),
            "comment was lost:\n{restored}"
        );
        assert!(
            restored.contains("[other]"),
            "unrelated [other] table was lost:\n{restored}"
        );
        assert!(
            restored.contains("foo = 1"),
            "unrelated content was lost:\n{restored}"
        );
        assert!(
            !restored.contains("[[skills.config]]"),
            "config row was not removed:\n{restored}"
        );
    }

    #[test]
    fn preserves_comment_between_config_rows_when_enabling_the_last_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let skill_a = home.join("skills/find-bugs/SKILL.md");
        let skill_b = home.join("skills/other/SKILL.md");
        std::fs::create_dir_all(skill_a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(skill_b.parent().unwrap()).unwrap();
        std::fs::write(&skill_a, "x").unwrap();
        std::fs::write(&skill_b, "x").unwrap();

        let original = format!(
            "model = \"gpt-5\"\n\n\
             [skills]\n[[skills.config]]\npath = \"{}\"\nenabled = false\n\
             # note before the second row\n\
             [[skills.config]]\npath = \"{}\"\nenabled = false\n",
            skill_a.display(),
            skill_b.display(),
        );
        std::fs::write(codex_config_path(home), &original).unwrap();

        // Enabling (removing) the LAST row leaves row `a` behind; the comment
        // that sat between the two rows has no successor row to inherit it, so
        // it is re-homed out of the block rather than dropped.
        set_skill_disabled(home, &skill_b, false).unwrap();
        let restored = std::fs::read_to_string(codex_config_path(home)).unwrap();
        assert!(
            restored.contains("# note before the second row"),
            "comment was lost:\n{restored}"
        );
        assert!(
            restored.contains(&format!("path = \"{}\"", skill_a.display())),
            "row a was lost:\n{restored}"
        );
        assert!(
            !restored.contains(&format!("path = \"{}\"", skill_b.display())),
            "row b was not removed:\n{restored}"
        );
        assert!(
            restored.contains("[[skills.config]]"),
            "skills block was lost:\n{restored}"
        );
    }
}
