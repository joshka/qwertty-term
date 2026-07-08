//! Highlights: contiguous runs of cells to call out (port of `src/terminal/highlight.zig`,
//! commit `2da015cd6`).
//!
//! A highlight is a generic range of cells — text selection is the headline use, but search
//! results and semantic prompt/input/output zones are the same concept. See
//! `docs/analysis/highlight.md` for the maintainer-grade survey this ports.
//!
//! Three storage flavors differ only in how the endpoints are stored and how robust they are
//! to terminal mutation:
//!
//! - [`Untracked`] — two [`Pin`]s, valid only for the current terminal state (cheap, transient).
//! - [`Tracked`] — two tracked pins that [`PageList`] keeps valid across mutations.
//! - [`Flattened`] — a list of page chunks, traversable without reading terminal state or
//!   dereferencing possibly-pruned pins.
//!
//! Invariant across all three: `start` MUST be before-or-equal-to `end` (top-left before
//! bottom-right in screen order). Rectangle shape is the *consumer's* interpretation of the
//! two endpoints; this module never encodes shape.
//!
//! # Unsafe boundary
//!
//! Like the pagelist module, some methods take/hold raw `*mut Node`/`*mut Pin` handles that
//! were vended by the same [`PageList`]; the contracts are documented per-method, so
//! `clippy::not_unsafe_ptr_arg_deref` is allowed module-wide (see `pagelist/mod.rs`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::page::size::CellCountInt;
use crate::pagelist::{Direction, PageList, Pin};

// `Node` is pagelist-private (`pub(crate)`); reachable within the crate for the flattened form.
use crate::pagelist::Node;

/// An untracked highlight stores its area as a top-left/bottom-right screen [`Pin`] pair.
///
/// Since it is untracked, the pins are valid only for the current terminal state and may not
/// be safe to use after any terminal modification. For rectangle highlights the downstream
/// consumer interprets the pins in whatever shape it wants.
///
/// `start` MUST be before-or-equal-to `end`. Port of `highlight.Untracked`.
#[derive(Debug, Clone, Copy)]
pub struct Untracked {
    pub start: Pin,
    pub end: Pin,
}

impl Untracked {
    /// Promote to a [`Tracked`] highlight by tracking both pins. Port of `Untracked.track`.
    ///
    /// The Zig version threads `Allocator.Error`; the Rust pin-tracking model is
    /// infallible-alloc (matching the PageList port), so this cannot fail and returns
    /// `Tracked` directly rather than `Allocator.Error!Tracked`.
    pub fn track(&self, pages: &mut PageList) -> Tracked {
        Tracked::init(pages, self.start, self.end)
    }

    /// Endpoint-wise pin equality. Port of `Untracked.eql`.
    pub fn eql(self, other: Untracked) -> bool {
        self.start.eql(other.start) && self.end.eql(other.end)
    }
}

/// A tracked highlight stores its area as tracked pins within a [`PageList`].
///
/// Tracked pins stay valid as the terminal state changes, so tracked highlights have more
/// operations available — at the cost of tracking overhead. If you are sure the terminal state
/// won't change, use [`Tracked::init_assume`] over already-tracked pins to skip that overhead.
///
/// Port of `highlight.Tracked`. In Zig these are `*Pin`; here they are the raw `*mut Pin`
/// handles vended by [`PageList::track_pin`].
#[derive(Debug, Clone, Copy)]
pub struct Tracked {
    pub start: *mut Pin,
    pub end: *mut Pin,
}

impl Tracked {
    /// Track both endpoints. Port of `Tracked.init`.
    ///
    /// Zig's `errdefer`-untrack-on-failure is moot here: `track_pin` is infallible in the
    /// Rust model, so there is no partial-failure path to unwind.
    pub fn init(pages: &mut PageList, start: Pin, end: Pin) -> Tracked {
        let start_tracked = pages.track_pin(start);
        let end_tracked = pages.track_pin(end);
        Tracked {
            start: start_tracked,
            end: end_tracked,
        }
    }

    /// Wrap already-tracked pins without tracking them. Port of `Tracked.initAssume`.
    ///
    /// Do not call [`Tracked::deinit`] on a highlight created this way — the pins are owned
    /// elsewhere.
    ///
    /// # Safety
    /// `start`/`end` must be live tracked pins for the intended [`PageList`], or the caller
    /// must guarantee the terminal state won't change while this highlight is used.
    pub unsafe fn init_assume(start: *mut Pin, end: *mut Pin) -> Tracked {
        Tracked { start, end }
    }

    /// Untrack both endpoints. Port of `Tracked.deinit`.
    ///
    /// # Safety
    /// The pins must have been tracked by `pages` (i.e. produced by [`Tracked::init`], not
    /// [`Tracked::init_assume`]).
    pub unsafe fn deinit(self, pages: &mut PageList) {
        pages.untrack_pin(self.start);
        pages.untrack_pin(self.end);
    }
}

/// A flattened highlight stores its area as a list of page chunks, so the whole area can be
/// traversed without reading terminal state or dereferencing page nodes (which may have been
/// pruned). Port of `highlight.Flattened`.
///
/// The chunk list handles the y-bounds: `chunks[0].start` is the first highlighted row and
/// `chunks[len - 1].end` is the last highlighted row (exclusive). `bot_x` may be numerically
/// less than `top_x` for a typical left-to-right highlight (the selection can start right of
/// the end on a higher row).
///
/// Zig stores the chunks as a `MultiArrayList` (struct-of-arrays micro-opt); the Rust port uses
/// a plain `Vec<Chunk>`.
#[derive(Debug, Clone, Default)]
pub struct Flattened {
    pub chunks: Vec<Chunk>,
    pub top_x: CellCountInt,
    pub bot_x: CellCountInt,
}

/// A flattened chunk: like a `PageList` chunk but with the page serial flattened in, making the
/// flattened highlight robust for comparisons/validity checks against the [`PageList`].
/// Port of `Flattened.Chunk`.
#[derive(Debug, Clone, Copy)]
pub struct Chunk {
    /// The page node. `pub(crate)` to match the crate-private [`Node`] type (mirrors
    /// `Pin.node`); the node identity is an internal handle, not part of the public API.
    pub(crate) node: *mut Node,
    pub serial: u64,
    pub start: CellCountInt,
    pub end: CellCountInt,
}

impl Flattened {
    /// An empty flattened highlight. Port of `Flattened.empty`.
    pub fn empty() -> Flattened {
        Flattened {
            chunks: Vec::new(),
            top_x: 0,
            bot_x: 0,
        }
    }

    /// Build a flattened highlight covering `[start, end]`. Port of `Flattened.init`.
    ///
    /// NOTE: the Zig source (`highlight.zig:155-159`) writes `.end_x = end.x` into a struct
    /// whose field is `bot_x` — dead/untested upstream code (no `Flattened` tests, no in-tree
    /// consumer of `Flattened.init`). We use the field the struct actually declares: `bot_x`.
    ///
    /// # Safety
    /// `start`/`end` must be live pins in `_pages` with `start` before-or-equal-to `end`; the
    /// node chain they span must be live.
    pub unsafe fn init(_pages: &PageList, start: Pin, end: Pin) -> Flattened {
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut it = unsafe { start.page_iterator(Direction::RightDown, Some(end)) };
        while let Some(chunk) = unsafe { it.next() } {
            let node = chunk.node;
            let serial = unsafe { (*node).serial };
            chunks.push(Chunk {
                node,
                serial,
                start: chunk.start,
                end: chunk.end,
            });
        }
        Flattened {
            chunks,
            top_x: start.x(),
            bot_x: end.x(),
        }
    }

    /// The top-left pin. Port of `Flattened.startPin`.
    pub fn start_pin(&self) -> Pin {
        let first = self.chunks[0];
        Pin::with(first.node, first.start, self.top_x)
    }

    /// The bottom-right pin (chunk end is exclusive, so `end - 1`). Port of `Flattened.endPin`.
    pub fn end_pin(&self) -> Pin {
        let last = self.chunks[self.chunks.len() - 1];
        Pin::with(last.node, last.end - 1, self.bot_x)
    }

    /// Collapse to an [`Untracked`] highlight. Port of `Flattened.untracked`.
    pub fn untracked(&self) -> Untracked {
        Untracked {
            start: self.start_pin(),
            end: self.end_pin(),
        }
    }
}
