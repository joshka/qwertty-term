//! M2-C runtime benchmark: identical load against both drivers, foreground,
//! release mode. Produces the results table for `docs/adr/002-termio-runtime.md`.
//!
//! Run:
//!   cargo run -p spike-runtime --release --bin bench -- all
//!   cargo run -p spike-runtime --release --bin bench -- latency|flood|coalesce|idle|backpressure
//!
//! Every scenario is implemented generically over the `Driver` trait so the
//! two runtimes run byte-identical logic; only `spawn` differs.

use spike_runtime::driver::{DriverHandle, Handler};
use spike_runtime::mailbox::{self, Message, Sender, Waker};
use spike_runtime::{CountingHandler, threads, tokio_rt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Which runtime, for labelling.
#[derive(Clone, Copy)]
enum Rt {
    Threads,
    Tokio,
}
impl Rt {
    fn name(self) -> &'static str {
        match self {
            Rt::Threads => "threads+polling",
            Rt::Tokio => "tokio current-thread",
        }
    }
}

/// A spawned driver + the producer handle bits the scenarios need.
struct Running {
    sender: Sender,
    stop: Box<dyn DriverHandle>,
    join: std::thread::JoinHandle<std::io::Result<()>>,
}

/// Spawn `rt` wired to a fresh mailbox using `handler`.
fn spawn(rt: Rt, handler: impl Handler) -> Running {
    match rt {
        Rt::Threads => {
            let driver = <threads::ThreadsDriver as spike_runtime::driver::Driver>::new().unwrap();
            let waker: Arc<dyn Waker> =
                spike_runtime::driver::Driver::waker(&driver) as Arc<dyn Waker>;
            let (sender, recv) = mailbox::channel(waker);
            let stop: Box<dyn DriverHandle> =
                Box::new(spike_runtime::driver::Driver::handle(&driver));
            let join = std::thread::Builder::new()
                .name("io".into())
                .spawn(move || spike_runtime::driver::Driver::run(driver, recv, handler))
                .unwrap();
            Running { sender, stop, join }
        }
        Rt::Tokio => {
            let driver = <tokio_rt::TokioDriver as spike_runtime::driver::Driver>::new().unwrap();
            let waker: Arc<dyn Waker> =
                spike_runtime::driver::Driver::waker(&driver) as Arc<dyn Waker>;
            let (sender, recv) = mailbox::channel(waker);
            let stop: Box<dyn DriverHandle> =
                Box::new(spike_runtime::driver::Driver::handle(&driver));
            let join = std::thread::Builder::new()
                .name("io-tokio".into())
                .spawn(move || spike_runtime::driver::Driver::run(driver, recv, handler))
                .unwrap();
            Running { sender, stop, join }
        }
    }
}

impl Running {
    fn shutdown(self) {
        self.stop.stop();
        let _ = self.join.join();
    }
}

// --- shared latency handler ---------------------------------------------------
//
// To measure wakeup latency we need the CONSUMER to record a timestamp the
// instant a probe message runs. The probe is a `WriteSmall` whose first 8 bytes
// encode a sequence id; the handler records elapsed nanos for the probe into a
// shared slot the producer reads.

struct LatencyHandler {
    recorded: Arc<LatencySlots>,
}

struct LatencySlots {
    start: Instant,
    done: Mutex<Vec<(u64, u128)>>,
    cv: Condvar,
}

impl Handler for LatencyHandler {
    fn on_messages(&mut self, batch: &[Message]) {
        let now = self.recorded.start.elapsed().as_nanos();
        for m in batch {
            if let Message::WriteSmall { data, len } = m
                && *len >= 8
            {
                let id = u64::from_le_bytes(data[..8].try_into().unwrap());
                if id != 0 {
                    let mut d = self.recorded.done.lock().unwrap();
                    d.push((id, now));
                    self.recorded.cv.notify_all();
                }
            }
        }
    }
    fn on_resize(&mut self, _c: u16, _r: u16) {}
    fn on_sync_reset(&mut self) {}
}

fn probe(id: u64) -> Message {
    let mut data = [0u8; 38];
    data[..8].copy_from_slice(&id.to_le_bytes());
    Message::WriteSmall { data, len: 8 }
}

// --- percentile helper --------------------------------------------------------

fn pct(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// --- scenario 1: wakeup latency (idle and flood) ------------------------------

fn latency(rt: Rt, iters: usize, idle: bool) -> (u128, u128) {
    let slots = Arc::new(LatencySlots {
        start: Instant::now(),
        done: Mutex::new(Vec::new()),
        cv: Condvar::new(),
    });
    let handler = LatencyHandler {
        recorded: slots.clone(),
    };
    let run = spawn(rt, handler);

    let mut samples: Vec<u128> = Vec::with_capacity(iters);
    let flood_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flood_join = if !idle {
        let s = run.sender.clone();
        let stop = flood_stop.clone();
        Some(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = s.try_send(Message::LinefeedMode(true));
                s.notify();
            }
        }))
    } else {
        None
    };

    for id in 1..=iters as u64 {
        if idle {
            std::thread::sleep(Duration::from_micros(500));
        }
        let posted = slots.start.elapsed().as_nanos();
        while run.sender.send(probe(id)).is_err() {
            std::hint::spin_loop();
        }
        let mut guard = slots.done.lock().unwrap();
        let delivered = loop {
            if let Some(&(_, t)) = guard.iter().find(|&&(i, _)| i == id) {
                break t;
            }
            guard = slots.cv.wait(guard).unwrap();
        };
        drop(guard);
        samples.push(delivered.saturating_sub(posted));
    }

    flood_stop.store(true, Ordering::Relaxed);
    if let Some(j) = flood_join {
        let _ = j.join();
    }
    run.shutdown();

    samples.sort_unstable();
    (pct(&samples, 50.0), pct(&samples, 99.0))
}

// --- scenario 2: mailbox flood throughput (1M messages) -----------------------

fn flood(rt: Rt, total: u64) -> (Duration, f64) {
    let counter = Arc::new(AtomicU64::new(0));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    struct H {
        counter: Arc<AtomicU64>,
        target: u64,
        done: Arc<(Mutex<bool>, Condvar)>,
    }
    impl Handler for H {
        fn on_messages(&mut self, batch: &[Message]) {
            let n = self
                .counter
                .fetch_add(batch.len() as u64, Ordering::Relaxed)
                + batch.len() as u64;
            if n >= self.target {
                *self.done.0.lock().unwrap() = true;
                self.done.1.notify_all();
            }
        }
        fn on_resize(&mut self, _: u16, _: u16) {}
        fn on_sync_reset(&mut self) {}
    }
    let run = spawn(
        rt,
        H {
            counter: counter.clone(),
            target: total,
            done: done.clone(),
        },
    );

    let start = Instant::now();
    let s = run.sender.clone();
    let producer = std::thread::spawn(move || {
        let msg = Message::small(b"x");
        let mut sent = 0u64;
        while sent < total {
            match s.try_send(msg.clone()) {
                Ok(()) => {
                    sent += 1;
                    if sent.is_multiple_of(32) {
                        s.notify();
                    }
                }
                Err(_) => {
                    s.notify();
                    std::hint::spin_loop();
                }
            }
        }
        s.notify();
    });
    producer.join().unwrap();

    let (lock, cv) = &*done;
    let mut g = lock.lock().unwrap();
    while !*g {
        let (ng, _) = cv.wait_timeout(g, Duration::from_secs(30)).unwrap();
        g = ng;
        if *g {
            break;
        }
    }
    let elapsed = start.elapsed();
    run.shutdown();
    let mps = total as f64 / elapsed.as_secs_f64();
    (elapsed, mps)
}

// --- scenario 3: timer coalescing accuracy ------------------------------------

fn coalesce(rt: Rt, bursts: usize, burst: usize) -> (u64, bool) {
    let h = CountingHandler::default();
    let resizes = h.resizes.clone();
    let last = h.last_resize.clone();
    let run = spawn(rt, h.clone());

    let mut expected_last = (0u16, 0u16);
    for b in 0..bursts {
        for i in 0..burst {
            let cols = (80 + b * 10 + i) as u16;
            let rows = (24 + i) as u16;
            expected_last = (cols, rows);
            let _ = run.sender.send(Message::Resize { cols, rows });
            std::thread::sleep(Duration::from_micros(200));
        }
        std::thread::sleep(Duration::from_millis(60));
    }
    std::thread::sleep(Duration::from_millis(60));
    let fired = resizes.load(Ordering::Relaxed);
    let packed = last.load(Ordering::Relaxed);
    let got = (((packed >> 16) & 0xffff) as u16, (packed & 0xffff) as u16);
    run.shutdown();
    (fired, got == expected_last)
}

// --- scenario 4: idle CPU over N seconds --------------------------------------

#[cfg(unix)]
fn cpu_time() -> Duration {
    use std::mem::MaybeUninit;
    unsafe {
        let mut ru = MaybeUninit::<LibcRusage>::zeroed().assume_init();
        getrusage(0 /* RUSAGE_SELF */, &mut ru);
        let u = Duration::new(
            ru.ru_utime.tv_sec as u64,
            (ru.ru_utime.tv_usec * 1000) as u32,
        );
        let s = Duration::new(
            ru.ru_stime.tv_sec as u64,
            (ru.ru_stime.tv_usec * 1000) as u32,
        );
        u + s
    }
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct Timeval {
    tv_sec: isize,
    tv_usec: isize,
}
#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct LibcRusage {
    ru_utime: Timeval,
    ru_stime: Timeval,
    _pad: [isize; 32],
}
#[cfg(unix)]
unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut LibcRusage) -> i32;
}

#[cfg(unix)]
fn idle_cpu(rt: Rt, secs: u64) -> Duration {
    let run = spawn(rt, CountingHandler::default());
    std::thread::sleep(Duration::from_millis(100));
    let before = cpu_time();
    std::thread::sleep(Duration::from_secs(secs));
    let after = cpu_time();
    run.shutdown();
    after.saturating_sub(before)
}

#[cfg(not(unix))]
fn idle_cpu(_rt: Rt, _secs: u64) -> Duration {
    Duration::ZERO
}

// --- scenario 5: backpressure-unlock (deadlock avoidance) ---------------------

fn backpressure(rt: Rt) -> (bool, u128) {
    let render_lock: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

    struct H {
        render_lock: Arc<Mutex<u64>>,
    }
    impl Handler for H {
        fn on_messages(&mut self, batch: &[Message]) {
            let mut g = self.render_lock.lock().unwrap();
            *g += batch.len() as u64;
        }
        fn on_resize(&mut self, _: u16, _: u16) {}
        fn on_sync_reset(&mut self) {}
    }

    let run = spawn(
        rt,
        H {
            render_lock: render_lock.clone(),
        },
    );

    let s = run.sender.clone();
    let rl = render_lock.clone();
    let handoff = Arc::new(AtomicU64::new(0));
    let hoff = handoff.clone();
    let done = Arc::new(AtomicU64::new(0));
    let d2 = done.clone();

    let producer = std::thread::spawn(move || {
        let mut guard = rl.lock().unwrap();
        for _ in 0..mailbox::CAPACITY {
            let _ = s.try_send(Message::small(b"fill"));
        }
        // Queue full, holding the lock the consumer needs. Must not deadlock.
        let t0 = Instant::now();
        guard = s.send_with_unlock(Message::small(b"probe"), guard, &rl);
        let dt = t0.elapsed().as_nanos();
        hoff.store(dt as u64, Ordering::SeqCst);
        *guard += 0;
        drop(guard);
        d2.store(1, Ordering::SeqCst);
    });

    let start = Instant::now();
    let no_deadlock = loop {
        if done.load(Ordering::SeqCst) == 1 {
            break true;
        }
        if start.elapsed() > Duration::from_secs(5) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    if no_deadlock {
        let _ = producer.join();
    }
    run.shutdown();
    (no_deadlock, handoff.load(Ordering::SeqCst) as u128)
}

// --- driver -------------------------------------------------------------------

fn fmt_ns(ns: u128) -> String {
    if ns >= 1_000_000 {
        format!("{:.2}ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.2}\u{00b5}s", ns as f64 / 1e3)
    } else {
        format!("{ns}ns")
    }
}

fn run_all() {
    println!("# M2-C runtime benchmark (release)\n");
    let rts = [Rt::Threads, Rt::Tokio];

    println!("## wakeup latency (posted -> handler runs)");
    println!("| runtime | idle p50 | idle p99 | flood p50 | flood p99 |");
    println!("|---|---|---|---|---|");
    for rt in rts {
        let (ip50, ip99) = latency(rt, 2000, true);
        let (fp50, fp99) = latency(rt, 5000, false);
        println!(
            "| {} | {} | {} | {} | {} |",
            rt.name(),
            fmt_ns(ip50),
            fmt_ns(ip99),
            fmt_ns(fp50),
            fmt_ns(fp99)
        );
    }

    println!("\n## mailbox flood throughput (1,000,000 msgs)");
    println!("| runtime | elapsed | msgs/sec |");
    println!("|---|---|---|");
    for rt in rts {
        let (el, mps) = flood(rt, 1_000_000);
        println!(
            "| {} | {:.3}s | {:.2}M/s |",
            rt.name(),
            el.as_secs_f64(),
            mps / 1e6
        );
    }

    println!("\n## resize coalescing (10 bursts x 20 resizes; ideal fired=10)");
    println!("| runtime | callbacks fired | final dims correct |");
    println!("|---|---|---|");
    for rt in rts {
        let (fired, ok) = coalesce(rt, 10, 20);
        println!("| {} | {} | {} |", rt.name(), fired, ok);
    }

    println!("\n## idle CPU over 10s (process user+sys delta)");
    println!("| runtime | cpu time |");
    println!("|---|---|");
    for rt in rts {
        let cpu = idle_cpu(rt, 10);
        println!("| {} | {} |", rt.name(), fmt_ns(cpu.as_nanos()));
    }

    println!("\n## backpressure-unlock (full queue, producer holds render lock)");
    println!("| runtime | no deadlock | handoff latency |");
    println!("|---|---|---|");
    for rt in rts {
        let (ok, hl) = backpressure(rt);
        println!("| {} | {} | {} |", rt.name(), ok, fmt_ns(hl));
    }
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    match arg.as_str() {
        "all" => run_all(),
        "latency" => {
            for rt in [Rt::Threads, Rt::Tokio] {
                let (ip50, ip99) = latency(rt, 2000, true);
                let (fp50, fp99) = latency(rt, 5000, false);
                println!(
                    "{}: idle p50={} p99={} | flood p50={} p99={}",
                    rt.name(),
                    fmt_ns(ip50),
                    fmt_ns(ip99),
                    fmt_ns(fp50),
                    fmt_ns(fp99)
                );
            }
        }
        "flood" => {
            for rt in [Rt::Threads, Rt::Tokio] {
                let (el, mps) = flood(rt, 1_000_000);
                println!(
                    "{}: {:.3}s {:.2}M/s",
                    rt.name(),
                    el.as_secs_f64(),
                    mps / 1e6
                );
            }
        }
        "coalesce" => {
            for rt in [Rt::Threads, Rt::Tokio] {
                let (fired, ok) = coalesce(rt, 10, 20);
                println!("{}: fired={} final_ok={}", rt.name(), fired, ok);
            }
        }
        "idle" => {
            for rt in [Rt::Threads, Rt::Tokio] {
                let cpu = idle_cpu(rt, 10);
                println!("{}: idle cpu(10s)={}", rt.name(), fmt_ns(cpu.as_nanos()));
            }
        }
        "backpressure" => {
            for rt in [Rt::Threads, Rt::Tokio] {
                let (ok, hl) = backpressure(rt);
                println!("{}: no_deadlock={} handoff={}", rt.name(), ok, fmt_ns(hl));
            }
        }
        other => {
            eprintln!("unknown scenario: {other}");
            eprintln!("usage: bench [all|latency|flood|coalesce|idle|backpressure]");
            std::process::exit(2);
        }
    }
}
