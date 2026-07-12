//! Full-binding parser. Port of `Binding.Parser` and `SequenceIterator` in
//! `input/Binding.zig` (upstream `2da015cd6`, lines 74-240).
//!
//! A raw binding string is `<flags>*<trigger>(>trigger)*=<action>` or the
//! special `chain=<action>` form. The parser strips flag prefixes, finds the
//! real `=` (skipping `=` that are part of the trigger), parses the action
//! eagerly, and then yields one [`ParseItem`] per trigger in the sequence.

use super::BindError;
use super::action::Action;
use super::flags::Flags;
use super::trigger::Trigger;

/// A parsed binding: a single trigger, an action, and flags. Port of
/// `Binding` (Binding.zig:16-23).
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub trigger: Trigger,
    pub action: Action,
    pub flags: Flags,
}

/// One yielded element of a binding parse. Port of `Parser.Elem`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseItem {
    /// A non-final trigger in a sequence (a leader).
    Leader(Trigger),
    /// The final trigger + action + flags.
    Binding(Binding),
    /// A `chain=<action>` action to append to the previously-added binding.
    /// Any action is parsed here, including chain-invalid ones like `unbind`;
    /// downstream consumers validate.
    Chain(Action),
}

/// Iterates the `>`-separated triggers of a sequence without allocating. Port
/// of `SequenceIterator`. The input must be triggers only — flag prefixes and
/// the action must be stripped first.
struct SequenceIterator<'a> {
    input: &'a str,
    i: usize,
}

impl<'a> SequenceIterator<'a> {
    // Not an `Iterator`: yields `Result<Option<_>>` so a trigger parse error
    // can surface. Named `next` to mirror upstream `SequenceIterator.next`.
    #[allow(clippy::should_implement_trait)]
    fn next(&mut self) -> Result<Option<Trigger>, BindError> {
        if self.done() {
            return Ok(None);
        }
        let rem = &self.input[self.i..];
        let idx = rem.find('>').unwrap_or(rem.len());
        self.i += idx + 1;
        Ok(Some(Trigger::parse(&rem[..idx])?))
    }

    /// True when there are no more triggers. Mirrors upstream's `i >
    /// input.len` (note strictly greater: after consuming the final segment
    /// `i` sits one past the end).
    fn done(&self) -> bool {
        self.i > self.input.len()
    }
}

/// Parses a full binding string into a sequence of [`ParseItem`]s. Port of
/// `Binding.Parser`.
pub struct Parser<'a> {
    trigger_it: SequenceIterator<'a>,
    action: Action,
    flags: Flags,
    chain: bool,
}

impl<'a> Parser<'a> {
    /// Initialize from a raw binding string (e.g. `unconsumed:ctrl+a=text:hi`
    /// or `ctrl+a>ctrl+b=new_tab` or `chain=new_tab`). The action is parsed
    /// eagerly; trigger parse errors surface later from [`Parser::next`].
    pub fn init(raw_input: &'a str) -> Result<Parser<'a>, BindError> {
        let (flags, start_idx) = parse_flags(raw_input)?;
        let input = &raw_input[start_idx..];

        // Find the `=` that separates trigger(s) from action, skipping any `=`
        // immediately followed by `+` or `=` (so `=` can be a trigger key, and
        // `text:=hello` action values survive). Port of Binding.zig:98-126.
        let eql_idx = {
            let bytes = input.as_bytes();
            let mut offset = 0usize;
            loop {
                match input[offset..].find('=') {
                    Some(off_idx) => {
                        let idx = offset + off_idx;
                        if idx < input.len().saturating_sub(1)
                            && (bytes[idx + 1] == b'+' || bytes[idx + 1] == b'=')
                        {
                            offset = idx + 1;
                            continue;
                        }
                        break idx;
                    }
                    None => return Err(BindError::InvalidFormat),
                }
            }
        };

        // Chains must not have flag prefixes.
        let chain = &input[..eql_idx] == "chain";
        if chain && start_idx > 0 {
            return Err(BindError::InvalidFormat);
        }

        Ok(Parser {
            // A dummy single trigger for chains; `next` never yields it because
            // `chain` is set (matches the upstream hack).
            trigger_it: SequenceIterator {
                input: if chain { "a" } else { &input[..eql_idx] },
                i: 0,
            },
            action: Action::parse(&input[eql_idx + 1..])?,
            flags,
            chain,
        })
    }

    /// Yield the next parse element, or `None` when done.
    // Not an `Iterator` (yields `Result<Option<_>>`); named to mirror upstream.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<ParseItem>, BindError> {
        let trigger = match self.trigger_it.next()? {
            Some(t) => t,
            None => return Ok(None),
        };

        // Not the last trigger → a leader. Global/all bindings can't sequence.
        if !self.trigger_it.done() {
            if self.flags.global || self.flags.all {
                return Err(BindError::InvalidFormat);
            }
            return Ok(Some(ParseItem::Leader(trigger)));
        }

        if self.chain {
            return Ok(Some(ParseItem::Chain(self.action.clone())));
        }

        Ok(Some(ParseItem::Binding(Binding {
            trigger,
            action: self.action.clone(),
            flags: self.flags,
        })))
    }
}

impl Binding {
    /// Parse a single, non-sequenced, non-chained binding. Convenience for
    /// callers and tests (mirrors upstream's `parseSingle`/`parse` helper);
    /// errors if the input is a sequence or a chain.
    pub fn parse(raw_input: &str) -> Result<Binding, BindError> {
        let mut parser = Parser::init(raw_input)?;
        match parser.next()? {
            Some(ParseItem::Binding(b)) => {
                // Ensure nothing follows.
                match parser.next()? {
                    None => Ok(b),
                    _ => Err(BindError::InvalidFormat),
                }
            }
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Strip recognized flag prefixes (`all:`, `global:`, `unconsumed:`,
/// `performable:`) from the front, returning the flags and the byte offset
/// where the trigger begins. An unrecognized prefix stops flag parsing (it may
/// be trigger-specific). Port of `parseFlags` (Binding.zig:148-187).
fn parse_flags(raw_input: &str) -> Result<(Flags, usize), BindError> {
    let mut flags = Flags::new();
    let mut start_idx = 0usize;
    let mut input = raw_input;
    while let Some(idx) = input.find(':') {
        match &input[..idx] {
            "all" => {
                if flags.all {
                    return Err(BindError::InvalidFormat);
                }
                flags.all = true;
            }
            "global" => {
                if flags.global {
                    return Err(BindError::InvalidFormat);
                }
                flags.global = true;
            }
            "unconsumed" => {
                if !flags.consumed {
                    return Err(BindError::InvalidFormat);
                }
                flags.consumed = false;
            }
            "performable" => {
                if flags.performable {
                    return Err(BindError::InvalidFormat);
                }
                flags.performable = true;
            }
            // Unknown prefix: stop, let it fall through to trigger parsing.
            _ => break,
        }
        start_idx += idx + 1;
        input = &input[idx + 1..];
    }
    Ok((flags, start_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::trigger::TriggerKey;
    use crate::key_mods::Mods;

    fn only_binding(raw: &str) -> Binding {
        Binding::parse(raw).unwrap()
    }

    #[test]
    fn parse_single_binding() {
        let b = only_binding("ctrl+a=ignore");
        assert_eq!(b.action, Action::Ignore);
        assert!(b.flags.consumed);
        assert_eq!(b.trigger.key, TriggerKey::Unicode('a' as u32));
        assert!(b.trigger.mods.ctrl);
    }

    /// Port of `parse: unconsumed/performable prefixes` behavior.
    #[test]
    fn parse_flag_prefixes() {
        assert!(!only_binding("unconsumed:ctrl+a=ignore").flags.consumed);
        assert!(only_binding("performable:ctrl+a=ignore").flags.performable);
        assert!(only_binding("global:a=ignore").flags.global);
        assert!(only_binding("all:a=ignore").flags.all);
        // Combined, any order.
        let b = only_binding("unconsumed:global:a=ignore");
        assert!(b.flags.global && !b.flags.consumed);
        // Duplicate prefix → error.
        assert!(Parser::init("unconsumed:unconsumed:a=ignore").is_err());
    }

    /// Port of `parse: equals sign` (3016) and `parse: text action equals
    /// sign` (3042).
    #[test]
    fn parse_equals_sign() {
        // `==ignore`: the first `=` is the key, the second the separator.
        assert_eq!(
            only_binding("==ignore").trigger.key,
            TriggerKey::Unicode('=' as u32)
        );
        // `ctrl+==ignore`: ctrl + `=` key.
        let b = only_binding("ctrl+==ignore");
        assert!(b.trigger.mods.ctrl);
        assert_eq!(b.trigger.key, TriggerKey::Unicode('=' as u32));
        // Action value may contain `=`.
        assert_eq!(
            only_binding("a=text:=hello").action,
            Action::Text("=hello".to_string())
        );
        // Bare `=ignore` (no trigger before the `=`) is InvalidFormat: `=` is
        // followed by `i`, so it's the separator, leaving an empty trigger.
        assert!(Binding::parse("=ignore").is_err());
    }

    /// Port of `parse: global triggers` (3118) / `all triggers` (3161):
    /// sequences with global/all are rejected.
    #[test]
    fn global_all_cannot_sequence() {
        assert!(run_to_end("global:a>b=ignore").is_err());
        assert!(run_to_end("all:a>b=ignore").is_err());
        // But non-sequenced global/all is fine.
        assert!(run_to_end("global:a=ignore").is_ok());
    }

    /// Port of `parse: sequences` (3490): leader then binding.
    #[test]
    fn parse_sequences() {
        let mut p = Parser::init("ctrl+a>ctrl+b=new_tab").unwrap();
        match p.next().unwrap().unwrap() {
            ParseItem::Leader(t) => {
                assert!(t.mods.ctrl);
                assert_eq!(t.key, TriggerKey::Unicode('a' as u32));
            }
            other => panic!("expected leader, got {other:?}"),
        }
        match p.next().unwrap().unwrap() {
            ParseItem::Binding(b) => {
                assert_eq!(b.action, Action::NewTab);
                assert_eq!(b.trigger.key, TriggerKey::Unicode('b' as u32));
            }
            other => panic!("expected binding, got {other:?}"),
        }
        assert!(p.next().unwrap().is_none());
    }

    /// Empty sequence segments are errors (`>a`, `a>`).
    #[test]
    fn empty_sequence_segments_error() {
        assert!(run_to_end(">a=ignore").is_err());
        assert!(run_to_end("a>=ignore").is_err());
    }

    /// Port of `parse: chain` (3429).
    #[test]
    fn parse_chain() {
        let mut p = Parser::init("chain=new_tab").unwrap();
        match p.next().unwrap().unwrap() {
            ParseItem::Chain(a) => assert_eq!(a, Action::NewTab),
            other => panic!("expected chain, got {other:?}"),
        }
        assert!(p.next().unwrap().is_none());
        // Flag-prefixed chain is invalid.
        assert!(Parser::init("global:chain=new_tab").is_err());
        // `a>chain=` fails because "chain" parses as a trigger and is invalid.
        assert!(run_to_end("a>chain=new_tab").is_err());
    }

    #[test]
    fn no_equals_is_error() {
        assert!(Parser::init("ctrl+a").is_err());
    }

    #[test]
    fn missing_mods_default_consumed() {
        let m = Mods::default();
        assert!(!m.ctrl);
        assert!(only_binding("a=ignore").flags.consumed);
    }

    /// Drive a parser to completion, returning the first error if any.
    fn run_to_end(raw: &str) -> Result<(), BindError> {
        let mut p = Parser::init(raw)?;
        while p.next()?.is_some() {}
        Ok(())
    }
}
