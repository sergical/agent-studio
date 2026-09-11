//! `diagnose` must never propose an action that the named operation refuses.
//!
//! This is the invariant behind a real regression: `diagnose` told the user to
//! run `preview_frontmatter_repair` on a hand-made skill with broken
//! frontmatter, and `preview_frontmatter_repair` answered `unsupported`
//! ("deployment is read-only"), because the core's gate had dropped the
//! desktop's carve-out for `Manual` owners. Both operations passed their own
//! tests. Only the pair was wrong.
//!
//! So the assertion here is over the pair, across every fixture: take every
//! `NextAction` that `diagnose` emits, invoke the operation it names, and
//! require that the operation accepts it. A next action the user cannot act on
//! is worse than no next action at all.

use std::sync::Arc;

use skill_studio_core::dto::{NextAction, RepairPreviewRequest, ScanRequest};
use skill_studio_core::ops::{diagnose, preview_frontmatter_repair};
use skill_studio_core::ports::Runtime;
use skill_studio_core::testing::fixtures;
use skill_studio_core::testing::golden::{ctx, materialized_ports, scope_for, unique_temp_dir};

use skill_studio_host::{FileLease, RealFs};

#[test]
fn every_next_action_diagnose_emits_is_accepted_by_the_operation_it_names() {
    let mut checked = 0usize;

    for (name, builder) in fixtures::all() {
        let dir = unique_temp_dir(name);
        std::fs::create_dir_all(&dir).unwrap();
        let home = dir.canonicalize().unwrap();
        builder
            .materialize(&home)
            .unwrap_or_else(|e| panic!("materialize {name}: {e}"));

        let scope = scope_for(name, &home);
        let ports = || {
            materialized_ports(
                Arc::new(RealFs::new()),
                Arc::new(FileLease::new(home.join(".leases"))),
            )
        };
        let rt = Runtime::new(&scope, ports()).expect("runtime");
        let diagnosis = diagnose(&rt, &ctx(), &ScanRequest::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        for issue in &diagnosis.issues {
            let NextAction::PreviewRepair { deployment_id } = &issue.next_action else {
                continue;
            };
            checked += 1;
            let request = RepairPreviewRequest {
                deployment_id: deployment_id.clone(),
            };
            // A fresh runtime per call: `diagnose` released its shared lease,
            // and re-entering it here would deadlock rather than test anything.
            let rt = Runtime::new(&scope, ports()).expect("runtime");
            if let Err(err) = preview_frontmatter_repair(&rt, &ctx(), &request) {
                panic!(
                    "fixture {name}: diagnose reported {:?} on skill {:?} and told the user to \
                     preview a repair, but preview_frontmatter_repair refused it: {} ({})",
                    issue.kind,
                    issue.skill,
                    err.message,
                    err.code.as_str()
                );
            }
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    assert!(
        checked > 0,
        "no fixture produced a preview_repair next action, so this test proved nothing; \
         a fixture with repairable frontmatter must exist"
    );
}
