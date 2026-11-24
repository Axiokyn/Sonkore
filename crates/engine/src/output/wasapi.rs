//! WASAPI output (Windows).
//!
//! Shared mode goes through the Windows mixer: the stream runs at the mix
//! format's rate in 32-bit float and the engine resamples to it (WASAPI's own
//! SRC stays off for the same reason). Exclusive mode talks to
//! the endpoint directly in an integer format, bit-perfect when the rate
//! matches, and carries DoP. Both are event driven on a thread of their own;
//! everything COM is made and used on that thread, so no interface crosses
//! an apartment.

use super::*;
use std::sync::mpsc;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_SERVICE_NOT_RUNNING,
    AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE, IAudioClient,
    IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToBSTR};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Error as WinError, HRESULT, PCWSTR};

/// Shared mode's buffer: 120 ms, 3 periods of 40 ms.
const SHARED_BUFFER_HNS: i64 = 1_200_000;
/// How long the device thread waits for an event before looking at `stop`.
const WAIT_MS: u32 = 100;

// --- Devices -------------------------------------------------------------------

/// Every active render endpoint, keyed by its friendly name as earlier
/// versions keyed them.
pub fn devices() -> Vec<DeviceInfo> {
    let _com = Com::init(); // dropped last, after every interface below
    let Ok(enumerator) = enumerator() else { return Vec::new() };
    // SAFETY: a plain COM call on a live enumerator.
    let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.ok().and_then(|d| device_id(&d));
    let list = endpoints(&enumerator);
    let keys = endpoint_keys(list.iter().map(|e| (e.id.as_str(), e.name.as_str())));
    list.iter()
        .zip(keys)
        .map(|(e, key)| DeviceInfo {
            key,
            backend: "WASAPI".into(),
            name: e.name.clone(),
            detail: String::new(),
            is_default: default_id.as_deref() == Some(e.id.as_str()),
            direct: false,
            alsa_card: -1,
        })
        .collect()
}

/// Each endpoint's key: "wasapi|<name>"; a second endpoint of the same name
/// is keyed by its id instead, so it stays reachable (`lookup` finds both).
fn endpoint_keys<'a>(endpoints: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (id, name) in endpoints {
        let by_name = format!("wasapi|{name}");
        keys.push(if keys.contains(&by_name) { format!("wasapi|{id}") } else { by_name });
    }
    keys
}

/// The endpoint a key's name stands for: the first of that name, else the
/// one whose id it is.
fn lookup<'a>(endpoints: impl Iterator<Item = (&'a str, &'a str)> + Clone, wanted: &str) -> Option<usize> {
    endpoints
        .clone()
        .position(|(_, name)| name == wanted)
        .or_else(|| endpoints.clone().position(|(id, _)| id == wanted))
}

struct Endpoint {
    id: String,
    name: String,
    device: IMMDevice,
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: COM is initialised on this thread (Com::init).
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// The active render endpoints, in Windows' order.
fn endpoints(enumerator: &IMMDeviceEnumerator) -> Vec<Endpoint> {
    // SAFETY: plain COM calls; the collection outlives the loop.
    let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }) else {
        return Vec::new();
    };
    let count = unsafe { collection.GetCount() }.unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            // SAFETY: i < GetCount().
            let device = unsafe { collection.Item(i) }.ok()?;
            let id = device_id(&device)?;
            let name = friendly_name(&device).unwrap_or_else(|| id.clone());
            Some(Endpoint { id, name, device })
        })
        .collect()
}

fn device_id(device: &IMMDevice) -> Option<String> {
    // SAFETY: GetId hands over a CoTaskMem string, read once and freed here.
    unsafe {
        let id = device.GetId().ok()?;
        let text = id.to_string().ok();
        CoTaskMemFree(Some(id.0 as *const _));
        text
    }
}

fn friendly_name(device: &IMMDevice) -> Option<String> {
    // SAFETY: the PROPVARIANT is ours and cleared after it is read.
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let mut value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let name = PropVariantToBSTR(&value).ok().map(|b| b.to_string());
        let _ = PropVariantClear(&mut value);
        name.filter(|n| !n.is_empty())
    }
}

/// COM for the current thread: multithreaded, unless the thread already
/// chose a single-threaded apartment (a UI thread), which works as well.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: balanced by CoUninitialize in drop when it succeeded
        // (S_OK or S_FALSE); RPC_E_CHANGED_MODE leaves nothing to undo.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: pairs the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

/// An auto-reset event, closed with the stream.
struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: our own handle, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// --- Formats -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sample {
    F32, // shared mode: the mixer's format
    S32,
    S24In32,   // 24 valid bits in the low three bytes of four
    S24Packed, // three bytes
    S16,
}

impl Sample {
    fn bytes(self) -> usize {
        match self {
            Sample::S24Packed => 3,
            Sample::S16 => 2,
            _ => 4,
        }
    }

    fn valid_bits(self) -> u32 {
        match self {
            Sample::F32 | Sample::S32 => 32,
            Sample::S24In32 | Sample::S24Packed => 24,
            Sample::S16 => 16,
        }
    }
}

/// The integer formats exclusive mode tries, best first (ALSA's order).
const EXCLUSIVE_SAMPLES: [Sample; 4] = [Sample::S32, Sample::S24In32, Sample::S24Packed, Sample::S16];

/// The speaker positions of the engine's channel order, which is also
/// WAVEFORMATEXTENSIBLE's bit order.
fn channel_mask(channels: usize) -> u32 {
    match channels {
        1 => 0x4,   // front center
        2 => 0x3,   // FL FR
        3 => 0x7,   // FL FR FC
        4 => 0x33,  // FL FR BL BR
        5 => 0x37,  // FL FR FC BL BR
        6 => 0x3F,  // 5.1
        7 => 0x70F, // FL FR FC LFE BC SL SR
        8 => 0x63F, // 7.1: FL FR FC LFE BL BR SL SR
        _ => 0,     // no positions: the driver decides
    }
}

fn wave_format(sample: Sample, rate: u32, channels: usize) -> WAVEFORMATEXTENSIBLE {
    let bytes = sample.bytes() as u16;
    let align = bytes * channels as u16;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels as u16,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * align as u32,
            nBlockAlign: align,
            wBitsPerSample: bytes * 8,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: sample.valid_bits() as u16 },
        dwChannelMask: channel_mask(channels),
        SubFormat: if sample == Sample::F32 { KSDATAFORMAT_SUBTYPE_IEEE_FLOAT } else { KSDATAFORMAT_SUBTYPE_PCM },
    }
}

fn as_waveformatex(format: &WAVEFORMATEXTENSIBLE) -> *const WAVEFORMATEX {
    (format as *const WAVEFORMATEXTENSIBLE).cast()
}

/// The engine's left-justified s32 in the device's format, little endian.
/// `out` holds exactly `src.len()` samples of `sample.bytes()`.
fn convert(sample: Sample, src: &[i32], out: &mut [u8]) {
    let n = sample.bytes();
    let out = out.chunks_exact_mut(n).zip(src);
    match sample {
        Sample::F32 => out.for_each(|(o, &s)| o.copy_from_slice(&(s as f32 / 2_147_483_648.0).to_le_bytes())),
        Sample::S32 => out.for_each(|(o, &s)| o.copy_from_slice(&s.to_le_bytes())),
        Sample::S24In32 => out.for_each(|(o, &s)| o.copy_from_slice(&(s >> 8).to_le_bytes())),
        Sample::S24Packed => out.for_each(|(o, &s)| o.copy_from_slice(&s.to_le_bytes()[1..4])),
        Sample::S16 => out.for_each(|(o, &s)| o.copy_from_slice(&((s >> 16) as i16).to_le_bytes())),
    }
}

/// A buffer of `frames` as a period in 100 ns units, as Microsoft's
/// AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED recipe computes it.
fn aligned_period(frames: u32, rate: u32) -> i64 {
    (10_000_000.0 * frames as f64 / rate as f64 + 0.5) as i64
}

fn hns_to_frames(hns: i64, rate: u32) -> usize {
    (hns.max(0) as u128 * rate as u128 / 10_000_000) as usize
}

/// The names of the HRESULTs WASAPI gives (Windows has no text for them).
fn hresult_name(code: HRESULT) -> Option<&'static str> {
    Some(match code {
        AUDCLNT_E_DEVICE_IN_USE => "the device is in use by another application",
        AUDCLNT_E_UNSUPPORTED_FORMAT => "format not supported",
        AUDCLNT_E_DEVICE_INVALIDATED => "the device was removed",
        AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED => "exclusive mode is not allowed for this device",
        AUDCLNT_E_SERVICE_NOT_RUNNING => "the Windows audio service is not running",
        AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => "buffer size not aligned",
        _ => return None,
    })
}

fn win_error(label: &str, what: &str, e: &WinError) -> OutputError {
    match hresult_name(e.code()) {
        Some(name) => OutputError(format!("{label}: {what}: {name} ({})", e.code())),
        None => OutputError(format!("{label}: {what}: {e}")),
    }
}

// --- The stream ------------------------------------------------------------------

pub struct WasapiOutput {
    key: String,
    format: DeviceFormat,
    /// Frames from render() to the speakers: the buffer and the stream's latency.
    latency: u64,
    stop: Arc<AtomicBool>,
    /// The device thread ended on its own (the endpoint went away).
    failed: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WasapiOutput {
    /// Opens the endpoint `name` ("" = the default console endpoint) on a
    /// thread of its own, which then feeds it until dropped.
    pub fn open(key: &str, name: &str, exclusive: bool, request: Request, render: Arc<dyn Render>) -> Result<Self> {
        if request.dsd.is_some_and(|p| p.is_native()) {
            return Err(OutputError(format!("{key}: WASAPI has no native DSD format")));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened) = mpsc::sync_channel(1);
        let (thread_stop, thread_failed, name) = (stop.clone(), failed.clone(), name.to_owned());
        let thread = std::thread::Builder::new()
            .name("sonkore-wasapi".into())
            .spawn(move || {
                let _com = Com::init(); // dropped last, after the stream
                match Stream::open(&name, exclusive, request) {
                    Ok(stream) => {
                        let _ = opened_tx.send(Ok((stream.format, stream.latency)));
                        stream.run(render, &thread_stop);
                        if !thread_stop.load(Ordering::Acquire) {
                            // Not asked to stop: unplugged, or the audio service went.
                            thread_failed.store(true, Ordering::Release);
                        }
                    }
                    Err(e) => {
                        let _ = opened_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| OutputError(e.to_string()))?;
        match opened.recv() {
            Ok(Ok((format, latency))) => {
                Ok(WasapiOutput { key: key.to_owned(), format, latency, stop, failed, thread: Some(thread) })
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(OutputError(format!("{key}: the WASAPI thread ended")))
            }
        }
    }
}

impl Output for WasapiOutput {
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

impl Drop for WasapiOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// An initialised audio client and what the device thread needs; lives on
/// the device thread only.
struct Stream {
    client: IAudioClient,
    render_client: IAudioRenderClient,
    sample: Sample,
    buffer_frames: u32,
    format: DeviceFormat,
    latency: u64,
    event: Event, // last: closed once the client is released
}

impl Stream {
    fn open(name: &str, exclusive: bool, request: Request) -> Result<Stream> {
        let label = if name.is_empty() { "Default output" } else { name };
        let enumerator = enumerator().map_err(|e| win_error(label, "device enumerator", &e))?;
        let device = if name.is_empty() {
            // SAFETY: a plain COM call on a live enumerator.
            unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| win_error(label, "default endpoint", &e))?
        } else {
            let mut list = endpoints(&enumerator);
            let index = lookup(list.iter().map(|e| (e.id.as_str(), e.name.as_str())), name)
                .ok_or_else(|| OutputError(format!("{name}: output not found")))?;
            list.swap_remove(index).device
        };
        if !exclusive {
            return Self::open_shared(&device, request, label);
        }
        match Self::open_exclusive(&device, request, label) {
            Ok(stream) => Ok(stream),
            // DSD only exclusively: the mixer would turn DoP into noise.
            Err(e) if request.dsd.is_some() => Err(e),
            // Busy (another application holds it exclusively), or no format
            // fits: share it.
            Err(exclusive_error) => Self::open_shared(&device, request, label)
                .map_err(|e| OutputError(format!("{e} (exclusive: {exclusive_error})"))),
        }
    }

    fn activate(device: &IMMDevice, label: &str) -> Result<IAudioClient> {
        // SAFETY: a plain COM call on a live device.
        unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }.map_err(|e| win_error(label, "audio client", &e))
    }

    /// Shared: the mixer's rate, 32-bit float, the request's channels (mono
    /// as stereo); Windows converts only the channel count, if anything.
    fn open_shared(device: &IMMDevice, request: Request, label: &str) -> Result<Stream> {
        let client = Self::activate(device, label)?;
        // SAFETY: GetMixFormat hands over a CoTaskMem WAVEFORMATEX(TENSIBLE),
        // read (unaligned: the struct is packed) and freed here.
        let (rate, mix_channels, mix_float) = unsafe {
            let mix = client.GetMixFormat().map_err(|e| win_error(label, "mix format", &e))?;
            let head = std::ptr::read_unaligned(mix);
            let subformat = (head.wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16 && head.cbSize >= 22)
                .then(|| std::ptr::read_unaligned(mix.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat);
            CoTaskMemFree(Some(mix as *const _));
            let float = head.wBitsPerSample == 32
                && (head.wFormatTag == WAVE_FORMAT_IEEE_FLOAT as u16
                    || subformat == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT));
            (head.nSamplesPerSec, head.nChannels as usize, float)
        };
        let (channels, mono_to_stereo) =
            if request.channels == 1 && mix_channels != 1 { (2, true) } else { (request.channels, false) };
        let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        if channels != mix_channels || !mix_float {
            flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM; // same rate: no SRC, only the matrix
        }
        let format = wave_format(Sample::F32, rate, channels);
        // SAFETY: `format` outlives the call, which copies it.
        unsafe {
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, SHARED_BUFFER_HNS, 0, as_waveformatex(&format), None)
        }
        .map_err(|e| win_error(label, &format!("shared mode at {rate} Hz, {channels} channels"), &e))?;
        Self::finish(client, Sample::F32, rate, channels, mono_to_stereo, None, label)
    }

    /// Exclusive: an integer format at the request's rate if the endpoint
    /// takes one, else at the standard rate pick_rate prefers; DoP only at
    /// exactly its rate in 24 or 32 bits.
    fn open_exclusive(device: &IMMDevice, request: Request, label: &str) -> Result<Stream> {
        let client = Self::activate(device, label)?;
        let dop = request.dsd.is_some();
        let samples = if dop { &EXCLUSIVE_SAMPLES[..3] } else { &EXCLUSIVE_SAMPLES[..] };
        let supported = |sample: Sample, rate: u32, channels: usize| {
            let format = wave_format(sample, rate, channels);
            // SAFETY: `format` outlives the call; exclusive mode takes no
            // closest-match pointer.
            unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, as_waveformatex(&format), None) }.is_ok()
        };
        // The exact rate in any depth first (resampling costs more than a
        // smaller container), then the best rate the deepest format takes.
        let choose = |channels: usize| -> Option<(Sample, u32)> {
            if let Some(&s) = samples.iter().find(|&&s| supported(s, request.rate, channels)) {
                return Some((s, request.rate));
            }
            if dop {
                return None; // DSD only at its own rate: anything else is another stream
            }
            samples.iter().find_map(|&s| {
                let rates: Vec<u32> = STANDARD_RATES.iter().copied().filter(|&r| supported(s, r, channels)).collect();
                pick_rate(rates, request.rate).map(|r| (s, r))
            })
        };
        let (channels, mono_to_stereo, (sample, rate)) = match choose(request.channels) {
            Some(found) => (request.channels, false, found),
            None if request.channels == 1 => match choose(2) {
                Some(found) => (2, true, found),
                None => return Err(Self::no_format(label, request)),
            },
            None => return Err(Self::no_format(label, request)),
        };
        let format = wave_format(sample, rate, channels);
        let mut period = 0i64;
        // SAFETY: a local out value.
        unsafe { client.GetDevicePeriod(Some(&mut period), None) }
            .map_err(|e| win_error(label, "device period", &e))?;
        let what = format!("exclusive mode at {rate} Hz, {} bits", sample.valid_bits());
        let initialize = |client: &IAudioClient, period: i64| {
            // SAFETY: `format` outlives the call, which copies it. Exclusive
            // event mode takes the same buffer duration and period.
            unsafe {
                client.Initialize(
                    AUDCLNT_SHAREMODE_EXCLUSIVE,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    period,
                    period,
                    as_waveformatex(&format),
                    None,
                )
            }
        };
        let client = match initialize(&client, period) {
            Ok(()) => client,
            Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
                // Microsoft's recipe: the aligned size from the failed client,
                // as a period, on a new client.
                // SAFETY: a plain COM call; the client is released after it.
                let frames = unsafe { client.GetBufferSize() }.map_err(|e| win_error(label, "buffer size", &e))?;
                drop(client);
                let client = Self::activate(device, label)?;
                initialize(&client, aligned_period(frames, rate)).map_err(|e| win_error(label, &what, &e))?;
                client
            }
            Err(e) => return Err(win_error(label, &what, &e)),
        };
        Self::finish(client, sample, rate, channels, mono_to_stereo, request.dsd, label)
    }

    fn no_format(label: &str, request: Request) -> OutputError {
        if request.dsd.is_some() {
            OutputError(format!("{label}: DoP at {} Hz not supported in exclusive mode", request.rate))
        } else {
            OutputError(format!("{label}: no {}-channel integer format in exclusive mode", request.channels))
        }
    }

    /// The event, the render client and the format the engine sees; the
    /// stream started on silence.
    fn finish(
        client: IAudioClient,
        sample: Sample,
        rate: u32,
        channels: usize,
        mono_to_stereo: bool,
        dsd: Option<DsdPacking>,
        label: &str,
    ) -> Result<Stream> {
        let exclusive = sample != Sample::F32;
        // SAFETY: plain COM and Win32 calls on an initialised client; the
        // event is closed by Event's drop.
        unsafe {
            let event =
                Event(CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| win_error(label, "event", &e))?);
            client.SetEventHandle(event.0).map_err(|e| win_error(label, "event", &e))?;
            let buffer_frames = client.GetBufferSize().map_err(|e| win_error(label, "buffer size", &e))?;
            let render_client =
                client.GetService::<IAudioRenderClient>().map_err(|e| win_error(label, "render client", &e))?;
            let latency = hns_to_frames(client.GetStreamLatency().unwrap_or(0), rate);
            let period = if exclusive {
                buffer_frames as usize
            } else {
                let mut hns = 0i64;
                let _ = client.GetDevicePeriod(Some(&mut hns), None);
                hns_to_frames(hns, rate).clamp(1, buffer_frames as usize)
            };
            let format = DeviceFormat {
                rate,
                channels,
                bits: sample.valid_bits(),
                float: !exclusive,
                mono_to_stereo,
                exclusive,
                period_frames: period,
                buffer_frames: buffer_frames as usize,
                dsd,
            };
            // Silence in the whole buffer, then the clock starts: the
            // stream is running before the engine hears it is open.
            render_client.GetBuffer(buffer_frames).map_err(|e| win_error(label, "buffer", &e))?;
            render_client
                .ReleaseBuffer(buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)
                .map_err(|e| win_error(label, "buffer", &e))?;
            client.Start().map_err(|e| win_error(label, "start", &e))?;
            Ok(Stream {
                client,
                render_client,
                sample,
                buffer_frames,
                format,
                latency: (buffer_frames as usize + latency) as u64,
                event,
            })
        }
    }

    /// The device thread: on every event as many frames as the buffer has
    /// room for (all of it in exclusive mode), rendered into a block made once.
    fn run(self, render: Arc<dyn Render>, stop: &AtomicBool) {
        realtime::raise_priority();
        let ch = self.format.channels;
        let bytes = self.sample.bytes() * ch;
        let mut block = vec![0i32; self.buffer_frames as usize * ch];
        // SAFETY: GetBuffer lends `frames` frames of `bytes` bytes each, ours
        // until ReleaseBuffer; nothing else writes them meanwhile.
        unsafe {
            while !stop.load(Ordering::Acquire) {
                if WaitForSingleObject(self.event.0, WAIT_MS) != WAIT_OBJECT_0 {
                    // No event yet: look at `stop` again, and whether the
                    // device is still there (a removed one sends no events).
                    if self.client.GetCurrentPadding().is_err() {
                        break;
                    }
                    continue;
                }
                let frames = if self.format.exclusive {
                    self.buffer_frames
                } else {
                    match self.client.GetCurrentPadding() {
                        Ok(padding) => self.buffer_frames.saturating_sub(padding),
                        Err(_) => break, // unplugged (AUDCLNT_E_DEVICE_INVALIDATED) or the service stopped
                    }
                };
                if frames == 0 {
                    continue;
                }
                let Ok(data) = self.render_client.GetBuffer(frames) else { break };
                let samples = &mut block[..frames as usize * ch];
                render.render(samples);
                convert(self.sample, samples, std::slice::from_raw_parts_mut(data, frames as usize * bytes));
                if self.render_client.ReleaseBuffer(frames, 0).is_err() {
                    break;
                }
            }
            // An unplugged device ends the thread here; the output then
            // reports itself failed and the engine closes it.
            let _ = self.client.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::GUID;

    fn converted(sample: Sample, src: &[i32]) -> Vec<u8> {
        let mut out = vec![0u8; src.len() * sample.bytes()];
        convert(sample, src, &mut out);
        out
    }

    #[test]
    fn converts_left_justified_s32() {
        let s = [0x1234_5600i32, -0x0000_0100, i32::MIN];
        assert_eq!(converted(Sample::S32, &s), [0x00, 0x56, 0x34, 0x12, 0x00, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0x80]);
        assert_eq!(converted(Sample::S24In32, &s), [0x56, 0x34, 0x12, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0x80, 0xFF]);
        assert_eq!(converted(Sample::S24Packed, &s), [0x56, 0x34, 0x12, 0xFF, 0xFF, 0xFF, 0, 0, 0x80]);
        assert_eq!(converted(Sample::S16, &s), [0x34, 0x12, 0xFF, 0xFF, 0, 0x80]);
        let f = converted(Sample::F32, &[i32::MIN, 1 << 30, 0]);
        let f: Vec<f32> = f.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        assert_eq!(f, [-1.0, 0.5, 0.0]);
    }

    #[test]
    fn float_keeps_24_bits_exactly() {
        let s = 0x7FFF_FF00i32; // the largest 24-bit sample
        let f = f32::from_le_bytes(converted(Sample::F32, &[s]).try_into().unwrap());
        assert_eq!((f as f64 * 2_147_483_648.0) as i32, s);
    }

    #[test]
    fn wave_formats_describe_the_samples() {
        let f = wave_format(Sample::S24Packed, 96_000, 2);
        let (tag, align, bits, avg, cb) = (
            f.Format.wFormatTag,
            f.Format.nBlockAlign,
            f.Format.wBitsPerSample,
            f.Format.nAvgBytesPerSec,
            f.Format.cbSize,
        );
        assert_eq!((tag as u32, align, bits, avg, cb), (WAVE_FORMAT_EXTENSIBLE, 6, 24, 576_000, 22));
        let f = wave_format(Sample::S24In32, 44_100, 6);
        // SAFETY: the union holds wValidBitsPerSample.
        let (valid, align, mask) = (unsafe { f.Samples.wValidBitsPerSample }, f.Format.nBlockAlign, f.dwChannelMask);
        assert_eq!((valid, align, mask), (24, 24, 0x3F));
        let sub: GUID = f.SubFormat;
        assert_eq!(sub, KSDATAFORMAT_SUBTYPE_PCM);
        let sub: GUID = wave_format(Sample::F32, 48_000, 2).SubFormat;
        assert_eq!(sub, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
    }

    #[test]
    fn channel_masks_have_one_bit_per_channel() {
        for ch in 1..=8 {
            assert_eq!(channel_mask(ch).count_ones() as usize, ch);
        }
    }

    #[test]
    fn duplicate_names_keep_both_reachable() {
        let list = [("{id-a}", "Speakers"), ("{id-b}", "Headphones"), ("{id-c}", "Speakers")];
        let keys = endpoint_keys(list.iter().copied());
        assert_eq!(keys, ["wasapi|Speakers", "wasapi|Headphones", "wasapi|{id-c}"]);
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(lookup(list.iter().copied(), key.strip_prefix("wasapi|").unwrap()), Some(i));
        }
        assert_eq!(lookup(list.iter().copied(), "Gone"), None);
    }

    #[test]
    fn aligned_periods_round_to_100ns() {
        assert_eq!(aligned_period(441, 44_100), 100_000); // 10 ms
        assert_eq!(aligned_period(448, 44_100), 101_587); // 10.1587 ms
        assert_eq!(hns_to_frames(100_000, 48_000), 480);
    }
}
