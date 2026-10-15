#include "PipeWireOutput.hpp"

#if defined(SONKORE_HAVE_PIPEWIRE)
#include <pipewire/extensions/metadata.h>
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/props.h>
#include <spa/pod/iter.h>

#include <dlfcn.h>

#include <algorithm>
#include <atomic>
#include <bit>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <thread>
#include <type_traits>
#endif

namespace sonkore {

#if defined(SONKORE_HAVE_PIPEWIRE)

namespace {

// libpipewire, resolved at run time: the program does not need it to start,
// and uses whichever version the system's daemon came with.
struct Api {
    bool ok = false;
    bool hasRequested = false; // pw_buffer::requested (0.3.49+)
#define SONKORE_PW_FN(name) decltype(&::name) name = nullptr
    SONKORE_PW_FN(pw_init);
    SONKORE_PW_FN(pw_get_library_version);
    SONKORE_PW_FN(pw_thread_loop_new);
    SONKORE_PW_FN(pw_thread_loop_destroy);
    SONKORE_PW_FN(pw_thread_loop_start);
    SONKORE_PW_FN(pw_thread_loop_stop);
    SONKORE_PW_FN(pw_thread_loop_lock);
    SONKORE_PW_FN(pw_thread_loop_unlock);
    SONKORE_PW_FN(pw_thread_loop_timed_wait);
    SONKORE_PW_FN(pw_thread_loop_signal);
    SONKORE_PW_FN(pw_thread_loop_get_loop);
    SONKORE_PW_FN(pw_context_new);
    SONKORE_PW_FN(pw_context_destroy);
    SONKORE_PW_FN(pw_context_connect);
    SONKORE_PW_FN(pw_core_disconnect);
    SONKORE_PW_FN(pw_properties_new);
    SONKORE_PW_FN(pw_properties_set);
    SONKORE_PW_FN(pw_proxy_destroy);
    SONKORE_PW_FN(pw_stream_new);
    SONKORE_PW_FN(pw_stream_add_listener);
    SONKORE_PW_FN(pw_stream_connect);
    SONKORE_PW_FN(pw_stream_destroy);
    SONKORE_PW_FN(pw_stream_dequeue_buffer);
    SONKORE_PW_FN(pw_stream_queue_buffer);
    SONKORE_PW_FN(pw_stream_set_active);
    SONKORE_PW_FN(pw_stream_set_control);
#undef SONKORE_PW_FN
};

const Api& api()
{
    static const Api loaded = [] {
        Api a;
        void* lib = dlopen("libpipewire-0.3.so.0", RTLD_NOW | RTLD_LOCAL);
        if (!lib)
            return a;
        bool all = true;
        const auto load = [&](auto& fn, const char* name) {
            fn = reinterpret_cast<std::remove_reference_t<decltype(fn)>>(dlsym(lib, name));
            all = all && fn != nullptr;
        };
#define SONKORE_PW_LOAD(name) load(a.name, #name)
        SONKORE_PW_LOAD(pw_init);
        SONKORE_PW_LOAD(pw_get_library_version);
        SONKORE_PW_LOAD(pw_thread_loop_new);
        SONKORE_PW_LOAD(pw_thread_loop_destroy);
        SONKORE_PW_LOAD(pw_thread_loop_start);
        SONKORE_PW_LOAD(pw_thread_loop_stop);
        SONKORE_PW_LOAD(pw_thread_loop_lock);
        SONKORE_PW_LOAD(pw_thread_loop_unlock);
        SONKORE_PW_LOAD(pw_thread_loop_timed_wait);
        SONKORE_PW_LOAD(pw_thread_loop_signal);
        SONKORE_PW_LOAD(pw_thread_loop_get_loop);
        SONKORE_PW_LOAD(pw_context_new);
        SONKORE_PW_LOAD(pw_context_destroy);
        SONKORE_PW_LOAD(pw_context_connect);
        SONKORE_PW_LOAD(pw_core_disconnect);
        SONKORE_PW_LOAD(pw_properties_new);
        SONKORE_PW_LOAD(pw_properties_set);
        SONKORE_PW_LOAD(pw_proxy_destroy);
        SONKORE_PW_LOAD(pw_stream_new);
        SONKORE_PW_LOAD(pw_stream_add_listener);
        SONKORE_PW_LOAD(pw_stream_connect);
        SONKORE_PW_LOAD(pw_stream_destroy);
        SONKORE_PW_LOAD(pw_stream_dequeue_buffer);
        SONKORE_PW_LOAD(pw_stream_queue_buffer);
        SONKORE_PW_LOAD(pw_stream_set_active);
        SONKORE_PW_LOAD(pw_stream_set_control);
#undef SONKORE_PW_LOAD
        if (!all) {
            dlclose(lib);
            return Api{};
        }
        a.pw_init(nullptr, nullptr);
        int major = 0, minor = 0, micro = 0;
        std::sscanf(a.pw_get_library_version(), "%d.%d.%d", &major, &minor, &micro);
        a.hasRequested = major > 0 || minor > 3 || (minor == 3 && micro >= 49);
        a.ok = true;
        return a;
    }();
    return loaded;
}

// The channel positions of a stream in WAVE / FLAC order (as the engine
// hands them over).
void channelPositions(uint32_t channels, uint32_t* position)
{
    static const uint32_t layouts[9][8] = {
        {},
        {SPA_AUDIO_CHANNEL_MONO},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_RL, SPA_AUDIO_CHANNEL_RR},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_RL, SPA_AUDIO_CHANNEL_RR},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_RL,
         SPA_AUDIO_CHANNEL_RR},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_RC,
         SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR},
        {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_LFE, SPA_AUDIO_CHANNEL_RL,
         SPA_AUDIO_CHANNEL_RR, SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR},
    };
    for (uint32_t c = 0; c < channels && c < 8; ++c)
        position[c] = layouts[std::min<uint32_t>(channels, 8)][c];
}

// `{"name":"alsa_output.usb-..."}` (the "default" metadata's JSON) -> the name.
std::string jsonName(const char* json)
{
    if (!json)
        return {};
    const std::string s = json;
    auto p = s.find("\"name\"");
    if (p == std::string::npos)
        return {};
    p = s.find(':', p);
    if (p == std::string::npos)
        return {};
    p = s.find('"', p);
    if (p == std::string::npos)
        return {};
    const auto e = s.find('"', p + 1);
    return e == std::string::npos ? std::string() : s.substr(p + 1, e - p - 1);
}

// "[ 44100, 48000 ]" -> its rates.
std::vector<uint32_t> rateList(const std::string& s)
{
    std::vector<uint32_t> rates;
    uint32_t v = 0;
    bool in = false;
    for (char c : s + " ") {
        if (c >= '0' && c <= '9') {
            v = v * 10 + static_cast<uint32_t>(c - '0');
            in = true;
        } else if (in) {
            rates.push_back(v);
            v = 0;
            in = false;
        }
    }
    return rates;
}

bool allUnity(const float* v, uint32_t n)
{
    return std::all_of(v, v + n, [](float x) { return std::fabs(x - 1.0f) < 1e-6f; });
}

struct SinkNode {
    uint32_t id = SPA_ID_INVALID;
    std::string name, description;
    bool hardware = false; // a device's node (ALSA...): its volume may be split between hardware and software
};

// One connection to the daemon: the registry, the "settings" and "default"
// metadata, and the volume of the sink being played to.
struct Connection {
    const Api& P = api();
    pw_thread_loop* loop = nullptr;
    pw_context* context = nullptr;
    pw_core* core = nullptr;
    pw_registry* registry = nullptr;
    spa_hook coreHook{}, registryHook{};
    int syncSeq = 0;
    bool synced = false;
    bool failed = false;

    std::vector<SinkNode> sinks;
    uint32_t settingsId = SPA_ID_INVALID, defaultsId = SPA_ID_INVALID;
    pw_metadata* settings = nullptr;
    pw_metadata* defaults = nullptr;
    spa_hook settingsHook{}, defaultsHook{};
    std::map<std::string, std::string> settingValues;
    std::string defaultSink;

    pw_node* sinkNode = nullptr;
    spa_hook sinkHook{};
    uint32_t watchedSink = SPA_ID_INVALID;
    bool watchedHardware = false;
    std::atomic<bool> sinkNeutral{true};
    // The watched sink's volume as last reported (for the volume control).
    std::atomic<float> sinkVolume{1.0f};
    std::atomic<bool> sinkMuted{false};
    std::atomic<bool> sinkVolumeKnown{false};
    std::atomic<bool> sinkSoftUnity{true}; // softVolumes all at 1: the hardware carries the volume
    std::atomic<uint32_t> sinkChannels{2};
    std::string watchedName;       // what watch() asked for ("" = the default)
    std::string watchedDescription;
    bool followDefault = false;    // re-watch when the default sink changes
    std::function<void()> sinkChanged;

    static void onCoreDone(void* data, uint32_t id, int seq)
    {
        auto* c = static_cast<Connection*>(data);
        if (id == PW_ID_CORE && seq == c->syncSeq) {
            c->synced = true;
            c->P.pw_thread_loop_signal(c->loop, false);
        }
    }
    static void onCoreError(void* data, uint32_t id, int /*seq*/, int res, const char* /*message*/)
    {
        auto* c = static_cast<Connection*>(data);
        if (id == PW_ID_CORE && res == -EPIPE) { // the daemon went away
            c->failed = true;
            c->P.pw_thread_loop_signal(c->loop, false);
        }
    }
    static void onGlobal(void* data, uint32_t id, uint32_t /*permissions*/, const char* type, uint32_t /*version*/,
                         const spa_dict* props)
    {
        auto* c = static_cast<Connection*>(data);
        if (!props)
            return;
        if (std::strcmp(type, PW_TYPE_INTERFACE_Node) == 0) {
            const char* cls = spa_dict_lookup(props, PW_KEY_MEDIA_CLASS);
            const char* name = spa_dict_lookup(props, PW_KEY_NODE_NAME);
            if (!cls || !name || std::strcmp(cls, "Audio/Sink") != 0)
                return;
            const char* desc = spa_dict_lookup(props, PW_KEY_NODE_DESCRIPTION);
            c->sinks.push_back({id, name, desc ? desc : name, spa_dict_lookup(props, PW_KEY_DEVICE_ID) != nullptr});
        } else if (std::strcmp(type, PW_TYPE_INTERFACE_Metadata) == 0) {
            const char* name = spa_dict_lookup(props, PW_KEY_METADATA_NAME);
            if (name && std::strcmp(name, "settings") == 0)
                c->settingsId = id;
            else if (name && std::strcmp(name, "default") == 0)
                c->defaultsId = id;
        }
    }
    static void onGlobalRemove(void* data, uint32_t id)
    {
        auto* c = static_cast<Connection*>(data);
        std::erase_if(c->sinks, [id](const SinkNode& s) { return s.id == id; });
    }
    static int onSetting(void* data, uint32_t subject, const char* key, const char* /*type*/, const char* value)
    {
        auto* c = static_cast<Connection*>(data);
        if (subject == PW_ID_CORE && key)
            c->settingValues[key] = value ? value : "";
        return 0;
    }
    static int onDefault(void* data, uint32_t subject, const char* key, const char* /*type*/, const char* value)
    {
        auto* c = static_cast<Connection*>(data);
        if (subject == PW_ID_CORE && key && std::strcmp(key, "default.audio.sink") == 0) {
            c->defaultSink = jsonName(value);
            if (c->followDefault && c->synced) { // the desktop chose another output: follow it
                c->watchSink(c->defaultSink);
                if (c->sinkChanged)
                    c->sinkChanged();
            }
        }
        return 0;
    }
    // The sink's Props: its software volume. A device's node (ALSA) splits
    // the volume: channelVolumes is what the user set, softVolumes the part
    // left to software once the hardware mixer has done what it can. Any
    // other sink (virtual, a filter) applies channelVolumes in software.
    static void onSinkParam(void* data, int /*seq*/, uint32_t id, uint32_t /*index*/, uint32_t /*next*/, const spa_pod* param)
    {
        auto* c = static_cast<Connection*>(data);
        if (id != SPA_PARAM_Props || !param || !spa_pod_is_object(param))
            return;
        bool haveVolume = false, haveSoft = false, haveSoftMute = false;
        bool volumeOk = true, softOk = true, mute = false, softMute = false;
        float v[SPA_AUDIO_MAX_CHANNELS];
        const spa_pod_prop* prop;
        const auto* obj = reinterpret_cast<const spa_pod_object*>(param);
        SPA_POD_OBJECT_FOREACH(obj, prop)
        {
            switch (prop->key) {
            case SPA_PROP_channelVolumes: {
                haveVolume = true;
                const uint32_t n = spa_pod_copy_array(&prop->value, SPA_TYPE_Float, v, SPA_AUDIO_MAX_CHANNELS);
                volumeOk = allUnity(v, n);
                if (n > 0) {
                    c->sinkVolume.store(*std::max_element(v, v + n), std::memory_order_relaxed);
                    c->sinkChannels.store(n, std::memory_order_relaxed);
                }
                break;
            }
            case SPA_PROP_softVolumes:
                haveSoft = true;
                softOk = allUnity(v, spa_pod_copy_array(&prop->value, SPA_TYPE_Float, v, SPA_AUDIO_MAX_CHANNELS));
                break;
            case SPA_PROP_mute:
                spa_pod_get_bool(&prop->value, &mute);
                break;
            case SPA_PROP_softMute:
                haveSoftMute = true;
                spa_pod_get_bool(&prop->value, &softMute);
                break;
            default:
                break;
            }
        }
        if (!haveVolume && !haveSoft)
            return; // some other Props object
        const bool split = c->watchedHardware && haveSoft;
        const bool muted = c->watchedHardware && haveSoftMute ? softMute : mute;
        c->sinkNeutral.store((split ? softOk : volumeOk) && !muted, std::memory_order_relaxed);
        c->sinkSoftUnity.store(split ? softOk : volumeOk, std::memory_order_relaxed);
        if (haveVolume) {
            c->sinkMuted.store(mute, std::memory_order_relaxed);
            c->sinkVolumeKnown.store(true, std::memory_order_release);
        }
        if (c->sinkChanged)
            c->sinkChanged();
    }

    static const pw_core_events* coreEvents()
    {
        static const pw_core_events e = [] {
            pw_core_events ev{};
            ev.version = PW_VERSION_CORE_EVENTS;
            ev.done = &onCoreDone;
            ev.error = &onCoreError;
            return ev;
        }();
        return &e;
    }
    static const pw_registry_events* registryEvents()
    {
        static const pw_registry_events e = [] {
            pw_registry_events ev{};
            ev.version = PW_VERSION_REGISTRY_EVENTS;
            ev.global = &onGlobal;
            ev.global_remove = &onGlobalRemove;
            return ev;
        }();
        return &e;
    }
    static const pw_metadata_events* settingEvents()
    {
        static const pw_metadata_events e = [] {
            pw_metadata_events ev{};
            ev.version = PW_VERSION_METADATA_EVENTS;
            ev.property = &onSetting;
            return ev;
        }();
        return &e;
    }
    static const pw_metadata_events* defaultEvents()
    {
        static const pw_metadata_events e = [] {
            pw_metadata_events ev{};
            ev.version = PW_VERSION_METADATA_EVENTS;
            ev.property = &onDefault;
            return ev;
        }();
        return &e;
    }
    static const pw_node_events* nodeEvents()
    {
        static const pw_node_events e = [] {
            pw_node_events ev{};
            ev.version = PW_VERSION_NODE_EVENTS;
            ev.param = &onSinkParam;
            return ev;
        }();
        return &e;
    }

    bool connected() const { return core != nullptr; }
    void lock() { P.pw_thread_loop_lock(loop); }
    void unlock() { P.pw_thread_loop_unlock(loop); }

    // Everything sent so far has been handled by the daemon (lock held).
    bool roundtrip()
    {
        synced = false;
        syncSeq = pw_core_sync(core, PW_ID_CORE, syncSeq);
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
        while (!synced && !failed && std::chrono::steady_clock::now() < deadline)
            P.pw_thread_loop_timed_wait(loop, 1);
        return synced && !failed;
    }

    bool connect(std::string* error)
    {
        if (core && !failed)
            return true;
        if (core)
            disconnect(); // the daemon went away: a new connection
        if (!P.ok) {
            if (error)
                *error = "libpipewire is not installed";
            return false;
        }
        loop = P.pw_thread_loop_new("sonkore-pipewire", nullptr);
        if (!loop)
            return false;
        context = P.pw_context_new(P.pw_thread_loop_get_loop(loop), nullptr, 0);
        if (!context || P.pw_thread_loop_start(loop) < 0) {
            disconnect();
            return false;
        }
        lock();
        core = P.pw_context_connect(context, nullptr, 0);
        if (!core) {
            unlock();
            disconnect();
            if (error)
                *error = "PipeWire is not running";
            return false;
        }
        pw_core_add_listener(core, &coreHook, coreEvents(), this);
        registry = pw_core_get_registry(core, PW_VERSION_REGISTRY, 0);
        pw_registry_add_listener(registry, &registryHook, registryEvents(), this);
        bool ok = roundtrip();
        if (ok && settingsId != SPA_ID_INVALID) {
            settings = static_cast<pw_metadata*>(
                pw_registry_bind(registry, settingsId, PW_TYPE_INTERFACE_Metadata, PW_VERSION_METADATA, 0));
            if (settings)
                pw_metadata_add_listener(settings, &settingsHook, settingEvents(), this);
        }
        if (ok && defaultsId != SPA_ID_INVALID) {
            defaults = static_cast<pw_metadata*>(
                pw_registry_bind(registry, defaultsId, PW_TYPE_INTERFACE_Metadata, PW_VERSION_METADATA, 0));
            if (defaults)
                pw_metadata_add_listener(defaults, &defaultsHook, defaultEvents(), this);
        }
        ok = ok && roundtrip();
        unlock();
        if (!ok) {
            disconnect();
            if (error)
                *error = "PipeWire does not answer";
        }
        return ok;
    }

    // Watches the volume of the sink called `name` (lock held).
    void watchSink(const std::string& name)
    {
        uint32_t id = SPA_ID_INVALID;
        bool hardware = false;
        for (const SinkNode& s : sinks)
            if (s.name == name) {
                id = s.id;
                hardware = s.hardware;
            }
        if (id == watchedSink)
            return;
        unwatchSink();
        sinkNeutral.store(true, std::memory_order_relaxed);
        sinkVolumeKnown.store(false, std::memory_order_relaxed);
        watchedDescription.clear();
        for (const SinkNode& s : sinks)
            if (s.id == id)
                watchedDescription = s.description;
        if (id == SPA_ID_INVALID)
            return;
        sinkNode = static_cast<pw_node*>(pw_registry_bind(registry, id, PW_TYPE_INTERFACE_Node, PW_VERSION_NODE, 0));
        if (!sinkNode)
            return;
        watchedSink = id;
        watchedHardware = hardware;
        pw_node_add_listener(sinkNode, &sinkHook, nodeEvents(), this);
        uint32_t ids[] = {SPA_PARAM_Props};
        pw_node_subscribe_params(sinkNode, ids, 1);
    }
    void unwatchSink()
    {
        if (sinkNode) {
            spa_hook_remove(&sinkHook);
            P.pw_proxy_destroy(reinterpret_cast<pw_proxy*>(sinkNode));
            sinkNode = nullptr;
        }
        watchedSink = SPA_ID_INVALID;
    }

    void disconnect()
    {
        if (loop && core) {
            lock();
            unwatchSink();
            for (auto [proxy, hook] : {std::pair{reinterpret_cast<pw_proxy*>(settings), &settingsHook},
                                       std::pair{reinterpret_cast<pw_proxy*>(defaults), &defaultsHook},
                                       std::pair{reinterpret_cast<pw_proxy*>(registry), &registryHook}})
                if (proxy) {
                    spa_hook_remove(hook);
                    P.pw_proxy_destroy(proxy);
                }
            settings = defaults = nullptr;
            registry = nullptr;
            spa_hook_remove(&coreHook);
            P.pw_core_disconnect(core);
            core = nullptr;
            unlock();
        }
        if (loop)
            P.pw_thread_loop_stop(loop);
        if (context)
            P.pw_context_destroy(context);
        if (loop)
            P.pw_thread_loop_destroy(loop);
        loop = nullptr;
        context = nullptr;
        sinks.clear();
        settingValues.clear();
        defaultSink.clear();
        settingsId = defaultsId = SPA_ID_INVALID;
        failed = false;
    }

    std::string setting(const char* key) const
    {
        const auto it = settingValues.find(key);
        return it == settingValues.end() ? std::string() : it->second;
    }
    // Sets a "settings" metadata key and waits until the daemon has it.
    void setSetting(const char* key, const std::string& value)
    {
        if (!settings)
            return;
        pw_metadata_set_property(settings, PW_ID_CORE, key, nullptr, value.c_str());
        roundtrip();
    }
};

} // namespace

struct PipeWireOutput::Impl {
    Connection c;
    pw_stream* stream = nullptr;
    spa_hook streamHook{};
    pw_stream_state state = PW_STREAM_STATE_UNCONNECTED;
    Render render;
    uint32_t channels = 0;
    uint32_t rate = 0;
    uint32_t quantum = 1024;
    uint32_t forcedRate = 0; // what clock.force-rate was set to here (0: not touched)
    std::string sinkDescription;
    std::atomic<bool> running{false};
    std::atomic<int> inCallback{0};
    std::atomic<spa_io_position*> position{nullptr};
    std::atomic<uint32_t> graphRate{0};
    std::atomic<bool> streamVolumeOk{true}, streamMuteOk{true};

    static void onState(void* data, pw_stream_state, pw_stream_state state, const char*)
    {
        auto* d = static_cast<Impl*>(data);
        d->state = state;
        d->c.P.pw_thread_loop_signal(d->c.loop, false);
    }
    static void onIo(void* data, uint32_t id, void* area, uint32_t /*size*/)
    {
        if (id == SPA_IO_Position)
            static_cast<Impl*>(data)->position.store(static_cast<spa_io_position*>(area), std::memory_order_release);
    }
    // This stream's own volume (another program may change it).
    static void onControl(void* data, uint32_t id, const pw_stream_control* control)
    {
        auto* d = static_cast<Impl*>(data);
        if (!control || !control->values)
            return;
        if (id == SPA_PROP_channelVolumes)
            d->streamVolumeOk.store(allUnity(control->values, control->n_values), std::memory_order_relaxed);
        else if (id == SPA_PROP_mute && control->n_values > 0)
            d->streamMuteOk.store(control->values[0] == 0.0f, std::memory_order_relaxed);
    }
    // Real-time data thread: one buffer of `requested` frames from the engine.
    static void onProcess(void* data)
    {
        auto* d = static_cast<Impl*>(data);
        const Api& P = d->c.P;
        pw_buffer* b = P.pw_stream_dequeue_buffer(d->stream);
        if (!b)
            return;
        spa_data& sd = b->buffer->datas[0];
        if (!sd.data || !sd.chunk) {
            P.pw_stream_queue_buffer(d->stream, b);
            return;
        }
        const uint32_t stride = 4 * d->channels;
        uint32_t frames = sd.maxsize / stride;
        if (P.hasRequested && b->requested > 0)
            frames = std::min<uint32_t>(frames, static_cast<uint32_t>(b->requested));
        else
            frames = std::min(frames, d->quantum);
        d->inCallback.fetch_add(1);
        if (d->running.load())
            d->render(static_cast<int32_t*>(sd.data), frames);
        else
            std::memset(sd.data, 0, std::size_t(frames) * stride);
        d->inCallback.fetch_sub(1);
        if (const spa_io_position* pos = d->position.load(std::memory_order_acquire))
            d->graphRate.store(pos->clock.rate.denom, std::memory_order_relaxed);
        sd.chunk->offset = 0;
        sd.chunk->stride = static_cast<int32_t>(stride);
        sd.chunk->size = frames * stride;
        if (P.hasRequested)
            b->size = frames;
        P.pw_stream_queue_buffer(d->stream, b);
    }
    static const pw_stream_events* streamEvents()
    {
        static const pw_stream_events e = [] {
            pw_stream_events ev{};
            ev.version = PW_VERSION_STREAM_EVENTS;
            ev.state_changed = &onState;
            ev.io_changed = &onIo;
            ev.control_info = &onControl;
            ev.process = &onProcess;
            return ev;
        }();
        return &e;
    }

    void destroyStream() // lock held
    {
        if (!stream)
            return;
        spa_hook_remove(&streamHook);
        c.P.pw_stream_destroy(stream);
        stream = nullptr;
        position.store(nullptr);
        graphRate.store(0);
        state = PW_STREAM_STATE_UNCONNECTED;
    }
};

bool PipeWireOutput::available()
{
    return api().ok && !sinks().empty();
}

std::string PipeWireOutput::libraryVersion()
{
    return api().ok ? api().pw_get_library_version() : std::string();
}

std::vector<PipeWireOutput::Sink> PipeWireOutput::sinks()
{
    std::vector<Sink> result;
    if (!api().ok)
        return result;
    Connection c;
    if (!c.connect(nullptr))
        return result;
    c.lock();
    for (const SinkNode& s : c.sinks)
        result.push_back({s.name, s.description, s.name == c.defaultSink});
    c.unlock();
    c.disconnect();
    return result;
}

PipeWireOutput::PipeWireOutput()
    : d(std::make_unique<Impl>())
{
}

PipeWireOutput::~PipeWireOutput()
{
    release();
}

bool PipeWireOutput::open(const std::string& target, uint32_t channels, uint32_t rate, bool switchRate, Render render,
                          std::string* error)
{
    close();
    if (channels == 0 || channels > 8 || rate == 0)
        return false;
    if (!d->c.connect(error))
        return false;
    Connection& c = d->c;
    const Api& P = c.P;
    c.lock();

    // The rate to open at: the track's when the graph is asked to run at it
    // (or may switch to it by itself); otherwise the graph's, and the engine
    // converts.
    const std::vector<uint32_t> allowed = rateList(c.setting("clock.allowed-rates"));
    uint32_t streamRate = rate;
    if (switchRate) {
        if (c.setting("clock.force-rate") != std::to_string(rate))
            c.setSetting("clock.force-rate", std::to_string(rate));
        d->forcedRate = rate;
    } else {
        if (d->forcedRate != 0 && c.setting("clock.force-rate") == std::to_string(d->forcedRate))
            c.setSetting("clock.force-rate", "0");
        d->forcedRate = 0;
        if (std::find(allowed.begin(), allowed.end(), rate) == allowed.end()) {
            const uint32_t forced = static_cast<uint32_t>(std::strtoul(c.setting("clock.force-rate").c_str(), nullptr, 10));
            const uint32_t base = static_cast<uint32_t>(std::strtoul(c.setting("clock.rate").c_str(), nullptr, 10));
            streamRate = forced ? forced : base ? base : 48000;
        }
    }

    // About 40 ms a cycle, as the engine's other outputs.
    d->quantum = std::clamp<uint32_t>(std::bit_ceil(streamRate / 25), 256, 8192);
    char rateFraction[32], latency[32];
    std::snprintf(rateFraction, sizeof(rateFraction), "1/%u", streamRate);
    std::snprintf(latency, sizeof(latency), "%u/%u", d->quantum, streamRate);
    pw_properties* props = P.pw_properties_new(PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Playback",
                                               PW_KEY_MEDIA_ROLE, "Music", PW_KEY_APP_NAME, "Sonkore",
                                               PW_KEY_APP_ICON_NAME, "sonkore", PW_KEY_NODE_NAME, "sonkore",
                                               PW_KEY_NODE_DESCRIPTION, "Sonkore", PW_KEY_STREAM_DONT_REMIX, "true",
                                               "state.restore-props", "false", nullptr);
    P.pw_properties_set(props, PW_KEY_NODE_RATE, rateFraction);
    P.pw_properties_set(props, PW_KEY_NODE_LATENCY, latency);
    if (!target.empty()) {
        P.pw_properties_set(props, PW_KEY_TARGET_OBJECT, target.c_str());
        P.pw_properties_set(props, "node.target", target.c_str()); // before 0.3.64
    }
    d->stream = P.pw_stream_new(c.core, "Sonkore", props);
    if (!d->stream) {
        c.unlock();
        if (error)
            *error = "PipeWire refused the stream";
        return false;
    }
    d->streamVolumeOk.store(true);
    d->streamMuteOk.store(true);
    P.pw_stream_add_listener(d->stream, &d->streamHook, Impl::streamEvents(), d.get());

    spa_audio_info_raw info{};
    info.format = SPA_AUDIO_FORMAT_S32;
    info.rate = streamRate;
    info.channels = channels;
    channelPositions(channels, info.position);
    uint8_t buffer[1024];
    spa_pod_builder b{};
    spa_pod_builder_init(&b, buffer, sizeof(buffer));
    const spa_pod* params[1] = {spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &info)};
    d->render = std::move(render);
    d->channels = channels;
    d->rate = streamRate;
    const auto flags = static_cast<pw_stream_flags>(PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS
                                                    | PW_STREAM_FLAG_RT_PROCESS | PW_STREAM_FLAG_INACTIVE);
    if (P.pw_stream_connect(d->stream, PW_DIRECTION_OUTPUT, PW_ID_ANY, flags, params, 1) < 0) {
        d->destroyStream();
        c.unlock();
        if (error)
            *error = "PipeWire refused the stream";
        return false;
    }
    // Linked to a sink and the format agreed (or not).
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    while ((d->state == PW_STREAM_STATE_UNCONNECTED || d->state == PW_STREAM_STATE_CONNECTING) && !c.failed
           && std::chrono::steady_clock::now() < deadline)
        P.pw_thread_loop_timed_wait(c.loop, 1);
    if (d->state != PW_STREAM_STATE_PAUSED && d->state != PW_STREAM_STATE_STREAMING) {
        d->destroyStream();
        c.unlock();
        if (error)
            *error = c.failed ? "PipeWire stopped" : "PipeWire found no output for the stream";
        return false;
    }
    // Nothing between the engine and the sink scales the samples: this
    // stream at 100 % (whatever volume was remembered for it).
    float unity[8] = {1, 1, 1, 1, 1, 1, 1, 1};
    float unmuted = 0.0f;
    P.pw_stream_set_control(d->stream, SPA_PROP_channelVolumes, channels, unity, SPA_PROP_mute, 1u, &unmuted, 0);

    const std::string sinkName = target.empty() ? c.defaultSink : target;
    d->sinkDescription = sinkName;
    for (const SinkNode& s : c.sinks)
        if (s.name == sinkName)
            d->sinkDescription = s.description;
    c.watchSink(sinkName);
    c.roundtrip();
    c.unlock();
    return true;
}

void PipeWireOutput::close()
{
    stop();
    if (!d->stream)
        return;
    d->c.lock();
    d->destroyStream();
    d->c.unlock();
}

void PipeWireOutput::release()
{
    close();
    Connection& c = d->c;
    if (!c.connected())
        return;
    if (d->forcedRate != 0) {
        // The graph's rate given back, unless someone else has set it since.
        c.lock();
        if (c.setting("clock.force-rate") == std::to_string(d->forcedRate))
            c.setSetting("clock.force-rate", "0");
        c.unlock();
        d->forcedRate = 0;
    }
    c.disconnect();
}

bool PipeWireOutput::isOpen() const
{
    return d->stream != nullptr;
}

bool PipeWireOutput::start()
{
    if (!d->stream)
        return false;
    d->running.store(true);
    d->c.lock();
    const bool ok = d->c.P.pw_stream_set_active(d->stream, true) >= 0;
    d->c.unlock();
    if (!ok)
        d->running.store(false);
    return ok;
}

void PipeWireOutput::stop()
{
    if (!d->stream)
        return;
    d->running.store(false);
    d->c.lock();
    d->c.P.pw_stream_set_active(d->stream, false);
    d->c.unlock();
    // A cycle that saw `running` before it was cleared ends first.
    while (d->inCallback.load() != 0)
        std::this_thread::sleep_for(std::chrono::microseconds(200));
}

uint32_t PipeWireOutput::rate() const
{
    return d->rate;
}

uint32_t PipeWireOutput::graphRate() const
{
    return d->stream ? d->graphRate.load(std::memory_order_relaxed) : 0;
}

bool PipeWireOutput::volumeNeutral() const
{
    return d->streamVolumeOk.load(std::memory_order_relaxed) && d->streamMuteOk.load(std::memory_order_relaxed)
        && d->c.sinkNeutral.load(std::memory_order_relaxed);
}

uint32_t PipeWireOutput::bufferFrames() const
{
    // This stream's cycle, the sink's, and the device's own buffering.
    return 4 * d->quantum;
}

std::string PipeWireOutput::sinkDescription() const
{
    return d->sinkDescription;
}

bool PipeWireOutput::watch(const std::string& target, std::string* error)
{
    Connection& c = d->c;
    if (!c.connected() && !c.connect(error))
        return false;
    c.lock();
    c.watchedName = target;
    c.followDefault = target.empty();
    const std::string name = target.empty() ? c.defaultSink : target;
    c.watchSink(name);
    const bool found = c.watchedSink != SPA_ID_INVALID;
    if (found)
        c.roundtrip(); // the first Props (the volume) arrive before this returns
    c.unlock();
    if (!found && error)
        *error = name.empty() ? "PipeWire has no default output" : "PipeWire has no output called " + name;
    return found;
}

void PipeWireOutput::unwatch()
{
    Connection& c = d->c;
    if (!c.connected())
        return;
    c.lock();
    c.followDefault = false;
    c.watchedName.clear();
    if (!d->stream)
        c.unwatchSink();
    c.unlock();
    if (!d->stream)
        c.disconnect();
}

bool PipeWireOutput::watching() const
{
    return d->c.connected() && d->c.watchedSink != SPA_ID_INVALID;
}

bool PipeWireOutput::sinkVolume(float* linear, bool* muted) const
{
    if (!d->c.sinkVolumeKnown.load(std::memory_order_acquire))
        return false;
    if (linear)
        *linear = d->c.sinkVolume.load(std::memory_order_relaxed);
    if (muted)
        *muted = d->c.sinkMuted.load(std::memory_order_relaxed);
    return true;
}

bool PipeWireOutput::setSinkVolume(float linear, bool mute)
{
    Connection& c = d->c;
    if (!c.connected())
        return false;
    c.lock();
    const bool ok = c.sinkNode != nullptr;
    if (ok) {
        const uint32_t n = std::clamp<uint32_t>(c.sinkChannels.load(std::memory_order_relaxed), 1, SPA_AUDIO_MAX_CHANNELS);
        float v[SPA_AUDIO_MAX_CHANNELS];
        std::fill(v, v + n, std::clamp(linear, 0.0f, 1.0f));
        uint8_t buffer[1024];
        spa_pod_builder b{};
        spa_pod_builder_init(&b, buffer, sizeof(buffer));
        const spa_pod* props = static_cast<const spa_pod*>(spa_pod_builder_add_object(
            &b, SPA_TYPE_OBJECT_Props, SPA_PARAM_Props,
            SPA_PROP_channelVolumes, SPA_POD_Array(sizeof(float), SPA_TYPE_Float, n, v),
            SPA_PROP_mute, SPA_POD_Bool(mute)));
        pw_node_set_param(c.sinkNode, SPA_PARAM_Props, 0, props);
    }
    c.unlock();
    return ok;
}

bool PipeWireOutput::sinkIsHardware() const
{
    return d->c.watchedHardware;
}

bool PipeWireOutput::sinkVolumeInHardware() const
{
    return d->c.watchedHardware && d->c.sinkSoftUnity.load(std::memory_order_relaxed);
}

std::string PipeWireOutput::watchedSinkDescription() const
{
    return d->c.watchedDescription;
}

void PipeWireOutput::setSinkChangedCallback(std::function<void()> callback)
{
    Connection& c = d->c;
    if (c.connected())
        c.lock();
    c.sinkChanged = std::move(callback);
    if (c.connected())
        c.unlock();
}

#else // no PipeWire headers at build time

struct PipeWireOutput::Impl {};

bool PipeWireOutput::available() { return false; }
std::vector<PipeWireOutput::Sink> PipeWireOutput::sinks() { return {}; }
std::string PipeWireOutput::libraryVersion() { return {}; }
PipeWireOutput::PipeWireOutput() = default;
PipeWireOutput::~PipeWireOutput() = default;
bool PipeWireOutput::open(const std::string&, uint32_t, uint32_t, bool, Render, std::string* error)
{
    if (error)
        *error = "Built without PipeWire";
    return false;
}
void PipeWireOutput::close() {}
void PipeWireOutput::release() {}
bool PipeWireOutput::isOpen() const { return false; }
bool PipeWireOutput::start() { return false; }
void PipeWireOutput::stop() {}
uint32_t PipeWireOutput::rate() const { return 0; }
uint32_t PipeWireOutput::graphRate() const { return 0; }
bool PipeWireOutput::volumeNeutral() const { return true; }
uint32_t PipeWireOutput::bufferFrames() const { return 0; }
std::string PipeWireOutput::sinkDescription() const { return {}; }
bool PipeWireOutput::watch(const std::string&, std::string* error)
{
    if (error)
        *error = "Built without PipeWire";
    return false;
}
void PipeWireOutput::unwatch() {}
bool PipeWireOutput::watching() const { return false; }
bool PipeWireOutput::sinkVolume(float*, bool*) const { return false; }
bool PipeWireOutput::setSinkVolume(float, bool) { return false; }
bool PipeWireOutput::sinkIsHardware() const { return false; }
bool PipeWireOutput::sinkVolumeInHardware() const { return false; }
std::string PipeWireOutput::watchedSinkDescription() const { return {}; }
void PipeWireOutput::setSinkChangedCallback(std::function<void()>) {}

#endif

} // namespace sonkore
