// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(rust_2018_idioms)]
// The following lints are allowed in all Symphonia crates. Please see clippy.toml for their
// justification.
#![allow(clippy::comparison_chain)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::identity_op)]
#![allow(clippy::manual_range_contains)]

mod atoms;
mod demuxer;
mod fourcc;
mod fp;
mod stream;

pub use demuxer::IsoMp4Reader;

// Sonkore: reads `len` bytes a chunk at a time. A length from a damaged file
// (an atom's, up to 2^64; a sample's, up to 4 GiB) was allocated whole before
// the read failed at the end of the stream (an allocation failure aborts).
pub(crate) fn read_growing<B: symphonia_core::io::ReadBytes>(
    reader: &mut B,
    len: usize,
) -> symphonia_core::errors::Result<Box<[u8]>> {
    const STEP: usize = 1 << 20;
    let mut buf = Vec::with_capacity(len.min(STEP));
    while buf.len() < len {
        let at = buf.len();
        buf.resize(at + (len - at).min(STEP), 0);
        reader.read_buf_exact(&mut buf[at..])?;
    }
    Ok(buf.into_boxed_slice())
}
