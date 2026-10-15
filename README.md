# Sonkore

Reproductor Hi-Fi minimalista para escritorio: biblioteca con carpetas vigiladas, listas de reproducción, ecualizador gráfico/paramétrico con AutoEQ, tono y tempo con SoundTouch en precisión `double`, 39 temas, fondos de color y fuentes libres incluidas, y salida bit-perfect (ALSA `hw:` directo, PipeWire, JACK, WASAPI exclusivo, ASIO con DSD nativo). Motor en C++20, interfaz en Qt 6 / QML. Para Linux (AppImage, cualquier distro) y Windows 10/11 de 64 bits.

## Descargas

En [Releases](https://github.com/Axiokyn/Sonkore/releases) están las compilaciones finales, listas para usar sin instalar nada:

| Sistema | Archivo | Cómo se usa |
|---|---|---|
| **Linux** x86_64 (cualquier distro con glibc 2.34+: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE…) | `Sonkore-<versión>-x86_64.AppImage` | `chmod +x Sonkore-*.AppImage` y abrirlo. Trae Qt, los módulos QML y FFmpeg adentro |
| **Windows** 10/11 de 64 bits | `Sonkore-<versión>-windows-x64.zip` | Descomprimir y abrir `Sonkore.exe`. Trae Qt, el runtime de Visual C++ y FFmpeg adentro |

Cada archivo tiene al lado su `.sha256`. Los paquetes se compilan desde el código de esta página: el AppImage dentro de un contenedor (`packaging/appimage/build.sh`) y el de Windows con las herramientas de Microsoft contra Qt para MSVC. El workflow `.github/workflows/build.yml` hace lo mismo en GitHub Actions y, además, prueba el AppImage sobre instalaciones limpias de Ubuntu 22.04 y 24.04, Debian 12, Fedora, Arch y openSUSE Tumbleweed.

## Funciones

| Área | Qué hace |
|---|---|
| **Formatos** | FLAC (hasta 24 bit / 384 kHz y más; los FLAC de 32 bits de FLAC 1.4 con FFmpeg), WAV (PCM y float) y MP3 con dr_libs (decodificado en float, sin pasar por 16 bits). Con FFmpeg (incluido en el AppImage y en el paquete de Windows; se carga recién cuando hace falta): ALAC y AAC (`.m4a`), Opus, Vorbis, FLAC en Ogg, AIFF, W64, APE, WavPack, TTA, TAK, Musepack, DSD (`.dsf`/`.dff`, también DST comprimido), WMA, MKA y más. Los formatos sin pérdida en enteros (ALAC, AIFF, APE, WavPack, TTA) llegan al DAC sin conversión, igual que FLAC, así que conservan el camino bit-perfect; en MP4 se recorta el relleno del codificador AAC |
| **Biblioteca** | Carpetas vigiladas con escaneo recursivo e incremental (inotify), caché JSON, tags FLAC/ID3v2/RIFF (y MP4/Vorbis/APE/ASF con FFmpeg), carátulas incrustadas o `cover.jpg`, y **las que faltan desde Cover Art Archive** (el álbum se busca en MusicBrainz por artista y álbum; la imagen queda en la caché, nunca en tus carpetas). **Letras** en Sonando ahora: un `.lrc` junto a la pista, la etiqueta LYRICS del archivo o LRCLIB (dominio público); las sincronizadas siguen la música y un clic salta al verso. Ambas cosas se apagan en Ajustes › Biblioteca. Vistas de pistas y álbumes, y el **árbol de carpetas en la barra lateral**. La cola, el ecualizador y los ajustes están en la barra de reproducción |
| **Columnas** | 20 columnas (por defecto nombre de archivo, álbum, formato y tiempo; cualquiera se puede quitar; la primera ocupa el espacio libre; título, artista, álbum, género, año, pista, disco, tiempo, formato, códec, frecuencia, bits, canales, bitrate, tamaño, archivo, carpeta, fecha de alta, modificado…): se muestran u ocultan con clic derecho en la cabecera, **se mueven arrastrando la cabecera**, se redimensionan arrastrando el divisor (que sigue al puntero; doble clic restaura el ancho) y se ordena haciendo clic en la cabecera |
| **Selección y arrastre** | Selección múltiple con Ctrl / Shift / Ctrl+A. Las pistas se arrastran a una lista de la barra lateral, a la cola o a "crear lista". Se pueden soltar archivos o carpetas del gestor de archivos sobre una lista, y dentro de una lista se reordena arrastrando |
| **Gapless** | La pista siguiente se abre por adelantado y se empalma sin silencio, exacta al sample y sin reabrir el dispositivo, también con ecualizador, tono o volumen activos (verificado bit a bit). Se recorta el relleno del codificador (LAME en MP3, AAC en MP4, pre-skip de Opus). Si cambia la frecuencia o los canales, el DAC se reabre en el formato nuevo. El final del último tema nunca se corta: el dispositivo se detiene recién cuando reprodujo su buffer |
| **ReplayGain** | Nivelación por pista o por álbum (ReplayGain 2.0, referencia −18 LUFS), con preamplificación y prevención de clipping por *true peak* (el pico queda en −1 dBTP, el techo de EBU R128, porque un medidor 4× puede leer hasta ~0,6 dB de menos). Lee las etiquetas `REPLAYGAIN_*` (FLAC, Ogg, APE, WavPack, MP4, ID3 `TXXX`) y `R128_*` (Opus); las pistas sin etiquetas se miden en segundo plano con libebur128 (EBU R128), álbum por álbum, con el mono pesado como se escucha (en los dos parlantes), los laterales de 7.1 contados y el *true peak* de los archivos con pérdida medido por encima de 0 dBFS. Los archivos nunca se modifican: los valores medidos quedan en la caché de la biblioteca. Apagado de fábrica; la ganancia cambia en el sample exacto de una unión gapless |
| **Listas** | Crear, renombrar, borrar, añadir desde cualquier vista, importar/exportar M3U/M3U8. Cola con aleatorio y repetición (todas/una) que se reordena arrastrando y se restaura al reabrir. Un clic en el título de la barra de reproducción lleva la lista abierta a la canción que suena (o abre su carpeta) |
| **Reproductor mini** | `Ctrl+M`: la ventana pasa a un reproductor chico (estilo Winamp) con su propia lista debajo: la cola o una carpeta de la biblioteca, con columnas y orden propios. Buscador que filtra mientras escribís (`Ctrl+F`; `Enter` reproduce el primer resultado) y un botón que lleva la lista a la canción que suena (cambia a su carpeta si hace falta). Al agregar la primera carpeta de la biblioteca, la lista la muestra sola |
| **Ajustes por salida** | Cada dispositivo de salida recuerda su ecualizador, convolución, crossfeed, headroom, dither, sobremuestreo y ajustes de DSD: auriculares y parlantes vuelven cada uno con lo suyo al cambiar de salida (Ajustes › Salida; una salida nueva parte de los ajustes actuales) |
| **Ecualizador** | Gráfico de 10 bandas o paramétrico de hasta 32 bandas (pico, shelves, LP/HP, BP, notch), curva de respuesta interactiva (arrastrar puntos en frecuencia y ganancia sin pasar a las bandas vecinas, rueda = Q, doble clic = frecuencia estándar en modo gráfico), preamp con aviso de clipping y modo "Auto", 13 presets y perfiles propios, importar/exportar perfiles paramétricos (`.txt`, también los shelves por pendiente de Equalizer APO). Bandas de pico y shelf con **diseño *matched*** (Vicanek) en lugar de la transformada bilineal: la curva no se aplasta cerca de Nyquist (una banda de +6 dB en 16 kHz a 44,1 kHz da +6,0 dB, antes +2,1) y suena igual a 44,1, 96 o 192 kHz. En modo gráfico las ganancias se resuelven entre sí para que la curva pase por el valor de cada control (las bandas de una octava se solapan: diez a +6 dB daban +8,8 dB). Prender, apagar o aplanar el ecualizador es un fundido de ~10 ms, sin saltos. **Fase lineal** opcional: la misma curva como filtro FIR (a menos de 0,01 dB de los biquads; de 0,5 s a 4 s de taps según cuánto resuene la banda más angosta, así una muesca de Q 10 en graves llega a su profundidad completa), que retrasa todas las frecuencias por igual y deja intacta la temporización de transitorios e imagen estéreo; se activa, cambia y desactiva en plena reproducción sin cortes ni repeticiones, también entre pistas gapless |
| **AutoEQ** | Busca entre los ~7 000 auriculares de [AutoEq](https://github.com/jaakkopasanen/AutoEq), aplica la corrección paramétrica y la guarda como perfil |
| **Salida** | Lista de dispositivos por backend: en Linux primero PipeWire, **hablado de forma nativa** (con "Predeterminado del sistema"): el grafo de PipeWire sigue la frecuencia de cada pista mientras Sonkore reproduce (*Seguir la frecuencia de la pista*, se devuelve tras un minuto en pausa), el volumen del stream queda al 100 % y el signal path avisa si PipeWire convierte la frecuencia o si un volumen por software baja de 100 %; verificado grabando el DAC: idéntico al archivo hasta 24 bits. después las tarjetas como `hw:` directo de ALSA (exclusivo, bit-perfect) y al final JACK. En Windows, WASAPI (compartido o exclusivo) y los drivers **ASIO** instalados (directos, bit-perfect, en el formato de muestra propio del driver: entero de 16/24/32 bits o float). El cambio de dispositivo es en caliente y conserva la posición; un DAC que se conecta o desconecta se detecta solo y se avisa |
| **DSD** | Ajustes › Salida › Reproducción DSD: *Convertir a PCM* (de fábrica, cualquier DAC; con un **conversor propio en doble precisión**: FIR de 160 dB evaluado por tablas de bytes hasta 352,8 kHz y luego SoX VHQ, DSD64 → 88,2 kHz plano hasta 30 kHz, DSD128 y superiores → 176,4 kHz plano hasta 50–70 kHz; alineado en el tiempo con la fuente y con el ruido ultrasónico del DSD filtrado: ~−120 dBFS por encima de la banda contra ~−28 dBFS con el conversor de FFmpeg), *Nativo* (Linux: DSD_U32_BE/LE por ALSA a los DAC que lo ofrecen en una salida `hw:`; Windows: el modo DSD de ASIO 2.2 en los drivers que lo tienen; hasta donde llegue el DAC) o *DoP* (DSD sobre PCM en una salida directa, exclusiva o ASIO). DSF y DSDIFF, también **DST** (DSDIFF comprimido, el formato de los SACD): un decodificador DST propio (ISO/IEC 14496-3, subparte 10) devuelve los bits exactos cuadro a cuadro, así que también va como DSD nativo o DoP; el bitstream llega intacto (verificado byte a byte), con silencio DSD (`0x69`) en lugar de ceros, unos ms de pre-roll para que el DAC cambie de modo, marcadores DoP siempre alternados y gapless entre pistas DSD de la misma frecuencia. El DSP y el volumen digital no se aplican al DSD; el volumen por hardware sí. Si la salida no puede llevarlo intacto, se convierte a PCM |
| **Signal path** | La pastilla de estado de la barra de reproducción («Bit-perfect · 96 kHz», «Remuestreado · 48 kHz», «Procesado · 44,1 kHz») lo abre, y la página **Sonando ahora** (clic en la carátula de la barra) lo muestra junto a la carátula grande y la cola: cada etapa del archivo al DAC (origen, decodificador, memoria, HDCD, DSD, tono/tempo, ecualizador, ReplayGain, volumen, remuestreo, recuantización, salida y prioridad del hilo de audio) con su calidad, como en Roon: *sin pérdida*, *alta calidad*, *mejorado* (DSP elegido) o *baja calidad* |
| **Reproducción desde RAM** | Ajustes › Salida: cada archivo (hasta 2 GB) se lee a la RAM antes de sonar y, si el sistema lo permite (`RLIMIT_MEMLOCK`), queda bloqueado ahí; la siguiente pista se carga en segundo plano. Durante la reproducción no hay acceso al disco |
| **Tiempo real** | El hilo que entrega el audio al DAC pide prioridad de tiempo real a rtkit (como PipeWire), sin permisos de root; el signal path muestra la prioridad obtenida |
| **Multicanal** | Hasta 7.1 sin mezclar a estéreo. Si el dispositivo usa otro orden de canales (ALSA: FL FR RL RR FC LFE…), los samples se reordenan sin tocar sus valores; bit-perfect verificado en 5.1 y 7.1 |
| **Señales de prueba** | Ajustes › Salida › Señales de prueba: tonos generados al momento que pasan por el mismo camino que la música y vuelven a lo que sonaba al terminar. *Frecuencia del DAC*: 1 kHz a −20 dBFS a cada frecuencia PCM (44,1–384 kHz) y en DSD64/128/256 (con el modulador propio), para leer la frecuencia en la pantalla del DAC. *Parlantes*: el tono en cada canal por turno (estéreo, 5.1, 7.1) en el orden del archivo. *Bit-perfect (DTS)*: un stream DTS dentro de un WAV de calidad CD (codificado con FFmpeg): un receptor AV reproduce el tono solo si cada bit llega intacto |
| **CUE** | Imágenes de CD con `.cue`: la biblioteca muestra sus pistas (título, intérprete, álbum, fecha, género y ReplayGain de la hoja; formato y carátula del archivo) en lugar del archivo entero. Hojas en UTF-8 o Windows-1252; si el `.cue` nombra `.wav` y el archivo es `.flac`, se encuentra igual. Cada pista termina exactamente donde empieza la siguiente: con gapless, el disco suena completo bit a bit |
| **HDCD** | Ajustes › Salida: los CD HDCD se detectan en los primeros segundos de cada pista de 16 bits y se decodifican a 20 bits con libhdcd (salida idéntica a la del filtro HDCD de FFmpeg); el resto sigue bit-perfect |
| **DSP de referencia** | Todo en doble precisión, con el signal path mostrando cada etapa: **convolución FIR** (filtros de REW, Dirac, Acourate… mono, estéreo o true stereo de 4 canales, hasta 2^20 coeficientes, overlap-save particionado con pocketfft; idéntica a la convolución directa con error ~10⁻¹⁵), **crossfeed** para auriculares (Bauer/bs2b: predeterminado, Chu Moy, Jan Meier; idéntico a libbs2b), **upsampling** ×2/×4/máximo del DAC con **filtro a elegir** (fase lineal, intermedia, mínima o lineal abrupto; SoX VHQ), **headroom** configurable antes del DSP con **detector de clipping**, **limitador true-peak** opcional al final del procesamiento (pico real sobremuestreado 4×, techo de −1 dBTP de fábrica, 1,5 ms de anticipación con ataque lineal y 50 ms de liberación, todos los canales juntos; exacto al bit por debajo del techo y nunca en el camino bit-perfect), y **dither** TPDF o con **noise shaping** F-weighted (solo en salidas de hasta 20 bits; a 24 bits el piso plano de −144 dBFS ya está muy por debajo de cualquier DAC y el shaping solo lo subiría 18 dB). Una pista con más bits que el DAC (24 bits en uno de 16, 32 bits, con pérdida o DSD convertido) se recuantiza con dither en vez de truncarse. Las fuentes en float (MP3, AAC, Opus, Vorbis, WAV float, DSD convertido) llegan al DSP en doble precisión con lo que pasa de 0 dBFS intacto, para que el volumen, ReplayGain o el headroom lo bajen en vez de recortarse en el decodificador. El silencio digital sale como ceros (sin colchón de dither), y los fundidos de pausa y búsqueda vuelven a la grilla del DAC con dither. Volumen, ReplayGain y headroom cambian deslizándose muestra a muestra a lo largo de un bloque, sin clicks ni *zipper* al mover el volumen (en una unión gapless la ganancia nueva sigue empezando en el sample exacto). Los denormales se descartan en el hilo de audio (FTZ/DAZ): un EQ sobre silencio digital no se vuelve 50 veces más lento |
| **PCM → DSD** | Convierte el PCM a DSD64/128/256/512 con un modulador sigma-delta de 8.º orden (NTF de Schreier con ceros óptimos, H∞ = 1,5) y lo envía como DSD nativo o DoP. El DSP y el volumen digital se aplican antes. Relación señal/ruido en la banda de audio (20 Hz–20 kHz, 997 Hz a −6,9 dBFS): ~113 dB en DSD64 (108 dB con el 7.º orden anterior) y muy por encima de 24 bits desde DSD128; lazo estable a plena escala; gapless entre pistas. Cada canal se modula en su propio núcleo: DSD512 estéreo (22,6 MHz por canal) corre en tiempo real |
| **Volumen por hardware** | Ajustes › Salida › Control de volumen › DAC (o Automático): **con PipeWire, el volumen del sink** (el control del propio DAC cuando el dispositivo lo tiene: PipeWire mueve el mixer de ALSA y deja las muestras intactas), así que las teclas de volumen del escritorio y el deslizador de Sonkore mueven el mismo control y el camino sigue bit-perfect; en una salida virtual (una cadena de efectos) se avisa que PipeWire escala por software. Sonkore sigue el sink que realmente alimenta el stream: si Easy Effects u otro filtro se abre o se cierra, el signal path, la lista de salidas y el control de volumen cambian con él, y la música sigue por la salida siguiente (WirePlumber pide pausar a todos los reproductores cuando desaparece su sink, pensado para un DAC desenchufado; Sonkore distingue ese caso de un filtro que se cierra). Con una salida directa (Linux: `hw:`, el mixer de ALSA en dB; Windows: WASAPI exclusivo, el control de volumen del propio dispositivo vía `IAudioEndpointVolume` cuando lo implementa en hardware), el volumen mueve el control del DAC y el motor queda en ganancia unitaria, así que el audio sigue siendo bit-perfect a cualquier volumen. Sigue los cambios hechos desde fuera (alsamixer, el control de volumen de Windows, otra app). Si el DAC no tiene control propio, se usa el volumen digital y se avisa |
| **Visualizador** | Espectro en vivo de lo que suena (24 bandas, FFT de 2048 puntos, sincronizado con la latencia del dispositivo) junto a la carátula: barras con picos que se sostienen y caen, espejo, línea suave o LEDs, o apagado. Se dibuja entero en una imagen con QPainter, como en foobar2000 o MusicBee: antialiasing, alturas fraccionarias (se mueve de a fracciones de píxel), todas las barras del mismo ancho sobre la grilla de píxeles del monitor y degradé por altura; se ve igual con el renderizador por software que con OpenGL o Vulkan. Lee una copia de la salida: no toca el audio |
| **DSP** | Tono en semitonos con 7 decimales, tempo y rate con **SoundTouch** (WSOLA, muy poca CPU; parámetros avanzados: filtro antialiasing, secuencia, ventana de búsqueda y solapamiento). Los valores se guardan entre sesiones sin perder un bit del `double`. Si el dispositivo no acepta la frecuencia del tema, se remuestrea con SoX VHQ (en vez del lineal de miniaudio). Dither TPDF opcional, volumen digital (100 % = bit-perfect) con escala lineal en dB en los últimos 60 dB (pasos de 1 dB con la rueda y `Ctrl+↑/↓`, el valor en dB al pasar el mouse) |
| **Temas** | 39 temas propios: Sonkore, Basalt, Verdant, Porcelain, Slate, Crimson, Graphite, Fjord, Nocturne, Parchment, Obsidian, Dusk, Classic (compacta), Ocean, Forest, Ember, Mocha, Aurora, Neon, Sakura, Sand, Lavender, Glacier, Paper y Carbon (alto contraste), Studio, Daylight, Ironwood, Islands, Polar, Velvet, Citrus, Solstice, Linen, Harvest, Metro, Cocoa, Harbor y Abyss; más color de acento, densidad y escala de interfaz |
| **Escritorio** | Icono propio (SVG, PNG, `.ico`, `.icns`). Icono de bandeja con controles y cerrar a la bandeja (desactivados de fábrica), MPRIS (widget multimedia de GNOME/KDE, teclas multimedia, `playerctl`) y acciones rápidas en el icono del dock. En Windows, botones anterior/play/siguiente en la miniatura de la barra de tareas y progreso en el botón. Al abrir un archivo con Sonkore ya abierto, se pasa a la instancia existente |
| **Fluidez** | Render loop con hilo propio por defecto (la frecuencia de refresco completa: 144 Hz medidos), barra de progreso interpolada en cada frame, búsqueda en vivo al arrastrar, contador de FPS opcional y selector de renderizador (OpenGL / Vulkan / Software). Con el renderizador software, las formas redondeadas que Qt dibuja mal a escalas fraccionarias (círculos, radios chicos y todo rectángulo redondeado con borde: perillas, interruptores, casillas, botones de ventana, menús, tarjetas de Ajustes, campos, etiquetas de formato) se dibujan con antialiasing sobre los píxeles del monitor; revisado a 125, 150, 166 y 175 %; y con la ventana maximizada se completan las últimas filas y columnas de píxeles que ese renderizador deja sin pintar a esas escalas (se veía el escritorio por una línea abajo) |
| **Transparencia** | Ajustes › Apariencia › Transparencia (Baja / Media / Alta): el fondo de la ventana deja ver el escritorio; texto, tarjetas y menús quedan sólidos. Apagada (de fábrica) la ventana no tiene canal alfa y no cuesta nada; activarla pide reiniciar y no suma memoria |
| **Interfaz adaptable** | Velocidad de la ruedita configurable con desplazamiento suave (el touchpad sigue al dedo 1:1). Barra lateral redimensionable arrastrando su borde, con división ajustable entre carpetas y listas (doble clic restablece); en modo solo íconos, la vista Carpetas muestra las subcarpetas para navegar. Barra de título integrada en los colores del tema (mover y redimensionar los hace el compositor, como en una app nativa), con su contorno y esquinas redondeadas parejos en los cuatro lados a cualquier escala, también con el renderizador software (se dibujan con antialiasing sobre los píxeles que la ventana realmente muestra, aunque el compositor recorte la última fila a 125 %, 150 % o 166 %); opcionalmente la barra del sistema (estilo Adwaita en GNOME, la de KWin en Plasma). Mínimo 760×480 lógicos (entra en 1920×1080 al 200 %): la barra lateral pasa a solo íconos en ventanas angostas (o a elección), las cabeceras y los ajustes se reorganizan y la ventana de tono y tempo se desplaza en pantallas bajas |
| **Memoria** | Las páginas se crean al abrirlas y Ajustes / Ecualizador / Cola se liberan al salir; la cola y las listas leen las pistas de la biblioteca sin copiarlas; las carátulas se decodifican al tamaño visible y se redondean al decodificar (sin efectos de GPU); solo se cargan los drivers gráficos de la GPU presente y los pipelines compilados se guardan entre sesiones; los diálogos de archivos (QtQuick.Dialogs, tres bibliotecas) se cargan la primera vez que se abre uno, y las carátulas se decodifican con su formato ya identificado, sin preguntar a cada plugin de imagen. En el motor de audio, el procesador de tono y tempo, el remuestreador, los buffers DSP y la copia para el visualizador existen solo mientras se usan (SoundTouch se libera entero al volver a neutro). Con PipeWire hablado de forma nativa, los clientes de PulseAudio y JACK no se cargan (libpulse arrastra libsndfile, FLAC, Vorbis, Opus, LAME y mpg123): se crean solo si PipeWire falla o hay un dispositivo JACK guardado. Los paquetes traen solo los plugins de imagen que se usan (SVG, JPEG, WebP, GIF). FFmpeg no se enlaza: se abre recién al escanear o reproducir un formato que lo necesita, así una sesión de FLAC/WAV/MP3 no carga sus ~190 librerías (~4 MB privados y ~29 MB de RSS). La biblioteca se lee del JSON sin copia intermedia y comparte los textos repetidos (artista, álbum, género, códec): 3,4 MB en lugar de 9 MB tras cargar una biblioteca de ~2600 pistas, con la mitad de pico al arrancar. Con esa misma biblioteca: ~255 MB con OpenGL (automático) o Vulkan (antes ~450–480 MB), ~150 MB con el renderizador software. Con el renderizador software, Qt repinta la ventana entera en cada fotograma cuando la escala no es entera (125 %, 150 %…); Sonkore le pide repintar solo lo que cambia y vuelve al repintado entero mientras se mueve media ventana (una lista que se desplaza): reproduciendo con el visualizador en una ventana 4K, de ~30 % de un núcleo a ~9 %. En Wayland, además, las animaciones avanzan al ritmo de los turnos de dibujo del compositor, para un desplazamiento más parejo |
| **CPU** | El hilo que decodifica y procesa duerme lo que el DAC tarda en liberar lugar (unos 40 despertares por segundo en vez de 500) y la interfaz consulta al motor 20 veces por segundo solo mientras suena (4 en pausa). Medido en un Xeon de 2,8 GHz, porcentaje de un núcleo: 44,1/16 bit-perfect 1,1 %; EQ 1,5 %; EQ de fase lineal 2,6 % (5,9 % a 96 kHz); convolución de 65 536 coeficientes 4,4 %; ×4 a 176,4 kHz 2,6 %; DSD64 → PCM 2,9 %; DSD256 → PCM 7 % |
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
│   │               # DstDecoder (DST → DSD), SigmaDelta (PCM → DSD), TestSignals,
│   │               # AlsaDsdOutput (DSD nativo en Linux), AsioOutput (ASIO en Windows),
│   │               # DSPManager, TimeStretch (SoundTouch), Resampler (soxr),
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

Requisitos: CMake ≥ 3.21, compilador C++20 y Qt ≥ 6.5 (Core, Gui, Widgets, Qml, Quick, QuickControls2, Network, Svg; DBus en Linux para MPRIS; alsa-lib en Linux para el volumen por hardware, opcional). SoundTouch, libsoxr y libebur128 se descargan y compilan solos.

Para los formatos extra hacen falta los headers de FFmpeg ≥ 5.1 (`libavformat`, `libavcodec`, `libavutil`, por pkg-config; en Fedora `ffmpeg-free-devel` o `ffmpeg-devel`, en Debian/Ubuntu `libavformat-dev libavcodec-dev`). Solo se usan los headers: las librerías se cargan al ejecutar. Con `-DSONKORE_WITH_FFMPEG=AUTO` (por defecto) se activa si están; `ON` lo exige y `OFF` lo quita. Con `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` o `-DSONKORE_USE_SYSTEM_SOXR=ON` se usan las del sistema.

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
tests/make_corpus.sh corpus            # necesita sox, flac, ffmpeg y python3 con numpy
build/engine_test corpus               # o: build/engine_test corpus gaplessAlbum seekAndPause
```

Reproducen archivos reales por el motor (en el dispositivo nulo de miniaudio, sin placa de sonido) y comparan cada muestra que llega al "DAC": bit-perfect en 16 formatos, uniones gapless exactas (álbum e imagen CUE, también con EQ), búsqueda, pausa, volumen, EQ (también de fase lineal: diseño, alineación exacta, activación en plena reproducción, gapless), convolución, crossfeed, sobremuestreo con cada filtro, tono y tempo, DSD (DoP; a PCM con el conversor propio: respuesta del filtro, nivel, SNR en banda, ruido ultrasónico, alineación con la fuente y seek; modulador PCM→DSD hasta DSD512, DST decodificado bit a bit contra un codificador DST propio que FFmpeg valida), señales de prueba (tono, canales, DSD, DTS), conversión de muestras ASIO, archivos dañados y una prueba de estrés con acciones al azar. La prueba `pipewire` (con `tests/pipewire-session.sh`, que levanta un PipeWire privado con un DAC virtual) reproduce por PipeWire y graba lo que recibe el DAC. Pasan limpios con AddressSanitizer, UBSan y ThreadSanitizer.

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
             └─ s32→f32 ─▶ SoundTouch ─▶ f64 ─▶ SuperEQ ─▶ vol ─▶ [soxr] ──────────────┘
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
| Caché (pipelines gráficos, carátulas para el sistema, carátulas y letras descargadas) | `~/.cache/sonkore/` | `%LOCALAPPDATA%\cache\sonkore\` |

## Pendiente

* El resto de la [hoja de ruta](ROADMAP.md): biblioteca (metadatos de MusicBrainz, scrobbling), red y multiroom, macOS.
* Windows: ASIO y el volumen por hardware se prueban en CI contra un driver simulado; falta confirmarlos con DACs reales (se agradecen reportes).

## Desarrollo

Sonkore se desarrolla con asistencia de IA (Claude, de Anthropic). Todo el código se revisa, se compila y se prueba antes de publicarse.

## Licencia

Sonkore es © Axiokyn y es software libre: se distribuye bajo la
[GNU General Public License v3.0 o posterior](LICENSE) (GPL-3.0-or-later).

Usa componentes de terceros con sus propias licencias (SoundTouch,
libsoxr, miniaudio, dr_libs, FFmpeg en tiempo de ejecución, íconos basados en
Feather Icons y fuentes con licencia OFL); el detalle y los avisos de copyright
están en [THIRD-PARTY.md](THIRD-PARTY.md).
