//! Kitty graphics image loading (port of `graphics_image.zig`, commit `2da015cd6`).
//!
//! [`LoadingImage`] assembles a transmitted image from one or more chunks across the several
//! transmission mediums (direct / file / temporary-file / shared-memory), handling base64
//! (done upstream by the [`command`](super::command) parser), zlib decompression, and PNG
//! decode. [`Image`] is the completed, fully-decoded raw-pixel result.
//!
//! # Codec seam
//!
//! To keep `qwertty-term-vt` free of image-codec dependencies (it is the crown-jewel crate), zlib
//! inflation and PNG decode go through the [`ImageDecoder`] trait supplied to
//! [`LoadingImage::complete`]. Upstream ghostty gates PNG decode on a `sys.decode_png`
//! function pointer that is null when the decoder isn't linked; this trait is the Rust seam
//! for the same idea, and additionally externalizes zlib so consumers pick their own
//! `flate2`/`miniz` (or none). [`NoDecoder`] is the "decoder isn't linked" case.

use std::path::Path;

use super::command::{self, Compression, Format, Transmission};
use crate::pagelist::Pin;

/// Maximum width or height of an image. Taken directly from kitty.
pub const MAX_DIMENSION: u32 = 10000;

/// Maximum size in bytes, taken from kitty (400MB).
pub const MAX_SIZE: usize = 400 * 1024 * 1024;

/// Errors image loading can produce. Port of `Image.Error` plus the loading-path errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidData,
    DecompressionFailed,
    DimensionsRequired,
    DimensionsTooLarge,
    FilePathTooLong,
    TemporaryFileNotInTempDir,
    TemporaryFileNotNamedCorrectly,
    UnsupportedFormat,
    UnsupportedMedium,
    UnsupportedDepth,
    OutOfMemory,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

/// The result of decoding a PNG: raw RGBA pixels plus dimensions. Port of the `decode_png`
/// result struct in `sys.zig`.
pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    /// RGBA pixel data (`width * height * 4` bytes).
    pub data: Vec<u8>,
}

/// The codec seam: supplies zlib inflation and PNG decode to [`LoadingImage::complete`].
///
/// Keeps codec crates out of `qwertty-term-vt`'s dependency graph. Consumers back this with their
/// chosen crates (e.g. `flate2` + `png`/`image`); the built-in [`NoDecoder`] rejects both,
/// mirroring upstream's `sys.decode_png == null`.
pub trait ImageDecoder {
    /// Inflate zlib-compressed data. `None` means zlib is unsupported by this decoder.
    fn inflate_zlib(&self, data: &[u8], max_size: usize) -> Option<Result<Vec<u8>, Error>>;

    /// Decode PNG bytes to RGBA. `None` means PNG decode is unsupported (mirrors
    /// `sys.decode_png == null`), which makes non-direct PNG transmits fail up front.
    fn decode_png(&self, data: &[u8]) -> Option<Result<DecodedPng, Error>>;

    /// Whether this decoder can decode PNGs. Mirrors upstream's `sys.decode_png != null`
    /// check, letting non-direct PNG transmits fail up front before buffering.
    fn supports_png(&self) -> bool;
}

/// An [`ImageDecoder`] that supports nothing. Zlib payloads and PNGs will fail. Use this when
/// no codec crate is linked. Mirrors upstream's null `sys.decode_png` (and here also no zlib).
pub struct NoDecoder;

impl ImageDecoder for NoDecoder {
    fn inflate_zlib(&self, _data: &[u8], _max_size: usize) -> Option<Result<Vec<u8>, Error>> {
        None
    }
    fn decode_png(&self, _data: &[u8]) -> Option<Result<DecodedPng, Error>> {
        None
    }
    fn supports_png(&self) -> bool {
        false
    }
}

/// The limits of the kitty graphics protocol we should allow. Restricts transmission mediums
/// for resource/security reasons. Port of `LoadingImage.Limits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub file: bool,
    pub temporary_file: bool,
    pub shared_memory: bool,
}

impl Limits {
    /// All non-direct mediums permitted. Port of `Limits.all`.
    pub const ALL: Limits = Limits {
        file: true,
        temporary_file: true,
        shared_memory: true,
    };

    /// Only the direct medium permitted (direct is always allowed regardless). Port of
    /// `Limits.direct`.
    pub const DIRECT: Limits = Limits {
        file: false,
        temporary_file: false,
        shared_memory: false,
    };
}

/// An image that is still being loaded. Initialize with [`LoadingImage::init`] on the first
/// chunk, [`LoadingImage::add_data`] for subsequent chunks, then [`LoadingImage::complete`].
/// Port of `graphics_image.LoadingImage`.
#[derive(Debug)]
pub struct LoadingImage {
    /// The in-progress image; metadata comes from the first chunk.
    pub image: Image,
    /// The data being built up (still compressed/encoded until `complete`).
    pub data: Vec<u8>,
    /// Non-null for a transmit-and-display so we display after loading.
    pub display: Option<command::Display>,
    /// Quiet setting from the initial load command.
    pub quiet: command::Quiet,
}

impl LoadingImage {
    /// Initialize a chunked image from the first transmission chunk. Port of
    /// `LoadingImage.init`.
    ///
    /// The `read_medium` closure loads path-based mediums (file/temporary-file/shared-memory);
    /// it is the Rust seam for the OS-touching parts of upstream's `readFile`/`readSharedMemory`
    /// (which belong to the exec/integration layer). For the direct medium it is never called.
    pub fn init(
        cmd: &command::Command,
        limits: Limits,
        decoder: &dyn ImageDecoder,
        read_medium: impl FnOnce(command::Medium, Transmission, &[u8]) -> Result<Vec<u8>, Error>,
    ) -> Result<LoadingImage, Error> {
        let t = cmd.transmission().ok_or(Error::InvalidData)?;
        let mut result = LoadingImage {
            image: Image {
                id: t.image_id,
                number: t.image_number,
                width: t.width,
                height: t.height,
                compression: t.compression,
                format: t.format,
                ..Image::default()
            },
            data: Vec::new(),
            display: cmd.display(),
            quiet: cmd.quiet,
        };

        // Direct medium: the chunk is added directly (base64 already decoded by the parser).
        if t.medium == command::Medium::Direct {
            result.add_data(&cmd.data)?;
            return Ok(result);
        }

        // Verify capabilities and limits.
        // Special-case: no PNG decoder and a PNG format — fail up front rather than buffering.
        if t.format == Format::Png && !decoder.supports_png() {
            return Err(Error::UnsupportedMedium);
        }
        match t.medium {
            command::Medium::Direct => unreachable!(),
            command::Medium::File => {
                if !limits.file {
                    return Err(Error::UnsupportedMedium);
                }
            }
            command::Medium::TemporaryFile => {
                if !limits.temporary_file {
                    return Err(Error::UnsupportedMedium);
                }
            }
            command::Medium::SharedMemory => {
                if !limits.shared_memory {
                    return Err(Error::UnsupportedMedium);
                }
            }
        }

        // Reject paths with embedded NUL (realpath would assert). Port of the NUL check.
        if cmd.data.contains(&0) {
            return Err(Error::InvalidData);
        }

        // Path-based media: load via the caller-supplied seam. The seam is responsible for the
        // path-safety checks (`/proc`, `/sys`, `/dev`, temp-dir + naming for temporary files),
        // offset/size, unlinking temporary files, and shm handling — see
        // `validate_file_path`/`is_path_in_temp_dir` helpers below, which port that logic and
        // which a real integration seam should call.
        let data = read_medium(t.medium, t, &cmd.data)?;
        result.data = data;
        Ok(result)
    }

    /// Adds a chunk of data (the `m` continuation parameter). Port of `LoadingImage.addData`.
    pub fn add_data(&mut self, data: &[u8]) -> Result<(), Error> {
        if data.is_empty() {
            return Ok(());
        }
        if self.data.len() + data.len() > MAX_SIZE {
            return Err(Error::InvalidData);
        }
        self.data.extend_from_slice(data);
        Ok(())
    }

    /// Complete the chunked image, returning a finished [`Image`]. Port of
    /// `LoadingImage.complete`.
    pub fn complete(&mut self, decoder: &dyn ImageDecoder) -> Result<Image, Error> {
        // Decompress if compressed.
        if self.image.compression == Compression::ZlibDeflate {
            let inflated = decoder
                .inflate_zlib(&self.data, MAX_SIZE)
                .ok_or(Error::DecompressionFailed)??;
            self.data = inflated;
            self.image.compression = Compression::None;
        }

        // Decode PNG if needed (updates dimensions, sets format rgba).
        if self.image.format == Format::Png {
            let decoded = decoder
                .decode_png(&self.data)
                .ok_or(Error::UnsupportedFormat)??;
            if decoded.data.len() > MAX_SIZE {
                return Err(Error::InvalidData);
            }
            self.data = decoded.data;
            self.image.width = decoded.width;
            self.image.height = decoded.height;
            self.image.format = Format::Rgba;
        }

        // Validate dimensions.
        if self.image.width == 0 || self.image.height == 0 {
            return Err(Error::DimensionsRequired);
        }
        if self.image.width > MAX_DIMENSION || self.image.height > MAX_DIMENSION {
            return Err(Error::DimensionsTooLarge);
        }

        // Data length must match.
        let bpp = Transmission::format_bpp(self.image.format) as usize;
        let expected_len = self.image.width as usize * self.image.height as usize * bpp;
        if self.data.len() != expected_len {
            return Err(Error::InvalidData);
        }

        // Move the data into the completed image.
        let mut result = std::mem::take(&mut self.image);
        result.data = std::mem::take(&mut self.data);
        Ok(result)
    }
}

/// Returns true if `path` appears to be in a temporary directory. Ports the safety check from
/// `LoadingImage.isPathInTempDir` (kitty logic). Exposed so an integration seam that actually
/// reads temporary-file mediums can reuse it.
pub fn is_path_in_temp_dir(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.starts_with("/tmp") {
        return true;
    }
    if s.starts_with("/dev/shm") {
        return true;
    }
    if let Ok(tmp) = std::env::var("TMPDIR")
        && !tmp.is_empty()
        && s.starts_with(tmp.trim_end_matches('/'))
    {
        return true;
    }
    // std::env::temp_dir realpath (macOS /tmp -> /private/var/...).
    let tmp = std::env::temp_dir();
    if let Ok(real) = std::fs::canonicalize(&tmp)
        && s.starts_with(&*real.to_string_lossy())
    {
        return true;
    }
    if s.starts_with(&*tmp.to_string_lossy()) {
        return true;
    }
    false
}

/// Validates that a resolved file path is "safe" for a file/temporary-file medium, porting the
/// checks in `LoadingImage.readFile`. Exposed for the integration seam. `is_temporary` selects
/// the extra temp-dir + naming requirements.
pub fn validate_file_path(path: &Path, is_temporary: bool) -> Result<(), Error> {
    let s = path.to_string_lossy();
    if s.starts_with("/proc/")
        || s.starts_with("/sys/")
        || (s.starts_with("/dev/") && !s.starts_with("/dev/shm/"))
    {
        return Err(Error::InvalidData);
    }
    if is_temporary {
        if !is_path_in_temp_dir(path) {
            return Err(Error::TemporaryFileNotInTempDir);
        }
        if !s.contains("tty-graphics-protocol") {
            return Err(Error::TemporaryFileNotNamedCorrectly);
        }
    }
    Ok(())
}

/// A fully loaded image. Post-`complete` invariant: `data` is fully-decoded raw pixels,
/// `compression == None`, `format != Png`, and `data.len() == width * height * bpp`. Port of
/// `graphics_image.Image`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub number: u32,
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub compression: Compression,
    pub data: Vec<u8>,
    /// Monotonic content-mutation stamp assigned by [`ImageStorage`](super::storage). Zero
    /// means "never stored".
    pub generation: u64,
    /// True if loaded by a command with no id/number (should not be responded to).
    pub implicit_id: bool,
}

impl Default for Image {
    fn default() -> Self {
        Image {
            id: 0,
            number: 0,
            width: 0,
            height: 0,
            format: Format::Rgb,
            compression: Compression::None,
            data: Vec::new(),
            generation: 0,
            implicit_id: false,
        }
    }
}

impl Image {
    /// A copy with data cleared, for logging. Port of `Image.withoutData`.
    pub fn without_data(&self) -> Image {
        Image {
            data: Vec::new(),
            ..self.clone()
        }
    }
}

/// The rect (in grid cells) a placement occupies. Rounded up to whole cells. Port of
/// `graphics_image.Rect`.
///
/// This references [`Pin`] (a `qwertty-term-vt` screen position), which is the one place the image
/// model leaks a qwertty-term-vt type — see the extraction notes in the analysis doc.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub top_left: Pin,
    pub bottom_right: Pin,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kitty::command::{Command, Control, Medium, Quiet};

    /// A decoder backed by `flate2` (zlib) and `png` for tests. These crates are dev-only, so
    /// the base crate stays codec-free.
    struct TestDecoder;

    impl ImageDecoder for TestDecoder {
        fn inflate_zlib(&self, data: &[u8], max_size: usize) -> Option<Result<Vec<u8>, Error>> {
            use std::io::Read as _;
            let mut out = Vec::new();
            let dec = flate2::read::ZlibDecoder::new(data);
            Some(match dec.take(max_size as u64).read_to_end(&mut out) {
                Ok(_) => Ok(out),
                Err(_) => Err(Error::DecompressionFailed),
            })
        }

        fn supports_png(&self) -> bool {
            true
        }

        fn decode_png(&self, data: &[u8]) -> Option<Result<DecodedPng, Error>> {
            let mut decoder = png::Decoder::new(data);
            // Expand palette/low-bit-depth images and normalize to 8-bit so `to_rgba` sees a
            // simple RGB/RGBA/grayscale 8-bit buffer, matching upstream's decode output.
            decoder.set_transformations(
                png::Transformations::EXPAND | png::Transformations::normalize_to_color8(),
            );
            Some((|| {
                let mut reader = decoder.read_info().map_err(|_| Error::InvalidData)?;
                let mut buf = vec![0; reader.output_buffer_size()];
                let info = reader
                    .next_frame(&mut buf)
                    .map_err(|_| Error::InvalidData)?;
                buf.truncate(info.buffer_size());
                // Expand to RGBA to match upstream's decode output.
                let (w, h) = (info.width, info.height);
                let rgba = to_rgba(&buf, info.color_type, info.bit_depth, w, h)
                    .ok_or(Error::InvalidData)?;
                Ok(DecodedPng {
                    width: w,
                    height: h,
                    data: rgba,
                })
            })())
        }
    }

    fn to_rgba(
        buf: &[u8],
        color: png::ColorType,
        depth: png::BitDepth,
        w: u32,
        h: u32,
    ) -> Option<Vec<u8>> {
        if depth != png::BitDepth::Eight {
            return None;
        }
        let px = (w * h) as usize;
        let mut out = Vec::with_capacity(px * 4);
        match color {
            png::ColorType::Rgba => return Some(buf.to_vec()),
            png::ColorType::Rgb => {
                for c in buf.chunks_exact(3) {
                    out.extend_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for &g in buf {
                    out.extend_from_slice(&[g, g, g, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for c in buf.chunks_exact(2) {
                    out.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
                }
            }
            _ => return None,
        }
        Some(out)
    }

    /// Reads a path-based medium for tests, porting the file-safety + read logic that the
    /// integration seam would own.
    fn read_medium(medium: Medium, t: Transmission, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let path_str = std::str::from_utf8(payload).map_err(|_| Error::InvalidData)?;
        let path = std::fs::canonicalize(path_str).map_err(|_| Error::InvalidData)?;
        match medium {
            Medium::File => validate_file_path(&path, false)?,
            Medium::TemporaryFile => validate_file_path(&path, true)?,
            Medium::SharedMemory => return Err(Error::UnsupportedMedium),
            Medium::Direct => unreachable!(),
        }
        let data = std::fs::read(&path).map_err(|_| Error::InvalidData)?;
        let start = t.offset as usize;
        let out = if t.size > 0 {
            data.get(start..(start + t.size as usize).min(data.len()))
        } else {
            data.get(start..)
        }
        .ok_or(Error::InvalidData)?
        .to_vec();
        // Temporary files are unlinked after reading.
        if medium == Medium::TemporaryFile {
            let _ = std::fs::remove_file(&path);
        }
        Ok(out)
    }

    fn transmit_cmd(t: Transmission, data: &[u8]) -> Command {
        Command {
            control: Control::Transmit(t),
            quiet: Quiet::No,
            data: data.to_vec(),
        }
    }

    // This specifically tests we ALLOW invalid RGB data because kitty documents this works.
    #[test]
    fn image_load_with_invalid_rgb_data() {
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                width: 1,
                height: 1,
                image_id: 31,
                ..Default::default()
            },
            b"AAAA",
        );
        let _loading = LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
    }

    #[test]
    fn image_load_with_image_too_wide() {
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                width: MAX_DIMENSION + 1,
                height: 1,
                image_id: 31,
                ..Default::default()
            },
            b"AAAA",
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        assert_eq!(
            loading.complete(&TestDecoder).unwrap_err(),
            Error::DimensionsTooLarge
        );
    }

    #[test]
    fn image_load_with_image_too_tall() {
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                height: MAX_DIMENSION + 1,
                width: 1,
                image_id: 31,
                ..Default::default()
            },
            b"AAAA",
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        assert_eq!(
            loading.complete(&TestDecoder).unwrap_err(),
            Error::DimensionsTooLarge
        );
    }

    #[test]
    fn image_load_rgb_zlib_compressed_direct() {
        let data = include_bytes!("testdata/image-rgb-zlib_deflate-128x96-2147483647-raw.data");
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::Direct,
                compression: Compression::ZlibDeflate,
                height: 96,
                width: 128,
                image_id: 31,
                ..Default::default()
            },
            data,
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
    }

    #[test]
    fn image_load_rgb_not_compressed_direct() {
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::Direct,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            data,
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
    }

    #[test]
    fn image_load_rgb_zlib_compressed_direct_chunked() {
        let data = include_bytes!("testdata/image-rgb-zlib_deflate-128x96-2147483647-raw.data");
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::Direct,
                compression: Compression::ZlibDeflate,
                height: 96,
                width: 128,
                image_id: 31,
                more_chunks: true,
                ..Default::default()
            },
            &data[0..1024],
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        for chunk in data[1024..].chunks(1024) {
            loading.add_data(chunk).unwrap();
        }
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
    }

    #[test]
    fn image_load_rgb_zlib_compressed_direct_chunked_with_zero_initial_chunk() {
        let data = include_bytes!("testdata/image-rgb-zlib_deflate-128x96-2147483647-raw.data");
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::Direct,
                compression: Compression::ZlibDeflate,
                height: 96,
                width: 128,
                image_id: 31,
                more_chunks: true,
                ..Default::default()
            },
            b"",
        );
        let mut loading =
            LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
        for chunk in data.chunks(1024) {
            loading.add_data(chunk).unwrap();
        }
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
    }

    #[test]
    fn image_load_temporary_file_without_correct_path() {
        let dir = tempdir_named("kitty-test-a");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::TemporaryFile,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let err = LoadingImage::init(&cmd, Limits::ALL, &TestDecoder, read_medium).unwrap_err();
        assert_eq!(err, Error::TemporaryFileNotNamedCorrectly);
        // File should still be there.
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn image_load_rgb_not_compressed_temporary_file() {
        let dir = tempdir_named("kitty-test-b");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("tty-graphics-protocol-image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::TemporaryFile,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let mut loading = LoadingImage::init(&cmd, Limits::ALL, &TestDecoder, read_medium).unwrap();
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
        // Temporary file should be gone.
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn image_load_rgb_not_compressed_regular_file() {
        let dir = tempdir_named("kitty-test-c");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::File,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let mut loading = LoadingImage::init(&cmd, Limits::ALL, &TestDecoder, read_medium).unwrap();
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn image_load_png_not_compressed_regular_file() {
        let dir = tempdir_named("kitty-test-d");
        let data = include_bytes!("testdata/image-png-none-50x76-2147483647-raw.data");
        let file = dir.join("tty-graphics-protocol-image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Png,
                medium: Medium::File,
                compression: Compression::None,
                width: 0,
                height: 0,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let mut loading = LoadingImage::init(&cmd, Limits::ALL, &TestDecoder, read_medium).unwrap();
        let img = loading.complete(&TestDecoder).unwrap();
        assert_eq!(img.compression, Compression::None);
        assert_eq!(img.format, Format::Rgba);
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn limits_direct_medium_always_allowed() {
        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::Direct,
                width: 1,
                height: 1,
                image_id: 31,
                ..Default::default()
            },
            b"AAAA",
        );
        // Direct works even with the most restrictive limits.
        let _loading = LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap();
    }

    #[test]
    fn limits_file_medium_blocked_by_limits() {
        let dir = tempdir_named("kitty-test-e");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::File,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let err = LoadingImage::init(&cmd, Limits::DIRECT, &TestDecoder, read_medium).unwrap_err();
        assert_eq!(err, Error::UnsupportedMedium);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn limits_file_medium_allowed_by_limits() {
        let dir = tempdir_named("kitty-test-f");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::File,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let _loading = LoadingImage::init(
            &cmd,
            Limits {
                file: true,
                temporary_file: false,
                shared_memory: false,
            },
            &TestDecoder,
            read_medium,
        )
        .unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn limits_temporary_file_medium_blocked_by_limits() {
        let dir = tempdir_named("kitty-test-g");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("tty-graphics-protocol-image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::TemporaryFile,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let err = LoadingImage::init(
            &cmd,
            Limits {
                file: true,
                temporary_file: false,
                shared_memory: true,
            },
            &TestDecoder,
            read_medium,
        )
        .unwrap_err();
        assert_eq!(err, Error::UnsupportedMedium);
        // File should still exist since we blocked before reading.
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn limits_temporary_file_medium_allowed_by_limits() {
        let dir = tempdir_named("kitty-test-h");
        let data = include_bytes!("testdata/image-rgb-none-20x15-2147483647-raw.data");
        let file = dir.join("tty-graphics-protocol-image.data");
        std::fs::write(&file, data).unwrap();
        let path = std::fs::canonicalize(&file).unwrap();

        let cmd = transmit_cmd(
            Transmission {
                format: Format::Rgb,
                medium: Medium::TemporaryFile,
                compression: Compression::None,
                width: 20,
                height: 15,
                image_id: 31,
                ..Default::default()
            },
            path.to_string_lossy().as_bytes(),
        );
        let _loading = LoadingImage::init(
            &cmd,
            Limits {
                file: false,
                temporary_file: true,
                shared_memory: false,
            },
            &TestDecoder,
            read_medium,
        )
        .unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Make a fresh temp subdir under the OS temp dir. Named uniquely per test to avoid
    /// collisions when tests run in parallel.
    fn tempdir_named(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("qwertty-term-vt-{name}-{pid}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
