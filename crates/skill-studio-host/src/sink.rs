//! [`EventSink`] adapters: discard, or print one JSON line per notice.

use skill_studio_core::ports::{CoreNotice, EventSink};

/// Discards every notice. The default when nobody is listening.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn notify(&self, _notice: CoreNotice) {}
}

/// Prints one JSON line per notice to stderr, for CLI and debug builds.
#[derive(Debug, Default, Clone, Copy)]
pub struct StderrSink;

impl EventSink for StderrSink {
    fn notify(&self, notice: CoreNotice) {
        match serde_json::to_string(&notice) {
            Ok(line) => eprintln!("{line}"),
            Err(e) => eprintln!("{{\"notice\":\"unserializable\",\"error\":{e:?}}}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_accepts_every_notice_without_panicking() {
        NoopSink.notify(CoreNotice::Recovered { events: Vec::new() });
    }
}
