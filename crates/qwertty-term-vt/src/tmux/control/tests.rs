//! Ported inline tests from `tmux/control.zig`, plus edge cases for the
//! idle/broken/overflow paths the Zig tests exercise implicitly.

use super::*;

/// Feed every byte of `s`, asserting each yields no notification.
#[track_caller]
fn put_all_none(p: &mut ControlParser, s: &str) {
    for &b in s.as_bytes() {
        assert_eq!(p.put(b), Ok(None), "unexpected notification at byte {b:#x}");
    }
}

/// Feed a trailing newline and return the notification it completes.
#[track_caller]
fn put_newline(p: &mut ControlParser) -> Notification {
    p.put(b'\n').unwrap().expect("expected a notification")
}

#[test]
fn begin_end_empty() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1578922740 269 1\n");
    put_all_none(&mut c, "%end 1578922740 269 1");
    assert_eq!(put_newline(&mut c), Notification::BlockEnd(b"".to_vec()));
}

#[test]
fn begin_error_empty() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1578922740 269 1\n");
    put_all_none(&mut c, "%error 1578922740 269 1");
    assert_eq!(put_newline(&mut c), Notification::BlockErr(b"".to_vec()));
}

#[test]
fn begin_end_data() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1578922740 269 1\n");
    put_all_none(&mut c, "hello\nworld\n");
    put_all_none(&mut c, "%end 1578922740 269 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockEnd(b"hello\nworld".to_vec())
    );
}

#[test]
fn block_payload_may_start_with_end() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1 1 1\n");
    put_all_none(&mut c, "%end not really\n");
    put_all_none(&mut c, "hello\n");
    put_all_none(&mut c, "%end 1 1 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockEnd(b"%end not really\nhello".to_vec())
    );
}

#[test]
fn block_payload_may_start_with_error() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1 1 1\n");
    put_all_none(&mut c, "%error not really\n");
    put_all_none(&mut c, "hello\n");
    put_all_none(&mut c, "%end 1 1 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockEnd(b"%error not really\nhello".to_vec())
    );
}

#[test]
fn block_terminates_with_real_error_after_misleading_payload() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1 1 1\n");
    put_all_none(&mut c, "%error not really\n");
    put_all_none(&mut c, "hello\n");
    put_all_none(&mut c, "%error 1 1 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockErr(b"%error not really\nhello".to_vec())
    );
}

#[test]
fn block_terminator_requires_exact_token_count() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1 1 1\n");
    put_all_none(&mut c, "%end 1 1 1 trailing\n");
    put_all_none(&mut c, "hello\n");
    put_all_none(&mut c, "%end 1 1 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockEnd(b"%end 1 1 1 trailing\nhello".to_vec())
    );
}

#[test]
fn block_terminator_requires_numeric_metadata() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%begin 1 1 1\n");
    put_all_none(&mut c, "%end foo bar baz\n");
    put_all_none(&mut c, "hello\n");
    put_all_none(&mut c, "%end 1 1 1");
    assert_eq!(
        put_newline(&mut c),
        Notification::BlockEnd(b"%end foo bar baz\nhello".to_vec())
    );
}

#[test]
fn output() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%output %42 foo bar baz");
    assert_eq!(
        put_newline(&mut c),
        Notification::Output {
            pane_id: 42,
            data: b"foo bar baz".to_vec()
        }
    );
}

#[test]
fn session_changed() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%session-changed $42 foo");
    assert_eq!(
        put_newline(&mut c),
        Notification::SessionChanged {
            id: 42,
            name: b"foo".to_vec()
        }
    );
}

#[test]
fn sessions_changed() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%sessions-changed");
    assert_eq!(put_newline(&mut c), Notification::SessionsChanged);
}

#[test]
fn sessions_changed_carriage_return() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%sessions-changed\r");
    assert_eq!(put_newline(&mut c), Notification::SessionsChanged);
}

#[test]
fn layout_change() {
    let mut c = ControlParser::new();
    put_all_none(
        &mut c,
        "%layout-change @2 1234x791,0,0{617x791,0,0,0,617x791,618,0,1} \
         1234x791,0,0{617x791,0,0,0,617x791,618,0,1} *-",
    );
    assert_eq!(
        put_newline(&mut c),
        Notification::LayoutChange {
            window_id: 2,
            layout: b"1234x791,0,0{617x791,0,0,0,617x791,618,0,1}".to_vec(),
            visible_layout: b"1234x791,0,0{617x791,0,0,0,617x791,618,0,1}".to_vec(),
            raw_flags: b"*-".to_vec(),
        }
    );
}

#[test]
fn window_add() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%window-add @14");
    assert_eq!(put_newline(&mut c), Notification::WindowAdd { id: 14 });
}

#[test]
fn window_renamed() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%window-renamed @42 bar");
    assert_eq!(
        put_newline(&mut c),
        Notification::WindowRenamed {
            id: 42,
            name: b"bar".to_vec()
        }
    );
}

#[test]
fn window_pane_changed() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%window-pane-changed @42 %2");
    assert_eq!(
        put_newline(&mut c),
        Notification::WindowPaneChanged {
            window_id: 42,
            pane_id: 2
        }
    );
}

#[test]
fn client_detached() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%client-detached /dev/pts/1");
    assert_eq!(
        put_newline(&mut c),
        Notification::ClientDetached {
            client: b"/dev/pts/1".to_vec()
        }
    );
}

#[test]
fn client_session_changed() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%client-session-changed /dev/pts/1 $2 mysession");
    assert_eq!(
        put_newline(&mut c),
        Notification::ClientSessionChanged {
            client: b"/dev/pts/1".to_vec(),
            session_id: 2,
            name: b"mysession".to_vec(),
        }
    );
}

// ---- edge cases the Zig tests exercise via the state machine --------------

#[test]
fn idle_non_percent_breaks_and_exits() {
    // Control-mode output must be wrapped in notifications; a bare byte in idle
    // means we've lost sync -> exit + broken.
    let mut c = ControlParser::new();
    assert_eq!(c.put(b'x'), Ok(Some(Notification::Exit)));
    // Broken: all further input is dropped silently.
    assert_eq!(c.put(b'%'), Ok(None));
    assert_eq!(c.put(b'\n'), Ok(None));
}

#[test]
fn unknown_notification_is_dropped_and_parsing_continues() {
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%totally-unknown 1 2 3");
    assert_eq!(c.put(b'\n'), Ok(None)); // unknown -> None, back to idle
    // A subsequent valid notification still parses.
    put_all_none(&mut c, "%window-add @7");
    assert_eq!(put_newline(&mut c), Notification::WindowAdd { id: 7 });
}

#[test]
fn malformed_known_command_is_dropped_but_not_broken() {
    let mut c = ControlParser::new();
    // %window-add with a non-numeric id doesn't match -> None, idle (not broken).
    put_all_none(&mut c, "%window-add @notanumber");
    assert_eq!(c.put(b'\n'), Ok(None));
    put_all_none(&mut c, "%window-add @9");
    assert_eq!(put_newline(&mut c), Notification::WindowAdd { id: 9 });
}

#[test]
fn output_requires_nonempty_data() {
    // `%output %42 ` (trailing space, empty data) fails `.+` -> no notification.
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%output %42 ");
    assert_eq!(c.put(b'\n'), Ok(None));
}

#[test]
fn overflow_returns_error_once_then_drops() {
    let mut c = ControlParser::with_max_bytes(8);
    // Start a notification and accumulate up to the limit.
    // The limit is checked at the *start* of put, against buffer length.
    let mut hit = false;
    for &b in b"%output %123456789" {
        match c.put(b) {
            Ok(_) => {}
            Err(BufferOverflow) => {
                hit = true;
                break;
            }
        }
    }
    assert!(
        hit,
        "expected a BufferOverflow once the budget was exceeded"
    );
    // Broken now: everything drops, no further error.
    assert_eq!(c.put(b'a'), Ok(None));
    assert_eq!(c.put(b'\n'), Ok(None));
}

#[test]
fn layout_change_empty_flags() {
    // raw_flags is `.*` and may be empty (line ends in a space).
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%layout-change @1 aaa bbb ");
    assert_eq!(
        put_newline(&mut c),
        Notification::LayoutChange {
            window_id: 1,
            layout: b"aaa".to_vec(),
            visible_layout: b"bbb".to_vec(),
            raw_flags: b"".to_vec(),
        }
    );
}

#[test]
fn carriage_return_stripped_before_parse() {
    // A trailing CR before the LF must not defeat an exact match.
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%window-add @3\r");
    assert_eq!(put_newline(&mut c), Notification::WindowAdd { id: 3 });
}

#[test]
fn output_octal_escapes_are_decoded() {
    // tmux escapes control/non-printable bytes in %output as `\ooo` (3 octal
    // digits): ESC=\033, LF=\012, CR=\015, backslash=\134, BEL=\007. The
    // decoded Output.data must carry the raw bytes so the pane terminal
    // interprets them (this was the "raw \033[1m on screen" bug).
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%output %1 \\033[1mhi\\033[0m\\134\\007");
    assert_eq!(
        put_newline(&mut c),
        Notification::Output {
            pane_id: 1,
            data: b"\x1b[1mhi\x1b[0m\\\x07".to_vec(),
        }
    );
}

#[test]
fn output_plain_ascii_passes_through() {
    // No escapes -> verbatim (regression guard for the common case).
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%output %42 foo bar baz");
    assert_eq!(
        put_newline(&mut c),
        Notification::Output {
            pane_id: 42,
            data: b"foo bar baz".to_vec(),
        }
    );
}

#[test]
fn output_lone_backslash_not_octal_is_literal() {
    // A backslash not followed by 3 octal digits stays literal (defensive).
    let mut c = ControlParser::new();
    put_all_none(&mut c, "%output %1 a\\9b\\12");
    assert_eq!(
        put_newline(&mut c),
        Notification::Output {
            pane_id: 1,
            data: b"a\\9b\\12".to_vec(),
        }
    );
}
