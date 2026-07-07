//! The swap chain: N per-frame state slots, gated by a semaphore, so the CPU
//! can build frame *k+1* while the GPU is still drawing frame *k*.
//!
//! Port of the `SwapChain` + `FrameState` machinery in Ghostty's
//! `src/renderer/generic.zig` (commit `2da015cd6`, lines ~230-430), lifted out
//! of the comptime-generic `Renderer` into a standalone type parameterized over
//! [`GpuBackend`].
//!
//! # Slots and the semaphore
//!
//! The chain holds `N` [`FrameSlot`]s (`N = GpuBackend::SWAP_CHAIN_COUNT`).
//! Each slot owns the per-frame GPU state that would otherwise race between CPU
//! and GPU: the render target, the uniform/cell/background instance buffers,
//! and the two atlas textures. A counting semaphore with `N` permits guards
//! slot availability: [`SwapChain::next_frame`] waits for a permit and hands
//! out the next slot; the permit is returned when that frame's GPU work
//! completes ([`FrameGuard::release`], driven by the [`Frame`] completion
//! hook).
//!
//! # Two modes behind one API (plan decision 3)
//!
//! - **Sync (day one, permits = 1)** — degenerate double-buffering: exactly one
//!   slot, and the caller completes each frame with `waitUntilCompleted` before
//!   asking for the next. Serializes CPU and GPU but is simple and correct; the
//!   plan calls this "acceptable day one". The permit is released inline when
//!   `waitUntilCompleted` returns.
//! - **Async (permits = 3)** — real triple buffering: three slots, frames
//!   committed with an `addCompletedHandler:` block that releases the permit
//!   from the GPU-completion thread. The CPU can be up to two frames ahead.
//!
//! The mode is [`SwapChainMode`]; the *only* behavioral difference at this
//! layer is the number of live permits and where the permit is released — the
//! slot-handout API is identical, so a window host can flip modes without
//! renderer changes. `SWAP_CHAIN_COUNT` bounds the maximum; sync mode simply
//! caps live permits at 1.
//!
//! [`Frame`]: crate::metal::Frame

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::gpu::GpuBackend;
use crate::wire::{CellBg, CellText, Uniforms};

/// How the swap chain paces CPU vs GPU. See module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapChainMode {
    /// One live slot; frames completed with `waitUntilCompleted`. Day-one
    /// default (plan decision 3).
    Sync,
    /// Up to `SWAP_CHAIN_COUNT` live slots; frames completed via a GPU
    /// completion handler.
    Async,
}

/// A small counting semaphore (permits-based), matching upstream's
/// `std.Thread.Semaphore` usage: `wait` blocks for a permit, `post` returns
/// one. `std` has no stable semaphore, so this is a `Mutex` + `Condvar`.
#[derive(Debug)]
struct Semaphore {
    inner: Mutex<usize>,
    cvar: Condvar,
}

impl Semaphore {
    fn new(permits: usize) -> Self {
        Self {
            inner: Mutex::new(permits),
            cvar: Condvar::new(),
        }
    }

    /// Acquire one permit, blocking until one is available. Port of
    /// `frame_sema.wait()`.
    fn wait(&self) {
        let mut permits = self.inner.lock().expect("swap-chain semaphore poisoned");
        while *permits == 0 {
            permits = self
                .cvar
                .wait(permits)
                .expect("swap-chain semaphore poisoned");
        }
        *permits -= 1;
    }

    /// Release one permit. Port of `frame_sema.post()`.
    fn post(&self) {
        let mut permits = self.inner.lock().expect("swap-chain semaphore poisoned");
        *permits += 1;
        self.cvar.notify_one();
    }

    #[cfg(test)]
    fn available(&self) -> usize {
        *self.inner.lock().expect("swap-chain semaphore poisoned")
    }
}

/// Per-frame GPU state. Port of `generic.zig`'s `FrameState`, reduced to what
/// exists after chunks R1/R2 (the custom-shader state and background-image
/// buffer are R6+; they slot in here later without changing the swap-chain
/// API).
///
/// Every field is state that could be in a CPU/GPU data race while a frame is
/// in flight, which is exactly why it's duplicated per slot.
pub struct FrameSlot<B: GpuBackend> {
    /// One `Uniforms` struct (upstream starts this at capacity 1).
    pub uniforms: B::Buffer<Uniforms>,
    /// Cell (glyph) instance buffer; grows as the grid does.
    pub cells: B::Buffer<CellText>,
    /// Cell background instance buffer.
    pub cells_bg: B::Buffer<CellBg>,
    /// Grayscale glyph atlas texture.
    pub grayscale: B::Texture,
    /// Color (emoji/bitmap) atlas texture.
    pub color: B::Texture,
    /// The IOSurface-backed render target for this slot.
    pub target: B::Target,
}

impl<B: GpuBackend> FrameSlot<B> {
    /// Build one slot with all buffers/textures/target at minimal size
    /// (upstream starts everything at size 1 and resizes on demand). Port of
    /// `FrameState.init`.
    fn new(backend: &B) -> Result<Self, B::Error> {
        use crate::gpu::{TextureFormat, TextureOptions, TextureUsage};

        let uniforms = backend.new_buffer::<Uniforms>(1)?;
        let cells = backend.new_buffer::<CellText>(1)?;
        let cells_bg = backend.new_buffer::<CellBg>(1)?;
        let grayscale = backend.new_texture(
            TextureOptions {
                format: TextureFormat::R8Unorm,
                usage: TextureUsage::SHADER_READ,
            },
            1,
            1,
            None,
        )?;
        let color = backend.new_texture(
            TextureOptions {
                format: TextureFormat::Bgra8Unorm,
                usage: TextureUsage::SHADER_READ,
            },
            1,
            1,
            None,
        )?;
        let target = backend.new_target(1, 1)?;

        Ok(Self {
            uniforms,
            cells,
            cells_bg,
            grayscale,
            color,
            target,
        })
    }

    /// Resize this slot's render target. Port of `FrameState.resize` (the
    /// custom-shader intermediate resize is R6+). The old target is dropped.
    pub fn resize(&mut self, backend: &B, width: usize, height: usize) -> Result<(), B::Error> {
        self.target = backend.new_target(width, height)?;
        Ok(())
    }
}

/// The swap chain. Port of `generic.zig`'s `SwapChain`.
pub struct SwapChain<B: GpuBackend> {
    /// Per-frame slots. `next_frame` cycles through these round-robin.
    slots: Vec<FrameSlot<B>>,
    /// Round-robin index of the most recently handed-out slot. Port of
    /// `frame_index`.
    frame_index: usize,
    /// The availability semaphore, shared with each outstanding [`FrameGuard`]
    /// so the guard can `post` on completion. Port of `frame_sema`.
    sema: Arc<Semaphore>,
    /// Pacing/completion mode.
    mode: SwapChainMode,
    /// Set once `deinit` has run so a defunct chain ignores further use. Port
    /// of `defunct`.
    defunct: Arc<AtomicBool>,
}

impl<B: GpuBackend> SwapChain<B> {
    /// Build the swap chain. Port of `SwapChain.init`.
    ///
    /// `mode` picks the live-permit count: `Sync` → 1 permit (one slot in
    /// play at a time), `Async` → `SWAP_CHAIN_COUNT` permits (full triple
    /// buffering). All `SWAP_CHAIN_COUNT` slots are always allocated so a mode
    /// switch needs no reallocation.
    pub fn new(backend: &B, mode: SwapChainMode) -> Result<Self, B::Error> {
        let count = B::SWAP_CHAIN_COUNT.max(1);
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            slots.push(FrameSlot::new(backend)?);
        }

        let permits = match mode {
            SwapChainMode::Sync => 1,
            SwapChainMode::Async => count,
        };

        Ok(Self {
            slots,
            frame_index: 0,
            sema: Arc::new(Semaphore::new(permits)),
            mode,
            defunct: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The pacing mode.
    pub fn mode(&self) -> SwapChainMode {
        self.mode
    }

    /// Number of allocated slots (`SWAP_CHAIN_COUNT`).
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The index [`SwapChain::next_frame`] would hand out next, without
    /// advancing or acquiring a permit. Used by the cell engine to sync the
    /// upcoming slot's atlas texture before drawing into it.
    pub fn peek_next_index(&self) -> usize {
        (self.frame_index + 1) % self.slots.len()
    }

    /// Mutable access to a slot by index (for out-of-band per-slot resource
    /// updates like atlas-texture sync). Callers must not hold this across a
    /// [`SwapChain::next_frame`] that hands out the same slot.
    pub fn slot_mut(&mut self, index: usize) -> &mut FrameSlot<B> {
        &mut self.slots[index]
    }

    /// Acquire the next frame slot, blocking on the semaphore until one is
    /// free. Port of `SwapChain.nextFrame`. Must be paired with releasing the
    /// returned [`FrameGuard`] (directly in sync mode, or from the frame's
    /// completion handler in async mode).
    ///
    /// Returns `None` if the chain is defunct (upstream `error.Defunct`).
    pub fn next_frame(&mut self) -> Option<FrameGuard<'_, B>> {
        if self.defunct.load(Ordering::Acquire) {
            return None;
        }
        self.sema.wait();
        self.frame_index = (self.frame_index + 1) % self.slots.len();
        let index = self.frame_index;
        Some(FrameGuard {
            index,
            slot: &mut self.slots[index],
            sema: Arc::clone(&self.sema),
            released: false,
        })
    }

    /// Build a permit-releasing closure for a [`FrameGuard`]'s index, callable
    /// from a [`Frame`](crate::metal::Frame) completion hook (async mode). The
    /// closure `post`s the semaphore exactly once regardless of health.
    pub fn release_hook(&self) -> impl Fn() + Send + 'static {
        let sema = Arc::clone(&self.sema);
        move || sema.post()
    }

    /// Wait for all in-flight frames to finish and mark the chain defunct.
    /// Port of `SwapChain.deinit`'s drain (draining before dropping GPU
    /// state). Idempotent.
    pub fn deinit(&mut self) {
        if self.defunct.swap(true, Ordering::AcqRel) {
            return;
        }
        // Reacquire every permit so we know no frame is still drawing.
        let permits = match self.mode {
            SwapChainMode::Sync => 1,
            SwapChainMode::Async => self.slots.len(),
        };
        for _ in 0..permits {
            self.sema.wait();
        }
    }
}

/// A borrowed frame slot plus the means to return its semaphore permit. Port of
/// the `nextFrame`/`releaseFrame` pairing.
///
/// In **sync** mode the caller releases this (via [`FrameGuard::release`])
/// right after `frame.complete(true)` returns. In **async** mode the caller
/// instead installs [`SwapChain::release_hook`] as part of the frame's
/// completion callback and lets the guard's permit be posted from there — in
/// that case the guard is consumed with [`FrameGuard::detach`] so its `Drop`
/// doesn't double-release.
pub struct FrameGuard<'a, B: GpuBackend> {
    index: usize,
    slot: &'a mut FrameSlot<B>,
    sema: Arc<Semaphore>,
    released: bool,
}

impl<'a, B: GpuBackend> FrameGuard<'a, B> {
    /// The slot's round-robin index.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Mutable access to the per-frame GPU state.
    pub fn slot(&mut self) -> &mut FrameSlot<B> {
        self.slot
    }

    /// Return the permit now (sync mode: after `waitUntilCompleted`). Port of
    /// `releaseFrame`. Idempotent.
    pub fn release(mut self) {
        self.do_release();
    }

    /// Give up ownership *without* releasing the permit — because a completion
    /// handler ([`SwapChain::release_hook`]) will release it instead (async
    /// mode). Prevents the `Drop` double-release.
    pub fn detach(mut self) {
        self.released = true;
    }

    fn do_release(&mut self) {
        if !self.released {
            self.released = true;
            self.sema.post();
        }
    }
}

impl<B: GpuBackend> Drop for FrameGuard<'_, B> {
    fn drop(&mut self) {
        // Safety net: if a sync-mode caller forgets to `release`, the permit is
        // still returned so the chain doesn't deadlock. Async callers must
        // `detach` (their completion hook owns the release).
        self.do_release();
    }
}

/// A minimal timer-driven pacing source (plan decision 3, day one): call a
/// draw callback every `interval` until stopped. `CVDisplayLink`
/// (`objc2-core-video`) is the later swap-in behind this same "tick a draw"
/// shape.
///
/// This is intentionally backend-agnostic and thread-based (no run loop
/// needed), so it works headless and in tests. The window host owns one of
/// these in steady state and stops it during resize (where the CALayer
/// `display` callback drives redraw synchronously instead).
pub struct TimerPacer {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TimerPacer {
    /// Start ticking `on_tick` every `interval` on a background thread. Plan
    /// decision 3 suggests an 8-16ms interval (~60-120Hz).
    pub fn start<F>(interval: std::time::Duration, on_tick: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Acquire) {
                on_tick();
                std::thread::sleep(interval);
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stop ticking and join the thread. Idempotent (also runs on `Drop`).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TimerPacer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    use crate::gpu::GpuBackend;

    use super::*;

    #[test]
    fn semaphore_blocks_until_posted() {
        let sema = Arc::new(Semaphore::new(1));
        sema.wait();
        assert_eq!(sema.available(), 0);

        // A second waiter blocks until we post from another thread.
        let s2 = Arc::clone(&sema);
        let done = Arc::new(AtomicBool::new(false));
        let done2 = Arc::clone(&done);
        let t = std::thread::spawn(move || {
            s2.wait();
            done2.store(true, Ordering::Release);
        });
        // Give the thread a chance to block, then release.
        std::thread::sleep(Duration::from_millis(20));
        assert!(!done.load(Ordering::Acquire), "waiter should still block");
        sema.post();
        t.join().unwrap();
        assert!(done.load(Ordering::Acquire));
    }

    /// Sync mode serializes frames: exactly one permit, so a second
    /// `next_frame` blocks until the first frame's guard is released. Backed by
    /// a real Metal swap chain (skips without a GPU).
    #[cfg(target_os = "macos")]
    #[test]
    fn sync_mode_serializes_frames() {
        let Some(metal) = crate::metal::test_metal() else {
            return;
        };
        let mut chain = SwapChain::new(&metal, SwapChainMode::Sync).expect("swap chain");
        assert_eq!(chain.mode(), SwapChainMode::Sync);
        let slot_count = chain.slot_count();
        // All slots allocated regardless of mode.
        assert_eq!(slot_count, crate::metal::Metal::SWAP_CHAIN_COUNT);

        // Snapshot the semaphore handle before borrowing the chain via a guard
        // (the guard holds `&mut chain`).
        let sema = Arc::clone(&chain.sema);

        // Acquire the single permit.
        let guard = chain.next_frame().expect("first frame");
        assert_eq!(guard.index(), 1 % slot_count);
        assert_eq!(sema.available(), 0, "sync mode has one live permit");

        // A second acquire must block until we release; prove it by racing a
        // background release against a foreground acquire.
        let released = Arc::new(AtomicBool::new(false));
        let released2 = Arc::clone(&released);
        let sema_waiter = Arc::clone(&sema);
        let waiter = std::thread::spawn(move || {
            // This blocks until the permit is posted.
            sema_waiter.wait();
            released2.load(Ordering::Acquire)
        });
        std::thread::sleep(Duration::from_millis(20));
        released.store(true, Ordering::Release);
        guard.release(); // posts the permit
        let saw_release_first = waiter.join().unwrap();
        assert!(
            saw_release_first,
            "second acquire only proceeded after the release"
        );
        // Return the permit the waiter consumed, so deinit's drain completes.
        sema.post();

        chain.deinit();
    }

    /// Async mode exposes `SWAP_CHAIN_COUNT` permits and its release hook posts
    /// the semaphore (the completion-handler path).
    #[cfg(target_os = "macos")]
    #[test]
    fn async_mode_has_full_permits_and_release_hook_posts() {
        let Some(metal) = crate::metal::test_metal() else {
            return;
        };
        let mut chain = SwapChain::new(&metal, SwapChainMode::Async).expect("swap chain");
        let n = crate::metal::Metal::SWAP_CHAIN_COUNT;
        let sema = Arc::clone(&chain.sema);
        assert_eq!(sema.available(), n, "async mode = full permits");

        // Acquire one, detach it (as async callers do), then post via the hook.
        let hook = chain.release_hook();
        let guard = chain.next_frame().expect("frame");
        assert_eq!(sema.available(), n - 1);
        guard.detach(); // completion handler owns the release
        hook(); // simulate the GPU-completion callback
        assert_eq!(sema.available(), n, "hook returned the permit");

        chain.deinit();
    }

    #[test]
    fn timer_pacer_ticks_then_stops() {
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::clone(&count);
        let mut pacer = TimerPacer::start(Duration::from_millis(2), move || {
            c2.fetch_add(1, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_millis(30));
        pacer.stop();
        let ticks = count.load(Ordering::Relaxed);
        assert!(ticks > 0, "pacer should have ticked at least once");
        // After stop, no further ticks.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            ticks,
            count.load(Ordering::Relaxed),
            "stopped pacer is quiet"
        );
    }
}
