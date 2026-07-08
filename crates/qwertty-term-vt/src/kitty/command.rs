//! Kitty graphics command grammar (port of `graphics_command.zig`, commit `2da015cd6`).
//!
//! Parses the key=value control payload of a kitty graphics APC sequence
//! (`ESC _ G <control> ; <base64-payload> ESC \`) into a typed [`Command`] tree, and encodes
//! a [`Response`]. The parser is fed the bytes immediately following the `G`.

use std::collections::HashMap;
use std::fmt::Write as _;

use base64::Engine as _;

/// The key-value pairs for the control information for a command. Keys are always single
/// characters; values are either a single printable ASCII byte or a 32-bit integer (stored as
/// `u32`, bitcast from `i32` for the signed keys `z`/`H`/`V`). Port of the `KV` AutoHashMap.
type Kv = HashMap<u8, u32>;

/// Errors the command parser and command-tree builders can produce. Mirrors the Zig error set
/// used across `parse`/`complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Malformed control data (e.g. ended mid-key, or an out-of-range enum value).
    InvalidFormat,
    /// Payload base64 failed to decode.
    InvalidData,
    /// The data payload exceeded `max_bytes`.
    OutOfMemory,
    /// An integer value overflowed its target type.
    Overflow,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Error::InvalidFormat => "invalid format",
            Error::InvalidData => "invalid data",
            Error::OutOfMemory => "out of memory",
            Error::Overflow => "integer overflow",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}

/// Internal parser state. The `_ignore` variants are in the same phase but drop bytes because
/// we know the current KV pair is invalid. Port of `Parser.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    ControlKey,
    ControlKeyIgnore,
    ControlValue,
    ControlValueIgnore,
    Data,
}

/// Command parser: a byte-at-a-time state machine over the kitty graphics control payload.
/// Port of `graphics_command.Parser`.
pub struct Parser {
    kv: Kv,
    /// Scratch for the key/value currently being accumulated. Max u32 is 10 digits plus a
    /// sign, so 11 bytes.
    kv_temp: [u8; 11],
    kv_temp_len: usize,
    kv_current: u8,
    /// Raw (still base64-encoded) payload bytes.
    data: Vec<u8>,
    max_bytes: usize,
    state: State,
}

impl Parser {
    /// Initialize the parser with a payload byte cap. Port of `Parser.init`.
    pub fn new(max_bytes: usize) -> Parser {
        Parser {
            kv: HashMap::new(),
            kv_temp: [0; 11],
            kv_temp_len: 0,
            kv_current: 0,
            data: Vec::new(),
            max_bytes,
            state: State::ControlKey,
        }
    }

    /// Parse a complete command string. Port of `Parser.parseString`.
    pub fn parse_string(data: &[u8]) -> Result<Command, Error> {
        let mut parser = Parser::new(1024 * 1024);
        for &c in data {
            parser.feed(c)?;
        }
        parser.complete()
    }

    /// Feed a single byte. The first byte should be the one immediately following the `G` in
    /// the APC sequence. Port of `Parser.feed`.
    pub fn feed(&mut self, c: u8) -> Result<(), Error> {
        match self.state {
            State::ControlKey => match c {
                // '=' finishes the key and moves to the value (only if a single char).
                b'=' => {
                    if self.kv_temp_len != 1 {
                        self.state = State::ControlValueIgnore;
                        self.kv_temp_len = 0;
                    } else {
                        self.kv_current = self.kv_temp[0];
                        self.kv_temp_len = 0;
                        self.state = State::ControlValue;
                    }
                }
                // ';' with no control data means payload-only ("ESC_G;<data>"), valid per kitty.
                b';' => self.state = State::Data,
                _ => self.accumulate_value(c, State::ControlKeyIgnore),
            },

            State::ControlKeyIgnore => {
                if c == b'=' {
                    self.state = State::ControlValueIgnore;
                }
            }

            State::ControlValue => match c {
                b',' => self.finish_value(State::ControlKey)?, // next key
                b';' => self.finish_value(State::Data)?,       // move to data
                _ => self.accumulate_value(c, State::ControlValueIgnore),
            },

            State::ControlValueIgnore => match c {
                b',' => self.state = State::ControlKeyIgnore,
                b';' => self.state = State::Data,
                _ => {}
            },

            State::Data => {
                if self.data.len() >= self.max_bytes {
                    return Err(Error::OutOfMemory);
                }
                self.data.push(c);
            }
        }
        Ok(())
    }

    /// Complete parsing after all bytes have been fed. Port of `Parser.complete`.
    pub fn complete(&mut self) -> Result<Command, Error> {
        match self.state {
            // Ending in a key state is never valid (e.g. "a=1,b").
            State::ControlKey | State::ControlKeyIgnore => return Err(Error::InvalidFormat),
            // Commands like placements end in the value state (e.g. "a=1,b=2").
            State::ControlValue => self.finish_value(State::Data)?,
            State::ControlValueIgnore => {}
            State::Data => {}
        }

        // The action key is always a single character; default 't'.
        let action: u8 = match self.kv.get(&b'a') {
            None => b't',
            Some(&value) => u8::try_from(value).map_err(|_| Error::InvalidFormat)?,
        };

        let control = match action {
            b'q' => Control::Query(Transmission::parse(&self.kv)?),
            b't' => Control::Transmit(Transmission::parse(&self.kv)?),
            b'T' => Control::TransmitAndDisplay {
                transmission: Transmission::parse(&self.kv)?,
                display: Display::parse(&self.kv)?,
            },
            b'p' => Control::Display(Display::parse(&self.kv)?),
            b'd' => Control::Delete(Delete::parse(&self.kv)?),
            b'f' => Control::TransmitAnimationFrame(AnimationFrameLoading::parse(&self.kv)?),
            b'a' => Control::ControlAnimation(AnimationControl::parse(&self.kv)?),
            b'c' => Control::ComposeAnimation(AnimationFrameComposition::parse(&self.kv)?),
            _ => return Err(Error::InvalidFormat),
        };

        let quiet = match self.kv.get(&b'q') {
            Some(&v) => match v {
                0 => Quiet::No,
                1 => Quiet::Ok,
                2 => Quiet::Failures,
                _ => return Err(Error::InvalidFormat),
            },
            None => Quiet::No,
        };

        let data = self.decode_data()?;

        Ok(Command {
            control,
            quiet,
            data,
        })
    }

    /// Decode the base64 payload. Port of `Parser.decodeData`.
    ///
    /// Zig decodes in-place on top of `self.data` (encoded size >= decoded size); the `base64`
    /// crate's slice API can't safely alias input and output, so we decode into a fresh `Vec`.
    /// Behavior-identical.
    fn decode_data(&mut self) -> Result<Vec<u8>, Error> {
        if self.data.is_empty() {
            return Ok(Vec::new());
        }
        // Zig uses `std.base64.standard.Decoder`, which does not validate the
        // trailing bits of the final partial group (it simply shifts them out).
        // The `base64` crate's `STANDARD_NO_PAD` engine rejects a non-zero
        // trailing-bit remainder by default, which would reject valid kitty
        // payloads such as an 8-byte RGBA image encoded as 11 `/` characters
        // (`graphics_exec.zig` "default format is rgba" test). Configure the
        // engine to permit trailing bits so the decode matches Zig exactly.
        use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
        let config = GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true);
        let engine = GeneralPurpose::new(&base64::alphabet::STANDARD, config);
        engine.decode(&self.data).map_err(|_| Error::InvalidData)
    }

    /// Accumulate a byte into `kv_temp`; on overflow drop to `overflow_state`. Port of
    /// `Parser.accumulateValue`.
    fn accumulate_value(&mut self, c: u8, overflow_state: State) {
        let idx = self.kv_temp_len;
        self.kv_temp_len += 1;
        if self.kv_temp_len > self.kv_temp.len() {
            self.state = overflow_state;
            self.kv_temp_len = 0;
            return;
        }
        self.kv_temp[idx] = c;
    }

    /// Finish the current value and store it. Port of `Parser.finishValue`.
    fn finish_value(&mut self, next_state: State) -> Result<(), Error> {
        self.state = next_state;

        // Single non-digit ASCII char: store its byte directly.
        if self.kv_temp_len == 1 {
            let c = self.kv_temp[0];
            if !c.is_ascii_digit() {
                self.kv.insert(self.kv_current, c as u32);
                self.kv_temp_len = 0;
                return Ok(());
            }
        }

        let s = std::str::from_utf8(&self.kv_temp[0..self.kv_temp_len])
            .map_err(|_| Error::InvalidFormat)?;
        // Signed fields are parsed as i32 then bitcast to u32; the rest as u32.
        let v: u32 = match self.kv_current {
            b'z' | b'H' | b'V' => s.parse::<i32>().map_err(map_int_err)? as u32,
            _ => s.parse::<u32>().map_err(map_int_err)?,
        };
        self.kv.insert(self.kv_current, v);
        self.kv_temp_len = 0;
        Ok(())
    }
}

/// Map a Rust `ParseIntError` to the Zig-equivalent error. Out-of-range → `Overflow`, anything
/// else (empty/invalid digit) → `InvalidFormat`. Zig's `parseInt` returns `error.Overflow`
/// specifically for range violations, which several tests assert on.
fn map_int_err(e: std::num::ParseIntError) -> Error {
    use std::num::IntErrorKind;
    match e.kind() {
        IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => Error::Overflow,
        _ => Error::InvalidFormat,
    }
}

/// A possible response to a command. Port of `graphics_command.Response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub id: u32,
    pub image_number: u32,
    pub placement_id: u32,
    pub message: String,
}

impl Default for Response {
    fn default() -> Self {
        Response {
            id: 0,
            image_number: 0,
            placement_id: 0,
            message: "OK".to_string(),
        }
    }
}

impl Response {
    /// Encode into `ESC_G...;<message>ESC\`. Emits nothing unless id or image_number is set.
    /// Port of `Response.encode`.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        // Only encode a result if we have either an id or an image number.
        if self.id == 0 && self.image_number == 0 {
            return out;
        }

        let mut prior = false;
        out.push_str("\x1b_G");
        if self.id > 0 {
            prior = true;
            let _ = write!(out, "i={}", self.id);
        }
        if self.image_number > 0 {
            if prior {
                out.push(',');
            } else {
                prior = true;
            }
            let _ = write!(out, "I={}", self.image_number);
        }
        if self.placement_id > 0 {
            if prior {
                out.push(',');
            }
            let _ = write!(out, "p={}", self.placement_id);
        }
        out.push(';');
        out.push_str(&self.message);
        out.push_str("\x1b\\");
        out
    }

    /// True if this response is not an error. Port of `Response.ok`.
    pub fn ok(&self) -> bool {
        self.message == "OK"
    }

    /// True if this response is empty (no id and no image number). Port of `Response.empty`.
    pub fn is_empty(&self) -> bool {
        self.id == 0 && self.image_number == 0
    }
}

/// A parsed kitty graphics command. Port of `graphics_command.Command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub control: Control,
    pub quiet: Quiet,
    pub data: Vec<u8>,
}

impl Command {
    /// The transmission data if this command carries any. Port of `Command.transmission`.
    pub fn transmission(&self) -> Option<Transmission> {
        match &self.control {
            Control::Query(t) => Some(*t),
            Control::Transmit(t) => Some(*t),
            Control::TransmitAndDisplay { transmission, .. } => Some(*transmission),
            _ => None,
        }
    }

    /// The display data if this command carries any. Port of `Command.display`.
    pub fn display(&self) -> Option<Display> {
        match &self.control {
            Control::Display(d) => Some(*d),
            Control::TransmitAndDisplay { display, .. } => Some(*display),
            _ => None,
        }
    }

    /// Take ownership of the payload data, leaving the command's data empty. Port of
    /// `Command.toOwnedData`.
    pub fn to_owned_data(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.data)
    }
}

/// The action taken by a command. Port of `Command.Action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Query,
    Transmit,
    TransmitAndDisplay,
    Display,
    Delete,
    TransmitAnimationFrame,
    ControlAnimation,
    ComposeAnimation,
}

/// The quiet mode: which responses to suppress. Port of `Command.Quiet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// `q=0`: respond normally.
    No,
    /// `q=1`: suppress success responses.
    Ok,
    /// `q=2`: suppress all responses.
    Failures,
}

/// The per-action typed control payload. Port of `Command.Control`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Query(Transmission),
    Transmit(Transmission),
    TransmitAndDisplay {
        transmission: Transmission,
        display: Display,
    },
    Display(Display),
    Delete(Delete),
    TransmitAnimationFrame(AnimationFrameLoading),
    ControlAnimation(AnimationControl),
    ComposeAnimation(AnimationFrameComposition),
}

/// Image pixel format. Port of `Transmission.Format`. The `gray`/`gray_alpha` variants are not
/// transmitted directly but are formats a PNG may decode to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rgb,
    Rgba,
    Png,
    GrayAlpha,
    Gray,
}

/// Transmission medium. Port of `Transmission.Medium`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    Direct,
    File,
    TemporaryFile,
    SharedMemory,
}

/// Payload compression. Port of `Transmission.Compression`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    ZlibDeflate,
}

/// Image transmission parameters. Port of `graphics_command.Transmission`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transmission {
    pub format: Format,           // f
    pub medium: Medium,           // t
    pub width: u32,               // s
    pub height: u32,              // v
    pub size: u32,                // S
    pub offset: u32,              // O
    pub image_id: u32,            // i
    pub image_number: u32,        // I
    pub placement_id: u32,        // p
    pub compression: Compression, // o
    pub more_chunks: bool,        // m
}

impl Default for Transmission {
    fn default() -> Self {
        Transmission {
            format: Format::Rgba,
            medium: Medium::Direct,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            compression: Compression::None,
            more_chunks: false,
        }
    }
}

impl Transmission {
    /// Bytes-per-pixel for a format. Panics on `Png` (must be decoded first). Port of
    /// `Transmission.formatBpp`.
    pub fn format_bpp(format: Format) -> u8 {
        match format {
            Format::Gray => 1,
            Format::GrayAlpha => 2,
            Format::Rgb => 3,
            Format::Rgba => 4,
            Format::Png => unreachable!("png must be validated/decoded before formatBpp"),
        }
    }

    fn parse(kv: &Kv) -> Result<Transmission, Error> {
        let mut result = Transmission::default();
        if let Some(&v) = kv.get(&b'f') {
            result.format = match v {
                24 => Format::Rgb,
                32 => Format::Rgba,
                100 => Format::Png,
                _ => return Err(Error::InvalidFormat),
            };
        }
        if let Some(&v) = kv.get(&b't') {
            let c = u8::try_from(v).map_err(|_| Error::InvalidFormat)?;
            result.medium = match c {
                b'd' => Medium::Direct,
                b'f' => Medium::File,
                b't' => Medium::TemporaryFile,
                b's' => Medium::SharedMemory,
                _ => return Err(Error::InvalidFormat),
            };
        }
        if let Some(&v) = kv.get(&b's') {
            result.width = v;
        }
        if let Some(&v) = kv.get(&b'v') {
            result.height = v;
        }
        if let Some(&v) = kv.get(&b'S') {
            result.size = v;
        }
        if let Some(&v) = kv.get(&b'O') {
            result.offset = v;
        }
        if let Some(&v) = kv.get(&b'i') {
            result.image_id = v;
        }
        if let Some(&v) = kv.get(&b'I') {
            result.image_number = v;
        }
        if let Some(&v) = kv.get(&b'p') {
            result.placement_id = v;
        }
        if let Some(&v) = kv.get(&b'o') {
            let c = u8::try_from(v).map_err(|_| Error::InvalidFormat)?;
            result.compression = match c {
                b'z' => Compression::ZlibDeflate,
                _ => return Err(Error::InvalidFormat),
            };
        }

        // The 'm' key is only honored for the direct medium. Kitty implements this and mpv
        // relies on it for shared-memory transfers; the spec only mentions 'm' for remote
        // clients. See graphics_command.zig:497-510.
        if result.medium == Medium::Direct
            && let Some(&v) = kv.get(&b'm')
        {
            result.more_chunks = v > 0;
        }

        Ok(result)
    }
}

/// How the cursor moves after a display. Port of `Display.CursorMovement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMovement {
    After,
    None,
}

/// Image display (placement) parameters. Port of `graphics_command.Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Display {
    pub image_id: u32,                          // i
    pub image_number: u32,                      // I
    pub placement_id: u32,                      // p
    pub x: u32,                                 // x
    pub y: u32,                                 // y
    pub width: u32,                             // w
    pub height: u32,                            // h
    pub x_offset: u32,                          // X
    pub y_offset: u32,                          // Y
    pub columns: u32,                           // c
    pub rows: u32,                              // r
    pub cursor_movement: CursorMovementDefault, // C
    pub virtual_placement: bool,                // U
    pub parent_id: u32,                         // P
    pub parent_placement_id: u32,               // Q
    pub horizontal_offset: i32,                 // H
    pub vertical_offset: i32,                   // V
    pub z: i32,                                 // z
}

/// Newtype so `Display` can derive `Default` (default cursor movement is `After`, not the
/// enum's first variant convention). Purely a Rust-idiom wrapper; not in Zig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorMovementDefault(pub CursorMovement);

impl Default for CursorMovementDefault {
    fn default() -> Self {
        CursorMovementDefault(CursorMovement::After)
    }
}

impl Display {
    fn parse(kv: &Kv) -> Result<Display, Error> {
        let mut result = Display::default();
        if let Some(&v) = kv.get(&b'i') {
            result.image_id = v;
        }
        if let Some(&v) = kv.get(&b'I') {
            result.image_number = v;
        }
        if let Some(&v) = kv.get(&b'p') {
            result.placement_id = v;
        }
        if let Some(&v) = kv.get(&b'x') {
            result.x = v;
        }
        if let Some(&v) = kv.get(&b'y') {
            result.y = v;
        }
        if let Some(&v) = kv.get(&b'w') {
            result.width = v;
        }
        if let Some(&v) = kv.get(&b'h') {
            result.height = v;
        }
        if let Some(&v) = kv.get(&b'X') {
            result.x_offset = v;
        }
        if let Some(&v) = kv.get(&b'Y') {
            result.y_offset = v;
        }
        if let Some(&v) = kv.get(&b'c') {
            result.columns = v;
        }
        if let Some(&v) = kv.get(&b'r') {
            result.rows = v;
        }
        if let Some(&v) = kv.get(&b'C') {
            result.cursor_movement = CursorMovementDefault(match v {
                0 => CursorMovement::After,
                1 => CursorMovement::None,
                _ => return Err(Error::InvalidFormat),
            });
        }
        if let Some(&v) = kv.get(&b'U') {
            result.virtual_placement = match v {
                0 => false,
                1 => true,
                _ => return Err(Error::InvalidFormat),
            };
        }
        if let Some(&v) = kv.get(&b'z') {
            result.z = v as i32; // parsed as i32 earlier, bitcast back
        }
        if let Some(&v) = kv.get(&b'P') {
            result.parent_id = v;
        }
        if let Some(&v) = kv.get(&b'Q') {
            result.parent_placement_id = v;
        }
        if let Some(&v) = kv.get(&b'H') {
            result.horizontal_offset = v as i32;
        }
        if let Some(&v) = kv.get(&b'V') {
            result.vertical_offset = v as i32;
        }
        Ok(result)
    }
}

/// Composition mode for animation frames. Port of `graphics_command.CompositionMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionMode {
    AlphaBlend,
    Overwrite,
}

/// Background color for an animation frame. Port of `AnimationFrameLoading.Background`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Background {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Background {
    /// Unpack from the packed u32 (r=low byte). Port of `@bitCast(u32)`.
    fn from_u32(v: u32) -> Background {
        Background {
            r: (v & 0xff) as u8,
            g: ((v >> 8) & 0xff) as u8,
            b: ((v >> 16) & 0xff) as u8,
            a: ((v >> 24) & 0xff) as u8,
        }
    }
}

/// Animation frame loading (`f` action). Port of `graphics_command.AnimationFrameLoading`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnimationFrameLoading {
    pub x: u32,                                   // x
    pub y: u32,                                   // y
    pub create_frame: u32,                        // c
    pub edit_frame: u32,                          // r
    pub gap_ms: u32,                              // z
    pub composition_mode: CompositionModeDefault, // X
    pub background: Background,                   // Y
}

/// Newtype default wrapper (default is `AlphaBlend`). Not in Zig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositionModeDefault(pub CompositionMode);

impl Default for CompositionModeDefault {
    fn default() -> Self {
        CompositionModeDefault(CompositionMode::AlphaBlend)
    }
}

impl AnimationFrameLoading {
    fn parse(kv: &Kv) -> Result<AnimationFrameLoading, Error> {
        let mut result = AnimationFrameLoading::default();
        if let Some(&v) = kv.get(&b'x') {
            result.x = v;
        }
        if let Some(&v) = kv.get(&b'y') {
            result.y = v;
        }
        if let Some(&v) = kv.get(&b'c') {
            result.create_frame = v;
        }
        if let Some(&v) = kv.get(&b'r') {
            result.edit_frame = v;
        }
        if let Some(&v) = kv.get(&b'z') {
            result.gap_ms = v;
        }
        if let Some(&v) = kv.get(&b'X') {
            result.composition_mode = CompositionModeDefault(match v {
                0 => CompositionMode::AlphaBlend,
                1 => CompositionMode::Overwrite,
                _ => return Err(Error::InvalidFormat),
            });
        }
        if let Some(&v) = kv.get(&b'Y') {
            result.background = Background::from_u32(v);
        }
        Ok(result)
    }
}

/// Animation frame composition (`c` action). Port of `graphics_command.AnimationFrameComposition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnimationFrameComposition {
    pub frame: u32,                               // c
    pub edit_frame: u32,                          // r
    pub x: u32,                                   // x
    pub y: u32,                                   // y
    pub width: u32,                               // w
    pub height: u32,                              // h
    pub left_edge: u32,                           // X
    pub top_edge: u32,                            // Y
    pub composition_mode: CompositionModeDefault, // C
}

impl AnimationFrameComposition {
    fn parse(kv: &Kv) -> Result<AnimationFrameComposition, Error> {
        let mut result = AnimationFrameComposition::default();
        if let Some(&v) = kv.get(&b'c') {
            result.frame = v;
        }
        if let Some(&v) = kv.get(&b'r') {
            result.edit_frame = v;
        }
        if let Some(&v) = kv.get(&b'x') {
            result.x = v;
        }
        if let Some(&v) = kv.get(&b'y') {
            result.y = v;
        }
        if let Some(&v) = kv.get(&b'w') {
            result.width = v;
        }
        if let Some(&v) = kv.get(&b'h') {
            result.height = v;
        }
        if let Some(&v) = kv.get(&b'X') {
            result.left_edge = v;
        }
        if let Some(&v) = kv.get(&b'Y') {
            result.top_edge = v;
        }
        if let Some(&v) = kv.get(&b'C') {
            result.composition_mode = CompositionModeDefault(match v {
                0 => CompositionMode::AlphaBlend,
                1 => CompositionMode::Overwrite,
                _ => return Err(Error::InvalidFormat),
            });
        }
        Ok(result)
    }
}

/// Animation control action. Port of `AnimationControl.AnimationAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationAction {
    Invalid,
    Stop,
    RunWait,
    Run,
}

/// Animation control (`a` action). Port of `graphics_command.AnimationControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationControl {
    pub action: AnimationAction, // s
    pub frame: u32,              // r
    pub gap_ms: u32,             // z
    pub current_frame: u32,      // c
    pub loops: u32,              // v
}

impl Default for AnimationControl {
    fn default() -> Self {
        AnimationControl {
            action: AnimationAction::Invalid,
            frame: 0,
            gap_ms: 0,
            current_frame: 0,
            loops: 0,
        }
    }
}

impl AnimationControl {
    fn parse(kv: &Kv) -> Result<AnimationControl, Error> {
        let mut result = AnimationControl::default();
        if let Some(&v) = kv.get(&b's') {
            result.action = match v {
                0 => AnimationAction::Invalid,
                1 => AnimationAction::Stop,
                2 => AnimationAction::RunWait,
                3 => AnimationAction::Run,
                _ => return Err(Error::InvalidFormat),
            };
        }
        if let Some(&v) = kv.get(&b'r') {
            result.frame = v;
        }
        if let Some(&v) = kv.get(&b'z') {
            result.gap_ms = v;
        }
        if let Some(&v) = kv.get(&b'c') {
            result.current_frame = v;
        }
        if let Some(&v) = kv.get(&b'v') {
            result.loops = v;
        }
        Ok(result)
    }
}

/// Delete command. The `delete` flag on each variant (uppercase key) means "also delete the
/// underlying image data if unused", not just the placement. Port of `graphics_command.Delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delete {
    /// a/A
    All(bool),
    /// i/I
    Id {
        delete: bool,
        image_id: u32,
        placement_id: u32,
    },
    /// n/N
    Newest {
        delete: bool,
        image_number: u32,
        placement_id: u32,
    },
    /// c/C
    IntersectCursor(bool),
    /// f/F
    AnimationFrames(bool),
    /// p/P
    IntersectCell { delete: bool, x: u32, y: u32 },
    /// q/Q
    IntersectCellZ {
        delete: bool,
        x: u32,
        y: u32,
        z: i32,
    },
    /// r/R
    Range { delete: bool, first: u32, last: u32 },
    /// x/X
    Column { delete: bool, x: u32 },
    /// y/Y
    Row { delete: bool, y: u32 },
    /// z/Z
    Z { delete: bool, z: i32 },
}

impl Delete {
    fn parse(kv: &Kv) -> Result<Delete, Error> {
        let what: u8 = match kv.get(&b'd') {
            None => b'a',
            Some(&value) => u8::try_from(value).map_err(|_| Error::InvalidFormat)?,
        };

        let result = match what {
            b'a' | b'A' => Delete::All(what == b'A'),

            b'i' | b'I' => {
                let mut image_id = 0;
                let mut placement_id = 0;
                if let Some(&v) = kv.get(&b'i') {
                    image_id = v;
                }
                if let Some(&v) = kv.get(&b'p') {
                    placement_id = v;
                }
                Delete::Id {
                    delete: what == b'I',
                    image_id,
                    placement_id,
                }
            }

            b'n' | b'N' => {
                let mut image_number = 0;
                let mut placement_id = 0;
                if let Some(&v) = kv.get(&b'I') {
                    image_number = v;
                }
                if let Some(&v) = kv.get(&b'p') {
                    placement_id = v;
                }
                Delete::Newest {
                    delete: what == b'N',
                    image_number,
                    placement_id,
                }
            }

            b'c' | b'C' => Delete::IntersectCursor(what == b'C'),

            b'f' | b'F' => Delete::AnimationFrames(what == b'F'),

            b'p' | b'P' => {
                let mut x = 0;
                let mut y = 0;
                if let Some(&v) = kv.get(&b'x') {
                    x = v;
                }
                if let Some(&v) = kv.get(&b'y') {
                    y = v;
                }
                Delete::IntersectCell {
                    delete: what == b'P',
                    x,
                    y,
                }
            }

            b'q' | b'Q' => {
                let mut x = 0;
                let mut y = 0;
                let mut z = 0;
                if let Some(&v) = kv.get(&b'x') {
                    x = v;
                }
                if let Some(&v) = kv.get(&b'y') {
                    y = v;
                }
                if let Some(&v) = kv.get(&b'z') {
                    z = v as i32;
                }
                Delete::IntersectCellZ {
                    delete: what == b'Q',
                    x,
                    y,
                    z,
                }
            }

            b'r' | b'R' => {
                let x = *kv.get(&b'x').ok_or(Error::InvalidFormat)?;
                let y = *kv.get(&b'y').ok_or(Error::InvalidFormat)?;
                if x > y {
                    return Err(Error::InvalidFormat);
                }
                Delete::Range {
                    delete: what == b'R',
                    first: x,
                    last: y,
                }
            }

            b'x' | b'X' => {
                let mut x = 0;
                if let Some(&v) = kv.get(&b'x') {
                    x = v;
                }
                Delete::Column {
                    delete: what == b'X',
                    x,
                }
            }

            b'y' | b'Y' => {
                let mut y = 0;
                if let Some(&v) = kv.get(&b'y') {
                    y = v;
                }
                Delete::Row {
                    delete: what == b'Y',
                    y,
                }
            }

            b'z' | b'Z' => {
                let mut z = 0;
                if let Some(&v) = kv.get(&b'z') {
                    z = v as i32;
                }
                Delete::Z {
                    delete: what == b'Z',
                    z,
                }
            }

            _ => return Err(Error::InvalidFormat),
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<Command, Error> {
        let mut p = Parser::new(1024 * 1024);
        for &c in input.as_bytes() {
            p.feed(c)?;
        }
        p.complete()
    }

    #[test]
    fn transmission_command() {
        let command = parse("f=24,s=10,v=20").unwrap();
        let Control::Transmit(v) = command.control else {
            panic!("expected transmit");
        };
        assert_eq!(v.format, Format::Rgb);
        assert_eq!(v.width, 10);
        assert_eq!(v.height, 20);
    }

    #[test]
    fn transmission_ignores_m_if_medium_is_not_direct() {
        let command = parse("a=t,t=t,m=1").unwrap();
        let Control::Transmit(v) = command.control else {
            panic!("expected transmit");
        };
        assert_eq!(v.medium, Medium::TemporaryFile);
        assert!(!v.more_chunks);
    }

    #[test]
    fn transmission_respects_m_if_medium_is_direct() {
        let command = parse("a=t,t=d,m=1").unwrap();
        let Control::Transmit(v) = command.control else {
            panic!("expected transmit");
        };
        assert_eq!(v.medium, Medium::Direct);
        assert!(v.more_chunks);
    }

    #[test]
    fn query_command() {
        let command = parse("i=31,s=1,v=1,a=q,t=d,f=24;QUFBQQ").unwrap();
        let Control::Query(v) = command.control else {
            panic!("expected query");
        };
        assert_eq!(v.medium, Medium::Direct);
        assert_eq!(v.width, 1);
        assert_eq!(v.height, 1);
        assert_eq!(v.image_id, 31);
        assert_eq!(command.data, b"AAAA");
    }

    #[test]
    fn display_command() {
        let command = parse("a=p,U=1,i=31,c=80,r=120").unwrap();
        let Control::Display(v) = command.control else {
            panic!("expected display");
        };
        assert_eq!(v.columns, 80);
        assert_eq!(v.rows, 120);
        assert_eq!(v.image_id, 31);
    }

    #[test]
    fn delete_command() {
        let command = parse("a=d,d=p,x=3,y=4").unwrap();
        let Control::Delete(v) = command.control else {
            panic!("expected delete");
        };
        let Delete::IntersectCell { delete, x, y } = v else {
            panic!("expected intersect_cell");
        };
        assert!(!delete);
        assert_eq!(x, 3);
        assert_eq!(y, 4);
    }

    #[test]
    fn no_control_data() {
        let command = parse(";QUFBQQ").unwrap();
        assert!(matches!(command.control, Control::Transmit(_)));
        assert_eq!(command.data, b"AAAA");
    }

    #[test]
    fn ignore_unknown_keys_long() {
        let command = parse("f=24,s=10,v=20,hello=world").unwrap();
        let Control::Transmit(v) = command.control else {
            panic!("expected transmit");
        };
        assert_eq!(v.format, Format::Rgb);
        assert_eq!(v.width, 10);
        assert_eq!(v.height, 20);
    }

    #[test]
    fn ignore_very_long_values() {
        let command = parse("f=24,s=10,v=2000000000000000000000000000000000000000").unwrap();
        let Control::Transmit(v) = command.control else {
            panic!("expected transmit");
        };
        assert_eq!(v.format, Format::Rgb);
        assert_eq!(v.width, 10);
        assert_eq!(v.height, 0);
    }

    #[test]
    fn ensure_very_large_negative_values_dont_get_skipped() {
        let command = parse("a=p,i=1,z=-2000000000").unwrap();
        let Control::Display(v) = command.control else {
            panic!("expected display");
        };
        assert_eq!(v.image_id, 1);
        assert_eq!(v.z, -2000000000);
    }

    #[test]
    fn ensure_proper_overflow_error_for_u32() {
        let mut p = Parser::new(1024 * 1024);
        for &c in b"a=p,i=10000000000" {
            p.feed(c).unwrap();
        }
        assert_eq!(p.complete().unwrap_err(), Error::Overflow);
    }

    #[test]
    fn ensure_proper_overflow_error_for_i32() {
        let mut p = Parser::new(1024 * 1024);
        for &c in b"a=p,i=1,z=-9999999999" {
            p.feed(c).unwrap();
        }
        assert_eq!(p.complete().unwrap_err(), Error::Overflow);
    }

    #[test]
    fn all_i32_values() {
        // 'z' (z-axis)
        {
            let command = parse("a=p,i=1,z=-1").unwrap();
            let Control::Display(v) = command.control else {
                panic!("expected display");
            };
            assert_eq!(v.image_id, 1);
            assert_eq!(v.z, -1);
        }
        // 'H' (horizontal offset)
        {
            let command = parse("a=p,i=1,H=-1").unwrap();
            let Control::Display(v) = command.control else {
                panic!("expected display");
            };
            assert_eq!(v.image_id, 1);
            assert_eq!(v.horizontal_offset, -1);
        }
        // 'V' (vertical offset)
        {
            let command = parse("a=p,i=1,V=-1").unwrap();
            let Control::Display(v) = command.control else {
                panic!("expected display");
            };
            assert_eq!(v.image_id, 1);
            assert_eq!(v.vertical_offset, -1);
        }
    }

    #[test]
    fn response_encode_nothing_without_id_or_image_number() {
        let r = Response::default();
        assert_eq!(r.encode(), "");
    }

    #[test]
    fn response_encode_with_only_image_id() {
        let r = Response {
            id: 4,
            ..Default::default()
        };
        assert_eq!(r.encode(), "\x1b_Gi=4;OK\x1b\\");
    }

    #[test]
    fn response_encode_with_only_image_number() {
        let r = Response {
            image_number: 4,
            ..Default::default()
        };
        assert_eq!(r.encode(), "\x1b_GI=4;OK\x1b\\");
    }

    #[test]
    fn response_encode_with_image_id_and_number() {
        let r = Response {
            id: 12,
            image_number: 4,
            ..Default::default()
        };
        assert_eq!(r.encode(), "\x1b_Gi=12,I=4;OK\x1b\\");
    }

    #[test]
    fn delete_range_command_1() {
        let command = parse("a=d,d=r,x=3,y=4").unwrap();
        let Control::Delete(Delete::Range {
            delete,
            first,
            last,
        }) = command.control
        else {
            panic!("expected delete range");
        };
        assert!(!delete);
        assert_eq!(first, 3);
        assert_eq!(last, 4);
    }

    #[test]
    fn delete_range_command_2() {
        let command = parse("a=d,d=R,x=5,y=11").unwrap();
        let Control::Delete(Delete::Range {
            delete,
            first,
            last,
        }) = command.control
        else {
            panic!("expected delete range");
        };
        assert!(delete);
        assert_eq!(first, 5);
        assert_eq!(last, 11);
    }

    #[test]
    fn delete_range_command_3() {
        let mut p = Parser::new(1024 * 1024);
        for &c in b"a=d,d=R,x=5,y=4" {
            p.feed(c).unwrap();
        }
        assert_eq!(p.complete().unwrap_err(), Error::InvalidFormat);
    }

    #[test]
    fn delete_range_command_4() {
        let mut p = Parser::new(1024 * 1024);
        for &c in b"a=d,d=R,x=5" {
            p.feed(c).unwrap();
        }
        assert_eq!(p.complete().unwrap_err(), Error::InvalidFormat);
    }

    #[test]
    fn delete_range_command_5() {
        let mut p = Parser::new(1024 * 1024);
        for &c in b"a=d,d=R,y=5" {
            p.feed(c).unwrap();
        }
        assert_eq!(p.complete().unwrap_err(), Error::InvalidFormat);
    }
}
