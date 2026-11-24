//! The signal path and the output's one-line descriptions (the output's
//! summary and description, the track's): the stages, wording and
//! translations earlier Sonkore versions used.

use sonkore_core::settings::Settings;
use sonkore_engine::Engine;
use sonkore_engine::decode::StreamInfo;

use crate::i18n::{locale_number, number_g, tr};

const CTX: &str = "DspController";

/// One stage of the path, as the popup lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub stage: String,
    pub detail: String,
    /// "lossless", "enhanced", "high", "low" or "warning".
    pub quality: &'static str,
    /// What the row's button does ("headroomAuto"), else empty.
    pub action: &'static str,
}

/// What the path needs besides the engine and the settings.
#[derive(Clone, Debug, Default)]
pub struct PathState {
    /// ReplayGain applied to this track (dB), None when off or unknown.
    pub gain_db: Option<f64>,
    /// Samples clipped in this track so far.
    pub clipped: u64,
    /// The volume is held at 100 % (playback/volumeFixed).
    pub volume_fixed: bool,
    /// The DAC's own control in use: (control name, linear volume).
    pub hardware_volume: Option<(String, f64)>,
    /// The sound server's volume for the sink: (linear, on the DAC's control).
    pub system_volume: Option<(f64, bool)>,
}

fn arg(text: String, values: &[&str]) -> String {
    let mut s = text;
    for (i, v) in values.iter().enumerate().rev() {
        s = s.replace(&format!("{{{i}}}"), v);
    }
    s
}

fn t(text: &str, values: &[&str]) -> String {
    arg(tr(CTX, text), values)
}

fn signed_db(db: f64) -> String {
    format!("{}{:.1}", if db >= 0.0 { "+" } else { "" }, db)
}

fn khz(hz: u32) -> String {
    number_g(hz as f64 / 1000.0, false)
}

/// "FLAC 24-bit · 96 kHz".
pub fn track_description(info: &StreamInfo) -> String {
    let mut s = info.codec.clone();
    if info.bits > 0 && !info.float {
        s += &format!(" {}-bit", info.bits);
    }
    s += &format!(" · {} kHz", khz(info.rate));
    if info.channels != 2 {
        s += &format!(" · {} ch", info.channels);
    }
    s
}

/// "Bit-perfect · 96 kHz" for the player bar.
pub fn output_summary(engine: &Engine) -> String {
    let s = engine.output_status();
    if s.backend.is_empty() || engine.info().is_none() {
        return String::new();
    }
    let rate =
        if !s.dsd.is_empty() { s.dsd.clone() } else { t("{0} kHz", &[&number_g(s.device_rate as f64 / 1000.0, true)]) };
    if s.bit_perfect_now {
        return if s.dsd.is_empty() { t("Bit-perfect · {0}", &[&rate]) } else { rate };
    }
    if s.upsampled || s.pcm_to_dsd_rate != 0 {
        return t("Upsampled · {0}", &[&rate]);
    }
    if s.resampled {
        return t("Resampled · {0}", &[&rate]);
    }
    t("Processed · {0}", &[&rate])
}

/// "ALSA · Topping DX1 · 96 kHz · s32 · exclusive".
pub fn output_description(engine: &Engine) -> String {
    let s = engine.output_status();
    if s.backend.is_empty() {
        return tr(CTX, "Output idle");
    }
    if !s.dsd.is_empty() {
        let rate = if s.pcm_to_dsd_rate != 0 { s.pcm_to_dsd_rate } else { engine.dsd_rate() };
        return t(
            "{0} · {1} · {2} · {3} MHz",
            &[&s.backend, &s.device_name, &s.dsd, &number_g(rate as f64 / 1e6, false)],
        );
    }
    let mut text = t(
        "{0} · {1} · {2} kHz · {3}{4}{5}",
        &[
            &s.backend,
            &s.device_name,
            &khz(s.device_rate),
            if s.device_float { "f" } else { "s" },
            &s.device_bits.to_string(),
            // A sound server's output is never held alone, whatever reaches it.
            &if s.exclusive && sonkore_engine::output::is_direct_key(engine.device_key()) {
                tr(CTX, " · exclusive")
            } else {
                String::new()
            },
        ],
    );
    if s.resampled {
        text += &t(" · resampled from {0} kHz (SoX VHQ)", &[&khz(s.source_rate)]);
    }
    text
}

/// The stages from the file to the device.
pub fn build(settings: &Settings, engine: &Engine, state: &PathState) -> Vec<Stage> {
    let mut out = Vec::new();
    let Some(info) = engine.info().cloned() else { return out };
    let mut add =
        |stage: String, detail: String, quality: &'static str| out.push(Stage { stage, detail, quality, action: "" });
    let s = engine.output_status();
    // PCM modulated to DSD goes through the PCM stages, then the modulator.
    let to_dsd = s.pcm_to_dsd_rate != 0;
    let dsd = !s.dsd.is_empty() && !to_dsd;
    let dsd_rate = engine.dsd_rate();
    let from_dsd = !dsd && dsd_rate != 0;
    let p = |k: &str| format!("playback/{k}");

    if dsd || from_dsd {
        add(
            tr(CTX, "Source"),
            t("{0} · {1} MHz", &[&sonkore_dsd::rate_label(dsd_rate), &number_g(dsd_rate as f64 / 1e6, false)]),
            "lossless",
        );
    } else {
        add(tr(CTX, "Source"), track_description(&info), if info.lossless { "lossless" } else { "low" });
    }
    let decoder = if dsd || from_dsd {
        if info.codec.contains("DST") { "DST decoder" } else { "DSD reader" }.to_owned()
    } else {
        // "Symphonia", or "FFmpeg (libavcodec 62.28)" for the formats it decodes.
        info.decoder.clone()
    };
    add(tr(CTX, "Decoder"), decoder, if info.lossless || from_dsd { "lossless" } else { "high" });
    if from_dsd {
        add(
            tr(CTX, "DSD → PCM"),
            t(
                "Double precision · {0} MHz → 352.8 kHz ({1}-tap FIR, 160 dB) → {2} kHz (SoX VHQ, flat to {3} kHz): DSD's ultrasonic noise removed",
                &[
                    &number_g(dsd_rate as f64 / 1e6, false),
                    &sonkore_dsd::DsdToPcm::first_stage_taps(dsd_rate).to_string(),
                    &khz(info.rate),
                    &number_g(sonkore_dsd::DsdToPcm::second_stage(dsd_rate).pass_hz / 1000.0, false),
                ],
            ),
            "high",
        );
    }
    if s.hdcd {
        add(
            tr(CTX, "HDCD"),
            tr(CTX, "Decoded to 20 bits (peak extend and gain control), at -6 dB to leave room for the extended peaks"),
            "lossless",
        );
        out.last_mut().unwrap().quality = "enhanced";
    }
    let mut add =
        |stage: String, detail: String, quality: &'static str| out.push(Stage { stage, detail, quality, action: "" });
    let dsp = engine.dsp();
    let volume = engine.volume();
    let hw_volume = state.hardware_volume.is_some();
    let rg_on = settings.int(&p("replayGain"), 0) != 0 && state.gain_db.is_some_and(|g| g != 0.0);
    let eq_neutral = dsp.eq.is_neutral();
    let st = &dsp.stretch;
    let stretch_neutral = st.is_neutral();
    if dsd {
        let native = s.dsd.contains("native");
        add(
            tr(CTX, "DSD"),
            if native { tr(CTX, "Sent as native DSD") } else { tr(CTX, "Sent as DoP (DSD over PCM)") },
            "lossless",
        );
    } else {
        let snapshot = dsp.snapshot();
        let crossfeed = snapshot.crossfeed;
        let crossfeed_on = crossfeed != 0 && info.channels == 2;
        let conv = snapshot.impulse.clone();
        if let Some(ir) = &conv {
            let layout = match ir.channels {
                4 => tr(CTX, "true stereo"),
                2 => tr(CTX, "stereo"),
                _ => tr(CTX, "mono"),
            };
            let name = ir.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut detail =
                t("{0} · {1} taps · {2}", &[&name, &ir.taps.first().map_or(0, Vec::len).to_string(), &layout]);
            if ir.sample_rate != info.rate {
                detail += &t(" · resampled from {0} kHz", &[&khz(ir.sample_rate)]);
            }
            add(tr(CTX, "Convolution"), detail, "enhanced");
        }
        let dsp_active = !stretch_neutral
            || !eq_neutral
            || s.resampled
            || crossfeed_on
            || conv.is_some()
            || (!hw_volume && volume != 1.0)
            || rg_on;
        let headroom = snapshot.headroom_db;
        if dsp_active && headroom < 0.0 {
            let text = if settings.flag(&p("headroomAuto"), false) {
                "{0} dB ahead of the DSP (automatic)"
            } else {
                "{0} dB ahead of the DSP"
            };
            add(tr(CTX, "Headroom"), t(text, &[&signed_db(headroom)]), "high");
        }
        if !stretch_neutral {
            add(
                tr(CTX, "Pitch / tempo"),
                t(
                    "{0} · pitch {1} st · tempo {2}%",
                    &[
                        "SoundTouch",
                        &signed_db(st.pitch_semitones()),
                        &format!("{:.1}", st.tempo() * st.rate() * 100.0),
                    ],
                ),
                "enhanced",
            );
        }
        if !eq_neutral {
            let mut detail =
                t("{0} bands · preamp {1} dB", &[&dsp.eq.bands().len().to_string(), &signed_db(dsp.eq.preamp_db())]);
            if dsp.eq.linear_phase() {
                detail += &t(
                    " · linear phase, {0} taps",
                    &[&sonkore_eq::linear_phase::taps_for_rate(info.rate as f64).to_string()],
                );
            }
            add(tr(CTX, "Equalizer"), detail, "enhanced");
        }
        if crossfeed_on {
            let preset = sonkore_dsp::crossfeed::Preset::from_index(crossfeed);
            let (cutoff, feed) = preset.parameters();
            let names = [String::new(), tr(CTX, "Default"), "Chu Moy".into(), "Jan Meier".into()];
            add(
                tr(CTX, "Crossfeed"),
                t(
                    "{0} · {1} Hz · {2} dB",
                    &[&names[crossfeed.clamp(0, 3) as usize], &cutoff.to_string(), &format!("{feed:.1}")],
                ),
                "enhanced",
            );
        }
        if rg_on {
            let mode = if settings.int(&p("replayGain"), 0) == 2 { tr(CTX, "Album") } else { tr(CTX, "Track") };
            add(
                tr(CTX, "ReplayGain"),
                t("{0} · {1} dB", &[&mode, &signed_db(state.gain_db.unwrap_or(0.0))]),
                "enhanced",
            );
        }
        if !hw_volume && volume != 1.0 {
            let detail = if volume > 0.0 {
                t("Digital · {0} dB", &[&signed_db(20.0 * volume.log10())])
            } else {
                tr(CTX, "Digital · muted")
            };
            add(tr(CTX, "Volume"), detail, "high");
        }
        if snapshot.limiter && dsp_active {
            let reduction = engine.limiter_reduction_db();
            let ceiling = locale_number(snapshot.limiter_ceiling_db, 1);
            if reduction < -0.05 {
                add(
                    tr(CTX, "Limiter"),
                    t("True peak, ceiling {0} dBTP · reducing {1} dB", &[&ceiling, &locale_number(reduction, 1)]),
                    "enhanced",
                );
            } else {
                add(tr(CTX, "Limiter"), t("True peak, ceiling {0} dBTP · not reducing", &[&ceiling]), "lossless");
            }
        }
        let filters = [
            tr(CTX, "linear phase"),
            tr(CTX, "intermediate phase"),
            tr(CTX, "minimum phase"),
            tr(CTX, "linear phase, steep"),
        ];
        let filter = &filters[snapshot.resample_filter.clamp(0, 3) as usize];
        if to_dsd {
            add(
                tr(CTX, "PCM → DSD"),
                t(
                    "SoX VHQ, {0} · {1} kHz → {2} MHz · 8th-order sigma-delta modulator",
                    &[filter, &khz(s.source_rate), &number_g(s.pcm_to_dsd_rate as f64 / 1e6, false)],
                ),
                "enhanced",
            );
            if s.modulator_resets > 0 {
                add(
                    tr(CTX, "Modulator"),
                    t("{0} overloads: lower the headroom", &[&s.modulator_resets.to_string()]),
                    "low",
                );
            }
        } else if s.resampled {
            let fixed = settings.int(&p("fixedRate"), 0) != 0 && !s.upsampled;
            let mut detail = t("SoX VHQ, {0} · {1} → {2} kHz", &[filter, &khz(s.source_rate), &khz(s.device_rate)]);
            if fixed {
                detail += &tr(CTX, " · fixed output rate");
            }
            add(
                if s.upsampled { tr(CTX, "Upsampling") } else { tr(CTX, "Sample rate conversion") },
                detail,
                if s.upsampled { "enhanced" } else { "high" },
            );
        }
        // A source wider than the device is requantised even untouched.
        let src_bits = if s.hdcd {
            24
        } else if info.bits > 0 && !info.float {
            info.bits
        } else {
            32
        };
        let device_bits = if s.device_float { 24 } else { s.device_bits };
        let wider = src_bits > device_bits;
        let processed =
            s.resampled || crossfeed_on || conv.is_some() || !stretch_neutral || !eq_neutral || volume != 1.0 || rg_on;
        if !to_dsd && !s.bit_perfect_now && (wider || (!s.hdcd && processed)) {
            let bits = if s.device_float { 24 } else { s.device_bits.min(24) };
            let dither_default = if settings.flag(&p("dither"), true) { 1 } else { 0 };
            let mode = settings.int(&p("ditherMode"), dither_default).clamp(0, 2);
            let shaped = mode == 2 && bits <= 20 && (s.device_rate == 44_100 || s.device_rate == 48_000);
            let detail = if shaped {
                t("{0}-bit with noise-shaped dither", &[&bits.to_string()])
            } else if mode != 0 {
                t("{0}-bit with TPDF dither", &[&bits.to_string()])
            } else {
                t("{0}-bit", &[&bits.to_string()])
            };
            add(tr(CTX, "Requantisation"), detail, "high");
        }
    }
    if state.volume_fixed {
        add(tr(CTX, "Volume"), tr(CTX, "Fixed at 100 %: set on the DAC or the amplifier"), "lossless");
    }
    if let Some((v, hw)) = state.system_volume {
        let detail = if v > 0.0 {
            let db = signed_db(20.0 * v.log10());
            if hw {
                t("System volume on the DAC's control · {0} dB", &[&db])
            } else {
                t("System volume, in PipeWire's software · {0} dB", &[&db])
            }
        } else {
            tr(CTX, "System volume · muted")
        };
        add(tr(CTX, "Volume"), detail, if hw || v == 1.0 { "lossless" } else { "high" });
    }
    if let Some((name, v)) = &state.hardware_volume {
        let detail = if *v > 0.0 {
            t("DAC control \"{0}\" · {1} dB", &[name, &signed_db(20.0 * v.log10())])
        } else {
            t("DAC control \"{0}\" · muted", &[name])
        };
        add(tr(CTX, "Volume"), detail, "lossless");
    }
    if !dsd && s.device_channels != 0 && s.device_channels != info.channels {
        if info.channels == 1 && s.device_channels == 2 {
            add(tr(CTX, "Channels"), tr(CTX, "Mono copied to both channels, sample for sample"), "lossless");
        } else if s.device_channels < info.channels {
            add(
                tr(CTX, "Channels"),
                t(
                    "{0} channels mixed down to {1}: this output takes no more",
                    &[&info.channels.to_string(), &s.device_channels.to_string()],
                ),
                "high",
            );
        }
    }
    if state.clipped > 0 {
        let auto = settings.flag(&p("headroomAuto"), false);
        out.push(Stage {
            stage: tr(CTX, "Clipping"),
            detail: if auto {
                tr(CTX, "This track is clipping even with the automatic headroom: set a lower one by hand.")
            } else {
                tr(CTX, "This track is clipping: lower the headroom.")
            },
            quality: "warning",
            action: if auto { "" } else { "headroomAuto" },
        });
    }
    let mixer = !s.exclusive && !s.path_can_be_bit_perfect;
    let mut output = output_description(engine);
    if mixer {
        output += &tr(CTX, " · through the system mixer");
    }
    // (Granted through rtkit, or PipeWire's own real-time loop.)
    let rt_priority = engine.device_rt_priority();
    if rt_priority > 0 {
        output += &t(" · real-time audio thread (priority {0})", &[&rt_priority.to_string()]);
    }
    out.push(Stage {
        stage: tr(CTX, "Output"),
        detail: output,
        quality: if s.path_can_be_bit_perfect { "lossless" } else { "high" },
        action: "",
    });
    out
}

#[cfg(test)]
mod tests {
    use crate::i18n::number_g;

    #[test]
    fn numbers_as_g6() {
        assert_eq!(number_g(44.1, false), "44.1");
        assert_eq!(number_g(96.0, false), "96");
        assert_eq!(number_g(352.8, false), "352.8");
        assert_eq!(number_g(2.8224, false), "2.8224");
        assert_eq!(number_g(5.6448, false), "5.6448");
    }
}
