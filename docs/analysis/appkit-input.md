# AppKit text input: upstream flow, winit-vs-AppKit, and the R5 recommendation

R5 de-risk spike (see `docs/plans/m3-first-pixels.md`). Proves macOS text input
— dead keys, IME composition, modifier fidelity, `macos-option-as-alt` — flows
from an AppKit `NSTextInputClient` `NSView` into `ghostty-input`'s encoder, and
settles whether the R5 window host should be raw AppKit or winit 0.30 **before**
R5 builds on the wrong one.

Upstream citations are commit-stamped. Ghostty (Swift/Zig) refs are at commit
`38e49a23`; the spike code lives in `spikes/appkit-input/`.

## 1. Upstream's exact keyDown event flow

Source: `macos/Sources/Ghostty/Surface View/SurfaceView_AppKit.swift`,
`macos/Sources/Ghostty/NSEvent+Extension.swift`,
`macos/Sources/Ghostty/Ghostty.Input.swift`, and `src/apprt/embedded.zig`
(all @ `38e49a23`).

### 1.1 The keyDown → interpretKeyEvents → insertText/setMarkedText dance

`keyDown(with:)` (SurfaceView_AppKit.swift:1078) does, in order:

1. **Compute translation mods** (:1088). Calls
   `ghostty_surface_key_translation_mods(surface, ghosttyMods(flags))`
   (embedded.zig:1763), which applies `Mods.translation(option_as_alt)` — this
   is where `macos-option-as-alt` strips the Option bit from the modifiers used
   for *character translation* (but NOT from the modifiers sent to the encoder).
   The "hidden bits" loop (:1099) copies only the four canonical flags back onto
   the raw `event.modifierFlags`, because dead keys depend on some private
   modifier bits that must be preserved.
2. **Reuse-or-rebuild the NSEvent** (:1112). If the translated mods differ from
   the original, it builds a *new* `NSEvent.keyEvent(...)` with
   `characters(byApplyingModifiers: translationMods)`. IMPORTANT (upstream's own
   caps): it MUST reuse the original event object when mods are unchanged or
   Korean input breaks — there's object-identity coupling somewhere in AppKit.
3. **Open the accumulator** (:1135). `keyTextAccumulator = []`. From here,
   `insertText` appends to this array instead of sending immediately.
4. **Snapshot marked-text + keyboard-id state** (:1140, :1145) so it can tell
   afterward whether this event cleared preedit or swapped the input source.
5. **`interpretKeyEvents([translationEvent])`** (:1156). This is the AppKit call
   that routes the event to the current input context, which then calls back
   into our `NSTextInputClient` methods: `setMarkedText:` for preedit,
   `insertText:` for a commit, `doCommandBySelector:` for editing commands.
6. **Post-interpret triage** (:1160–1246):
   - If the keyboard layout changed and there was no marked text, assume an IME
     grabbed the key; return (:1160).
   - `syncPreedit(clearIfNeeded:)` (:1167) pushes the current marked string to
     libghostty via `ghostty_surface_preedit`.
   - `composing = markedText.length > 0 || markedTextBefore` (:1175).
   - If text was committed while marked-before, send each committed chunk via
     `committedPreeditTextAction`, suppressing bare control chars while
     composing (:1181).
   - Else if the accumulator has text, that's the committed compose result:
     `keyAction(..., text:)` for each (:1206).
   - Else it's a normal key: `keyAction(..., text: translationEvent.ghosttyCharacters, composing:)`
     (:1239).

`keyAction` (:1452) builds the C key event via `event.ghosttyKeyEvent(action, translationMods:)`,
sets `composing`, and — crucially — only attaches `text` when it is **not** a
single control character (`codepoint >= 0x20`, :1467). Control chars are encoded
by Ghostty's `KeyEncoder`, not passed as literal text. Then
`ghostty_surface_key(surface, key_ev)` (embedded.zig:1781) hands it to the
shared encoder.

### 1.2 The NSTextInputClient conformance

`extension Ghostty.SurfaceView: NSTextInputClient` (SurfaceView_AppKit.swift:1879):

- `setMarkedText(_:selectedRange:replacementRange:)` (:1901) stores the marked
  string; if not inside a keyDown (`keyTextAccumulator == nil`, e.g. layout
  change mid-compose) it `syncPreedit()`s immediately.
- `unmarkText()` (:1922) clears marked text and syncs (composing ended).
- `insertText(_:replacementRange:)` (:2029) calls `unmarkText()` (a commit means
  preedit is over), then either appends to `keyTextAccumulator` (inside a key
  event) or `surfaceModel.sendText(chars)` directly.
- `hasMarkedText`/`markedRange`/`selectedRange`/`firstRectForCharacterRange`
  (IME box placement)/`attributedSubstring` (QuickLook + dictation) round out the
  protocol.
- `syncPreedit` (:2073) is the single funnel that mirrors marked text into
  libghostty (`ghostty_surface_preedit(ptr, len)` or `(nil, 0)` to clear).

### 1.3 When performKeyEquivalent fires, and why it's separate

`performKeyEquivalent(with:)` (:1282) fires **before** `keyDown` for command-ish
keys because an `NSTextInputClient`/responder chain and the menu bar get first
crack at key-equivalents. Upstream's handling:

- Guards on focus (:1293) — otherwise C-/ leaks to the wrong view.
- Asks libghostty `keyIsBinding` (:1298). If it's a binding and the binding is
  `consumed` (not `all`/`performable`), it tries the menu key-equivalent first
  (:1314), else calls `self.keyDown(with: event)` to encode it (:1324).
- Special-cases C-Return (:1330) and C-/ → C-_ (:1339, to dodge the macOS beep).
- For other Cmd/Ctrl events it uses the `lastPerformKeyEvent` timestamp trick
  (:1376) to decide, on the *second* pass through `doCommandBySelector`
  (:2062), whether to re-inject the event into `keyDown` for encoding. This
  round-trip exists because macOS doesn't tell you what a command is bound to
  until you let it flow through the system (:1263 docstring).

**Takeaway:** `performKeyEquivalent` is non-optional for correct Cmd/Ctrl
handling on macOS — a raw `NSView` gives full control of it; a framework that
owns the responder chain does not.

### 1.4 How option-as-alt bypasses composition

`Mods.translation(option_as_alt)` (our `key_mods.rs:194`, port of the Zig) drops
the `alt` bit from the *text-translation* mods when option-as-alt is active
(`True`, or the pressed side under `Left`/`Right`). AppKit therefore asks macOS
for `characters(byApplyingModifiers:)` **without** Option, so it produces the
base ASCII letter ("e") instead of entering the dead-key/accent state ("´" →
"é"). The original mods (with `alt` set) are still sent to the encoder, which
ESC-prefixes (legacy) or sets the kitty `alt` param. So option-as-alt is a
*pre-`interpretKeyEvents` modifier filter* — it prevents the IME/dead-key path
from ever engaging, rather than undoing it after.

### 1.5 Dead-key state

Dead keys have no explicit state in Ghostty's own code — the state lives in
macOS's input context (`interpretKeyEvents`). Option-e emits
`setMarkedText("´")` (preedit); the next key emits `insertText("é")` (commit) or
`setMarkedText` again. `flagsChanged` (:1405) short-circuits when
`hasMarkedText()` (:1417) so modifier presses don't disturb an in-progress
compose. This is why dead-key composition **cannot be fully unit-tested** without
a live input context — see §4.

### 1.6 The keycode → Key mapping happens in Zig, not Swift

`NSEvent+Extension.swift::ghosttyKeyEvent` (:12) passes the raw `event.keyCode`
straight through as `key_ev.keycode`. libghostty resolves it in
`embedded.zig::KeyEvent.core` (:103) by scanning `input.keycodes.entries` for the
matching `.native` column, else `.unidentified`. Our `ghostty-input` crate does
**not** port that `keycodes.zig` native table, so a Rust AppKit host must supply
the physical `Key` itself. The spike's `keymap.rs` is that map (partial,
transcribed from the macOS column of `keycodes.zig`).

## 2. winit 0.30 macOS backend vs raw AppKit, per capability

Sources: winit 0.30 `WindowExtMacOS`
(<https://docs.rs/winit/0.30.5/winit/platform/macos/trait.WindowExtMacOS.html>),
winit `Window` IME + `HasWindowHandle`
(<https://docs.rs/winit/latest/winit/window/struct.Window.html>),
`raw-window-handle` `AppKitWindowHandle`
(<https://docs.rs/raw-window-handle/latest/raw_window_handle/struct.AppKitWindowHandle.html>),
and winit issues #3617, #2651, #3342, #1806.

| Capability                                  | winit 0.30 macOS backend                                                                                                                                                                                                                                                                                                                                             | Raw AppKit (`objc2`)                                                                                                                                                                                         | Verdict                                                                          |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| **Dead keys**                               | winit's `WinitView` implements `NSTextInputClient` and emits `Ime::Preedit`/`Ime::Commit`. Works for the common case, but **breaks with custom keyboard layouts + modifiers** (issue #2651: preedits/commits fire as if the modifier weren't pressed). You get winit's cooked `Ime` events, not the raw `setMarkedText`/`insertText` calls.                          | Full control: our own `setMarkedText`/`insertText`/`unmarkText`, exactly the upstream flow §1.1–1.5.                                                                                                         | **AppKit** — for the modifier-edge-case fidelity Ghostty needs.                  |
| **IME / marked text**                       | `Ime::Preedit(text, Option<(usize, usize)>)` gives preedit string + cursor range. **But** `selectedRange`/`replacementRange` from the IME are **ignored** (issue #3617) — breaks IMEs with nested inline composition buffers. macOS Character Viewer has known glitches (#3342). Enabled via `set_ime_allowed(true)`, positioned via `set_ime_cursor_area`.          | Full `NSTextInputClient`: we honor selected/replacement ranges, feed `ghostty_surface_preedit`, place the IME box via `firstRectForCharacterRange`, and serve QuickLook/dictation via `attributedSubstring`. | **AppKit** — winit's dropped ranges are a correctness gap for CJK.               |
| **`macos-option-as-alt`**                   | **Supported**: `set_option_as_alt(OptionAsAlt)` / `option_as_alt()` on `WindowExtMacOS`, with `None`/`Only{Left,Right}`/`Both` variants — a near 1:1 match to our `OptionAsAlt`.                                                                                                                                                                                     | Full control via `Mods.translation` before `interpretKeyEvents` (§1.4).                                                                                                                                      | **Tie** — winit covers this one natively.                                        |
| **`performKeyEquivalent` access**           | **Not exposed.** winit owns the view and the responder chain; there's no hook to intercept key-equivalents or run the binding→menu→re-inject dance (§1.3). Cmd-key routing would have to be reconstructed from winit's cooked `KeyEvent`s, losing the "is this bound at the system level?" signal.                                                                   | Native override — the whole §1.3 flow is ours.                                                                                                                                                               | **AppKit** — decisive for correct Cmd/Ctrl + native menu integration.            |
| **CALayer hosting (IOSurface)**             | Indirect: `Window: HasWindowHandle` → `AppKitWindowHandle { ns_view: NonNull<c_void> }`. From `ns_view` you get `view.layer()` and can assign the IOSurface (our `IOSurfaceLayer`, R2). Works, but you're reaching *through* winit's view, which winit also drives.                                                                                                  | Direct: our `NSView` owns its `CALayer`; `IOSurfaceLayer` (crates/ghostty-renderer) is assigned as `layer`/`contents` with no intermediary.                                                                  | **AppKit** (clean), winit (workable). Not decisive alone.                        |
| **Native tabbing (`NSWindow.tabbingMode`)** | Partial: `set_tabbing_identifier`/`select_next_tab`/`num_tabs` etc. drive macOS *automatic* window tabbing by identifier. But no `NSWindow` handle (`AppKitWindowHandle` has **no `ns_window`** — you must go `ns_view.window()`), and no control over `tabbingMode`, per-tab `NSWindow` subclassing, or the menu wiring R5 wants ("each tab = its own Engine+PTY"). | Full `NSWindow` + `NSWindowTabbingMode` + custom `NSToolbar`/`NSMenu`; each tab is a real `NSWindow` we own. This is exactly the R5 "cheap native-shell wins" line item.                                     | **AppKit** — winit's identifier-only tabbing can't host per-tab engines cleanly. |
| **Native menu bar / Cmd-N/T/W**             | Not part of winit. Would need a separate `objc2` `NSMenu` + `AppDelegate` alongside winit's app, fighting winit for `NSApplication` ownership.                                                                                                                                                                                                                       | Native `NSMenu` + `AppDelegate`, same object graph as everything else.                                                                                                                                       | **AppKit**.                                                                      |
| **Event-loop / threading model**            | winit owns `NSApplication.run` and the run loop; our render thread + PTY channels must fit winit's `ApplicationHandler`.                                                                                                                                                                                                                                             | We own `NSApplication`; R5's "Thread.zig-lite (plain thread+channel)" sits naturally beside it.                                                                                                              | **AppKit** — matches the R5 plan's threading shape.                              |

## 3. Verification matrix results

Run: `cargo test --manifest-path spikes/appkit-input/Cargo.toml` and
`cargo run --manifest-path spikes/appkit-input/Cargo.toml` (headless matrix).

Encoder note: `ghostty_input::key_encode::encode` dispatches to the **kitty**
encoder when any kitty flag is set, and to a **legacy stub** otherwise. The
legacy stub is a *narrow placeholder* (crates/ghostty-input/src/key_encode.rs:583):
it handles special/navigation keys and ctrl-combos but does **not** echo plain
printable text nor apply alt/ESC-prefix. The full legacy encoder is an unported
later chunk. The spike therefore exercises the **disambiguate-only kitty** path
(plain keys pass through as their literal byte) as the realistic default, and
also asserts the legacy ctrl path where the stub is complete.

| Case                                             | Config             | Encoded                                                        | Verified how                                                                                                           | Status                                                   |
| ------------------------------------------------ | ------------------ | -------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| plain `a`                                        | kitty disambiguate | `a` (`[0x61]`)                                                 | pure + real NSView seam                                                                                                | **PASS (programmatic)**                                  |
| shift-`A`                                        | kitty disambiguate | `A` (shift consumed for text)                                  | pure                                                                                                                   | **PASS (programmatic)**                                  |
| ctrl-`c`                                         | kitty disambiguate | `ESC[99;5u`                                                    | pure                                                                                                                   | **PASS (programmatic)**                                  |
| ctrl-`c`                                         | legacy             | `0x03`                                                         | pure + real NSView seam                                                                                                | **PASS (programmatic)**                                  |
| option-`e`, option-as-alt=false                  | kitty disambiguate | composed text passthrough (`é`)                                | pure (composition *sequencing* via preedit tests + NSView seam)                                                        | **PASS (programmatic; live compose is manual)**          |
| option-`e`, option-as-alt=true                   | kitty disambiguate | `ESC[101;3u` (alt+e)                                           | pure + implied by option_is_alt test                                                                                   | **PASS (programmatic)**                                  |
| option-as-alt Left/Right side discrimination     | kitty              | n/a (mod decision)                                             | pure (`option_is_alt`)                                                                                                 | **PASS (programmatic)**                                  |
| cmd-`v`                                          | any                | *nothing* (menu/binding territory)                             | pure + real NSView seam                                                                                                | **PASS (programmatic)**                                  |
| dead-key start → commit (`option-e` → `e` ⇒ `é`) | kitty              | preedit `´` then committed text `é`, no PTY bytes for the keys | **real NSTextInputClient NSView** via synthetic keyDown seam (`setMarkedText`/`insertText`/`unmarkText` state machine) | **PASS (state machine); live IME composition is manual** |
| IME mark → unmark (Korean jamo)                  | kitty              | preedit set then cleared, no bytes                             | real NSView seam                                                                                                       | **PASS (state machine)**                                 |

The macOS layer registers the **real** `objc2` `NSTextInputClient` `NSView`
subclass (`GhosttySpikeInputView`) and invokes its actual method bodies + ivar
state through the `handle_key_down_raw` seam. So the objc2 `define_class!` +
`NSTextInputClient` conformance + preedit plumbing are all proven to compile,
register, and execute.

### 4. What remains manual (needs a real event loop / input context / user)

These cannot be driven headlessly because they require macOS's live input
context (`interpretKeyEvents` calling back through the system IME), an on-screen
window in the responder chain, or a human:

1. **Live dead-key composition** — a *real* `interpretKeyEvents([option-e])`
   that makes macOS emit `setMarkedText("´")` then `insertText("é")` on the next
   key. The spike proves the *handler* state machine end-to-end with synthetic
   `setMarkedText`/`insertText` calls (which is what the OS would call), but the
   OS→handler edge itself needs a GUI session. Synthetic `NSEvent`s posted
   through `keyDown:` do **not** reliably drive `interpretKeyEvents` without a
   key window + input context.
2. **Real IME candidate windows** (Japanese/Chinese/Korean multi-key compose,
   candidate selection) — needs the actual input method + user selection.
3. **`performKeyEquivalent` menu routing** — needs a live `NSApplication`, key
   window, and `NSMenu`; the timestamp round-trip (§1.3) only exercises against
   real AppKit event dispatch. The spike does not build a window, so this is
   documented, not tested. R5 implements it.
4. **Right-side modifier device bits** (`NX_DEVICER*KEYMASK`) — objc2's safe
   `NSEvent` surface doesn't expose them; the spike leaves them Left and covers
   Left/Right *logic* via the pure `option_is_alt` test. R5 reads the raw
   `modifierFlags().rawValue` bits like upstream `ghosttyMods`.

## 5. Recommendation (PROPOSED)

**Use raw AppKit (`objc2`) for the R5 window host, not winit 0.30.**
Confidence: **high** for the host choice; medium on effort estimate.

### Decisive factors (from §2)

1. **`performKeyEquivalent` is not exposed by winit**, and it is non-optional for
   correct Cmd/Ctrl encoding + native menu key-equivalents (§1.3). This alone is
   close to disqualifying for winit.
2. **Native window tabbing with per-tab engines** is an explicit R5 deliverable
   ("each tab = its own Engine+PTY", m3 plan R5 row). winit exposes only
   identifier-based *automatic* tabbing and hides the `NSWindow`, so it cannot
   host per-tab engines the way R5 wants.
3. **Native `NSMenu` + `AppDelegate` + Cmd-N/T/W** (another R5 line item) want to
   own `NSApplication`; winit owns it instead.
4. **IME correctness edges**: winit drops the IME selected/replacement ranges
   (#3617) and mishandles dead keys under custom layouts + modifiers (#2651) —
   both are real fidelity gaps versus Ghostty's full `NSTextInputClient`.
5. The renderer already presents via an **IOSurface `CALayer`** we own
   (crates/ghostty-renderer `IOSurfaceLayer`); an AppKit `NSView` hosts it with
   no intermediary. winit would have us reach through its view for the same
   layer.

winit's genuine wins — `set_option_as_alt` and cross-platform portability — do
not outweigh the above **for a macOS-first native terminal**. Ghostty upstream is
itself a raw-AppKit app for exactly these reasons, and our port already commits
to the objc2 stack (renderer, fonts). Adopting winit would mean re-deriving the
§1 flow through a thinner, lossier abstraction and then fighting it for
`NSApplication`/`NSWindow`/menu ownership.

### Input behaviors R5 must implement, and their proof status

Proven here (encoder + handler plumbing):

- keycode → `Key` mapping (extend `keymap.rs` to the full `keycodes.zig` table,
  or port that table into `ghostty-input`). **[partial map proven]**
- NSEvent mods → `Mods` incl. option-as-alt filtering (`translation`). **[proven]**
- `KeyEvent` construction + `key_encode::encode` dispatch (kitty + legacy). **[proven]**
- Preedit state machine (`setMarkedText`/`unmarkText`/`insertText`). **[proven]**
- The `objc2` `NSView` + `NSTextInputClient` conformance itself. **[proven: compiles, registers, executes]**

R5 must still build (NOT proven here — need a live window/app):

- The real `keyDown` → `interpretKeyEvents` bridge (§1.1) in a live window.
- `performKeyEquivalent` + `doCommandBySelector` binding/menu round-trip (§1.3).
- `flagsChanged` for modifier press/release with side detection + the raw
  `NX_DEVICER*` bits (§1.5, §4.4).
- `firstRectForCharacterRange` IME box placement + `attributedSubstring` for
  QuickLook/dictation, fed from real terminal geometry.
- **A dependency to resolve first:** the **legacy key encoder in `ghostty-input`
  is still a stub** (§3). A production terminal must handle the legacy (non-kitty)
  path fully — that ported chunk is a prerequisite for R5 input parity, or R5
  ships only correct under kitty-protocol apps.
- Native `NSWindow` tabbing (`tabbingMode`) + `NSMenu` + Cmd-N/T/W, per the R5
  plan.
