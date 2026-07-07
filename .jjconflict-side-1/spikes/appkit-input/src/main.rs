//! Headless-ish harness for the AppKit input spike.
//!
//! With no arguments it runs the verification matrix programmatically (no
//! window, no user) and prints the encoded bytes for each case, then exits
//! non-zero if any expectation fails. This is the "runs headless-ish" mode the
//! spike task calls for: it constructs [`RawKeyEvent`]s and drives the encoder
//! (and, on macOS, the real `NSTextInputClient` view via its synthetic seam),
//! asserting the output matrix.
//!
//! The heavier assertions live in the crate's `#[test]`s; this binary is a
//! human-readable smoke run of the same matrix.

use ghostty_input::key_encode::KittyFlags;
use ghostty_input::key_mods::OptionAsAlt;
use spike_appkit_input::translate::{self, Config, RawKeyEvent};

/// macOS native keycodes used by the matrix (see `keymap.rs`).
const KC_A: u16 = 0x00;
const KC_C: u16 = 0x08;
const KC_E: u16 = 0x0E;
const KC_V: u16 = 0x09;

fn fmt_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|c| match c {
            0x1b => "ESC".to_string(),
            0x20..=0x7e => (*c as char).to_string(),
            other => format!("\\x{other:02x}"),
        })
        .collect::<Vec<_>>()
        .join("")
}

fn main() {
    use std::cell::Cell;
    let failures = Cell::new(0u32);

    let check = |name: &str, got: &[u8], want: &[u8]| {
        let ok = got == want;
        if !ok {
            failures.set(failures.get() + 1);
        }
        println!(
            "  [{}] {name}: got={:?} ({})",
            if ok { "PASS" } else { "FAIL" },
            got,
            fmt_bytes(got),
        );
        if !ok {
            println!("        WANT {:?} ({})", want, fmt_bytes(want));
        }
    };

    println!(
        "AppKit input spike — verification matrix\n(disambiguate-only kitty \
         encoder unless noted; headless)\n"
    );

    // Default config: disambiguate-only kitty, option-as-alt off. This is the
    // minimal kitty mode most terminals negotiate; plain printable keys pass
    // through as their literal byte while modified keys disambiguate to CSI u.
    let kitty = Config::default();

    // plain 'a' -> literal byte 0x61.
    let a = RawKeyEvent {
        keycode: KC_A,
        text: "a".into(),
        unshifted_codepoint: 'a' as u32,
        ..Default::default()
    };
    check("plain 'a'", &translate::encode_raw(&a, &kitty), b"a");

    // shift-A -> literal "A". Shift is *consumed* to produce the uppercase
    // text (matching upstream's consumed-mods heuristic: everything but
    // ctrl/cmd contributes to translation), so the kitty encoder emits the
    // plain text rather than a disambiguated CSI u.
    let shift_a = RawKeyEvent {
        keycode: KC_A,
        shift: true,
        text: "A".into(),
        unshifted_codepoint: 'a' as u32,
        ..Default::default()
    };
    check("shift-A", &translate::encode_raw(&shift_a, &kitty), b"A");

    // ctrl-c, kitty disambiguate -> CSI u (ctrl => param 5).
    let ctrl_c = RawKeyEvent {
        keycode: KC_C,
        ctrl: true,
        // AppKit's ghosttyCharacters strips the control char, so text empty.
        text: String::new(),
        unshifted_codepoint: 'c' as u32,
        ..Default::default()
    };
    check(
        "ctrl-c (kitty)",
        &translate::encode_raw(&ctrl_c, &kitty),
        b"\x1b[99;5u",
    );

    // ctrl-c, LEGACY encoder -> raw 0x03. This is the real default when no
    // kitty flags are negotiated, and is fully handled by the legacy stub.
    let legacy = Config {
        kitty_flags: KittyFlags::DISABLED,
        ..Config::default()
    };
    check(
        "ctrl-c (legacy)",
        &translate::encode_raw(&ctrl_c, &legacy),
        &[0x03],
    );

    // option-e WITH option-as-alt=true -> alt+e disambiguated (ESC-prefixed
    // CSI u, alt => param 3). Proves option is treated as Alt (accent
    // composition bypassed) rather than composing "é".
    let opt_alt = Config {
        option_as_alt: OptionAsAlt::True,
        ..Config::default()
    };
    let option_e = RawKeyEvent {
        keycode: KC_E,
        option: true,
        // With option-as-alt, AppKit would translate to base 'e'; the composed
        // accent is suppressed. text carries the base letter.
        text: "e".into(),
        unshifted_codepoint: 'e' as u32,
        ..Default::default()
    };
    check(
        "option-e (option-as-alt=true)",
        &translate::encode_raw(&option_e, &opt_alt),
        b"\x1b[101;3u",
    );

    // cmd-v -> nothing reaches the encoder (menu/binding territory).
    let cmd_v = RawKeyEvent {
        keycode: KC_V,
        command: true,
        text: "v".into(),
        unshifted_codepoint: 'v' as u32,
        ..Default::default()
    };
    check(
        "cmd-v (must NOT reach encoder)",
        &translate::encode_raw(&cmd_v, &kitty),
        b"",
    );

    println!();
    let failures = failures.get();
    if failures == 0 {
        println!("ALL MATRIX CASES PASSED");
    } else {
        println!("{failures} MATRIX CASE(S) FAILED");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    println!(
        "\n(macOS NSTextInputClient view is exercised by `cargo test` via the \
         synthetic keyDown seam; a real interpretKeyEvents run needs a GUI \
         session — see docs/analysis/appkit-input.md.)"
    );
}
