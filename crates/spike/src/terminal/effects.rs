use crate::{
    osc::{OscAction, parse_osc},
    terminal::Terminal,
};

impl Terminal {
    pub(super) fn finish_osc(&mut self) {
        match parse_osc(&self.osc) {
            Some(OscAction::Title(title)) => self.title = Some(title),
            Some(OscAction::Clipboard(text)) => self.clipboard.push(text),
            None => {
                // TODO(port): OSC palette, hyperlinks, shell integration,
                // and Kitty protocols are outside this PoC.
            }
        }
        self.osc.clear();
    }
}
