//! Criterion bench for the page offset hash map (`page::offset_map`).
//!
//! Models the churn/lookup workload of upstream Ghostty's
//! `src/benchmark/HyperlinkMap.zig`: a fixed-capacity, open-addressed map is
//! filled to a target load factor, then either every populated key is looked
//! up or every populated key is removed and reinserted ("churn"). The churn
//! mode is what terminal output does when it repeatedly replaces hyperlink /
//! grapheme cells in a page whose map is close to full — and it is where a
//! tombstone-based map hits a probe-length cliff: at a 100% load factor a
//! removal leaves a tombstone rather than a free slot, so the following
//! reinsertion (a miss) must scan the whole capacity before it can recycle
//! the tombstone. Backward-shift deletion keeps free slots in every probe
//! chain and removes that cliff.
//!
//! Run with `cargo bench -p qwertty-term-vt --bench hash_map`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use qwertty_term_vt::page::hash::MapKey;
use qwertty_term_vt::page::offset_map::{Map, OffsetHashMap, Size};
use qwertty_term_vt::page::size::OffsetBuf;

/// An aligned backing buffer that outlives the map view, mirroring the page's
/// single-allocation ownership of the map.
struct Backing<K: MapKey, V: Copy> {
    _buf: Vec<u8>,
    off: OffsetHashMap<K, V>,
    base: *mut u8,
}

impl<K: MapKey, V: Copy> Backing<K, V> {
    fn new(cap: Size) -> Self {
        let layout = OffsetHashMap::<K, V>::layout(cap);
        let align = OffsetHashMap::<K, V>::base_align();
        let mut buf = vec![0u8; layout.total_size + align];
        let pad = buf.as_ptr().align_offset(align);
        // SAFETY: buffer holds total_size + align bytes; base is align-aligned
        // and exclusively owned by this map for the buffer's lifetime.
        let base = unsafe { buf.as_mut_ptr().add(pad) };
        let off = unsafe { OffsetHashMap::<K, V>::init(OffsetBuf::new(base), &layout) };
        Self {
            _buf: buf,
            off,
            base,
        }
    }

    fn map(&self) -> Map<K, V> {
        // SAFETY: `base` is the true base this map was initialized against.
        unsafe { self.off.map(self.base) }
    }
}

/// Populate `n` sequential keys into a fresh `cap`-slot map.
fn populated(cap: Size, n: Size) -> Backing<u32, u32> {
    let backing = Backing::<u32, u32>::new(cap);
    let mut map = backing.map();
    // SAFETY: exclusive access; n <= cap so capacity is never exceeded.
    unsafe {
        for k in 0..n {
            map.put(k, k).unwrap();
        }
    }
    backing
}

/// Remove then reinsert every populated key once — one "churn" pass.
fn churn_pass(map: &mut Map<u32, u32>, n: Size) {
    // SAFETY: exclusive access; each key is removed then immediately
    // reinserted, so the live count returns to `n` and capacity holds.
    unsafe {
        for k in 0..n {
            map.remove(&black_box(k));
            map.put(black_box(k), black_box(k)).unwrap();
        }
    }
}

/// Sliding-window churn: keep `n` live keys, and each step evict the oldest
/// key and insert a fresh, never-seen key. Models terminal cells coming and
/// going at *different* offsets over time — the workload that accumulates
/// tombstones in a tombstone-deletion map (a removed slot is never the
/// reinserted key), where same-key churn would recycle in place.
fn slide_pass(map: &mut Map<u32, u32>, n: Size, start: &mut u32) {
    // SAFETY: exclusive access; live count stays `n` (evict then insert), and
    // `n` never exceeds capacity.
    unsafe {
        for _ in 0..n {
            let old = *start;
            let fresh = start.wrapping_add(n);
            map.remove(&black_box(old));
            map.put(black_box(fresh), black_box(fresh)).unwrap();
            *start = start.wrapping_add(1);
        }
    }
}

/// Look up every populated key once.
fn lookup_pass(map: &Map<u32, u32>, n: Size) -> u64 {
    let mut sink = 0u64;
    // SAFETY: exclusive access; keys 0..n are present.
    unsafe {
        for k in 0..n {
            sink = sink.wrapping_add(map.get(&black_box(k)).unwrap() as u64);
        }
    }
    sink
}

/// Capacity under test: 4096 raw slots, matching upstream's default working
/// set. Load factors span the range where tombstone probe cost diverges from
/// backward-shift — 50% (loose), 90% (tight), 100% (the cliff).
const CAP: Size = 4096;
const LOADS: [u32; 3] = [50, 90, 100];

fn bench_churn(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_map/churn");
    for load in LOADS {
        let n = (CAP * load / 100).max(1);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(load), &n, |b, &n| {
            // Populate once outside the timed region; each iteration is a full
            // remove+reinsert pass over all n keys.
            let backing = populated(CAP, n);
            let mut map = backing.map();
            b.iter(|| churn_pass(&mut map, n));
            black_box(map.count());
        });
    }
    group.finish();
}

fn bench_slide(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_map/slide");
    for load in LOADS {
        // 100% fill cannot slide (no free slot to insert the fresh key before
        // the evict completes in a tombstone map); cap the slide fill below it.
        if load >= 100 {
            continue;
        }
        let n = (CAP * load / 100).max(1);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(load), &n, |b, &n| {
            let backing = populated(CAP, n);
            let mut map = backing.map();
            let mut start = 0u32;
            b.iter(|| slide_pass(&mut map, n, &mut start));
            black_box(map.count());
        });
    }
    group.finish();
}

fn bench_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_map/lookup");
    for load in LOADS {
        let n = (CAP * load / 100).max(1);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(load), &n, |b, &n| {
            let backing = populated(CAP, n);
            let map = backing.map();
            b.iter(|| black_box(lookup_pass(&map, n)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_churn, bench_slide, bench_lookup);
criterion_main!(benches);
