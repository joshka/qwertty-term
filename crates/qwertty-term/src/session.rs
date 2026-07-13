//! Window-session serialization for `window-save-state` content restore.
//!
//! A window's restorable content is its split tree with each pane's working
//! directory (cwd). This module is the pure, serde-serializable model plus the
//! encode/decode round-trip — mirroring upstream, whose restoration state is a
//! plain `Codable` unit-tested independently of the OS quit/relaunch cycle
//! (`macos/Tests/Terminal/TerminalRestorableTests.swift`).
//!
//! Slice 1 (#176) gated macOS native restoration on `window-save-state`; this is
//! slice 2's core. The app captures a live tab into a [`WindowSession`], JSON is
//! the wire form, and the tree is rebuilt on restore. Wiring this into macOS's
//! `NSWindowRestoration` (`restorationClass` + `NSCoder`) is the remaining
//! slice-2b step — that path is only exercised by a genuine relaunch, so this
//! serializable core is what's unit- and smoke-tested.
//!
//! Deviation from upstream (documented): the tree carries only structure +
//! per-pane cwd (title/uuid/zoom/fullscreen/tab-color are not yet modeled), and
//! the wire form is JSON rather than an `NSKeyedArchiver` plist — the app owns
//! the bytes it hands the OS coder, so the archive format is our choice.

use serde::{Deserialize, Serialize};

/// The split axis of a session node (mirrors [`crate::splits::Axis`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAxis {
    /// Panes are side by side (a vertical divider).
    Horizontal,
    /// Panes are stacked (a horizontal divider).
    Vertical,
}

/// One node of a window's restorable split tree: either a leaf pane (with its
/// working directory) or a split of two child nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionNode {
    /// A single pane. `cwd` is its working directory (from OSC 7), or `None` if
    /// the shell never reported one.
    Leaf { cwd: Option<String> },
    /// A division into a `first` (left/top) and `second` (right/bottom) child;
    /// `ratio` is the first child's fraction, `[0.1, 0.9]`.
    Split {
        axis: SessionAxis,
        ratio: f64,
        first: Box<SessionNode>,
        second: Box<SessionNode>,
    },
}

impl SessionNode {
    /// Number of leaf panes in this subtree.
    pub fn leaf_count(&self) -> usize {
        match self {
            SessionNode::Leaf { .. } => 1,
            SessionNode::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }
}

/// A restorable window: its split tree. (Frame/position ride on AppKit's own
/// window restoration, as upstream; only content is modeled here.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowSession {
    /// Bumped when the wire shape changes; a mismatched version is rejected on
    /// decode so an old archive can't be misread (upstream gates the same way).
    pub version: u32,
    pub tree: SessionNode,
}

/// The current session wire-format version.
pub const SESSION_VERSION: u32 = 1;

impl WindowSession {
    /// Wrap a captured tree at the current version.
    pub fn new(tree: SessionNode) -> Self {
        WindowSession {
            version: SESSION_VERSION,
            tree,
        }
    }

    /// Serialize to the JSON wire form.
    pub fn to_json(&self) -> String {
        // The struct is always serializable; fall back to an empty-object-ish
        // string only defensively (never hit in practice).
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse from the JSON wire form, rejecting a version we don't understand
    /// (returns `None` so a stale/foreign archive is ignored rather than
    /// misrestored).
    pub fn from_json(s: &str) -> Option<WindowSession> {
        let session: WindowSession = serde_json::from_str(s).ok()?;
        (session.version == SESSION_VERSION).then_some(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WindowSession {
        WindowSession::new(SessionNode::Split {
            axis: SessionAxis::Horizontal,
            ratio: 0.6,
            first: Box::new(SessionNode::Leaf {
                cwd: Some("/Users/me/proj".into()),
            }),
            second: Box::new(SessionNode::Split {
                axis: SessionAxis::Vertical,
                ratio: 0.5,
                first: Box::new(SessionNode::Leaf {
                    cwd: Some("/tmp".into()),
                }),
                second: Box::new(SessionNode::Leaf { cwd: None }),
            }),
        })
    }

    #[test]
    fn json_round_trip_preserves_the_tree() {
        let s = sample();
        let json = s.to_json();
        let back = WindowSession::from_json(&json).expect("round-trips");
        assert_eq!(back, s);
    }

    #[test]
    fn leaf_count_walks_the_tree() {
        assert_eq!(sample().tree.leaf_count(), 3);
        assert_eq!(SessionNode::Leaf { cwd: None }.leaf_count(), 1);
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let mut json = serde_json::to_value(sample()).unwrap();
        json["version"] = serde_json::json!(999);
        assert!(WindowSession::from_json(&json.to_string()).is_none());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(WindowSession::from_json("not json").is_none());
        assert!(WindowSession::from_json("{}").is_none());
    }
}
