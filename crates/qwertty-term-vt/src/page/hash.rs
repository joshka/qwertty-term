//! Stable 64-bit hash helpers for the offset-based containers.
//!
//! Ghostty uses Zig's `autoHash` (Wyhash) for map keys, `std.hash.int` for the
//! folded style hash, and Wyhash for hyperlink entries. Exact hash values are
//! internal to each implementation (tables are always rebuilt through the same
//! code path and clones are byte copies), so the port uses a SplitMix64-based
//! mix instead. What matters is uniformity, especially in the top 7 bits
//! (hash-map fingerprints) and low bits (bucket indices).

/// SplitMix64 finalizer. Stand-in for Zig's `std.hash.int`.
#[inline]
pub fn splitmix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    x
}

/// FNV-1a over bytes, finished with SplitMix64. Stand-in for Wyhash on
/// byte strings (hyperlink URIs/IDs).
#[inline]
pub fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325 ^ seed;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    splitmix64(h)
}

/// Key trait for the offset hash map (stand-in for Zig's `AutoContext`).
pub trait MapKey: Copy + PartialEq {
    fn hash64(&self) -> u64;
}

macro_rules! impl_map_key_int {
    ($($t:ty),*) => {$(
        impl MapKey for $t {
            #[inline]
            fn hash64(&self) -> u64 {
                splitmix64(*self as u64)
            }
        }
    )*};
}

impl_map_key_int!(u16, u32, u64, usize);

impl MapKey for i32 {
    #[inline]
    fn hash64(&self) -> u64 {
        splitmix64(*self as u32 as u64)
    }
}

impl<T> MapKey for super::size::Offset<T> {
    #[inline]
    fn hash64(&self) -> u64 {
        splitmix64(self.get() as u64)
    }
}
