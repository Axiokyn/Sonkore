# Building Sonkore

[← Back](../README.md)

Sonkore is written in Rust (stable toolchain, edition 2024) with a
[Slint](https://slint.dev) interface. Install Rust with
[rustup](https://rustup.rs).

## Linux

```sh
# Debian / Ubuntu
sudo apt install pkg-config libasound2-dev libxkbcommon-dev libwayland-dev libfontconfig-dev
# Fedora
sudo dnf install pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel wayland-devel fontconfig-devel

cargo build --release -p sonkore
target/release/sonkore
```

PipeWire is not needed to build: `libpipewire-0.3` is loaded at run time when
it is there. Neither is FFmpeg: its libraries (FFmpeg 5 to 8) are loaded at
run time to play Opus, APE, WavPack, TTA, TAK, Musepack, WMA, AC-3 and DTS
(the `ffmpeg` package of any distribution brings them; on
Windows, FFmpeg's shared build's `avformat-*.dll`, `avcodec-*.dll` and
`avutil-*.dll` next to `sonkore.exe` or on the PATH).

For day-to-day work, `cargo build -p sonkore` (the dev profile is optimized
enough to play music) and `--profile diario` (optimized, without the release
profile's fat LTO) build in seconds; `--release` takes a few minutes.

## Windows

With the MSVC toolchain (Visual Studio Build Tools, "Desktop development with
C++"):

```powershell
cargo build --release -p sonkore
target\release\sonkore.exe
```

The C runtime is linked statically (`.cargo/config.toml`): the `.exe` runs on
a clean Windows 10/11 with nothing else installed.

From Linux, `tools/check-windows.sh` type-checks the whole workspace for
Windows (it downloads the Windows standard library once).

## Tests

```sh
tests/make_corpus.sh "$PWD/target/test-corpus"   # needs ffmpeg and flac
SONKORE_TEST_CORPUS="$PWD/target/test-corpus" \
ALSA_CONFIG_PATH="$PWD/tests/pipewire/alsa-null.conf" \
cargo test --workspace
```

Without the corpus, the tests that need audio files are skipped.
`ALSA_CONFIG_PATH` points ALSA's default device at a null sink, so nothing
reaches your speakers. Optional extras:

- libhdcd's test files (the `test/` folder of
  [libhdcd](https://github.com/bp0/libhdcd)) in `target/test-corpus/libhdcd/`
  check the HDCD decoder against the md5 sums libhdcd expects.
- `crates/dsd/tests/reference/make_reference.sh DIR`, then
  `SONKORE_DSD_CORPUS=DIR`, compares DSD and DST decoding with FFmpeg.
- The FFmpeg tests (`crates/ffmpeg`, and the engine's and library's
  `tests/ffmpeg*.rs`) use the corpus's `ffmpeg/` folder and the system's
  FFmpeg libraries, and are skipped without them. With FFmpeg's headers
  installed (`libavformat-dev`…) they also check the struct offsets Sonkore
  reads against them; `tools/ffmpeg-offsets.sh` prints those offsets for
  every supported release (run it when a new FFmpeg major comes out).
- `tests/pipewire/run.sh` runs a small PipeWire daemon of its own with a null
  sink, to test the PipeWire output without touching the desktop's sound.

`tools/check.sh` runs everything before a release: build, tests, untranslated
texts, the interface's views drawn offscreen against a baseline
(`tools/ui-tour.sh`) and the real window driven in a headless Wayland
compositor (`tools/ui-live.py`).

## AppImage (Linux, any distribution)

```sh
packaging/appimage.sh [--rapido] [runtime]
# -> dist/Sonkore-x86_64.AppImage (+ .sha256)
```

`runtime` is an AppImage type 2 runtime (from
[type2-runtime](https://github.com/AppImage/type2-runtime/releases)) or any
type 2 AppImage to take it from; the squashfs is written by `tools/mkappimage`,
no appimagetool needed. The binary depends on the glibc it was built with: the
published AppImage is built on AlmaLinux 9 (glibc 2.34).

## Published packages

`.github/workflows/build.yml` builds and tests on Linux and Windows, checks
the AppImage on six distributions and, on a `v*` tag or a commit marked
`[release]`, publishes `Sonkore-x86_64.AppImage` and `Sonkore-windows-x64.zip`
as a GitHub release named after the version in `Cargo.toml`. A published
version is never replaced: raise the version first.

## Translations

The interface texts are translated in `i18n/<lang>.po` (gettext; Spanish,
Brazilian Portuguese, French, German, Italian), with Slint's `{0}`
placeholders; `crates/ui/build.rs` bundles them into the app. After changing interface text, add the
translations to `i18n/translations_*.py` and run `python3 i18n/update.py`;
`tools/textos-sin-traducir.py` lists what is missing.

## Registering Sonkore with the desktop (Linux)

```sh
packaging/linux/install-user.sh [path/to/sonkore]   # default: target/release/sonkore
```

adds the menu entry and icons for your user, so the dock shows the right icon
and the media controls find the app.

## Code layout

| Crate | What it holds |
|---|---|
| `crates/core` | Settings (`sonkore.conf`), paths, shared types |
| `crates/engine` | Decoding (Symphonia), the player, gapless, outputs (ALSA, PipeWire, WASAPI) |
| `crates/dsp` | Convolution, crossfeed, limiter, resampling, test signals |
| `crates/eq` | Equalizer (graphic, parametric, linear phase), AutoEQ |
| `crates/soundtouch` | SoundTouch ported to Rust: pitch, tempo, rate |
| `crates/dsd` | DSF/DFF and DST decoding, DSD → PCM, the sigma-delta modulator |
| `crates/hdcd` | HDCD decoding (a port of libhdcd) and CUE sheets |
| `crates/library` | Scanning and watching folders, tags, covers, loudness, playlists |
| `crates/platform` | MPRIS, tray, single instance, ALSA mixer, device reservation (Linux) and their Windows counterparts |
| `crates/app` | The interface (`ui/*.slint`, `icons/`) and what ties it to the engine |
| `vendor/` | Slint, parley and softbuffer crates with small changes (marked "Sonkore:") |
| `tools/` | Checks, screenshots, the headless compositor, the AppImage writer |
