//! Terminal search: literal case-insensitive-ASCII substring search over a `PageList`.
//!
//! Port of `src/terminal/search/*.zig` (ghostty commit `2da015cd6`). See
//! `docs/analysis/search.md` for the maintainer-grade survey.
//!
//! **Key fact:** upstream search is *not* regex — it is `std.ascii.indexOfIgnoreCase`
//! over encoded page text. The Rust port therefore needs **no regex crate and no feature
//! flag**; the matcher is a small ASCII-case-insensitive windowed scan.
//!
//! # Modules
//!
//! - [`sliding_window`] — the incremental matcher: a circular byte buffer of encoded page
//!   text plus per-node metadata, pruned as the search advances. Returns
//!   [`crate::highlight::Flattened`] match ranges.
//! - [`active`] — [`ActiveSearch`], the mutable active-area entry point (forward window).
//! - [`viewport`] — [`ViewportSearch`], the viewport entry point with change detection.
//! - [`pagelist`] — [`PageListSearch`], the whole-list reverse (history) entry point.
//!
//! The async `Thread.zig` wrapper and the `ScreenSearch` result cache (`screen.zig`) are
//! deferred (Phase 2 / needs `Screen`); see `docs/analysis/search.md`. Everything here is
//! synchronous and thread-ready: a future thread calls `update`/`feed`/`next` under a lock.
//!
//! **Phase-2 porting note (upstream `627518447`):** when `ScreenSearch` is ported, mirror
//! its *post-fix* shape — a `reset_if_dimensions_changed` helper invalidated **before**
//! `feed`, `reload_active`, AND `select` inspect cached state. Upstream originally reset
//! dimensions only in `feed`, so selecting/reloading a cached result immediately after a
//! resize dereferenced page nodes freed by reflow and crashed. The bug cannot occur here
//! today (the whole `ScreenSearch` result cache is unported); this note keeps the fix from
//! being silently reintroduced by porting the pre-`627518447` code.
//!
//! # Unsafe boundary
//!
//! Like the pagelist and highlight modules, the searchers hold raw `*mut Node`/`*mut Pin`
//! handles vended by the same [`PageList`](crate::pagelist::PageList). The contracts are
//! documented per-method; `clippy::not_unsafe_ptr_arg_deref` is allowed module-wide for the
//! same reason as `pagelist/mod.rs`.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod active;
pub mod pagelist;
pub mod sliding_window;
pub mod viewport;

pub use active::ActiveSearch;
pub use pagelist::PageListSearch;
pub use sliding_window::{Direction, SlidingWindow};
pub use viewport::ViewportSearch;
