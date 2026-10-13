#pragma once

#include <algorithm>
#include <atomic>
#include <cstddef>
#include <cstring>
#include <memory>
#include <type_traits>

namespace sonkore {

// Wait-free single-producer / single-consumer ring buffer.
// Producer: DSP worker thread. Consumer: miniaudio real-time callback.
// Indices grow monotonically and wrap through unsigned arithmetic.
template <typename T>
class SpscRingBuffer {
    static_assert(std::is_trivially_copyable_v<T>);

public:
    SpscRingBuffer() = default;

    // Not thread-safe: call only while neither side is active.
    void allocate(std::size_t minCapacity)
    {
        std::size_t cap = 1;
        while (cap < minCapacity)
            cap <<= 1;
        m_buffer = std::make_unique<T[]>(cap);
        m_capacity = cap;
        m_mask = cap - 1;
        clear();
    }

    // Not thread-safe: call only while neither side is active.
    void clear() noexcept
    {
        m_write.store(0, std::memory_order_relaxed);
        m_read.store(0, std::memory_order_relaxed);
    }

    std::size_t capacity() const noexcept { return m_capacity; }

    // Safe from any thread (approximate while both sides run).
    std::size_t size() const noexcept
    {
        return m_write.load(std::memory_order_acquire) - m_read.load(std::memory_order_acquire);
    }

    // Producer side.
    std::size_t availableToWrite() const noexcept
    {
        return m_capacity - (m_write.load(std::memory_order_relaxed) - m_read.load(std::memory_order_acquire));
    }

    // Consumer side.
    std::size_t availableToRead() const noexcept
    {
        return m_write.load(std::memory_order_acquire) - m_read.load(std::memory_order_relaxed);
    }

    // Producer side. Returns the number of elements written.
    std::size_t write(const T* src, std::size_t count) noexcept
    {
        const std::size_t w = m_write.load(std::memory_order_relaxed);
        const std::size_t r = m_read.load(std::memory_order_acquire);
        const std::size_t n = std::min(count, m_capacity - (w - r));
        if (n == 0)
            return 0;
        const std::size_t start = w & m_mask;
        const std::size_t first = std::min(n, m_capacity - start);
        std::memcpy(m_buffer.get() + start, src, first * sizeof(T));
        if (n > first)
            std::memcpy(m_buffer.get(), src + first, (n - first) * sizeof(T));
        m_write.store(w + n, std::memory_order_release);
        return n;
    }

    // Consumer side. Returns the number of elements read.
    std::size_t read(T* dst, std::size_t count) noexcept
    {
        const std::size_t r = m_read.load(std::memory_order_relaxed);
        const std::size_t w = m_write.load(std::memory_order_acquire);
        const std::size_t n = std::min(count, w - r);
        if (n == 0)
            return 0;
        const std::size_t start = r & m_mask;
        const std::size_t first = std::min(n, m_capacity - start);
        std::memcpy(dst, m_buffer.get() + start, first * sizeof(T));
        if (n > first)
            std::memcpy(dst + first, m_buffer.get(), (n - first) * sizeof(T));
        m_read.store(r + n, std::memory_order_release);
        return n;
    }

private:
    std::unique_ptr<T[]> m_buffer;
    std::size_t m_capacity = 0;
    std::size_t m_mask = 0;
    alignas(64) std::atomic<std::size_t> m_write{0};
    alignas(64) std::atomic<std::size_t> m_read{0};
};

} // namespace sonkore
