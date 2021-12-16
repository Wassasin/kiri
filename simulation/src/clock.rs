use std::{
    ops::Add,
    sync::atomic::{AtomicU64, Ordering},
};

use csma_csma::Clock;
use rand::distributions::uniform::{SampleUniform, UniformInt, UniformSampler};

#[derive(PartialEq, PartialOrd, Debug, Clone, Copy)]
pub struct FakeInstant(pub u64);

#[derive(PartialEq, PartialOrd, Debug, Clone, Copy)]
pub struct FakeDuration(pub u64);

#[derive(Debug)]
pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    pub fn new() -> Self {
        Self {
            now: AtomicU64::new(0),
        }
    }

    pub fn increase(&self, duration: u64) {
        self.now.fetch_add(duration, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    type Instant = FakeInstant;
    type Duration = FakeDuration;

    fn now(&self) -> Self::Instant {
        FakeInstant(self.now.load(Ordering::Relaxed))
    }
}

impl Add<FakeDuration> for FakeInstant {
    type Output = FakeInstant;

    fn add(self, rhs: FakeDuration) -> Self::Output {
        FakeInstant(self.0 + rhs.0)
    }
}

impl SampleUniform for FakeDuration {
    type Sampler = UniformFakeDuration;
}

pub struct UniformFakeDuration(UniformInt<u64>);

impl UniformSampler for UniformFakeDuration {
    type X = FakeDuration;

    fn new<B1, B2>(low: B1, high: B2) -> Self
    where
        B1: rand::distributions::uniform::SampleBorrow<Self::X> + Sized,
        B2: rand::distributions::uniform::SampleBorrow<Self::X> + Sized,
    {
        Self(UniformInt::new(low.borrow().0, high.borrow().0))
    }

    fn new_inclusive<B1, B2>(low: B1, high: B2) -> Self
    where
        B1: rand::distributions::uniform::SampleBorrow<Self::X> + Sized,
        B2: rand::distributions::uniform::SampleBorrow<Self::X> + Sized,
    {
        Self(UniformInt::new_inclusive(low.borrow().0, high.borrow().0))
    }

    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> Self::X {
        FakeDuration(UniformInt::sample(&self.0, rng))
    }
}
