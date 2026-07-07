//! Stream dispatch layer: bytes in, terminal state out.
//!
//! Port of `src/terminal/stream.zig` + `src/terminal/stream_terminal.zig`
//! (commit `2da015cd6`). This composes the [`Utf8Decoder`] and [`Parser`]
//! and dispatches every parser action onto a [`Handler`], mirroring
//! ghostty's `Stream(Handler)` + `stream_terminal.Handler`.
//!
//! # Design: comptime handler interface -> Rust trait
//!
//! Ghostty's `Stream` is generic over a `comptime Handler` type and calls a
//! single `handler.vt(comptime action, value)` method, where `Action.Tag`
//! selects (at comptime) both the operation and the value type. Zig
//! monomorphizes this into a giant switch. Rust has no comptime-tag-indexed
//! value types, so we split that single `vt` into one trait method per
//! operation family, keeping the *same routing* (`csiDispatch`/`escDispatch`/
//! `oscDispatch`/`execute`) in the stream itself and letting the handler
//! implement the terminal-modifying effects. `stream_terminal.Handler`
//! becomes [`TerminalHandler`], the concrete impl over a [`Terminal`].
//!
//! Replies (DSR/DA/CPR/DECRQSS) are collected into a caller-drainable output
//! queue on `TerminalHandler` (its `output` field), matching how the spike
//! accumulated replies and how upstream surfaces them via a `writePty`
//! effect callback. The differential harness compares screen text + cursor,
//! so replies only need to be *collected*, not routed anywhere.
//!
//! # Fast-path scalar/SIMD split (note only)
//!
//! Upstream's `nextSlice` has a SIMD `utf8DecodeUntilControlSeq` bulk path
//! and hand-inlined `csi_entry`/`csi_param` fast paths that dispatch without
//! going through `Parser.next`. Those are behavior-equivalent throughput
//! optimizations (Phase-7 perf item per the rewrite prompt). This port
//! implements only the scalar path: ground-state bytes go through the
//! [`Utf8Decoder`]; non-ground bytes go through [`Parser::next`]; the CSI
//! fast paths are omitted because `Parser::next` already produces identical
//! actions.

use crate::apc;
use crate::csi::{EraseDisplay, EraseLine, TabClear};
use crate::dcs;
use crate::kitty;
use crate::modes::{self, Mode};
use crate::osc;
use crate::parser::{Action, MAX_INTERMEDIATE, MAX_PARAMS, Parser, State};
use crate::sgr;
use crate::terminal::{SwitchScreenMode, Terminal};
use crate::utf8_decoder::Utf8Decoder;

/// A device-status-report / device-attributes / DECRQSS reply, collected in
/// order so a caller (or the pty layer) can drain them.
pub type Reply = Vec<u8>;

/// Owned CSI dispatch payload (a copy of [`Csi`] so the parser borrow can be
/// released before handler dispatch).
///
/// The parser's intermediates/params are small, fixed-capacity arrays
/// (`MAX_INTERMEDIATE` / `MAX_PARAMS`), so this copies them into inline arrays
/// rather than heap `Vec`s. That keeps the dispatch hot path allocation-free —
/// which matters for control-sequence-dense streams (SGR, cursor-heavy), where
/// a per-sequence `Vec` allocation dominated the dispatch cost (see
/// `docs/analysis/perf.md`). Behavior is identical to the previous `Vec` copy.
struct CsiOwned {
    intermediates: [u8; MAX_INTERMEDIATE],
    intermediates_len: u8,
    params: [u16; MAX_PARAMS],
    params_len: u8,
    params_sep: crate::parser::SepList,
    final_byte: u8,
}

impl CsiOwned {
    #[inline]
    fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediates_len as usize]
    }
    #[inline]
    fn params(&self) -> &[u16] {
        &self.params[..self.params_len as usize]
    }
}

/// Owned ESC dispatch payload. See [`CsiOwned`] for the inline-array rationale.
struct EscOwned {
    intermediates: [u8; MAX_INTERMEDIATE],
    intermediates_len: u8,
    final_byte: u8,
}

impl EscOwned {
    #[inline]
    fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediates_len as usize]
    }
}

/// Owned DCS hook payload. See [`CsiOwned`] for the inline-array rationale.
struct DcsOwned {
    intermediates: [u8; MAX_INTERMEDIATE],
    intermediates_len: u8,
    params: [u16; MAX_PARAMS],
    params_len: u8,
    final_byte: u8,
}

impl DcsOwned {
    #[inline]
    fn intermediates(&self) -> &[u8] {
        &self.intermediates[..self.intermediates_len as usize]
    }
    #[inline]
    fn params(&self) -> &[u16] {
        &self.params[..self.params_len as usize]
    }
}

/// Copy a bounded slice into a fixed-capacity array, returning the array and
/// the length. The source is guaranteed by the parser to fit within `N`.
#[inline]
fn copy_bounded<T: Copy + Default, const N: usize>(src: &[T]) -> ([T; N], u8) {
    let mut arr = [T::default(); N];
    let n = src.len();
    debug_assert!(n <= N);
    arr[..n].copy_from_slice(src);
    (arr, n as u8)
}

/// An owned copy of one [`Action`], detached from the parser borrow.
enum Emitted {
    Print(char),
    Execute(u8),
    Csi(CsiOwned),
    Esc(EscOwned),
    OscStart,
    OscPut(u8),
    OscEnd(u8),
    DcsHook(DcsOwned),
    DcsPut(u8),
    DcsUnhook,
    ApcStart,
    ApcPut(u8),
    ApcEnd,
}

impl Emitted {
    fn from_action(a: Action<'_>) -> Emitted {
        match a {
            Action::Print(c) => Emitted::Print(c),
            Action::Execute(c) => Emitted::Execute(c),
            Action::CsiDispatch(csi) => {
                let (intermediates, intermediates_len) = copy_bounded(csi.intermediates);
                let (params, params_len) = copy_bounded(csi.params);
                Emitted::Csi(CsiOwned {
                    intermediates,
                    intermediates_len,
                    params,
                    params_len,
                    params_sep: csi.params_sep,
                    final_byte: csi.final_byte,
                })
            }
            Action::EscDispatch(esc) => {
                let (intermediates, intermediates_len) = copy_bounded(esc.intermediates);
                Emitted::Esc(EscOwned {
                    intermediates,
                    intermediates_len,
                    final_byte: esc.final_byte,
                })
            }
            Action::OscStart => Emitted::OscStart,
            Action::OscPut(b) => Emitted::OscPut(b),
            Action::OscEnd(b) => Emitted::OscEnd(b),
            Action::DcsHook(d) => {
                let (intermediates, intermediates_len) = copy_bounded(d.intermediates);
                let (params, params_len) = copy_bounded(d.params);
                Emitted::DcsHook(DcsOwned {
                    intermediates,
                    intermediates_len,
                    params,
                    params_len,
                    final_byte: d.final_byte,
                })
            }
            Action::DcsPut(b) => Emitted::DcsPut(b),
            Action::DcsUnhook => Emitted::DcsUnhook,
            Action::ApcStart => Emitted::ApcStart,
            Action::ApcPut(b) => Emitted::ApcPut(b),
            Action::ApcEnd => Emitted::ApcEnd,
        }
    }
}

/// The handler interface the [`Stream`] dispatches parser actions onto.
///
/// This is the Rust analogue of ghostty's `comptime Handler` with its
/// `vt(action, value)` method — split into one method per operation so Rust
/// can type the values. Every method has a default no-op body so a partial
/// handler (e.g. a test spy) only overrides what it needs.
#[allow(unused_variables)]
pub trait Handler {
    // ---- printing -------------------------------------------------------
    fn print(&mut self, cp: u32) {}

    /// Print a run of already-decoded printable codepoints at once. The
    /// default loops [`Handler::print`]; the concrete [`TerminalHandler`]
    /// overrides it with the batched [`Terminal::print_slice`] fast path. This
    /// is the sink for the stream's decode-until-control-seq bulk-print path.
    fn print_slice(&mut self, cps: &[u32]) {
        for &cp in cps {
            self.print(cp);
        }
    }

    // ---- C0 / simple motion --------------------------------------------
    fn backspace(&mut self) {}
    fn carriage_return(&mut self) {}
    fn linefeed(&mut self) {}
    fn index(&mut self) {}
    fn next_line(&mut self) {}
    fn reverse_index(&mut self) {}
    fn bell(&mut self) {}
    fn enquiry(&mut self) {}

    // ---- cursor moves ---------------------------------------------------
    fn cursor_up(&mut self, count: u16) {}
    fn cursor_down(&mut self, count: u16) {}
    fn cursor_left(&mut self, count: u16) {}
    fn cursor_right(&mut self, count: u16) {}
    /// 1-indexed row/col (already resolved from params).
    fn cursor_pos(&mut self, row: u16, col: u16) {}
    fn cursor_col(&mut self, col: u16) {}
    fn cursor_row(&mut self, row: u16) {}
    fn cursor_col_relative(&mut self, count: u16) {}
    fn cursor_row_relative(&mut self, count: u16) {}
    fn save_cursor(&mut self) {}
    fn restore_cursor(&mut self) {}

    // ---- tabs -----------------------------------------------------------
    fn horizontal_tab(&mut self, count: u16) {}
    fn horizontal_tab_back(&mut self, count: u16) {}
    fn tab_clear(&mut self, cmd: TabClear) {}
    fn tab_set(&mut self) {}
    fn tab_reset(&mut self) {}

    // ---- erase / scroll / edit -----------------------------------------
    fn erase_display(&mut self, mode: EraseDisplay, protected: bool) {}
    fn erase_line(&mut self, mode: EraseLine, protected: bool) {}
    fn delete_chars(&mut self, count: u16) {}
    fn erase_chars(&mut self, count: u16) {}
    fn insert_lines(&mut self, count: u16) {}
    fn insert_blanks(&mut self, count: u16) {}
    fn delete_lines(&mut self, count: u16) {}
    fn scroll_up(&mut self, count: u16) {}
    fn scroll_down(&mut self, count: u16) {}

    // ---- modes / margins ------------------------------------------------
    fn set_mode(&mut self, mode: Mode, enabled: bool) {}
    fn save_mode(&mut self, mode: Mode) {}
    fn restore_mode(&mut self, mode: Mode) {}
    fn top_and_bottom_margin(&mut self, top: u16, bottom: u16) {}
    fn left_and_right_margin(&mut self, left: u16, right: u16) {}
    fn left_and_right_margin_ambiguous(&mut self) {}

    // ---- charset --------------------------------------------------------
    fn configure_charset(&mut self, intermediates: &[u8], set: crate::charsets::Charset) {}
    fn invoke_charset(
        &mut self,
        active: crate::charsets::ActiveSlot,
        slot: crate::charsets::Slots,
        single: bool,
    ) {
    }

    // ---- SGR / attributes ----------------------------------------------
    fn set_attribute(&mut self, attr: sgr::Attribute) {}

    // ---- protected mode / status ---------------------------------------
    fn protected_mode(&mut self, mode: crate::terminal::ProtectedMode) {}
    fn active_status_display(&mut self, display: crate::terminal::StatusDisplay) {}

    // ---- reset / alignment ---------------------------------------------
    fn decaln(&mut self) {}
    fn full_reset(&mut self) {}

    // ---- REP (repeat previous char, CSI b) ------------------------------
    /// Repeat the previously printed character `count` times. Port of
    /// `Terminal::printRepeat` dispatch (`.print_repeat`).
    fn print_repeat(&mut self, count: u16) {}

    // ---- kitty keyboard protocol (CSI … u) ------------------------------
    /// `CSI ? u` — report the current kitty keyboard flags.
    fn kitty_keyboard_query(&mut self) {}
    /// `CSI > flags u` — push flags onto the stack.
    fn kitty_keyboard_push(&mut self, flags: crate::screen::kitty_key::Flags) {}
    /// `CSI < count u` — pop `count` entries off the stack.
    fn kitty_keyboard_pop(&mut self, count: u16) {}
    /// `CSI = flags ; mode u` — set/or/not the current flags.
    fn kitty_keyboard_set(
        &mut self,
        mode: crate::screen::kitty_key::SetMode,
        flags: crate::screen::kitty_key::Flags,
    ) {
    }

    // ---- XTWINOPS (CSI t) title stack -----------------------------------
    /// `CSI 22 ; Ps [ ; index ] t` — push the current window title onto the
    /// title stack. `index` defaults to 0. Port of `.title_push`. Upstream's
    /// concrete handler treats this as a no-op (the title stack lives in the
    /// apprt/surface layer, not the terminal core); the dispatch is kept so an
    /// embedder can wire it.
    fn title_push(&mut self, index: u16) {}
    /// `CSI 23 ; Ps [ ; index ] t` — pop a window title off the title stack.
    /// Port of `.title_pop`.
    fn title_pop(&mut self, index: u16) {}

    // ---- XTSHIFTESCAPE / mouse-tracking setters (state only) ------------
    /// `CSI > Ps s` — set whether the terminal captures shift for mouse
    /// events. State only; the input layer interprets it. Port of
    /// `.mouse_shift_capture`.
    fn mouse_shift_capture(&mut self, capture: bool) {}

    // ---- cursor style ---------------------------------------------------
    fn cursor_style(&mut self, style: CursorStyle) {}

    // ---- OSC-driven -----------------------------------------------------
    fn window_title(&mut self, title: &str) {}
    fn report_pwd(&mut self, url: &str) {}
    fn semantic_prompt(&mut self, cmd: &osc::SemanticPrompt) {}
    fn color_operation(&mut self, requests: &osc::ColorList) {}
    fn kitty_color(&mut self, cmd: &osc::KittyColorProtocol) {}
    fn mouse_shape(&mut self, value: &str) {}
    /// OSC 52 clipboard get/set. `kind` is the clipboard-selection char
    /// (`'c'` standard, `'s'` selection, `'p'` primary; upstream ignores the
    /// distinction and always uses the standard clipboard, but the value is
    /// preserved for a future policy). `data` is the raw OSC body: either
    /// `"?"` (a read request) or a base64 payload (write; empty means
    /// clear). Port of `stream_terminal.Handler.clipboardContents` — upstream
    /// hands this raw, undecoded data up to the apprt surface and lets *that*
    /// layer decode base64 / perform the actual clipboard I/O; this trait
    /// method is the same seam.
    fn clipboard(&mut self, kind: u8, data: &str) {}

    // ---- reports (queue-emitting) --------------------------------------
    fn device_attributes(&mut self, req: DeviceAttributesReq) {}
    fn device_status(&mut self, req: DeviceStatusReq) {}
    fn request_mode(&mut self, mode: Mode) {}
    fn request_mode_unknown(&mut self, mode_raw: u16, ansi: bool) {}
    fn decrqss(&mut self, setting: dcs::Decrqss) {}
    fn xtversion(&mut self) {}

    // ---- APC ------------------------------------------------------------
    fn apc_start(&mut self) {}
    fn apc_put(&mut self, byte: u8) {}
    fn apc_end(&mut self) {}

    // ---- DCS ------------------------------------------------------------
    fn dcs_hook(&mut self, dcs: crate::parser::Dcs) {}
    fn dcs_put(&mut self, byte: u8) {}
    fn dcs_unhook(&mut self) {}
}

/// DECSCUSR cursor styles (`CSI Ps SP q`). Port of `ansi.CursorStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

/// Device-attributes request kind (`CSI c`). Port of `device_attributes.Req`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAttributesReq {
    Primary,
    Secondary,
    Tertiary,
}

/// Device-status request (`CSI n`). Port of `device_status.Request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatusReq {
    OperatingStatus,
    CursorPosition,
}

impl DeviceStatusReq {
    /// Port of `device_status.reqFromInt`.
    fn from_int(value: u16, question: bool) -> Option<DeviceStatusReq> {
        match (value, question) {
            (5, false) => Some(DeviceStatusReq::OperatingStatus),
            (6, _) => Some(DeviceStatusReq::CursorPosition),
            _ => None,
        }
    }
}

/// The stream: composes decoder + parser and routes parser actions to a
/// [`Handler`]. Port of `Stream(Handler)`.
pub struct Stream<H: Handler> {
    pub handler: H,
    parser: Parser,
    utf8: Utf8Decoder,
    /// The structured OSC parser (fed by the raw OscStart/OscPut/OscEnd
    /// events; see `docs/analysis/osc.md`).
    osc: osc::Parser,
    /// DCS handler (hook/put/unhook -> Command).
    dcs: dcs::Handler,
    /// Scratch codepoint buffer for the decode-until-control-seq bulk path
    /// (`feed`). Sized like upstream's 4096-codepoint `cp_buf`.
    cp_buf: Box<[u32; CP_BUF_LEN]>,
    /// Bytes consumed by the last [`Self::decode_until_control_seq`] scan.
    scan_consumed: usize,
}

/// Bulk-decode codepoint buffer length (matches upstream's `cp_buf` in
/// `nextSlice`). Boxed so the `Stream` struct stays small on the stack.
const CP_BUF_LEN: usize = 4096;

impl<H: Handler> Stream<H> {
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            parser: Parser::new(),
            utf8: Utf8Decoder::new(),
            // Allocator-permitting parser so OSC 4/52/… don't spuriously
            // invalidate (matches ghostty's `osc.Parser` with an allocator).
            osc: osc::Parser::with_allocator(),
            dcs: dcs::Handler::new(),
            cp_buf: Box::new([0u32; CP_BUF_LEN]),
            scan_consumed: 0,
        }
    }

    /// Feed a slice of bytes. Port of `nextSlice` (scalar path).
    ///
    /// The hot path is the ground-state decode-until-control-seq scan: while
    /// the parser is in ground state and the UTF-8 decoder has no pending
    /// bytes, this bulk-decodes a run of codepoints into a scratch buffer and
    /// hands each printable run to `Handler::print_slice` in one call, instead
    /// of stepping the parser byte-by-byte. Non-ground bytes and partial UTF-8
    /// fall back to the per-byte `next` path. This mirrors upstream's
    /// `nextSlice`/`nextSliceCapped` structure (minus the SIMD decode and the
    /// CSI-param bulk fast path, which are separate perf items).
    pub fn feed(&mut self, input: &[u8]) {
        let mut offset = 0;

        while offset < input.len() {
            // Drain any partial UTF-8 sequence carried across a previous call
            // or run: process byte-at-a-time until the decoder is idle.
            while self.utf8.is_partial() {
                if offset >= input.len() {
                    return;
                }
                self.next_utf8(input[offset]);
                offset += 1;
            }

            // If we're not in ground state (mid control sequence), process
            // per-byte until we return to ground.
            while self.parser.state() != State::Ground {
                if offset >= input.len() {
                    return;
                }
                // Bulk-consume CSI parameter bytes (digits/separators and the
                // final byte) without stepping the parser byte-by-byte. This is
                // the hot path for control-dense streams (SGR params, cursor
                // moves). Port of `consumeUntilGround`'s `consumeCsiParams`
                // call. On the final byte it yields the CSI dispatch directly.
                if self.parser.state() == State::CsiParam {
                    let (consumed, action) = self.parser.bulk_consume_csi_params(&input[offset..]);
                    if let Some(action) = action {
                        // Materialize the borrowed action before dispatch (the
                        // parser borrow must end first), matching next_non_utf8.
                        let emitted = Emitted::from_action(action);
                        offset += consumed;
                        if let Emitted::Csi(csi) = emitted {
                            self.csi_dispatch(&csi);
                        }
                        continue;
                    }
                    offset += consumed;
                    // Still in csi_param: the next byte isn't a parameter byte;
                    // fall through to the per-byte path below to handle it.
                    if offset >= input.len() {
                        return;
                    }
                }

                self.next_non_utf8(input[offset]);
                offset += 1;
            }

            if offset >= input.len() {
                return;
            }

            // Ground state + idle decoder: bulk-decode until a control byte.
            let cp_count = self.decode_until_control_seq(&input[offset..]);
            let consumed = self.scan_consumed;
            self.dispatch_ground_run(cp_count);
            offset += consumed;

            // The scan stops *before* a control byte (ESC) without consuming
            // it, so a scan can legitimately consume zero bytes (e.g. input
            // starts with ESC). Guarantee forward progress and drive that byte
            // through the scalar path, which performs the ground->escape
            // transition (matching `handle_codepoint`). Any subsequent control
            // sequence is finished by the non-ground loop at the top.
            if consumed == 0 {
                if offset >= input.len() {
                    return;
                }
                self.next(input[offset]);
                offset += 1;
            }
            // Loop: re-check decoder-partial / non-ground / more input.
        }
    }

    /// Bulk-decode ground-state bytes into `self.cp_buf` until the next ESC
    /// (0x1B) or until the decoder would need bytes not yet present (partial
    /// UTF-8 at the end of `input`). Records the number of *bytes* consumed in
    /// `self.scan_consumed` and returns the number of *codepoints* produced.
    ///
    /// Scalar analogue of `simd.vt.utf8DecodeUntilControlSeq`: it stops at ESC
    /// so `feed` can hand the control sequence to the parser, and it leaves any
    /// trailing incomplete UTF-8 sequence unconsumed (the decoder holds no
    /// partial state on return unless the whole tail was a partial sequence, in
    /// which case `feed`'s drain loop finishes it on the next chunk). C0
    /// controls other than ESC are emitted into the buffer as their codepoints
    /// (all <= 0xF), and split back out by `dispatch_ground_run`.
    fn decode_until_control_seq(&mut self, input: &[u8]) -> usize {
        let cap = self.cp_buf.len();
        let mut n = 0; // codepoints produced
        let mut i = 0; // bytes consumed
        while i < input.len() && n < cap {
            let byte = input[i];
            // ESC breaks the run so the parser state machine can take over.
            // (handle_codepoint would set escape state manually; we stop here
            // and let feed's non-ground loop drive the escape sequence.)
            if byte == 0x1B {
                break;
            }
            // ASCII fast path: when the decoder is idle, a byte < 0x80 is a
            // complete 1-byte codepoint equal to the byte itself (the Hoehrmann
            // DFA emits exactly `byte` for char-class 0 in the ACCEPT state).
            // Skip the table lookups for the common ASCII run. C0 controls
            // (< 0x20) still land here as codepoints <= 0x1F and are split back
            // out to `execute` by `dispatch_ground_run`, matching the general
            // path. This mirrors what the SIMD decode-until-control-seq does in
            // bulk; here it is a scalar branch.
            if byte < 0x80 && !self.utf8.is_partial() {
                self.cp_buf[n] = byte as u32;
                n += 1;
                i += 1;
                continue;
            }
            let (cp, consumed) = self.utf8.next(byte);
            if consumed {
                i += 1;
            }
            match cp {
                Some(c) => {
                    self.cp_buf[n] = c as u32;
                    n += 1;
                }
                None => {
                    // Mid-sequence. If the input ended mid-sequence, stop; the
                    // decoder keeps its partial state and feed's drain loop
                    // finishes it next call. Otherwise keep decoding.
                    if i >= input.len() {
                        break;
                    }
                }
            }
        }
        self.scan_consumed = i;
        n
    }

    /// Dispatch the first `cp_count` codepoints of `self.cp_buf` produced by
    /// [`Self::decode_until_control_seq`]: split off C0 controls (`cp <= 0xF`)
    /// as `execute`, and hand each maximal run of printable codepoints to
    /// `Handler::print_slice` in one call. Port of the inner emit loop of
    /// `nextSliceCapped`.
    fn dispatch_ground_run(&mut self, cp_count: usize) {
        let mut i = 0;
        while i < cp_count {
            let cp = self.cp_buf[i];
            if cp <= 0xF {
                self.execute(cp as u8);
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while end < cp_count && self.cp_buf[end] > 0xF {
                end += 1;
            }
            // Disjoint borrow: `cp_buf` and `handler` are separate fields, so
            // split the struct borrow to hand the run slice to the handler.
            let Self {
                cp_buf, handler, ..
            } = self;
            handler.print_slice(&cp_buf[i..end]);
            i = end;
        }
    }

    /// Feed one byte. Port of `next`.
    pub fn next(&mut self, c: u8) {
        if self.parser.state() == State::Ground {
            self.next_utf8(c);
        } else {
            self.next_non_utf8(c);
        }
    }

    /// Ground-state byte: run the UTF-8 decoder. Port of `nextUtf8`.
    fn next_utf8(&mut self, c: u8) {
        let (cp, consumed) = self.utf8.next(c);
        if let Some(cp) = cp {
            self.handle_codepoint(cp);
        }
        if !consumed {
            let (cp, consumed) = self.utf8.next(c);
            debug_assert!(consumed, "decoder must consume on retry");
            if let Some(cp) = cp {
                self.handle_codepoint(cp);
            }
        }
    }

    /// A decoded codepoint in ground state. Port of `handleCodepoint`.
    fn handle_codepoint(&mut self, c: char) {
        let cp = c as u32;
        // C0 control.
        if cp <= 0xF {
            self.execute(cp as u8);
            return;
        }
        // ESC: manually enter escape state (bypassing the table), matching
        // ghostty's fast path.
        if cp == 0x1B {
            self.parser.set_state(State::Escape);
            self.parser.clear();
            return;
        }
        self.handler.print(cp);
    }

    /// A non-ground-state byte goes through the parser. Port of the general
    /// `nextNonUtf8` path (the CSI fast paths are omitted; see module docs).
    fn next_non_utf8(&mut self, c: u8) {
        // Convert the three borrowed actions into owned `Emitted` values so
        // the parser borrow ends before we call `&mut self` handler methods.
        // The parser's slices are stable until the next `next()`, so copying
        // them here is behavior-equivalent to Zig's borrow-until-next-call
        // contract — we just make the copy explicit for the borrow checker.
        let emitted: [Option<Emitted>; 3] = {
            let actions = self.parser.next(c);
            actions.map(|a| a.map(Emitted::from_action))
        };

        for e in emitted {
            let Some(e) = e else { continue };
            match e {
                Emitted::Print(p) => self.handler.print(p as u32),
                Emitted::Execute(code) => self.execute(code),
                Emitted::Csi(csi) => self.csi_dispatch(&csi),
                Emitted::Esc(esc) => self.esc_dispatch(&esc),

                Emitted::OscStart => self.osc.reset(),
                Emitted::OscPut(b) => self.osc.next(b),
                Emitted::OscEnd(term) => {
                    if let Some(cmd) = self.osc.end(Some(term)) {
                        self.osc_dispatch(cmd);
                    }
                }

                Emitted::DcsHook(d) => {
                    if let Some(cmd) = self.dcs.hook(crate::parser::Dcs {
                        intermediates: d.intermediates(),
                        params: d.params(),
                        final_byte: d.final_byte,
                    }) {
                        self.dcs_command(cmd);
                    }
                }
                Emitted::DcsPut(b) => {
                    if let Some(cmd) = self.dcs.put(b) {
                        self.dcs_command(cmd);
                    }
                }
                Emitted::DcsUnhook => {
                    if let Some(cmd) = self.dcs.unhook() {
                        self.dcs_command(cmd);
                    }
                }

                Emitted::ApcStart => self.handler.apc_start(),
                Emitted::ApcPut(b) => self.handler.apc_put(b),
                Emitted::ApcEnd => self.handler.apc_end(),
            }
        }
    }

    fn dcs_command(&mut self, cmd: dcs::Command) {
        // Only DECRQSS produces a terminal-visible reply. XTGETTCAP / tmux
        // are seams (no terminal-modifying effect), matching upstream, which
        // ignores dcs_hook/put/unhook for terminal state.
        if let dcs::Command::Decrqss(setting) = cmd {
            self.handler.decrqss(setting);
        }
    }

    // ---- C0 execute (port of `execute`) --------------------------------
    fn execute(&mut self, c: u8) {
        // C1 (8-bit) controls are equivalent to ESC + (c - 0x40).
        if c > 0x7F {
            self.esc_dispatch(&EscOwned {
                intermediates: [0; MAX_INTERMEDIATE],
                intermediates_len: 0,
                final_byte: c - 0x40,
            });
            return;
        }

        match c {
            // NUL/SOH/STX ignored.
            0x00..=0x02 => {}
            0x05 => self.handler.enquiry(),         // ENQ
            0x07 => self.handler.bell(),            // BEL
            0x08 => self.handler.backspace(),       // BS
            0x09 => self.handler.horizontal_tab(1), // HT
            0x0A..=0x0C => self.handler.linefeed(), // LF/VT/FF
            0x0D => self.handler.carriage_return(), // CR
            0x0E => self.handler.invoke_charset(
                crate::charsets::ActiveSlot::Gl,
                crate::charsets::Slots::G1,
                false,
            ), // SO
            0x0F => self.handler.invoke_charset(
                crate::charsets::ActiveSlot::Gl,
                crate::charsets::Slots::G0,
                false,
            ), // SI
            _ => {}
        }
    }
}

// -------------------------------------------------------------------------
// CSI / ESC / OSC dispatch (port of csiDispatch / escDispatch / oscDispatch)
// -------------------------------------------------------------------------

impl<H: Handler> Stream<H> {
    /// Port of `csiDispatch`. Routes on the final byte + intermediates.
    fn csi_dispatch(&mut self, input: &CsiOwned) {
        let params: &[u16] = input.params();
        let intermediates: &[u8] = input.intermediates();

        // Helper: single count param (default 1), reject 2+.
        let one = |p: &[u16]| -> Option<u16> {
            match p.len() {
                0 => Some(1),
                1 => Some(p[0]),
                _ => None,
            }
        };

        match input.final_byte {
            // CUU
            b'A' | b'k' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_up(v);
                }
            }
            // CUD
            b'B' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_down(v);
                }
            }
            // CUF
            b'C' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_right(v);
                }
            }
            // CUB
            b'D' | b'j' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_left(v);
                }
            }
            // CNL
            b'E' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_down(v);
                    self.handler.carriage_return();
                }
            }
            // CPL
            b'F' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_up(v);
                    self.handler.carriage_return();
                }
            }
            // HPA
            b'G' | b'`' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_col(v);
                }
            }
            // CUP
            b'H' | b'f' => {
                if intermediates.is_empty() {
                    let pos = match params.len() {
                        0 => Some((1, 1)),
                        1 => Some((params[0], 1)),
                        2 => Some((params[0], params[1])),
                        _ => None,
                    };
                    if let Some((row, col)) = pos {
                        self.handler.cursor_pos(row, col);
                    }
                }
            }
            // CHT
            b'I' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.horizontal_tab(v);
                }
            }
            // ED
            b'J' => {
                let Some(protected) = protected_from_intermediates(intermediates) else {
                    return;
                };
                let mode = match params.len() {
                    0 => Some(EraseDisplay::Below),
                    1 => erase_display_from_param(params[0]),
                    _ => None,
                };
                if let Some(mode) = mode {
                    self.handler.erase_display(mode, protected);
                }
            }
            // EL
            b'K' => {
                let Some(protected) = protected_from_intermediates(intermediates) else {
                    return;
                };
                let mode = match params.len() {
                    0 => Some(EraseLine::Right),
                    1 if params[0] < 3 => Some(EraseLine::from_param(params[0] as u8)),
                    _ => None,
                };
                if let Some(mode) = mode {
                    self.handler.erase_line(mode, protected);
                }
            }
            // IL
            b'L' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.insert_lines(v);
                }
            }
            // DL
            b'M' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.delete_lines(v);
                }
            }
            // DCH
            b'P' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.delete_chars(v);
                }
            }
            // SU
            b'S' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.scroll_up(v);
                }
            }
            // SD
            b'T' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.scroll_down(v);
                }
            }
            // CTC (tab set/clear/reset)
            b'W' => match intermediates.len() {
                0 => {
                    if params.is_empty() || (params.len() == 1 && params[0] == 0) {
                        self.handler.tab_set();
                    } else if params.len() == 1 {
                        match params[0] {
                            2 => self.handler.tab_clear(TabClear::Current),
                            5 => self.handler.tab_clear(TabClear::All),
                            _ => {}
                        }
                    }
                }
                1 if intermediates[0] == b'?' && params.len() == 1 && params[0] == 5 => {
                    self.handler.tab_reset();
                }
                _ => {}
            },
            // ECH
            b'X' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.erase_chars(v);
                }
            }
            // CBT
            b'Z' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.horizontal_tab_back(v);
                }
            }
            // HPR
            b'a' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_col_relative(v);
                }
            }
            // REP (repeat previous char). Port of the `'b'` prong.
            b'b' => {
                if intermediates.is_empty() {
                    let count = match params.len() {
                        0 => Some(1),
                        1 => Some(params[0]),
                        _ => None,
                    };
                    if let Some(count) = count {
                        self.handler.print_repeat(count);
                    }
                }
            }
            // DA
            b'c' => {
                let req = match intermediates.len() {
                    0 => Some(DeviceAttributesReq::Primary),
                    1 => match intermediates[0] {
                        b'>' => Some(DeviceAttributesReq::Secondary),
                        b'=' => Some(DeviceAttributesReq::Tertiary),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(r) = req {
                    self.handler.device_attributes(r);
                }
            }
            // VPA
            b'd' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_row(v);
                }
            }
            // VPR
            b'e' => {
                if intermediates.is_empty()
                    && let Some(v) = one(params)
                {
                    self.handler.cursor_row_relative(v);
                }
            }
            // TBC
            b'g' => {
                if intermediates.is_empty() && params.len() == 1 {
                    match TabClear::from_param(params[0] as u8) {
                        TabClear::Current => self.handler.tab_clear(TabClear::Current),
                        TabClear::All => self.handler.tab_clear(TabClear::All),
                        TabClear::Other(_) => {}
                    }
                }
            }
            // SM
            b'h' => {
                if let Some(ansi) = mode_ansi(intermediates) {
                    for &m in params {
                        if let Some(mode) = modes::mode_from_int(m, ansi) {
                            self.handler.set_mode(mode, true);
                        }
                    }
                }
            }
            // RM
            b'l' => {
                if let Some(ansi) = mode_ansi(intermediates) {
                    for &m in params {
                        if let Some(mode) = modes::mode_from_int(m, ansi) {
                            self.handler.set_mode(mode, false);
                        }
                    }
                }
            }
            // SGR
            b'm' => {
                if intermediates.is_empty() {
                    let mut p = sgr::Parser {
                        params,
                        params_sep: input.params_sep,
                        idx: 0,
                    };
                    while let Some(attr) = p.next() {
                        self.handler.set_attribute(attr);
                    }
                }
                // Intermediate forms (XTMODKEYS `CSI > … m`) not modeled.
            }
            // DSR
            b'n' => {
                if intermediates.is_empty()
                    || (intermediates.len() == 1 && intermediates[0] == b'?')
                {
                    if params.len() != 1 {
                        return;
                    }
                    let question = intermediates.len() == 1;
                    if let Some(req) = DeviceStatusReq::from_int(params[0], question) {
                        self.handler.device_status(req);
                    }
                }
            }
            // DECRQM. Upstream (`stream.zig`, both commit 2da015cd6 and current)
            // only reaches this dispatch for `intermediates.len == 2`, i.e. the
            // private form `CSI ? $ p` (ansi = false). The ANSI form `CSI $ p`
            // (one intermediate) is *dead code* in upstream: its inner
            // `len == 1 => $` branch sits under an outer `2 =>` arm that a
            // single-intermediate sequence never matches, so `CSI $ p` falls to
            // the `else` (log-and-ignore). We match that reachable behavior
            // exactly — only `? $ p` is answered.
            b'p' => {
                if intermediates.len() == 2
                    && intermediates[0] == b'?'
                    && intermediates[1] == b'$'
                    && params.len() == 1
                {
                    let raw = params[0];
                    match modes::mode_from_int(raw, false) {
                        Some(m) => self.handler.request_mode(m),
                        None => self.handler.request_mode_unknown(raw, false),
                    }
                }
            }
            // DECSCUSR / DECSCA / XTVERSION
            b'q' => {
                if intermediates.len() == 1 {
                    match intermediates[0] {
                        b' ' => {
                            let style = match params.len() {
                                0 => Some(CursorStyle::Default),
                                1 => cursor_style_from_param(params[0]),
                                _ => None,
                            };
                            if let Some(s) = style {
                                self.handler.cursor_style(s);
                            }
                        }
                        b'"' => {
                            let mode = match params.len() {
                                0 => Some(crate::terminal::ProtectedMode::Off),
                                1 => match params[0] {
                                    0 | 2 => Some(crate::terminal::ProtectedMode::Off),
                                    1 => Some(crate::terminal::ProtectedMode::Dec),
                                    _ => None,
                                },
                                _ => None,
                            };
                            if let Some(m) = mode {
                                self.handler.protected_mode(m);
                            }
                        }
                        b'>' => self.handler.xtversion(),
                        _ => {}
                    }
                }
            }
            // DECSTBM / DECRSM
            b'r' => match intermediates.len() {
                0 => match params.len() {
                    0 => self.handler.top_and_bottom_margin(0, 0),
                    1 => self.handler.top_and_bottom_margin(params[0], 0),
                    2 => self.handler.top_and_bottom_margin(params[0], params[1]),
                    _ => {}
                },
                1 if intermediates[0] == b'?' => {
                    for &m in params {
                        if let Some(mode) = modes::mode_from_int(m, false) {
                            self.handler.restore_mode(mode);
                        }
                    }
                }
                _ => {}
            },
            // DECSLRM / DECSC-save-mode / XTSHIFTESCAPE
            b's' => match intermediates.len() {
                0 => match params.len() {
                    0 => self.handler.left_and_right_margin_ambiguous(),
                    1 => self.handler.left_and_right_margin(params[0], 0),
                    2 => self.handler.left_and_right_margin(params[0], params[1]),
                    _ => {}
                },
                1 if intermediates[0] == b'?' => {
                    for &m in params {
                        if let Some(mode) = modes::mode_from_int(m, false) {
                            self.handler.save_mode(mode);
                        }
                    }
                }
                // XTSHIFTESCAPE (`CSI > Ps s`). State only; interpreted by the
                // input layer. Port of the `'>'` capture prong.
                1 if intermediates[0] == b'>' => {
                    let capture = match params.len() {
                        0 => Some(false),
                        1 => match params[0] {
                            0 => Some(false),
                            1 => Some(true),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(capture) = capture {
                        self.handler.mouse_shift_capture(capture);
                    }
                }
                _ => {}
            },
            // XTWINOPS (`CSI … t`). We implement the title push/pop ops (22/23)
            // that upstream backs with terminal state; the size-report ops
            // (14/16/18/21) route to a pixel-geometry effect callback upstream
            // and remain a documented seam here (they need a size effect this
            // chunk's `Terminal` does not own — see docs/analysis/stream.md).
            // Port of the `'t'` prong.
            b't' => {
                if intermediates.is_empty() && !params.is_empty() {
                    match params[0] {
                        // 14/16/18/21: size reports — seam (need pixel geometry).
                        14 | 16 | 18 | 21 => {}
                        // 22/23: push/pop title. We only support window title
                        // (param[1] must be 0 or 2); when present, param[2] is
                        // the stack index. Port of the inline `22, 23 => …`
                        // block.
                        n @ (22 | 23)
                            if (params.len() == 2 || params.len() == 3)
                                && (params[1] == 0 || params[1] == 2) =>
                        {
                            let index = if params.len() == 3 { params[2] } else { 0 };
                            if n == 22 {
                                self.handler.title_push(index);
                            } else {
                                self.handler.title_pop(index);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // DECRC (u, no intermediate) / kitty keyboard protocol (`? > < =`)
            b'u' => match intermediates.len() {
                0 => self.handler.restore_cursor(),
                1 => match intermediates[0] {
                    // Query current flags.
                    b'?' => self.handler.kitty_keyboard_query(),
                    // Push flags (a u5; out-of-range is ignored).
                    b'>' => {
                        let flags = match params.len() {
                            0 => Some(0u8),
                            1 if params[0] <= 0b1_1111 => Some(params[0] as u8),
                            _ => None,
                        };
                        if let Some(flags) = flags {
                            self.handler.kitty_keyboard_push(
                                crate::screen::kitty_key::Flags::from_int(flags),
                            );
                        }
                    }
                    // Pop `count` entries (default 1).
                    b'<' => {
                        let count = if params.len() == 1 { params[0] } else { 1 };
                        self.handler.kitty_keyboard_pop(count);
                    }
                    // Set flags with a mode (1=set, 2=or, 3=not; default set).
                    b'=' => {
                        let flags = if !params.is_empty() {
                            if params[0] <= 0b1_1111 {
                                Some(params[0] as u8)
                            } else {
                                None
                            }
                        } else {
                            Some(0u8)
                        };
                        let mode = match params.get(1).copied().unwrap_or(1) {
                            1 => Some(crate::screen::kitty_key::SetMode::Set),
                            2 => Some(crate::screen::kitty_key::SetMode::Or),
                            3 => Some(crate::screen::kitty_key::SetMode::Not),
                            _ => None,
                        };
                        if let (Some(flags), Some(mode)) = (flags, mode) {
                            self.handler.kitty_keyboard_set(
                                mode,
                                crate::screen::kitty_key::Flags::from_int(flags),
                            );
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            // ICH
            b'@' => {
                if intermediates.is_empty() {
                    let v = match params.len() {
                        0 => Some(1),
                        1 => Some(params[0].max(1)),
                        _ => None,
                    };
                    if let Some(v) = v {
                        self.handler.insert_blanks(v);
                    }
                }
            }
            // DECSASD
            b'}' if intermediates.len() == 1 && intermediates[0] == b'$' && params.len() == 1 => {
                let display = match params[0] {
                    0 => Some(crate::terminal::StatusDisplay::Main),
                    1 => Some(crate::terminal::StatusDisplay::StatusLine),
                    _ => None,
                };
                if let Some(d) = display {
                    self.handler.active_status_display(d);
                }
            }
            _ => {}
        }
    }

    /// Port of `escDispatch`.
    fn esc_dispatch(&mut self, action: &EscOwned) {
        use crate::charsets::{ActiveSlot, Charset, Slots};
        let intermediates: &[u8] = action.intermediates();
        let no_inter = intermediates.is_empty();
        match action.final_byte {
            // Charset designations.
            b'B' => self
                .handler
                .configure_charset(intermediates, Charset::Ascii),
            b'A' => self
                .handler
                .configure_charset(intermediates, Charset::British),
            b'0' => self
                .handler
                .configure_charset(intermediates, Charset::DecSpecial),
            // DECSC
            b'7' if no_inter => self.handler.save_cursor(),
            // DECRC / DECALN
            b'8' => {
                if no_inter {
                    self.handler.restore_cursor();
                } else if intermediates == b"#" {
                    self.handler.decaln();
                }
            }
            // IND
            b'D' if no_inter => self.handler.index(),
            // NEL
            b'E' if no_inter => self.handler.next_line(),
            // HTS
            b'H' if no_inter => self.handler.tab_set(),
            // RI
            b'M' if no_inter => self.handler.reverse_index(),
            // SS2 / SS3
            b'N' if no_inter => self.handler.invoke_charset(ActiveSlot::Gl, Slots::G2, true),
            b'O' if no_inter => self.handler.invoke_charset(ActiveSlot::Gl, Slots::G3, true),
            // SPA / EPA
            b'V' if no_inter => self
                .handler
                .protected_mode(crate::terminal::ProtectedMode::Iso),
            b'W' if no_inter => self
                .handler
                .protected_mode(crate::terminal::ProtectedMode::Off),
            // DECID
            b'Z' if no_inter => self.handler.device_attributes(DeviceAttributesReq::Primary),
            // RIS
            b'c' if no_inter => self.handler.full_reset(),
            // LS2 / LS3
            b'n' if no_inter => self
                .handler
                .invoke_charset(ActiveSlot::Gl, Slots::G2, false),
            b'o' if no_inter => self
                .handler
                .invoke_charset(ActiveSlot::Gl, Slots::G3, false),
            // LS1R / LS2R / LS3R
            b'~' if no_inter => self
                .handler
                .invoke_charset(ActiveSlot::Gr, Slots::G1, false),
            b'}' if no_inter => self
                .handler
                .invoke_charset(ActiveSlot::Gr, Slots::G2, false),
            b'|' if no_inter => self
                .handler
                .invoke_charset(ActiveSlot::Gr, Slots::G3, false),
            // Application/normal keypad.
            b'=' if no_inter => self.handler.set_mode(Mode::KeypadKeys, true),
            b'>' if no_inter => self.handler.set_mode(Mode::KeypadKeys, false),
            // ST: nothing to do.
            b'\\' => {}
            _ => {}
        }
    }

    /// Port of `oscDispatch`.
    fn osc_dispatch(&mut self, cmd: osc::Command) {
        use osc::Command as C;
        match cmd {
            C::SemanticPrompt(sp) => self.handler.semantic_prompt(&sp),
            C::ChangeWindowTitle(title) => {
                // Upstream validates UTF-8; the Rust osc parser already
                // captured a `String`, so it is valid by construction.
                self.handler.window_title(&title);
            }
            C::ChangeWindowIcon(_) => {}
            C::ReportPwd { value } => self.handler.report_pwd(&value),
            C::MouseShape { value } => self.handler.mouse_shape(&value),
            C::ColorOperation { requests, .. } => self.handler.color_operation(&requests),
            C::KittyColorProtocol(k) => self.handler.kitty_color(&k),
            C::ClipboardContents { kind, data } => self.handler.clipboard(kind, &data),
            C::HyperlinkStart { .. } | C::HyperlinkEnd => {
                // Hyperlink start/end are Screen effects (seam); not needed
                // for the differential screen-text comparison.
            }
            // Everything else has no terminal-modifying effect (kitty
            // clipboard protocol (5522, a NON-goal — parsed but not applied,
            // see module docs), notifications, conemu, kitty text/dnd,
            // context signal).
            _ => {}
        }
    }
}

// ---- small pure helpers (port of the inline switch bodies) --------------

fn protected_from_intermediates(intermediates: &[u8]) -> Option<bool> {
    match intermediates.len() {
        0 => Some(false),
        1 if intermediates[0] == b'?' => Some(true),
        _ => None,
    }
}

fn erase_display_from_param(param: u16) -> Option<EraseDisplay> {
    match param {
        0 => Some(EraseDisplay::Below),
        1 => Some(EraseDisplay::Above),
        2 => Some(EraseDisplay::Complete),
        3 => Some(EraseDisplay::Scrollback),
        22 => Some(EraseDisplay::ScrollComplete),
        _ => None,
    }
}

fn mode_ansi(intermediates: &[u8]) -> Option<bool> {
    match intermediates.len() {
        0 => Some(true),
        1 if intermediates[0] == b'?' => Some(false),
        _ => None,
    }
}

fn cursor_style_from_param(param: u16) -> Option<CursorStyle> {
    Some(match param {
        0 => CursorStyle::Default,
        1 => CursorStyle::BlinkingBlock,
        2 => CursorStyle::SteadyBlock,
        3 => CursorStyle::BlinkingUnderline,
        4 => CursorStyle::SteadyUnderline,
        5 => CursorStyle::BlinkingBar,
        6 => CursorStyle::SteadyBar,
        _ => return None,
    })
}

// -------------------------------------------------------------------------
// TerminalHandler: the concrete handler over a `Terminal`, with a reply queue.
// Port of `stream_terminal.Handler`.
// -------------------------------------------------------------------------

/// The concrete stream handler that drives a [`Terminal`] and accumulates
/// query replies (DSR/DA/CPR/DECRQSS/mode reports) into a caller-drainable
/// output queue. Port of `stream_terminal.Handler` (the reply effects become
/// pushes onto `output`).
pub struct TerminalHandler {
    pub terminal: Terminal,
    /// Accumulated replies destined for the pty, in order.
    pub output: Reply,
    /// The most recent OSC 52 clipboard *write* request, if one hasn't been
    /// drained yet. Port of the spike/architecture-doc's
    /// `Terminal::take_clipboard()` side-effect queue, now sitting on
    /// `TerminalHandler` alongside the reply queue. Only write requests are
    /// queued here (`data != "?"`); a read request (`data == "?"`) has no
    /// terminal-state effect to surface — upstream's `clipboardContents`
    /// turns it into a distinct `clipboard_read` apprt message instead of a
    /// `clipboard_write`, and this crate doesn't model the read-reply path
    /// (embedder-specific; a future chunk can add a `take_clipboard_read()`
    /// analogue if a frontend needs to answer OSC 52 queries).
    ///
    /// Matches upstream's policy of handing the apprt *raw* (still
    /// base64-encoded) bytes — decoding is a host/embedder decision (e.g.
    /// `Surface.clipboardWrite` in Zig), not a terminal-core one.
    pending_clipboard: Option<(u8, String)>,
    /// The APC sub-protocol handler (kitty graphics / glyph). Port of
    /// `stream_terminal.Handler.apc_handler`. The stream's `apc_start`/
    /// `apc_put`/`apc_end` events drive it; on `end` a completed
    /// [`apc::Command::KittyRaw`] is parsed by [`kitty::CommandParser`] and
    /// executed against the terminal ([`kitty::execute`]), with any response
    /// pushed onto `output`. Glyph commands are a documented seam.
    apc_handler: apc::Handler,
}

impl TerminalHandler {
    pub fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            output: Vec::new(),
            pending_clipboard: None,
            apc_handler: apc::Handler::new(),
        }
    }

    /// Drain the accumulated reply bytes.
    pub fn take_output(&mut self) -> Reply {
        std::mem::take(&mut self.output)
    }

    /// Drain the most recent OSC 52 clipboard write request, if any. Returns
    /// `(kind, raw_data)` where `raw_data` is the still-base64-encoded OSC
    /// body (empty string means "clear the clipboard"), matching upstream's
    /// policy of not decoding at the terminal-core layer. Only the most
    /// recent write is kept (matches how a reply queue would coalesce a
    /// rapid burst into "latest wins" for a UI-facing side effect; screen
    /// text and query replies are unaffected).
    pub fn take_clipboard(&mut self) -> Option<(u8, String)> {
        self.pending_clipboard.take()
    }

    fn write_pty(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    /// Port of `setMode`'s mode-specific side effects (the ones that affect
    /// terminal state; mouse/format flags and pure seams are elided).
    fn apply_mode_side_effects(&mut self, mode: Mode, enabled: bool) {
        match mode {
            Mode::Origin => self.terminal.set_cursor_pos(1, 1),
            Mode::EnableLeftAndRightMargin => {
                if !enabled {
                    self.terminal.scrolling_region.left = 0;
                    self.terminal.scrolling_region.right = self.terminal.cols - 1;
                }
            }
            Mode::AltScreenLegacy => {
                self.terminal
                    .switch_screen_mode(SwitchScreenMode::M47, enabled);
            }
            Mode::AltScreen => {
                self.terminal
                    .switch_screen_mode(SwitchScreenMode::M1047, enabled);
            }
            Mode::AltScreenSaveCursorClearEnter => {
                self.terminal
                    .switch_screen_mode(SwitchScreenMode::M1049, enabled);
            }
            Mode::SaveCursor => {
                if enabled {
                    self.terminal.save_cursor();
                } else {
                    self.terminal.restore_cursor();
                }
            }
            Mode::Column132 => self.terminal.deccolm(enabled),
            _ => {}
        }
    }
}

impl Handler for TerminalHandler {
    fn print(&mut self, cp: u32) {
        self.terminal.print(cp);
    }
    fn print_slice(&mut self, cps: &[u32]) {
        self.terminal.print_slice(cps);
    }

    fn backspace(&mut self) {
        self.terminal.backspace();
    }
    fn carriage_return(&mut self) {
        self.terminal.carriage_return();
    }
    fn linefeed(&mut self) {
        self.terminal.linefeed();
    }
    fn index(&mut self) {
        self.terminal.index();
    }
    fn next_line(&mut self) {
        self.terminal.index();
        self.terminal.carriage_return();
    }
    fn reverse_index(&mut self) {
        self.terminal.reverse_index();
    }
    fn bell(&mut self) {}
    fn enquiry(&mut self) {}

    fn cursor_up(&mut self, count: u16) {
        self.terminal.cursor_up(count as usize);
    }
    fn cursor_down(&mut self, count: u16) {
        self.terminal.cursor_down(count as usize);
    }
    fn cursor_left(&mut self, count: u16) {
        self.terminal.cursor_left(count as usize);
    }
    fn cursor_right(&mut self, count: u16) {
        self.terminal.cursor_right(count as usize);
    }
    fn cursor_pos(&mut self, row: u16, col: u16) {
        self.terminal.set_cursor_pos(row as usize, col as usize);
    }
    fn cursor_col(&mut self, col: u16) {
        let y = self.terminal.screen().cursor.y as usize;
        self.terminal.set_cursor_pos(y + 1, col as usize);
    }
    fn cursor_row(&mut self, row: u16) {
        let x = self.terminal.screen().cursor.x as usize;
        self.terminal.set_cursor_pos(row as usize, x + 1);
    }
    fn cursor_col_relative(&mut self, count: u16) {
        let y = self.terminal.screen().cursor.y as usize;
        let x = self.terminal.screen().cursor.x as usize;
        self.terminal.set_cursor_pos(y + 1, x + 1 + count as usize);
    }
    fn cursor_row_relative(&mut self, count: u16) {
        let y = self.terminal.screen().cursor.y as usize;
        let x = self.terminal.screen().cursor.x as usize;
        self.terminal.set_cursor_pos(y + 1 + count as usize, x + 1);
    }
    fn save_cursor(&mut self) {
        self.terminal.save_cursor();
    }
    fn restore_cursor(&mut self) {
        self.terminal.restore_cursor();
    }

    fn horizontal_tab(&mut self, count: u16) {
        for _ in 0..count {
            let x = self.terminal.screen().cursor.x;
            self.terminal.horizontal_tab();
            if x == self.terminal.screen().cursor.x {
                break;
            }
        }
    }
    fn horizontal_tab_back(&mut self, count: u16) {
        for _ in 0..count {
            let x = self.terminal.screen().cursor.x;
            self.terminal.horizontal_tab_back();
            if x == self.terminal.screen().cursor.x {
                break;
            }
        }
    }
    fn tab_clear(&mut self, cmd: TabClear) {
        self.terminal.tab_clear(cmd);
    }
    fn tab_set(&mut self) {
        self.terminal.tab_set();
    }
    fn tab_reset(&mut self) {
        self.terminal.tab_reset();
    }

    fn erase_display(&mut self, mode: EraseDisplay, protected: bool) {
        self.terminal.erase_display(mode, protected);
    }
    fn erase_line(&mut self, mode: EraseLine, protected: bool) {
        self.terminal.erase_line(mode, protected);
    }
    fn delete_chars(&mut self, count: u16) {
        self.terminal.delete_chars(count as usize);
    }
    fn erase_chars(&mut self, count: u16) {
        self.terminal.erase_chars(count as usize);
    }
    fn insert_lines(&mut self, count: u16) {
        self.terminal.insert_lines(count as usize);
    }
    fn insert_blanks(&mut self, count: u16) {
        self.terminal.insert_blanks(count as usize);
    }
    fn delete_lines(&mut self, count: u16) {
        self.terminal.delete_lines(count as usize);
    }
    fn scroll_up(&mut self, count: u16) {
        self.terminal.scroll_up(count as usize);
    }
    fn scroll_down(&mut self, count: u16) {
        self.terminal.scroll_down(count as usize);
    }

    fn set_mode(&mut self, mode: Mode, enabled: bool) {
        self.terminal.modes.set(mode, enabled);
        self.apply_mode_side_effects(mode, enabled);
    }
    fn save_mode(&mut self, mode: Mode) {
        self.terminal.modes.save(mode);
    }
    fn restore_mode(&mut self, mode: Mode) {
        let v = self.terminal.modes.restore(mode);
        self.set_mode(mode, v);
    }
    fn top_and_bottom_margin(&mut self, top: u16, bottom: u16) {
        self.terminal
            .set_top_and_bottom_margin(top as usize, bottom as usize);
    }
    fn left_and_right_margin(&mut self, left: u16, right: u16) {
        self.terminal
            .set_left_and_right_margin(left as usize, right as usize);
    }
    fn left_and_right_margin_ambiguous(&mut self) {
        if self.terminal.modes.get(Mode::EnableLeftAndRightMargin) {
            self.terminal.set_left_and_right_margin(0, 0);
        } else {
            self.terminal.save_cursor();
        }
    }

    fn configure_charset(&mut self, intermediates: &[u8], set: crate::charsets::Charset) {
        if intermediates.len() != 1 {
            return;
        }
        let slot = match intermediates[0] {
            b'(' => crate::charsets::Slots::G0,
            b')' => crate::charsets::Slots::G1,
            b'*' => crate::charsets::Slots::G2,
            b'+' => crate::charsets::Slots::G3,
            _ => return,
        };
        self.terminal.configure_charset(slot, set);
    }
    fn invoke_charset(
        &mut self,
        active: crate::charsets::ActiveSlot,
        slot: crate::charsets::Slots,
        single: bool,
    ) {
        self.terminal.invoke_charset(active, slot, single);
    }

    fn set_attribute(&mut self, attr: sgr::Attribute) {
        // Ignore Unset/Unknown like upstream (`.unknown => {}`).
        match attr {
            sgr::Attribute::Unknown(_) => {}
            other => self.terminal.set_attribute(other),
        }
    }

    fn protected_mode(&mut self, mode: crate::terminal::ProtectedMode) {
        self.terminal.set_protected_mode(mode);
    }
    fn active_status_display(&mut self, display: crate::terminal::StatusDisplay) {
        self.terminal.status_display = display;
    }

    fn decaln(&mut self) {
        self.terminal.decaln();
    }
    fn full_reset(&mut self) {
        self.terminal.full_reset();
    }

    fn cursor_style(&mut self, style: CursorStyle) {
        // The Rust Screen doesn't model cursor-style rendering; upstream sets
        // `cursor_blinking` mode + `cursor.cursor_style`. We only track the
        // blink mode, which is observable via mode reports.
        let blink = matches!(
            style,
            CursorStyle::BlinkingBlock | CursorStyle::BlinkingBar | CursorStyle::BlinkingUnderline
        );
        if !matches!(style, CursorStyle::Default) {
            self.terminal.modes.set(Mode::CursorBlinking, blink);
        }
    }

    fn window_title(&mut self, title: &str) {
        const MAX: usize = 1024;
        let t = if title.len() > MAX {
            &title[..MAX]
        } else {
            title
        };
        self.terminal.set_title(t.as_bytes());
    }
    fn report_pwd(&mut self, url: &str) {
        const MAX: usize = 4096;
        let u = if url.len() > MAX { &url[..MAX] } else { url };
        self.terminal.set_pwd(u.as_bytes());
    }
    fn semantic_prompt(&mut self, cmd: &osc::SemanticPrompt) {
        self.terminal.semantic_prompt(cmd);
    }
    fn color_operation(&mut self, requests: &osc::ColorList) {
        use osc::{ColorRequest, ColorTarget, Dynamic};
        for req in requests {
            match req {
                ColorRequest::Set { target, color } => match target {
                    ColorTarget::Palette(i) => {
                        self.terminal.flags.dirty.palette = true;
                        self.terminal.colors.palette.set(*i, *color);
                    }
                    ColorTarget::Dynamic(dynamic) => match dynamic {
                        Dynamic::Foreground => self.terminal.colors.foreground.set(*color),
                        Dynamic::Background => self.terminal.colors.background.set(*color),
                        Dynamic::Cursor => self.terminal.colors.cursor.set(*color),
                        _ => {}
                    },
                    ColorTarget::Special(_) => {}
                },
                ColorRequest::Reset(target) => match target {
                    ColorTarget::Palette(i) => {
                        self.terminal.flags.dirty.palette = true;
                        self.terminal.colors.palette.reset(*i);
                    }
                    ColorTarget::Dynamic(dynamic) => match dynamic {
                        Dynamic::Foreground => self.terminal.colors.foreground.reset(),
                        Dynamic::Background => self.terminal.colors.background.reset(),
                        Dynamic::Cursor => self.terminal.colors.cursor.reset(),
                        _ => {}
                    },
                    ColorTarget::Special(_) => {}
                },
                ColorRequest::ResetPalette => {
                    self.terminal.flags.dirty.palette = true;
                    self.terminal.colors.palette.reset_all();
                }
                ColorRequest::Query { .. } | ColorRequest::ResetSpecial => {}
            }
        }
    }
    fn kitty_color(&mut self, _cmd: &osc::KittyColorProtocol) {
        // Kitty color-set effects mirror color_operation; queries emit
        // replies. Left as a seam (not needed for the differential text
        // comparison; the fixtures don't use OSC 21).
    }
    fn mouse_shape(&mut self, _value: &str) {
        // Stored on flags in upstream; not interpreted by Terminal.
    }
    fn clipboard(&mut self, kind: u8, data: &str) {
        // Port of `stream_terminal.Handler.clipboardContents`: a lone `?`
        // is a *read* request (upstream dispatches a `clipboard_read`
        // apprt message and returns, writing nothing) — no terminal-state
        // effect to queue here. Anything else (including empty, i.e.
        // "clear") is a write request the embedder should drain and act on.
        if data == "?" {
            return;
        }
        self.pending_clipboard = Some((kind, data.to_string()));
    }

    // ---- reports: build the reply bytes and push onto `output` ----------
    fn device_attributes(&mut self, req: DeviceAttributesReq) {
        // Match libghostty's default DA responses.
        match req {
            // Primary: VT220 with common feature bits (same set libghostty
            // advertises: 62;22 at minimum). We emit the widely-compatible
            // `\e[?62;22c`.
            DeviceAttributesReq::Primary => self.write_pty(b"\x1b[?62;22c"),
            // Secondary: `\e[>1;10;0c` (VT220-ish, version 10).
            DeviceAttributesReq::Secondary => self.write_pty(b"\x1b[>1;10;0c"),
            // Tertiary: DECRPTUI, empty unit id.
            DeviceAttributesReq::Tertiary => self.write_pty(b"\x1bP!|00000000\x1b\\"),
        }
    }
    fn device_status(&mut self, req: DeviceStatusReq) {
        match req {
            DeviceStatusReq::OperatingStatus => self.write_pty(b"\x1b[0n"),
            DeviceStatusReq::CursorPosition => {
                let (x, y) = if self.terminal.modes.get(Mode::Origin) {
                    (
                        self.terminal
                            .screen()
                            .cursor
                            .x
                            .saturating_sub(self.terminal.scrolling_region.left),
                        self.terminal
                            .screen()
                            .cursor
                            .y
                            .saturating_sub(self.terminal.scrolling_region.top),
                    )
                } else {
                    (
                        self.terminal.screen().cursor.x,
                        self.terminal.screen().cursor.y,
                    )
                };
                let resp = format!("\x1b[{};{}R", y + 1, x + 1);
                self.write_pty(resp.as_bytes());
            }
        }
    }
    fn request_mode(&mut self, mode: Mode) {
        let report = self
            .terminal
            .modes
            .get_report(modes::ModeTag::from_mode(mode));
        let mut s = String::new();
        report.encode(&mut s);
        self.write_pty(s.as_bytes());
    }
    fn request_mode_unknown(&mut self, mode_raw: u16, ansi: bool) {
        let report = self.terminal.modes.get_report(modes::ModeTag {
            value: mode_raw,
            ansi,
        });
        let mut s = String::new();
        report.encode(&mut s);
        self.write_pty(s.as_bytes());
    }
    fn decrqss(&mut self, setting: dcs::Decrqss) {
        // Build the `\eP1$r ... \e\\` reply. Only SGR is answered with real
        // content; the others match upstream's DECRQSS surface.
        let body = match setting {
            // SGR: the params from `printAttributes`, suffixed with the
            // request's own final byte `m` (xterm echoes the setting).
            dcs::Decrqss::Sgr => format!("{}m", self.terminal.print_attributes()),
            // DECSTBM: report the current scrolling region as `top;bottom r`.
            dcs::Decrqss::Decstbm => format!(
                "{};{}r",
                self.terminal.scrolling_region.top + 1,
                self.terminal.scrolling_region.bottom + 1
            ),
            // DECSLRM: `left;right s`.
            dcs::Decrqss::Decslrm => format!(
                "{};{}s",
                self.terminal.scrolling_region.left + 1,
                self.terminal.scrolling_region.right + 1
            ),
            // DECSCUSR / none: nothing meaningful to report.
            dcs::Decrqss::Decscusr | dcs::Decrqss::None => String::new(),
        };
        if body.is_empty() {
            return;
        }
        let reply = format!("\x1bP1$r{body}\x1b\\");
        self.write_pty(reply.as_bytes());
    }
    fn xtversion(&mut self) {
        self.write_pty(b"\x1bP>|libghostty\x1b\\");
    }

    // ---- REP ------------------------------------------------------------
    fn print_repeat(&mut self, count: u16) {
        self.terminal.print_repeat(count as usize);
    }

    // ---- kitty keyboard protocol ----------------------------------------
    fn kitty_keyboard_query(&mut self) {
        // Port of `queryKittyKeyboard`: `CSI ? <flags> u` with the current
        // stack value (a u5).
        let flags = self.terminal.screen().kitty_keyboard.current().int();
        let resp = format!("\x1b[?{flags}u");
        self.write_pty(resp.as_bytes());
    }
    fn kitty_keyboard_push(&mut self, flags: crate::screen::kitty_key::Flags) {
        self.terminal.screen_mut().kitty_keyboard.push(flags);
    }
    fn kitty_keyboard_pop(&mut self, count: u16) {
        self.terminal
            .screen_mut()
            .kitty_keyboard
            .pop(count as usize);
    }
    fn kitty_keyboard_set(
        &mut self,
        mode: crate::screen::kitty_key::SetMode,
        flags: crate::screen::kitty_key::Flags,
    ) {
        self.terminal.screen_mut().kitty_keyboard.set(mode, flags);
    }

    // ---- XTWINOPS title stack (no terminal-core effect) -----------------
    fn title_push(&mut self, _index: u16) {
        // Port of upstream's `.title_push => {}`: the title stack lives in the
        // apprt/surface layer, not the terminal core.
    }
    fn title_pop(&mut self, _index: u16) {
        // Port of upstream's `.title_pop => {}`.
    }

    // ---- XTSHIFTESCAPE (state only) -------------------------------------
    fn mouse_shift_capture(&mut self, capture: bool) {
        // Port of `.mouse_shift_capture => self.terminal.flags.mouse_shift_capture = …`.
        self.terminal.flags.mouse_shift_capture = if capture {
            crate::terminal::MouseShiftCapture::True
        } else {
            crate::terminal::MouseShiftCapture::False
        };
    }

    // ---- APC (kitty graphics / glyph) -----------------------------------
    fn apc_start(&mut self) {
        self.apc_handler.start();
    }
    fn apc_put(&mut self, byte: u8) {
        self.apc_handler.feed(byte);
    }
    fn apc_end(&mut self) {
        // Port of `stream_terminal.Handler.apcEnd`. Finalize the APC handler;
        // a completed kitty-graphics command is parsed and executed against the
        // terminal, and any response is written back out the reply queue. Glyph
        // commands remain a documented seam (`TODO(chunk:font-glyph-protocol)`).
        let Some(cmd) = self.apc_handler.end() else {
            return;
        };
        match cmd {
            apc::Command::KittyRaw(raw) => {
                // The seam hands us the raw APC payload bytes (everything after
                // the identifying `G`). Parse them with the kitty command
                // grammar, then execute. A parse failure is silently dropped
                // (matches upstream, where a malformed kitty command yields no
                // response).
                let Ok(kitty_cmd) = kitty::CommandParser::parse_string(&raw) else {
                    return;
                };
                if let Some(resp) = kitty::execute(&mut self.terminal, &kitty_cmd) {
                    // Encode the response as a complete APC sequence and write
                    // it. `Response::encode` returns the empty string when there
                    // is nothing to say (no id/number), in which case we write
                    // nothing.
                    let encoded = resp.encode();
                    if !encoded.is_empty() {
                        self.write_pty(encoded.as_bytes());
                    }
                }
            }
            // Glyph protocol needs the font subsystem (not yet ported); the APC
            // handler already recognizes and buffers it, but there is no
            // consumer, so it is dropped here. `TODO(chunk:font-glyph-protocol)`.
            apc::Command::GlyphRaw(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
