//! PipeWire spoken natively: libpipewire loaded at run
//! time, so Sonkore starts where it is missing. The stream asks for the
//! track's rate, and with "follow the track's rate" the graph is held at it
//! (the "settings" metadata's clock.force-rate) while the output is open and
//! given back on release. The stream's own volume is held at 100 %.
//!
//! The C structures below are laid out as PipeWire 1.x's headers lay them
//! out (sizes and offsets checked against them); the method tables of the
//! core, registry and metadata objects are called as the headers' inline
//! functions call them.

use std::cell::UnsafeCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{DeviceFormat, Output, OutputError, Render, Request, Result};

// --- C layouts ------------------------------------------------------------------------

#[repr(C)]
struct SpaList {
    next: *mut SpaList,
    prev: *mut SpaList,
}

#[repr(C)]
struct SpaCallbacks {
    funcs: *const c_void,
    data: *mut c_void,
}

#[repr(C)]
struct SpaHook {
    link: SpaList,
    cb: SpaCallbacks,
    removed: Option<unsafe extern "C" fn(*mut SpaHook)>,
    priv_: *mut c_void,
}

impl SpaHook {
    const fn zeroed() -> Self {
        SpaHook {
            link: SpaList { next: null_mut(), prev: null_mut() },
            cb: SpaCallbacks { funcs: null(), data: null_mut() },
            removed: None,
            priv_: null_mut(),
        }
    }
}

/// spa_hook_remove.
unsafe fn hook_remove(hook: *mut SpaHook) {
    unsafe {
        let link = &mut (*hook).link;
        if !link.next.is_null() {
            (*link.prev).next = link.next;
            (*link.next).prev = link.prev;
            link.next = null_mut();
            link.prev = null_mut();
        }
        if let Some(removed) = (*hook).removed {
            removed(hook);
        }
    }
}

#[repr(C)]
struct SpaInterface {
    type_: *const c_char,
    version: u32,
    cb: SpaCallbacks,
}

#[repr(C)]
struct SpaDictItem {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct SpaDict {
    flags: u32,
    n_items: u32,
    items: *const SpaDictItem,
}

unsafe fn dict_lookup<'a>(dict: *const SpaDict, key: &str) -> Option<&'a str> {
    if dict.is_null() {
        return None;
    }
    unsafe {
        let d = &*dict;
        for i in 0..d.n_items as usize {
            let item = &*d.items.add(i);
            if !item.key.is_null() && CStr::from_ptr(item.key).to_bytes() == key.as_bytes() {
                return if item.value.is_null() { None } else { CStr::from_ptr(item.value).to_str().ok() };
            }
        }
    }
    None
}

#[repr(C)]
struct CoreMethods {
    version: u32,
    add_listener: unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const CoreEvents, *mut c_void) -> c_int,
    hello: *const c_void,
    sync: unsafe extern "C" fn(*mut c_void, u32, c_int) -> c_int,
    pong: *const c_void,
    error: *const c_void,
    get_registry: unsafe extern "C" fn(*mut c_void, u32, usize) -> *mut c_void,
    create_object: *const c_void,
    destroy: *const c_void,
}

#[repr(C)]
struct CoreEvents {
    version: u32,
    info: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    done: Option<unsafe extern "C" fn(*mut c_void, u32, c_int)>,
    ping: Option<unsafe extern "C" fn(*mut c_void, u32, c_int)>,
    error: Option<unsafe extern "C" fn(*mut c_void, u32, c_int, c_int, *const c_char)>,
    remove_id: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    bound_id: Option<unsafe extern "C" fn(*mut c_void, u32, u32)>,
    add_mem: Option<unsafe extern "C" fn(*mut c_void, u32, u32, c_int, u32)>,
    remove_mem: Option<unsafe extern "C" fn(*mut c_void, u32)>,
    bound_props: Option<unsafe extern "C" fn(*mut c_void, u32, u32, *const SpaDict)>,
}

#[repr(C)]
struct RegistryMethods {
    version: u32,
    add_listener: unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const RegistryEvents, *mut c_void) -> c_int,
    bind: unsafe extern "C" fn(*mut c_void, u32, *const c_char, u32, usize) -> *mut c_void,
    destroy: *const c_void,
}

#[repr(C)]
struct RegistryEvents {
    version: u32,
    global: Option<unsafe extern "C" fn(*mut c_void, u32, u32, *const c_char, u32, *const SpaDict)>,
    global_remove: Option<unsafe extern "C" fn(*mut c_void, u32)>,
}

#[repr(C)]
struct MetadataMethods {
    version: u32,
    add_listener: unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const MetadataEvents, *mut c_void) -> c_int,
    set_property: unsafe extern "C" fn(*mut c_void, u32, *const c_char, *const c_char, *const c_char) -> c_int,
    clear: *const c_void,
}

#[repr(C)]
struct MetadataEvents {
    version: u32,
    property: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_char, *const c_char, *const c_char) -> c_int>,
}

#[repr(C)]
struct NodeMethods {
    version: u32,
    add_listener: unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const NodeEvents, *mut c_void) -> c_int,
    subscribe_params: unsafe extern "C" fn(*mut c_void, *mut u32, u32) -> c_int,
    enum_params: *const c_void,
    set_param: unsafe extern "C" fn(*mut c_void, u32, u32, *const c_void) -> c_int,
    send_command: *const c_void,
}

#[repr(C)]
struct NodeEvents {
    version: u32,
    info: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    param: Option<unsafe extern "C" fn(*mut c_void, c_int, u32, u32, u32, *const c_void)>,
}

#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    /// Since 0.3.50 (stream.h): older libraries end the struct before it,
    /// and reading it reads their private fields (see `has_requested`).
    requested: u64,
    time: u64,
}

/// The library fills pw_buffer.requested: 0.3.50 or later (stream.h says
/// "Since 0.3.50"). A version that does not parse counts as older.
fn has_requested(version: &str) -> bool {
    let mut parts = version.trim().split('.').map(|p| {
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let (Some(Some(major)), Some(Some(minor)), micro) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let micro = micro.flatten().unwrap_or(0);
    (major, minor, micro) >= (0, 3, 50)
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

#[repr(C)]
struct SpaData {
    type_: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

// Sizes the headers give (pipewire 1.x, 64-bit).
const _: () = {
    assert!(size_of::<SpaHook>() == 48);
    assert!(size_of::<SpaDict>() == 16);
    assert!(size_of::<SpaInterface>() == 32);
    assert!(size_of::<CoreMethods>() == 72);
    assert!(size_of::<CoreEvents>() == 80);
    assert!(size_of::<RegistryMethods>() == 32);
    assert!(size_of::<RegistryEvents>() == 24);
    assert!(size_of::<MetadataMethods>() == 32);
    assert!(size_of::<MetadataEvents>() == 16);
    assert!(size_of::<NodeMethods>() == 48);
    assert!(std::mem::offset_of!(NodeMethods, set_param) == 32);
    assert!(size_of::<NodeEvents>() == 24);
    assert!(size_of::<StreamEvents>() == 96);
    assert!(size_of::<PwBuffer>() == 40);
    assert!(size_of::<SpaBuffer>() == 24);
    assert!(size_of::<SpaData>() == 40);
    assert!(size_of::<SpaChunk>() == 16);
};

const PW_VERSION_REGISTRY: u32 = 3;
const PW_VERSION_METADATA: u32 = 3;
const PW_VERSION_NODE: u32 = 3;
const PW_VERSION_NODE_EVENTS: u32 = 0;
const PW_VERSION_CORE_EVENTS: u32 = 1;
const PW_VERSION_REGISTRY_EVENTS: u32 = 0;
const PW_VERSION_METADATA_EVENTS: u32 = 0;
const PW_VERSION_STREAM_EVENTS: u32 = 2;
const PW_ID_CORE: u32 = 0;
const PW_ID_ANY: u32 = u32::MAX;
const PW_DIRECTION_OUTPUT: c_int = 1;
const PW_STREAM_FLAG_AUTOCONNECT: c_int = 1;
const PW_STREAM_FLAG_INACTIVE: c_int = 2;
const PW_STREAM_FLAG_MAP_BUFFERS: c_int = 4;
const PW_STREAM_FLAG_RT_PROCESS: c_int = 16;
const STATE_ERROR: c_int = -1;
const STATE_UNCONNECTED: c_int = 0;
const STATE_CONNECTING: c_int = 1;
const STATE_PAUSED: c_int = 2;
const STATE_STREAMING: c_int = 3;
const SPA_IO_POSITION: u32 = 7;
const SPA_PROP_CHANNEL_VOLUMES: u32 = 0x10008;
const SPA_PROP_MUTE: u32 = 0x10004;
const SPA_PROP_SOFT_MUTE: u32 = 0x1000f;
const SPA_PROP_SOFT_VOLUMES: u32 = 0x10010;
const SPA_PARAM_PROPS: u32 = 2;
const SPA_TYPE_OBJECT_PROPS: u32 = 0x40002;
const SPA_AUDIO_MAX_CHANNELS: usize = 64;
const EPIPE: c_int = 32;
const TYPE_NODE: &CStr = c"PipeWire:Interface:Node";
const TYPE_METADATA: &CStr = c"PipeWire:Interface:Metadata";
const TYPE_LINK: &CStr = c"PipeWire:Interface:Link";
/// The node.name of Sonkore's own playback stream (PipeWireOutput).
const STREAM_NODE_NAME: &str = "sonkore";

// --- The library ---------------------------------------------------------------------

#[allow(clippy::type_complexity)]
struct Api {
    pw_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    get_library_version: unsafe extern "C" fn() -> *const c_char,
    thread_loop_new: unsafe extern "C" fn(*const c_char, *const SpaDict) -> *mut c_void,
    thread_loop_destroy: unsafe extern "C" fn(*mut c_void),
    thread_loop_start: unsafe extern "C" fn(*mut c_void) -> c_int,
    thread_loop_stop: unsafe extern "C" fn(*mut c_void),
    thread_loop_lock: unsafe extern "C" fn(*mut c_void),
    thread_loop_unlock: unsafe extern "C" fn(*mut c_void),
    thread_loop_timed_wait: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    thread_loop_signal: unsafe extern "C" fn(*mut c_void, bool),
    thread_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    context_new: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void,
    context_destroy: unsafe extern "C" fn(*mut c_void),
    context_connect: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void,
    core_disconnect: unsafe extern "C" fn(*mut c_void) -> c_int,
    properties_new_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    properties_set: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    proxy_destroy: unsafe extern "C" fn(*mut c_void),
    stream_new: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> *mut c_void,
    stream_add_listener: unsafe extern "C" fn(*mut c_void, *mut SpaHook, *const StreamEvents, *mut c_void),
    stream_connect: unsafe extern "C" fn(*mut c_void, c_int, u32, c_int, *mut *const c_void, u32) -> c_int,
    stream_destroy: unsafe extern "C" fn(*mut c_void),
    stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut PwBuffer) -> c_int,
    stream_set_active: unsafe extern "C" fn(*mut c_void, bool) -> c_int,
    stream_set_control: unsafe extern "C" fn(*mut c_void, u32, u32, *mut f32, ...) -> c_int,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

// Each symbol takes the type of the Api field it fills.
#[allow(clippy::missing_transmute_annotations)]
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let lib = libc::dlopen(c"libpipewire-0.3.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:literal) => {{
                let p = libc::dlsym(lib, concat!($name, "\0").as_ptr() as *const c_char);
                if p.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(p)
            }};
        }
        let api = Api {
            pw_init: sym!("pw_init"),
            get_library_version: sym!("pw_get_library_version"),
            thread_loop_new: sym!("pw_thread_loop_new"),
            thread_loop_destroy: sym!("pw_thread_loop_destroy"),
            thread_loop_start: sym!("pw_thread_loop_start"),
            thread_loop_stop: sym!("pw_thread_loop_stop"),
            thread_loop_lock: sym!("pw_thread_loop_lock"),
            thread_loop_unlock: sym!("pw_thread_loop_unlock"),
            thread_loop_timed_wait: sym!("pw_thread_loop_timed_wait"),
            thread_loop_signal: sym!("pw_thread_loop_signal"),
            thread_loop_get_loop: sym!("pw_thread_loop_get_loop"),
            context_new: sym!("pw_context_new"),
            context_destroy: sym!("pw_context_destroy"),
            context_connect: sym!("pw_context_connect"),
            core_disconnect: sym!("pw_core_disconnect"),
            properties_new_string: sym!("pw_properties_new_string"),
            properties_set: sym!("pw_properties_set"),
            proxy_destroy: sym!("pw_proxy_destroy"),
            stream_new: sym!("pw_stream_new"),
            stream_add_listener: sym!("pw_stream_add_listener"),
            stream_connect: sym!("pw_stream_connect"),
            stream_destroy: sym!("pw_stream_destroy"),
            stream_dequeue_buffer: sym!("pw_stream_dequeue_buffer"),
            stream_queue_buffer: sym!("pw_stream_queue_buffer"),
            stream_set_active: sym!("pw_stream_set_active"),
            stream_set_control: sym!("pw_stream_set_control"),
        };
        (api.pw_init)(null_mut(), null_mut());
        Some(api)
    })
    .as_ref()
}

/// "1.6.8", or "" without libpipewire.
pub fn library_version() -> String {
    api()
        .map(|a| unsafe { CStr::from_ptr((a.get_library_version)()).to_string_lossy().into_owned() })
        .unwrap_or_default()
}

/// A PipeWire socket for this user (the daemon is running).
pub fn running() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|dir| std::path::Path::new(&dir).join("pipewire-0").exists())
}

/// "alsa_output.x.3" -> "alsa_output.x": without the suffix WirePlumber adds
/// when a node is made again under a name still in use.
pub fn base_sink_name(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !base.is_empty() => base,
        _ => name,
    }
}

// --- Method calls on interfaces ---------------------------------------------------------

unsafe fn iface<'a, M>(object: *mut c_void) -> (&'a M, *mut c_void) {
    unsafe {
        let i = &*(object as *const SpaInterface);
        (&*(i.cb.funcs as *const M), i.cb.data)
    }
}

// --- The connection: registry, sinks, metadata --------------------------------------------

#[derive(Clone, Debug)]
pub struct Sink {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub is_default: bool,
    /// The ALSA card behind it, -1 for none.
    pub alsa_card: i32,
}

/// The ALSA card number a node stands for.
/// The registry announces only some properties: "object.path"
/// ("alsa:acp:DX1:0:playback", "alsa:pcm:1:hw:1,0:..."), whose card id
/// leads to the number through /proc/asound/<id> -> card<N>.
unsafe fn alsa_card_of(props: *const SpaDict) -> i32 {
    for key in ["alsa.card", "api.alsa.pcm.card"] {
        if let Some(v) = unsafe { dict_lookup(props, key) } {
            if let Ok(n) = v.parse::<i32>() {
                return n;
            }
        }
    }
    let Some(path) = (unsafe { dict_lookup(props, "object.path") }) else { return -1 };
    let Some(rest) = path.strip_prefix("alsa:") else { return -1 };
    let Some((_, rest)) = rest.split_once(':') else { return -1 };
    let id = rest.split(':').next().unwrap_or("");
    if id.is_empty() {
        return -1;
    }
    if id.bytes().all(|b| b.is_ascii_digit()) {
        return id.parse().unwrap_or(-1);
    }
    std::fs::read_link(format!("/proc/asound/{id}"))
        .ok()
        .and_then(|t| t.to_str().and_then(|t| t.strip_prefix("card")).and_then(|n| n.parse().ok()))
        .unwrap_or(-1)
}

struct Connection {
    api: &'static Api,
    lp: *mut c_void,
    context: *mut c_void,
    core: *mut c_void,
    registry: *mut c_void,
    core_hook: SpaHook,
    registry_hook: SpaHook,
    // Written by the loop thread's callbacks while a caller waits on them:
    // atomics, so the wait sees them (a plain field behind `&mut self` may be
    // read once and kept in a register).
    sync_seq: AtomicI32,
    synced: AtomicBool,
    failed: AtomicBool,
    sinks: Vec<Sink>,
    settings_id: u32,
    defaults_id: u32,
    settings: *mut c_void,
    defaults: *mut c_void,
    settings_hook: SpaHook,
    defaults_hook: SpaHook,
    setting_values: Vec<(String, String)>,
    default_sink: String,
    /// The sinks that belong to a device (ALSA...: "device.id" set), whose
    /// volume may sit on the DAC's own control.
    devices: Vec<u32>,
    /// Sonkore's playback streams in the graph (their node ids).
    streams: Vec<u32>,
    /// The graph's links: (link id, output node, input node).
    links: Vec<(u32, u32, u32)>,
    /// The sink volume watched over this connection (SinkVolume), or null.
    volume: std::sync::atomic::AtomicPtr<VolumeWatch>,
}

static CORE_EVENTS: CoreEvents = CoreEvents {
    version: PW_VERSION_CORE_EVENTS,
    info: None,
    done: Some(on_core_done),
    ping: None,
    error: Some(on_core_error),
    remove_id: None,
    bound_id: None,
    add_mem: None,
    remove_mem: None,
    bound_props: None,
};
static REGISTRY_EVENTS: RegistryEvents = RegistryEvents {
    version: PW_VERSION_REGISTRY_EVENTS,
    global: Some(on_global),
    global_remove: Some(on_global_remove),
};
static SETTING_EVENTS: MetadataEvents =
    MetadataEvents { version: PW_VERSION_METADATA_EVENTS, property: Some(on_setting) };
static DEFAULT_EVENTS: MetadataEvents =
    MetadataEvents { version: PW_VERSION_METADATA_EVENTS, property: Some(on_default) };

unsafe extern "C" fn on_core_done(data: *mut c_void, id: u32, seq: c_int) {
    let c = unsafe { &mut *(data as *mut Connection) };
    if id == PW_ID_CORE && seq == c.sync_seq.load(Ordering::Acquire) {
        c.synced.store(true, Ordering::Release);
        unsafe { (c.api.thread_loop_signal)(c.lp, false) };
    }
}

unsafe extern "C" fn on_core_error(data: *mut c_void, id: u32, _seq: c_int, res: c_int, _msg: *const c_char) {
    let c = unsafe { &mut *(data as *mut Connection) };
    if id == PW_ID_CORE && res == -EPIPE {
        c.failed.store(true, Ordering::Release);
        unsafe { (c.api.thread_loop_signal)(c.lp, false) };
    }
}

unsafe extern "C" fn on_global(
    data: *mut c_void,
    id: u32,
    _perm: u32,
    type_: *const c_char,
    _version: u32,
    props: *const SpaDict,
) {
    let c = unsafe { &mut *(data as *mut Connection) };
    if type_.is_null() || props.is_null() {
        return;
    }
    let ty = unsafe { CStr::from_ptr(type_) };
    unsafe {
        if ty == TYPE_NODE
            && dict_lookup(props, "node.name") == Some(STREAM_NODE_NAME)
            && dict_lookup(props, "media.class").is_some_and(|m| m.starts_with("Stream/Output/Audio"))
        {
            c.streams.push(id);
        } else if ty == TYPE_LINK {
            let node = |key| dict_lookup(props, key).and_then(|v| v.parse::<u32>().ok());
            if let (Some(out), Some(input)) = (node("link.output.node"), node("link.input.node")) {
                c.links.push((id, out, input));
                if c.streams.contains(&out) {
                    // The stream linked (or moved, as Easy Effects moves the
                    // streams to its own sink): that sink's volume.
                    if let Some(w) = c.volume.load(Ordering::Acquire).as_ref() {
                        if retarget(c, w) {
                            w.notify();
                        }
                    }
                }
            }
        } else if ty == TYPE_NODE {
            let (Some("Audio/Sink"), Some(name)) = (dict_lookup(props, "media.class"), dict_lookup(props, "node.name"))
            else {
                return;
            };
            let description = dict_lookup(props, "node.description").unwrap_or(name);
            let alsa_card = alsa_card_of(props);
            c.sinks.push(Sink {
                id,
                name: name.to_owned(),
                description: description.to_owned(),
                is_default: false,
                alsa_card,
            });
            if dict_lookup(props, "device.id").is_some() {
                c.devices.push(id);
            }
            if let Some(w) = c.volume.load(Ordering::Acquire).as_ref() {
                // The sink asked for by name is back (the session manager
                // moves a stream with that target back to it): its volume
                // again, unless the stream is linked elsewhere.
                if !w.follow_default && same_sink(name, &w.name) && retarget(c, w) {
                    w.notify();
                }
            }
        } else if ty == TYPE_METADATA {
            match dict_lookup(props, "metadata.name") {
                Some("settings") => c.settings_id = id,
                Some("default") => c.defaults_id = id,
                _ => {}
            }
        }
    }
}

unsafe extern "C" fn on_global_remove(data: *mut c_void, id: u32) {
    let c = unsafe { &mut *(data as *mut Connection) };
    let sink = c.sinks.iter().any(|s| s.id == id);
    c.sinks.retain(|s| s.id != id);
    c.devices.retain(|&d| d != id);
    // (A stream or a link going away changes nothing by itself: between
    // tracks the stream is made again and links to the same sink.)
    c.streams.retain(|&s| s != id);
    c.links.retain(|l| l.0 != id && l.1 != id && l.2 != id);
    if let Some(w) = unsafe { c.volume.load(Ordering::Acquire).as_ref() } {
        if sink && id == w.watched.load(Ordering::Acquire) {
            // The sink being watched is gone: the one the stream is linked
            // to, else the one asked for, else the desktop's default (where
            // the session manager moves the streams too).
            unwatch_sink(c, w);
            retarget(c, w);
            w.notify();
        }
    }
}

unsafe extern "C" fn on_setting(
    data: *mut c_void,
    subject: u32,
    key: *const c_char,
    _t: *const c_char,
    value: *const c_char,
) -> c_int {
    let c = unsafe { &mut *(data as *mut Connection) };
    if subject == PW_ID_CORE && !key.is_null() {
        let key = unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned();
        let value = if value.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned()
        };
        c.setting_values.retain(|(k, _)| *k != key);
        c.setting_values.push((key, value));
    }
    0
}

unsafe extern "C" fn on_default(
    data: *mut c_void,
    subject: u32,
    key: *const c_char,
    _t: *const c_char,
    value: *const c_char,
) -> c_int {
    let c = unsafe { &mut *(data as *mut Connection) };
    if subject == PW_ID_CORE && !key.is_null() && unsafe { CStr::from_ptr(key) } == c"default.audio.sink" {
        let json = if value.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned()
        };
        c.default_sink = json_name(&json);
        if let Some(w) = unsafe { c.volume.load(Ordering::Acquire).as_ref() } {
            // The desktop chose another output: follow it (unless the
            // stream is linked to another sink).
            if w.follow_default && retarget(c, w) {
                w.notify();
            }
        }
    }
    0
}

/// `{"name":"alsa_output.usb-..."}` (the "default" metadata's JSON) -> the name.
fn json_name(json: &str) -> String {
    let Some(p) = json.find("\"name\"") else { return String::new() };
    let rest = &json[p + 6..];
    let Some(colon) = rest.find(':') else { return String::new() };
    let rest = &rest[colon..];
    let Some(q) = rest.find('"') else { return String::new() };
    let rest = &rest[q + 1..];
    rest.find('"').map(|e| rest[..e].to_owned()).unwrap_or_default()
}

/// "[ 44100, 48000 ]" -> its rates.
fn rate_list(s: &str) -> Vec<u32> {
    s.split(|c: char| !c.is_ascii_digit()).filter(|t| !t.is_empty()).filter_map(|t| t.parse().ok()).collect()
}

impl Connection {
    /// Connects and lists the graph (boxed: the callbacks hold its address).
    fn connect() -> Result<Box<Connection>> {
        let api = api().ok_or_else(|| OutputError("libpipewire is not installed".into()))?;
        if !running() {
            return Err(OutputError("PipeWire is not running".into()));
        }
        let mut c = Box::new(Connection {
            api,
            lp: null_mut(),
            context: null_mut(),
            core: null_mut(),
            registry: null_mut(),
            core_hook: SpaHook::zeroed(),
            registry_hook: SpaHook::zeroed(),
            sync_seq: AtomicI32::new(0),
            synced: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            sinks: Vec::new(),
            settings_id: u32::MAX,
            defaults_id: u32::MAX,
            settings: null_mut(),
            defaults: null_mut(),
            settings_hook: SpaHook::zeroed(),
            defaults_hook: SpaHook::zeroed(),
            setting_values: Vec::new(),
            default_sink: String::new(),
            devices: Vec::new(),
            streams: Vec::new(),
            links: Vec::new(),
            volume: std::sync::atomic::AtomicPtr::new(null_mut()),
        });
        unsafe {
            c.lp = (api.thread_loop_new)(c"sonkore-pipewire".as_ptr(), null());
            if c.lp.is_null() {
                return Err(OutputError("PipeWire: no loop".into()));
            }
            c.context = (api.context_new)((api.thread_loop_get_loop)(c.lp), null_mut(), 0);
            if c.context.is_null() || (api.thread_loop_start)(c.lp) < 0 {
                c.disconnect();
                return Err(OutputError("PipeWire: no context".into()));
            }
            c.lock();
            c.core = (api.context_connect)(c.context, null_mut(), 0);
            if c.core.is_null() {
                c.unlock();
                c.disconnect();
                return Err(OutputError("PipeWire is not running".into()));
            }
            let data = &mut *c as *mut Connection as *mut c_void;
            let (m, o) = iface::<CoreMethods>(c.core);
            (m.add_listener)(o, &mut c.core_hook, &CORE_EVENTS, data);
            c.registry = (m.get_registry)(o, PW_VERSION_REGISTRY, 0);
            let (r, ro) = iface::<RegistryMethods>(c.registry);
            (r.add_listener)(ro, &mut c.registry_hook, &REGISTRY_EVENTS, data);
            let mut ok = c.roundtrip();
            if ok && c.settings_id != u32::MAX {
                c.settings = (r.bind)(ro, c.settings_id, TYPE_METADATA.as_ptr(), PW_VERSION_METADATA, 0);
                if !c.settings.is_null() {
                    let (mm, mo) = iface::<MetadataMethods>(c.settings);
                    (mm.add_listener)(mo, &mut c.settings_hook, &SETTING_EVENTS, data);
                }
            }
            if ok && c.defaults_id != u32::MAX {
                c.defaults = (r.bind)(ro, c.defaults_id, TYPE_METADATA.as_ptr(), PW_VERSION_METADATA, 0);
                if !c.defaults.is_null() {
                    let (mm, mo) = iface::<MetadataMethods>(c.defaults);
                    (mm.add_listener)(mo, &mut c.defaults_hook, &DEFAULT_EVENTS, data);
                }
            }
            ok = ok && c.roundtrip();
            c.unlock();
            if !ok {
                c.disconnect();
                return Err(OutputError("PipeWire does not answer".into()));
            }
        }
        Ok(c)
    }

    fn lock(&self) {
        unsafe { (self.api.thread_loop_lock)(self.lp) };
    }
    fn unlock(&self) {
        unsafe { (self.api.thread_loop_unlock)(self.lp) };
    }

    /// Waits until the daemon has handled everything asked so far (lock held).
    fn roundtrip(&self) -> bool {
        self.synced.store(false, Ordering::Release);
        unsafe {
            let (m, o) = iface::<CoreMethods>(self.core);
            let seq = (m.sync)(o, PW_ID_CORE, self.sync_seq.load(Ordering::Acquire));
            self.sync_seq.store(seq, Ordering::Release);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.synced.load(Ordering::Acquire) && !self.failed.load(Ordering::Acquire) && Instant::now() < deadline
        {
            unsafe { (self.api.thread_loop_timed_wait)(self.lp, 1) };
        }
        self.synced.load(Ordering::Acquire) && !self.failed.load(Ordering::Acquire)
    }

    fn setting(&self, key: &str) -> String {
        self.setting_values.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    /// Sets a "settings" key and waits until the daemon has it (lock held).
    fn set_setting(&self, key: &str, value: &str) {
        if self.settings.is_null() {
            return;
        }
        let (k, v) = (CString::new(key).unwrap(), CString::new(value).unwrap());
        unsafe {
            let (m, o) = iface::<MetadataMethods>(self.settings);
            (m.set_property)(o, PW_ID_CORE, k.as_ptr(), null(), v.as_ptr());
        }
        self.roundtrip();
    }

    fn disconnect(&mut self) {
        unsafe {
            if !self.lp.is_null() && !self.core.is_null() {
                self.lock();
                for (proxy, hook) in [
                    (self.settings, &mut self.settings_hook as *mut SpaHook),
                    (self.defaults, &mut self.defaults_hook),
                    (self.registry, &mut self.registry_hook),
                ] {
                    if !proxy.is_null() {
                        hook_remove(hook);
                        (self.api.proxy_destroy)(proxy);
                    }
                }
                self.settings = null_mut();
                self.defaults = null_mut();
                self.registry = null_mut();
                hook_remove(&mut self.core_hook);
                (self.api.core_disconnect)(self.core);
                self.core = null_mut();
                self.unlock();
            }
            if !self.lp.is_null() {
                (self.api.thread_loop_stop)(self.lp);
            }
            if !self.context.is_null() {
                (self.api.context_destroy)(self.context);
            }
            if !self.lp.is_null() {
                (self.api.thread_loop_destroy)(self.lp);
            }
        }
        self.lp = null_mut();
        self.context = null_mut();
    }
}

/// The graph's playback sinks (empty without PipeWire).
pub fn sinks() -> Vec<Sink> {
    let Ok(mut c) = Connection::connect() else { return Vec::new() };
    c.lock();
    let default = c.default_sink.clone();
    let list: Vec<Sink> = c.sinks.iter().map(|s| Sink { is_default: s.name == default, ..s.clone() }).collect();
    c.unlock();
    c.disconnect();
    list
}

// --- The sink's volume ----------------------------------------------------------------

/// The watched sink: written by the loop thread's callbacks, read from any
/// thread (atomics and mutexes only, see Connection).
struct VolumeWatch {
    api: &'static Api,
    /// What open() asked for ("" = the desktop's default, followed).
    name: String,
    follow_default: bool,
    /// The sink's node proxy (bound from the registry), null when none.
    node: std::sync::atomic::AtomicPtr<c_void>,
    /// The node listener's hook (touched under the loop lock only).
    hook: UnsafeCell<SpaHook>,
    /// The watched sink's global id, u32::MAX for none.
    watched: AtomicU32,
    watched_name: Mutex<String>,
    description: Mutex<String>,
    /// A device's sink (ALSA...): its volume may sit in hardware.
    device: AtomicBool,
    /// The last Props: the loudest channel's channelVolumes (f32 bits), mute,
    /// softVolumes all at 1 (the hardware carries the volume), the channels.
    volume: AtomicU32,
    muted: AtomicBool,
    known: AtomicBool,
    soft_unity: AtomicBool,
    channels: AtomicU32,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl VolumeWatch {
    fn notify(&self) {
        let f = self.on_change.lock().unwrap().clone();
        if let Some(f) = f {
            f();
        }
    }
}

static NODE_EVENTS: NodeEvents = NodeEvents { version: PW_VERSION_NODE_EVENTS, info: None, param: Some(on_sink_param) };

/// "x" and "x.2" are the same sink (WirePlumber's suffix for a node made again).
fn same_sink(a: &str, b: &str) -> bool {
    a == b || (!a.is_empty() && base_sink_name(a) == base_sink_name(b))
}

/// Watches the volume of the sink called `name` (lock held): its node bound
/// and its Props subscribed to (they come in through on_sink_param).
fn watch_sink(c: &Connection, w: &VolumeWatch, name: &str) {
    let found = c
        .sinks
        .iter()
        .rev()
        .find(|s| s.name == name)
        .or_else(|| if name.is_empty() { None } else { c.sinks.iter().find(|s| same_sink(&s.name, name)) });
    let id = found.map_or(u32::MAX, |s| s.id);
    if id == w.watched.load(Ordering::Acquire) {
        return;
    }
    unwatch_sink(c, w);
    w.known.store(false, Ordering::Release);
    w.soft_unity.store(true, Ordering::Release);
    *w.description.lock().unwrap() = found.map(|s| s.description.clone()).unwrap_or_default();
    *w.watched_name.lock().unwrap() = found.map(|s| s.name.clone()).unwrap_or_default();
    if id == u32::MAX || c.registry.is_null() {
        return;
    }
    unsafe {
        let (r, ro) = iface::<RegistryMethods>(c.registry);
        let node = (r.bind)(ro, id, TYPE_NODE.as_ptr(), PW_VERSION_NODE, 0);
        if node.is_null() {
            return;
        }
        w.node.store(node, Ordering::Release);
        w.watched.store(id, Ordering::Release);
        w.device.store(c.devices.contains(&id), Ordering::Release);
        let (m, o) = iface::<NodeMethods>(node);
        (m.add_listener)(o, w.hook.get(), &NODE_EVENTS, w as *const VolumeWatch as *mut c_void);
        let mut ids = [SPA_PARAM_PROPS];
        (m.subscribe_params)(o, ids.as_mut_ptr(), 1);
    }
}

/// The sink Sonkore's stream is linked to (the first one, when several):
/// a sink's id among `sinks`, never another stream (a recorder).
fn linked_sink(streams: &[u32], links: &[(u32, u32, u32)], sinks: &[u32]) -> Option<u32> {
    links.iter().find(|l| streams.contains(&l.1) && sinks.contains(&l.2)).map(|l| l.2)
}

/// Watches the sink whose volume is heard (lock held): the one Sonkore's
/// stream is linked to (an Easy Effects filter, for one, takes the streams
/// over), else the one asked for when it is there, else the desktop's
/// default. True when another sink is watched now.
fn retarget(c: &Connection, w: &VolumeWatch) -> bool {
    let before = w.watched.load(Ordering::Acquire);
    let ids: Vec<u32> = c.sinks.iter().map(|s| s.id).collect();
    let name = match linked_sink(&c.streams, &c.links, &ids) {
        Some(id) => c.sinks.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default(),
        None if !w.follow_default && c.sinks.iter().any(|s| same_sink(&s.name, &w.name)) => w.name.clone(),
        None => c.default_sink.clone(),
    };
    watch_sink(c, w, &name);
    w.watched.load(Ordering::Acquire) != before
}

/// Stops watching the sink's node (lock held).
fn unwatch_sink(_c: &Connection, w: &VolumeWatch) {
    let node = w.node.swap(null_mut(), Ordering::AcqRel);
    if !node.is_null() {
        unsafe {
            hook_remove(w.hook.get());
            (w.api.proxy_destroy)(node);
        }
    }
    w.watched.store(u32::MAX, Ordering::Release);
}

/// What a sink's Props say about its volume.
#[derive(Debug, Default, PartialEq)]
struct PropsInfo {
    channel_volumes: Option<Vec<f32>>,
    soft_volumes: Option<Vec<f32>>,
    mute: Option<bool>,
    soft_mute: Option<bool>,
}

fn all_unity(v: &[f32]) -> bool {
    v.iter().all(|x| (x - 1.0).abs() < 1e-6)
}

/// Reads a Props object POD (the bytes of the whole POD, header included).
fn parse_props(pod: &[u8]) -> Option<PropsInfo> {
    let u32_at = |b: &[u8], at: usize| b.get(at..at + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()));
    let size = u32_at(pod, 0)? as usize;
    if u32_at(pod, 4)? != 15 || size < 8 || pod.len() < 8 + size {
        return None; // not an Object
    }
    let body = &pod[8..8 + size];
    if u32_at(body, 0)? != SPA_TYPE_OBJECT_PROPS {
        return None;
    }
    let floats = |value: &[u8], vsize: usize| -> Option<Vec<f32>> {
        // Array: child size, child type, then the children.
        if vsize < 8 || u32_at(value, 0)? != 4 || u32_at(value, 4)? != 6 {
            return None;
        }
        let n = ((vsize - 8) / 4).min(SPA_AUDIO_MAX_CHANNELS);
        (0..n).map(|i| u32_at(value, 8 + 4 * i).map(f32::from_bits)).collect()
    };
    let mut info = PropsInfo::default();
    let mut at = 8;
    while at + 16 <= body.len() {
        let key = u32_at(body, at)?;
        let vsize = u32_at(body, at + 8)? as usize;
        let vtype = u32_at(body, at + 12)?;
        let value = body.get(at + 16..at + 16 + vsize)?;
        match (key, vtype) {
            (SPA_PROP_CHANNEL_VOLUMES, 13) => info.channel_volumes = floats(value, vsize),
            (SPA_PROP_SOFT_VOLUMES, 13) => info.soft_volumes = floats(value, vsize),
            (SPA_PROP_MUTE, 2) if vsize >= 4 => info.mute = Some(u32_at(value, 0)? != 0),
            (SPA_PROP_SOFT_MUTE, 2) if vsize >= 4 => info.soft_mute = Some(u32_at(value, 0)? != 0),
            _ => {}
        }
        at += 16 + vsize.next_multiple_of(8);
    }
    Some(info)
}

/// The Props that set a sink's volume: channelVolumes (each channel alike)
/// and mute, as spa_pod_builder_add_object builds them.
fn props_pod(volumes: &[f32], mute: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&SPA_TYPE_OBJECT_PROPS.to_le_bytes());
    body.extend_from_slice(&SPA_PARAM_PROPS.to_le_bytes());
    body.extend_from_slice(&SPA_PROP_CHANNEL_VOLUMES.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&(8 + 4 * volumes.len() as u32).to_le_bytes());
    body.extend_from_slice(&13u32.to_le_bytes()); // Array
    body.extend_from_slice(&4u32.to_le_bytes()); // child size
    body.extend_from_slice(&6u32.to_le_bytes()); // child type: Float
    for v in volumes {
        body.extend_from_slice(&v.to_le_bytes());
    }
    while body.len() % 8 != 0 {
        body.push(0);
    }
    body.extend_from_slice(&SPA_PROP_MUTE.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&4u32.to_le_bytes());
    body.extend_from_slice(&2u32.to_le_bytes()); // Bool
    body.extend_from_slice(&(mute as u32).to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // to 8 bytes
    let mut pod = Vec::with_capacity(8 + body.len());
    pod.extend_from_slice(&(body.len() as u32).to_le_bytes());
    pod.extend_from_slice(&15u32.to_le_bytes()); // Object
    pod.extend(body);
    pod
}

/// The sink's Props: its volume. A device's node (ALSA) splits the volume:
/// channelVolumes is what the user set, softVolumes the part left to software
/// once the hardware mixer has done what it can. Any other sink (virtual, a
/// filter) applies channelVolumes in software.
unsafe extern "C" fn on_sink_param(
    data: *mut c_void,
    _seq: c_int,
    id: u32,
    _index: u32,
    _next: u32,
    param: *const c_void,
) {
    let w = unsafe { &*(data as *const VolumeWatch) };
    if id != SPA_PARAM_PROPS || param.is_null() {
        return;
    }
    let size = unsafe { *(param as *const u32) } as usize;
    let pod = unsafe { std::slice::from_raw_parts(param as *const u8, 8 + size) };
    let Some(info) = parse_props(pod) else { return };
    if info.channel_volumes.is_none() && info.soft_volumes.is_none() {
        return; // some other Props object
    }
    let device = w.device.load(Ordering::Acquire);
    let volume_ok = info.channel_volumes.as_deref().is_none_or(all_unity);
    let soft_ok = info.soft_volumes.as_deref().is_none_or(all_unity);
    let split = device && info.soft_volumes.is_some();
    w.soft_unity.store(if split { soft_ok } else { volume_ok }, Ordering::Release);
    if let Some(v) = info.channel_volumes.as_deref() {
        if let Some(max) = v.iter().copied().reduce(f32::max) {
            w.volume.store(max.to_bits(), Ordering::Release);
            w.channels.store(v.len() as u32, Ordering::Release);
        }
        w.muted.store(info.mute.unwrap_or(false), Ordering::Release);
        w.known.store(true, Ordering::Release);
    }
    w.notify();
}

/// A PipeWire sink's own volume (platform/PipeWireVolume): the desktop's
/// default (followed when it changes) or a sink by name. A device's sink
/// (ALSA) carries the volume on the DAC's own control where it has one
/// (PipeWire splits what is left into softVolumes), so moving it keeps the
/// samples untouched. Volumes are linear amplitudes (channelVolumes), not the
/// cubic scale desktop mixers show.
pub struct SinkVolume {
    connection: Box<Connection>,
    watch: Box<VolumeWatch>,
}

// The raw pointers are PipeWire's, used under its loop lock; the state the
// callbacks write is atomics and mutexes.
unsafe impl Send for SinkVolume {}
unsafe impl Sync for SinkVolume {}

impl SinkVolume {
    /// Watches the sink `sink` (a node.name; "" = the desktop's default,
    /// followed when it changes) on a connection of its own. None without a
    /// daemon or when there is no such sink; the first Props have arrived
    /// when it returns.
    pub fn open(sink: &str) -> Option<SinkVolume> {
        let connection = Connection::connect().ok()?;
        let watch = Box::new(VolumeWatch {
            api: connection.api,
            name: sink.to_owned(),
            follow_default: sink.is_empty(),
            node: std::sync::atomic::AtomicPtr::new(null_mut()),
            hook: UnsafeCell::new(SpaHook::zeroed()),
            watched: AtomicU32::new(u32::MAX),
            watched_name: Mutex::new(String::new()),
            description: Mutex::new(String::new()),
            device: AtomicBool::new(false),
            volume: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            known: AtomicBool::new(false),
            soft_unity: AtomicBool::new(true),
            channels: AtomicU32::new(2),
            on_change: Mutex::new(None),
        });
        connection.lock();
        connection.volume.store(&*watch as *const VolumeWatch as *mut VolumeWatch, Ordering::Release);
        let name = if sink.is_empty() { connection.default_sink.clone() } else { sink.to_owned() };
        watch_sink(&connection, &watch, &name);
        let found = watch.watched.load(Ordering::Acquire) != u32::MAX;
        if found {
            // Where the stream really goes, if it already plays.
            retarget(&connection, &watch);
            connection.roundtrip(); // the first Props (the volume) arrive before this returns
        }
        connection.unlock();
        let volume = SinkVolume { connection, watch };
        found.then_some(volume)
    }

    /// Linear amplitude (1 = 100 %) of the watched sink: 0 while muted, 1
    /// until its Props are known.
    pub fn volume(&self) -> f64 {
        if !self.watch.known.load(Ordering::Acquire) {
            return 1.0;
        }
        if self.watch.muted.load(Ordering::Acquire) {
            return 0.0;
        }
        (f32::from_bits(self.watch.volume.load(Ordering::Acquire)) as f64).clamp(0.0, 1.0)
    }

    /// Sets the sink's volume, its channels alike: 0 mutes, above 0 unmutes.
    pub fn set_volume(&self, linear: f64) {
        if !linear.is_finite() {
            return;
        }
        let v = linear.clamp(0.0, 1.0) as f32;
        self.connection.lock();
        let node = self.watch.node.load(Ordering::Acquire);
        if !node.is_null() {
            let n = (self.watch.channels.load(Ordering::Acquire) as usize).clamp(1, SPA_AUDIO_MAX_CHANNELS);
            let pod = props_pod(&vec![v; n], v <= 0.0);
            unsafe {
                let (m, o) = iface::<NodeMethods>(node);
                (m.set_param)(o, SPA_PARAM_PROPS, 0, pod.as_ptr() as *const c_void);
            }
        }
        self.connection.unlock();
    }

    /// The device's own control carries all of the volume now (no software
    /// scaling left: softVolumes at 1). False for a virtual sink.
    pub fn hardware(&self) -> bool {
        self.watch.device.load(Ordering::Acquire) && self.watch.soft_unity.load(Ordering::Acquire)
    }

    /// The watched sink belongs to a device (its volume may sit in hardware).
    pub fn device(&self) -> bool {
        self.watch.device.load(Ordering::Acquire)
    }

    /// A sink is watched (false once it went away with no default to take its place).
    pub fn watching(&self) -> bool {
        self.watch.watched.load(Ordering::Acquire) != u32::MAX
    }

    /// What the desktop calls the watched sink.
    pub fn sink_description(&self) -> String {
        self.watch.description.lock().unwrap().clone()
    }

    /// The watched sink's node.name.
    pub fn sink_name(&self) -> String {
        self.watch.watched_name.lock().unwrap().clone()
    }

    /// Called from PipeWire's thread whenever the watched sink's volume or
    /// mute changed (from here or from outside: volume keys, pavucontrol,
    /// wpctl), and when another sink is watched (the default changed under a
    /// "" sink, the sink went away or came back).
    pub fn set_on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.watch.on_change.lock().unwrap() = Some(Arc::new(f));
    }
}

impl Drop for SinkVolume {
    fn drop(&mut self) {
        self.connection.lock();
        *self.watch.on_change.lock().unwrap() = None;
        self.connection.volume.store(null_mut(), Ordering::Release);
        unwatch_sink(&self.connection, &self.watch);
        self.connection.unlock();
        self.connection.disconnect();
    }
}

// --- The stream ---------------------------------------------------------------------

/// The format the stream offers (spa_format_audio_raw_build's POD): S32,
/// the rate, the channels and their positions in WAVE / FLAC order.
pub fn format_pod(rate: u32, channels: u32) -> Vec<u8> {
    const LAYOUTS: [[u32; 8]; 9] = [
        [0; 8],
        [2, 0, 0, 0, 0, 0, 0, 0],
        [3, 4, 0, 0, 0, 0, 0, 0],
        [3, 4, 5, 0, 0, 0, 0, 0],
        [3, 4, 12, 13, 0, 0, 0, 0],
        [3, 4, 5, 12, 13, 0, 0, 0],
        [3, 4, 5, 6, 12, 13, 0, 0],
        [3, 4, 5, 6, 11, 7, 8, 0],
        [3, 4, 5, 6, 12, 13, 7, 8],
    ];
    let mut body = Vec::new();
    let mut prop = |key: u32, value_type: u32, value: &[u32]| {
        body.extend_from_slice(&key.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&value_type.to_le_bytes());
        body.extend_from_slice(&value[0].to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // to 8 bytes
    };
    prop(1, 3, &[1]); // mediaType: audio (Id)
    prop(2, 3, &[1]); // mediaSubtype: raw (Id)
    prop(0x10001, 3, &[0x10b]); // format: S32 (Id)
    prop(0x10003, 4, &[rate]); // rate (Int)
    prop(0x10004, 4, &[channels]); // channels (Int)
    // position: an Array of Ids.
    let n = channels.min(8) as usize;
    let array_size = 8 + 4 * n as u32;
    body.extend_from_slice(&0x10005u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&array_size.to_le_bytes());
    body.extend_from_slice(&13u32.to_le_bytes()); // Array
    body.extend_from_slice(&4u32.to_le_bytes()); // child size
    body.extend_from_slice(&3u32.to_le_bytes()); // child type: Id
    for &p in &LAYOUTS[n][..n] {
        body.extend_from_slice(&p.to_le_bytes());
    }
    while body.len() % 8 != 0 {
        body.push(0);
    }
    let mut pod = Vec::with_capacity(16 + body.len());
    pod.extend_from_slice(&(8 + body.len() as u32).to_le_bytes()); // object body size
    pod.extend_from_slice(&15u32.to_le_bytes()); // Object
    pod.extend_from_slice(&0x40003u32.to_le_bytes()); // Format
    pod.extend_from_slice(&3u32.to_le_bytes()); // EnumFormat
    pod.extend(body);
    pod
}

/// What the real-time callback reads.
struct StreamData {
    api: &'static Api,
    stream: AtomicPtr,
    render: Arc<dyn Render>,
    channels: u32,
    quantum: u32,
    running: AtomicBool,
    in_callback: AtomicI32,
    state: AtomicI32,
    /// pw_buffer.requested exists in this library (has_requested).
    requested_ok: bool,
    lp: *mut c_void,
    position: std::sync::atomic::AtomicPtr<c_void>,
    graph_rate: AtomicU32,
}

struct AtomicPtr(std::sync::atomic::AtomicPtr<c_void>);

static STREAM_EVENTS: StreamEvents = StreamEvents {
    version: PW_VERSION_STREAM_EVENTS,
    destroy: None,
    state_changed: Some(on_state),
    control_info: None,
    io_changed: Some(on_io),
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

unsafe extern "C" fn on_state(data: *mut c_void, _old: c_int, state: c_int, _err: *const c_char) {
    let d = unsafe { &*(data as *const StreamData) };
    d.state.store(state, Ordering::Release);
    unsafe { (d.api.thread_loop_signal)(d.lp, false) };
}

unsafe extern "C" fn on_io(data: *mut c_void, id: u32, area: *mut c_void, _size: u32) {
    if id == SPA_IO_POSITION {
        let d = unsafe { &*(data as *const StreamData) };
        d.position.store(area, Ordering::Release);
    }
}

/// The real-time data thread: one buffer from the engine.
unsafe extern "C" fn on_process(data: *mut c_void) {
    let d = unsafe { &*(data as *const StreamData) };
    let stream = d.stream.0.load(Ordering::Acquire);
    if stream.is_null() {
        return;
    }
    unsafe {
        let b = (d.api.stream_dequeue_buffer)(stream);
        if b.is_null() {
            return;
        }
        let buffer = &*(*b).buffer;
        if buffer.n_datas == 0 {
            (d.api.stream_queue_buffer)(stream, b);
            return;
        }
        let sd = &mut *buffer.datas;
        if sd.data.is_null() || sd.chunk.is_null() {
            (d.api.stream_queue_buffer)(stream, b);
            return;
        }
        let stride = 4 * d.channels;
        let mut frames = sd.maxsize / stride;
        if d.requested_ok && (*b).requested > 0 {
            frames = frames.min((*b).requested as u32);
        } else {
            frames = frames.min(d.quantum);
        }
        let out = std::slice::from_raw_parts_mut(sd.data as *mut i32, (frames * d.channels) as usize);
        d.in_callback.fetch_add(1, Ordering::AcqRel);
        if d.running.load(Ordering::Acquire) {
            d.render.render(out);
        } else {
            out.fill(0);
        }
        d.in_callback.fetch_sub(1, Ordering::AcqRel);
        let position = d.position.load(Ordering::Acquire);
        if !position.is_null() {
            // spa_io_position.clock.rate.denom (offset 84).
            let denom = *((position as *const u8).add(84) as *const u32);
            d.graph_rate.store(denom, Ordering::Relaxed);
        }
        let chunk = &mut *sd.chunk;
        chunk.offset = 0;
        chunk.stride = stride as i32;
        chunk.size = frames * stride;
        (*b).size = frames as u64;
        (d.api.stream_queue_buffer)(stream, b);
    }
}

pub struct PipeWireOutput {
    key: String,
    format: DeviceFormat,
    connection: Box<Connection>,
    data: Box<StreamData>,
    hook: Box<SpaHook>,
    forced_rate: u32,
}

// The raw pointers are PipeWire's, used under its loop lock.
unsafe impl Send for PipeWireOutput {}
unsafe impl Send for Connection {}
unsafe impl Sync for StreamData {}

impl PipeWireOutput {
    /// `target`: a sink's node.name, "" for the desktop's default.
    pub fn open(key: &str, target: &str, request: Request, render: Arc<dyn Render>) -> Result<Self> {
        if request.channels == 0 || request.channels > 8 || request.rate == 0 {
            return Err(OutputError("PipeWire: unsupported format".into()));
        }
        let mut c = Connection::connect()?;
        let api = c.api;
        c.lock();
        // The rate to open at: the track's when the graph follows it (or may
        // switch to it by itself); else the graph's.
        let allowed = rate_list(&c.setting("clock.allowed-rates"));
        let mut rate = request.rate;
        let mut forced_rate = 0;
        if request.follow_rate {
            if c.setting("clock.force-rate") != rate.to_string() {
                c.set_setting("clock.force-rate", &rate.to_string());
            }
            forced_rate = rate;
        } else if !allowed.contains(&rate) {
            let forced: u32 = c.setting("clock.force-rate").parse().unwrap_or(0);
            let base: u32 = c.setting("clock.rate").parse().unwrap_or(0);
            rate = if forced != 0 {
                forced
            } else if base != 0 {
                base
            } else {
                48000
            };
        }
        // About 40 ms a cycle, as the other outputs.
        let quantum = (rate / 25).max(1).next_power_of_two().clamp(256, 8192);
        let channels = request.channels as u32;
        let props_text = format!(
            "media.type=Audio media.category=Playback media.role=Music application.name=Sonkore application.icon-name=sonkore \
             node.name=sonkore node.description=Sonkore stream.dont-remix=true state.restore-props=false node.rate=1/{rate} \
             node.latency={quantum}/{rate}"
        );
        let props_text = CString::new(props_text).unwrap();
        let data = Box::new(StreamData {
            api,
            stream: AtomicPtr(std::sync::atomic::AtomicPtr::new(null_mut())),
            render,
            channels,
            quantum,
            running: AtomicBool::new(false),
            in_callback: AtomicI32::new(0),
            state: AtomicI32::new(STATE_UNCONNECTED),
            requested_ok: has_requested(&library_version()),
            lp: c.lp,
            position: std::sync::atomic::AtomicPtr::new(null_mut()),
            graph_rate: AtomicU32::new(0),
        });
        let mut hook = Box::new(SpaHook::zeroed());
        unsafe {
            let props = (api.properties_new_string)(props_text.as_ptr());
            if !target.is_empty() {
                let t = CString::new(target).unwrap();
                (api.properties_set)(props, c"target.object".as_ptr(), t.as_ptr());
                (api.properties_set)(props, c"node.target".as_ptr(), t.as_ptr());
            }
            let stream = (api.stream_new)(c.core, c"Sonkore".as_ptr(), props);
            if stream.is_null() {
                c.unlock();
                c.disconnect();
                return Err(OutputError("PipeWire refused the stream".into()));
            }
            data.stream.0.store(stream, Ordering::Release);
            (api.stream_add_listener)(stream, &mut *hook, &STREAM_EVENTS, &*data as *const StreamData as *mut c_void);
            let pod = format_pod(rate, channels);
            let mut params = [pod.as_ptr() as *const c_void];
            let flags = PW_STREAM_FLAG_AUTOCONNECT
                | PW_STREAM_FLAG_MAP_BUFFERS
                | PW_STREAM_FLAG_RT_PROCESS
                | PW_STREAM_FLAG_INACTIVE;
            if (api.stream_connect)(stream, PW_DIRECTION_OUTPUT, PW_ID_ANY, flags, params.as_mut_ptr(), 1) < 0 {
                hook_remove(&mut *hook);
                (api.stream_destroy)(stream);
                c.unlock();
                c.disconnect();
                return Err(OutputError("PipeWire refused the stream".into()));
            }
            // Linked to a sink and the format agreed (or not).
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(data.state.load(Ordering::Acquire), STATE_UNCONNECTED | STATE_CONNECTING)
                && !c.failed.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                (api.thread_loop_timed_wait)(c.lp, 1);
            }
            let state = data.state.load(Ordering::Acquire);
            if state != STATE_PAUSED && state != STATE_STREAMING {
                hook_remove(&mut *hook);
                (api.stream_destroy)(stream);
                c.unlock();
                c.disconnect();
                let why = if c.failed.load(Ordering::Acquire) || state == STATE_ERROR {
                    "PipeWire stopped"
                } else {
                    "PipeWire found no output for the stream"
                };
                return Err(OutputError(why.into()));
            }
            // Nothing between the engine and the sink scales the samples.
            let mut unity = [1f32; 8];
            let mut unmuted = 0f32;
            (api.stream_set_control)(
                stream,
                SPA_PROP_CHANNEL_VOLUMES,
                channels,
                unity.as_mut_ptr(),
                SPA_PROP_MUTE,
                1u32,
                &mut unmuted as *mut f32,
                0u32,
            );
            data.running.store(true, Ordering::Release);
            (api.stream_set_active)(stream, true);
            c.unlock();
        }
        let format = DeviceFormat {
            rate,
            channels: request.channels,
            bits: 32,
            // PipeWire carries f32: integer samples up to 24 bits survive.
            float: true,
            mono_to_stereo: false,
            // Nothing else is mixed in on the way (graph rate and volume permitting).
            exclusive: true,
            period_frames: quantum as usize,
            buffer_frames: 4 * quantum as usize,
            dsd: None,
        };
        Ok(PipeWireOutput { key: key.to_owned(), format, connection: c, data, hook, forced_rate })
    }

    pub(crate) fn set_mono_to_stereo(&mut self, on: bool) {
        self.format.mono_to_stereo = on;
    }

    /// The rate the graph runs at now (0: not known yet).
    pub fn graph_rate(&self) -> u32 {
        self.data.graph_rate.load(Ordering::Relaxed)
    }
}

impl Output for PipeWireOutput {
    fn format(&self) -> DeviceFormat {
        self.format
    }
    fn latency_frames(&self) -> u64 {
        self.format.buffer_frames as u64
    }
    fn key(&self) -> &str {
        &self.key
    }
    /// The stream went into its error state, or the connection to PipeWire
    /// broke (the daemon stopped): no more cycles come.
    fn failed(&self) -> bool {
        self.data.state.load(Ordering::Acquire) == STATE_ERROR || self.connection.failed.load(Ordering::Acquire)
    }
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        let api = self.connection.api;
        self.data.running.store(false, Ordering::Release);
        let stream = self.data.stream.0.load(Ordering::Acquire);
        self.connection.lock();
        unsafe { (api.stream_set_active)(stream, false) };
        self.connection.unlock();
        // A cycle that saw `running` before it was cleared ends first.
        while self.data.in_callback.load(Ordering::Acquire) != 0 {
            std::thread::sleep(Duration::from_micros(200));
        }
        self.connection.lock();
        unsafe {
            hook_remove(&mut *self.hook);
            self.data.stream.0.store(null_mut(), Ordering::Release);
            (api.stream_destroy)(stream);
        }
        // The graph's rate given back, unless someone else has set it since.
        if self.forced_rate != 0 && self.connection.setting("clock.force-rate") == self.forced_rate.to_string() {
            self.connection.set_setting("clock.force-rate", "0");
        }
        self.connection.unlock();
        self.connection.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linked_sink_is_the_one_heard() {
        // Stream 40 linked (two channels) to sink 7, and recorded by node 90.
        let links = [(100, 40, 90), (101, 40, 7), (102, 40, 7), (103, 55, 8)];
        assert_eq!(linked_sink(&[40], &links, &[7, 8]), Some(7));
        assert_eq!(linked_sink(&[41], &links, &[7, 8]), None, "not our stream");
        assert_eq!(linked_sink(&[40], &links[..1], &[7, 8]), None, "a recorder is no sink");
        assert_eq!(linked_sink(&[], &links, &[7, 8]), None);
    }

    #[test]
    fn format_pod_matches_spa() {
        // spa_format_audio_raw_build(EnumFormat, S32, 44100, 2, FL FR) from
        // PipeWire 1.6's headers.
        let reference = "a0 00 00 00 0f 00 00 00 03 00 04 00 03 00 00 00 01 00 00 00 00 00 00 00 04 00 00 00 03 00 00 00 01 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00 04 00 00 00 03 00 00 00 01 00 00 00 00 00 00 00 01 00 01 00 00 00 00 00 04 00 00 00 03 00 00 00 0b 01 00 00 00 00 00 00 03 00 01 00 00 00 00 00 04 00 00 00 04 00 00 00 44 ac 00 00 00 00 00 00 04 00 01 00 00 00 00 00 04 00 00 00 04 00 00 00 02 00 00 00 00 00 00 00 05 00 01 00 00 00 00 00 10 00 00 00 0d 00 00 00 04 00 00 00 03 00 00 00 03 00 00 00 04 00 00 00";
        let bytes: Vec<u8> = reference.split(' ').map(|h| u8::from_str_radix(h, 16).unwrap()).collect();
        assert_eq!(format_pod(44100, 2), bytes);
    }

    #[test]
    fn props_pod_matches_spa() {
        // spa_pod_builder_add_object(Props, Props, channelVolumes [0.25 x3],
        // mute true) from PipeWire 1.6's headers.
        let reference = "48 00 00 00 0f 00 00 00 02 00 04 00 02 00 00 00 08 00 01 00 00 00 00 00 14 00 00 00 0d 00 00 00 04 00 00 00 06 00 00 00 00 00 80 3e 00 00 80 3e 00 00 80 3e 00 00 00 00 04 00 01 00 00 00 00 00 04 00 00 00 02 00 00 00 01 00 00 00 00 00 00 00";
        let bytes: Vec<u8> = reference.split(' ').map(|h| u8::from_str_radix(h, 16).unwrap()).collect();
        assert_eq!(props_pod(&[0.25; 3], true), bytes);
        let info = parse_props(&bytes).unwrap();
        assert_eq!(
            info,
            PropsInfo { channel_volumes: Some(vec![0.25; 3]), soft_volumes: None, mute: Some(true), soft_mute: None }
        );
        assert!(parse_props(&bytes[..40]).is_none());
        assert!(parse_props(&format_pod(44100, 2)).is_none()); // a Format object, not Props
    }

    #[test]
    fn requested_is_read_from_0_3_50() {
        for (version, ok) in [
            ("1.6.8", true),
            ("0.3.50", true),
            ("0.3.65", true),
            ("0.4.0", true),
            ("0.3.49", false),
            ("0.3.48", false),
            ("0.2.7", false),
            ("1.0", true),
            ("", false),
            ("garbage", false),
            ("1.2.3-rc1", true),
        ] {
            assert_eq!(has_requested(version), ok, "{version}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(base_sink_name("alsa_output.usb-DX1.3"), "alsa_output.usb-DX1");
        assert_eq!(base_sink_name("alsa_output.usb-DX1"), "alsa_output.usb-DX1");
        assert_eq!(json_name(r#"{ "name": "alsa_output.x" }"#), "alsa_output.x");
        assert_eq!(rate_list("[ 44100, 48000 ]"), vec![44100, 48000]);
    }
}
