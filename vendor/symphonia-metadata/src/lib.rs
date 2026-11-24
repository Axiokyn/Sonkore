// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(rust_2018_idioms)]
#![forbid(unsafe_code)]
// The following lints are allowed in all Symphonia crates. Please see clippy.toml for their
// justification.
#![allow(clippy::comparison_chain)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::identity_op)]
#![allow(clippy::manual_range_contains)]

pub mod flac;
pub mod id3v1;
pub mod id3v2;
pub mod itunes;
pub mod riff;
pub mod vorbis;

// Sonkore: a length read from a damaged file (up to 4 GiB from a 32-bit
// field) was allocated whole before reading. Read a chunk at a time
// instead: a length past the end of the stream fails there, having
// allocated about what the stream holds.
pub(crate) fn read_bytes_growing<B: symphonia_core::io::ReadBytes>(
    reader: &mut B,
    len: usize,
) -> symphonia_core::errors::Result<Vec<u8>> {
    const STEP: usize = 1 << 20;
    let mut buf = Vec::with_capacity(len.min(STEP));
    while buf.len() < len {
        let at = buf.len();
        buf.resize(at + (len - at).min(STEP), 0);
        reader.read_buf_exact(&mut buf[at..])?;
    }
    Ok(buf)
}
