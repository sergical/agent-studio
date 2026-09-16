#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::time::Duration;

use skill_studio_core::skill_backup_reservation::BackupCopyLimits;
#[cfg(test)]
use skill_studio_core::skill_copy_removal::{recover_copy_removal, EVENT_KIND};
#[cfg(test)]
use skill_studio_core::skill_service::ScopedSkillService;

#[cfg(test)]
use super::event_store::EventStore;

pub(crate) fn removal_limits() -> BackupCopyLimits {
    BackupCopyLimits {
        max_bytes: 256 * 1024 * 1024,
        max_entries: 20_000,
        max_depth: 64,
    }
}

#[cfg(test)]
pub(crate) fn recover_at_startup(store: &EventStore, home: &Path) -> Result<(), String> {
    let rows = store.interrupted_events_of_kind(EVENT_KIND)?;
    if rows.is_empty() {
        return Ok(());
    }
    let projects = super::project_discovery::discover_skill_projects(home)
        .into_iter()
        .filter(|path| path != home)
        .collect::<Vec<_>>();
    let scope = super::skill_scope_config::desktop_skill_scope(home, &projects)?;
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    for row in rows {
        // Recovery must finish in journal order. A conflict keeps this and later
        // operations interrupted, visible in History, and available for retry.
        recover_copy_removal(
            &mut service,
            store,
            &row,
            removal_limits(),
            Some(Duration::from_secs(30)),
        )
        .map_err(|error| format!("event {} remains interrupted: {error}", row.id))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::event_store::{EventDraft, EventStatus};
    use super::*;

    fn draft(kind: &str) -> EventDraft {
        EventDraft {
            kind: kind.into(),
            skill: "recovery-fixture".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::json!({"invalid": "intent"}),
            inverse: None,
            backup_dir: None,
            restorable: false,
        }
    }

    #[test]
    fn startup_ignores_completed_failed_and_other_operations() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        store.record("done", draft(EVENT_KIND)).unwrap();
        store.finish("done", EventStatus::Done).unwrap();
        store.record("failed", draft(EVENT_KIND)).unwrap();
        store.finish("failed", EventStatus::Failed).unwrap();
        store.record("other", draft("remove")).unwrap();
        store.reconcile_at_startup().unwrap();
        // With no removal to recover, startup does not read scope settings.
        recover_at_startup(&store, &temp.path().join("nonexistent-home")).unwrap();
        assert_eq!(store.get("other").unwrap().unwrap().status, "interrupted");
        assert_eq!(store.get("done").unwrap().unwrap().status, "done");
        assert_eq!(store.get("failed").unwrap().unwrap().status, "failed");
    }

    #[test]
    fn invalid_oldest_removal_stays_visible_and_stops_later_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        store.record("first", draft(EVENT_KIND)).unwrap();
        store.record("second", draft(EVENT_KIND)).unwrap();
        store.reconcile_at_startup().unwrap();
        let rows = store.interrupted_events_of_kind(EVENT_KIND).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        let error = recover_at_startup(&store, &home).unwrap_err();
        assert!(error.contains("event first remains interrupted"), "{error}");
        for id in ["first", "second"] {
            let row = store.get(id).unwrap().unwrap();
            assert_eq!(row.status, "interrupted");
            assert!(!row.restorable);
            assert_eq!(row.payload, draft(EVENT_KIND).payload);
        }
    }
}
