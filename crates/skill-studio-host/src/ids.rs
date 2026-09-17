//! [`IdSource`] over a monotonic ULID generator.

use std::sync::Mutex;

use skill_studio_core::identity::EventId;
use skill_studio_core::ports::IdSource;

/// `IdSource` backed by `ulid::Generator`, which bumps the random part
/// within one millisecond so two ids generated back to back still sort.
pub struct UlidIds {
    generator: Mutex<ulid::Generator>,
}

impl UlidIds {
    /// Builds a fresh generator.
    pub fn new() -> Self {
        UlidIds {
            generator: Mutex::new(ulid::Generator::new()),
        }
    }
}

impl Default for UlidIds {
    fn default() -> Self {
        Self::new()
    }
}

impl IdSource for UlidIds {
    fn next_event_id(&self) -> EventId {
        let mut generator = self.generator.lock().expect("ulid generator lock poisoned");
        loop {
            // `generate` errs only when a millisecond's 80 bits of random
            // tail are exhausted; retry on the next tick rather than fail.
            if let Ok(id) = generator.generate() {
                return EventId::from_ulid(id);
            }
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_in_generation_order() {
        let ids = UlidIds::new();
        let a = ids.next_event_id();
        let b = ids.next_event_id();
        assert!(a.0 < b.0);
    }
}
