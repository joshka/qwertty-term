//! The messages that can be sent to an IO thread. Port of
//! `src/termio/message.zig` + the `MessageData` union from
//! `src/datastruct/message_data.zig` (Ghostty `2da015cd6`).
//!
//! Upstream pins the size: "This is not a tiny structure (~40 bytes at the
//! time of writing this comment), but the messages [the] IO thread sends are
//! also very few. At the current size we can queue 26,000 messages before
//! consuming a MB of RAM." The 38-byte small-write capacity is chosen
//! *backwards* from the largest other union member, so inline writes use
//! every byte the union already pays for. The `message_size_pinned` test
//! keeps both properties honest in Rust.

use crate::size::Size;
use ghostty_vt::terminal::ScrollViewport;

/// Inline capacity of a small write request. Upstream: "Magic number comes
/// from the largest other union value. It can be upped if we add a larger
/// union member in the future."
pub const WRITE_SMALL_MAX: usize = 38;

/// A write request. Port of `Message.WriteReq = MessageData(u8, 38)`.
pub type WriteReq = MessageData<WRITE_SMALL_MAX>;

/// Inline small-data payload. Port of `MessageData.Small`: a fixed array
/// plus a length (upstream `IntFittingRange(0, N)`; `u8` here, which holds
/// any `N ≤ 255` — enforced by [`MessageData::init`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Small<const N: usize> {
    data: [u8; N],
    len: u8,
}

impl<const N: usize> Small<N> {
    /// Copy `data` inline. Returns `None` if it doesn't fit.
    pub fn new(data: &[u8]) -> Option<Self> {
        const {
            assert!(N <= u8::MAX as usize, "Small len is a u8");
        }
        if data.len() > N {
            return None;
        }
        let mut buf = [0u8; N];
        buf[..data.len()].copy_from_slice(data);
        Some(Small {
            data: buf,
            len: data.len() as u8,
        })
    }

    /// The valid prefix of the inline buffer.
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..usize::from(self.len)]
    }
}

/// Data that fits inline, is a stable pointer, or is owned. Port of
/// `datastruct.MessageData` — a three-way ownership union for thread
/// messaging.
///
/// Deviations (see `docs/analysis/termio-foundations.md`):
/// * `Stable` is `&'static [u8]` — upstream's `[]const u8` is a borrowed
///   slice with an unchecked lifetime, used in practice for static/const
///   data. If a non-static stable use appears in chunk D, add an
///   `Arc<[u8]>`-style variant; do not weaken this to a raw pointer.
/// * `Alloc` is a `Vec<u8>` — Rust owns allocation implicitly; upstream
///   carries the allocator in the message.
/// * Element type is fixed to `u8` (the only instantiation termio uses);
///   upstream is also generic over the element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageData<const N: usize> {
    /// A small write where the data fits into the union size.
    Small(Small<N>),
    /// A stable pointer passed through directly (e.g. const data).
    Stable(&'static [u8]),
    /// Owned heap data. "This should be rarely used."
    Alloc(Vec<u8>),
}

impl<const N: usize> MessageData<N> {
    /// Port of `MessageData.init`: fit inline if possible, otherwise
    /// allocate. This can't and will never produce `Stable` — stable
    /// pointers are opt-in at the call site.
    pub fn init(data: &[u8]) -> Self {
        match Small::new(data) {
            Some(small) => MessageData::Small(small),
            None => MessageData::Alloc(data.to_vec()),
        }
    }

    /// Port of `MessageData.slice`: the payload regardless of variant.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            MessageData::Small(s) => s.as_slice(),
            MessageData::Stable(s) => s,
            MessageData::Alloc(v) => v,
        }
    }
}

/// Output formats for terminal size reports written to the pty. Port of
/// `terminal/size_report.zig` `Style` (the termio `Message.SizeReport`
/// alias). Distinct from `csi.zig`'s `SizeReportStyle` (ported in
/// `ghostty-vt`), which trades `mode_2048` for `csi_21_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeReport {
    /// In-band size report (mode 2048).
    Mode2048,
    /// XTWINOPS `CSI 14 t`: text area size in pixels.
    Csi14T,
    /// XTWINOPS `CSI 16 t`: cell size in pixels.
    Csi16T,
    /// XTWINOPS `CSI 18 t`: text area size in characters.
    Csi18T,
}

/// Placeholder for `Termio.DerivedConfig`, which is ported with Exec/Termio
/// in chunks D/E. Exists so [`Message::ChangeConfig`] has its 1:1 variant
/// (upstream carries `{ alloc, *DerivedConfig }`; boxing owns the same
/// indirection).
#[derive(Debug, Clone, Default)]
pub struct DerivedConfig {}

/// The messages that can be sent to an IO thread. Port of `termio.Message`,
/// all 16 variants in upstream order.
#[derive(Debug, Clone)]
pub enum Message {
    /// Request a color scheme report is sent to the pty.
    ColorSchemeReport {
        /// Force write the current color scheme.
        force: bool,
    },

    /// Purposely crash the renderer; used for testing and debugging (the
    /// "crash" binding action).
    Crash,

    /// The derived configuration to update the implementation with.
    ChangeConfig(Box<DerivedConfig>),

    /// Activate or deactivate the inspector.
    Inspector(bool),

    /// Resize the window.
    Resize(Size),

    /// Request a size report is sent to the pty (in-band mode 2048 /
    /// XTWINOPS).
    SizeReport(SizeReport),

    /// Clear the screen.
    ClearScreen {
        /// Include clearing the history.
        history: bool,
    },

    /// Scroll the viewport.
    ScrollViewport(ScrollViewport),

    /// Selection scrolling. `true` starts the termio-thread timer that pings
    /// `selection_scroll_tick` back to the surface (the surface thread has no
    /// event loop of its own).
    SelectionScroll(bool),

    /// Jump forward/backward n prompts.
    JumpToPrompt(isize),

    /// Synchronized output mode started: arm the timer that force-disables
    /// the mode so a bad actor can't hang the terminal.
    StartSynchronizedOutput,

    /// Enable or disable linefeed mode (mode 20).
    LinefeedMode(bool),

    /// The surface gained or lost focus.
    Focused(bool),

    /// Write where the data fits in the union.
    WriteSmall(Small<WRITE_SMALL_MAX>),

    /// Write where the data pointer is stable.
    WriteStable(&'static [u8]),

    /// Write where the data is allocated and must be freed.
    WriteAlloc(Vec<u8>),
}

impl Message {
    /// Return a write request for the given data: `WriteSmall` if it fits,
    /// `WriteAlloc` otherwise. Port of `Message.writeReq`. "This should NOT
    /// be used for stable pointers which can be manually set to
    /// [`Message::WriteStable`]."
    pub fn write_req(data: &[u8]) -> Message {
        match WriteReq::init(data) {
            MessageData::Small(small) => Message::WriteSmall(small),
            MessageData::Alloc(vec) => Message::WriteAlloc(vec),
            MessageData::Stable(_) => unreachable!("init never produces Stable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of the upstream size pin: "Ensure we don't grow our IO message
    /// size without explicitly wanting to." The Zig union is exactly 40
    /// bytes (38 data + 1 len + 1 tag); the Rust enum lands on the same 40
    /// (39-byte largest payload + tag, rounded to the `Vec` variant's 8-byte
    /// alignment).
    #[test]
    fn message_size_pinned() {
        assert_eq!(std::mem::size_of::<Message>(), 40);
    }

    /// Port of `message_data.zig` "MessageData init small".
    #[test]
    fn message_data_init_small() {
        let data = MessageData::<10>::init(b"hello!");
        assert!(matches!(data, MessageData::Small(_)));
        assert_eq!(data.as_slice(), b"hello!");
    }

    /// Port of `message_data.zig` "MessageData init alloc".
    #[test]
    fn message_data_init_alloc() {
        let input: Vec<u8> = b"hello! ".repeat(100);
        let data = MessageData::<10>::init(&input);
        assert!(matches!(data, MessageData::Alloc(_)));
        assert_eq!(data.as_slice(), &input[..]);
    }

    #[test]
    fn write_req_picks_small_or_alloc() {
        assert!(matches!(
            Message::write_req(&[b'x'; WRITE_SMALL_MAX]),
            Message::WriteSmall(_)
        ));
        assert!(matches!(
            Message::write_req(&[b'x'; WRITE_SMALL_MAX + 1]),
            Message::WriteAlloc(_)
        ));
    }

    #[test]
    fn small_rejects_oversize() {
        assert!(Small::<4>::new(b"12345").is_none());
        assert_eq!(Small::<4>::new(b"123").unwrap().as_slice(), b"123");
    }
}
