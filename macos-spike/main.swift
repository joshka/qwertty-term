// macOS Swift driver for the qwertty-term FFI spike.
//
// This is the Swift side of the Rust -> C-ABI -> Swift round-trip. It links the
// `libqwertty_term_ffi.a` staticlib and drives the C ABI declared in
// `crates/qwertty-term-ffi/include/qwertty_term.h` (imported as the `CQwerttyTerm`
// clang module; see module.modulemap + build.sh). It deliberately mirrors how
// upstream's `macos/Sources/Ghostty/*.swift` would call the app/surface API, so
// the ergonomics we hit here are the ones the real adaptation will hit.
//
// Flow:
//   1. qwertty_term_init
//   2. app_new with a runtime config carrying a write-clipboard callback
//   3. surface_new (80x24)
//   4. write "hi\r\n" PTY bytes  (a shell's `printf 'hi\n'`)
//   5. send a key event ("x")   (a typed key)
//   6. read the screen text back (the pre-M3 draw stand-in)
//   7. write an OSC 52 sequence  -> the clipboard callback fires
//   8. free surface + app
//
// It prints PASS/FAIL lines and exits non-zero on any failure so build.sh can
// gate on it.

import CQwerttyTerm
import Foundation

// --- Callback plumbing -------------------------------------------------------
//
// A C function pointer callback cannot capture Swift state, so the clipboard
// result is stashed in a global the (top-level, @convention(c)) callback writes
// to. This is exactly the shape upstream uses: the callback trampolines through
// `userdata` back into a Swift object. Here the apartment is single-threaded so
// a plain global is sound.

var clipboardFired = false
var clipboardKind: QwerttyTermClipboard = QWERTTY_TERM_CLIPBOARD_STANDARD
var clipboardData = ""

let writeClipboard: @convention(c) (
    UnsafeMutableRawPointer?, QwerttyTermClipboard, UnsafePointer<CChar>?
) -> Void = { _userdata, kind, data in
    clipboardFired = true
    clipboardKind = kind
    if let data = data {
        clipboardData = String(cString: data)
    }
}

// --- Test harness ------------------------------------------------------------

var failures = 0
func check(_ cond: Bool, _ label: String) {
    if cond {
        print("PASS: \(label)")
    } else {
        print("FAIL: \(label)")
        failures += 1
    }
}

// 1. init
check(qwertty_term_init() == QWERTTY_TERM_RESULT_SUCCESS, "qwertty_term_init")

// 2. app_new
var runtime = QwerttyTermRuntimeConfig(
    userdata: nil,
    wakeup_cb: nil,
    write_clipboard_cb: writeClipboard
)
let app = qwertty_term_app_new(&runtime)
check(app != nil, "qwertty_term_app_new returns non-null")

// 3. surface_new
var surfaceConfig = QwerttyTermSurfaceConfig(cols: 80, rows: 24, max_scrollback: 1000)
let surface = qwertty_term_surface_new(app, &surfaceConfig)
check(surface != nil, "qwertty_term_surface_new returns non-null")

// 4. write PTY bytes: "hi\r\n"
let ptyBytes = Array("hi\r\n".utf8)
let writeRc = ptyBytes.withUnsafeBufferPointer {
    qwertty_term_surface_write_pty_bytes(surface, $0.baseAddress, $0.count)
}
check(writeRc == QWERTTY_TERM_RESULT_SUCCESS, "surface_write_pty_bytes")

// 5. send a key event: typed "x"
let keyText = "x"
keyText.withCString { textPtr in
    let event = QwerttyTermInputKey(
        action: QWERTTY_TERM_INPUT_ACTION_PRESS,
        mods: QwerttyTermInputMods(bits: 0),
        text: textPtr,
        unshifted_codepoint: UInt32(UnicodeScalar("x").value),
        composing: false
    )
    let keyRc = qwertty_term_surface_key(surface, event)
    check(keyRc == QWERTTY_TERM_RESULT_SUCCESS, "surface_key")
}

// 6. read screen text back (two-call buffer convention)
func readScreenText() -> String? {
    var needed: Int = 0
    let sizeRc = qwertty_term_surface_read_text(surface, nil, 0, &needed)
    guard sizeRc == QWERTTY_TERM_RESULT_SUCCESS else { return nil }
    var buf = [CChar](repeating: 0, count: needed + 1)
    var written: Int = 0
    let rc = buf.withUnsafeMutableBufferPointer {
        qwertty_term_surface_read_text(surface, $0.baseAddress, $0.count, &written)
    }
    guard rc == QWERTTY_TERM_RESULT_SUCCESS else { return nil }
    return String(cString: buf)
}

let screen = readScreenText()
check(screen != nil, "surface_read_text succeeds")
if let screen = screen {
    let firstLine = screen.split(separator: "\n", omittingEmptySubsequences: false).first.map(String.init) ?? ""
    // "hi" from the PTY write, then "x" from the key press on the next line
    // (the "\r\n" moved the cursor down).
    print("      screen first line = \(firstLine.debugDescription)")
    print("      full screen = \(screen.debugDescription)")
    check(screen.contains("hi"), "screen contains PTY text 'hi'")
    check(screen.contains("x"), "screen contains typed key 'x'")
}

// 7. OSC 52 write -> clipboard callback fires. base64("hi") == "aGk=".
let osc = Array("\u{1b}]52;c;aGk=\u{07}".utf8)
let oscRc = osc.withUnsafeBufferPointer {
    qwertty_term_surface_write_pty_bytes(surface, $0.baseAddress, $0.count)
}
check(oscRc == QWERTTY_TERM_RESULT_SUCCESS, "OSC 52 write accepted")
check(clipboardFired, "clipboard callback fired")
check(clipboardKind == QWERTTY_TERM_CLIPBOARD_STANDARD, "clipboard kind is STANDARD")
check(clipboardData == "aGk=", "clipboard data is raw base64 'aGk='")
print("      clipboard data = \(clipboardData.debugDescription)")

// 8. teardown (order matters: surface before app)
qwertty_term_surface_free(surface)
qwertty_term_app_free(app)
check(true, "teardown (surface_free + app_free)")

// Result
print(failures == 0 ? "\nALL PASS" : "\n\(failures) FAILURE(S)")
exit(failures == 0 ? 0 : 1)
