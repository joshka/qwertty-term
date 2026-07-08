//! Kitty graphics command execution against a live [`Terminal`] (port of
//! `src/terminal/kitty/graphics_exec.zig`, commit `2da015cd6`).
//!
//! [`execute`] is the top of the subsystem: it takes a fully-parsed
//! [`command::Command`] (produced by [`command::Parser`] from the APC payload)
//! and applies it to the terminal's active-screen [`ImageStorage`], returning an
//! optional [`Response`] that the stream handler writes back out the reply queue.
//!
//! This is the layer that couples the renderer-independent kitty *model*
//! ([`command`]/[`image`]/[`storage`](super::storage)) to the live `Terminal`:
//! it advances the cursor for `T`/`p` placements, tracks placement pins against
//! the active screen's [`crate::pagelist::PageList`], carries the `q`
//! (quiet-mode) inheritance rule across chunked transmissions, and applies the
//! quiet filter to decide whether a response is emitted.
//!
//! # Decoder / medium seams
//!
//! Upstream calls into a linked PNG decoder (wuffs) and reads file/shm media
//! from the OS. This port keeps both behind seams: [`execute`] uses
//! [`image::NoDecoder`] (no zlib/png) and rejects all non-`direct` media, which
//! matches the default `image_limits = .direct` (all path-media disabled) that
//! the model ships with. A future integration can call
//! [`execute_with`] with a real decoder and medium reader.

use super::command::{self, Command, Response};
use super::image::{self, Image, ImageDecoder, LoadingImage};
use super::storage::{Location, Placement};
use crate::terminal::Terminal;

/// Execute a kitty graphics command against the terminal using the default
/// (no-op) decoder and a medium reader that rejects all non-direct media. Port
/// of `graphics_exec.execute`.
///
/// This never fails; the returned [`Response`], if any, may carry an error
/// message but the terminal is always left in a recoverable state.
pub fn execute(terminal: &mut Terminal, cmd: &Command) -> Option<Response> {
    execute_with(terminal, cmd, &image::NoDecoder, reject_non_direct)
}

/// A medium reader that rejects every non-direct transmission medium. The direct
/// medium never reaches this seam (it is handled inline by
/// [`LoadingImage::init`]). Matches the default `.direct` limits.
fn reject_non_direct(
    _medium: command::Medium,
    _t: command::Transmission,
    _data: &[u8],
) -> Result<Vec<u8>, image::Error> {
    Err(image::Error::UnsupportedMedium)
}

/// Execute with a caller-supplied decoder and medium reader. Port of
/// `graphics_exec.execute` (`graphics_exec.zig:23-91`).
pub fn execute_with(
    terminal: &mut Terminal,
    cmd: &Command,
    decoder: &dyn ImageDecoder,
    read_medium: impl Fn(command::Medium, command::Transmission, &[u8]) -> Result<Vec<u8>, image::Error>
    + Copy,
) -> Option<Response> {
    // If storage is disabled then the whole protocol is disabled: we don't even
    // respond to queries, so the terminal acts as if the feature is unsupported.
    if !terminal.screen().kitty_images.enabled() {
        return None;
    }

    // The quiet setting controls the response. It is a `var` in Zig because for
    // chunked transmissions it can be adjusted from the in-progress load.
    let mut quiet = cmd.quiet;

    let resp: Option<Response> = match &cmd.control {
        command::Control::Query(_) => Some(query(cmd, decoder, read_medium)),
        command::Control::Display(_) => Some(display(terminal, cmd)),
        command::Control::Delete(_) => Some(delete(terminal, cmd)),

        command::Control::Transmit(_) | command::Control::TransmitAndDisplay { .. } => {
            // The `q` setting for a transmission is complicated: it inherits the
            // value from the starting chunk unless `q >= 1` on this command, in
            // which case that becomes the new `q` setting for the transfer.
            // Port of `graphics_exec.zig:56-67`.
            if let Some(loading) = terminal.screen().kitty_loading.as_ref() {
                match cmd.quiet {
                    // q=0: use whatever the start command's value was.
                    command::Quiet::No => quiet = loading.quiet,
                    // q>=1: use the new value (we're already set to it, since
                    // `quiet == cmd.quiet` here), and persist it onto the
                    // in-progress load. Port of the `assert(quiet == tag)` +
                    // `loading.quiet = tag` in `graphics_exec.zig:62-65`.
                    command::Quiet::Ok | command::Quiet::Failures => {
                        debug_assert_eq!(quiet, cmd.quiet);
                        let _ = loading;
                        if let Some(loading) = terminal.screen_mut().kitty_loading.as_mut() {
                            loading.quiet = cmd.quiet;
                        }
                    }
                }
            }

            Some(transmit(terminal, cmd, decoder, read_medium))
        }

        command::Control::TransmitAnimationFrame(_)
        | command::Control::ControlAnimation(_)
        | command::Control::ComposeAnimation(_) => Some(Response {
            message: "ERROR: unimplemented action".to_string(),
            ..Response::default()
        }),
    };

    // Handle the quiet settings (`graphics_exec.zig:78-88`).
    let resp = resp?;
    match quiet {
        command::Quiet::No => {
            if resp.is_empty() {
                None
            } else {
                Some(resp)
            }
        }
        command::Quiet::Ok => {
            if resp.ok() {
                None
            } else {
                Some(resp)
            }
        }
        command::Quiet::Failures => None,
    }
}

/// Execute a "query" command: attempt to load an image and respond with
/// success/error, but never persist anything. Port of `graphics_exec.query`.
fn query(
    cmd: &Command,
    decoder: &dyn ImageDecoder,
    read_medium: impl Fn(command::Medium, command::Transmission, &[u8]) -> Result<Vec<u8>, image::Error>,
) -> Response {
    let t = match &cmd.control {
        command::Control::Query(t) => *t,
        _ => unreachable!("query called on non-query command"),
    };

    // Query requires an image ID. We cannot send a response without one either,
    // but we return the error which is logged downstream.
    if t.image_id == 0 {
        return Response {
            message: "EINVAL: image ID required".to_string(),
            ..Response::default()
        };
    }

    let mut result = Response {
        id: t.image_id,
        image_number: t.image_number,
        placement_id: t.placement_id,
        message: "OK".to_string(),
    };

    // Attempt to load the image. If we cannot, set the appropriate error.
    // The default `.direct` limits are used (query never persists so storage
    // limits are irrelevant); the loaded image is dropped immediately.
    match LoadingImage::init(cmd, image::Limits::DIRECT, decoder, |m, tt, d| {
        read_medium(m, tt, d)
    }) {
        Ok(_loading) => result,
        Err(err) => {
            encode_error(&mut result, err);
            result
        }
    }
}

/// Transmit image data: load, validate, and store the image. Does not display.
/// Port of `graphics_exec.transmit`.
fn transmit(
    terminal: &mut Terminal,
    cmd: &Command,
    decoder: &dyn ImageDecoder,
    read_medium: impl Fn(command::Medium, command::Transmission, &[u8]) -> Result<Vec<u8>, image::Error>
    + Copy,
) -> Response {
    let t = cmd
        .transmission()
        .expect("transmit called on non-transmit command");
    let mut result = Response {
        id: t.image_id,
        image_number: t.image_number,
        placement_id: t.placement_id,
        message: "OK".to_string(),
    };
    if t.image_id > 0 && t.image_number > 0 {
        return Response {
            message: "EINVAL: image ID and number are mutually exclusive".to_string(),
            ..Response::default()
        };
    }

    let load = match load_and_add_image(terminal, cmd, decoder, read_medium) {
        Ok(load) => load,
        Err(err) => {
            encode_error(&mut result, err);
            return result;
        }
    };

    // If we're also displaying, do it now (this fn does both transmit and
    // transmit-and-display). Display may be deferred if multi-chunk.
    if let Some(d) = load.display {
        debug_assert!(!load.more);
        let mut d_copy = d;
        d_copy.image_id = load.image_id;
        let display_cmd = Command {
            control: command::Control::Display(d_copy),
            quiet: cmd.quiet,
            data: Vec::new(),
        };
        result = display(terminal, &display_cmd);
    }

    // More chunks expected: no response.
    if load.more {
        return Response::default();
    }

    // Auto-assigned (implicit) IDs are not responded to.
    if load.implicit_id {
        return Response::default();
    }

    // After the image is added, set the ID in case it changed.
    result.id = load.image_id;
    result
}

/// Display a previously-transmitted image at the cursor. Port of
/// `graphics_exec.display`.
fn display(terminal: &mut Terminal, cmd: &Command) -> Response {
    let d = cmd
        .display()
        .expect("display called on non-display command");

    // Display requires an image ID or number.
    if d.image_id == 0 && d.image_number == 0 {
        return Response {
            message: "EINVAL: image ID or number required".to_string(),
            ..Response::default()
        };
    }

    let mut result = Response {
        id: d.image_id,
        image_number: d.image_number,
        placement_id: d.placement_id,
        message: "OK".to_string(),
    };

    // Verify the requested image exists.
    let img_id = {
        let storage = &terminal.screen().kitty_images;
        let img = if d.image_id != 0 {
            storage.image_by_id(d.image_id)
        } else {
            storage.image_by_number(d.image_number)
        };
        match img {
            Some(img) => img.id,
            None => {
                result.message = "ENOENT: image not found".to_string();
                return result;
            }
        }
    };

    // Make sure our response has the image id in case we looked up by number.
    result.id = img_id;

    // Determine the placement location.
    let location: Location = if d.virtual_placement {
        // Virtual placements are not tracked.
        if d.parent_id > 0 {
            result.message = "EINVAL: virtual placement cannot refer to a parent".to_string();
            return result;
        }
        Location::Virtual
    } else {
        // Track a new pin for our cursor. The cursor is always tracked but we
        // don't want this one to move with the cursor.
        let screen = terminal.screen_mut();
        // SAFETY: the cursor's page_pin is a live tracked pin in `pages`.
        let cursor_pin = unsafe { *screen.cursor.page_pin };
        let pin = screen.pages.track_pin(cursor_pin);
        Location::Pin(pin)
    };

    // Build the placement.
    let mut p = Placement::new(location);
    p.x_offset = d.x_offset;
    p.y_offset = d.y_offset;
    p.source_x = d.x;
    p.source_y = d.y;
    p.source_width = d.width;
    p.source_height = d.height;
    p.columns = d.columns;
    p.rows = d.rows;
    p.z = d.z;

    // Add the placement.
    terminal
        .screen_mut()
        .kitty_images
        .add_placement(img_id, result.placement_id, p);

    // Apply cursor movement. This only applies to pin (non-virtual) placements.
    if let Location::Pin(pin) = p.location
        && d.cursor_movement.0 == command::CursorMovement::After
    {
        // Compute the grid size the placement occupies.
        let (cols, rows) = {
            let screen = terminal.screen();
            let img = screen
                .kitty_images
                .image_by_id(img_id)
                .expect("image just confirmed to exist");
            let geo = geometry(terminal);
            p.grid_size(img, &geo)
        };

        // Move the cursor down `rows` lines using index() (honors scroll region).
        for _ in 0..rows {
            terminal.index();
        }

        // SAFETY: `pin` is a live tracked placement pin.
        let pin_x = unsafe { (*pin).x() };
        let new_y = terminal.screen().cursor.y as usize;
        terminal.set_cursor_pos(new_y + 1, pin_x as usize + cols as usize + 1);
    }

    result
}

/// Delete image(s)/placement(s). Port of `graphics_exec.delete`.
fn delete(terminal: &mut Terminal, cmd: &Command) -> Response {
    let del = match &cmd.control {
        command::Control::Delete(d) => *d,
        _ => unreachable!("delete called on non-delete command"),
    };

    let geo = geometry(terminal);
    let screen = terminal.screen_mut();
    let cursor = (screen.cursor.x, screen.cursor.y);
    // Split-borrow the screen so storage.delete can hold `&mut pages` while the
    // storage is also borrowed mutably (disjoint fields of `Screen`).
    let crate::screen::Screen {
        kitty_images,
        pages,
        ..
    } = screen;
    kitty_images.delete(pages, &geo, cursor, del);

    // Delete never responds on success.
    Response::default()
}

/// The successful result of loading (and possibly deferring) an image.
struct LoadResult {
    /// The final image id (0 for a still-chunking transfer without an id).
    image_id: u32,
    /// True if the image's id was auto-assigned (implicit) — should not respond.
    implicit_id: bool,
    /// True if more chunks are expected (do not respond, do not display).
    more: bool,
    /// A deferred display, if the command was transmit-and-display.
    display: Option<command::Display>,
}

/// Load an image (handling chunking) and add it to storage on completion. Port
/// of `graphics_exec.loadAndAddImage`.
fn load_and_add_image(
    terminal: &mut Terminal,
    cmd: &Command,
    decoder: &dyn ImageDecoder,
    read_medium: impl Fn(command::Medium, command::Transmission, &[u8]) -> Result<Vec<u8>, image::Error>,
) -> Result<LoadResult, image::Error> {
    let t = cmd
        .transmission()
        .expect("load_and_add_image on non-transmit command");

    // Determine our (loading) image: continue an in-progress chunked transfer,
    // or start a new one.
    let mut loading: LoadingImage = if terminal.screen().kitty_loading.is_some() {
        // Continue the in-progress transfer.
        let more = t.more_chunks;
        let display;
        {
            let loading = terminal.screen_mut().kitty_loading.as_mut().unwrap();
            loading.add_data(&cmd.data)?;
            display = loading.display;
        }

        // If we have more, we're done for now (defer completion).
        if more {
            let image_id = terminal.screen().kitty_loading.as_ref().unwrap().image.id;
            return Ok(LoadResult {
                image_id,
                implicit_id: false,
                more: true,
                display,
            });
        }

        // No more chunks: take the loading image out to complete it.
        terminal.screen_mut().kitty_loading.take().unwrap()
    } else {
        let limits = terminal.screen().kitty_images.image_limits;
        LoadingImage::init(cmd, limits, decoder, |m, tt, d| read_medium(m, tt, d))?
    };

    // If the image has no ID, assign one.
    if loading.image.id == 0 {
        let storage = &mut terminal.screen_mut().kitty_images;
        loading.image.id = storage.next_image_id;
        storage.next_image_id = storage.next_image_id.wrapping_add(1);

        // If it also has no number, its auto-ID is "implicit".
        if loading.image.number == 0 {
            loading.image.implicit_id = true;
        }
    }

    // If this is chunked, this is the beginning of a new chunked transmission.
    if t.more_chunks {
        let image_id = loading.image.id;
        terminal.screen_mut().kitty_loading = Some(loading);
        return Ok(LoadResult {
            image_id,
            implicit_id: false,
            more: true,
            display: None,
        });
    }

    // Validate and store the image.
    let display = loading.display;
    let img: Image = loading.complete(decoder)?;
    let image_id = img.id;
    let implicit_id = img.implicit_id;
    terminal
        .screen_mut()
        .kitty_images
        .add_image(img)
        .map_err(|_| image::Error::OutOfMemory)?;

    Ok(LoadResult {
        image_id,
        implicit_id,
        more: false,
        display,
    })
}

/// Snapshot the terminal geometry the placement model reads. Port of the ad-hoc
/// `t.cols`/`t.rows`/`t.width_px`/`t.height_px` reads in the Zig exec/storage.
fn geometry(terminal: &Terminal) -> super::TerminalGeometry {
    super::TerminalGeometry::new(
        terminal.cols,
        terminal.rows,
        terminal.width_px,
        terminal.height_px,
    )
}

/// Encode an error code into a response message. Port of `graphics_exec.encodeError`.
fn encode_error(r: &mut Response, err: image::Error) {
    r.message = match err {
        image::Error::OutOfMemory => "ENOMEM: out of memory",
        image::Error::InvalidData => "EINVAL: invalid data",
        image::Error::DecompressionFailed => "EINVAL: decompression failed",
        image::Error::FilePathTooLong => "EINVAL: file path too long",
        image::Error::TemporaryFileNotInTempDir => "EINVAL: temporary file not in temp dir",
        image::Error::TemporaryFileNotNamedCorrectly => {
            "EINVAL: temporary file not named correctly"
        }
        image::Error::UnsupportedFormat => "EINVAL: unsupported format",
        image::Error::UnsupportedMedium => "EINVAL: unsupported medium",
        image::Error::UnsupportedDepth => "EINVAL: unsupported pixel depth",
        image::Error::DimensionsRequired => "EINVAL: dimensions required",
        image::Error::DimensionsTooLarge => "EINVAL: dimensions too large",
    }
    .to_string();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{Options, Terminal};

    fn term() -> Terminal {
        Terminal::new(Options {
            cols: 5,
            rows: 5,
            max_scrollback: 0,
            colors: crate::terminal::Colors::default(),
        })
    }

    fn parse(s: &str) -> Command {
        command::Parser::parse_string(s.as_bytes()).expect("parse")
    }

    /// Port of `graphics_exec.zig:396-424`, "kittygfx more chunks with q=1".
    #[test]
    fn more_chunks_with_q1() {
        let mut t = term();

        // Initial chunk has q=1.
        let cmd = parse("a=T,f=24,t=d,i=1,s=1,v=2,c=10,r=1,m=1,q=1;////");
        assert!(execute(&mut t, &cmd).is_none());

        // Subsequent chunk has no q but should respect the initial q=1.
        let cmd = parse("m=0;////");
        assert!(execute(&mut t, &cmd).is_none());
    }

    /// Port of `graphics_exec.zig:426-454`, "kittygfx more chunks with q=0".
    #[test]
    fn more_chunks_with_q0() {
        let mut t = term();

        // Initial chunk has q=0.
        let cmd = parse("a=t,f=24,t=d,s=1,v=2,c=10,r=1,m=1,i=1,q=0;////");
        assert!(execute(&mut t, &cmd).is_none());

        // Subsequent chunk has no q so should respond OK.
        let cmd = parse("m=0;////");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());
    }

    /// Port of `graphics_exec.zig:456-484`, "kittygfx more chunks with chunk
    /// increasing q".
    #[test]
    fn more_chunks_increasing_q() {
        let mut t = term();

        // Initial chunk has q=0.
        let cmd = parse("a=t,f=24,t=d,s=1,v=2,c=10,r=1,m=1,i=1,q=0;////");
        assert!(execute(&mut t, &cmd).is_none());

        // Subsequent chunk sets q=1 so should not respond.
        let cmd = parse("m=0,q=1;////");
        assert!(execute(&mut t, &cmd).is_none());
    }

    /// Port of `graphics_exec.zig:486-504`, "kittygfx default format is rgba".
    #[test]
    fn default_format_is_rgba() {
        let mut t = term();

        let cmd = parse("a=t,t=d,i=1,s=1,v=2,c=10,r=1;///////////");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());

        let img = t.screen().kitty_images.image_by_id(1).expect("image");
        assert_eq!(img.format, command::Format::Rgba);
    }

    /// Port of `graphics_exec.zig:506-521`, "kittygfx test valid u32 (expect
    /// invalid image ID)".
    #[test]
    fn valid_u32_invalid_image_id() {
        let mut t = term();

        let cmd = parse("a=p,i=4294967295");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(!resp.ok());
        assert_eq!(resp.message, "ENOENT: image not found");
    }

    /// Port of `graphics_exec.zig:523-538`, "kittygfx test valid i32 (expect
    /// invalid image ID)".
    #[test]
    fn valid_i32_invalid_image_id() {
        let mut t = term();

        let cmd = parse("a=p,i=1,z=-2147483648");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(!resp.ok());
        assert_eq!(resp.message, "ENOENT: image not found");
    }

    /// Port of `graphics_exec.zig:540-556`, "kittygfx no response with no image
    /// ID or number".
    #[test]
    fn no_response_no_id_or_number() {
        let mut t = term();

        let cmd = parse("a=t,f=24,t=d,s=1,v=2,c=10,r=1,i=0,I=0;////////");
        assert!(execute(&mut t, &cmd).is_none());
    }

    /// Port of `graphics_exec.zig:558-574`, "kittygfx no response with no image
    /// ID or number load and display".
    #[test]
    fn no_response_no_id_or_number_load_and_display() {
        let mut t = term();

        let cmd = parse("a=T,f=24,t=d,s=1,v=2,c=10,r=1,i=0,I=0;////////");
        assert!(execute(&mut t, &cmd).is_none());
    }

    /// Port of `graphics_exec.zig:576-613`, "kittygfx retransmit same id gets
    /// fresh image generation".
    #[test]
    fn retransmit_same_id_fresh_generation() {
        let mut t = term();

        // Transmit a 1x2 RGB image with id=1.
        let cmd = parse("a=t,t=d,f=24,i=1,s=1,v=2;////////");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());

        let gen1 = t.screen().kitty_images.image_by_id(1).unwrap().generation;
        assert!(gen1 > 0);
        assert_eq!(gen1, t.screen().kitty_images.generation);

        // Retransmit the same id with identical dimensions/length. Only the
        // generation reveals the contents were replaced.
        let cmd = parse("a=t,t=d,f=24,i=1,s=1,v=2;AAAAAAAA");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());

        let gen2 = t.screen().kitty_images.image_by_id(1).unwrap().generation;
        assert!(gen2 > gen1);
        assert_eq!(gen2, t.screen().kitty_images.generation);
    }

    /// Port of `graphics_exec.zig:615-658`, "kittygfx delete then retransmit same
    /// id gets fresh generation".
    #[test]
    fn delete_then_retransmit_fresh_generation() {
        let mut t = term();

        // Transmit and display, then delete everything, then retransmit.
        let cmd = parse("a=T,t=d,f=24,i=1,s=1,v=2,c=1,r=1;////////");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());

        let gen1 = t.screen().kitty_images.image_by_id(1).unwrap().generation;

        let cmd = parse("a=d,d=A");
        assert!(execute(&mut t, &cmd).is_none());
        assert!(t.screen().kitty_images.image_by_id(1).is_none());
        let gen_delete = t.screen().kitty_images.generation;
        assert!(gen_delete > gen1);

        let cmd = parse("a=t,t=d,f=24,i=1,s=1,v=2;////////");
        let resp = execute(&mut t, &cmd).expect("resp");
        assert!(resp.ok());

        let gen2 = t.screen().kitty_images.image_by_id(1).unwrap().generation;
        assert!(gen2 > gen1);
        assert!(gen2 > gen_delete);
    }
}
