# Building Sonkore

[← Back](../README.md)

Requirements: CMake ≥ 3.21, a C++20 compiler and Qt ≥ 6.5 (Core, Gui, Widgets, Qml, Quick, QuickControls2, Network, Svg; DBus on Linux for MPRIS; alsa-lib on Linux for hardware volume, optional). SoundTouch, libsoxr and libebur128 are downloaded and built automatically.

The extra formats need the FFmpeg ≥ 5.1 headers (`libavformat`, `libavcodec`, `libavutil`, through pkg-config; on Fedora `ffmpeg-free-devel` or `ffmpeg-devel`, on Debian/Ubuntu `libavformat-dev libavcodec-dev`). Only the headers are used: the libraries are loaded at run time. With `-DSONKORE_WITH_FFMPEG=AUTO` (the default) it is enabled when they are found; `ON` requires it and `OFF` leaves it out. `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` or `-DSONKORE_USE_SYSTEM_SOXR=ON` use the system's copies.

```bash
# Linux
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=/path/to/Qt/6.x/<kit>
cmake --build build
./build/bin/Sonkore
cmake --install build --prefix dist      # deploys the Qt runtime

# Windows (Developer PowerShell for VS 2022)
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=C:/Qt/6.x/msvc2022_64 `
      -DSONKORE_FFMPEG_INCLUDE_DIR=C:/path/to/ffmpeg/include      # headers only; the DLLs go next to the .exe
cmake --build build
packaging/windows/package.ps1 -Build build -FFmpeg C:/path/to/ffmpeg   # -> dist/Sonkore-windows-x64.zip
```

`packaging/windows/build-ffmpeg.sh` builds (on Linux, with MinGW-w64) the same audio-only FFmpeg that the AppImage carries. ASIO drivers need no Steinberg SDK: the interface is declared in `src/audio/AsioAbi.hpp`.

## Tests

```bash
cmake -S . -B build -DSONKORE_BUILD_TESTS=ON ...
cmake --build build --target engine_test
tests/make_corpus.sh corpus            # needs sox, flac, ffmpeg and python3 with numpy
build/engine_test corpus               # or: build/engine_test corpus gaplessAlbum seekAndPause
```

They play real files through the engine (on miniaudio's null device, no sound card needed) and compare every sample that reaches the "DAC": bit-perfect in 16 formats, exact gapless joins (album and CUE image, also with EQ), seeking, pause, volume, EQ (linear-phase too: design, exact alignment, switching during playback, gapless), convolution, crossfeed, oversampling with every filter, pitch and tempo, DSD (DoP; to PCM with the in-house converter: filter response, level, in-band SNR, ultrasonic noise, alignment with the source and seeking; the PCM→DSD modulator up to DSD512; DST decoded bit for bit against an in-house DST encoder that FFmpeg validates), test signals (tone, channels, DSD, DTS), ASIO sample conversion, damaged files and a stress test with random actions. The `pipewire` test (run with `tests/pipewire-session.sh`, which starts a private PipeWire with a virtual DAC) plays through PipeWire and records what the DAC receives. They pass clean under AddressSanitizer, UBSan and ThreadSanitizer.

## AppImage (Linux, any distribution)

```bash
packaging/appimage/build.sh [path/to/Qt/6.x/gcc_64]   # needs podman
# -> dist/Sonkore-x86_64.AppImage (+ .sha256)
```

It is built inside an AlmaLinux 9 container (glibc 2.34, the same base as Qt's binaries), so it works on any x86_64 distribution since 2021: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE Tumbleweed, etc. It includes Qt (with the X11 and Wayland plugins and the portal file dialog), the QML modules and an audio-only FFmpeg (LGPL), so ALAC, AAC, Opus, DSD and the rest work without installing anything. Graphics drivers and audio (ALSA, PipeWire, PulseAudio, JACK) come from the system. The licenses and the details of what is bundled are in `usr/share/doc/sonkore/` inside the AppImage.

## Published packages

The packages on [Releases](https://github.com/Axiokyn/Sonkore/releases) are built from this code by the `.github/workflows/build.yml` workflow on GitHub Actions: the AppImage inside the container above and the Windows zip with MSVC against Qt for MSVC. Before publishing, it runs the engine tests and tries the AppImage on clean installs of Ubuntu 22.04 and 24.04, Debian 12, Fedora, Arch and openSUSE Tumbleweed.

To publish a version, raise `VERSION` in `CMakeLists.txt`, add it to `packaging/linux/com.axiokyn.Sonkore.metainfo.xml` and push a commit whose message contains `[release]`. A version that is already published is never replaced.

## Offline builds (Flatpak, distribution packages)

CMake downloads seven SHA256-pinned tarballs from `cmake/Dependencies.cmake` (miniaudio, dr_libs, libebur128, libhdcd, pocketfft, SoundTouch, libsoxr). Without network, extract each one and give its folder instead:

```bash
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/app \
      -DFETCHCONTENT_FULLY_DISCONNECTED=ON -DSONKORE_DEPLOY_QT=OFF \
      -DFETCHCONTENT_SOURCE_DIR_MINIAUDIO=/path/to/miniaudio \
      -DFETCHCONTENT_SOURCE_DIR_DR_LIBS=/path/to/dr_libs \
      -DFETCHCONTENT_SOURCE_DIR_LIBEBUR128=/path/to/libebur128 \
      -DFETCHCONTENT_SOURCE_DIR_LIBHDCD=/path/to/libhdcd \
      -DFETCHCONTENT_SOURCE_DIR_POCKETFFT=/path/to/pocketfft \
      -DFETCHCONTENT_SOURCE_DIR_SOUNDTOUCH=/path/to/soundtouch \
      -DFETCHCONTENT_SOURCE_DIR_SOXR=/path/to/soxr
```

`-DSONKORE_DEPLOY_QT=OFF` leaves the Qt runtime out of the install: the system's (or the Flatpak runtime's) is used. `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` and `-DSONKORE_USE_SYSTEM_SOXR=ON` replace the last two. `cmake --install` puts the program, the desktop entry, the metainfo and the icons (all named after the app ID, `com.axiokyn.Sonkore`) and the licenses (`share/licenses/com.axiokyn.Sonkore`) under the prefix.

What Sonkore uses at run time on Linux, for a sandbox or a package:

- **Audio:** PipeWire's socket (`$XDG_RUNTIME_DIR/pipewire-0`, native output and volume), PulseAudio and JACK as fallbacks, and `/dev/snd` for direct ALSA `hw:` output, native DSD and the DAC's mixer.
- **Session bus:** it owns `org.mpris.MediaPlayer2.sonkore` (MPRIS; a second launch also hands its files and the desktop entry's quick actions to the running instance through it) and, while it holds a card for `hw:` output, `org.freedesktop.ReserveDevice1.Audio<card>`, asking the owner of that name to release the card.
- **Real-time priority:** rtkit on the system bus, or the Realtime portal (`org.freedesktop.portal.Realtime`) inside a Flatpak.
- **Files:** the music folders the user adds (read only; nothing is written next to the music) and the file chooser for playlists and EQ profiles; its own data under `~/.config/sonkore`, `~/.local/share/sonkore` and `~/.cache/sonkore`.
- **Network:** only when the user turns on missing covers (MusicBrainz and the Cover Art Archive) and for AutoEQ's headphone list.
- **FFmpeg:** `libavformat`, `libavcodec` and `libavutil` with the same major version as the headers it was built with, opened only when a file needs them.

## Registering Sonkore with the desktop (Linux)

```bash
packaging/linux/install-user.sh          # menu entry + icons in ~/.local/share
```

On Wayland the dock matches the window to `com.axiokyn.Sonkore.desktop` (the app ID): without this step the panel shows a generic icon.

## Code layout

```
Sonkore/
├── CMakeLists.txt / cmake/Dependencies.cmake   # dependencies as SHA256-verified tarballs
├── src/
│   ├── main.cpp
│   ├── audio/      # Qt-free engine: Decoder (FLAC/WAV/MP3 with dr_libs), FfmpegDecoder (the rest), DsdDecoder,
│   │               # DstDecoder (DST → DSD), SigmaDelta (PCM → DSD), TestSignals,
│   │               # AlsaDsdOutput (native DSD on Linux), AsioOutput (ASIO on Windows),
│   │               # DSPManager, TimeStretch (SoundTouch), Resampler (soxr),
│   │               # SuperEQ + Biquad, SampleConvert, SpscRingBuffer, backend_impl.c
│   ├── library/    # Metadata (tag parsers), LibraryModel, TrackFilterModel, TrackListModel,
│   │               # AlbumModel, FolderTreeModel, PlaylistManager, CoverImageProvider, OnlineArt
│   ├── ui/         # AppController (scale, renderer, safe start, restart), DspController (transport/queue/output/pitch and tempo), EqController (EQ/AutoEQ)
│   └── platform/   # MprisService, AlsaMixer, RealtimeKit (Linux); WinTaskbar, WinEndpointVolume (Windows)
├── docs/           # these pages and the README screenshots
├── tests/          # engine_test (bit-perfect, gapless, DSP, DSD, ASIO, stress) and make_corpus.sh (the audio it plays)
├── .github/        # workflows/build.yml: tests, AppImage, Windows package and releases
├── fonts/          # interface fonts (SIL Open Font License, license in each <font>/OFL.txt): Inter
│                   # (the default: Latin, Greek and Cyrillic, hinted), Manrope,
│                   # Plus Jakarta Sans, Figtree, Outfit, DM Sans, Nunito Sans, Rubik; weights 400-700
├── i18n/           # translations (Spanish, Portuguese, French, German, Italian)
├── packaging/      # icons/ (master SVGs + generate.py), fonts/ (build-inter.py: Inter for the app), linux/ (.desktop, metainfo, screenshots, install-user.sh), appimage/ (build.sh),
│                   # ffmpeg/ (shared configuration), windows/ (.rc, .ico, build-ffmpeg.sh, package.ps1)
└── qml/
    ├── Main.qml, Theme.qml (themes), TrackColumns.qml (columns), Prefs.qml, AppActions.qml (event bus and drag and drop)
    ├── EditSoundtouch.qml, PrecisionField.qml
    ├── components/ # Sidebar, NowPlayingBar, TrackRow, ResponseGraph, CoverArt, themed controls
    ├── pages/      # Tracks, Albums, Folders, Playlist, Queue, Equalizer, Settings
    └── icons/      # monochrome SVG icons (tinted with the theme color)
```
