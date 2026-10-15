#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace sonkore {

// Linux: PipeWire spoken natively (libpipewire, loaded at run time, so the
// program still starts where it is missing), instead of through its
// PulseAudio emulation. What that buys a Hi-Fi player:
//
// * The graph runs at the track's own rate. The stream asks for it
//   (node.rate) and, with `switchRate`, the graph is held at it through the
//   "settings" metadata (clock.force-rate) for as long as the output is open,
//   so a 44.1 kHz album reaches the DAC at 44.1 kHz instead of being resampled
//   to the desktop's 48 kHz. The rate is given back (0) on release().
// * Without `switchRate` the stream opens at the rate the graph runs at, and
//   the engine converts with SoX VHQ rather than leaving it to PipeWire.
// * The stream's own volume is held at 100 %; graphRate() and volumeNeutral()
//   say whether the graph really runs at the stream's rate and whether
//   anything (this stream, the sink's software volume) scales the samples.
//   PipeWire carries audio as 32-bit float: integer samples of up to 24 bits
//   survive the trip exactly, which is what the engine's status assumes.
//
// The engine fills buffers through `render` exactly as for its other
// outputs: left-justified s32 frames, interleaved in WAVE/FLAC channel order.
class PipeWireOutput {
public:
    using Render = std::function<void(int32_t* out, uint32_t frames)>;

    struct Sink {
        std::string name;        // node.name: the key to open it by
        std::string description; // what the desktop calls it
        bool isDefault = false;
        int alsaCard = -1;       // the ALSA card behind it, -1 for none (virtual sinks)
    };
    // "alsa_output.x.3" -> "alsa_output.x": the name without the suffix
    // WirePlumber adds when a node is made again under a name still in use.
    static std::string baseSinkName(const std::string& name);

    // libpipewire loads and a PipeWire daemon answers.
    static bool available();
    // The playback sinks of the graph (Audio/Sink nodes). Empty without PipeWire.
    static std::vector<Sink> sinks();
    // "1.0.5", or "" without libpipewire.
    static std::string libraryVersion();

    PipeWireOutput();
    ~PipeWireOutput();
    PipeWireOutput(const PipeWireOutput&) = delete;
    PipeWireOutput& operator=(const PipeWireOutput&) = delete;

    // `target`: a sink's node.name, "" for the desktop's default. `rate`: the
    // track's. False, with a reason, when no stream could be set up; nothing
    // is left open but the connection (kept for the next open()).
    bool open(const std::string& target, uint32_t channels, uint32_t rate, bool switchRate, Render render,
              std::string* error);
    // Ends the stream. A rate held for it stays held: the next track usually
    // wants it too, and the DAC need not re-lock in between.
    void close();
    // close(), the rate given back to the graph, and the connection ended.
    void release();
    bool isOpen() const;
    bool start();
    void stop(); // returns once `render` is no longer called

    uint32_t rate() const;       // the stream's
    uint32_t graphRate() const;  // the graph's now (0: not known yet)
    bool volumeNeutral() const;  // nothing scales the samples on the way
    uint32_t bufferFrames() const;
    // What the desktop calls the sink the stream feeds now: the session
    // manager moves a stream when its sink goes away (a filter program
    // closing) or the default changes, and this follows.
    std::string sinkDescription() const;
    std::string sinkName() const; // its node.name ("" until linked)
    // The sink the stream fed went away less than `withinMs` ago and was not
    // a device's (a filter program's sink, a loopback). The session manager
    // moves the stream on, and may ask players to pause as for an unplugged
    // DAC: that pause is not meant for this case.
    bool lostVirtualSink(unsigned withinMs) const;

    // --- The graph ---------------------------------------------------------
    // Keeps the connection open without a stream or a watched sink, so the
    // callback below reports while nothing plays.
    bool watchGraph(std::string* error);
    // Called from PipeWire's thread when a sink appeared or disappeared, the
    // default sink changed, or the stream was moved to another sink.
    void setGraphChangedCallback(std::function<void()> callback);
    // The sinks as this connection sees them (no new connection, unlike sinks()).
    std::vector<Sink> currentSinks() const;

    // --- The sink's volume, with or without a stream ------------------------
    // Watches the sink `target` ("" = the desktop's default, followed when it
    // changes) and reads its volume. A device's sink (ALSA) carries the volume
    // on the DAC's own control where it has one (PipeWire splits what is left
    // into softVolumes), so a player that moves it keeps its samples untouched.
    // Connects to the daemon if needed; false with a reason when it does not
    // answer or the sink is not there.
    bool watch(const std::string& target, std::string* error);
    void unwatch(); // stops following it (the connection ends too without a stream)
    bool watching() const;
    // Linear amplitude (1 = 100 %) of the watched sink, false until the first
    // Props arrived. `muted` is the sink's mute switch.
    bool sinkVolume(float* linear, bool* muted) const;
    // Sets the sink's volume (its channels alike) and mute; false when not watching.
    bool setSinkVolume(float linear, bool mute);
    // The watched sink belongs to a device (its volume may sit in hardware).
    bool sinkIsHardware() const;
    // Whether the device's own control carries all of the volume now (no
    // software scaling left: softVolumes at 1). False for a virtual sink.
    bool sinkVolumeInHardware() const;
    std::string watchedSinkDescription() const;
    // Called from PipeWire's thread whenever the watched sink's volume or
    // mute changed (also when the default sink changed under a "" target).
    void setSinkChangedCallback(std::function<void()> callback);

private:
    struct Impl;
    std::unique_ptr<Impl> d;
};

} // namespace sonkore
