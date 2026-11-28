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

/// One test at a time: they move the same sinks' volumes (and a stream
/// moves where every SinkVolume looks).
static SERIAL: Mutex<()> = Mutex::new(());

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
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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

/// Silence for the stream.
struct Silence;

impl sonkore_engine::output::Render for Silence {
    fn render(&self, out: &mut [i32]) {
        out.fill(0);
    }
}

#[test]
fn sink_volume_follows_the_sink_the_stream_is_linked_to() {
    // An Easy Effects filter takes the streams over (moves them to its own
    // sink): the volume heard is that sink's, not the one Sonkore asked for.
    // The test daemon moves the streams to the sink named in
    // $XDG_RUNTIME_DIR/test-link-target.
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !test_daemon() {
        eprintln!("skipped: not under tests/pipewire/run.sh");
        return;
    }
    use sonkore_engine::output::{self, Request};
    let filter = "filter-sink";
    let made = std::process::Command::new("pw-cli")
        .args([
            "create-node",
            "adapter",
            &format!(
                "{{ factory.name=support.null-audio-sink node.name={filter} node.description=\"Filter Sink\" \
                 media.class=Audio/Sink audio.position=[ FL FR ] object.linger=true }}"
            ),
        ])
        .output();
    if !made.is_ok_and(|o| o.status.success()) || !wait_for(|| pipewire::sinks().iter().any(|s| s.name == filter)) {
        eprintln!("skipped: no pw-cli to make a second sink");
        return;
    }
    let target = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("test-link-target");
    let _ = std::fs::remove_file(&target);
    let request = Request { rate: 48000, channels: 2, follow_rate: false, dsd: None };
    let _stream = output::open("pipewire|test-sink", request, std::sync::Arc::new(Silence)).expect("a stream");
    let volume = SinkVolume::open(SINK).expect("test-sink");
    assert!(wait_for(|| volume.sink_name() == SINK), "watching {}", volume.sink_name());
    volume.set_volume(0.8);
    assert!(wait_for(|| (volume.volume() - 0.8).abs() < 1e-6), "test-sink volume {}", volume.volume());
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    volume.set_on_change(move || {
        let _ = tx.lock().unwrap().send(());
    });

    std::fs::write(&target, filter).unwrap();
    assert!(wait_for(|| volume.sink_name() == filter), "watching {}", volume.sink_name());
    assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok(), "the change was reported");
    assert_eq!(volume.sink_description(), "Filter Sink");
    // Its volume is the one moved.
    volume.set_volume(0.5);
    let other = SinkVolume::open(filter).expect("filter-sink");
    assert!(wait_for(|| (other.volume() - 0.5).abs() < 1e-6), "filter volume {}", other.volume());
    drop(other);

    // Moved back: test-sink's own volume, untouched meanwhile.
    std::fs::remove_file(&target).unwrap();
    assert!(wait_for(|| volume.sink_name() == SINK), "watching {}", volume.sink_name());
    assert!(wait_for(|| (volume.volume() - 0.8).abs() < 1e-6), "test-sink volume {}", volume.volume());
    volume.set_volume(1.0);
}
