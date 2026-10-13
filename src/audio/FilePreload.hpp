#pragma once

#include <cstddef>
#include <filesystem>
#include <memory>

namespace sonkore {

// Keeps a whole file in RAM while it plays ("play from memory"): it is mapped,
// read in at once and, where the system allows, locked so it cannot be paged
// out. The decoders still open the file by name; the operating system then
// serves every read from these pages, so playback touches no disk at all.
class FilePreload {
public:
    // Null when the file cannot be mapped or is larger than `maxBytes`.
    static std::unique_ptr<FilePreload> load(const std::filesystem::path& path, std::size_t maxBytes);
    ~FilePreload();
    FilePreload(const FilePreload&) = delete;
    FilePreload& operator=(const FilePreload&) = delete;

    std::size_t size() const { return m_size; }
    bool locked() const { return m_locked; } // pinned in RAM, not only cached

private:
    FilePreload() = default;
    void* m_address = nullptr;
    std::size_t m_size = 0;
    bool m_locked = false;
#if defined(_WIN32)
    void* m_file = nullptr;
    void* m_mapping = nullptr;
#endif
};

} // namespace sonkore
