//! Plays files with the Rust engine, to try it from a terminal:
//!   cargo run --release -p sonkore-engine --example play -- [--device KEY] [--follow-rate] [--volume 0.5] FILE...
//! KEY as in `--example devices` ("" = the system's default, "null" = ALSA's
//! silent device). Prints the position, whether the path is bit-perfect, and
//! the memory used at the end.

use std::time::{Duration, Instant};

use sonkore_engine::{Engine, Player};

fn main() {
    let mut device = String::new();
    let mut follow = false;
    let mut volume = 1.0;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--device" => device = args.next().unwrap_or_default(),
            "--follow-rate" => follow = true,
            "--volume" => volume = args.next().and_then(|v| v.parse().ok()).unwrap_or(1.0),
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        eprintln!("play [--device KEY] [--follow-rate] [--volume V] FILE...");
        std::process::exit(2);
    }
    let mut engine = Engine::new();
    engine.set_device(&device);
    engine.set_follow_rate(follow);
    engine.set_volume(volume);
    let mut player = Player::new(engine);
    player.play_paths(files, 0);
    if !player.last_error().is_empty() {
        eprintln!("{}", player.last_error());
    }
    let started = Instant::now();
    let mut last = String::new();
    loop {
        let changes = player.poll();
        if changes.finished || !player.loaded() {
            break;
        }
        let e = player.engine();
        let line = format!(
            "{} · {:.1} / {:.1} s · {} · {}",
            player.current_path().unwrap_or(""),
            e.position(),
            e.duration(),
            e.device_format()
                .map(|f| format!("{} Hz {} bit{}", f.rate, f.bits, if f.float { " float" } else { "" }))
                .unwrap_or_default(),
            if e.bit_perfect() { "bit-perfect" } else { "processed" }
        );
        if line != last && started.elapsed().as_millis() % 500 < 60 {
            println!("{line}");
            last = line;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for key in ["VmHWM", "VmRSS"] {
        if let Some(l) = status.lines().find(|l| l.starts_with(key)) {
            println!("{l}");
        }
    }
}
