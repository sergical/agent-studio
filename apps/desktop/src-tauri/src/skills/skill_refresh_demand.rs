use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SkillRefreshPosition {
    pub instance_id: String,
    pub generation: String,
}

pub(super) struct RefreshDemand {
    instance_id: String,
    requested: AtomicU64,
    completed: AtomicU64,
}

impl Default for RefreshDemand {
    fn default() -> Self {
        Self {
            instance_id: ulid::Ulid::new().to_string(),
            requested: AtomicU64::new(0),
            completed: AtomicU64::new(0),
        }
    }
}

impl RefreshDemand {
    pub(super) fn position(&self, generation: u64) -> SkillRefreshPosition {
        SkillRefreshPosition {
            instance_id: self.instance_id.clone(),
            generation: generation.to_string(),
        }
    }

    pub(super) fn request(&self) -> u64 {
        self.requested
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                Some(value.saturating_add(1))
            })
            .unwrap_or_else(|value| value)
            .saturating_add(1)
    }

    pub(super) fn is_pending(&self) -> bool {
        let completed = self.completed.load(Ordering::SeqCst);
        let requested = self.requested.load(Ordering::SeqCst);
        requested == u64::MAX || requested > completed
    }

    pub(super) fn begin(&self) -> RefreshBatch<'_> {
        RefreshBatch {
            demand: self,
            through: self.requested.load(Ordering::SeqCst),
        }
    }
}

pub(super) struct RefreshBatch<'a> {
    demand: &'a RefreshDemand,
    through: u64,
}

impl RefreshBatch<'_> {
    pub(super) fn position(&self) -> SkillRefreshPosition {
        self.demand.position(self.through)
    }

    pub(super) fn complete(self) {
        self.demand
            .completed
            .fetch_max(self.through, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn successful_build_covers_a_burst_of_requests() {
        let demand = RefreshDemand::default();
        assert!(!demand.is_pending());
        for _ in 0..10_000 {
            demand.request();
        }
        demand.begin().complete();
        assert!(!demand.is_pending());
    }

    #[test]
    fn failed_build_keeps_demand_pending() {
        let demand = RefreshDemand::default();
        demand.request();
        let _failed = demand.begin();
        assert!(demand.is_pending());
        demand.begin().complete();
        assert!(!demand.is_pending());
    }

    #[test]
    fn request_during_build_needs_another_build() {
        let demand = Arc::new(RefreshDemand::default());
        demand.request();
        let started = Arc::new(Barrier::new(2));
        let requested = Arc::new(Barrier::new(2));
        let worker_demand = demand.clone();
        let worker_started = started.clone();
        let worker_requested = requested.clone();
        let worker = std::thread::spawn(move || {
            let batch = worker_demand.begin();
            worker_started.wait();
            worker_requested.wait();
            batch.complete();
        });
        started.wait();
        demand.request();
        requested.wait();
        worker.join().unwrap();
        assert!(demand.is_pending());
        demand.begin().complete();
        assert!(!demand.is_pending());
    }

    #[test]
    fn receipts_keep_instance_and_exact_generation() {
        let demand = RefreshDemand::default();
        let first = demand.position(demand.request());
        let batch = demand.begin();
        let later = demand.position(demand.request());
        assert_eq!(first, batch.position());
        assert_eq!(first.instance_id, later.instance_id);
        assert_ne!(first.generation, later.generation);
        assert_ne!(
            first.instance_id,
            RefreshDemand::default().position(1).instance_id
        );
        let large = demand.position(9_007_199_254_740_993);
        assert_eq!(
            serde_json::to_value(large).unwrap()["generation"],
            "9007199254740993"
        );
    }

    #[test]
    fn exhaustion_never_wraps_into_a_clean_state() {
        let demand = RefreshDemand {
            instance_id: "test".into(),
            requested: AtomicU64::new(u64::MAX - 1),
            completed: AtomicU64::new(u64::MAX - 1),
        };
        demand.request();
        demand.request();
        demand.begin().complete();
        assert!(demand.is_pending());
    }
}
