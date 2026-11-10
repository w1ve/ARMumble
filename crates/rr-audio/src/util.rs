//! Small lock-free helpers shared by the engine.

use std::sync::atomic::{AtomicU32, Ordering};

/// An `f32` that can be shared between threads.
#[derive(Debug)]
pub(crate) struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub(crate) fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }

    pub(crate) fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Raise the value to `v` if larger. Only valid for non-negative values,
    /// whose IEEE bit patterns sort like the numbers themselves.
    pub(crate) fn max_assign(&self, v: f32) {
        let _ = self.0.fetch_max(v.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Read and reset to zero.
    pub(crate) fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}
