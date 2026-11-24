# Third-party components

Sonkore is © Axiokyn and is licensed under the
[GNU General Public License v3.0 or later](LICENSE). It is written in Rust;
the parts below come from other projects and keep their own licenses.

## Ported into Sonkore's own code

| Component | Author | License | Where |
|---|---|---|---|
| [SoundTouch](https://codeberg.org/soundtouch/soundtouch) 2.3.3 — pitch, tempo and rate, ported to Rust | Olli Parviainen | LGPL-2.1-or-later ([licenses/LGPL-2.1.txt](licenses/LGPL-2.1.txt)), used under the GPL | `crates/soundtouch` |
| [libhdcd](https://github.com/bp0/libhdcd) 1.4 — HDCD decoding (based on foo_hdcd), ported to Rust | Burt P and others; Chris Moeller | BSD-2-Clause (below) | `crates/hdcd` |
| DST decoder, written after ISO/IEC 14496-3 subpart 10 and the layout read by FFmpeg's `dstdec.c` | Peter Ross (FFmpeg) for the reference implementation | LGPL-2.1-or-later ([licenses/LGPL-2.1.txt](licenses/LGPL-2.1.txt)), used under the GPL | `crates/dsd/src/dst.rs` |
| Headphone crossfeed, after [bs2b](https://bs2b.sourceforge.net) — Bauer stereophonic-to-binaural filter, same coefficients and presets | Boris Mikhaylov | MIT (below) | `crates/dsp/src/crossfeed.rs` |
| Interface icons, based on [Feather Icons](https://feathericons.com) | Cole Bemis | MIT (below) | `crates/app/icons` |

## Rust crates built into the app

The app is built from 475 crates (Linux and Windows together; the list,
grouped by license, comes from `tools/licencias.py`). Their licenses are all
compatible with the GPL-3.0: MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0,
BSL-1.0, CC0, CDLA-Permissive-2.0 (the root certificates of `webpki-roots`),
and these:

| Crates | License | Notes |
|---|---|---|
| [Slint](https://slint.dev) (`slint`, `i-slint-*`) — the interface | GPL-3.0-only, Slint Royalty-free 2.0 or Slint Software 3.0 | Used under the GPL-3.0. `vendor/` holds three of them (and `parley`, `softbuffer`) with small changes, marked "Sonkore:" |
| [Symphonia](https://github.com/pdeljanov/Symphonia) (`symphonia-*`) — decoding of FLAC, MP3, AAC, ALAC, Vorbis, WAV, AIFF, CAF, MKV | MPL-2.0 | Unmodified |
| [ebur128](https://github.com/sdroege/ebur128) — EBU R128 loudness and true peak (a port of libebur128) | MIT | |
| [rubato](https://github.com/HEnquist/rubato), [rustfft](https://github.com/ejmahler/RustFFT) / realfft — resampling and the convolution engine's FFT | MIT / MIT OR Apache-2.0 | |

## Used at run time

| Component | License | Notes |
|---|---|---|
| [alsa-lib](https://www.alsa-project.org) | LGPL-2.1-or-later | Linux only, loaded from the system (also by the AppImage): ALSA output and the DAC's volume control |
| [PipeWire](https://pipewire.org) | MIT | Linux only, `libpipewire-0.3` loaded from the system when present |
| [AutoEq](https://github.com/jaakkopasanen/AutoEq) by Jaakko Pasanen | MIT | Headphone correction profiles, downloaded only when you search for them |
| [MusicBrainz](https://musicbrainz.org) and [Cover Art Archive](https://coverartarchive.org) | Web services (CC0 data; each cover belongs to its rights holder) | Only with "Missing covers" turned on (off by default): the album is looked up by artist and album name and its cover is kept in Sonkore's cache, for personal use |

## Methods

The PCM-to-DSD modulator (`crates/dsd/src/sigma_delta.rs`) uses noise
transfer functions designed with the published procedure of R. Schreier's
Delta-Sigma Toolbox (`synthesizeNTF`: optimised zeros, out-of-band gain 1.5);
the coefficients were computed for Sonkore, no Toolbox code is included. The
F-weighted noise-shaping dither (`crates/engine/src/quantize.rs`) uses the
9-tap error-feedback coefficients published by Wannamaker (as used in SoX).

## Fonts

The interface fonts are distributed under the SIL Open Font License 1.1; each
font's license is in `fonts/<font>/OFL.txt`.

| Font | Copyright |
|---|---|
| Inter (the default) | © 2016 The Inter Project Authors — https://github.com/rsms/inter |
| Manrope | © 2019 The Manrope Project Authors — https://github.com/google/fonts/tree/main/ofl/manrope |
| Plus Jakarta Sans | © 2020 The Plus Jakarta Sans Project Authors — https://github.com/tokotype/PlusJakartaSans |
| Figtree | © 2022 The Figtree Project Authors — https://github.com/erikdkennedy/figtree |
| Outfit | © 2021 The Outfit Project Authors — https://github.com/Outfitio/Outfit-Fonts |
| DM Sans | © 2014 The DM Sans Project Authors — https://github.com/googlefonts/dm-fonts |
| Nunito Sans | © 2016 The Nunito Sans Project Authors — https://github.com/Fonthausen/NunitoSans |
| Rubik | © 2015 The Rubik Project Authors — https://github.com/googlefonts/rubik |
| Sora (the sidebar's wordmark only) | © 2019 The Sora Project Authors — https://github.com/sora-xor/sora-font |

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
