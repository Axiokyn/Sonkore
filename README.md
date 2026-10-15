<div align="center">

<img src="packaging/icons/sonkore.svg" width="88" alt="">

# Sonkore

**Reproductor de música Hi-Fi para escritorio: bit-perfect, liviano y sin distracciones.**

Linux · Windows 10/11 · Software libre (GPL-3.0)

[**Descargar**](https://github.com/Axiokyn/Sonkore/releases/latest) &nbsp;·&nbsp; [Funciones](docs/funciones.md) &nbsp;·&nbsp; [Compilar](docs/compilar.md) &nbsp;·&nbsp; [Hoja de ruta](ROADMAP.md)

<br>

<img src="docs/screenshots/now-playing.webp" alt="Sonkore: la página Sonando ahora, con la carátula, la ruta de la señal y la cola" width="900">

</div>

## Por qué Sonkore

- **Bit-perfect de verdad.** ALSA `hw:` directo, PipeWire nativo (sigue la frecuencia de cada pista), JACK, WASAPI exclusivo y ASIO. Sin tocar el volumen ni el DSP, cada bit del archivo llega al DAC.
- **DSD nativo, DoP o a PCM**, incluido DST, con un conversor propio en doble precisión.
- **DSP de referencia.** Ecualizador gráfico o paramétrico con fase lineal y AutoEQ para unos 7000 auriculares, convolución FIR, crossfeed, sobremuestreo, limitador true-peak, dither con noise shaping, ReplayGain 2.0 y HDCD.
- **Ruta de la señal** a la vista: cada etapa del archivo al DAC, con su calidad.
- **Gapless exacto al sample**, también con ecualizador o volumen activos.
- **Casi todos los formatos:** FLAC, WAV, MP3, ALAC, AAC, Opus, Vorbis, AIFF, APE, WavPack, DSF/DFF, CUE y más.
- **Una biblioteca cómoda:** carpetas vigiladas, árbol de carpetas, 20 columnas, listas M3U, reproductor mini y 39 temas, con Sonkore Sans, su propia tipografía.

<p align="center">
  <img src="docs/screenshots/albums.webp" alt="Vista de álbumes" width="600">
  &nbsp;
  <img src="docs/screenshots/mini-player.webp" alt="Reproductor mini" width="270">
</p>

## Descargar

Listo para usar, sin instalar nada. Cada archivo trae al lado su `.sha256`.

| Sistema | Archivo | Cómo se usa |
|---|---|---|
| **Linux** x86_64 (glibc 2.34+: Ubuntu 22.04+, Debian 12+, Fedora, Arch, openSUSE…) | `Sonkore-<versión>-x86_64.AppImage` | `chmod +x` y abrirlo |
| **Windows** 10/11 de 64 bits | `Sonkore-<versión>-windows-x64.zip` | Descomprimir y abrir `Sonkore.exe` |

En Linux, `packaging/linux/install-user.sh` agrega Sonkore al menú de aplicaciones.

## Desarrollo

Sonkore se desarrolla con asistencia de IA (Claude, de Anthropic). Todo el código se revisa, se compila y se prueba antes de publicarse. Para compilarlo, ver [docs/compilar.md](docs/compilar.md).

## Licencia

© Axiokyn. Software libre bajo la [GNU General Public License v3.0 o posterior](LICENSE). Los componentes de terceros (SoundTouch, libsoxr, miniaudio, dr_libs, FFmpeg, íconos basados en Feather y fuentes OFL) y sus avisos de copyright están en [THIRD-PARTY.md](THIRD-PARTY.md).
