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
    };

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
    std::string sinkDescription() const;

private:
    struct Impl;
    std::unique_ptr<Impl> d;
};

} // namespace sonkore
