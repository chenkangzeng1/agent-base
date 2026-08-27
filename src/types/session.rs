pub use agent_types::SessionId;

pub trait SessionIdGenerator: Send + Sync {
    fn generate(&self) -> SessionId;
}

pub struct AtomicU64SessionIdGenerator {
    counter: std::sync::atomic::AtomicU64,
}

impl AtomicU64SessionIdGenerator {
    pub fn new(start: u64) -> Self {
        Self {
            counter: std::sync::atomic::AtomicU64::new(start),
        }
    }
}

impl Default for AtomicU64SessionIdGenerator {
    fn default() -> Self {
        Self::new(1)
    }
}

impl SessionIdGenerator for AtomicU64SessionIdGenerator {
    fn generate(&self) -> SessionId {
        SessionId::new(
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }
}

pub struct UuidSessionIdGenerator;

impl SessionIdGenerator for UuidSessionIdGenerator {
    fn generate(&self) -> SessionId {
        SessionId::with_external_id(0, uuid::Uuid::new_v4().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_display_with_and_without_external_id() {
        assert_eq!(SessionId::new(42).to_string(), "42");
        assert_eq!(
            SessionId::with_external_id(42, "ext").to_string(),
            "42(ext)"
        );
    }

    #[test]
    fn atomic_generator_increments() {
        let g = AtomicU64SessionIdGenerator::new(10);
        assert_eq!(g.generate().id, 10);
        assert_eq!(g.generate().id, 11);

        let g = AtomicU64SessionIdGenerator::default();
        assert_eq!(g.generate().id, 1);
    }

    #[test]
    fn uuid_generator_sets_external_id() {
        let sid = UuidSessionIdGenerator.generate();
        assert_eq!(sid.id, 0);
        assert!(sid.external_id.is_some());
    }
}
