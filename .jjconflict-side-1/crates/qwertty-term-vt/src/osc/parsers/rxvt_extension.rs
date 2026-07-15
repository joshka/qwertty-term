//! OSC 777: rxvt extension (only `notify` is recognized). Port of
//! `osc/parsers/rxvt_extension.zig`.

use crate::osc::Command;

/// Parse OSC 777. Port of `rxvt_extension.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    let k = data.find(';')?;
    let ext = &data[..k];
    if ext != "notify" {
        return None;
    }
    let after_ext = &data[k + 1..];
    let t = after_ext.find(';')?;
    let title = &after_ext[..t];
    let body = &after_ext[t + 1..];
    Some(Command::ShowDesktopNotification {
        title: title.to_string(),
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    // Zig: rxvt_extension.zig "OSC: OSC 777 show desktop notification with title".
    #[test]
    fn osc_777_show_desktop_notification_with_title() {
        let mut p = osc::Parser::new();
        for c in "777;notify;Title;Body".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(Some(0x1b)),
            Some(Command::ShowDesktopNotification {
                title: "Title".to_string(),
                body: "Body".to_string(),
            })
        );
    }
}
