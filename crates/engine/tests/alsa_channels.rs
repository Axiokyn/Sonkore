//! Multichannel order on an ALSA `hw:` device, end to end: this test's own
//! ALSA configuration makes `hw:` a file plugin (what the device gets, to a
//! file) over a null device that reports ALSA's 5.1 map (FL FR RL RR FC LFE).
//! A 5.1 WAV (FL FR FC LFE RL RR) has to reach it in the card's order.
//! Its own test binary: ALSA reads its configuration once per process.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sonkore_engine::Engine;

/// A 16-bit 6-channel WAV whose channel c holds the constant (c + 1) × 1000.
fn write_wav(path: &Path, frames: usize) {
    let channels = 6u16;
    let mut data = Vec::new();
    for _ in 0..frames {
        for c in 0..channels {
            data.extend_from_slice(&((c as i16 + 1) * 1000).to_le_bytes());
        }
    }
    let rate = 48_000u32;
    let block = channels as u32 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * block).to_le_bytes());
    w.extend_from_slice(&(block as u16).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).unwrap();
}

#[test]
fn five_one_reaches_the_card_in_its_order() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("alsa-channels-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("device.raw");
    let conf = dir.join("asound.conf");
    std::fs::write(
        &conf,
        format!(
            r#"pcm.!default {{ type null }}
pcm.hw {{
    @args [ CARD DEV ]
    @args.CARD {{ type string default 0 }}
    @args.DEV {{ type integer default 0 }}
    type file
    slave.pcm {{ type null chmap [ "FL,FR,RL,RR,FC,LFE" ] }}
    file "{}"
    format raw
}}
"#,
            raw.display()
        ),
    )
    .unwrap();
    // SAFETY: the only test in this binary, before any ALSA call.
    unsafe { std::env::set_var("ALSA_CONFIG_PATH", &conf) };
    let wav = dir.join("five-one.wav");
    write_wav(&wav, 48_000 / 2);

    let mut engine = Engine::new();
    engine.set_device("alsa|hw:CARD=Test,DEV=0");
    if let Err(e) = engine.load(&wav, 0.0) {
        eprintln!("no ALSA here: {e}");
        return;
    }
    assert_eq!(engine.device_format().unwrap().channels, 6);
    engine.play();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine.finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(engine);
    let bytes = std::fs::read(&raw).unwrap();
    let samples: Vec<i32> = bytes.chunks_exact(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    // A frame well inside the music (past the fade-in).
    let frames: Vec<&[i32]> = samples.chunks_exact(6).filter(|f| f.iter().all(|&v| v != 0)).collect();
    assert!(frames.len() > 10_000, "{} frames of music", frames.len());
    let frame = frames[frames.len() / 2];
    let source: Vec<i32> = frame.iter().map(|v| (v >> 16) / 1000 - 1).collect();
    // The card's FL FR RL RR FC LFE: the source's channels 0 1 4 5 2 3.
    assert_eq!(source, vec![0, 1, 4, 5, 2, 3], "device channel → source channel");
    let _ = std::fs::remove_dir_all(&dir);
}
