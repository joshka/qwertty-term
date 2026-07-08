//! OSC 9 (+ ConEmu 9;1-9;12 sub-extensions). Port of
//! `osc/parsers/osc9.zig`.

use crate::osc::{Command, ConemuChangeTabTitle, ProgressReport, ProgressState};

use super::semantic_prompt;

/// Parse OSC 9. Port of `osc9.zig` `parse`. `rest` is the body after the
/// `9` prefix, e.g. for `9;1;420` this is called with `";1;420"`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");

    if let Some(cmd) = try_conemu(data) {
        return Some(cmd);
    }

    // Not a recognized ConEmu shape: the entire body is an iTerm2-style
    // desktop notification with an empty title.
    Some(Command::ShowDesktopNotification {
        title: String::new(),
        body: data.to_string(),
    })
}

fn try_conemu(data: &str) -> Option<Command> {
    let bytes = data.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    match bytes[0] {
        b'1' => {
            if bytes.len() < 2 {
                return None;
            }
            match bytes[1] {
                // 9;1 sleep
                b';' => {
                    let duration_ms = data[2..].parse::<u16>().unwrap_or(100).min(10_000);
                    Some(Command::ConemuSleep { duration_ms })
                }
                // 9;10 xterm keyboard/output emulation
                b'0' => {
                    if bytes.len() == 2 {
                        return Some(Command::ConemuXtermEmulation {
                            keyboard: Some(true),
                            output: Some(true),
                        });
                    }
                    if bytes.len() < 4 || bytes[2] != b';' {
                        return None;
                    }
                    match bytes[3] {
                        b'0' => Some(Command::ConemuXtermEmulation {
                            keyboard: Some(false),
                            output: Some(false),
                        }),
                        b'1' => Some(Command::ConemuXtermEmulation {
                            keyboard: Some(true),
                            output: Some(true),
                        }),
                        b'2' => Some(Command::ConemuXtermEmulation {
                            keyboard: None,
                            output: Some(false),
                        }),
                        b'3' => Some(Command::ConemuXtermEmulation {
                            keyboard: None,
                            output: Some(true),
                        }),
                        _ => None,
                    }
                }
                // 9;11 comment
                b'1' => {
                    if bytes.len() < 3 || bytes[2] != b';' {
                        return None;
                    }
                    Some(Command::ConemuComment(data[3..].to_string()))
                }
                // 9;12 mark prompt start
                b'2' => Some(Command::SemanticPrompt(
                    semantic_prompt::fresh_line_new_prompt(),
                )),
                _ => None,
            }
        }
        // 9;2 show message box
        b'2' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            Some(Command::ConemuShowMessageBox(data[2..].to_string()))
        }
        // 9;3 change tab title
        b'3' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            if bytes.len() == 2 {
                return Some(Command::ConemuChangeTabTitle(ConemuChangeTabTitle::Reset));
            }
            Some(Command::ConemuChangeTabTitle(ConemuChangeTabTitle::Value(
                data[2..].to_string(),
            )))
        }
        // 9;4 progress report
        b'4' => {
            if bytes.len() < 2 || bytes[1] != b';' || bytes.len() < 3 {
                return None;
            }
            let state = match bytes[2] {
                b'0' => ProgressState::Remove,
                b'1' => ProgressState::Set,
                b'2' => ProgressState::Error,
                b'3' => ProgressState::Indeterminate,
                b'4' => ProgressState::Pause,
                _ => return None,
            };
            let progress = match state {
                ProgressState::Remove | ProgressState::Indeterminate => None,
                ProgressState::Set | ProgressState::Error | ProgressState::Pause => {
                    if bytes.len() < 4 || bytes[3] != b';' {
                        None
                    } else {
                        data[4..].parse::<u32>().ok().map(|v| v.clamp(0, 100) as u8)
                    }
                }
            };
            Some(Command::ConemuProgressReport(ProgressReport {
                state,
                progress,
            }))
        }
        // 9;5 wait for input
        b'5' => Some(Command::ConemuWaitInput),
        // 9;6 guimacro
        b'6' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            Some(Command::ConemuGuimacro(data[2..].to_string()))
        }
        // 9;7 run process
        b'7' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            Some(Command::ConemuRunProcess(data[2..].to_string()))
        }
        // 9;8 output environment variable
        b'8' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            Some(Command::ConemuOutputEnvironmentVariable(
                data[2..].to_string(),
            ))
        }
        // 9;9 current working directory
        b'9' => {
            if bytes.len() < 2 || bytes[1] != b';' {
                return None;
            }
            Some(Command::ReportPwd {
                value: data[2..].to_string(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> Option<Command> {
        let mut p = osc::Parser::new();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(Some(0x1b))
    }

    // Zig: "OSC 9: show desktop notification".
    #[test]
    fn show_desktop_notification() {
        assert_eq!(
            run("9;Hello world"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "Hello world".to_string()
            })
        );
    }

    // Zig: "OSC 9: show single character desktop notification".
    #[test]
    fn show_single_character_desktop_notification() {
        assert_eq!(
            run("9;H"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "H".to_string()
            })
        );
    }

    // Zig: "OSC 9;1: ConEmu sleep".
    #[test]
    fn conemu_sleep() {
        assert_eq!(
            run("9;1;420"),
            Some(Command::ConemuSleep { duration_ms: 420 })
        );
    }

    // Zig: "OSC 9;1: ConEmu sleep with no value default to 100ms".
    #[test]
    fn conemu_sleep_default() {
        assert_eq!(run("9;1;"), Some(Command::ConemuSleep { duration_ms: 100 }));
    }

    // Zig: "OSC 9;1: conemu sleep cannot exceed 10000ms".
    #[test]
    fn conemu_sleep_clamped() {
        assert_eq!(
            run("9;1;12345"),
            Some(Command::ConemuSleep { duration_ms: 10000 })
        );
    }

    // Zig: "OSC 9;1: conemu sleep invalid input".
    #[test]
    fn conemu_sleep_invalid_input() {
        assert_eq!(
            run("9;1;foo"),
            Some(Command::ConemuSleep { duration_ms: 100 })
        );
    }

    // Zig: "OSC 9;1: conemu sleep -> desktop notification 1".
    #[test]
    fn conemu_sleep_fallback_1() {
        assert_eq!(
            run("9;1"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "1".to_string()
            })
        );
    }

    // Zig: "OSC 9;1: conemu sleep -> desktop notification 2".
    #[test]
    fn conemu_sleep_fallback_2() {
        assert_eq!(
            run("9;1a"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "1a".to_string()
            })
        );
    }

    // Zig: "OSC 9;2: ConEmu message box".
    #[test]
    fn conemu_message_box() {
        assert_eq!(
            run("9;2;hello world"),
            Some(Command::ConemuShowMessageBox("hello world".to_string()))
        );
    }

    // Zig: "OSC 9;2: ConEmu message box invalid input".
    #[test]
    fn conemu_message_box_invalid_input() {
        assert_eq!(
            run("9;2"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "2".to_string()
            })
        );
    }

    // Zig: "OSC 9;2: ConEmu message box empty message".
    #[test]
    fn conemu_message_box_empty_message() {
        assert_eq!(
            run("9;2;"),
            Some(Command::ConemuShowMessageBox(String::new()))
        );
    }

    // Zig: "OSC 9;2: ConEmu message box spaces only message".
    #[test]
    fn conemu_message_box_spaces_only() {
        assert_eq!(
            run("9;2;   "),
            Some(Command::ConemuShowMessageBox("   ".to_string()))
        );
    }

    // Zig: "OSC 9;2: message box -> desktop notification 1".
    #[test]
    fn conemu_message_box_fallback_1() {
        assert_eq!(
            run("9;2"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "2".to_string()
            })
        );
    }

    // Zig: "OSC 9;2: message box -> desktop notification 2".
    #[test]
    fn conemu_message_box_fallback_2() {
        assert_eq!(
            run("9;2a"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "2a".to_string()
            })
        );
    }

    // Zig: "OSC 9;3: ConEmu change tab title".
    #[test]
    fn conemu_change_tab_title() {
        assert_eq!(
            run("9;3;foo bar"),
            Some(Command::ConemuChangeTabTitle(ConemuChangeTabTitle::Value(
                "foo bar".to_string()
            )))
        );
    }

    // Zig: "OSC 9;3: ConEmu change tab title reset".
    #[test]
    fn conemu_change_tab_title_reset() {
        assert_eq!(
            run("9;3;"),
            Some(Command::ConemuChangeTabTitle(ConemuChangeTabTitle::Reset))
        );
    }

    // Zig: "OSC 9;3: ConEmu change tab title spaces only".
    #[test]
    fn conemu_change_tab_title_spaces_only() {
        assert_eq!(
            run("9;3;   "),
            Some(Command::ConemuChangeTabTitle(ConemuChangeTabTitle::Value(
                "   ".to_string()
            )))
        );
    }

    // Zig: "OSC 9;3: change tab title -> desktop notification 1".
    #[test]
    fn conemu_change_tab_title_fallback_1() {
        assert_eq!(
            run("9;3"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "3".to_string()
            })
        );
    }

    // Zig: "OSC 9;3: message box -> desktop notification 2".
    #[test]
    fn conemu_change_tab_title_fallback_2() {
        assert_eq!(
            run("9;3a"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "3a".to_string()
            })
        );
    }

    // Zig: "OSC 9;4: ConEmu progress set".
    #[test]
    fn conemu_progress_set() {
        assert_eq!(
            run("9;4;1;100"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Set,
                progress: Some(100)
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress set overflow".
    #[test]
    fn conemu_progress_set_overflow() {
        assert_eq!(
            run("9;4;1;900"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Set,
                progress: Some(100)
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress set single digit".
    #[test]
    fn conemu_progress_set_single_digit() {
        assert_eq!(
            run("9;4;1;9"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Set,
                progress: Some(9)
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress set double digit".
    #[test]
    fn conemu_progress_set_double_digit() {
        assert_eq!(
            run("9;4;1;94"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Set,
                progress: Some(94)
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress set extra semicolon ignored".
    #[test]
    fn conemu_progress_set_extra_semicolon_ignored() {
        assert_eq!(
            run("9;4;1;100"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Set,
                progress: Some(100)
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress remove with no progress".
    #[test]
    fn conemu_progress_remove_no_progress() {
        assert_eq!(
            run("9;4;0;"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Remove,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress remove with double semicolon".
    #[test]
    fn conemu_progress_remove_double_semicolon() {
        assert_eq!(
            run("9;4;0;;"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Remove,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress remove ignores progress".
    #[test]
    fn conemu_progress_remove_ignores_progress() {
        assert_eq!(
            run("9;4;0;100"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Remove,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress remove extra semicolon".
    #[test]
    fn conemu_progress_remove_extra_semicolon() {
        assert_eq!(
            run("9;4;0;100;"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Remove,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress error".
    #[test]
    fn conemu_progress_error() {
        assert_eq!(
            run("9;4;2"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Error,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress error with progress".
    #[test]
    fn conemu_progress_error_with_progress() {
        assert_eq!(
            run("9;4;2;100"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Error,
                progress: Some(100)
            }))
        );
    }

    // Zig: "OSC 9;4: progress pause".
    #[test]
    fn conemu_progress_pause() {
        assert_eq!(
            run("9;4;4"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Pause,
                progress: None
            }))
        );
    }

    // Zig: "OSC 9;4: ConEmu progress pause with progress".
    #[test]
    fn conemu_progress_pause_with_progress() {
        assert_eq!(
            run("9;4;4;100"),
            Some(Command::ConemuProgressReport(ProgressReport {
                state: ProgressState::Pause,
                progress: Some(100)
            }))
        );
    }

    // Zig: "OSC 9;4: progress -> desktop notification 1".
    #[test]
    fn conemu_progress_fallback_1() {
        assert_eq!(
            run("9;4"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "4".to_string()
            })
        );
    }

    // Zig: "OSC 9;4: progress -> desktop notification 2".
    #[test]
    fn conemu_progress_fallback_2() {
        assert_eq!(
            run("9;4;"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "4;".to_string()
            })
        );
    }

    // Zig: "OSC 9;4: progress -> desktop notification 3".
    #[test]
    fn conemu_progress_fallback_3() {
        assert_eq!(
            run("9;4;5"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "4;5".to_string()
            })
        );
    }

    // Zig: "OSC 9;4: progress -> desktop notification 4".
    #[test]
    fn conemu_progress_fallback_4() {
        assert_eq!(
            run("9;4;5a"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "4;5a".to_string()
            })
        );
    }

    // Zig: "OSC 9;5: ConEmu wait input".
    #[test]
    fn conemu_wait_input() {
        assert_eq!(run("9;5"), Some(Command::ConemuWaitInput));
    }

    // Zig: "OSC 9;5: ConEmu wait ignores trailing characters".
    #[test]
    fn conemu_wait_ignores_trailing() {
        assert_eq!(run("9;5;foo"), Some(Command::ConemuWaitInput));
    }

    // Zig: "OSC 9;6: ConEmu guimacro 1".
    #[test]
    fn conemu_guimacro_1() {
        assert_eq!(run("9;6;a"), Some(Command::ConemuGuimacro("a".to_string())));
    }

    // Zig: "OSC: 9;6: ConEmu guimacro 2".
    #[test]
    fn conemu_guimacro_2() {
        assert_eq!(
            run("9;6;ab"),
            Some(Command::ConemuGuimacro("ab".to_string()))
        );
    }

    // Zig: "OSC: 9;6: ConEmu guimacro 3 incomplete -> desktop notification".
    #[test]
    fn conemu_guimacro_incomplete() {
        assert_eq!(
            run("9;6"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "6".to_string()
            })
        );
    }

    // Zig: "OSC: 9;7: ConEmu run process 1".
    #[test]
    fn conemu_run_process_1() {
        assert_eq!(
            run("9;7;ab"),
            Some(Command::ConemuRunProcess("ab".to_string()))
        );
    }

    // Zig: "OSC: 9;7: ConEmu run process 2".
    #[test]
    fn conemu_run_process_2() {
        assert_eq!(run("9;7;"), Some(Command::ConemuRunProcess(String::new())));
    }

    // Zig: "OSC: 9;7: ConEmu run process incomplete -> desktop notification".
    #[test]
    fn conemu_run_process_incomplete() {
        assert_eq!(
            run("9;7"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "7".to_string()
            })
        );
    }

    // Zig: "OSC: 9;8: ConEmu output environment variable 1".
    #[test]
    fn conemu_output_env_var_1() {
        assert_eq!(
            run("9;8;ab"),
            Some(Command::ConemuOutputEnvironmentVariable("ab".to_string()))
        );
    }

    // Zig: "OSC: 9;8: ConEmu output environment variable 2".
    #[test]
    fn conemu_output_env_var_2() {
        assert_eq!(
            run("9;8;"),
            Some(Command::ConemuOutputEnvironmentVariable(String::new()))
        );
    }

    // Zig: "OSC: 9;8: ConEmu output environment variable incomplete -> desktop notification".
    #[test]
    fn conemu_output_env_var_incomplete() {
        assert_eq!(
            run("9;8"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "8".to_string()
            })
        );
    }

    // Zig: "OSC: 9;9: ConEmu set current working directory".
    #[test]
    fn conemu_set_cwd() {
        assert_eq!(
            run("9;9;ab"),
            Some(Command::ReportPwd {
                value: "ab".to_string()
            })
        );
    }

    // Zig: "OSC: 9;9: ConEmu set current working directory incomplete -> desktop notification".
    #[test]
    fn conemu_set_cwd_incomplete() {
        assert_eq!(
            run("9;9"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "9".to_string()
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 1".
    #[test]
    fn conemu_xterm_emulation_1() {
        assert_eq!(
            run("9;10"),
            Some(Command::ConemuXtermEmulation {
                keyboard: Some(true),
                output: Some(true)
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 2".
    #[test]
    fn conemu_xterm_emulation_2() {
        assert_eq!(
            run("9;10;0"),
            Some(Command::ConemuXtermEmulation {
                keyboard: Some(false),
                output: Some(false)
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 3".
    #[test]
    fn conemu_xterm_emulation_3() {
        assert_eq!(
            run("9;10;1"),
            Some(Command::ConemuXtermEmulation {
                keyboard: Some(true),
                output: Some(true)
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 4".
    #[test]
    fn conemu_xterm_emulation_4() {
        assert_eq!(
            run("9;10;2"),
            Some(Command::ConemuXtermEmulation {
                keyboard: None,
                output: Some(false)
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 5".
    #[test]
    fn conemu_xterm_emulation_5() {
        assert_eq!(
            run("9;10;3"),
            Some(Command::ConemuXtermEmulation {
                keyboard: None,
                output: Some(true)
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 6".
    #[test]
    fn conemu_xterm_emulation_6() {
        assert_eq!(
            run("9;10;4"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "10;4".to_string()
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 7".
    #[test]
    fn conemu_xterm_emulation_7() {
        assert_eq!(
            run("9;10;"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "10;".to_string()
            })
        );
    }

    // Zig: "OSC: 9;10: ConEmu xterm keyboard and output emulation 8".
    #[test]
    fn conemu_xterm_emulation_8() {
        assert_eq!(
            run("9;10;abc"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "10;abc".to_string()
            })
        );
    }

    // Zig: "OSC: 9;11: ConEmu comment".
    #[test]
    fn conemu_comment() {
        assert_eq!(
            run("9;11;ab"),
            Some(Command::ConemuComment("ab".to_string()))
        );
    }

    // Zig: "OSC: 9;11: ConEmu comment incomplete -> desktop notification".
    #[test]
    fn conemu_comment_incomplete() {
        assert_eq!(
            run("9;11"),
            Some(Command::ShowDesktopNotification {
                title: String::new(),
                body: "11".to_string()
            })
        );
    }

    // Zig: "OSC: 9;12: ConEmu mark prompt start 1".
    #[test]
    fn conemu_mark_prompt_start_1() {
        assert!(matches!(run("9;12"), Some(Command::SemanticPrompt(_))));
    }

    // Zig: "OSC: 9;12: ConEmu mark prompt start 2".
    #[test]
    fn conemu_mark_prompt_start_2() {
        assert!(matches!(run("9;12;abc"), Some(Command::SemanticPrompt(_))));
    }
}
