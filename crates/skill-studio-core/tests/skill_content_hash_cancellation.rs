//! Proves `ops::skill_content_hash` is interruptible mid-walk, not just at
//! its entry checkpoint: an already-cancelled [`OpContext`] must fail the
//! call outright rather than return a hash computed over a partial walk.

use std::sync::Arc;

use skill_studio_core::error::ErrorCode;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::CorrelationId;
use skill_studio_core::ops;
use skill_studio_core::ports::{OpContext, Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::{
    AlwaysCancel, FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory, RecordingSink,
};

#[test]
fn skill_content_hash_fails_cancelled_instead_of_hashing() {
    let fs = FixtureBuilder::new()
        .dir("/h")
        .file("/h/skill/SKILL.md", b"---\nname: giant\ndescription: A skill folder with several files to walk.\n---\nBody.\n")
        .file("/h/skill/a.txt", b"aaaa")
        .file("/h/skill/b.txt", b"bbbb")
        .file("/h/skill/c.txt", b"cccc")
        .build_fs();
    let ports = Ports {
        fs: Arc::new(fs),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
    let ctx = OpContext {
        correlation_id: CorrelationId("cancelled".into()),
        cancel: Arc::new(AlwaysCancel),
    };

    let err = ops::skill_content_hash(
        rt.ports.fs.as_ref(),
        &ctx,
        &rt.scope.home.lexical.join("skill"),
    )
    .expect_err("an already-cancelled context must not yield a hash");
    assert_eq!(err.code, ErrorCode::Cancelled);
}
