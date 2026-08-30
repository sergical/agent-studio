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

use toml_edit::{value, DocumentMut, Item, Table};

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
            let array = doc["skills"]["config"]
                .as_array_of_tables_mut()
                .expect("find_row_index only returns Some when this is an array of tables");
            array.remove(idx);
            let array_is_empty = array.is_empty();
            let skills_table = doc["skills"]
                .as_table_mut()
                .expect("skills is a table when config was");
            if array_is_empty {
                skills_table.remove("config");
            }
            if skills_table.is_empty() {
                doc.as_table_mut().remove("skills");
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
}
