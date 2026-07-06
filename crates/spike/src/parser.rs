use crate::color::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParserState {
    Ground,
    Escape,
    EscapeHash,
    Csi,
    Osc,
    OscEscape,
}

pub(crate) struct CsiSequence {
    pub(crate) raw: String,
    pub(crate) private: bool,
    pub(crate) params: Vec<Option<usize>>,
}

impl CsiSequence {
    pub(crate) fn parse(raw: String) -> Self {
        let private = raw.starts_with('?');
        let params_text = raw.trim_start_matches('?').trim();
        let params = parse_params(params_text);
        Self {
            raw,
            private,
            params,
        }
    }

    pub(crate) fn has_intermediate_space(&self) -> bool {
        self.raw.ends_with(' ')
    }
}

pub(crate) fn is_csi_parameter_or_intermediate(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b';' | b':' | b'?' | b'>' | b'=' | b'<' | b' ' | b'!')
}

pub(crate) fn parse_params(text: &str) -> Vec<Option<usize>> {
    if text.is_empty() {
        return Vec::new();
    }

    text.trim_start_matches(['>', '=', '<'])
        .split([';', ':'])
        .map(|param| {
            if param.is_empty() {
                None
            } else {
                param.parse::<usize>().ok()
            }
        })
        .collect()
}

pub(crate) fn parse_extended_color(params: &[Option<usize>], idx: usize) -> Option<(Color, usize)> {
    match params.get(idx).copied().flatten()? {
        5 => {
            let value = params.get(idx + 1).copied().flatten()?;
            Some((Color::Indexed(value.min(255) as u8), 2))
        }
        2 => {
            let mut offset = idx + 1;
            if params.get(offset).is_some_and(Option::is_none) {
                offset += 1;
            }
            let r = params.get(offset).copied().flatten()?.min(255) as u8;
            let g = params.get(offset + 1).copied().flatten()?.min(255) as u8;
            let b = params.get(offset + 2).copied().flatten()?.min(255) as u8;
            Some((Color::Rgb { r, g, b }, offset + 3 - idx))
        }
        _ => None,
    }
}

pub(crate) fn param_or(params: &[Option<usize>], idx: usize, default: usize) -> usize {
    match params.get(idx).copied().flatten() {
        Some(0) | None => default,
        Some(n) => n,
    }
}

pub(crate) fn one_based_to_zero(n: usize) -> usize {
    n.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csi_sequence_tracks_private_params_and_intermediate_space() {
        let csi = CsiSequence::parse("?25".to_string());
        assert!(csi.private);
        assert_eq!(csi.params, [Some(25)]);
        assert!(!csi.has_intermediate_space());

        let csi = CsiSequence::parse("4 ".to_string());
        assert!(!csi.private);
        assert_eq!(csi.params, [Some(4)]);
        assert!(csi.has_intermediate_space());
    }

    #[test]
    fn csi_collects_parameter_and_intermediate_bytes() {
        assert!(is_csi_parameter_or_intermediate(b'?'));
        assert!(is_csi_parameter_or_intermediate(b' '));
        assert!(is_csi_parameter_or_intermediate(b'9'));
        assert!(!is_csi_parameter_or_intermediate(b'm'));
    }
}
