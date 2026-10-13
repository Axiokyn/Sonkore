#pragma once

#include <QtGlobal>

// Real-time scheduling for the audio thread on Linux, through rtkit (the
// D-Bus service PipeWire and PulseAudio use): an ordinary user may not set
// SCHED_FIFO directly, rtkit grants it within safe limits.
namespace RealtimeKit {

// Priority of `tid` (a thread of this process) when it already runs under
// SCHED_FIFO / SCHED_RR, otherwise 0.
int currentPriority(qint64 tid);

// Asks rtkit for real-time scheduling (SCHED_RR) at the highest priority it
// allows. Returns the priority granted, or 0 when rtkit is missing or refuses.
int makeRealtime(qint64 tid);

} // namespace RealtimeKit
