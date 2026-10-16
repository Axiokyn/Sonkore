# Compilar Sonkore

[← Volver](../README.md)

Requisitos: CMake ≥ 3.21, compilador C++20 y Qt ≥ 6.5 (Core, Gui, Widgets, Qml, Quick, QuickControls2, Network, Svg; DBus en Linux para MPRIS; alsa-lib en Linux para el volumen por hardware, opcional). SoundTouch, libsoxr y libebur128 se descargan y compilan solos.

Para los formatos extra hacen falta los headers de FFmpeg ≥ 5.1 (`libavformat`, `libavcodec`, `libavutil`, por pkg-config; en Fedora `ffmpeg-free-devel` o `ffmpeg-devel`, en Debian/Ubuntu `libavformat-dev libavcodec-dev`). Solo se usan los headers: las librerías se cargan al ejecutar. Con `-DSONKORE_WITH_FFMPEG=AUTO` (por defecto) se activa si están; `ON` lo exige y `OFF` lo quita. Con `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` o `-DSONKORE_USE_SYSTEM_SOXR=ON` se usan las del sistema.

```bash
# Linux
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=/ruta/a/Qt/6.x/<kit>
cmake --build build
./build/bin/Sonkore
cmake --install build --prefix dist      # despliega el runtime de Qt

# Windows (Developer PowerShell de VS 2022)
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=C:/Qt/6.x/msvc2022_64 `
      -DSONKORE_FFMPEG_INCLUDE_DIR=C:/ruta/a/ffmpeg/include      # solo headers; los DLL van junto al .exe
cmake --build build
packaging/windows/package.ps1 -Build build -FFmpeg C:/ruta/a/ffmpeg   # -> dist/Sonkore-windows-x64.zip
```

`packaging/windows/build-ffmpeg.sh` compila (desde Linux, con MinGW-w64) el mismo FFmpeg solo de audio que trae el AppImage. Los drivers ASIO no necesitan el SDK de Steinberg: la interfaz está declarada en `src/audio/AsioAbi.hpp`.

## Tests

```bash
cmake -S . -B build -DSONKORE_BUILD_TESTS=ON ...
cmake --build build --target engine_test
tests/make_corpus.sh corpus            # necesita sox, flac, ffmpeg y python3 con numpy
build/engine_test corpus               # o: build/engine_test corpus gaplessAlbum seekAndPause
```

Reproducen archivos reales por el motor (en el dispositivo nulo de miniaudio, sin placa de sonido) y comparan cada muestra que llega al "DAC": bit-perfect en 16 formatos, uniones gapless exactas (álbum e imagen CUE, también con EQ), búsqueda, pausa, volumen, EQ (también de fase lineal: diseño, alineación exacta, activación en plena reproducción, gapless), convolución, crossfeed, sobremuestreo con cada filtro, tono y tempo, DSD (DoP; a PCM con el conversor propio: respuesta del filtro, nivel, SNR en banda, ruido ultrasónico, alineación con la fuente y seek; modulador PCM→DSD hasta DSD512, DST decodificado bit a bit contra un codificador DST propio que FFmpeg valida), señales de prueba (tono, canales, DSD, DTS), conversión de muestras ASIO, archivos dañados y una prueba de estrés con acciones al azar. La prueba `pipewire` (con `tests/pipewire-session.sh`, que levanta un PipeWire privado con un DAC virtual) reproduce por PipeWire y graba lo que recibe el DAC. Pasan limpios con AddressSanitizer, UBSan y ThreadSanitizer.

## AppImage (Linux, cualquier distro)

```bash
packaging/appimage/build.sh [ruta/a/Qt/6.x/gcc_64]   # requiere podman
# -> dist/Sonkore-x86_64.AppImage (+ .sha256)
```

Se compila dentro de un contenedor AlmaLinux 9 (glibc 2.34, la misma base que los binarios de Qt), así que funciona en cualquier distro x86_64 desde 2021: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE Tumbleweed, etc. Incluye Qt (con los plugins de X11 y Wayland y el diálogo de archivos del portal), los módulos QML y un FFmpeg solo de audio (LGPL), por lo que ALAC, AAC, Opus, DSD y el resto funcionan sin instalar nada. Los drivers gráficos y el audio (ALSA, PipeWire, PulseAudio, JACK) se toman del sistema. Las licencias y el detalle de lo incluido quedan en `usr/share/doc/sonkore/` dentro del AppImage.

## Paquetes publicados

Los paquetes de [Releases](https://github.com/Axiokyn/Sonkore/releases) se compilan desde este código: el AppImage dentro de un contenedor (`packaging/appimage/build.sh`) y el de Windows contra Qt para MSVC. El workflow `.github/workflows/build.yml` hace lo mismo en GitHub Actions y, además, prueba el AppImage sobre instalaciones limpias de Ubuntu 22.04 y 24.04, Debian 12, Fedora, Arch y openSUSE Tumbleweed.

## Registrar Sonkore en el escritorio (Linux)

```bash
packaging/linux/install-user.sh          # entrada de menú + iconos en ~/.local/share
```

En Wayland el dock asocia la ventana con `sonkore.desktop`: sin este paso el panel muestra un icono genérico.

## Estructura del código

```
Sonkore/
├── CMakeLists.txt / cmake/Dependencies.cmake   # dependencias como tarballs verificados por SHA256
├── src/
│   ├── main.cpp
│   ├── audio/      # motor sin Qt: Decoder (FLAC/WAV/MP3 con dr_libs), FfmpegDecoder (el resto), DsdDecoder,
│   │               # DstDecoder (DST → DSD), SigmaDelta (PCM → DSD), TestSignals,
│   │               # AlsaDsdOutput (DSD nativo en Linux), AsioOutput (ASIO en Windows),
│   │               # DSPManager, TimeStretch (SoundTouch), Resampler (soxr),
│   │               # SuperEQ + Biquad, SampleConvert, SpscRingBuffer, backend_impl.c
│   ├── library/    # Metadata (parsers de tags), LibraryModel, TrackFilterModel, TrackListModel,
│   │               # AlbumModel, FolderTreeModel, PlaylistManager, CoverImageProvider
│   ├── ui/         # AppController (escala, renderizador, arranque seguro, reinicio), DspController (transporte/cola/salida/tono y tempo), EqController (EQ/AutoEQ)
│   └── platform/   # MprisService, AlsaMixer, RealtimeKit (Linux); WinTaskbar, WinEndpointVolume (Windows)
├── docs/           # estas páginas y las capturas del README
├── tests/          # engine_test (bit-perfect, gapless, DSP, DSD, ASIO, stress) y make_corpus.sh (el audio que reproduce)
├── .github/        # workflows/build.yml: tests, AppImage, paquete de Windows y releases
├── fonts/          # fuentes de la interfaz (SIL Open Font License, licencia en cada <fuente>/OFL.txt): Inter
│                   # (la predeterminada: latino, griego y cirílico, con hinting), Manrope,
│                   # Plus Jakarta Sans, Figtree, Outfit, DM Sans, Nunito Sans, Rubik; pesos 400-700
├── packaging/      # icons/ (SVG maestros + generate.py), fonts/ (build-inter.py: Inter para la app), linux/ (.desktop, metainfo, install-user.sh), appimage/ (build.sh),
│                   # ffmpeg/ (configuración común), windows/ (.rc, .ico, build-ffmpeg.sh, package.ps1)
└── qml/
    ├── Main.qml, Theme.qml (temas), TrackColumns.qml (columnas), Prefs.qml, AppActions.qml (bus de eventos y arrastre)
    ├── EditSoundtouch.qml, PrecisionField.qml
    ├── components/ # Sidebar, NowPlayingBar, TrackRow, ResponseGraph, CoverArt, controles con el tema
    ├── pages/      # Tracks, Albums, Folders, Playlist, Queue, Equalizer, Settings
    └── icons/      # iconos SVG monocromos (se tiñen con el color del tema)
```
