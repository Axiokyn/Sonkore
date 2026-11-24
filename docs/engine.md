# Sonkore's audio engine

What the engine (`crates/engine`, with `crates/dsp`, `crates/dsd`, `crates/eq`, `crates/soundtouch` and `crates/hdcd`) does and must keep doing: behaviour, constants, formulas and settings keys. Items marked **(not done yet)** are required behaviour the engine does not have yet.

## 1. Threads and the signal chain

**Threads**

- **Control thread.** The only caller of the engine's methods. The interface polls the engine every 50 ms while playing and every 250 ms otherwise.
- **Worker thread.**
  - Decodes and runs the DSP. It is the only thread that allocates or locks. It sets FTZ/DAZ (denormals as zero).
  - Load, seek and stop "park" it: a mutex, a condvar, an atomic request flag and a "parked" acknowledgement.
  - Idle (paused with a full ring, end of track, stopped, nothing loaded): waits on the condvar with no timeout; play, seek, load and park requests wake it.
  - Ring full while playing: sleeps the drain time, at least one device period and at most a quarter of the ring. A park request wakes it.
- **Device thread** (ALSA writer, PipeWire's real-time callback, WASAPI event thread).
  - It only copies from the ring.
  - Its first call records its kernel thread id. The worker hands it (once per device thread) to the engine's real-time requester, a thread of its own that asks rtkit (see §6) without ever blocking the audio threads. A thread already real-time (PipeWire's own data loop, or SCHED_FIFO the ALSA writer got directly) is not asked for; each thread is asked for once; a refusal is logged once per process and otherwise silent. The signal path's output row shows the priority it runs at.
- **Other threads:** preload of the next track; one sigma-delta worker per extra channel when modulating to DSD; the real-time requester (`sonkore-rtkit`); PipeWire's own loop (its events are sent back to the interface thread).

**Ring**

- SPSC, `i32` samples, power-of-two size, monotonic indices, acquire/release ordering, 64-byte aligned.
- In device frames: PCM frames, or DSD words when the device takes DSD (also for PCM→DSD, whose chain runs at the 1-bit rate).
- Sized after the device opens, at the rate it opened at (a device faster than the track: an 8 kHz file on a 48 kHz graph never played with the ring sized for 8 kHz): `max(rate*bufferMs/1000, 2*devicePeriod) + 1024` frames, and at least twice the room one block needs at that rate. `bufferMs` is 150, clamped 60–2000. Formats outside 1 kHz–3.072 MHz or above 8 channels are refused.
- It is written in partial frames and read in whole frames only.

**Shared state**

- **Atomics:** volume, track gain, headroom, dither, crossfeed, limiter, mute state, the frame counters (pushed, played, decoded) and the splice point.
- **Mutexes:**
  - output config;
  - gapless (next track);
  - convolver handoff, picked up at a block boundary;
  - the EQ's pending curve.
- **SoundTouch** settings are atomics plus dirty bits, applied at block boundaries.

**De-click mute**

- States: Open, FadingOut, Muted.
- The fade is a raised cosine of `max(64, rate*8/1000)` frames.
- Faded samples are requantised to the device grid with TPDF from a xorshift32 seeded `0x9E3779B9`.
- Muting waits for the device thread's acknowledgement; after 500 ms it stops the device.
- DSD has no fade: it switches to the silence pattern.
- After a rate change, the device plays `playback/resyncDelayMs` of silence (default 250, at most 3000) before the music, so the DAC can lock; the position waits for the music.

**Bit-perfect bypass, per block**

- DSD words are never touched.
- PCM is bypassed only when all of these hold:
  - the DSP is neutral;
  - no resampler;
  - `sourceBits <= deviceDepth`.
- `sourceBits`: 24 for HDCD, else the source's bits, else 32 for float or lossy.
- `deviceDepth`: 24 for a float device, else the device's bits.
- The DSP is neutral when all of these hold:
  - SoundTouch neutral;
  - EQ neutral, and no longer crossfading;
  - no convolver;
  - linear-phase EQ idle;
  - volume == 1.0 and track gain == 1.0 exactly;
  - crossfeed off, or channels != 2.
- On the way back to bypass, the gains first glide to 1 (one extra processed block) and the limiter's held frames are flushed.

**Processed chain**

1. **Decoder → f64** (`s32 / 2^31`; floats keep values above full scale). HDCD instead: s32 `>> 16`, then the HDCD decoder, giving 32-bit output at -6 dB.
2. **SoundTouch**, if active: f64 → f32, in 4096-frame chunks, → f64.
3. **Headroom gain**, glided linearly across the block. A new track snaps to its gain.
4. **Linear-phase EQ (FIR)**, in linear mode.
5. **Convolver.**
6. **Biquad EQ** (not in linear mode; it applies the preamp).
7. **Crossfeed.**
8. **Volume × ReplayGain gain**, glided.
9. **Resampler**, only when needed (see §4).
10. **True-peak limiter**, at the output rate (the DSD rate when modulating).
11. **Quantizer** (dither) to the device grid, or the **sigma-delta** modulator → ring.

## 2. Outputs

**Device keys** are `"<backend>|<id>"`:

- `pipewire|<node.name>`, with WirePlumber's ".N" suffix stripped (a second sink with the same base name keeps its full name); opening matches the current node by base name;
- `alsa|hw:CARD=<id>,DEV=<n>`, from `/proc/asound/cards` and `cardN/pcmNp/info`;
- `wasapi|<friendly name>` on Windows (`wasapi|<endpoint id>` for a second endpoint with the same name).

`auto_null` is skipped. A saved `pulse|X` maps to `pipewire|X`. `""` means the system default: PipeWire's default sink, else ALSA's `default` (on Windows, the default endpoint). DSD needs a direct `alsa|hw:` device (or WASAPI exclusive, as DoP).

**ALSA**

- `hw:` devices are opened directly, without format, channel or rate conversion, in the card's integer format (S32, S24 in 32, packed S24, S16); 3 periods × 40 ms.
- A busy device (just let go by the sound server) is retried 40 × 25 ms.
- **Rate pick**, when the device refuses the track's rate:
  1. the lowest integer multiple above the wanted rate;
  2. else the lowest rate above it;
  3. else the highest integer divisor;
  4. else the highest rate.
  
  The device is opened at that rate and the engine's own resampler converts.
- **Channel order** for multichannel (`output::channels`): the engine's samples are in WAVE / FLAC order (mono; FL FR; FL FR FC; FL FR RL RR; FL FR FC RL RR; FL FR FC LFE RL RR; FL FR FC LFE RC SL SR; FL FR FC LFE RL RR SL SR). On a `hw:` device with 3 or more channels:
  1. a channel map the driver lets be chosen (`SND_CHMAP_TYPE_VAR`) is set to the source's order;
  2. otherwise the frames are reordered on the device thread from the map the device reports, or, when it reports none, ALSA's standard order (5: FL FR RL RR FC; 6: + LFE; 8: + SL SR).
  
  Samples are only moved, never mixed: exact positions first, then side and rear (back) surrounds stand in for each other. A layout that would need mixing (a channel with nowhere to go, driver-specific positions) goes out as it comes. DSD words are moved the same way.
- **Mono:** if the device refuses mono, open stereo and duplicate.
- **Quantizer target:** `float ? 24 : min(bits, 24)`.

**Bit-perfect**

- The path can be bit-perfect when all of these hold:
  - no resampling;
  - device rate == track rate;
  - channels match (mono→stereo counts);
  - depth OK;
  - an exclusive output, or PipeWire;
  - a lossless source;
  - channels exact.
- `bitPerfectNow = pathCanBeBitPerfect && !hdcd && (dspNeutral || DSD)`.
- On PipeWire it is also void when the graph rate ≠ the stream rate, or the system volume is not neutral.

**PipeWire** (libpipewire is loaded with dlopen)

- Stream: S32 at the track rate.
- `playback/followTrackRate` sets `clock.force-rate` in the "settings" metadata. It is held across a close and reset to 0 on release. Without it: a rate in `clock.allowed-rates` is used as is; otherwise force-rate, else `clock.rate`, else 48000, and the engine converts.
- Quantum `clamp(bit_ceil(rate/25), 256, 8192)`; buffer = 4 × quantum.
- The format carries the channel positions in WAVE / FLAC order (SPA FL FR FC LFE RL RR SL SR, RC for 7 channels); `stream.dont-remix` keeps PipeWire from mixing them.
- Properties:
  - `node.rate=1/R`, `node.latency=Q/R`;
  - `stream.dont-remix=true`, `state.restore-props=false`;
  - `node.name=sonkore`, role Music;
  - `target.object` and `node.target`.
- The stream's channelVolumes are forced to 1.
- It reports 32-bit float, so the quantizer targets 24 bits.
- No DSD. Mono is sent as stereo.

**WASAPI** (Windows)

- **Shared:** through the Windows mixer, at the mix format's rate in 32-bit float; the engine resamples to it (Windows' own conversion stays off). Buffer 120 ms.
- **Exclusive** (`playback/exclusive`, and always for DSD): the endpoint directly, in an integer format; bit-perfect when the rate matches; carries DoP.
- The WAVEFORMATEXTENSIBLE channel mask is the WAVE layout for the channel count (5.1 = 0x3F, 7.1 = 0x63F), so the samples need no reordering.
- Both are event driven on a thread of their own.

**PCM → DSD** (`playback/pcmToDsd`, see §4): a PCM track on a direct output, with `playback/dsdOutput` native or DoP and no fixed output rate, opens the device for DSD at the highest wanted DSD rate it takes (native: whichever endianness the card takes; DoP: the same checks as for DSD files). Nothing taken: the track plays as PCM, and later tracks on the same device and channel count do not ask again until the device or a DSD setting changes. Never on PipeWire.

**ALSA native DSD**

- `DSD_U32_BE/LE`, period 40 ms × 3, start threshold = the whole buffer, frame rate = DSD rate / 32.
- Only on `hw:` devices.

**DoP**

- Exclusive outputs only.
- Rate = DSD rate / 16; integer ≥ 24 bits; the path must be able to be bit-perfect.
- The device thread stamps the 0x05/0xFA markers. The marker phase is kept across gapless joins.
- DSD pre-roll: 60 ms of 0x69, an even number of frames.

**Device reservation** (`org.freedesktop.ReserveDevice1.Audio<card>`, on opening `alsa|hw:`)

- Acquired at priority 10, calling the holder's `RequestRelease(10)`.
- If the holder is Sonkore itself (still closing): 8 × 250 ms.
- After taking the card: opening is retried 40 × 50 ms.
- When the card is released, wait up to 3 s for the PipeWire sink to reappear.

**Idle release** (`playback/releaseDacWhenIdle`, not done yet): after 60 s in pause or stop, a held output is released.

**Hotplug** (Linux): a watch on `/dev/snd`, debounced 1.5 s, refreshes the device list and reports the outputs added and removed, by name.

## 3. Decoding

**Routing**

| Extension | Decoder | Notes |
|---|---|---|
| `.flac` | Symphonia | |
| `.wav`/`.wave` | Symphonia | honours the valid bits per sample; f32 not clipped |
| `.mp3`/`.mp2`/`.mp1` | Symphonia | float, lossy |
| `.m4a`/`.m4b`/`.mp4`/`.aac`, `.caf`, `.ogg`/`.oga`, `.aif`/`.aiff`/`.aifc`, `.mka`/`.webm` | Symphonia | ALAC's depth comes from its magic cookie; AIFF's frame count from COMM. A file symphonia opens but has no decoder for (Opus in `.ogg` or `.webm`) goes to FFmpeg |
| `.dsf`/`.dff` | Sonkore's DSD decoder (DST included) | to PCM, or DSD to the device |
| `.opus`, `.ape`, `.wv`, `.tta`, `.tak`, `.mpc`, `.wma`, `.w64`, `.ac3`, `.eac3`, `.dts` | FFmpeg, loaded at run time (`crates/ffmpeg`) | without FFmpeg: not played ("Playing … needs FFmpeg, which is not installed") and not measured; still listed with their tags |

**FFmpeg** (`crates/ffmpeg`: `sonkore_ffmpeg::Stream`, wrapped by `decode::FfmpegDecoder`)

- Loaded the first time a file needs it, never linked: `libavutil` / `libavcodec` / `libavformat` `.so.<major>` with dlopen (Windows: `avutil-<major>.dll`… next to the exe or on the PATH, LoadLibrary). Releases tried newest first: FFmpeg 8 (libavformat 62, libavcodec 62, libavutil 60), 7 (61/61/59), 6 (60/60/58), 5 (59/59/57); the three libraries of one release, accepted only if `avformat_version()` and the others give those majors. 64-bit only. If none loads, one line on stderr and each such file fails to open.
- ABI: only functions whose signature is the same in 5 to 8; struct fields read at offsets per major (`abi.rs`), taken from the real headers of 5.0, 5.1, 6.0, 6.1, 7.0, 7.1 and 8.0 (`tools/ffmpeg-offsets.sh`). The test `offsets_match_the_installed_headers` compiles the same program against the system's headers and the downloaded ones; at load, AVCodecContext's offsets are checked by setting fields through `av_opt_set*` and reading them back (a mismatch refuses FFmpeg). Channel counts: `channels` up to 6.x, `ch_layout.nb_channels` from 7.0.
- I/O through Sonkore's own reader (custom AVIOContext): Play from memory applies, and FFmpeg opens no file, URL or protocol of its own. One decoding thread (`threads=1`), `pkt_timebase` set so skipped samples move the timestamps. FFmpeg's log is silent unless `SONKORE_FFMPEG_LOG` is set.
- Output: lossless integer codecs (APE, WavPack, TTA, TAK, WMA Lossless, PCM in Wave64, DTS-HD MA) as left-justified s32 with `bits` = the decoder's `bits_per_raw_sample` (bit-perfect path); lossy ones (Opus at 48 kHz, WMA, Musepack, AC-3, E-AC-3, DTS core) and float WavPack as f64.
- Length: the stream's duration (Ogg: minus `initial_padding`), else the container's (Matroska, estimated). Opus pre-skip and end trimming are FFmpeg's (skip-samples side data): the source's exact frame count.
- Seek: `av_seek_frame(BACKWARD)` to the target (lossy codecs: 100 ms earlier, so the decoder settles), then the frames before it are dropped by their timestamps. When the time base is coarser than a sample (Matroska and ASF count milliseconds, Musepack packets), a landing timestamp is rounded to the grid of the codec's frames (learnt from the first two frames); a demuxer that lands after the target is sent further back.

**Sample conventions**

- Every decoder delivers interleaved s32, left-justified.
- u8 → `(v-128)<<24`; s16 → `<<16`; s64 → `>>32`.
- lossless = integer bits > 0.
- double → s32 = round to nearest + clamp.

**Gapless**

- MP3: the encoder delay and padding are trimmed. MP4: trimmed to the edit list's length (own `elst` parser). Ogg: minus `initial_padding`.
- **Engine splice.** The next decoder is opened ahead of time. At EOF it is spliced in if rate, channels and DSD mode match.
  - Splice point = frames pushed + the frames still held by the DSP (convolver, linear EQ, resampler, limiter).
  - The track change is signalled when the frames played reach it.
  - A seek undoes the splice.
  - Gains land sample-exactly on each side.
- The device is reused between tracks with the same format.

**CUE**

- Track paths are `<sheet>.cue#<n>`.
- CD frame → sample frame: `(cd*rate+37)/75`.
- The sheet is UTF-8 or CP1252.
- Missing audio file: try other extensions.
- `REM REPLAYGAIN_*` is read. Pregaps stay with the previous track.

**Seeking**

- Park the worker, seek to `seconds*rate` (clamped, falling back to 0) and reset all DSP.
- Prefill half the ring, at most 400 ms, then unmute.
- Sample-accurate: seek to before the target, then drop samples.

**Position**

`decoded - stretchHeld - (ringQueued*rateRatio + linEqLatency + resamplerDelay*rateRatio) * speed`

**Play from memory** (`playback/playFromMemory`, not done yet)

- mmap with POPULATE and WILLNEED, then mlock.
- At most 2 GB. Files over 64 MB load asynchronously. A CUE image is loaded whole.
- Memory state: 0 none, 1 cached, 2 locked.

**HDCD:** 16-bit stereo only, when codes are found in the first 5 s. Reset per track and per seek.

**ReplayGain**

- Keys: `rgTrackGain/Peak`, `rgAlbumGain/Peak`. NaN = play as is. Album falls back to track.
- `linear = 10^((gain+preamp)/20)`, preamp ±15.
- Prevent clipping: `min(linear, 0.8912509381337456/peak)`.
- `R128_*` tags = Q7.8/256 + 5 dB.
- Scanning: EBU R128 in modes I + TRUE_PEAK + HISTOGRAM. Mono is DUAL_MONO; channels 6 and 7 are Mp090 / Mm090.

**Crossfade:** none; only gapless.

## 4. DSP

**Biquad EQ (SuperEQ)**

- Up to 32 bands and 8 channels; TDF-II biquads in f64.
- Types: PK, LSC, HSC, LPQ, HPQ, BP (0 dB peak), NO.
- **Peak and shelf** bands use Vicanek's matched design, unless RBJ is closer to the analog curve (48 log points from 20 Hz to min(20k, 0.45·fs)). Cuts are inverted boosts.
- **Other types** use RBJ.
- Clamps: freq [1, 0.4999·fs], Q ≥ 1e-4.
- **Graphic mode:** 10 bands at {31.25, 62.5, 125, 250, 500, 1k, 2k, 4k, 8k, 16k}, Q 1.41421356237.
  - Engine gains are solved with 5 Newton steps (inverse overlap matrix, at 48 kHz), each clamped ±36.
  - The interface rounds them to 0.001 dB.
- **On/off** is a 10 ms crossfade; back to flat too (the old sections stay until the fade ends). When the mix reaches 0 the filter state is cleared, so switching on again starts clean.
- Imported and restored values (APO, AutoEQ, profiles, settings) are clamped to the documented ranges; non-finite numbers take the default.
- **Neutral:** disabled, or preamp 0 and no effective band. Peak and shelf bands count only if gain ≠ 0; the other types always count.
- Peak gain: 400 log points from 20 Hz to 20 kHz, at 96 kHz.

**Linear-phase EQ**

- The cascade's magnitude on an FFT grid → IFFT → centred, Tukey window 0.25.
- Taps: 0.5 s, up to 4 s; length `2·ln(1e4)·Q/(π·f0)/0.75`, rounded up to a multiple of 2048.
- Latency = taps/2 + 1024.
- Engaging: primed with history, no gap. Curve change: one-block crossfade. Disengaging: drained, then the held input is handed back.

**Convolver**

- Uniformly partitioned overlap-save, FFT 2048, block 1024, latency 1024, up to 2^20 taps.
- IR channels: 1, 2 or 4 (true stereo).
- An IR at another rate is resampled (high quality), scaled by `irRate/trackRate`.
- Not on DSD. Tail = 1024 + taps.

**Crossfeed** (bs2b)

| Preset | Cutoff | Feed |
|---|---|---|
| Default | 700 Hz | 4.5 dB |
| ChuMoy | 700 Hz | 6 dB |
| JanMeier | 650 Hz | 9.5 dB |

Stereo only.

**Limiter**

- True peak with 4× oversampling, 12 taps per phase, Blackman sinc. 1 phase at ≥ 352.8 kHz.
- Look-ahead 1.5 ms (sliding minimum plus box average), release 50 ms. Below 352.8 kHz the samples wait 6 more frames (half the interpolator), so an inter-sample peak is seen before its frames leave: isolated overs stay under the ceiling (they passed it by up to 0.16 dB at 44.1 kHz).
- One gain for all channels; exactly 1.0 below the ceiling.
- Ceiling [-6, 0] dBTP, default -1.

**SoundTouch**

| Setting | Default | Range / rule |
|---|---|---|
| pitch (semitones, f64) | — | ±48 |
| tempo, rate | — | 0.05–20 |
| AA filter | on | — |
| AA filter length | 64 | 8–128, `(t+4)/8*8` |
| quickseek | off | — |
| sequence ms | 0 (auto) | 0–500 |
| seek window ms | 0 (auto) | 0–250 |
| overlap ms | 8 | 1–100 |

- Sample rate capped at 192000. Enabled defaults to off.
- Neutral is an exact `==`. Speed = tempo × rate.
- Built lazily, freed when neutral.

**Resampler** (`crates/dsp/src/resampler.rs`, `polyphase.rs`)

- Filters (`playback/resampleFilter`): 0 linear phase, 1 intermediate phase, 2 minimum phase, 3 linear steep.
- Linear and linear steep: rubato's FFT resampler, a Blackman-Harris² windowed sinc of ≥ 400 / ≥ 2048 taps at the lower rate (flat to 0.945–0.965 / 0.985 of its Nyquist, 184–215 / 192–195 dB down from 1.005).
- Minimum and intermediate: a polyphase FIR of its own (rubato has linear phase only).
  - Prototype: Kaiser-windowed sinc, 640 taps at the lower rate, β 19.98, cut-off 0.98 of Nyquist (transition 0.96–1.0, ~190 dB), designed at 8 points per lower-rate sample. When downsampling the kernel is stretched by in/out.
  - Minimum phase by the cepstral (homomorphic) method on a 2¹⁸-point FFT (32× the prototype): log of the magnitude (floored at −240 dB, for the stop band's nulls) → real cepstrum → folded onto positive quefrencies (c₀, 2c₁ … 2c_{M/2−1}, c_{M/2}, 0 …) → `ln|H| + jφ_min` → `|H|·e^{jφ_min}`.
  - Intermediate: soxr's (phase response 25 of 0–100), the phase halfway between minimum and linear: `φ = ½·φ_min − ½·ω·D`, D the prototype's centre; same magnitude.
  - The kernel is the span from −0.5 to +1.5 prototype lengths around time 0 (the nulls alias the cepstrum: a ~10⁻⁵ copy of the response half the FFT away, left out), trimmed below 10⁻¹¹ of the peak: 622 taps (minimum), 1074 (intermediate). Both are designed once per run (~5 ms each).
  - Each output phase (b of them, for the ratio a/b reduced) gets a row of coefficients read from the design points by 16-point Lagrange interpolation; rows sum to 1. Above 4M coefficients (rates of no common family, e.g. 44100 → 47999) 512 rows, four blended per output (cubic). f64 throughout; AVX2+FMA dot products when the CPU has them.
  - Upsampling by more than 16× (PCM→DSD): the FIR to 2× the input rate, then the linear filter (its transition is at the input rate, where the FIR left nothing to ring).
  - Measured (`tests/resampler_phase.rs`): ripple < 2·10⁻⁷ dB to 0.9, flat (0.1 dB) to 0.965, stop band 191–195 dB, within 2·10⁻⁷ dB of the linear filter up to 0.9. A click's energy more than 8 lower-rate samples before its peak: linear 4–6·10⁻³ (−23 dB), intermediate 7–10·10⁻⁶ (−51 dB), minimum 0 (3·10⁻¹⁹ through two stages).
  - Speed, one core, stereo: 44.1 → 48 kHz minimum 154× real time; 44.1 → 192 kHz minimum 37×, intermediate 23× (linear 567×); → 705.6 kHz 11×; DSD64's rate 29× (linear 36×); DSD512's 3×.
- Timing, every filter (soxr's rules): the output starts at the input's first frame, `drain` ends on exactly `round(in · out / in)` frames, and `delay_frames()` (soxr_delay) is the output still owed. Linear filters are aligned on their centre; minimum and intermediate on the kernel's peak (a click peaks on its own output frame), so they look ahead only 9 input frames (minimum) or ~440 (intermediate), × in/out when downsampling.
- Used when:
  - the device refuses the rate (or a shared output runs at its mixer's rate);
  - upsampling is on: ×2; ×4 then ×2; Max tries ×16, ×8, ×4, ×2; ≤ 768 kHz, and only if the device opens at exactly that rate;
  - a fixed output rate is set;
  - PCM→DSD: straight to the 1-bit rate (2.8224 MHz for DSD64), with the chosen filter.

**DSD→PCM**

- Stage 1: Kaiser FIR, 160 dB, using byte lookup tables.
- Stage 1 lands at 352.8 k (44.1 kHz family) or 384 k (48 kHz family), for every rate from DSD64 to DSD1024.
- Stage 2: resample to 88.2/96 k (DSD64) or 176.4/192 k (DSD128 and above).
- Stage-2 bands: DSD64 30 k; DSD128 50 k, stop at 70 k; DSD256/512 flat to 70 k.
- All-ones input = PCM full scale.

**PCM→DSD** (`playback/pcmToDsd`: 0 off, 1 DSD64, 2 DSD128, 3 DSD256, 4 the highest the DAC takes up to DSD256, 5 DSD512)

- Rates tried, best first: DSD64 → 64; DSD128 → 128, 64; DSD256 and Max → 256, 128, 64; DSD512 → 512, 256, 128, 64 (× 44.1 kHz).
- The whole processed chain runs (headroom, EQ, convolver, crossfeed, volume × ReplayGain), then the resampler converts to the 1-bit rate and the limiter (if on) runs there; never bypassed, never bit-perfect.
- On the worker, in place of the quantizer: 8th order, error feedback, four NTF tables (numerator and denominator, 9 coefficients each); packed like DSD files (native DSD_U32 words, or DoP words whose markers the device thread stamps).
- Input × 0.45 (PCM full scale = 45 % modulation, -6.9 dB), limit 0.5, reset when |state| > 8; the resets are reported (the signal path's "Modulator" row).
- The DSD rules hold: 60 ms pre-roll of the idle pattern at load and seek, silence and pauses as the idle pattern, DoP markers alternating across everything. At the end the last partial word is completed with the idle pattern.
- Gapless: carries on into the next PCM track at the same rate and channels (the chain and the modulator go on); never into a DSD file.
- The visualizer gets nothing while modulating (the device's samples are DSD words).

**Quantizer**

- TPDF from xorshift64*, seed `0x9E3779B97F4A7C15`.
- Noise shaping: SoX F-weighted 9 taps `{2.412, -3.370, 3.937, -4.174, 3.353, -2.205, 1.281, -0.569, 0.0847}`, error ±1.5. Only at ≤ 20 bits and 44.1/48 kHz; otherwise TPDF.
- After 2048 zeros per channel the output is exact 0.
- Clipped samples are counted.

**Headroom**

- Manual: -24..0 dB.
- Auto: `-(3 + max(0, eqPeak))`, rounded to 0.1 dB.
- Changing it resets the clip counter.

**Test signals** (cached in `cache/test-signals`; not done yet: the generators are in `crates/dsp`, playing them is not wired)

- tone: 1 kHz, -20 dBFS, 10 s, 24-bit.
- DSF at 64/128/256, 5 s.
- channels: one channel at a time.
- DTS-in-WAV.
- All have 10 ms fades.

## 5. Controls (what the interface uses)

### Playback

**Settings it can change**

- SoundTouch: pitch (semitones), tempo, rate, AA filter, AA filter length, quickseek, sequence, seek window, overlap, enabled.
- Volume and dither: volume, volume mode (0 Digital, 1 Hardware, 2 Auto), dither mode, headroom dB, auto headroom.
- DSP: crossfeed (0–3), limiter, limiter ceiling, upsampling (0–3), convolution enabled, resample filter (0–3).
- ReplayGain: mode (0 Off, 1 Track, 2 Album), preamp, prevent clipping.
- Output: exclusive mode, follow track rate, DSD mode (0 PCM, 1 Native, 2 DoP), PCM→DSD, play from memory, HDCD.
- Playback: shuffle, repeat (0 Off, 1 All, 2 One), resume session, buffer ms, resync delay ms, fixed rate, release when idle, sound settings per output.

**State it reads**

- SoundTouch active, speed.
- Volume: hardware volume, system volume, system volume in hardware, volume fixed, hardware volume supported, hardware volume info.
- Effective headroom, clipped samples, convolution file, current gain dB (NaN when unknown).
- Output:
  - output devices: {key, backend, name, detail, is default, direct};
  - current device key, output description, bit-perfect, output summary;
  - signal path: [{stage, detail, quality: lossless|high|enhanced|low|warning, action?}];
  - output reserved, output exclusive-capable.
- State: playing, loaded, position, duration, current track, current path, track description, queue, current index, last error.
- Other: audio components, test signal playing, DTS test available, visualizer active, spectrum.

**Operations**

- Load / clear a convolution IR.
- Volume:
  - slider position ↔ volume: linear in dB over 60 dB; position ≥ 0.999 gives exactly 1; steps of 0.1 dB;
  - step volume by dB; gain text.
- Set rate mode (fixed Hz, upsampling); watch the spectrum.
- Exact-decimal helpers: parse, format, step, pitch ratio, time format.
- Reset SoundTouch.
- Devices: refresh, select, release the output.
- Test signals: play (kind, rate, channels), stop (not done yet).
- Queue: play paths (paths, start), enqueue, play index, clear, remove, forget folder, move, sort (field, descending), reverse.
- Transport: load, play, pause, pause from the desktop, toggle play/pause, stop, next, previous, seek.

**Notifications**

- A change notification for each group of the state above.
- Also: track finished, error, output busy (holder), seeked, output taken by another program, output devices hot-plugged (added, removed).

**Queue rules**

- Shuffle is a permutation with the current track first.
- Repeat One returns the same track. At the end, Off returns -1 and All wraps.
- An unreadable file is skipped.
- Previous: over 3 s restarts the track; otherwise it goes back, wrapping only under All.
- Pause from the desktop is ignored if a sink or the card was lost in the last 3 s.

**Volume modes**

- Hardware on `alsa|` uses the ALSA mixer.
- Non-Digital on PipeWire or `""` uses PipeWire's volume: of the sink Sonkore's stream (`node.name=sonkore`) is linked to, e.g. an Easy Effects filter's sink when it takes the streams over; else the chosen sink when it is there; else the desktop's default. Watched through the registry's links: a new link from the stream to another sink switches to it; a stream or link going away changes nothing by itself (between tracks the stream is made again and links back).
- On WASAPI exclusive, the endpoint's own volume.
- Fixed = Auto on a direct output, or Hardware with no control.
- The engine is held at 1.0 while an external volume is in use. Engine volume range: 0–4.

**Spectrum**

- 2048-point FFT, Hann window, on a mono tap delayed by the output latency (ring of 65536).
- 24 log bands from 40 Hz to min(16k, Nyquist), each taking its peak bin.
- -66..0 dBFS → 0..1. Updated every 33 ms.

### Equalizer

**State**

- enabled, linear phase, mode (graphic | parametric), preamp.
- bands: [{type, freq, gain, q, enabled}].
- response: 192 points {f, db}, 20 Hz–20 kHz, evaluated at 48 kHz.
- peak gain, max bands = 32, filter types, built-in profiles (13), user profiles, current profile.
- AutoEQ results, busy flag, status.

**Operations**

- Bands: set gain, graphic frequency, set band, add band (switches to parametric; 1 kHz, Q 1), remove band, reset.
- Auto preamp: `-ceil(peak·10)/10`.
- Type and frequency labels.
- Profiles: load, save, delete, import, export.
- AutoEQ: search, refresh the index, apply.

**Clamps:** preamp -30..12; gain ±24; freq 10–24000; Q 0.05–30. Switching to graphic fits the 10 centres, rounded to 0.1 dB.

## 6. Platform

- **MPRIS** (Linux; the system media controls on Windows)
  - Name `org.mpris.MediaPlayer2.sonkore`, or `.instance<pid>` when taken. Desktop entry `com.axiokyn.Sonkore`.
  - LoopStatus None / Playlist / Track. Rate is 1. Track id is `/org/sonkore/track/<n>`. Positions in µs.
  - Also forwards files and commands to a running instance.
- **ALSA mixer**
  - Control order: PCM, Master, Digital, Headphone, Speaker, Front, DAC.
  - dB relative to the top, capped at 0 dB; 0 mutes via the switch.
  - Follows external changes.
- **PipeWire volume:** watches the sink's Props. "Hardware" means a device sink whose softVolumes are all 1.
- **RealtimeKit** (`crates/platform/src/rtkit.rs`, installed by the app as the engine's real-time hook, see §1 Device thread): `org.freedesktop.RealtimeKit1` on the system bus, or `org.freedesktop.portal.Realtime` in Flatpak. Reads `MaxRealtimePriority` and `RTTimeUSecMax`, sets RLIMIT_RTTIME to the latter (the hard limit only comes down), then `MakeThreadRealtimeWithPID` at the highest priority allowed.
- **Document portal:** maps `/run/user/*/doc/<id>/` paths to the host.

## 7. Subtleties

- **Exact parameters.** f64 end to end, compared with `==`.
  - Parsing: locale-independent ("." decimal point), correctly rounded.
  - Stepping: round-trips through fixed-point text with the field's decimals, and -0 becomes 0.
  - Formatting: strips "-0".
  - pitch, tempo and rate are saved as 17-significant-digit text.
- **Settings keys** (`sonkore.conf`)
  - **`playback/`:**
    - device, exclusive, followTrackRate;
    - ditherMode (the legacy key `dither` was a bool and is still read as the default), headroomDb, headroomAuto;
    - crossfeed, limiter, limiterCeilingDb, upsampling, convolutionFile, convolution, resampleFilter;
    - replayGain, replayGainPreamp, replayGainPreventClipping;
    - shuffle, repeat, volume, volumeMode (2);
    - dsdOutput (1), pcmToDsd (0–5), playFromMemory (true), hdcd (false);
    - resume, releaseDacWhenIdle, soundPerOutput (true), bufferMs (150), resyncDelayMs (250), fixedRate.
  - **`pitch/`:** pitch, tempo, rate, enabled. Legacy copies lived in `soundtouch/`.
  - **`soundtouch/`:** aaFilter, aaFilterLength, quickSeek, sequenceMs, seekWindowMs, overlapMs.
  - **`soundProfiles/<deviceKey with / and \ as |, "" = default>`** (with soundPerOutput on, each output keeps its own sound: stored when the output is left, when the switch is turned on and at quit; put back when it is chosen; an output without one keeps the sound in use): compact JSON of ditherMode, headroomDb, headroomAuto, crossfeed, upsampling, convolutionFile, limiter, limiterCeilingDb, convolution, resampleFilter, dsdOutput, pcmToDsd, equalizer.
  - **`equalizer/`:** enabled (false), linearPhase, mode, preamp, profile ("Flat"), bands (JSON).
  - The legacy groups `rubberband` and `pitch/engine` are removed.
- **Data files**
  - `session.json`: {queue, index, position}, written atomically and only when it changed.
  - `eq-profiles.json`: [{name, mode, preamp, bands}], indented.
  - `autoeq-index.md`: refreshed after 14 days. Profiles come from `raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/<path>/<leaf>%20ParametricEQ.txt`. At most 150 results.
- **Equalizer APO format**
  - Aliases: PEQ→PK; LS/LSQ→LSC; HS/HSQ→HSC; LP→LPQ; HP→HPQ.
  - Default Q is 0.7071.
  - A shelf given by slope (S = 0.9 when bare) uses `Q = 1/sqrt((A+1/A)(1/S-1)+2)`.
  - Export: `Preamp: %.1f dB` / `Filter n: ON|OFF TYPE Fc %g Hz Gain %.1f dB Q %.2f`.
- **Consistency rules:**
  - `pcmToDsd` is 0..5 both when set and when loaded, so DSD512 survives a restart.
  - HDCD defaults to off everywhere.
- **Constants:**
  - decode block 1024; SoundTouch chunk 4096;
  - DSD pre-roll 60 ms; fade 8 ms;
  - analysis ring 65536; prefill half the ring within 400 ms;
  - RG ceiling 0.8912509381337456; auto-headroom margin 3 dB;
  - volume slider 60 dB; 8 channels; upsampling ≤ 768 kHz.
- **User-facing text:** the output summary, the output description and the signal path. Their strings and quality tags depend on the whole output status: graph rate, system volume neutral, modulator resets, upsampled vs resampled.
- **Audio components** (About): names and versions joined with U+00A0.
