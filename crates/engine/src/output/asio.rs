//! ASIO output (Windows): a DAC's own driver, the way pro-audio and hi-fi
//! DACs are meant to be driven: no Windows mixer, the DAC at the track's own
//! rate, DoP, and native DSD for the drivers that offer it (ASIO 2.2's DSD
//! mode).
//!
//! The driver interface is declared here from its published binary layout
//! (Steinberg's SDK is not needed): drivers are in-process COM objects listed
//! under `HKLM\SOFTWARE\ASIO`, whose vtable a host calls. Windows x64 only,
//! where there is a single calling convention. A driver is created, used and
//! released on a thread of its own (an STA, as ASIO drivers expect); it fills
//! its buffers from its own thread through the callbacks, which carry no
//! context, so one ASIO stream runs at a time.
//!
//! Everything but finding and loading a driver builds everywhere, so the
//! tests drive this code with a fake driver on any system.

use super::*;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, AtomicPtr};
use std::sync::mpsc;

/// The driver's binary interface (Steinberg ASIO 2.3). `long` is 32 bits on
/// Windows; the structs have natural alignment.
pub mod abi {
    use std::ffi::{c_char, c_void};

    pub type AsioBool = i32;
    pub type AsioError = i32;

    pub const ASE_OK: AsioError = 0;
    /// What `future()` answers when it did what was asked.
    pub const ASE_SUCCESS: AsioError = 0x3f48_47a0;

    // Sample types.
    pub const INT16_MSB: i32 = 0;
    pub const INT24_MSB: i32 = 1;
    pub const INT32_MSB: i32 = 2;
    pub const FLOAT32_MSB: i32 = 3;
    pub const FLOAT64_MSB: i32 = 4;
    pub const INT32_MSB16: i32 = 8;
    pub const INT32_MSB18: i32 = 9;
    pub const INT32_MSB20: i32 = 10;
    pub const INT32_MSB24: i32 = 11;
    pub const INT16_LSB: i32 = 16;
    pub const INT24_LSB: i32 = 17;
    pub const INT32_LSB: i32 = 18;
    pub const FLOAT32_LSB: i32 = 19;
    pub const FLOAT64_LSB: i32 = 20;
    pub const INT32_LSB16: i32 = 24;
    pub const INT32_LSB18: i32 = 25;
    pub const INT32_LSB20: i32 = 26;
    pub const INT32_LSB24: i32 = 27;
    /// 8 DSD bits per byte, the oldest in the least significant bit.
    pub const DSD_INT8_LSB1: i32 = 32;
    /// 8 DSD bits per byte, the oldest in the most significant bit.
    pub const DSD_INT8_MSB1: i32 = 33;

    // asioMessage selectors.
    pub const SELECTOR_SUPPORTED: i32 = 1;
    pub const ENGINE_VERSION: i32 = 2;
    pub const RESET_REQUEST: i32 = 3;
    pub const BUFFER_SIZE_CHANGE: i32 = 4;
    pub const RESYNC_REQUEST: i32 = 5;
    pub const LATENCIES_CHANGED: i32 = 6;
    pub const SUPPORTS_TIME_INFO: i32 = 7;

    // future() selectors for the I/O format, and the formats.
    pub const SET_IO_FORMAT: i32 = 0x2311_1961;
    pub const GET_IO_FORMAT: i32 = 0x2311_1983;
    pub const CAN_DO_IO_FORMAT: i32 = 0x2311_2004;
    pub const PCM_FORMAT: i32 = 0;
    pub const DSD_FORMAT: i32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ChannelInfo {
        pub channel: i32,
        pub is_input: AsioBool,
        pub is_active: AsioBool,
        pub channel_group: i32,
        pub sample_type: i32,
        pub name: [c_char; 32],
    }

    #[repr(C)]
    pub struct BufferInfo {
        pub is_input: AsioBool,
        pub channel_num: i32,
        pub buffers: [*mut c_void; 2],
    }

    #[repr(C)]
    pub struct IoFormat {
        pub format_type: i32,
        pub future: [c_char; 508],
    }

    impl IoFormat {
        pub fn new(format_type: i32) -> IoFormat {
            IoFormat { format_type, future: [0; 508] }
        }
    }

    /// `ASIOSamples` and `ASIOTimeStamp`: a 64-bit value as two halves.
    #[repr(C)]
    pub struct Wide {
        pub hi: u32,
        pub lo: u32,
    }

    #[repr(C)]
    pub struct Callbacks {
        pub buffer_switch: unsafe extern "C" fn(index: i32, direct_process: AsioBool),
        pub sample_rate_did_change: unsafe extern "C" fn(rate: f64),
        pub asio_message: unsafe extern "C" fn(selector: i32, value: i32, message: *mut c_void, opt: *mut f64) -> i32,
        /// Takes and gives back an `ASIOTime`, which is never read here.
        pub buffer_switch_time_info:
            unsafe extern "C" fn(params: *mut c_void, index: i32, direct_process: AsioBool) -> *mut c_void,
    }

    /// The driver object: a COM object whose first field is its vtable.
    #[repr(C)]
    pub struct IAsio {
        pub vtbl: *const IAsioVtbl,
    }

    type This = *mut IAsio;

    /// IUnknown's three methods, then IASIO's, in the order drivers lay them out.
    #[repr(C)]
    pub struct IAsioVtbl {
        pub query_interface: unsafe extern "system" fn(This, *const c_void, *mut *mut c_void) -> i32,
        pub add_ref: unsafe extern "system" fn(This) -> u32,
        pub release: unsafe extern "system" fn(This) -> u32,
        pub init: unsafe extern "system" fn(This, sys_handle: *mut c_void) -> AsioBool,
        pub get_driver_name: unsafe extern "system" fn(This, name: *mut c_char),
        pub get_driver_version: unsafe extern "system" fn(This) -> i32,
        pub get_error_message: unsafe extern "system" fn(This, text: *mut c_char),
        pub start: unsafe extern "system" fn(This) -> AsioError,
        pub stop: unsafe extern "system" fn(This) -> AsioError,
        pub get_channels: unsafe extern "system" fn(This, inputs: *mut i32, outputs: *mut i32) -> AsioError,
        pub get_latencies: unsafe extern "system" fn(This, input: *mut i32, output: *mut i32) -> AsioError,
        pub get_buffer_size: unsafe extern "system" fn(
            This,
            min: *mut i32,
            max: *mut i32,
            preferred: *mut i32,
            granularity: *mut i32,
        ) -> AsioError,
        pub can_sample_rate: unsafe extern "system" fn(This, rate: f64) -> AsioError,
        pub get_sample_rate: unsafe extern "system" fn(This, rate: *mut f64) -> AsioError,
        pub set_sample_rate: unsafe extern "system" fn(This, rate: f64) -> AsioError,
        pub get_clock_sources: unsafe extern "system" fn(This, clocks: *mut c_void, count: *mut i32) -> AsioError,
        pub set_clock_source: unsafe extern "system" fn(This, reference: i32) -> AsioError,
        pub get_sample_position: unsafe extern "system" fn(This, position: *mut Wide, stamp: *mut Wide) -> AsioError,
        pub get_channel_info: unsafe extern "system" fn(This, info: *mut ChannelInfo) -> AsioError,
        pub create_buffers: unsafe extern "system" fn(
            This,
            infos: *mut BufferInfo,
            channels: i32,
            buffer_size: i32,
            callbacks: *const Callbacks,
        ) -> AsioError,
        pub dispose_buffers: unsafe extern "system" fn(This) -> AsioError,
        pub control_panel: unsafe extern "system" fn(This) -> AsioError,
        pub future: unsafe extern "system" fn(This, selector: i32, opt: *mut c_void) -> AsioError,
        pub output_ready: unsafe extern "system" fn(This) -> AsioError,
    }

    /// A C string of at most `N` bytes as text.
    pub fn text<const N: usize>(bytes: &[c_char; N]) -> String {
        let bytes: Vec<u8> = bytes.iter().take_while(|&&b| b != 0).map(|&b| b as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// The engine's samples in a driver's own sample types. PCM comes as
/// left-justified s32 already quantized to the device's depth, so every
/// conversion is exact; DSD as `DsdPacking::NativeU32Be` words, whose 4
/// bytes in memory are the stream's, oldest first, oldest bit in each MSB.
pub mod convert {
    use super::abi::*;

    pub fn is_dsd(sample_type: i32) -> bool {
        sample_type == DSD_INT8_LSB1 || sample_type == DSD_INT8_MSB1
    }

    pub fn is_pcm(sample_type: i32) -> bool {
        matches!(sample_type, 0..=4 | 8..=11 | 16..=20 | 24..=27)
    }

    pub fn is_float(sample_type: i32) -> bool {
        matches!(sample_type, FLOAT32_MSB | FLOAT64_MSB | FLOAT32_LSB | FLOAT64_LSB)
    }

    /// The bits of resolution a PCM type carries (a float: its own width;
    /// the engine treats a float device as 24 bits deep).
    pub fn pcm_bits(sample_type: i32) -> u32 {
        match sample_type {
            INT16_MSB | INT16_LSB | INT32_MSB16 | INT32_LSB16 => 16,
            INT32_MSB18 | INT32_LSB18 => 18,
            INT32_MSB20 | INT32_LSB20 => 20,
            INT24_MSB | INT24_LSB | INT32_MSB24 | INT32_LSB24 => 24,
            FLOAT64_MSB | FLOAT64_LSB => 64,
            _ => 32,
        }
    }

    pub fn bytes_per_sample(sample_type: i32) -> usize {
        match sample_type {
            INT16_MSB | INT16_LSB => 2,
            INT24_MSB | INT24_LSB => 3,
            FLOAT64_MSB | FLOAT64_LSB => 8,
            _ => 4,
        }
    }

    fn put(out: &mut [u8], value: u64, bytes: usize, big_endian: bool) {
        let le = value.to_le_bytes();
        for (i, b) in out[..bytes].iter_mut().enumerate() {
            *b = if big_endian { le[bytes - 1 - i] } else { le[i] };
        }
    }

    /// One channel of interleaved `src` (every `stride`-th sample, starting
    /// at the first) into `dst`, in `sample_type`.
    pub fn write_pcm(sample_type: i32, dst: &mut [u8], src: &[i32], stride: usize) {
        let bytes = bytes_per_sample(sample_type);
        let be = sample_type < INT16_LSB;
        for (out, &s) in dst.chunks_exact_mut(bytes).zip(src.iter().step_by(stride)) {
            let u = s as u32;
            match sample_type {
                INT16_MSB | INT16_LSB => put(out, (u >> 16) as u64, 2, be),
                INT24_MSB | INT24_LSB => put(out, (u >> 8) as u64, 3, be),
                INT32_MSB | INT32_LSB => put(out, u as u64, 4, be),
                INT32_MSB16 | INT32_LSB16 => put(out, (s >> 16) as u32 as u64, 4, be),
                INT32_MSB18 | INT32_LSB18 => put(out, (s >> 14) as u32 as u64, 4, be),
                INT32_MSB20 | INT32_LSB20 => put(out, (s >> 12) as u32 as u64, 4, be),
                INT32_MSB24 | INT32_LSB24 => put(out, (s >> 8) as u32 as u64, 4, be),
                // Exact: s / 2^31 of a value on a grid of at most 24 bits.
                FLOAT32_MSB | FLOAT32_LSB => put(out, (s as f32 / 2_147_483_648.0).to_bits() as u64, 4, be),
                FLOAT64_MSB | FLOAT64_LSB => put(out, (s as f64 / 2_147_483_648.0).to_bits(), 8, be),
                _ => out.fill(0),
            }
        }
    }

    /// One channel of DSD words (every `stride`-th, starting at the first)
    /// into `dst`, 4 bytes per word.
    pub fn write_dsd(sample_type: i32, dst: &mut [u8], src: &[i32], stride: usize) {
        let lsb_first = sample_type == DSD_INT8_LSB1;
        for (out, &w) in dst.chunks_exact_mut(4).zip(src.iter().step_by(stride)) {
            let bytes = w.to_ne_bytes();
            for (o, b) in out.iter_mut().zip(bytes) {
                *o = if lsb_first { b.reverse_bits() } else { b };
            }
        }
    }

    /// The silence of a buffer: zeros, or DSD's idle pattern.
    pub fn silence(sample_type: i32, dst: &mut [u8]) {
        let byte = match sample_type {
            DSD_INT8_MSB1 => 0x69,
            DSD_INT8_LSB1 => 0x69u8.reverse_bits(),
            _ => 0,
        };
        dst.fill(byte);
    }
}

// --- The driver -------------------------------------------------------------------

/// One reference to a driver object, released when dropped. Used on the
/// thread that made it.
pub struct Driver(*mut abi::IAsio);

// SAFETY: a driver is made, used and released on one thread; it only moves
// once, into that thread, before anything is called on it.
unsafe impl Send for Driver {}

impl Driver {
    /// # Safety
    /// `raw` is a live IASIO object, one of whose references this takes.
    pub unsafe fn from_raw(raw: *mut abi::IAsio) -> Driver {
        Driver(raw)
    }

    fn vt(&self) -> &abi::IAsioVtbl {
        // SAFETY: a COM object's first field points at its vtable for as long
        // as it lives, and this reference keeps it alive.
        unsafe { &*(*self.0).vtbl }
    }

    // SAFETY (every call below): the methods of a live driver, with pointers
    // to locals that outlive the call.

    fn init(&self, sys_handle: *mut c_void) -> bool {
        unsafe { (self.vt().init)(self.0, sys_handle) != 0 }
    }
    pub fn name(&self) -> String {
        let mut name = [0; 32];
        unsafe { (self.vt().get_driver_name)(self.0, name.as_mut_ptr()) };
        abi::text(&name)
    }
    pub fn error_message(&self) -> String {
        let mut text = [0; 124];
        unsafe { (self.vt().get_error_message)(self.0, text.as_mut_ptr()) };
        abi::text(&text)
    }
    fn start(&self) -> bool {
        unsafe { (self.vt().start)(self.0) == abi::ASE_OK }
    }
    fn stop(&self) {
        unsafe { (self.vt().stop)(self.0) };
    }
    fn outputs(&self) -> Option<usize> {
        let (mut inputs, mut outputs) = (0, 0);
        (unsafe { (self.vt().get_channels)(self.0, &mut inputs, &mut outputs) } == abi::ASE_OK)
            .then_some(outputs.max(0) as usize)
    }
    fn output_latency(&self) -> usize {
        let (mut input, mut output) = (0, 0);
        unsafe { (self.vt().get_latencies)(self.0, &mut input, &mut output) };
        output.max(0) as usize
    }
    fn preferred_buffer(&self) -> Option<usize> {
        let (mut min, mut max, mut preferred, mut granularity) = (0, 0, 0, 0);
        let ok = unsafe { (self.vt().get_buffer_size)(self.0, &mut min, &mut max, &mut preferred, &mut granularity) };
        (ok == abi::ASE_OK && preferred > 0).then_some(preferred as usize)
    }
    pub fn can_sample_rate(&self, rate: u32) -> bool {
        unsafe { (self.vt().can_sample_rate)(self.0, rate as f64) == abi::ASE_OK }
    }
    fn sample_rate(&self) -> Option<f64> {
        let mut rate = 0.0;
        (unsafe { (self.vt().get_sample_rate)(self.0, &mut rate) } == abi::ASE_OK).then_some(rate)
    }
    fn set_sample_rate(&self, rate: u32) -> bool {
        unsafe { (self.vt().set_sample_rate)(self.0, rate as f64) == abi::ASE_OK }
    }
    fn channel_type(&self, channel: usize) -> Option<i32> {
        let mut info = abi::ChannelInfo {
            channel: channel as i32,
            is_input: 0,
            is_active: 0,
            channel_group: 0,
            sample_type: -1,
            name: [0; 32],
        };
        (unsafe { (self.vt().get_channel_info)(self.0, &mut info) } == abi::ASE_OK).then_some(info.sample_type)
    }
    fn future(&self, selector: i32, opt: *mut c_void) -> abi::AsioError {
        unsafe { (self.vt().future)(self.0, selector, opt) }
    }
    fn io_format(&self, selector: i32, format_type: i32) -> bool {
        let mut format = abi::IoFormat::new(format_type);
        self.future(selector, (&mut format as *mut abi::IoFormat).cast()) == abi::ASE_SUCCESS
    }
    fn create_buffers(&self, infos: &mut [abi::BufferInfo], size: usize) -> bool {
        unsafe {
            (self.vt().create_buffers)(self.0, infos.as_mut_ptr(), infos.len() as i32, size as i32, &CALLBACKS)
                == abi::ASE_OK
        }
    }
    fn dispose_buffers(&self) {
        unsafe { (self.vt().dispose_buffers)(self.0) };
    }
    fn output_ready(&self) -> bool {
        unsafe { (self.vt().output_ready)(self.0) == abi::ASE_OK }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        // SAFETY: gives back the one reference this holds.
        unsafe { (self.vt().release)(self.0) };
    }
}

// --- The callbacks ------------------------------------------------------------------

/// What the driver's thread fills its buffers from while a stream runs.
struct Active {
    render: Arc<dyn Render>,
    driver: *mut abi::IAsio,
    dsd: bool,
    output_ready: bool,
    channels: usize,
    types: Vec<i32>,
    /// Per channel, both halves, and each half's length in bytes.
    buffers: Vec<[*mut u8; 2]>,
    half_bytes: Vec<usize>,
    block: UnsafeCell<Vec<i32>>,
    /// The driver asked to be reset (its settings changed).
    failed: Arc<AtomicBool>,
}

// SAFETY: `block` and the buffers are only touched by the driver's callback
// thread while the stream runs (one buffer switch at a time); the stream is
// torn down only once no callback is inside (`IN_CALLBACK`).
unsafe impl Sync for Active {}
unsafe impl Send for Active {}

/// The stream the callbacks fill (they carry no context of their own).
static ACTIVE: AtomicPtr<Active> = AtomicPtr::new(null_mut());
/// Callbacks inside right now: counted before `ACTIVE` is read, so a stream
/// cleared from `ACTIVE` is freed only once this is back to 0.
static IN_CALLBACK: AtomicI32 = AtomicI32::new(0);

static CALLBACKS: abi::Callbacks = abi::Callbacks {
    buffer_switch: on_buffer_switch,
    sample_rate_did_change: on_sample_rate_did_change,
    asio_message: on_message,
    buffer_switch_time_info: on_buffer_switch_time_info,
};

/// Runs `f` with the active stream, if any, guarded against its teardown.
fn with_active(f: impl FnOnce(&Active)) {
    IN_CALLBACK.fetch_add(1, Ordering::SeqCst);
    let active = ACTIVE.load(Ordering::SeqCst);
    if !active.is_null() {
        // SAFETY: a stream is freed only after it left ACTIVE and every
        // callback counted in IN_CALLBACK is out.
        f(unsafe { &*active });
    }
    IN_CALLBACK.fetch_sub(1, Ordering::SeqCst);
}

unsafe extern "C" fn on_buffer_switch(index: i32, _direct_process: abi::AsioBool) {
    with_active(|a| a.fill(index));
}

unsafe extern "C" fn on_buffer_switch_time_info(
    params: *mut c_void,
    index: i32,
    _direct_process: abi::AsioBool,
) -> *mut c_void {
    with_active(|a| a.fill(index));
    params
}

unsafe extern "C" fn on_sample_rate_did_change(_rate: f64) {}

unsafe extern "C" fn on_message(selector: i32, value: i32, _message: *mut c_void, _opt: *mut f64) -> i32 {
    match selector {
        abi::SELECTOR_SUPPORTED => {
            matches!(value, abi::ENGINE_VERSION | abi::RESET_REQUEST | abi::RESYNC_REQUEST | abi::LATENCIES_CHANGED)
                as i32
        }
        abi::ENGINE_VERSION => 2,
        abi::RESET_REQUEST | abi::BUFFER_SIZE_CHANGE => {
            // The driver's settings changed (buffer, rate, clock): the engine
            // closes the stream and opens it again.
            with_active(|a| a.failed.store(true, Ordering::Release));
            1
        }
        abi::RESYNC_REQUEST | abi::LATENCIES_CHANGED => 1,
        // Plain bufferSwitch is enough.
        abi::SUPPORTS_TIME_INFO => 0,
        _ => 0,
    }
}

impl Active {
    fn fill(&self, index: i32) {
        if index != 0 && index != 1 {
            return;
        }
        // SAFETY: only this callback thread touches the block (see `Active`).
        let block = unsafe { &mut *self.block.get() };
        self.render.render(block);
        for c in 0..self.channels {
            // SAFETY: the driver lent these buffers, of `half_bytes[c]` bytes
            // each, until disposeBuffers, which comes after the last callback.
            let dst = unsafe { std::slice::from_raw_parts_mut(self.buffers[c][index as usize], self.half_bytes[c]) };
            let src = &block[c..];
            if self.dsd {
                convert::write_dsd(self.types[c], dst, src, self.channels);
            } else {
                convert::write_pcm(self.types[c], dst, src, self.channels);
            }
        }
        if self.output_ready {
            // SAFETY: the driver outlives the stream.
            unsafe { ((*(*self.driver).vtbl).output_ready)(self.driver) };
        }
    }
}

// --- A stream -------------------------------------------------------------------------

/// A running driver, on the thread that made it.
struct Stream {
    driver: Driver,
    active: *mut Active,
    dsd_mode: bool,
    format: DeviceFormat,
    latency: u64,
}

impl Stream {
    /// Opens `driver` for `request` and starts it: PCM at the track's rate
    /// (or the one the driver takes closest to it), DoP at exactly the
    /// request's rate in an integer type of 24 bits or more, native DSD in
    /// the driver's DSD mode at the 1-bit rate. Nothing stays open on error.
    fn open(
        driver: Driver,
        name: &str,
        request: Request,
        render: Arc<dyn Render>,
        failed: Arc<AtomicBool>,
    ) -> Result<Stream> {
        let label = format!("ASIO \"{name}\"");
        let fail = |why: String| OutputError(format!("{label}: {why}"));
        if request.channels == 0 || request.rate == 0 {
            return Err(fail("invalid stream".into()));
        }
        if !ACTIVE.load(Ordering::SeqCst).is_null() {
            return Err(fail("another ASIO stream is running".into()));
        }
        let native = request.dsd.is_some_and(|p| p.is_native());
        // PCM or DSD, before the rate (a DSD rate means nothing in PCM mode).
        if native {
            if !driver.io_format(abi::CAN_DO_IO_FORMAT, abi::DSD_FORMAT)
                || !driver.io_format(abi::SET_IO_FORMAT, abi::DSD_FORMAT)
            {
                return Err(fail("no native DSD mode".into()));
            }
        } else {
            // Drivers without DSD do not know the call: fine.
            driver.io_format(abi::SET_IO_FORMAT, abi::PCM_FORMAT);
        }
        let back_to_pcm = |driver: &Driver| {
            if native {
                driver.io_format(abi::SET_IO_FORMAT, abi::PCM_FORMAT);
            }
        };
        let result = Self::configure(&driver, &label, request, native);
        let (channels, mono_to_stereo, rate, types, size) = match result {
            Ok(found) => found,
            Err(e) => {
                back_to_pcm(&driver);
                return Err(e);
            }
        };

        // DSD: the driver counts bits; the engine, 32-bit words.
        let per_word = if native { 32 } else { 1 };
        let frames = size / per_word;
        let half_bytes: Vec<usize> =
            types.iter().map(|&t| if native { size / 8 } else { size * convert::bytes_per_sample(t) }).collect();
        let mut infos: Vec<abi::BufferInfo> = (0..channels)
            .map(|c| abi::BufferInfo { is_input: 0, channel_num: c as i32, buffers: [null_mut(); 2] })
            .collect();
        if !driver.create_buffers(&mut infos, size) {
            back_to_pcm(&driver);
            return Err(fail("could not create its buffers".into()));
        }
        let buffers: Vec<[*mut u8; 2]> = infos.iter().map(|i| [i.buffers[0].cast(), i.buffers[1].cast()]).collect();
        if buffers.iter().flatten().any(|b| b.is_null()) {
            driver.dispose_buffers();
            back_to_pcm(&driver);
            return Err(fail("gave no buffers".into()));
        }
        // Both halves silent until the engine fills them.
        for (c, halves) in buffers.iter().enumerate() {
            for &b in halves {
                // SAFETY: the driver lent `half_bytes[c]` bytes at `b`.
                convert::silence(types[c], unsafe { std::slice::from_raw_parts_mut(b, half_bytes[c]) });
            }
        }
        let output_ready = driver.output_ready();
        let latency = (2 * frames + driver.output_latency() / per_word) as u64;
        let (bits, float) = if native {
            (32, false)
        } else {
            let bits = types.iter().map(|&t| convert::pcm_bits(t)).min().unwrap_or(32).min(32);
            (bits, types.iter().any(|&t| convert::is_float(t)))
        };
        let format = DeviceFormat {
            rate: if native { rate / 32 } else { rate },
            channels,
            bits,
            float,
            mono_to_stereo,
            exclusive: true,
            period_frames: frames,
            buffer_frames: 2 * frames,
            dsd: request.dsd.map(|p| if p.is_native() { DsdPacking::NativeU32Be } else { p }),
        };
        let active = Box::into_raw(Box::new(Active {
            render,
            driver: driver.0,
            dsd: native,
            output_ready,
            channels,
            types,
            buffers,
            half_bytes,
            block: UnsafeCell::new(vec![0; frames * channels]),
            failed,
        }));
        ACTIVE.store(active, Ordering::SeqCst);
        let stream = Stream { driver, active, dsd_mode: native, format, latency };
        if !stream.driver.start() {
            return Err(fail("did not start".into())); // dropping the stream undoes it all
        }
        Ok(stream)
    }

    /// The channels, whether a mono track goes out in stereo, the driver's
    /// rate (DSD: the 1-bit rate), each channel's sample type and the buffer
    /// size the driver counts in.
    fn configure(
        driver: &Driver,
        label: &str,
        request: Request,
        native: bool,
    ) -> Result<(usize, bool, u32, Vec<i32>, usize)> {
        let fail = |why: String| OutputError(format!("{label}: {why}"));
        let outputs = driver.outputs().ok_or_else(|| fail("no channel count".into()))?;
        // A mono track on a stereo DAC plays on both sides.
        let (channels, mono_to_stereo) =
            if request.channels == 1 && outputs >= 2 { (2, true) } else { (request.channels, false) };
        if outputs < channels {
            return Err(fail(format!("{channels} output channels needed, the driver has {outputs}")));
        }
        let rate = if native {
            let bit_rate = request.rate.saturating_mul(32);
            if !driver.can_sample_rate(bit_rate) {
                return Err(fail(format!("no DSD at {bit_rate} Hz")));
            }
            bit_rate
        } else if driver.can_sample_rate(request.rate) {
            request.rate
        } else if request.dsd.is_some() {
            // DoP only at its own rate: anything else is another stream.
            return Err(fail(format!("DoP at {} Hz not supported", request.rate)));
        } else {
            let rates: Vec<u32> = STANDARD_RATES.iter().copied().filter(|&r| driver.can_sample_rate(r)).collect();
            pick_rate(rates, request.rate).ok_or_else(|| fail(format!("does not take {} Hz", request.rate)))?
        };
        if driver.sample_rate() != Some(rate as f64) && !driver.set_sample_rate(rate) {
            return Err(fail(format!("could not switch to {rate} Hz")));
        }
        let mut types = Vec::with_capacity(channels);
        for c in 0..channels {
            let t = driver.channel_type(c).ok_or_else(|| fail("no channel information".into()))?;
            let usable = if native {
                convert::is_dsd(t)
            } else if request.dsd.is_some() {
                // DoP: 24 integer bits or more carry the marker and the data.
                convert::is_pcm(t) && !convert::is_float(t) && convert::pcm_bits(t) >= 24
            } else {
                convert::is_pcm(t)
            };
            if !usable {
                return Err(fail(format!("sample type {t} is not supported")));
            }
            types.push(t);
        }
        let size = driver.preferred_buffer().ok_or_else(|| fail("no buffer size".into()))?;
        if native && size % 32 != 0 {
            return Err(fail(format!("a DSD buffer of {size} bits is not a whole number of 32-bit words")));
        }
        Ok((channels, mono_to_stereo, rate, types, size))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Out of ACTIVE first; then the driver stops, and the stream is
        // freed once no callback is inside.
        ACTIVE.store(null_mut(), Ordering::SeqCst);
        self.driver.stop();
        while IN_CALLBACK.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_micros(200));
        }
        self.driver.dispose_buffers();
        if self.dsd_mode {
            // Back to PCM for whoever opens it next.
            self.driver.io_format(abi::SET_IO_FORMAT, abi::PCM_FORMAT);
        }
        // SAFETY: made by Box::into_raw in open, out of ACTIVE, no callback inside.
        drop(unsafe { Box::from_raw(self.active) });
    }
}

// --- The output -------------------------------------------------------------------------

/// An ASIO driver playing, on a thread of its own that made it and releases
/// it when this is dropped.
pub struct AsioOutput {
    key: String,
    format: DeviceFormat,
    latency: u64,
    failed: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AsioOutput {
    /// Opens the driver installed as `name`.
    #[cfg(windows)]
    pub fn open(key: &str, name: &str, request: Request, render: Arc<dyn Render>) -> Result<Self> {
        let owned = name.to_owned();
        Self::open_with(key, name, request, render, move || windows_driver::create(&owned))
    }

    /// Opens the driver `create` makes, on the output's thread.
    pub fn open_with(
        key: &str,
        name: &str,
        request: Request,
        render: Arc<dyn Render>,
        create: impl FnOnce() -> Result<Driver> + Send + 'static,
    ) -> Result<Self> {
        let failed = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (thread_failed, name) = (failed.clone(), name.to_owned());
        let thread = std::thread::Builder::new()
            .name("sonkore-asio".into())
            .spawn(move || {
                #[cfg(windows)]
                let _com = windows_driver::Sta::init(); // dropped last, after the driver
                match create().and_then(|d| Stream::open(d, &name, request, render, thread_failed)) {
                    Ok(stream) => {
                        let _ = opened_tx.send(Ok((stream.format, stream.latency)));
                        wait_for_stop(&stop_rx);
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = opened_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| OutputError(e.to_string()))?;
        match opened.recv() {
            Ok(Ok((format, latency))) => Ok(AsioOutput {
                key: key.to_owned(),
                format,
                latency,
                failed,
                stop: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(OutputError(format!("{key}: the ASIO thread ended")))
            }
        }
    }
}

/// Until the output is dropped. On Windows the thread's window messages are
/// handled meanwhile: some drivers post them to the thread that made them.
fn wait_for_stop(stop: &mpsc::Receiver<()>) {
    #[cfg(windows)]
    loop {
        match stop.try_recv() {
            Err(mpsc::TryRecvError::Empty) => windows_driver::pump_messages(50),
            _ => return,
        }
    }
    #[cfg(not(windows))]
    let _ = stop.recv();
}

impl Output for AsioOutput {
    fn format(&self) -> DeviceFormat {
        self.format
    }
    fn latency_frames(&self) -> u64 {
        self.latency
    }
    fn key(&self) -> &str {
        &self.key
    }
    fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

impl Drop for AsioOutput {
    fn drop(&mut self) {
        drop(self.stop.take()); // the thread's receiver sees the sender gone
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// --- Windows: the installed drivers ------------------------------------------------------

/// Every installed ASIO driver, as an output: `asio|<name>`.
#[cfg(windows)]
pub fn devices() -> Vec<DeviceInfo> {
    windows_driver::names()
        .into_iter()
        .map(|name| DeviceInfo {
            key: format!("asio|{name}"),
            backend: "ASIO".into(),
            name,
            detail: String::new(),
            is_default: false,
            direct: true,
            alsa_card: -1,
        })
        .collect()
}

#[cfg(windows)]
mod windows_driver {
    use super::*;
    use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CLSIDFromString, COINIT_APARTMENTTHREADED, CoCreateInstanceEx, CoInitializeEx,
        CoUninitialize, MULTI_QI,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
        RegQueryValueExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetDesktopWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
        TranslateMessage,
    };
    use windows::core::{GUID, HSTRING, Interface, PCWSTR, PWSTR};

    const ROOT: &str = "SOFTWARE\\ASIO";

    /// COM on this thread, single-threaded as ASIO drivers expect.
    pub struct Sta(bool);

    impl Sta {
        pub fn init() -> Sta {
            // SAFETY: plain initialisation of this thread, undone in drop.
            Sta(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
        }
    }

    impl Drop for Sta {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: matches the successful CoInitializeEx of this thread.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// Handles this thread's window messages for up to `ms`.
    pub fn pump_messages(ms: u32) {
        // SAFETY: plain Win32 calls with a local message.
        unsafe {
            MsgWaitForMultipleObjects(None, false, ms, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    struct Key(HKEY);

    impl Key {
        fn open(path: &str) -> Option<Key> {
            let mut key = HKEY::default();
            // SAFETY: a local out value; closed in drop.
            let status =
                unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, &HSTRING::from(path), Some(0), KEY_READ, &mut key) };
            (status == ERROR_SUCCESS).then_some(Key(key))
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: a key this opened.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    /// The class id a driver registered under `HKLM\SOFTWARE\ASIO\<name>\CLSID`.
    fn clsid(name: &str) -> Option<GUID> {
        let key = Key::open(&format!("{ROOT}\\{name}"))?;
        let mut value = [0u16; 128];
        let mut size = (value.len() * 2 - 2) as u32;
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY: `value` holds `size` bytes, one character short of full, so
        // the text stays NUL-terminated.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                &HSTRING::from("CLSID"),
                None,
                Some(&mut kind),
                Some(value.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS || kind != REG_SZ {
            return None;
        }
        // SAFETY: a NUL-terminated string in a local buffer.
        unsafe { CLSIDFromString(PCWSTR(value.as_ptr())) }.ok()
    }

    /// The installed drivers' names, sorted.
    pub fn names() -> Vec<String> {
        let Some(root) = Key::open(ROOT) else { return Vec::new() };
        let mut names = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut length = 255u32;
            // SAFETY: `name` holds `length` characters and the NUL.
            let status = unsafe {
                RegEnumKeyExW(root.0, i, Some(PWSTR(name.as_mut_ptr())), &mut length, None, None, None, None)
            };
            if status != ERROR_SUCCESS {
                break;
            }
            let name = String::from_utf16_lossy(&name[..length as usize]);
            if clsid(&name).is_some() {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    /// The driver installed as `name`, loaded and initialised.
    pub fn create(name: &str) -> Result<Driver> {
        let label = format!("ASIO \"{name}\"");
        let clsid = clsid(name).ok_or_else(|| OutputError(format!("{label}: the driver is not installed")))?;
        // ASIO drivers answer to their class id as interface id.
        let mut wanted = [MULTI_QI { pIID: &clsid, pItf: std::mem::ManuallyDrop::new(None), hr: Default::default() }];
        // SAFETY: `wanted` outlives the call, which fills its interface.
        let made = unsafe { CoCreateInstanceEx(&clsid, None, CLSCTX_INPROC_SERVER, None, &mut wanted) };
        let object = unsafe { std::mem::ManuallyDrop::take(&mut wanted[0].pItf) };
        let object = match (made, wanted[0].hr.is_ok(), object) {
            (Ok(()), true, Some(object)) => object,
            _ => return Err(OutputError(format!("{label}: the driver could not be loaded"))),
        };
        // SAFETY: the object answered to IASIO's id; its one reference goes
        // to the Driver.
        let driver = unsafe { Driver::from_raw(object.into_raw().cast()) };
        // SAFETY: a plain call; the desktop window is the usual parent for a
        // driver's own dialogs.
        let window: HWND = unsafe { GetDesktopWindow() };
        if !driver.init(window.0) {
            let message = driver.error_message();
            return Err(OutputError(if message.is_empty() {
                format!("{label}: the driver did not start")
            } else {
                format!("{label}: the driver did not start: {message}")
            }));
        }
        Ok(driver)
    }
}

// --- Tests: a fake driver -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::abi::*;
    use super::*;
    use std::ffi::c_char;

    /// What the fake driver offers and what it was asked.
    #[derive(Default)]
    struct State {
        outputs: i32,
        rates: Vec<f64>,
        rate: f64,
        sample_type: i32,
        dsd_type: i32,
        can_dsd: bool,
        dsd_mode: bool,
        buffer: i32,
        latency: i32,
        buffers: Vec<[Vec<u8>; 2]>,
        /// The callbacks' address (a number: the state crosses threads).
        callbacks: Option<usize>,
        started: bool,
        disposed: bool,
        released: bool,
    }

    #[repr(C)]
    struct Fake {
        vtbl: *const IAsioVtbl,
        state: Arc<Mutex<State>>,
    }

    // SAFETY (all below): `this` is the Fake the test made.
    unsafe fn st(this: *mut IAsio) -> std::sync::MutexGuard<'static, State> {
        let fake = unsafe { &*(this as *mut Fake) };
        // The Fake outlives every call (it is leaked).
        let state: &'static Mutex<State> = unsafe { &*Arc::as_ptr(&fake.state) };
        state.lock().unwrap()
    }
    unsafe extern "system" fn qi(_: *mut IAsio, _: *const c_void, _: *mut *mut c_void) -> i32 {
        -1
    }
    unsafe extern "system" fn add_ref(_: *mut IAsio) -> u32 {
        2
    }
    unsafe extern "system" fn release(this: *mut IAsio) -> u32 {
        unsafe { st(this) }.released = true;
        0
    }
    unsafe extern "system" fn init(_: *mut IAsio, _: *mut c_void) -> AsioBool {
        1
    }
    unsafe extern "system" fn name(_: *mut IAsio, out: *mut c_char) {
        for (i, b) in b"Fake\0".iter().enumerate() {
            unsafe { *out.add(i) = *b as c_char };
        }
    }
    unsafe extern "system" fn version(_: *mut IAsio) -> i32 {
        1
    }
    unsafe extern "system" fn error_message(_: *mut IAsio, out: *mut c_char) {
        unsafe { *out = 0 };
    }
    unsafe extern "system" fn start(this: *mut IAsio) -> AsioError {
        unsafe { st(this) }.started = true;
        ASE_OK
    }
    unsafe extern "system" fn stop(this: *mut IAsio) -> AsioError {
        unsafe { st(this) }.started = false;
        ASE_OK
    }
    unsafe extern "system" fn channels(this: *mut IAsio, i: *mut i32, o: *mut i32) -> AsioError {
        unsafe {
            *i = 0;
            *o = st(this).outputs;
        }
        ASE_OK
    }
    unsafe extern "system" fn latencies(this: *mut IAsio, i: *mut i32, o: *mut i32) -> AsioError {
        unsafe {
            *i = 0;
            *o = st(this).latency;
        }
        ASE_OK
    }
    unsafe extern "system" fn buffer_size(
        this: *mut IAsio,
        a: *mut i32,
        b: *mut i32,
        p: *mut i32,
        g: *mut i32,
    ) -> AsioError {
        let size = unsafe { st(this) }.buffer;
        unsafe {
            *a = size;
            *b = size;
            *p = size;
            *g = 0;
        }
        ASE_OK
    }
    unsafe extern "system" fn can_rate(this: *mut IAsio, rate: f64) -> AsioError {
        let s = unsafe { st(this) };
        let dsd = s.dsd_mode && [2_822_400.0, 5_644_800.0].contains(&rate);
        if dsd || (!s.dsd_mode && s.rates.contains(&rate)) { ASE_OK } else { -1 }
    }
    unsafe extern "system" fn get_rate(this: *mut IAsio, rate: *mut f64) -> AsioError {
        unsafe { *rate = st(this).rate };
        ASE_OK
    }
    unsafe extern "system" fn set_rate(this: *mut IAsio, rate: f64) -> AsioError {
        unsafe { st(this) }.rate = rate;
        ASE_OK
    }
    unsafe extern "system" fn clocks(_: *mut IAsio, _: *mut c_void, _: *mut i32) -> AsioError {
        -1
    }
    unsafe extern "system" fn set_clock(_: *mut IAsio, _: i32) -> AsioError {
        -1
    }
    unsafe extern "system" fn position(_: *mut IAsio, _: *mut Wide, _: *mut Wide) -> AsioError {
        -1
    }
    unsafe extern "system" fn channel_info(this: *mut IAsio, info: *mut ChannelInfo) -> AsioError {
        let s = unsafe { st(this) };
        unsafe { (*info).sample_type = if s.dsd_mode { s.dsd_type } else { s.sample_type } };
        ASE_OK
    }
    unsafe extern "system" fn create(
        this: *mut IAsio,
        infos: *mut BufferInfo,
        n: i32,
        size: i32,
        callbacks: *const Callbacks,
    ) -> AsioError {
        let mut s = unsafe { st(this) };
        let bytes =
            if s.dsd_mode { size as usize / 8 } else { size as usize * convert::bytes_per_sample(s.sample_type) };
        s.buffers = (0..n).map(|_| [vec![0xAB; bytes], vec![0xAB; bytes]]).collect();
        for c in 0..n as usize {
            let info = unsafe { &mut *infos.add(c) };
            info.buffers = [s.buffers[c][0].as_mut_ptr().cast(), s.buffers[c][1].as_mut_ptr().cast()];
        }
        s.callbacks = Some(callbacks as usize);
        ASE_OK
    }
    unsafe extern "system" fn dispose(this: *mut IAsio) -> AsioError {
        unsafe { st(this) }.disposed = true;
        ASE_OK
    }
    unsafe extern "system" fn panel(_: *mut IAsio) -> AsioError {
        -1
    }
    unsafe extern "system" fn future(this: *mut IAsio, selector: i32, opt: *mut c_void) -> AsioError {
        let mut s = unsafe { st(this) };
        let format = unsafe { (*(opt as *mut IoFormat)).format_type };
        match selector {
            CAN_DO_IO_FORMAT if format == DSD_FORMAT && s.can_dsd => ASE_SUCCESS,
            SET_IO_FORMAT if format == DSD_FORMAT && s.can_dsd => {
                s.dsd_mode = true;
                ASE_SUCCESS
            }
            SET_IO_FORMAT if format == PCM_FORMAT => {
                s.dsd_mode = false;
                ASE_SUCCESS
            }
            _ => -1000,
        }
    }
    unsafe extern "system" fn ready(_: *mut IAsio) -> AsioError {
        ASE_OK
    }

    static VTBL: IAsioVtbl = IAsioVtbl {
        query_interface: qi,
        add_ref,
        release,
        init,
        get_driver_name: name,
        get_driver_version: version,
        get_error_message: error_message,
        start,
        stop,
        get_channels: channels,
        get_latencies: latencies,
        get_buffer_size: buffer_size,
        can_sample_rate: can_rate,
        get_sample_rate: get_rate,
        set_sample_rate: set_rate,
        get_clock_sources: clocks,
        set_clock_source: set_clock,
        get_sample_position: position,
        get_channel_info: channel_info,
        create_buffers: create,
        dispose_buffers: dispose,
        control_panel: panel,
        future,
        output_ready: ready,
    };

    /// The tests share one ASIO slot (as the callbacks do): one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn fake(state: State) -> (Arc<Mutex<State>>, impl FnOnce() -> Result<Driver> + Send + 'static) {
        let state = Arc::new(Mutex::new(state));
        let raw = Box::into_raw(Box::new(Fake { vtbl: &VTBL, state: state.clone() })) as usize;
        // SAFETY: a leaked Fake, alive for the whole test.
        (state, move || Ok(unsafe { Driver::from_raw(raw as *mut IAsio) }))
    }

    fn stereo_pcm() -> State {
        State {
            outputs: 2,
            rates: vec![44_100.0, 48_000.0, 88_200.0, 96_000.0],
            rate: 48_000.0,
            sample_type: INT24_LSB,
            dsd_type: DSD_INT8_MSB1,
            buffer: 4,
            latency: 10,
            ..Default::default()
        }
    }

    /// Renders 1, 2, 3… shifted to the top of each sample (left-justified),
    /// or a fixed word for DSD.
    struct Counter(Mutex<i32>, bool);
    impl Render for Counter {
        fn render(&self, out: &mut [i32]) {
            let mut n = self.0.lock().unwrap();
            for s in out {
                *n += 1;
                *s = if self.1 { i32::from_ne_bytes([0x12, 0x34, 0x56, 0x78]) } else { *n << 8 };
            }
        }
    }

    fn request(rate: u32, channels: usize, dsd: Option<DsdPacking>) -> Request {
        Request { rate, channels, follow_rate: false, exclusive: true, dsd }
    }

    /// The fake opened for `request`, fed by a Counter (DSD words when native).
    fn open(make: impl FnOnce() -> Result<Driver> + Send + 'static, request: Request) -> Result<AsioOutput> {
        let words = request.dsd.is_some_and(|p| p.is_native());
        AsioOutput::open_with("asio|Fake", "Fake", request, Arc::new(Counter(Mutex::new(0), words)), make)
    }

    fn switch(state: &Arc<Mutex<State>>, index: i32) {
        let callbacks = state.lock().unwrap().callbacks.unwrap() as *const Callbacks;
        // SAFETY: the static callbacks the stream gave the fake.
        unsafe { ((*callbacks).buffer_switch)(index, 1) };
    }

    #[test]
    fn the_abi_has_windows_layout() {
        assert_eq!(std::mem::size_of::<ChannelInfo>(), 52);
        assert_eq!(std::mem::size_of::<BufferInfo>(), 24);
        assert_eq!(std::mem::size_of::<IoFormat>(), 512);
        assert_eq!(std::mem::size_of::<IAsioVtbl>(), 24 * std::mem::size_of::<usize>());
    }

    #[test]
    fn pcm_types_take_the_samples_exactly() {
        let s = [0x1234_5600i32, -0x0000_0100];
        let mut out = [0u8; 6];
        convert::write_pcm(INT24_LSB, &mut out, &s, 1);
        assert_eq!(out, [0x56, 0x34, 0x12, 0xFF, 0xFF, 0xFF]);
        convert::write_pcm(INT24_MSB, &mut out, &s, 1);
        assert_eq!(out, [0x12, 0x34, 0x56, 0xFF, 0xFF, 0xFF]);
        let mut out = [0u8; 4];
        convert::write_pcm(INT16_LSB, &mut out, &[0x1234_0000, 0x7FFF_0000], 1);
        assert_eq!(out, [0x34, 0x12, 0xFF, 0x7F]);
        let mut out = [0u8; 4];
        convert::write_pcm(INT32_LSB24, &mut out, &[0x1234_5600], 1);
        assert_eq!(out, [0x56, 0x34, 0x12, 0x00]);
        convert::write_pcm(INT32_LSB24, &mut out, &[-0x100], 1);
        assert_eq!(out, [0xFF; 4]);
        convert::write_pcm(FLOAT32_LSB, &mut out, &[0x4000_0000], 1);
        assert_eq!(f32::from_le_bytes(out), 0.5);
        let mut out = [0u8; 8];
        convert::write_pcm(FLOAT64_MSB, &mut out, &[i32::MIN], 1);
        assert_eq!(f64::from_be_bytes(out), -1.0);
        // Every other sample: one channel of two.
        let mut out = [0u8; 4];
        convert::write_pcm(INT16_MSB, &mut out, &[0x0102_0000, 0x7777_0000, 0x0304_0000, 0x7777_0000], 2);
        assert_eq!(out, [0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn dsd_words_keep_their_bit_order() {
        let w = i32::from_ne_bytes([0x80, 0x01, 0x69, 0xF0]);
        let mut out = [0u8; 4];
        convert::write_dsd(DSD_INT8_MSB1, &mut out, &[w], 1);
        assert_eq!(out, [0x80, 0x01, 0x69, 0xF0]);
        convert::write_dsd(DSD_INT8_LSB1, &mut out, &[w], 1);
        assert_eq!(out, [0x01, 0x80, 0x96, 0x0F]);
        convert::silence(DSD_INT8_LSB1, &mut out);
        assert_eq!(out, [0x96; 4]);
    }

    #[test]
    fn pcm_plays_at_the_tracks_rate_and_fills_both_halves() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (state, make) = fake(stereo_pcm());
        let out = open(make, request(44_100, 2, None)).unwrap();
        let f = out.format();
        assert_eq!((f.rate, f.channels, f.bits, f.float, f.exclusive, f.dsd), (44_100, 2, 24, false, true, None));
        assert_eq!((f.period_frames, f.buffer_frames, out.latency_frames()), (4, 8, 18));
        assert_eq!(state.lock().unwrap().rate, 44_100.0);
        assert!(state.lock().unwrap().started);
        // Silent until the first switch.
        assert_eq!(state.lock().unwrap().buffers[0][1], [0; 12]);
        switch(&state, 1);
        {
            let s = state.lock().unwrap();
            // Left takes 1, 3, 5, 7; right 2, 4, 6, 8: 3-byte little endian.
            assert_eq!(s.buffers[0][1], [1, 0, 0, 3, 0, 0, 5, 0, 0, 7, 0, 0]);
            assert_eq!(s.buffers[1][1], [2, 0, 0, 4, 0, 0, 6, 0, 0, 8, 0, 0]);
            assert_eq!(s.buffers[0][0], [0; 12]);
        }
        drop(out);
        let s = state.lock().unwrap();
        assert!(!s.started && s.disposed && s.released);
        assert!(ACTIVE.load(Ordering::SeqCst).is_null());
    }

    #[test]
    fn a_rate_the_driver_refuses_becomes_its_closest_multiple() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stereo_pcm();
        s.rates = vec![48_000.0, 88_200.0, 96_000.0];
        let (state, make) = fake(s);
        let out = open(make, request(44_100, 2, None)).unwrap();
        assert_eq!(out.format().rate, 88_200);
        assert_eq!(state.lock().unwrap().rate, 88_200.0);
    }

    #[test]
    fn mono_goes_out_on_both_sides() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_state, make) = fake(stereo_pcm());
        let out = open(make, request(48_000, 1, None)).unwrap();
        assert_eq!((out.format().channels, out.format().mono_to_stereo), (2, true));
    }

    #[test]
    fn native_dsd_runs_in_the_drivers_dsd_mode() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stereo_pcm();
        s.can_dsd = true;
        s.buffer = 64; // bits: two 32-bit words
        s.latency = 32;
        let (state, make) = fake(s);
        let out = open(make, request(88_200, 2, Some(DsdPacking::NativeU32Be))).unwrap();
        let f = out.format();
        assert_eq!((f.rate, f.bits, f.dsd), (88_200, 32, Some(DsdPacking::NativeU32Be)));
        assert_eq!((f.period_frames, out.latency_frames()), (2, 5));
        assert_eq!(state.lock().unwrap().rate, 2_822_400.0);
        assert_eq!(state.lock().unwrap().buffers[0][0], [0x69; 8]); // idle pattern first
        switch(&state, 0);
        assert_eq!(state.lock().unwrap().buffers[1][0], [0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78]);
        drop(out);
        assert!(!state.lock().unwrap().dsd_mode, "back to PCM for the next one");
    }

    #[test]
    fn a_driver_without_dsd_mode_refuses_native_dsd() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (state, make) = fake(stereo_pcm());
        let e = open(make, request(88_200, 2, Some(DsdPacking::NativeU32Be))).err().unwrap();
        assert!(e.0.contains("no native DSD mode"), "{e}");
        assert!(state.lock().unwrap().released);
        assert!(ACTIVE.load(Ordering::SeqCst).is_null());
    }

    #[test]
    fn dop_needs_its_own_rate_and_24_integer_bits() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_s, make) = fake(stereo_pcm());
        let out = open(make, request(176_400, 2, Some(DsdPacking::Dop)));
        assert!(out.err().unwrap().0.contains("DoP at 176400 Hz"));
        let mut s = stereo_pcm();
        s.sample_type = FLOAT32_LSB;
        let (_s, make) = fake(s);
        let out = open(make, request(88_200, 2, Some(DsdPacking::Dop)));
        assert!(out.err().unwrap().0.contains("sample type 19"));
        let (_s, make) = fake(stereo_pcm());
        let out = open(make, request(88_200, 2, Some(DsdPacking::Dop))).unwrap();
        assert_eq!(out.format().dsd, Some(DsdPacking::Dop));
    }

    #[test]
    fn a_reset_request_fails_the_output() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (state, make) = fake(stereo_pcm());
        let out = open(make, request(48_000, 2, None)).unwrap();
        assert!(!out.failed());
        let callbacks = state.lock().unwrap().callbacks.unwrap() as *const Callbacks;
        // SAFETY: the static callbacks the stream gave the fake.
        unsafe {
            assert_eq!(((*callbacks).asio_message)(SELECTOR_SUPPORTED, RESET_REQUEST, null_mut(), null_mut()), 1);
            assert_eq!(((*callbacks).asio_message)(RESET_REQUEST, 0, null_mut(), null_mut()), 1);
        }
        assert!(out.failed());
    }

    #[test]
    fn too_few_channels_leave_nothing_open() {
        let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (state, make) = fake(stereo_pcm());
        let e = open(make, request(48_000, 6, None)).err().unwrap();
        assert!(e.0.contains("6 output channels needed, the driver has 2"), "{e}");
        let s = state.lock().unwrap();
        assert!(s.released && !s.started && s.callbacks.is_none());
    }
}
