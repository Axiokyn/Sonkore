# Sonkore: estado y pendientes

Reproductor hi-fi en Rust con interfaz Slint, para Linux y Windows. La
descripción del motor (hilos, cadena de señal, constantes, claves de
configuración) está en `docs/engine.md`; las funciones, en
`docs/features.md`; cómo compilar y probar, en `docs/building.md`.

## Hecho

- **Motor:** decodificación con Symphonia (FLAC, WAV, AIFF, CAF, MP3,
  AAC/ALAC en MP4, Vorbis, MKV), bit-exacta y con búsquedas exactas a la
  muestra; gapless con empalme exacto (lista de edición de MP4, longitud real
  de AIFF); anillo sin bloqueos; fundido anti-clic de 8 ms con dither;
  bit-perfect por bloque.
- **Salidas:** ALSA directa (`alsa|hw:…`: S32 / S24 / S24_3 / S16, periodos de
  40 ms × 3), PipeWire nativo cargado en tiempo de ejecución (sigue la
  frecuencia de la pista, volumen del stream a 100 %) y WASAPI compartido y
  exclusivo en Windows.
- **DSP:** ecualizador gráfico y paramétrico (diseño Vicanek/RBJ, ajuste
  gráfico de Newton, fase lineal), AutoEQ, convolución, crossfeed (bs2b),
  limitador true-peak, headroom, ReplayGain, remuestreo y sobremuestreo con
  rubato, cuantizador con dither y conformado de ruido; SoundTouch portado
  (tono, tempo, velocidad); HDCD (libhdcd portado).
- **DSD:** DSF/DFF y DST, salida nativa y DoP, DSD→PCM.
- **Biblioteca:** escaneo y vigilancia de carpetas, etiquetas, carátulas
  (también de internet), sonoridad EBU R128, CUE, listas; `library.json`,
  `favorites.json`, `playlists.json` y `sonkore.conf` en el formato de
  siempre, así que una instalación existente se abre sin perder nada.
- **Plataforma:** MPRIS, bandeja, instancia única, mezclador ALSA, volumen
  de PipeWire, reserva del DAC (ReserveDevice1); en Windows, controles
  multimedia del sistema, bandeja, instancia única por tubería con nombre y
  volumen del dispositivo (`IAudioEndpointVolume`).
- **Interfaz:** ventana principal, todas las páginas, mini reproductor,
  Ajustes, ecualizador, tono y tempo, menús, temas, visualizador, fondos y
  escala de interfaz; 5 idiomas (`i18n/<idioma>.po`).
- **Entrega:** AppImage (`packaging/appimage.sh`, sin appimagetool: el
  runtime es `target/appimage-runtime-x86_64` y el squashfs lo escribe
  `tools/mkappimage`) y zip de Windows. La CI (`.github/workflows/build.yml`)
  compila y prueba en Linux y Windows, arma la AppImage en AlmaLinux 9
  (glibc 2.34), la prueba en Ubuntu 22.04 y 24.04, Debian 12, Fedora, Arch y
  openSUSE, y publica con `[release]` o un tag `v*`.

## Pendiente

- **Motor:**
  - salida PCM→DSD (el modulador está escrito, falta conectarlo);
  - filtros de conversión de fase intermedia y mínima (hoy convierten como el
    lineal: rubato solo tiene fase lineal);
  - prioridad de tiempo real (rtkit);
  - reordenado de canales en multicanal;
  - seguir el sink de PipeWire al que se enlaza el flujo (por ejemplo, un
    filtro de Easy Effects);
  - respaldo FFmpeg para Opus, APE, WavPack, TTA, Musepack y WMA, que hoy no
    se reproducen ni se les mide la sonoridad.
- **Interfaz:**
  - soltar archivos sobre la ventana;
  - paneles Género → Artista → Álbum sobre Pistas (la opción «Filtros en la
    página Pistas», `ui/trackBrowser`, está escondida hasta que existan: no
    hacía nada).
- **De la auditoría del 2026-11-24, sin hacer** (rendimiento con bibliotecas
  grandes, nada que se rompa):
  - la barra lateral rearma el árbol de carpetas entero (dos veces por cambio
    de pista); el buscador del mini arma un HashMap de 50.000 rutas por tecla y
    calcula `folder_tree` tres veces; `widest_badge` recorre todas las pistas en
    cada `fill_list`;
  - el menú de salidas lista los dispositivos en el hilo de la interfaz (7 ms
    en la prueba);
  - «Restaurar valores» se deshace en parte al cerrar (la geometría y el perfil
    de sonido de la salida se vuelven a guardar);
  - cambiar el modo de volumen desde el mini no actualiza la ventana de Ajustes
    abierta;
  - quitar varios favoritos a la vez escribe `favorites.json` una vez por pista;
  - el diseño del EQ de fase lineal (hasta 4 s a 384 kHz) corre en el hilo de
    audio: ya reutiliza buffers y se espacia, falta llevarlo a otro hilo;
  - con un búfer más largo que una pista corta (2000 ms), esa unión no es sin
    pausas (el motor llega al final antes de `set_next`).
- **Windows:** compila y corre en la CI; falta confirmar WASAPI exclusivo y el
  volumen del dispositivo con DACs reales.
- **Entrega:**
  - Flatpak;
  - antes de publicar: subir la versión en `Cargo.toml` (la 1.0.7 ya está
    publicada) y agregar la entrada en `<releases>` del metainfo;
  - `THIRD-PARTY.md` resume las licencias de los crates
    (`tools/licencias.py`), pero los paquetes todavía no llevan el texto de
    cada una (por ejemplo con cargo-about);
  - la AppImage local usa la glibc del sistema (2.44): para repartirla, la de
    la CI.

## Dependencias parcheadas

`vendor/` (`[patch.crates-io]`; los cambios llevan comentarios "Sonkore:"):
`softbuffer` (buffers ARGB en Wayland: esquinas transparentes),
`i-slint-backend-winit` (`SLINT_SCALE_FACTOR_MULTIPLIER` multiplica la escala
de la pantalla; relleno de la última fila/columna a escalas fraccionarias),
`i-slint-renderer-software` (bordes en píxeles de pantalla enteros),
`i-slint-core` (más zonas sucias por cuadro) y `parley` (métricas verticales
redondeadas a píxeles lógicos). Al actualizar Slint hay que volver a
aplicarlos; los detalles están en `CLAUDE.md`.
