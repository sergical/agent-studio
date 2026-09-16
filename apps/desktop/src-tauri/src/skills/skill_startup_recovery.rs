use super::event_store::{EventRow, EventStore};
use rusqlite::OptionalExtension;
use skill_studio_core::skill_service::{ScopedSkillService, SkillScope};

#[cfg(test)]
pub(crate) fn recover_all(scope: SkillScope, store: &EventStore) -> Result<usize, String> {
    recover_with_scope(&scope.home, store, || Ok(scope.clone()))
}

fn recover_with_scope(
    home: &std::path::Path,
    store: &EventStore,
    mut load_scope: impl FnMut() -> Result<SkillScope, String>,
) -> Result<usize, String> {
    recover_in_order(store, |row| {
        match row.kind.as_str() {
            "make_independent_copy" => {
                return super::skill_independent_copy::reconcile_interrupted_independent_copy(
                    store, home, row,
                );
            }
            "restore" => {
                return super::skill_independent_copy::reconcile_interrupted_independent_copy_restore(
                    store, home, row,
                );
            }
            "materialize_then_disable" => {
                return super::skill_materialize::reconcile_interrupted_convert_then_disable(
                    store, row,
                );
            }
            _ => {}
        }
        if row.kind == "repair_skill_frontmatter" {
            return super::skill_frontmatter_repair::reconcile_interrupted_frontmatter_repair(
                store, home, row,
            );
        }
        let _transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
        let mut service =
            ScopedSkillService::bind(load_scope()?).map_err(|error| error.to_string())?;
        if row.kind == skill_studio_core::skill_copy_removal::EVENT_KIND {
            return skill_studio_core::skill_copy_removal::recover_copy_removal(
                &mut service,
                store,
                row,
                super::skill_copy_recovery::removal_limits(),
                Some(std::time::Duration::from_secs(30)),
            )
            .map(|_| ());
        }
        if super::skill_copy_repair::is_copy_event(&row.kind) {
            return super::skill_copy_repair::recover(&mut service, store, row);
        }
        Err(format!(
            "Event {} ({}) requires unsupported startup recovery",
            row.id, row.kind
        ))
    })
}

pub(crate) fn recover_at_startup(store: &EventStore, home: &std::path::Path) -> Result<(), String> {
    let mut scope = None;
    recover_with_scope(home, store, || {
        if let Some(scope) = &scope {
            return Ok(Clone::clone(scope));
        }
        let projects = super::skill_project_authority::scoped_projects(home, [])?;
        let configured = super::skill_scope_config::desktop_skill_scope(home, &projects)?;
        scope = Some(configured.clone());
        Ok(configured)
    })
    .map(|_| ())
}

fn oldest(store: &EventStore) -> Result<Option<EventRow>, String> {
    let id: Option<String> = store.conn.query_row(
        "SELECT event.id FROM events AS event WHERE event.status IN ('pending', 'interrupted') AND NOT EXISTS (SELECT 1 FROM events AS restoration WHERE restoration.id = event.reverted_by AND restoration.status = 'done') ORDER BY event.rowid LIMIT 1",
        [], |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;
    id.map(|id| {
        store
            .get(&id)?
            .ok_or_else(|| "Startup recovery event disappeared".into())
    })
    .transpose()
}

fn recover_in_order(
    store: &EventStore,
    mut recover: impl FnMut(&EventRow) -> Result<(), String>,
) -> Result<usize, String> {
    for count in 0..32 {
        let Some(row) = oldest(store)? else {
            return Ok(count);
        };
        recover(&row).map_err(|error| format!("{}: {error}", row.id))?;
        let current = store
            .get(&row.id)?
            .ok_or("Startup recovery event disappeared")?;
        if !matches!(current.status.as_str(), "done" | "failed") {
            return Err(format!("Startup recovery left event {} unresolved", row.id));
        }
    }
    match oldest(store)? {
        None => Ok(32),
        Some(row) => Err(format!(
            "Startup recovery reached its 32-event limit; next event is {}",
            row.id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::event_store::{EventDraft, EventStatus};
    use super::*;

    fn record(store: &EventStore, id: &str) {
        store
            .record(
                id,
                EventDraft {
                    kind: "repair_copy_frontmatter".into(),
                    skill: "sample".into(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: serde_json::Value::Null,
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
    }

    #[test]
    fn recovery_stops_at_the_first_conflict_and_retries_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        record(&store, "first");
        record(&store, "second");
        store.reconcile_at_startup().unwrap();
        let mut visited = Vec::new();
        let error = recover_in_order(&store, |row| {
            visited.push(row.id.clone());
            Err("external replacement".into())
        })
        .unwrap_err();
        assert!(error.contains("first: external replacement"));
        assert_eq!(visited, ["first"]);
        let count = recover_in_order(&store, |row| {
            visited.push(row.id.clone());
            store.finish(&row.id, EventStatus::Failed)
        })
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(visited, ["first", "first", "second"]);
        assert!(oldest(&store).unwrap().is_none());
    }

    #[test]
    fn legacy_recovery_does_not_require_scope_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        store
            .record(
                "legacy",
                EventDraft {
                    kind: "materialize_then_disable".into(),
                    skill: "sample".into(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: serde_json::Value::Null,
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        store.reconcile_at_startup().unwrap();
        let error = recover_with_scope(temp.path(), &store, || {
            panic!("legacy recovery must not load unrelated scope settings")
        })
        .unwrap_err();
        assert!(error.contains("root"), "{error}");
        assert_eq!(store.get("legacy").unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn mixed_recovery_stops_before_newer_legacy_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        let kinds = [
            "repair_copy_frontmatter",
            "make_independent_copy",
            "materialize_then_disable",
        ];
        for kind in kinds {
            store
                .record(
                    kind,
                    EventDraft {
                        kind: kind.into(),
                        skill: "sample".into(),
                        harness: None,
                        scope: Some("global".into()),
                        project_path: None,
                        payload: serde_json::Value::Null,
                        inverse: None,
                        backup_dir: None,
                        restorable: false,
                    },
                )
                .unwrap();
        }
        store.reconcile_at_startup().unwrap();
        let mut visited = Vec::new();
        let error = recover_in_order(&store, |row| {
            visited.push(row.kind.clone());
            Err("external replacement".into())
        })
        .unwrap_err();
        assert!(error.contains("repair_copy_frontmatter: external replacement"));
        assert_eq!(visited, ["repair_copy_frontmatter"]);
        for kind in kinds {
            assert_eq!(store.get(kind).unwrap().unwrap().status, "interrupted");
        }
        let mut resumed = Vec::new();
        let count = recover_in_order(&store, |row| {
            resumed.push(row.kind.clone());
            store.finish(&row.id, EventStatus::Failed)
        })
        .unwrap();
        assert_eq!(count, 3);
        assert_eq!(resumed, kinds);
    }

    #[test]
    fn completed_restoration_settles_legacy_event_but_failed_restoration_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        record(&store, "legacy");
        record(&store, "restoration");
        store.reconcile_at_startup().unwrap();
        store.claim_event_restore("legacy", "restoration").unwrap();
        store.finish("restoration", EventStatus::Failed).unwrap();
        assert_eq!(oldest(&store).unwrap().unwrap().id, "legacy");
        store.finish("restoration", EventStatus::Done).unwrap();
        assert!(oldest(&store).unwrap().is_none());
    }

    #[test]
    fn unfinished_recovery_is_not_reported_as_success() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        record(&store, "pending");
        let error = recover_in_order(&store, |_| Ok(())).unwrap_err();
        assert!(error.contains("left event pending unresolved"));
        assert_eq!(store.get("pending").unwrap().unwrap().status, "pending");
    }
}
