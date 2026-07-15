//! Bell handling: which `bell-features` fire on a terminal BEL, as pure data
//! plus a config-string parser. The AppKit side effects (system beep, dock
//! attention request, per-tab title indicator) live in [`crate::app`]; this
//! module is the testable policy layer, mirroring [`crate::quickterm`].
//!
//! Port of upstream's `Config.BellFeatures` (`Config.zig:9049`): a packed
//! set of independent flags, defaulting to `attention` + `title` on. The
//! config value is a comma-separated flag list (`system`, `audio`,
//! `attention`, `title`, `border`), each optionally `no-`-prefixed to
//! disable, applied over the defaults; the bare strings `true`/`false`
//! enable/disable all.

/// The enabled bell features (upstream `BellFeatures`). Defaults: `attention`
/// and `title` on; `system`, `audio`, `border` off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BellFeatures {
    /// Ring the system alert sound (macOS `NSSound.beep()`).
    pub system: bool,
    /// Play the configured `bell-audio-path` file (deferred in slice 1;
    /// parsed for parity so a config carrying it round-trips).
    pub audio: bool,
    /// Request user attention — bounce the Dock icon
    /// (`NSApp.requestUserAttention`).
    pub attention: bool,
    /// Show a bell indicator in the tab/window title.
    pub title: bool,
    /// Flash a window border (deferred in slice 1 — a renderer concern;
    /// parsed for parity).
    pub border: bool,
}

impl Default for BellFeatures {
    /// Upstream `Config.zig:9049` field defaults.
    fn default() -> Self {
        BellFeatures {
            system: false,
            audio: false,
            attention: true,
            title: true,
            border: false,
        }
    }
}

impl BellFeatures {
    /// All features off.
    fn none() -> Self {
        BellFeatures {
            system: false,
            audio: false,
            attention: false,
            title: false,
            border: false,
        }
    }

    /// All features on.
    fn all() -> Self {
        BellFeatures {
            system: true,
            audio: true,
            attention: true,
            title: true,
            border: true,
        }
    }

    /// Set the flag named `name` to `on`. Unknown names are ignored (upstream
    /// logs and skips). Returns whether the name was recognized.
    fn set(&mut self, name: &str, on: bool) -> bool {
        match name {
            "system" => self.system = on,
            "audio" => self.audio = on,
            "attention" => self.attention = on,
            "title" => self.title = on,
            "border" => self.border = on,
            _ => return false,
        }
        true
    }

    /// Parse the `bell-features` config value. Starting from the upstream
    /// defaults, apply each comma-separated token: `name` enables, `no-name`
    /// disables. The bare `true`/`false` set all on/off. Whitespace around
    /// tokens is tolerated; unknown tokens are ignored. An empty string keeps
    /// the defaults.
    pub fn parse(s: &str) -> BellFeatures {
        let s = s.trim();
        if s.is_empty() {
            return BellFeatures::default();
        }
        if s.eq_ignore_ascii_case("true") {
            return BellFeatures::all();
        }
        if s.eq_ignore_ascii_case("false") {
            return BellFeatures::none();
        }
        let mut features = BellFeatures::default();
        for token in s.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            if let Some(name) = token.strip_prefix("no-") {
                features.set(name, false);
            } else {
                features.set(token, true);
            }
        }
        features
    }

    /// Whether any feature this slice acts on (`system`/`attention`/`title`)
    /// is enabled — a cheap gate so the pace tick can skip bell work entirely
    /// when nothing is configured.
    pub fn any_active(&self) -> bool {
        self.system || self.attention || self.title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_attention_and_title() {
        let d = BellFeatures::default();
        assert!(d.attention && d.title);
        assert!(!d.system && !d.audio && !d.border);
        assert!(d.any_active());
    }

    #[test]
    fn empty_keeps_defaults() {
        assert_eq!(BellFeatures::parse(""), BellFeatures::default());
        assert_eq!(BellFeatures::parse("   "), BellFeatures::default());
    }

    #[test]
    fn true_false_set_all() {
        assert_eq!(BellFeatures::parse("true"), BellFeatures::all());
        assert_eq!(BellFeatures::parse("false"), BellFeatures::none());
        assert!(!BellFeatures::parse("false").any_active());
    }

    #[test]
    fn tokens_toggle_over_defaults() {
        // Enable system on top of the defaults (attention+title stay on).
        let f = BellFeatures::parse("system");
        assert!(f.system && f.attention && f.title);
        // Disable title, enable system: attention still on.
        let f = BellFeatures::parse("system, no-title");
        assert!(f.system && f.attention && !f.title);
        // Disable everything this slice acts on → not active.
        let f = BellFeatures::parse("no-attention,no-title,no-system");
        assert!(!f.any_active());
    }

    #[test]
    fn unknown_tokens_ignored() {
        // An unknown token doesn't change or crash anything.
        assert_eq!(BellFeatures::parse("nonsense"), BellFeatures::default());
        let f = BellFeatures::parse("system,bogus,no-title");
        assert!(f.system && !f.title && f.attention);
    }

    #[test]
    fn audio_and_border_round_trip_even_though_slice1_defers_them() {
        let f = BellFeatures::parse("audio,border");
        assert!(f.audio && f.border);
        // any_active only gates the slice-1 features (system/attention/title),
        // so audio/border with the others disabled don't count as active.
        let only_deferred = BellFeatures::parse("no-attention,no-title,audio,border");
        assert!(only_deferred.audio && only_deferred.border);
        assert!(!only_deferred.any_active());
    }
}
