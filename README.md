# Sonkore

Reproductor Hi-Fi minimalista para escritorio: biblioteca con carpetas vigiladas, listas de reproducción, ecualizador gráfico/paramétrico con AutoEQ, tono y tempo con Rubber Band o SoundTouch en precisión `double`, 39 temas, fondos de color y fuentes libres incluidas, y salida bit-perfect (ALSA `hw:` directo, PipeWire, JACK, WASAPI exclusivo, ASIO con DSD nativo). Motor en C++20, interfaz en Qt 6 / QML. Para Linux (AppImage, cualquier distro) y Windows 10/11 de 64 bits.

## Descargas

En [Releases](https://github.com/Axiokyn/Sonkore/releases) están las compilaciones finales, listas para usar sin instalar nada:

| Sistema | Archivo | Cómo se usa |
|---|---|---|
| **Linux** x86_64 (cualquier distro con glibc 2.34+: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE…) | `Sonkore-<versión>-x86_64.AppImage` | `chmod +x Sonkore-*.AppImage` y abrirlo. Trae Qt, los módulos QML y FFmpeg adentro |
| **Windows** 10/11 de 64 bits | `Sonkore-<versión>-windows-x64.zip` | Descomprimir y abrir `Sonkore.exe`. Trae Qt, el runtime de Visual C++ y FFmpeg adentro |

Cada archivo tiene al lado su `.sha256`. El AppImage se prueba en cada compilación sobre instalaciones limpias de Ubuntu 22.04 y 24.04, Debian 12, Fedora, Arch y openSUSE Tumbleweed (todas sus bibliotecas se resuelven y el reproductor arranca), y el paquete de Windows se compila con MSVC y se abre en Windows. Las compilaciones las hace GitHub Actions (`.github/workflows/build.yml`) desde el código de esta página, y cada tag `v*` publica una release.

## Funciones

| Área | Qué hace |
|---|---|
| **Formatos** | FLAC (hasta 32 bit / 384 kHz y más), WAV (PCM y float) y MP3 con dr_libs. Con FFmpeg (incluido en el AppImage y en el paquete de Windows; se carga recién cuando hace falta): ALAC y AAC (`.m4a`), Opus, Vorbis, FLAC en Ogg, AIFF, W64, APE, WavPack, TTA, TAK, Musepack, DSD (`.dsf`/`.dff`, también DST comprimido), WMA, MKA y más. Los formatos sin pérdida en enteros (ALAC, AIFF, APE, WavPack, TTA) llegan al DAC sin conversión, igual que FLAC, así que conservan el camino bit-perfect; en MP4 se recorta el relleno del codificador AAC |
| **Biblioteca** | Carpetas vigiladas con escaneo recursivo e incremental (inotify), caché JSON, tags FLAC/ID3v2/RIFF (y MP4/Vorbis/APE/ASF con FFmpeg), carátulas incrustadas o `cover.jpg`. Vistas de pistas y álbumes, y el **árbol de carpetas en la barra lateral**. La cola, el ecualizador y los ajustes están en la barra de reproducción |
| **Columnas** | 20 columnas (por defecto nombre de archivo, álbum, formato y tiempo; cualquiera se puede quitar; la primera ocupa el espacio libre; título, artista, álbum, género, año, pista, disco, tiempo, formato, códec, frecuencia, bits, canales, bitrate, tamaño, archivo, carpeta, fecha de alta, modificado…): se muestran u ocultan con clic derecho en la cabecera, **se mueven arrastrando la cabecera**, se redimensionan arrastrando el divisor (que sigue al puntero; doble clic restaura el ancho) y se ordena haciendo clic en la cabecera |
| **Selección y arrastre** | Selección múltiple con Ctrl / Shift / Ctrl+A. Las pistas se arrastran a una lista de la barra lateral, a la cola o a "crear lista". Se pueden soltar archivos o carpetas del gestor de archivos sobre una lista, y dentro de una lista se reordena arrastrando |
| **Gapless** | La pista siguiente se abre por adelantado y se empalma sin silencio, exacta al sample y sin reabrir el dispositivo, también con ecualizador, tono o volumen activos (verificado bit a bit). Se recorta el relleno del codificador (LAME en MP3, AAC en MP4, pre-skip de Opus). Si cambia la frecuencia o los canales, el DAC se reabre en el formato nuevo. El final del último tema nunca se corta: el dispositivo se detiene recién cuando reprodujo su buffer |
| **ReplayGain** | Nivelación por pista o por álbum (ReplayGain 2.0, referencia −18 LUFS), con preamplificación y prevención de clipping por *true peak*. Lee las etiquetas `REPLAYGAIN_*` (FLAC, Ogg, APE, WavPack, MP4, ID3 `TXXX`) y `R128_*` (Opus); las pistas sin etiquetas se miden en segundo plano con libebur128 (EBU R128), álbum por álbum. Los archivos nunca se modifican: los valores medidos quedan en la caché de la biblioteca. Apagado de fábrica; la ganancia cambia en el sample exacto de una unión gapless |
| **Listas** | Crear, renombrar, borrar, añadir desde cualquier vista, importar/exportar M3U/M3U8. Cola con aleatorio y repetición (todas/una) que se reordena arrastrando y se restaura al reabrir |
| **Ecualizador** | Gráfico de 10 bandas o paramétrico de hasta 32 bandas (pico, shelves, LP/HP, BP, notch), curva de respuesta interactiva (arrastrar puntos en frecuencia y ganancia sin pasar a las bandas vecinas, rueda = Q, doble clic = frecuencia estándar en modo gráfico), preamp con aviso de clipping y modo "Auto", 13 presets y perfiles propios, importar/exportar perfiles paramétricos (`.txt`) |
| **AutoEQ** | Busca entre los ~7 000 auriculares de [AutoEq](https://github.com/jaakkopasanen/AutoEq), aplica la corrección paramétrica y la guarda como perfil |
| **Salida** | Lista de dispositivos por backend: en Linux primero PipeWire (con "Predeterminado del sistema"), después las tarjetas como `hw:` directo de ALSA (exclusivo, bit-perfect) y al final JACK. En Windows, WASAPI (compartido o exclusivo) y los drivers **ASIO** instalados (directos, bit-perfect, en el formato de muestra propio del driver: entero de 16/24/32 bits o float). El cambio de dispositivo es en caliente y conserva la posición; un DAC que se conecta o desconecta se detecta solo y se avisa |
| **DSD** | Ajustes › Salida › Reproducción DSD: *Convertir a PCM* (de fábrica, cualquier DAC), *Nativo* (Linux: DSD_U32_BE/LE por ALSA a los DAC que lo ofrecen en una salida `hw:`; Windows: el modo DSD de ASIO 2.2 en los drivers que lo tienen; hasta donde llegue el DAC) o *DoP* (DSD sobre PCM en una salida directa, exclusiva o ASIO). DSF y DSDIFF sin comprimir; el bitstream llega intacto (verificado byte a byte), con silencio DSD (`0x69`) en lugar de ceros, unos ms de pre-roll para que el DAC cambie de modo, marcadores DoP siempre alternados y gapless entre pistas DSD de la misma frecuencia. El DSP y el volumen digital no se aplican al DSD; el volumen por hardware sí. Si la salida no puede llevarlo intacto, se convierte a PCM |
| **Signal path** | Botón ▣ en la barra de reproducción: cada etapa del archivo al DAC (origen, decodificador, memoria, HDCD, DSD, tono/tempo, ecualizador, ReplayGain, volumen, remuestreo, recuantización, salida y prioridad del hilo de audio) con su calidad, como en Roon: *sin pérdida*, *alta calidad*, *mejorado* (DSP elegido) o *baja calidad* |
| **Reproducción desde RAM** | Ajustes › Salida: cada archivo (hasta 2 GB) se lee a la RAM antes de sonar y, si el sistema lo permite (`RLIMIT_MEMLOCK`), queda bloqueado ahí; la siguiente pista se carga en segundo plano. Durante la reproducción no hay acceso al disco |
| **Tiempo real** | El hilo que entrega el audio al DAC pide prioridad de tiempo real a rtkit (como PipeWire), sin permisos de root; el signal path muestra la prioridad obtenida |
| **Multicanal** | Hasta 7.1 sin mezclar a estéreo. Si el dispositivo usa otro orden de canales (ALSA: FL FR RL RR FC LFE…), los samples se reordenan sin tocar sus valores; bit-perfect verificado en 5.1 y 7.1 |
| **CUE** | Imágenes de CD con `.cue`: la biblioteca muestra sus pistas (título, intérprete, álbum, fecha, género y ReplayGain de la hoja; formato y carátula del archivo) en lugar del archivo entero. Hojas en UTF-8 o Windows-1252; si el `.cue` nombra `.wav` y el archivo es `.flac`, se encuentra igual. Cada pista termina exactamente donde empieza la siguiente: con gapless, el disco suena completo bit a bit |
| **HDCD** | Ajustes › Salida: los CD HDCD se detectan en los primeros segundos de cada pista de 16 bits y se decodifican a 20 bits con libhdcd (salida idéntica a la del filtro HDCD de FFmpeg); el resto sigue bit-perfect |
| **DSP de referencia** | Todo en doble precisión, con el signal path mostrando cada etapa: **convolución FIR** (filtros de REW, Dirac, Acourate… mono, estéreo o true stereo de 4 canales, hasta 2^20 coeficientes, overlap-save particionado con pocketfft; idéntica a la convolución directa con error ~10⁻¹⁵), **crossfeed** para auriculares (Bauer/bs2b: predeterminado, Chu Moy, Jan Meier; idéntico a libbs2b), **upsampling** ×2/×4/máximo del DAC con **filtro a elegir** (fase lineal, intermedia, mínima o lineal abrupto; SoX VHQ), **headroom** configurable antes del DSP con **detector de clipping**, y **dither** TPDF o con **noise shaping** F-weighted |
| **PCM → DSD** | Convierte el PCM a DSD64/128/256 con un modulador sigma-delta de 7.º orden (NTF de Schreier con ceros óptimos, H∞ = 1,5) y lo envía como DSD nativo o DoP. El DSP y el volumen digital se aplican antes. Relación señal/ruido en la banda de audio: ~130 dB en DSD64 y ~170 dB en DSD128; lazo estable a plena escala; gapless entre pistas |
| **Volumen por hardware** | Ajustes › Salida › Control de volumen › DAC: con una salida directa (Linux: `hw:`, el mixer de ALSA en dB; Windows: WASAPI exclusivo, el control de volumen del propio dispositivo vía `IAudioEndpointVolume` cuando lo implementa en hardware), el volumen mueve el control del DAC y el motor queda en ganancia unitaria, así que el audio sigue siendo bit-perfect a cualquier volumen. Sigue los cambios hechos desde fuera (alsamixer, el control de volumen de Windows, otra app). Si el DAC no tiene control propio, se usa el volumen digital y se avisa |
| **Visualizador** | Espectro en vivo de lo que suena (24 bandas, FFT de 2048 puntos, sincronizado con la latencia del dispositivo) junto a la carátula: barras, espejo, línea o puntos, o apagado. Lee una copia de la salida: no toca el audio |
| **DSP** | Tono en semitonos con 7 decimales, tempo y rate, con dos motores a elegir: **Rubber Band** (por defecto; opciones de motor R3 "finer", formantes conservados, cambios de tono suaves e imagen estéreo centrada, todas apagadas de fábrica: lo más transparente para ajustes chicos de afinación) o **SoundTouch** (parámetros WSOLA avanzados). Los valores se guardan entre sesiones sin perder un bit del `double`. Si el dispositivo no acepta la frecuencia del tema, se remuestrea con SoX VHQ (en vez del lineal de miniaudio). Dither TPDF opcional, volumen digital (100 % = bit-perfect) |
| **Temas** | 39 temas propios: Sonkore, Basalt, Verdant, Porcelain, Slate, Crimson, Graphite, Fjord, Nocturne, Parchment, Obsidian, Dusk, Classic (compacta), Ocean, Forest, Ember, Mocha, Aurora, Neon, Sakura, Sand, Lavender, Glacier, Paper y Carbon (alto contraste), Studio, Daylight, Ironwood, Islands, Polar, Velvet, Citrus, Solstice, Linen, Harvest, Metro, Cocoa, Harbor y Abyss; más color de acento, densidad y escala de interfaz |
| **Escritorio** | Icono propio (SVG, PNG, `.ico`, `.icns`). Icono de bandeja con controles y cerrar a la bandeja (desactivados de fábrica), MPRIS (widget multimedia de GNOME/KDE, teclas multimedia, `playerctl`) y acciones rápidas en el icono del dock. En Windows, botones anterior/play/siguiente en la miniatura de la barra de tareas y progreso en el botón. Al abrir un archivo con Sonkore ya abierto, se pasa a la instancia existente |
| **Fluidez** | Render loop con hilo propio por defecto (la frecuencia de refresco completa: 144 Hz medidos), barra de progreso interpolada en cada frame, búsqueda en vivo al arrastrar, contador de FPS opcional y selector de renderizador (OpenGL / Vulkan / Software) |
| **Transparencia** | Ajustes › Apariencia › Transparencia (Baja / Media / Alta): el fondo de la ventana deja ver el escritorio; texto, tarjetas y menús quedan sólidos. Apagada (de fábrica) la ventana no tiene canal alfa y no cuesta nada; activarla pide reiniciar y no suma memoria |
| **Interfaz adaptable** | Velocidad de la ruedita configurable con desplazamiento suave (el touchpad sigue al dedo 1:1). Barra lateral redimensionable arrastrando su borde, con división ajustable entre carpetas y listas (doble clic restablece); en modo solo íconos, la vista Carpetas muestra las subcarpetas para navegar. Barra de título integrada en los colores del tema (mover y redimensionar los hace el compositor, como en una app nativa); opcionalmente la barra del sistema (estilo Adwaita en GNOME, la de KWin en Plasma). Mínimo 760×480 lógicos (entra en 1920×1080 al 200 %): la barra lateral pasa a solo íconos en ventanas angostas (o a elección), las cabeceras y los ajustes se reorganizan y la ventana de tono y tempo se desplaza en pantallas bajas |
| **Memoria** | Las páginas se crean al abrirlas y Ajustes / Ecualizador / Cola se liberan al salir; la cola y las listas leen las pistas de la biblioteca sin copiarlas; las carátulas se decodifican al tamaño visible y se redondean al decodificar (sin efectos de GPU); solo se cargan los drivers gráficos de la GPU presente y los pipelines compilados se guardan entre sesiones. En el motor de audio, el procesador de tono y tempo, el remuestreador, los buffers DSP y la copia para el visualizador existen solo mientras se usan (Rubber Band R3 ocupa ~6 MB a 44,1 kHz y ~24 MB a 192 kHz, y se libera entero al volver a neutro). FFmpeg no se enlaza: se abre recién al escanear o reproducir un formato que lo necesita, así una sesión de FLAC/WAV/MP3 no carga sus ~190 librerías (~4 MB privados y ~29 MB de RSS). La biblioteca se lee del JSON sin copia intermedia y comparte los textos repetidos (artista, álbum, género, códec): 3,4 MB en lugar de 9 MB tras cargar una biblioteca de ~2600 pistas, con la mitad de pico al arrancar. Con esa misma biblioteca: ~255 MB con OpenGL (automático) o Vulkan (antes ~450–480 MB), ~150 MB con el renderizador software |
| **Arranque seguro** | Si la app se cierra al iniciar, el arranque siguiente baja la configuración gráfica un escalón (renderizador elegido → automático → sin hilo de render → software) y lo avisa; si la GPU no puede iniciar, se reinicia sola con uno más seguro. `Sonkore --safe-mode` usa el renderizador software solo esa vez. El cierre de sesión o `kill` guardan la cola y los ajustes como un cierre normal, y la salida nunca queda colgada |

Atajos: `Espacio` reproducir/pausa · `Ctrl+←/→` anterior/siguiente · `Ctrl+↑/↓` volumen · `Ctrl+F` buscar · `Ctrl+A` seleccionar todo · `Enter` reproducir la selección · `Supr` quitar de la lista o la cola · `Ctrl+E` ecualizador · `Ctrl+,` ajustes · `Ctrl+Q` salir.

### Registrar Sonkore en el escritorio (Linux)

```bash
packaging/linux/install-user.sh          # entrada de menú + iconos en ~/.local/share
```

En Wayland el dock asocia la ventana con `sonkore.desktop`: sin este paso el panel muestra un icono genérico.

## Estructura

```
Sonkore/
├── CMakeLists.txt / cmake/Dependencies.cmake   # dependencias como tarballs verificados por SHA256
├── src/
│   ├── main.cpp
│   ├── audio/      # motor sin Qt: Decoder (FLAC/WAV/MP3 con dr_libs), FfmpegDecoder (el resto), DsdDecoder,
│   │               # AlsaDsdOutput (DSD nativo en Linux), AsioOutput (ASIO en Windows),
│   │               # DSPManager, TimeStretch (Rubber Band / SoundTouch), Resampler (soxr),
│   │               # SuperEQ + Biquad, SampleConvert, SpscRingBuffer, backend_impl.c
│   ├── library/    # Metadata (parsers de tags), LibraryModel, TrackFilterModel, TrackListModel,
│   │               # AlbumModel, FolderTreeModel, PlaylistManager, CoverImageProvider
│   ├── ui/         # AppController (escala, renderizador, arranque seguro, reinicio), DspController (transporte/cola/salida/tono y tempo), EqController (EQ/AutoEQ)
│   └── platform/   # MprisService, AlsaMixer, RealtimeKit (Linux); WinTaskbar, WinEndpointVolume (Windows)
├── tests/          # engine_test (bit-perfect, gapless, DSP, DSD, ASIO, stress) y make_corpus.sh (el audio que reproduce)
├── .github/        # workflows/build.yml: tests, AppImage, paquete de Windows y releases
├── fonts/          # fuentes de la interfaz (SIL Open Font License, licencia en cada <fuente>/OFL.txt): Inter, Manrope,
│                   # Plus Jakarta Sans, Figtree, Outfit, DM Sans, Nunito Sans, Rubik; pesos 400-700, subconjunto latino
├── packaging/      # icons/ (SVG maestros + generate.py), linux/ (.desktop, metainfo, install-user.sh), appimage/ (build.sh),
│                   # ffmpeg/ (configuración común), windows/ (.rc, .ico, build-ffmpeg.sh, package.ps1)
└── qml/
    ├── Main.qml, Theme.qml (temas), TrackColumns.qml (columnas), Prefs.qml, AppActions.qml (bus de eventos y arrastre)
    ├── EditSoundtouch.qml, PrecisionField.qml
    ├── components/ # Sidebar, NowPlayingBar, TrackRow, ResponseGraph, CoverArt, controles con el tema
    ├── pages/      # Tracks, Albums, Folders, Playlist, Queue, Equalizer, Settings
    └── icons/      # iconos SVG monocromos (se tiñen con el color del tema)
```

## Compilación

Requisitos: CMake ≥ 3.21, compilador C++20 y Qt ≥ 6.5 (Core, Gui, Widgets, Qml, Quick, QuickControls2, Network, Svg; DBus en Linux para MPRIS; alsa-lib en Linux para el volumen por hardware, opcional). Rubber Band, SoundTouch, libsoxr y libebur128 se descargan y compilan solos.

Para los formatos extra hacen falta los headers de FFmpeg ≥ 5.1 (`libavformat`, `libavcodec`, `libavutil`, por pkg-config; en Fedora `ffmpeg-free-devel` o `ffmpeg-devel`, en Debian/Ubuntu `libavformat-dev libavcodec-dev`). Solo se usan los headers: las librerías se cargan al ejecutar. Con `-DSONKORE_WITH_FFMPEG=AUTO` (por defecto) se activa si están; `ON` lo exige y `OFF` lo quita. Con `-DSONKORE_USE_SYSTEM_RUBBERBAND=ON`, `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` o `-DSONKORE_USE_SYSTEM_SOXR=ON` se usan las del sistema.

```bash
# Linux / macOS
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=/ruta/a/Qt/6.x/<kit>
cmake --build build
./build/bin/Sonkore
cmake --install build --prefix dist      # despliega el runtime de Qt

# Windows (Developer PowerShell de VS 2022)
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH=C:/Qt/6.x/msvc2022_64 `
      -DSONKORE_FFMPEG_INCLUDE_DIR=C:/ruta/a/ffmpeg/include      # solo headers; los DLL van junto al .exe
cmake --build build
packaging/windows/package.ps1 -Build build -FFmpeg C:/ruta/a/ffmpeg   # -> dist/Sonkore-<versión>-windows-x64.zip
```

`packaging/windows/build-ffmpeg.sh` compila (desde Linux, con MinGW-w64) el mismo FFmpeg solo de audio que trae el AppImage. Los drivers ASIO no necesitan el SDK de Steinberg: la interfaz está declarada en `src/audio/AsioAbi.hpp`.

### Tests

```bash
cmake -S . -B build -DSONKORE_BUILD_TESTS=ON ...
cmake --build build --target engine_test
tests/make_corpus.sh corpus            # necesita sox, flac, ffmpeg y python3
build/engine_test corpus               # o: build/engine_test corpus gaplessAlbum seekAndPause
```

Reproducen archivos reales por el motor (en el dispositivo nulo de miniaudio, sin placa de sonido) y comparan cada muestra que llega al "DAC": bit-perfect en 16 formatos, uniones gapless exactas (álbum e imagen CUE, también con EQ), búsqueda, pausa, volumen, EQ, convolución, crossfeed, sobremuestreo con cada filtro, tono y tempo con los dos motores, DSD (DoP, a PCM, modulador PCM→DSD), conversión de muestras ASIO, archivos dañados y una prueba de estrés con acciones al azar. Pasan limpios con AddressSanitizer, UBSan y ThreadSanitizer.

### AppImage (Linux, cualquier distro)

```bash
packaging/appimage/build.sh [ruta/a/Qt/6.x/gcc_64]   # requiere podman
# -> dist/Sonkore-<versión>-x86_64.AppImage (+ .sha256)
```

Se compila dentro de un contenedor AlmaLinux 9 (glibc 2.34, la misma base que los binarios de Qt), así que funciona en cualquier distro x86_64 desde 2021: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE Tumbleweed, etc. Incluye Qt (con los plugins de X11 y Wayland y el diálogo de archivos del portal), los módulos QML y un FFmpeg solo de audio (LGPL), por lo que ALAC, AAC, Opus, DSD y el resto funcionan sin instalar nada. Los drivers gráficos y el audio (ALSA, PipeWire, PulseAudio, JACK) se toman del sistema. Las licencias y el detalle de lo incluido quedan en `usr/share/doc/sonkore/` dentro del AppImage.

## Flujo de audio

```
                worker (no RT)                                                          callback RT
decoder s32 ─┬─ bypass ──────────────────────────────────────────────────────────┬─▶ ring<int32> ─▶ miniaudio (s32)
             └─ s32→f32 ─▶ Rubber Band / SoundTouch ─▶ f64 ─▶ SuperEQ ─▶ vol ─▶ [soxr] ─┘
                (o s32→f64 directo si solo hay EQ/volumen)        ─▶ cuantizar (+TPDF)
```

* **Bit-perfect**: con tono y tempo neutros, el EQ apagado o plano y el volumen al 100 %, los enteros decodificados llegan al driver sin tocarse. El dispositivo se abre en `s32` a la frecuencia del archivo, que cambia automáticamente en cada pista.
* **Remuestreo**: si el dispositivo no acepta la frecuencia del archivo (WASAPI compartido, DACs sin 44,1 kHz, DSD), se abre a su propia frecuencia y el worker convierte con SoX VHQ (fase lineal). La barra de estado lo indica ("remuestreado desde 44,1 kHz") y el indicador bit-perfect se apaga.
* **Tiempo real**: el callback solo copia desde una cola lock-free. El buffer se dimensiona a ≥ 2× el buffer del dispositivo y se pre-llena antes de arrancar, así que no hay underruns ni al reproducir ni al buscar.
* **Linux, salida directa**: al reproducir por un dispositivo `hw:` (◆), Sonkore le pide el DAC al servidor de sonido con el protocolo estándar de reserva de dispositivos (`org.freedesktop.ReserveDevice1`), igual que los programas de audio profesional. PipeWire lo suelta, las demás apps quedan en silencio o pasan a otra salida, y al liberarlo (botón **Liberar DAC**, menú del parlante, un minuto en pausa o al cerrar Sonkore) PipeWire lo recupera solo. Si el DAC lo tiene otro programa que no usa ese protocolo, Sonkore avisa que está ocupado.

## Primera ejecución

Salida "Predeterminado del sistema" (PipeWire en Linux), renderizador software (el más liviano; Ajustes › Avanzado ofrece OpenGL y Vulkan), ecualizador apagado (perfil Flat) y tono y tempo apagados (valores neutros): el camino bit-perfect queda intacto hasta que actives algo.

## Datos del usuario

Todo va en carpetas `sonkore`, sin carpeta de empresa:

| Qué | Linux | Windows |
|---|---|---|
| Ajustes | `~/.config/sonkore/sonkore.conf` | registro `HKCU\Software\sonkore` |
| Biblioteca, listas, sesión (cola y posición), perfiles de EQ, índice de AutoEQ | `~/.local/share/sonkore/` | `%LOCALAPPDATA%\sonkore\` |
| Caché (pipelines gráficos, carátulas para el sistema) | `~/.cache/sonkore/` | `%LOCALAPPDATA%\cache\sonkore\` |

## Pendiente

* El resto de la [hoja de ruta](ROADMAP.md): biblioteca (MusicBrainz, letras, scrobbling), red y multiroom, macOS.
* Windows: ASIO y el volumen por hardware se prueban en CI contra un driver simulado; falta confirmarlos con DACs reales (se agradecen reportes).

## Desarrollo

Sonkore se desarrolla con asistencia de IA (Claude, de Anthropic). Todo el código se revisa, se compila y se prueba antes de publicarse.

## Licencia

Sonkore es © Axiokyn y es software libre: se distribuye bajo la
[GNU General Public License v3.0 o posterior](LICENSE) (GPL-3.0-or-later).

Usa componentes de terceros con sus propias licencias (Rubber Band, SoundTouch,
libsoxr, miniaudio, dr_libs, FFmpeg en tiempo de ejecución, íconos basados en
Feather Icons y fuentes con licencia OFL); el detalle y los avisos de copyright
están en [THIRD-PARTY.md](THIRD-PARTY.md).
