# Third-party components

Sonkore is © Axiokyn and is licensed under the
[GNU General Public License v3.0 or later](LICENSE). It is built on the
components below, which keep their own licenses.

## Bundled in the application

| Component | Author | License | Source |
|---|---|---|---|
| [SoundTouch](https://codeberg.org/soundtouch/soundtouch) 2.3.3 — pitch, tempo and rate | Olli Parviainen | LGPL-2.1-or-later ([licenses/LGPL-2.1.txt](licenses/LGPL-2.1.txt)) | https://codeberg.org/soundtouch/soundtouch |
| [SoX Resampler (libsoxr)](https://sourceforge.net/projects/soxr/) 0.1.3 — sample-rate conversion when the device refuses the track's rate | Rob Sykes | LGPL-2.1-or-later ([licenses/LGPL-2.1.txt](licenses/LGPL-2.1.txt)); includes PFFFT (below) | https://sourceforge.net/projects/soxr/ |
| [miniaudio](https://miniaud.io) 0.11.21 — audio output (ALSA, PipeWire/PulseAudio, JACK, WASAPI, Core Audio) | David Reid | Public domain (Unlicense) or MIT-0, at your choice | https://github.com/mackron/miniaudio |
| [dr_libs](https://github.com/mackron/dr_libs) (dr_flac, dr_wav, dr_mp3) — FLAC, WAV and MP3 decoding | David Reid | Public domain (Unlicense) or MIT-0, at your choice | https://github.com/mackron/dr_libs |
| [libebur128](https://github.com/jiixyj/libebur128) 1.2.6 — EBU R128 loudness and true peak for ReplayGain analysis | Jan Kokemüller | MIT (below) | https://github.com/jiixyj/libebur128 |
| [libhdcd](https://github.com/bp0/libhdcd) 1.4 — HDCD decoding (based on foo_hdcd) | Burt P and others; Chris Moeller | BSD-2-Clause (below) | https://github.com/bp0/libhdcd |
| DST decoder (`src/audio/DstDecoder.cpp`), written after ISO/IEC 14496-3 subpart 10 and the layout read by FFmpeg's `dstdec.c` | Peter Ross (FFmpeg) for the reference implementation | LGPL-2.1-or-later ([licenses/LGPL-2.1.txt](licenses/LGPL-2.1.txt)), used under the GPL | https://ffmpeg.org |
| Headphone crossfeed (`src/audio/Crossfeed.hpp`), after [bs2b](https://bs2b.sourceforge.net) — Bauer stereophonic-to-binaural filter, same coefficients and presets | Boris Mikhaylov | MIT (below) | https://bs2b.sourceforge.net |
| [pocketfft](https://github.com/mreineck/pocketfft) (C++, header-only) — double-precision FFT for the convolution engine | Max-Planck-Society (Martin Reinecke) | BSD-3-Clause (below) | https://github.com/mreineck/pocketfft |
| Interface icons, based on [Feather Icons](https://feathericons.com) | Cole Bemis | MIT (below) | https://github.com/feathericons/feather |

SoundTouch, libsoxr, libebur128, libhdcd and pocketfft are compiled from their unmodified
upstream sources (see `cmake/Dependencies.cmake`); SoundTouch and libsoxr can also be linked from
the system with `-DSONKORE_USE_SYSTEM_SOUNDTOUCH=ON` or `-DSONKORE_USE_SYSTEM_SOXR=ON`. The complete source of Sonkore, which is
needed to rebuild it against a modified version of any of them, is available
under the GPL.

### Fonts

The interface fonts are distributed under the SIL Open Font License 1.1; each
font's license is in `fonts/<font>/OFL.txt`.

| Font | Copyright |
|---|---|
| Inter | © 2016 The Inter Project Authors — https://github.com/rsms/inter |
| Manrope | © 2019 The Manrope Project Authors — https://github.com/sharanda/manrope |
| Plus Jakarta Sans | © 2020 The Plus Jakarta Sans Project Authors — https://github.com/tokotype/PlusJakartaSans |
| Figtree | © 2022 The Figtree Project Authors — https://github.com/erikdkennedy/figtree |
| Outfit | © 2021 The Outfit Project Authors — https://github.com/Outfitio/Outfit-Fonts |
| DM Sans | © 2014 The DM Sans Project Authors — https://github.com/googlefonts/dm-fonts |
| Nunito Sans | © 2016 The Nunito Sans Project Authors — https://github.com/Fonthausen/NunitoSans |
| Rubik | © 2015 The Rubik Project Authors — https://github.com/googlefonts/rubik |
| Sora (the sidebar's wordmark only) | © 2019 The Sora Project Authors — https://github.com/sora-xor/sora-font |

## Used at run time (bundled in the AppImage and the Windows package)

| Component | License | Notes |
|---|---|---|
| [Qt 6](https://www.qt.io) | LGPL-3.0 ([licenses/LGPL-3.0.txt](licenses/LGPL-3.0.txt)) | Linked dynamically. The AppImage and the Windows package ship the unmodified Qt libraries next to the program, where they can be replaced |
| [FFmpeg](https://ffmpeg.org) (libavformat, libavcodec, libavutil) | LGPL-2.1-or-later (GPL if the installed build enables GPL parts) | Optional decoding of ALAC, AAC, Opus, Vorbis, AIFF, APE, WavPack, TTA, DSD and more. Not linked: the system's libraries (on Flathub, the runtime's) are loaded when such a file is scanned or played. A package that ships its own FFmpeg must use an LGPL build and offer its source: the AppImage carries an audio-only LGPL build, with its version, source and configure options in `usr/share/doc/sonkore/BUNDLED.txt`; the Windows package carries the same build as DLLs (`licenses/FFmpeg-BUILD.txt`) |
| [alsa-lib](https://www.alsa-project.org) | LGPL-2.1-or-later | Linux only, linked dynamically and always taken from the system (also by the AppImage): the DAC's volume control |
| [AutoEq](https://github.com/jaakkopasanen/AutoEq) by Jaakko Pasanen | MIT | Headphone correction profiles, downloaded only when you search for them |

## Methods

The PCM-to-DSD modulator (`src/audio/SigmaDelta.cpp`) uses noise transfer
functions designed with the published procedure of R. Schreier's Delta-Sigma
Toolbox (`synthesizeNTF`: optimised zeros, out-of-band gain 1.5); the
coefficients were computed for Sonkore, no Toolbox code is included. The
F-weighted noise-shaping dither uses the 9-tap error-feedback coefficients
published by Wannamaker (as used in SoX).

## PFFFT license (inside libsoxr)

```
Copyright (c) 2013  Julien Pommier ( pommier@modartt.com )

Based on original fortran 77 code from FFTPACKv4 from NETLIB
(http://www.netlib.org/fftpack), authored by Dr Paul Swarztrauber
of NCAR, in 1985.

As confirmed by the NCAR fftpack software curators, the following
FFTPACKv5 license applies to FFTPACKv4 sources. My changes are
released under the same terms.

FFTPACK license:

http://www.cisl.ucar.edu/css/software/fftpack5/ftpk.html

Copyright (c) 2004 the University Corporation for Atmospheric
Research ("UCAR"). All rights reserved. Developed by NCAR's
Computational and Information Systems Laboratory, UCAR,
www.cisl.ucar.edu.

Redistribution and use of the Software in source and binary forms,
with or without modification, is permitted provided that the
following conditions are met:

- Neither the names of NCAR's Computational and Information Systems
Laboratory, the University Corporation for Atmospheric Research,
nor the names of its sponsors or contributors may be used to
endorse or promote products derived from this Software without
specific prior written permission.

- Redistributions of source code must retain the above copyright
notices, this list of conditions, and the disclaimer below.

- Redistributions in binary form must reproduce the above copyright
notice, this list of conditions, and the disclaimer below in the
documentation and/or other materials provided with the
distribution.

THIS SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING, BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE CONTRIBUTORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES OR OTHER LIABILITY, WHETHER IN AN
ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS WITH THE
SOFTWARE.
```

## libebur128 license

```
Copyright (c) 2011 Jan Kokemüller

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## libhdcd license

```
libhdcd:
 Copyright (c) 2016, Burt P and others (see libhdcd/AUTHORS),
 based on foo_hdcd by Chris Moeller (see below),
 All rights reserved.

 Redistribution and use in source and binary forms, with or without
 modification, are permitted provided that the following conditions are met:

 1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
 2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

 THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
 ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
 WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
 ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
 (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
 ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
 SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Original HDCD decoder from foo_hdcd:
 Copyright (c) 2010, Chris Moeller,
 All rights reserved.
 Optimizations by Gumboot

 Redistribution and use in source and binary forms, with or without
 modification, are permitted provided that the following conditions are met:

 1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
 2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

 THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
 ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
 WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
 ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
 (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
 ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
 SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## bs2b license (crossfeed)

```
Copyright (c) 2005 Boris Mikhaylov

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## pocketfft license

```
Copyright (C) 2010-2018 Max-Planck-Society
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.
* Neither the name of the copyright holder nor the names of its contributors may
  be used to endorse or promote products derived from this software without
  specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Feather Icons license

```
The MIT License (MIT)

Copyright (c) 2013-2023 Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Windows

| Component | License | Notes |
|---|---|---|
| ASIO driver interface | — | ASIO is a trademark and software of Steinberg Media Technologies GmbH. Sonkore does not include Steinberg's SDK: `src/audio/AsioAbi.hpp` declares the driver interface (types, constants, the IASIO call table) from its published binary layout, and the drivers themselves are the ones installed on the user's computer. |
| Microsoft Visual C++ runtime (`msvcp140.dll`, `vcruntime140*.dll`) | Microsoft Visual Studio redistributable terms | Shipped next to `Sonkore.exe` in the Windows package (app-local deployment), so nothing has to be installed first. |
