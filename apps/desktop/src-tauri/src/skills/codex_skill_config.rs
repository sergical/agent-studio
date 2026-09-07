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

/// Converts a removed table header's surrounding text into text that can sit
/// before the next table header. `toml_edit` writes the header's suffix before
/// adding its own newline, so the newline must move with a non-empty suffix.
fn table_decor_as_prefix(table: &Table) -> String {
    let prefix = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default();
    let suffix = table
        .decor()
        .suffix()
        .and_then(|suffix| suffix.as_str())
        .unwrap_or_default();
    if !prefix.contains('#') && !suffix.contains('#') {
        return String::new();
    }

    let mut text = prefix.to_string();
    if !suffix.is_empty() {
        text.push_str(suffix);
        if !suffix.ends_with('\n') {
            text.push('\n');
        }
    }
    text
}

fn prepend_table_decor(table: &mut Table, text: &str) {
    if text.is_empty() {
        return;
    }
    let existing = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_string();
    table.decor_mut().set_prefix(format!("{text}{existing}"));
}

struct OrphanedTableDecor {
    position: Option<isize>,
    text: String,
}

fn next_table_position(table: &Table, removed_position: isize) -> Option<isize> {
    let mut next = table
        .position()
        .filter(|position| *position > removed_position);
    for (_, item) in table.iter() {
        let child_next = match item {
            Item::Table(child) => next_table_position(child, removed_position),
            Item::ArrayOfTables(array) => array
                .iter()
                .filter_map(|child| next_table_position(child, removed_position))
                .min(),
            _ => None,
        };
        next = next.into_iter().chain(child_next).min();
    }
    next
}

fn table_at_position_mut(table: &mut Table, position: isize) -> Option<&mut Table> {
    if table.position() == Some(position) {
        return Some(table);
    }
    for (_, item) in table.iter_mut() {
        let found = match item {
            Item::Table(child) => table_at_position_mut(child, position),
            Item::ArrayOfTables(array) => array
                .iter_mut()
                .find_map(|child| table_at_position_mut(child, position)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Moves decor orphaned by a removed table to the next table in document
/// order, or to the document trailing text when no table follows it.
fn rehome_table_decor(doc: &mut DocumentMut, removed_position: Option<isize>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(next_position) =
        removed_position.and_then(|position| next_table_position(doc.as_table(), position))
    {
        let next = table_at_position_mut(doc.as_table_mut(), next_position)
            .expect("next_table_position returned an existing table");
        prepend_table_decor(next, text);
        return;
    }

    let trailing = doc.trailing().as_str().unwrap_or_default();
    doc.set_trailing(format!("{text}{trailing}"));
}

fn rehome_table_decor_blocks(doc: &mut DocumentMut, mut blocks: Vec<OrphanedTableDecor>) {
    blocks.retain(|block| !block.text.is_empty());
    blocks.sort_by(|left, right| right.position.cmp(&left.position));
    for block in blocks {
        rehome_table_decor(doc, block.position, &block.text);
    }
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
            let (removed_decor, array_is_empty) = {
                let array = doc["skills"]["config"]
                    .as_array_of_tables_mut()
                    .expect("find_row_index only returns Some when this is an array of tables");
                let removed = array.remove(idx);
                let removed_decor = OrphanedTableDecor {
                    position: removed.position(),
                    text: table_decor_as_prefix(&removed),
                };
                if let Some(next_row) = array.get_mut(idx) {
                    prepend_table_decor(next_row, &removed_decor.text);
                    (None, false)
                } else {
                    (Some(removed_decor), array.is_empty())
                }
            };

            let mut orphaned_decor = removed_decor.into_iter().collect::<Vec<_>>();
            let remove_skills = {
                let skills_table = doc["skills"]
                    .as_table_mut()
                    .expect("skills is a table when config was");
                if array_is_empty {
                    skills_table.remove("config");
                }
                if skills_table.is_empty() {
                    orphaned_decor.push(OrphanedTableDecor {
                        position: skills_table.position(),
                        text: table_decor_as_prefix(skills_table),
                    });
                    true
                } else {
                    false
                }
            };
            if remove_skills {
                doc.as_table_mut().remove("skills");
            }
            rehome_table_decor_blocks(&mut doc, orphaned_decor);
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

    fn write_config(home: &Path, content: &str) {
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(codex_config_path(home), content).unwrap();
    }

    fn read_config(home: &Path) -> String {
        std::fs::read_to_string(codex_config_path(home)).unwrap()
    }

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
    fn removing_first_row_rehomes_comments_to_the_next_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_config(
            home,
            "[skills] # skills header\n# between skills and first row\n\n# first row note\n[[skills.config]] # first row header\npath = \"/skills/first/SKILL.md\"\nenabled = false\n\n# second row note\n[[skills.config]]\npath = \"/skills/second/SKILL.md\"\nenabled = false\n",
        );

        set_skill_disabled(home, Path::new("/skills/first/SKILL.md"), false).unwrap();

        let updated = read_config(home);
        assert!(!updated.contains("/skills/first/SKILL.md"));
        assert!(updated.contains("/skills/second/SKILL.md"));
        for comment in [
            "# between skills and first row",
            "# first row note",
            "# first row header",
            "# second row note",
        ] {
            assert_eq!(updated.matches(comment).count(), 1, "{updated}");
        }
        assert!(
            updated.find("# first row header").unwrap()
                < updated.find("# second row note").unwrap(),
            "{updated}"
        );
    }

    #[test]
    fn removing_middle_and_last_rows_rehomes_each_rows_comments_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_config(
            home,
            "[[skills.config]]\npath = \"/skills/first/SKILL.md\"\nenabled = false\n\n# middle row note\n[[skills.config]] # middle row header\npath = \"/skills/middle/SKILL.md\"\nenabled = false\n\n# last row note\n[[skills.config]] # last row header\npath = \"/skills/last/SKILL.md\"\nenabled = false\n\n# neighboring table note\n[other]\nvalue = 1\n",
        );

        set_skill_disabled(home, Path::new("/skills/middle/SKILL.md"), false).unwrap();
        let after_middle = read_config(home);
        assert!(
            after_middle.find("# middle row note").unwrap()
                < after_middle.find("# last row note").unwrap(),
            "{after_middle}"
        );

        set_skill_disabled(home, Path::new("/skills/last/SKILL.md"), false).unwrap();
        let after_last = read_config(home);
        assert!(!after_last.contains("/skills/middle/SKILL.md"));
        assert!(!after_last.contains("/skills/last/SKILL.md"));
        for comment in [
            "# middle row note",
            "# middle row header",
            "# last row note",
            "# last row header",
            "# neighboring table note",
        ] {
            assert_eq!(after_last.matches(comment).count(), 1, "{after_last}");
        }
        assert!(
            after_last.find("# last row header").unwrap()
                < after_last.find("# neighboring table note").unwrap(),
            "{after_last}"
        );
    }

    #[test]
    fn removing_sole_row_and_skills_table_preserves_all_header_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_config(
            home,
            "model = \"gpt-5\"\n\n# skills table note\n[skills] # skills header\n# between skills and row\n\n# sole row note\n[[skills.config]] # sole row header\npath = \"/skills/only/SKILL.md\"\nenabled = false\n\n# other table note\n[other]\nvalue = 1\n",
        );

        set_skill_disabled(home, Path::new("/skills/only/SKILL.md"), false).unwrap();

        let updated = read_config(home);
        assert!(!updated.contains("[skills]"));
        assert!(!updated.contains("[[skills.config]]"));
        for comment in [
            "# skills table note",
            "# skills header",
            "# between skills and row",
            "# sole row note",
            "# sole row header",
            "# other table note",
        ] {
            assert_eq!(updated.matches(comment).count(), 1, "{updated}");
        }
        let positions: Vec<_> = [
            "# skills table note",
            "# skills header",
            "# between skills and row",
            "# sole row note",
            "# sole row header",
            "# other table note",
            "[other]",
        ]
        .iter()
        .map(|part| updated.find(part).unwrap())
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{updated}"
        );
    }

    #[test]
    fn removing_sole_row_rehomes_parent_after_intervening_table_in_document_order() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_config(
            home,
            "# sole row note\n[[skills.config]] # sole row header\npath = \"/skills/only/SKILL.md\"\nenabled = false\n\n# other table note\n[other]\nvalue = 1\n\n# skills table note\n[skills] # skills header\n",
        );

        set_skill_disabled(home, Path::new("/skills/only/SKILL.md"), false).unwrap();

        let updated = read_config(home);
        assert!(!updated.contains("[skills]"));
        assert!(!updated.contains("[[skills.config]]"));
        for comment in [
            "# sole row note",
            "# sole row header",
            "# other table note",
            "# skills table note",
            "# skills header",
        ] {
            assert_eq!(updated.matches(comment).count(), 1, "{updated}");
        }
        let positions: Vec<_> = [
            "# sole row note",
            "# sole row header",
            "# other table note",
            "[other]",
            "# skills table note",
            "# skills header",
        ]
        .iter()
        .map(|part| updated.find(part).unwrap())
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{updated}"
        );
    }

    #[test]
    fn enabling_then_disabling_again_keeps_rehomed_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_config(
            home,
            "[skills]\n# keep this target note\n[[skills.config]] # keep this target header\npath = \"/skills/target/SKILL.md\"\nenabled = false\n\n# survivor note\n[[skills.config]]\npath = \"/skills/survivor/SKILL.md\"\nenabled = false\n",
        );

        let target = Path::new("/skills/target/SKILL.md");
        set_skill_disabled(home, target, false).unwrap();
        set_skill_disabled(home, target, true).unwrap();

        let updated = read_config(home);
        assert!(updated.contains("/skills/target/SKILL.md"));
        assert!(updated.contains("/skills/survivor/SKILL.md"));
        for comment in [
            "# keep this target note",
            "# keep this target header",
            "# survivor note",
        ] {
            assert_eq!(updated.matches(comment).count(), 1, "{updated}");
        }
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
