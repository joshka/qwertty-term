//! M2-C termio runtime spike.
//!
//! Two implementations of `Thread.zig`'s writer-loop semantics behind one
//! [`driver::Driver`] trait and one [`mailbox`] API:
//!   * [`threads`] — OS thread + `polling` + a hand-rolled timer wheel.
//!   * [`tokio_rt`] — tokio current-thread runtime + `Notify` + `tokio::time`.
//!
//! The point of the spike is to pick one by measurement (see
//! `docs/adr/002-termio-runtime.md`), while proving the [`mailbox`] API is
//! identical either way. The benchmark bin (`src/bin/bench.rs`) drives both.

pub mod driver;
pub mod mailbox;
pub mod threads;
pub mod tokio_rt;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A [`driver::Handler`] that just counts what it sees. Used by the benchmark
/// and smoke tests to observe the loop without any real terminal work.
#[derive(Clone, Default)]
pub struct CountingHandler {
    /// Total non-resize messages delivered via `on_messages`.
    pub messages: Arc<AtomicU64>,
    /// Times `on_resize` fired (i.e. coalesced resize bursts).
    pub resizes: Arc<AtomicU64>,
    /// Times `on_sync_reset` fired.
    pub sync_resets: Arc<AtomicU64>,
    /// Last resize dims seen, packed `(cols as u64) << 16 | rows`.
    pub last_resize: Arc<AtomicU64>,
}

impl driver::Handler for CountingHandler {
    fn on_messages(&mut self, batch: &[mailbox::Message]) {
        self.messages
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
    }
    fn on_resize(&mut self, cols: u16, rows: u16) {
        self.resizes.fetch_add(1, Ordering::Relaxed);
        self.last_resize
            .store(((cols as u64) << 16) | rows as u64, Ordering::Relaxed);
    }
    fn on_sync_reset(&mut self) {
        self.sync_resets.fetch_add(1, Ordering::Relaxed);
    }
}
