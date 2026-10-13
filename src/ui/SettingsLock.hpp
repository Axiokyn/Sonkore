#pragma once

#include <atomic>

namespace sonkore {

// Set once the preferences have been reset and the app is about to restart:
// from then on nothing may write the old values back on the way out.
inline std::atomic<bool> g_settingsFrozen{false};

} // namespace sonkore
