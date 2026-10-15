# Hoja de ruta

Objetivo: un reproductor Hi-Fi a la altura de Roon y JRiver en todo lo que piden los usuarios más puristas, hecho solo con software libre y publicable gratis en todas partes.

## Reglas

1. Sonkore es GPL-3.0-or-later. Solo se agregan dependencias compatibles: MIT, BSD, Apache-2.0, LGPL o GPLv3.
2. Sin SDKs privativos, salvo las APIs del sistema (WASAPI, CoreAudio, ALSA). ASIO es GPLv3 desde octubre de 2025.
3. Cada binario publicado incluye sus licencias y el acceso al código fuente (ver `THIRD-PARTY.md` y `packaging/appimage/`).
4. El README declara que el desarrollo usa asistencia de IA.

## Fase 1: núcleo purista

- [x] **Gapless**: la pista siguiente se decodifica y empalma sin silencio, exacta al sample; se recorta el relleno del codificador (MP3 LAME/Xing, AAC/MP4, Opus pre-skip). El final de la última pista ya no se corta.
- [x] **ReplayGain / EBU R128** (libebur128, MIT): etiquetas ReplayGain y R128, análisis por pista y por álbum sin modificar los archivos, modo pista/álbum/apagado, preamp y prevención de clipping por true peak.
- [x] **Volumen por hardware** en Linux: control del mixer del DAC (ALSA, en dB) para conservar el bit-perfect al bajar el volumen; con PipeWire, el volumen del sink (el mismo control, a través del servidor de sonido).
- [x] **Volumen por hardware** en Windows: IAudioEndpointVolume con WASAPI exclusivo, en los dispositivos que lo implementan en hardware.
- [ ] **Volumen por hardware** en macOS (volumen del dispositivo en CoreAudio).
- [x] **DSD nativo y DoP** en Linux: DSF y DSDIFF sin pasar a PCM (ALSA DSD_U32_BE/LE, DoP de 24 bits), hasta la frecuencia máxima del DAC.
- [x] **DSD nativo** en Windows: salida ASIO (modo DSD de ASIO 2.2), también PCM bit-perfect en el formato propio del driver.
- [x] **DST** (DSD comprimido en DSDIFF, el formato de los SACD): decodificador propio (ISO/IEC 14496-3 subparte 10) que devuelve los bits exactos, así que va como DSD nativo o DoP.
- [x] **DSD → PCM propio** en doble precisión (FIR de 160 dB + SoX VHQ): 88,2 / 176,4 kHz, sin el ruido ultrasónico que deja FFmpeg (~−120 contra ~−28 dBFS).
- [ ] DSD nativo en macOS.
- [x] **Signal path**: cada etapa (archivo → decodificador → DSP → dispositivo) con formato, bits, frecuencia y calidad.
- [x] **Reproducción desde RAM**: el archivo completo en RAM (bloqueado si el sistema lo permite) antes de reproducir; la siguiente pista se carga en segundo plano.
- [x] **Prioridad de tiempo real** del hilo de audio (rtkit).
- [x] **CUE sheets y multicanal** (5.1 / 7.1 sin mezclar a estéreo, con reordenamiento exacto de canales).
- [x] **HDCD**: decodificación a 20 bits (libhdcd), solo en pistas con códigos HDCD.

## Fase 2: DSP de referencia

- [x] **Convolución FIR** (pocketfft, BSD-3, doble precisión): corrección de sala y filtros de REW, Dirac, etc.; mono, estéreo y true stereo.
- [x] **Crossfeed** para auriculares (diseño Bauer / bs2b, MIT).
- [x] **Remuestreo con filtros a elegir** (fase lineal, intermedia, mínima o lineal abrupto, SoX VHQ) y **upsampling** ×2 / ×4 / máximo del DAC.
- [x] **Dither con noise shaping** (F-weighted de 9 coeficientes).
- [x] **Upsampling PCM → DSD** (DSD64–DSD512) con modulador sigma-delta de 8.º orden (~113 dB de SNR en DSD64), un hilo por canal.
- [x] **Gestión de headroom** antes del DSP, con detector de clipping.
- [x] **Limitador true-peak** opcional al final del procesamiento (BS.1770, 4×; −1 dBTP de fábrica; exacto al bit por debajo del techo).
- [x] **Perfiles de sonido por salida**: EQ, convolución, crossfeed, headroom, dither, sobremuestreo y DSD recordados por dispositivo.
- [x] **Página Sonando ahora**: carátula grande, signal path y cola en una vista.
- [x] **Ecualizador de fase lineal**: la curva del EQ como FIR (convolución por FFT), activable en plena reproducción sin cortes.
- [x] **Salida nativa de PipeWire** con cambio de frecuencia por pista: bit-perfect (hasta 24 bits) sin tomar el DAC en exclusiva.
- [x] **Señales de prueba** desde la app: frecuencia del DAC (PCM y DSD), canales y bit-perfect (DTS).

## Calidad

- [x] Tests del motor (`tests/engine_test.cpp`): bit-perfect, gapless exacto, DSP, DSD, ASIO, estrés; limpios con ASan, UBSan y TSan. Corren en cada push (CI).

## Fase 3: biblioteca

- [ ] Metadatos, créditos y versiones desde MusicBrainz (CC0); carátulas de Cover Art Archive.
- [ ] Biografías desde Wikipedia/Wikidata (CC BY-SA, citando la fuente).
- [ ] Letras sincronizadas (LRCLIB).
- [ ] Scrobbling (ListenBrainz, Last.fm).
- [x] Vistas de artistas y géneros, paneles Género → Artista → Álbum y disposiciones al estilo de Spotify, Roon, MusicBee, JRiver, foobar2000, Winamp y Audacious.
- [x] Escaneo configurable: carpetas omitidas y archivos cortos fuera.
- [ ] Listas inteligentes, historial, valoraciones y filtros.

## Fase 4: red y multiroom

- [ ] Control remoto desde el navegador y el celular.
- [ ] UPnP/DLNA para enviar a streamers y DACs de red (libupnp, BSD).
- [ ] Chromecast.
- [ ] Multiroom sincronizado (Snapcast, GPLv3).

## Fase 5: plataformas y distribución

- [x] Linux: AppImage en GitHub Releases (CI en AlmaLinux 9: corre en cualquier distro con glibc 2.34+).
- [ ] Linux: AUR, COPR, openSUSE Build Service, Snap.
- [x] Windows x64: ASIO + WASAPI exclusivo; zip portable en GitHub Releases (Qt, runtime de MSVC y FFmpeg incluidos).
- [ ] Windows: instalador, Scoop, winget.
- [ ] macOS: CoreAudio en modo exclusivo y modo entero; GitHub Releases y tap propio de Homebrew.

## Fuera de alcance

- **MQA**: privativo, sin implementación abierta.
- **Roon Ready / RAAT**: protocolo cerrado (alternativa: UPnP + Snapcast).
- **Tidal / Qobuz integrados**: requieren un acuerdo comercial con cada servicio.
- **Flathub**: no acepta apps desarrolladas con IA.
