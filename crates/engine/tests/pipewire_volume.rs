//! The PipeWire sink volume against the test daemon only:
//!
//!   tests/pipewire/run.sh cargo test --release -p sonkore-engine --test pipewire_volume
//!
//! Skips (passes) unless PipeWire runs and has the test daemon's "test-sink",
//! so it never touches a desktop's real sinks.

#![cfg(target_os = "linux")]

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sonkore_engine::output::pipewire::{self, SinkVolume};

const SINK: &str = "test-sink";

fn test_daemon() -> bool {
    pipewire::running() && pipewire::sinks().iter().any(|s| s.name == SINK)
}

/// Waits until `cond` holds (the daemon answers on its own thread).
fn wait_for(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

#[test]
fn sink_volume_follows_changes_from_outside() {
    if !test_daemon() {
        eprintln!("skipped: not under tests/pipewire/run.sh");
        return;
    }
    assert!(SinkVolume::open("no-such-sink").is_none());

    let ours = SinkVolume::open(SINK).expect("test-sink");
    assert_eq!(ours.sink_name(), SINK);
    assert_eq!(ours.sink_description(), "Test Sink");
    assert!(!ours.hardware(), "a null sink is virtual");
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    ours.set_on_change(move || {
        let _ = tx.lock().unwrap().send(());
    });

    // Another client (as the desktop's volume keys would) moves it.
    let outside = SinkVolume::open(SINK).expect("test-sink");
    outside.set_volume(0.25);
    assert!(rx.recv_timeout(Duration::from_secs(3)).is_ok(), "no change reported");
    assert!(wait_for(|| (ours.volume() - 0.25).abs() < 1e-6), "volume {}", ours.volume());

    // 0 mutes (and reads 0), above 0 unmutes.
    outside.set_volume(0.0);
    assert!(wait_for(|| ours.volume() == 0.0), "volume {}", ours.volume());
    ours.set_volume(0.5);
    assert!(wait_for(|| (outside.volume() - 0.5).abs() < 1e-6), "volume {}", outside.volume());
    assert!(wait_for(|| (ours.volume() - 0.5).abs() < 1e-6), "volume {}", ours.volume());

    // Out of range and non-finite values.
    ours.set_volume(2.0);
    assert!(wait_for(|| (outside.volume() - 1.0).abs() < 1e-6), "volume {}", outside.volume());
    ours.set_volume(f64::NAN);
    std::thread::sleep(Duration::from_millis(100));
    assert!((outside.volume() - 1.0).abs() < 1e-6);
    drop(outside);
    drop(ours);
}
