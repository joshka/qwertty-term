//! Terminfo capability responses for XTGETTCAP (`DCS + q <names> ST`).
//!
//! A program can ask the terminal for a terminfo capability's value with
//! XTGETTCAP; we reply `ESC P 1 + r <hexname>=<hexvalue> ESC \` for a known
//! capability, or nothing for one we don't answer (matching upstream, which
//! simply skips unknown keys).
//!
//! This is a byte-faithful port of `terminfo.ghostty.xtgettcapMap()`
//! (`ghostty/src/terminfo/ghostty.zig` + `Source.zig:71-150`): the full
//! capability set (`TN`/`Co`/`RGB` specials plus every entry in the ghostty
//! terminfo Source), with the same value encoding — string caps containing a
//! `%` parameter are returned verbatim in terminfo source form, everything else
//! has `\E` mapped to ESC (0x1B) and a leading `^X` mapped to its control byte.
//! The one deliberate divergence is `TN` (terminal name): we answer
//! `qwertty-term`, never `xterm-ghostty` (product identity — trademark).
//!
//! XTGETTCAP is answered only by the app/termio layer upstream (the lib-vt core
//! ignores DCS), so it has no differential-oracle coverage and is verified by
//! unit tests. Regenerate the table from upstream with
//! `scripts/gen_terminfo.py` (kept alongside this file's history).

/// A terminfo capability value, mirroring `Source.Capability.Value`
/// (`ghostty/src/terminfo/Source.zig`). String values are stored in terminfo
/// *source* form (literal `\E`, `^X`, `%`-parameters) exactly as they appear in
/// `ghostty.zig`; [`encode_string_cap`] applies the query-time transform.
enum CapValue {
    /// Present, carries no value (replied as `<hexname>` with no `=value`).
    Boolean,
    /// A decimal number (replied as its ASCII digits).
    Numeric(i64),
    /// A string in terminfo source form.
    Str(&'static [u8]),
}

/// The full ghostty terminfo capability table, in upstream declaration order.
/// Port of `ghostty.zig`'s `.capabilities`. The `TN`/`Co`/`RGB` query specials
/// are handled separately in [`xtgettcap_value`] (upstream adds them ahead of
/// this list in `xtgettcapMap`).
#[rustfmt::skip]
static CAPABILITIES: &[(&[u8], CapValue)] = &[
    (b"am", CapValue::Boolean),
    (b"bce", CapValue::Boolean),
    (b"ccc", CapValue::Boolean),
    (b"hs", CapValue::Boolean),
    (b"km", CapValue::Boolean),
    (b"mc5i", CapValue::Boolean),
    (b"mir", CapValue::Boolean),
    (b"msgr", CapValue::Boolean),
    (b"npc", CapValue::Boolean),
    (b"xenl", CapValue::Boolean),
    (b"AX", CapValue::Boolean),
    (b"Tc", CapValue::Boolean),
    (b"Su", CapValue::Boolean),
    (b"XT", CapValue::Boolean),
    (b"fullkbd", CapValue::Boolean),
    (b"colors", CapValue::Numeric(256)),
    (b"cols", CapValue::Numeric(80)),
    (b"it", CapValue::Numeric(8)),
    (b"lines", CapValue::Numeric(24)),
    (b"pairs", CapValue::Numeric(32767)),
    (b"acsc", CapValue::Str(br"++\,\,--..00``aaffgghhiijjkkllmmnnooppqqrrssttuuvvwwxxyyzz{{||}}~~")),
    (b"Smulx", CapValue::Str(br"\E[4:%p1%dm")),
    (b"Setulc", CapValue::Str(br"\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m")),
    (b"Ss", CapValue::Str(br"\E[%p1%d q")),
    (b"Se", CapValue::Str(br"\E[0 q")),
    (b"Ms", CapValue::Str(br"\E]52;%p1%s;%p2%s\007")),
    (b"Sync", CapValue::Str(br"\E[?2026%?%p1%{1}%-%tl%eh%;")),
    (b"BD", CapValue::Str(br"\E[?2004l")),
    (b"BE", CapValue::Str(br"\E[?2004h")),
    (b"PS", CapValue::Str(br"\E[200~")),
    (b"PE", CapValue::Str(br"\E[201~")),
    (b"XM", CapValue::Str(br"\E[?1006;1000%?%p1%{1}%=%th%el%;")),
    (b"xm", CapValue::Str(br"\E[<%i%p3%d;%p1%d;%p2%d;%?%p4%tM%em%;")),
    (b"RV", CapValue::Str(br"\E[>c")),
    (b"rv", CapValue::Str(br"\E\\[[0-9]+;[0-9]+;[0-9]+c")),
    (b"XR", CapValue::Str(br"\E[>0q")),
    (b"xr", CapValue::Str(br"\EP>\\|[ -~]+a\E\\")),
    (b"Enmg", CapValue::Str(br"\E[?69h")),
    (b"Dsmg", CapValue::Str(br"\E[?69l")),
    (b"Clmg", CapValue::Str(br"\E[s")),
    (b"Cmg", CapValue::Str(br"\E[%i%p1%d;%p2%ds")),
    (b"clear", CapValue::Str(br"\E[H\E[2J")),
    (b"E3", CapValue::Str(br"\E[3J")),
    (b"fe", CapValue::Str(br"\E[?1004h")),
    (b"fd", CapValue::Str(br"\E[?1004l")),
    (b"kxIN", CapValue::Str(br"\E[I")),
    (b"kxOUT", CapValue::Str(br"\E[O")),
    (b"bel", CapValue::Str(br"^G")),
    (b"blink", CapValue::Str(br"\E[5m")),
    (b"bold", CapValue::Str(br"\E[1m")),
    (b"cbt", CapValue::Str(br"\E[Z")),
    (b"civis", CapValue::Str(br"\E[?25l")),
    (b"cnorm", CapValue::Str(br"\E[?12l\E[?25h")),
    (b"cr", CapValue::Str(br"\r")),
    (b"csr", CapValue::Str(br"\E[%i%p1%d;%p2%dr")),
    (b"cub", CapValue::Str(br"\E[%p1%dD")),
    (b"cub1", CapValue::Str(br"^H")),
    (b"cud", CapValue::Str(br"\E[%p1%dB")),
    (b"cud1", CapValue::Str(br"^J")),
    (b"cuf", CapValue::Str(br"\E[%p1%dC")),
    (b"cuf1", CapValue::Str(br"\E[C")),
    (b"cup", CapValue::Str(br"\E[%i%p1%d;%p2%dH")),
    (b"cuu", CapValue::Str(br"\E[%p1%dA")),
    (b"cuu1", CapValue::Str(br"\E[A")),
    (b"cvvis", CapValue::Str(br"\E[?12;25h")),
    (b"dch", CapValue::Str(br"\E[%p1%dP")),
    (b"dch1", CapValue::Str(br"\E[P")),
    (b"dim", CapValue::Str(br"\E[2m")),
    (b"dl", CapValue::Str(br"\E[%p1%dM")),
    (b"dl1", CapValue::Str(br"\E[M")),
    (b"dsl", CapValue::Str(br"\E]2;\007")),
    (b"ech", CapValue::Str(br"\E[%p1%dX")),
    (b"ed", CapValue::Str(br"\E[J")),
    (b"el", CapValue::Str(br"\E[K")),
    (b"el1", CapValue::Str(br"\E[1K")),
    (b"flash", CapValue::Str(br"\E[?5h$<100/>\E[?5l")),
    (b"fsl", CapValue::Str(br"^G")),
    (b"home", CapValue::Str(br"\E[H")),
    (b"hpa", CapValue::Str(br"\E[%i%p1%dG")),
    (b"ht", CapValue::Str(br"^I")),
    (b"hts", CapValue::Str(br"\EH")),
    (b"ich", CapValue::Str(br"\E[%p1%d@")),
    (b"ich1", CapValue::Str(br"\E[@")),
    (b"il", CapValue::Str(br"\E[%p1%dL")),
    (b"il1", CapValue::Str(br"\E[L")),
    (b"ind", CapValue::Str(br"\n")),
    (b"indn", CapValue::Str(br"\E[%p1%dS")),
    (b"initc", CapValue::Str(br"\E]4;%p1%d;rgb\:%p2%{255}%*%{1000}%/%2.2X/%p3%{255}%*%{1000}%/%2.2X/%p4%{255}%*%{1000}%/%2.2X\E\\")),
    (b"invis", CapValue::Str(br"\E[8m")),
    (b"oc", CapValue::Str(br"\E]104\007")),
    (b"op", CapValue::Str(br"\E[39;49m")),
    (b"rc", CapValue::Str(br"\E8")),
    (b"rep", CapValue::Str(br"%p1%c\E[%p2%{1}%-%db")),
    (b"rev", CapValue::Str(br"\E[7m")),
    (b"ri", CapValue::Str(br"\EM")),
    (b"rin", CapValue::Str(br"\E[%p1%dT")),
    (b"ritm", CapValue::Str(br"\E[23m")),
    (b"rmacs", CapValue::Str(br"\E(B")),
    (b"rmam", CapValue::Str(br"\E[?7l")),
    (b"rmcup", CapValue::Str(br"\E[?1049l")),
    (b"rmir", CapValue::Str(br"\E[4l")),
    (b"rmkx", CapValue::Str(br"\E[?1l\E>")),
    (b"rmso", CapValue::Str(br"\E[27m")),
    (b"rmul", CapValue::Str(br"\E[24m")),
    (b"rmxx", CapValue::Str(br"\E[29m")),
    (b"setab", CapValue::Str(br"\E[%?%p1%{8}%<%t4%p1%d%e%p1%{16}%<%t10%p1%{8}%-%d%e48;5;%p1%d%;m")),
    (b"setaf", CapValue::Str(br"\E[%?%p1%{8}%<%t3%p1%d%e%p1%{16}%<%t9%p1%{8}%-%d%e38;5;%p1%d%;m")),
    (b"setrgbb", CapValue::Str(br"\E[48:2:%p1%d:%p2%d:%p3%dm")),
    (b"setrgbf", CapValue::Str(br"\E[38:2:%p1%d:%p2%d:%p3%dm")),
    (b"sgr", CapValue::Str(br"%?%p9%t\E(0%e\E(B%;\E[0%?%p6%t;1%;%?%p5%t;2%;%?%p2%t;4%;%?%p1%p3%|%t;7%;%?%p4%t;5%;%?%p7%t;8%;m")),
    (b"sgr0", CapValue::Str(br"\E(B\E[m")),
    (b"sitm", CapValue::Str(br"\E[3m")),
    (b"smacs", CapValue::Str(br"\E(0")),
    (b"smam", CapValue::Str(br"\E[?7h")),
    (b"smcup", CapValue::Str(br"\E[?1049h")),
    (b"smir", CapValue::Str(br"\E[4h")),
    (b"smkx", CapValue::Str(br"\E[?1h\E=")),
    (b"smso", CapValue::Str(br"\E[7m")),
    (b"smul", CapValue::Str(br"\E[4m")),
    (b"smxx", CapValue::Str(br"\E[9m")),
    (b"tbc", CapValue::Str(br"\E[3g")),
    (b"tsl", CapValue::Str(br"\E]2;")),
    (b"u6", CapValue::Str(br"\E[%i%d;%dR")),
    (b"u7", CapValue::Str(br"\E[6n")),
    (b"u8", CapValue::Str(br"\E[?%[;0123456789]c")),
    (b"u9", CapValue::Str(br"\E[c")),
    (b"vpa", CapValue::Str(br"\E[%i%p1%dd")),
    (b"kDC", CapValue::Str(br"\E[3;2~")),
    (b"kDC3", CapValue::Str(br"\E[3;3~")),
    (b"kDC4", CapValue::Str(br"\E[3;4~")),
    (b"kDC5", CapValue::Str(br"\E[3;5~")),
    (b"kDC6", CapValue::Str(br"\E[3;6~")),
    (b"kDC7", CapValue::Str(br"\E[3;7~")),
    (b"kDN", CapValue::Str(br"\E[1;2B")),
    (b"kDN3", CapValue::Str(br"\E[1;3B")),
    (b"kDN4", CapValue::Str(br"\E[1;4B")),
    (b"kDN5", CapValue::Str(br"\E[1;5B")),
    (b"kDN6", CapValue::Str(br"\E[1;6B")),
    (b"kDN7", CapValue::Str(br"\E[1;7B")),
    (b"kEND", CapValue::Str(br"\E[1;2F")),
    (b"kEND3", CapValue::Str(br"\E[1;3F")),
    (b"kEND4", CapValue::Str(br"\E[1;4F")),
    (b"kEND5", CapValue::Str(br"\E[1;5F")),
    (b"kEND6", CapValue::Str(br"\E[1;6F")),
    (b"kEND7", CapValue::Str(br"\E[1;7F")),
    (b"kHOM", CapValue::Str(br"\E[1;2H")),
    (b"kHOM3", CapValue::Str(br"\E[1;3H")),
    (b"kHOM4", CapValue::Str(br"\E[1;4H")),
    (b"kHOM5", CapValue::Str(br"\E[1;5H")),
    (b"kHOM6", CapValue::Str(br"\E[1;6H")),
    (b"kHOM7", CapValue::Str(br"\E[1;7H")),
    (b"kIC", CapValue::Str(br"\E[2;2~")),
    (b"kIC3", CapValue::Str(br"\E[2;3~")),
    (b"kIC4", CapValue::Str(br"\E[2;4~")),
    (b"kIC5", CapValue::Str(br"\E[2;5~")),
    (b"kIC6", CapValue::Str(br"\E[2;6~")),
    (b"kIC7", CapValue::Str(br"\E[2;7~")),
    (b"kLFT", CapValue::Str(br"\E[1;2D")),
    (b"kLFT3", CapValue::Str(br"\E[1;3D")),
    (b"kLFT4", CapValue::Str(br"\E[1;4D")),
    (b"kLFT5", CapValue::Str(br"\E[1;5D")),
    (b"kLFT6", CapValue::Str(br"\E[1;6D")),
    (b"kLFT7", CapValue::Str(br"\E[1;7D")),
    (b"kNXT", CapValue::Str(br"\E[6;2~")),
    (b"kNXT3", CapValue::Str(br"\E[6;3~")),
    (b"kNXT4", CapValue::Str(br"\E[6;4~")),
    (b"kNXT5", CapValue::Str(br"\E[6;5~")),
    (b"kNXT6", CapValue::Str(br"\E[6;6~")),
    (b"kNXT7", CapValue::Str(br"\E[6;7~")),
    (b"kPRV", CapValue::Str(br"\E[5;2~")),
    (b"kPRV3", CapValue::Str(br"\E[5;3~")),
    (b"kPRV4", CapValue::Str(br"\E[5;4~")),
    (b"kPRV5", CapValue::Str(br"\E[5;5~")),
    (b"kPRV6", CapValue::Str(br"\E[5;6~")),
    (b"kPRV7", CapValue::Str(br"\E[5;7~")),
    (b"kRIT", CapValue::Str(br"\E[1;2C")),
    (b"kRIT3", CapValue::Str(br"\E[1;3C")),
    (b"kRIT4", CapValue::Str(br"\E[1;4C")),
    (b"kRIT5", CapValue::Str(br"\E[1;5C")),
    (b"kRIT6", CapValue::Str(br"\E[1;6C")),
    (b"kRIT7", CapValue::Str(br"\E[1;7C")),
    (b"kUP", CapValue::Str(br"\E[1;2A")),
    (b"kUP3", CapValue::Str(br"\E[1;3A")),
    (b"kUP4", CapValue::Str(br"\E[1;4A")),
    (b"kUP5", CapValue::Str(br"\E[1;5A")),
    (b"kUP6", CapValue::Str(br"\E[1;6A")),
    (b"kUP7", CapValue::Str(br"\E[1;7A")),
    (b"kbs", CapValue::Str(br"^?")),
    (b"kcbt", CapValue::Str(br"\E[Z")),
    (b"kcub1", CapValue::Str(br"\EOD")),
    (b"kcud1", CapValue::Str(br"\EOB")),
    (b"kcuf1", CapValue::Str(br"\EOC")),
    (b"kcuu1", CapValue::Str(br"\EOA")),
    (b"kdch1", CapValue::Str(br"\E[3~")),
    (b"kend", CapValue::Str(br"\EOF")),
    (b"kent", CapValue::Str(br"\EOM")),
    (b"kf1", CapValue::Str(br"\EOP")),
    (b"kf10", CapValue::Str(br"\E[21~")),
    (b"kf11", CapValue::Str(br"\E[23~")),
    (b"kf12", CapValue::Str(br"\E[24~")),
    (b"kf13", CapValue::Str(br"\E[1;2P")),
    (b"kf14", CapValue::Str(br"\E[1;2Q")),
    (b"kf15", CapValue::Str(br"\E[1;2R")),
    (b"kf16", CapValue::Str(br"\E[1;2S")),
    (b"kf17", CapValue::Str(br"\E[15;2~")),
    (b"kf18", CapValue::Str(br"\E[17;2~")),
    (b"kf19", CapValue::Str(br"\E[18;2~")),
    (b"kf2", CapValue::Str(br"\EOQ")),
    (b"kf20", CapValue::Str(br"\E[19;2~")),
    (b"kf21", CapValue::Str(br"\E[20;2~")),
    (b"kf22", CapValue::Str(br"\E[21;2~")),
    (b"kf23", CapValue::Str(br"\E[23;2~")),
    (b"kf24", CapValue::Str(br"\E[24;2~")),
    (b"kf25", CapValue::Str(br"\E[1;5P")),
    (b"kf26", CapValue::Str(br"\E[1;5Q")),
    (b"kf27", CapValue::Str(br"\E[1;5R")),
    (b"kf28", CapValue::Str(br"\E[1;5S")),
    (b"kf29", CapValue::Str(br"\E[15;5~")),
    (b"kf3", CapValue::Str(br"\EOR")),
    (b"kf30", CapValue::Str(br"\E[17;5~")),
    (b"kf31", CapValue::Str(br"\E[18;5~")),
    (b"kf32", CapValue::Str(br"\E[19;5~")),
    (b"kf33", CapValue::Str(br"\E[20;5~")),
    (b"kf34", CapValue::Str(br"\E[21;5~")),
    (b"kf35", CapValue::Str(br"\E[23;5~")),
    (b"kf36", CapValue::Str(br"\E[24;5~")),
    (b"kf37", CapValue::Str(br"\E[1;6P")),
    (b"kf38", CapValue::Str(br"\E[1;6Q")),
    (b"kf39", CapValue::Str(br"\E[1;6R")),
    (b"kf4", CapValue::Str(br"\EOS")),
    (b"kf40", CapValue::Str(br"\E[1;6S")),
    (b"kf41", CapValue::Str(br"\E[15;6~")),
    (b"kf42", CapValue::Str(br"\E[17;6~")),
    (b"kf43", CapValue::Str(br"\E[18;6~")),
    (b"kf44", CapValue::Str(br"\E[19;6~")),
    (b"kf45", CapValue::Str(br"\E[20;6~")),
    (b"kf46", CapValue::Str(br"\E[21;6~")),
    (b"kf47", CapValue::Str(br"\E[23;6~")),
    (b"kf48", CapValue::Str(br"\E[24;6~")),
    (b"kf49", CapValue::Str(br"\E[1;3P")),
    (b"kf5", CapValue::Str(br"\E[15~")),
    (b"kf50", CapValue::Str(br"\E[1;3Q")),
    (b"kf51", CapValue::Str(br"\E[1;3R")),
    (b"kf52", CapValue::Str(br"\E[1;3S")),
    (b"kf53", CapValue::Str(br"\E[15;3~")),
    (b"kf54", CapValue::Str(br"\E[17;3~")),
    (b"kf55", CapValue::Str(br"\E[18;3~")),
    (b"kf56", CapValue::Str(br"\E[19;3~")),
    (b"kf57", CapValue::Str(br"\E[20;3~")),
    (b"kf58", CapValue::Str(br"\E[21;3~")),
    (b"kf59", CapValue::Str(br"\E[23;3~")),
    (b"kf6", CapValue::Str(br"\E[17~")),
    (b"kf60", CapValue::Str(br"\E[24;3~")),
    (b"kf61", CapValue::Str(br"\E[1;4P")),
    (b"kf62", CapValue::Str(br"\E[1;4Q")),
    (b"kf63", CapValue::Str(br"\E[1;4R")),
    (b"kf7", CapValue::Str(br"\E[18~")),
    (b"kf8", CapValue::Str(br"\E[19~")),
    (b"kf9", CapValue::Str(br"\E[20~")),
    (b"khome", CapValue::Str(br"\EOH")),
    (b"kich1", CapValue::Str(br"\E[2~")),
    (b"kind", CapValue::Str(br"\E[1;2B")),
    (b"kmous", CapValue::Str(br"\E[<")),
    (b"knp", CapValue::Str(br"\E[6~")),
    (b"kpp", CapValue::Str(br"\E[5~")),
    (b"kri", CapValue::Str(br"\E[1;2A")),
    (b"rs1", CapValue::Str(br"\E]\E\\\Ec")),
    (b"sc", CapValue::Str(br"\E7")),
];

/// Build the XTGETTCAP reply for a requested (uppercase) hex-encoded capability
/// name, or `None` if we don't answer that capability. `hex_name` is the raw
/// requested key from the DCS parser (already uppercased). The reply is
/// `ESC P 1 + r <hexname>[=<hexvalue>] ESC \`; a boolean capability (empty
/// value) has no `=value`. Port of the `xtgettcapMap` lookup + reply framing in
/// `stream_handler.zig:467-473` / `Source.zig:124-146`.
pub fn xtgettcap_response(hex_name: &[u8]) -> Option<Vec<u8>> {
    let value = xtgettcap_value(hex_name)?;
    let mut resp = Vec::with_capacity(6 + hex_name.len() + 1 + value.len() * 2 + 2);
    resp.extend_from_slice(b"\x1bP1+r");
    resp.extend_from_slice(hex_name);
    if !value.is_empty() {
        resp.push(b'=');
        for &b in &value {
            resp.push(hex_digit(b >> 4));
            resp.push(hex_digit(b & 0x0f));
        }
    }
    resp.extend_from_slice(b"\x1b\\");
    Some(resp)
}

/// Resolve a hex-encoded capability name to its raw (pre-hex) terminfo value, or
/// `None` if we don't answer it. A boolean capability yields `Some(vec![])`
/// (present, no value). Ports the `TN`/`Co`/`RGB` specials plus the capability
/// table lookup and per-kind value encoding from `Source.zig:80-122`.
fn xtgettcap_value(hex_name: &[u8]) -> Option<Vec<u8>> {
    let name = hex_decode(hex_name)?;
    // Query specials, added ahead of the capability list upstream
    // (`Source.zig:80-82`). `TN` is our sole deliberate divergence.
    match name.as_slice() {
        b"TN" => return Some(b"qwertty-term".to_vec()),
        b"Co" => return Some(b"256".to_vec()),
        b"RGB" => return Some(b"8".to_vec()),
        _ => {}
    }
    let (_, value) = CAPABILITIES.iter().find(|(n, _)| *n == name.as_slice())?;
    Some(match value {
        CapValue::Boolean => Vec::new(),
        CapValue::Numeric(n) => n.to_string().into_bytes(),
        CapValue::Str(s) => encode_string_cap(s),
    })
}

/// Encode a terminfo source-form string capability into the bytes XTGETTCAP
/// reports (before hex-encoding). Port of the `.string` arm of
/// `Source.zig:88-115`:
///
/// - If the string contains a `%` parameter, it is returned verbatim in
///   terminfo source form (xterm/kitty convention — parameters are never
///   escaped).
/// - Otherwise every literal `\E` becomes ESC (0x1B), and a single leading
///   `^X` becomes its control byte (`^?` → DEL 0x7F, else `X - 64`).
fn encode_string_cap(s: &[u8]) -> Vec<u8> {
    // Parameterized strings are returned as-is.
    if s.contains(&b'%') {
        return s.to_vec();
    }
    // Replace every `\E` (backslash, 'E') with ESC.
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'\\' && i + 1 < s.len() && s[i + 1] == b'E' {
            out.push(0x1b);
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    // A leading `^X` is a control char. Upstream only handles this at index 0
    // (it `@compileError`s otherwise), so a single leading conversion suffices.
    if out.first() == Some(&b'^') && out.len() >= 2 {
        let ctrl = if out[1] == b'?' {
            0x7f
        } else {
            out[1].wrapping_sub(64)
        };
        out.splice(0..2, std::iter::once(ctrl));
    }
    out
}

/// Decode an uppercase-hex capability name to its raw ASCII bytes, or `None` if
/// it is not valid even-length hex.
fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for pair in hex.chunks_exact(2) {
        out.push((hex_val(pair[0])? << 4) | hex_val(pair[1])?);
    }
    Some(out)
}

/// A single hex ASCII digit (upper or lower case) to its nibble value.
fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// A single nibble (0-15) to its uppercase-hex ASCII byte.
fn hex_digit(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'A' + (n - 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uppercase-hex-encode a cap name the way a client sends it.
    fn hex(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in name.as_bytes() {
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
        out
    }

    /// The decoded (name, value-bytes) a reply carries, for readable asserts.
    fn reply_value(name: &str) -> Option<Vec<u8>> {
        xtgettcap_value(&hex(name))
    }

    #[test]
    fn terminal_name_is_product_not_ghostty() {
        // TN is the sole deliberate divergence: our product identity.
        assert_eq!(reply_value("TN").unwrap(), b"qwertty-term");
        let resp = xtgettcap_response(&hex("TN")).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1bP1+r544E=");
        for &b in b"qwertty-term" {
            expected.push(hex_digit(b >> 4));
            expected.push(hex_digit(b & 0x0f));
        }
        expected.extend_from_slice(b"\x1b\\");
        assert_eq!(resp, expected);
    }

    #[test]
    fn specials_co_and_rgb() {
        assert_eq!(reply_value("Co").unwrap(), b"256");
        assert_eq!(reply_value("RGB").unwrap(), b"8");
        // Co = "256" hex-encoded is 323536.
        assert_eq!(
            xtgettcap_response(&hex("Co")).unwrap(),
            b"\x1bP1+r436F=323536\x1b\\"
        );
    }

    #[test]
    fn numeric_capability() {
        // `colors` numeric 256 — distinct from the `Co` special (both answered).
        assert_eq!(reply_value("colors").unwrap(), b"256");
        assert_eq!(reply_value("cols").unwrap(), b"80");
        assert_eq!(reply_value("pairs").unwrap(), b"32767");
    }

    #[test]
    fn boolean_capability_has_no_value() {
        // Tc is boolean: present, empty value, so no `=value` in the reply.
        assert_eq!(reply_value("Tc").unwrap(), b"");
        assert_eq!(
            xtgettcap_response(&hex("Tc")).unwrap(),
            b"\x1bP1+r5463\x1b\\"
        );
        assert_eq!(reply_value("am").unwrap(), b"");
        assert_eq!(reply_value("bce").unwrap(), b"");
    }

    #[test]
    fn string_cap_escape_e_becomes_esc() {
        // `clear` = "\E[H\E[2J" (no %) → both \E become ESC (0x1b).
        assert_eq!(reply_value("clear").unwrap(), b"\x1b[H\x1b[2J");
        // `smcup` = "\E[?1049h".
        assert_eq!(reply_value("smcup").unwrap(), b"\x1b[?1049h");
        // `kf1` = "\EOP".
        assert_eq!(reply_value("kf1").unwrap(), b"\x1bOP");
    }

    #[test]
    fn string_cap_leading_caret_becomes_control() {
        // `bel` = "^G" → 0x07.
        assert_eq!(reply_value("bel").unwrap(), b"\x07");
        // `cub1` = "^H" → 0x08.
        assert_eq!(reply_value("cub1").unwrap(), b"\x08");
        // `kbs` = "^?" → DEL 0x7f (special-cased).
        assert_eq!(reply_value("kbs").unwrap(), b"\x7f");
        // `ht` = "^I" → 0x09.
        assert_eq!(reply_value("ht").unwrap(), b"\x09");
    }

    #[test]
    fn parameterized_string_is_verbatim() {
        // `cup` has %-parameters → returned in terminfo source form, \E NOT
        // converted (matches upstream's xterm/kitty verbatim rule).
        assert_eq!(reply_value("cup").unwrap(), b"\\E[%i%p1%d;%p2%dH");
        // `Smulx` = "\E[4:%p1%dm" — has %, stays literal backslash-E.
        assert_eq!(reply_value("Smulx").unwrap(), b"\\E[4:%p1%dm");
        // `Ms` (OSC 52) keeps its literal \E and \007.
        assert_eq!(reply_value("Ms").unwrap(), b"\\E]52;%p1%s;%p2%s\\007");
    }

    #[test]
    fn non_param_string_with_literal_backslash_seq_left_alone() {
        // `cr` = "\r" (literal backslash-r, no % and no \E) → unchanged bytes.
        assert_eq!(reply_value("cr").unwrap(), b"\\r");
        // `ind` = "\n".
        assert_eq!(reply_value("ind").unwrap(), b"\\n");
    }

    #[test]
    fn unknown_capability_is_none() {
        assert_eq!(xtgettcap_response(&hex("ZZ")), None);
        assert_eq!(reply_value("nonesuch"), None);
    }

    #[test]
    fn odd_length_hex_is_none() {
        assert_eq!(xtgettcap_response(b"54E"), None);
        assert_eq!(xtgettcap_response(b"XYZ"), None);
    }

    #[test]
    fn full_table_encodes_without_panic() {
        // Every capability must produce a well-formed reply.
        for (name, _) in CAPABILITIES {
            let resp = xtgettcap_response(&{
                let mut h = Vec::new();
                for &b in *name {
                    h.push(hex_digit(b >> 4));
                    h.push(hex_digit(b & 0x0f));
                }
                h
            });
            assert!(
                resp.is_some(),
                "cap {:?} produced no reply",
                String::from_utf8_lossy(name)
            );
            let resp = resp.unwrap();
            assert!(resp.starts_with(b"\x1bP1+r"));
            assert!(resp.ends_with(b"\x1b\\"));
        }
        // Sanity: the whole ghostty table plus 3 specials.
        assert_eq!(CAPABILITIES.len(), 268);
    }
}
