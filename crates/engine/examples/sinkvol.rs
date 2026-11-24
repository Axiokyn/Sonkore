//! The volume of a PipeWire sink: prints it, sets it, follows its changes.
//!
//!   sinkvol [--sink NAME] [--set LINEAR] [--watch SECONDS]
//!
//! Without --sink: the desktop's default sink (followed when it changes).
//! Try it against the test daemon only (tests/pipewire/run.sh).

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use sonkore_engine::output::pipewire::SinkVolume;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("sinkvol: PipeWire only");
}

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let sink = arg("--sink").unwrap_or_default();
    let Some(vol) = SinkVolume::open(&sink) else {
        eprintln!("no such sink (or no PipeWire)");
        std::process::exit(1);
    };
    let show = |what: &str, v: &SinkVolume| {
        println!(
            "{what}: sink={} ({}) volume={:.4} hardware={} device={}",
            v.sink_name(),
            v.sink_description(),
            v.volume(),
            v.hardware(),
            v.device()
        );
    };
    show("open", &vol);
    if let Some(set) = arg("--set").and_then(|s| s.parse::<f64>().ok()) {
        vol.set_volume(set);
        std::thread::sleep(Duration::from_millis(300));
        show("set", &vol);
    }
    if let Some(secs) = arg("--watch").and_then(|s| s.parse::<f64>().ok()) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        vol.set_on_change(move || {
            let _ = tx.lock().unwrap().send(());
        });
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(secs);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            if rx.recv_timeout(left).is_ok() {
                show("changed", &vol);
            }
        }
    }
    drop(vol);
    println!("closed");
}
