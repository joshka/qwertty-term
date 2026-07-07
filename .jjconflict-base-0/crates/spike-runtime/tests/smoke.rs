//! CI smoke test: proves the mailbox API and BOTH drivers work end-to-end with
//! the identical calling code. Fast (no benchmarking); keeps
//! `cargo test --workspace` green. The heavy measurement lives in the `bench`
//! bin (`cargo run -p spike-runtime --release --bin bench`).

use spike_runtime::driver::{Driver, DriverHandle};
use spike_runtime::mailbox::{self, Message, Waker};
use spike_runtime::{CountingHandler, threads, tokio_rt};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Run a closure against a spawned driver, giving it the sender + counters.
/// Generic over the runtime so both share one test body.
fn with_threads<F: FnOnce(&mailbox::Sender, &CountingHandler)>(f: F) {
    let handler = CountingHandler::default();
    let observe = handler.clone();
    let driver = threads::ThreadsDriver::new().unwrap();
    let waker: Arc<dyn Waker> = driver.waker() as Arc<dyn Waker>;
    let (sender, recv) = mailbox::channel(waker);
    let stop = driver.handle();
    let join = std::thread::spawn(move || driver.run(recv, handler));
    f(&sender, &observe);
    stop.stop();
    let _ = join.join();
}

fn with_tokio<F: FnOnce(&mailbox::Sender, &CountingHandler)>(f: F) {
    let handler = CountingHandler::default();
    let observe = handler.clone();
    let driver = tokio_rt::TokioDriver::new().unwrap();
    let waker: Arc<dyn Waker> = driver.waker() as Arc<dyn Waker>;
    let (sender, recv) = mailbox::channel(waker);
    let stop = driver.handle();
    let join = std::thread::spawn(move || driver.run(recv, handler));
    f(&sender, &observe);
    stop.stop();
    let _ = join.join();
}

fn wait_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    pred()
}

fn body_messages_delivered(sender: &mailbox::Sender, h: &CountingHandler) {
    for i in 0..10u8 {
        sender.send(Message::small(&[i])).unwrap();
    }
    assert!(
        wait_until(Duration::from_secs(2), || h
            .messages
            .load(Ordering::Relaxed)
            == 10),
        "expected 10 messages, got {}",
        h.messages.load(Ordering::Relaxed)
    );
}

fn body_resize_coalesces(sender: &mailbox::Sender, h: &CountingHandler) {
    // Burst of resizes within the 25ms window -> exactly one on_resize.
    for c in 80..100u16 {
        sender.send(Message::Resize { cols: c, rows: 24 }).unwrap();
    }
    assert!(
        wait_until(Duration::from_secs(2), || h.resizes.load(Ordering::Relaxed)
            >= 1),
        "resize never fired"
    );
    // Give the window fully clear; should still be a single coalesced callback.
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        h.resizes.load(Ordering::Relaxed),
        1,
        "burst should coalesce to exactly one on_resize"
    );
    let packed = h.last_resize.load(Ordering::Relaxed);
    assert_eq!((packed >> 16) & 0xffff, 99, "latest resize dims should win");
}

fn body_backpressure_no_deadlock(sender: &mailbox::Sender, _h: &CountingHandler) {
    use std::sync::Mutex;
    let lock = Arc::new(Mutex::new(0u64));
    // Fill the queue while holding an unrelated lock, then send_with_unlock.
    let g = lock.lock().unwrap();
    for _ in 0..mailbox::CAPACITY {
        let _ = sender.try_send(Message::small(b"x"));
    }
    // Even though the consumer's handler here does not need `lock`, the API
    // contract must complete without deadlock and return the guard.
    let g = sender.send_with_unlock(Message::small(b"probe"), g, &lock);
    drop(g);
}

#[test]
fn threads_messages_delivered() {
    with_threads(body_messages_delivered);
}

#[test]
fn tokio_messages_delivered() {
    with_tokio(body_messages_delivered);
}

#[test]
fn threads_resize_coalesces() {
    with_threads(body_resize_coalesces);
}

#[test]
fn tokio_resize_coalesces() {
    with_tokio(body_resize_coalesces);
}

#[test]
fn threads_backpressure_no_deadlock() {
    with_threads(body_backpressure_no_deadlock);
}

#[test]
fn tokio_backpressure_no_deadlock() {
    with_tokio(body_backpressure_no_deadlock);
}
