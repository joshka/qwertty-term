//! Verifies the vendored default-font assets (`res/*.ttf`, licenses) are
//! byte-identical to the manifest (`res/font-manifest.sha256`), which records
//! the exact upstream Ghostty release assets (`src/font/embedded.zig`'s
//! `variable`, `variable_italic`, and `symbols_nerd_font` fetches) at pinned
//! commit `2da015cd6`. This makes any drift (hand-edit, partial re-vendor,
//! accidental reformat) a loud, immediate failure rather than a silent
//! divergence from what real Ghostty ships as its default font stack.
//!
//! A self-contained SHA-256 (no new crate dependency for a single test file)
//! is used to compute each vendored file's digest and compare against the
//! manifest. Adapted from the identical pattern in
//! `qwertty-term-termio/tests/shell_integration_scripts.rs`.

use std::path::Path;

const MANIFEST: &str = include_str!("../res/font-manifest.sha256");
const RES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/res");

/// Files in `res/` that are not part of the default-font manifest (kept for
/// other purposes) and should be skipped by the "no unmanifested files" check.
const UNMANIFESTED_ALLOWLIST: &[&str] = &["font-manifest.sha256"];

#[test]
fn vendored_fonts_match_manifest() {
    let entries = parse_manifest(MANIFEST);
    assert!(!entries.is_empty(), "manifest parsed to zero entries");

    let mut checked = 0usize;
    for (expected_hash, expected_size, rel_path) in &entries {
        let full_path = Path::new(RES_DIR).join(rel_path);
        let bytes = std::fs::read(&full_path)
            .unwrap_or_else(|e| panic!("failed to read vendored file {full_path:?}: {e}"));

        assert_eq!(
            bytes.len(),
            *expected_size,
            "size mismatch for {rel_path}: manifest says {expected_size}, file is {} bytes \
             (vendored default-font assets must stay byte-identical to upstream; if this is a \
             deliberate re-vendor against a new pinned commit, regenerate the manifest per its \
             header)",
            bytes.len()
        );

        let actual_hash = sha256_hex(&bytes);
        assert_eq!(
            &actual_hash, expected_hash,
            "sha256 mismatch for {rel_path}: vendored copy has drifted from the manifest \
             (upstream commit `2da015cd6`). Default-font assets must be copied verbatim -- do \
             not hand-edit them. If this is a deliberate re-vendor, regenerate the manifest."
        );
        checked += 1;
    }

    // Also check there's no extra, unmanifested file sitting in res/ (would
    // silently escape verification).
    let mut on_disk = Vec::new();
    collect_files(Path::new(RES_DIR), Path::new(RES_DIR), &mut on_disk);
    on_disk.retain(|f| !UNMANIFESTED_ALLOWLIST.contains(&f.as_str()));
    on_disk.sort();
    let mut manifested: Vec<String> = entries.iter().map(|(_, _, p)| p.clone()).collect();
    manifested.sort();
    assert_eq!(
        on_disk, manifested,
        "res/ contains files not listed in font-manifest.sha256 (or vice versa) -- every \
         vendored default-font asset must be manifested"
    );

    assert_eq!(checked, entries.len());
}

/// Parse `sha256  size  path` lines, skipping blank lines and `#` comments.
fn parse_manifest(text: &str) -> Vec<(String, usize, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts
                .next()
                .expect("manifest line missing hash")
                .to_string();
            let size: usize = parts
                .next()
                .expect("manifest line missing size")
                .parse()
                .expect("manifest size not a number");
            let path = parts
                .next()
                .expect("manifest line missing path")
                .to_string();
            (hash, size, path)
        })
        .collect()
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
}

// ============================================================================
// Minimal SHA-256 (FIPS 180-4), sufficient for hashing font-sized files in
// this test. Not optimized; not exposed outside this test binary.
// ============================================================================

fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Padding: message || 0x80 || zeros || 64-bit big-endian bit length,
    // total length a multiple of 64 bytes.
    let bit_len = (message.len() as u64) * 8;
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[test]
fn sha256_known_answer_tests() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
