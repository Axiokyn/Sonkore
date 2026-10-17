# Roadmap

Goal: a hi-fi player that matches Roon and JRiver in everything the most demanding listeners ask for, built only with free software and free to publish everywhere.

## Rules

1. Sonkore is GPL-3.0-or-later. Only compatible dependencies are added: MIT, BSD, Apache-2.0, LGPL or GPLv3.
2. No proprietary SDKs, except the system APIs (ALSA, PipeWire, WASAPI). ASIO has been GPLv3 since October 2025.
3. Every published binary includes its licenses and access to the source code (see `THIRD-PARTY.md` and `packaging/appimage/`).

## Phase 1: purist core

- [x] **Gapless**: the next track is decoded and spliced without silence, sample-exact; encoder padding is trimmed (MP3 LAME/Xing, AAC/MP4, Opus pre-skip). The end of the last track is no longer cut off.
- [x] **ReplayGain / EBU R128** (libebur128, MIT): ReplayGain and R128 tags, per-track and per-album analysis without modifying the files, track/album/off modes, preamp and true-peak clipping prevention.
- [x] **Hardware volume** on Linux: the DAC's mixer control (ALSA, in dB) to keep bit-perfect playback when turning the volume down; with PipeWire, the sink volume (the same control, through the sound server).
- [x] **Hardware volume** on Windows: IAudioEndpointVolume with WASAPI exclusive, on devices that implement it in hardware.
- [x] **Native DSD and DoP** on Linux: DSF and DSDIFF without going through PCM (ALSA DSD_U32_BE/LE, 24-bit DoP), up to the DAC's highest rate.
- [x] **Native DSD** on Windows: ASIO output (ASIO 2.2 DSD mode), plus bit-perfect PCM in the driver's own format.
- [x] **DST** (compressed DSD in DSDIFF, the SACD format): an in-house decoder (ISO/IEC 14496-3 subpart 10) that returns the exact bits, so it plays as native DSD or DoP.
- [x] **In-house DSD → PCM** in double precision (160 dB FIR + SoX VHQ): 88.2 / 176.4 kHz, without the ultrasonic noise FFmpeg leaves (~−120 vs. ~−28 dBFS).
- [x] **Signal path**: every stage (file → decoder → DSP → device) with format, bits, rate and quality.
- [x] **Playback from RAM**: the whole file in RAM (locked if the system allows it) before playing; the next track loads in the background.
- [x] **Real-time priority** for the audio thread (rtkit).
- [x] **CUE sheets and multichannel** (5.1 / 7.1 without downmixing to stereo, with exact channel reordering).
- [x] **HDCD**: decoding to 20 bits (libhdcd), only on tracks with HDCD codes.

## Phase 2: reference DSP

- [x] **FIR convolution** (pocketfft, BSD-3, double precision): room correction and filters from REW, Dirac, etc.; mono, stereo and true stereo.
- [x] **Crossfeed** for headphones (Bauer / bs2b design, MIT).
- [x] **Resampling with a choice of filters** (linear phase, intermediate, minimum or steep linear, SoX VHQ) and **upsampling** ×2 / ×4 / the DAC's maximum.
- [x] **Noise-shaped dither** (9-coefficient F-weighted).
- [x] **PCM → DSD upsampling** (DSD64–DSD512) with an 8th-order sigma-delta modulator (~113 dB SNR at DSD64), one thread per channel.
- [x] **Headroom management** before the DSP, with a clipping detector.
- [x] Optional **true-peak limiter** at the end of processing (BS.1770, 4×; −1 dBTP by default; bit-exact below the ceiling).
- [x] **Sound profiles per output**: EQ, convolution, crossfeed, headroom, dither, oversampling and DSD remembered per device.
- [x] **Now playing page**: large cover, signal path and queue in one view.
- [x] **Linear-phase equalizer**: the EQ curve as an FIR (FFT convolution), switchable during playback without dropouts.
- [x] **Native PipeWire output** with per-track rate switching: bit-perfect (up to 24 bits) without taking the DAC exclusively.
- [x] **Test signals** from the app: DAC sample rate (PCM and DSD), channels and bit-perfect (DTS).

## Quality

- [x] Engine tests (`tests/engine_test.cpp`): bit-perfect, exact gapless, DSP, DSD, ASIO, stress; clean under ASan, UBSan and TSan. They run on every push (CI).

## Phase 3: library

- [x] Covers from the Cover Art Archive for albums without a picture (MusicBrainz lookup by artist and album, off by default).
- [ ] Metadata, credits and releases from MusicBrainz (CC0).
- [ ] Biographies from Wikipedia/Wikidata (CC BY-SA, citing the source).
- [ ] Scrobbling (ListenBrainz, Last.fm).
- [x] Artist and genre views, Genre → Artist → Album panes and layouts in the style of Spotify, Roon, MusicBee, JRiver, foobar2000, Winamp and Audacious.
- [x] Configurable scanning: skipped folders and short files left out.
- [ ] Smart playlists, history, ratings and filters.

## Phase 4: network and multiroom

- [ ] Remote control from the browser and the phone.
- [ ] UPnP/DLNA to send to streamers and network DACs (libupnp, BSD).
- [ ] Chromecast.
- [ ] Synchronized multiroom (Snapcast, GPLv3).

## Phase 5: platforms and distribution

- [x] Linux: AppImage on GitHub Releases (built on AlmaLinux 9 in CI: runs on any distribution with glibc 2.34+).
- [ ] Linux: AUR, COPR, openSUSE Build Service, Snap.
- [x] Windows x64: ASIO + WASAPI exclusive; portable zip on GitHub Releases (Qt, the MSVC runtime and FFmpeg included).
- [ ] Windows: installer, Scoop, winget.

## Out of scope

- **MQA**: proprietary, with no open implementation.
- **Roon Ready / RAAT**: closed protocol (alternative: UPnP + Snapcast).
- **Built-in Tidal / Qobuz**: they require a commercial agreement with each service.
