#include "FilePreload.hpp"

#if defined(_WIN32)
#include <windows.h>
#else
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace sonkore {

std::unique_ptr<FilePreload> FilePreload::load(const std::filesystem::path& path, std::size_t maxBytes)
{
    std::unique_ptr<FilePreload> p(new FilePreload);
#if defined(_WIN32)
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
    if (file == INVALID_HANDLE_VALUE)
        return nullptr;
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file, &size) || size.QuadPart <= 0 || static_cast<unsigned long long>(size.QuadPart) > maxBytes) {
        CloseHandle(file);
        return nullptr;
    }
    HANDLE mapping = CreateFileMappingW(file, nullptr, PAGE_READONLY, 0, 0, nullptr);
    void* view = mapping ? MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) : nullptr;
    if (!view) {
        if (mapping)
            CloseHandle(mapping);
        CloseHandle(file);
        return nullptr;
    }
    p->m_file = file;
    p->m_mapping = mapping;
    p->m_address = view;
    p->m_size = static_cast<std::size_t>(size.QuadPart);
    WIN32_MEMORY_RANGE_ENTRY range{view, p->m_size};
    PrefetchVirtualMemory(GetCurrentProcess(), 1, &range, 0);
    p->m_locked = VirtualLock(view, p->m_size) != 0; // limited by the working set
#else
    const int fd = ::open(path.c_str(), O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return nullptr;
    struct stat st {};
    if (::fstat(fd, &st) != 0 || st.st_size <= 0 || static_cast<unsigned long long>(st.st_size) > maxBytes) {
        ::close(fd);
        return nullptr;
    }
    p->m_size = static_cast<std::size_t>(st.st_size);
    int flags = MAP_PRIVATE;
#if defined(MAP_POPULATE)
    flags |= MAP_POPULATE; // read it all in now, not on first touch
#endif
    void* address = ::mmap(nullptr, p->m_size, PROT_READ, flags, fd, 0);
    ::close(fd); // the mapping keeps the file
    if (address == MAP_FAILED)
        return nullptr;
    p->m_address = address;
    ::madvise(address, p->m_size, MADV_WILLNEED);
    // Pinned when RLIMIT_MEMLOCK allows it; otherwise it stays in the page
    // cache, which still serves every read without the disk.
    p->m_locked = ::mlock(address, p->m_size) == 0;
#endif
    return p;
}

FilePreload::~FilePreload()
{
#if defined(_WIN32)
    if (m_address) {
        if (m_locked)
            VirtualUnlock(m_address, m_size);
        UnmapViewOfFile(m_address);
    }
    if (m_mapping)
        CloseHandle(m_mapping);
    if (m_file)
        CloseHandle(m_file);
#else
    if (m_address) {
        if (m_locked)
            ::munlock(m_address, m_size);
        ::munmap(m_address, m_size);
    }
#endif
}

} // namespace sonkore
