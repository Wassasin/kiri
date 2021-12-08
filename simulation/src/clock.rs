use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct Time(u64);

pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    pub fn new() -> Self {
        Self {
            now: AtomicU64::new(0),
        }
    }

    pub fn now(&self) -> Time {
        Time(self.now.load(Ordering::Relaxed))
    }

    pub fn increase(&self, duration: u64) {
        self.now.fetch_add(duration, Ordering::Relaxed);
    }
}
