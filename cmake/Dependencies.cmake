# Third-party dependencies, fetched at configure time as hash-verified
# tarballs (no git needed) so every platform builds the same decoder / DSP code.

include(FetchContent)

# SoundTouch 2.3.3 declares cmake_minimum_required(VERSION 3.1), which CMake 4
# rejects unless a policy floor is provided.
set(CMAKE_POLICY_VERSION_MINIMUM 3.5)

option(SONKORE_USE_SYSTEM_SOUNDTOUCH "Link the system SoundTouch (pkg-config) instead of fetching it" OFF)

# --- miniaudio (header-only; implementation compiled in backend_impl.c) -------
# SOURCE_SUBDIR points at a non-existent dir so only the sources are downloaded
# and miniaudio's own CMake project (examples, tests) is not configured.
FetchContent_Declare(miniaudio
    URL      https://github.com/mackron/miniaudio/archive/refs/tags/0.11.21.tar.gz
    URL_HASH SHA256=6afb5c231613d2fab4f1c668b7243ff9a7d6d78a7f5a2692c133f026fe508506
    SOURCE_SUBDIR _header_only_)

# --- dr_libs (dr_flac) --------------------------------------------------------
FetchContent_Declare(dr_libs
    URL      https://github.com/mackron/dr_libs/archive/dfe8377631000664666519fdb83da193fd8037f4.tar.gz
    URL_HASH SHA256=4654acb029f4f2a43ac2edb60c4cb09f40615b4b5bee9709954f910cb979e5fd
    SOURCE_SUBDIR _header_only_)

FetchContent_MakeAvailable(miniaudio dr_libs)

add_library(sonkore_miniaudio INTERFACE)
target_include_directories(sonkore_miniaudio SYSTEM INTERFACE ${miniaudio_SOURCE_DIR})
add_library(sonkore::miniaudio ALIAS sonkore_miniaudio)

add_library(sonkore_dr_flac INTERFACE)
target_include_directories(sonkore_dr_flac SYSTEM INTERFACE ${dr_libs_SOURCE_DIR})
add_library(sonkore::dr_flac ALIAS sonkore_dr_flac)

# --- libebur128 (EBU R128 loudness and true peak, for ReplayGain analysis) ---
# One C file; its own CMake project (shared library, tests) is not used.
FetchContent_Declare(libebur128
    URL      https://github.com/jiixyj/libebur128/archive/refs/tags/v1.2.6.tar.gz
    URL_HASH SHA256=baa7fc293a3d4651e244d8022ad03ab797ca3c2ad8442c43199afe8059faa613
    SOURCE_SUBDIR _header_only_)
FetchContent_MakeAvailable(libebur128)

add_library(sonkore_ebur128 STATIC ${libebur128_SOURCE_DIR}/ebur128/ebur128.c)
target_include_directories(sonkore_ebur128 SYSTEM
    PUBLIC  ${libebur128_SOURCE_DIR}/ebur128
    PRIVATE ${libebur128_SOURCE_DIR}/ebur128/queue)
set_target_properties(sonkore_ebur128 PROPERTIES POSITION_INDEPENDENT_CODE ON)
if(MSVC)
    target_compile_definitions(sonkore_ebur128 PRIVATE _USE_MATH_DEFINES) # M_PI
else()
    target_link_libraries(sonkore_ebur128 PRIVATE m)
endif()
add_library(sonkore::ebur128 ALIAS sonkore_ebur128)

# --- libhdcd (HDCD decoding: 20-bit CDs with peak extend / gain control) -------
# BSD-2-Clause library code (based on foo_hdcd); built from its sources.
FetchContent_Declare(libhdcd
    URL      https://github.com/bp0/libhdcd/archive/refs/tags/v1.4.tar.gz
    URL_HASH SHA256=0a576ee42fdff97f15b8e82cc2d27ddb1eb667df0c38971f2d5a9afc5a77b8b4
    SOURCE_SUBDIR _header_only_)
FetchContent_MakeAvailable(libhdcd)

add_library(sonkore_hdcd STATIC
    ${libhdcd_SOURCE_DIR}/src/hdcd_simple.c
    ${libhdcd_SOURCE_DIR}/src/hdcd_decode2.c
    ${libhdcd_SOURCE_DIR}/src/hdcd_tables.c
    ${libhdcd_SOURCE_DIR}/src/hdcd_strings.c
    ${libhdcd_SOURCE_DIR}/src/hdcd_libversion.c
    ${libhdcd_SOURCE_DIR}/src/hdcd_analyze_tonegen.c)
target_include_directories(sonkore_hdcd SYSTEM PUBLIC ${libhdcd_SOURCE_DIR}/src)
set_target_properties(sonkore_hdcd PROPERTIES POSITION_INDEPENDENT_CODE ON)
# hdcd_tables.c relies on autotools' config for <stdint.h>: included here,
# the upstream sources stay untouched.
target_compile_options(sonkore_hdcd PRIVATE $<IF:$<C_COMPILER_ID:MSVC>,/FIstdint.h,-include stdint.h>)
if(MSVC)
    target_compile_definitions(sonkore_hdcd PRIVATE _USE_MATH_DEFINES _CRT_SECURE_NO_WARNINGS)
endif()
if(NOT MSVC)
    target_link_libraries(sonkore_hdcd PRIVATE m)
endif()
add_library(sonkore::hdcd ALIAS sonkore_hdcd)

# --- pocketfft (double-precision FFT for the convolution engine) -------------
# Header-only C++ (BSD-3-Clause), the FFT behind NumPy and SciPy.
FetchContent_Declare(pocketfft
    URL      https://github.com/mreineck/pocketfft/archive/c90e55b3d529f8efa40ed01a20de22405f45fc65.tar.gz
    URL_HASH SHA256=2d90016da31a8cc1dd0ac0c89c1f2fb55df8f4ccc3f0386dcfbad09ac6d1c3ba
    SOURCE_SUBDIR _header_only_)
FetchContent_MakeAvailable(pocketfft)
add_library(sonkore_pocketfft INTERFACE)
target_include_directories(sonkore_pocketfft SYSTEM INTERFACE ${pocketfft_SOURCE_DIR})
target_compile_definitions(sonkore_pocketfft INTERFACE POCKETFFT_NO_MULTITHREADING)
add_library(sonkore::pocketfft ALIAS sonkore_pocketfft)

# --- SoundTouch ---------------------------------------------------------------
add_library(sonkore_soundtouch INTERFACE)
add_library(sonkore::soundtouch ALIAS sonkore_soundtouch)

if(SONKORE_USE_SYSTEM_SOUNDTOUCH)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(SOUNDTOUCH REQUIRED IMPORTED_TARGET soundtouch>=2.3)
    target_link_libraries(sonkore_soundtouch INTERFACE PkgConfig::SOUNDTOUCH)
else()
    # Float sample build (INTEGER_SAMPLES OFF) is mandatory: the engine feeds
    # SoundTouch 32-bit float frames.
    set(INTEGER_SAMPLES OFF CACHE BOOL "" FORCE)
    set(SOUNDSTRETCH    OFF CACHE BOOL "" FORCE)
    set(SOUNDTOUCH_DLL  OFF CACHE BOOL "" FORCE)
    set(OPENMP          OFF CACHE BOOL "" FORCE)

    FetchContent_Declare(soundtouch
        URL      https://codeberg.org/soundtouch/soundtouch/archive/2.3.3.tar.gz
        URL_HASH SHA256=06c00f37d28e5984944951c20d0845c7931e691c59820922230915d205131a47)
    FetchContent_MakeAvailable(soundtouch)

    target_link_libraries(sonkore_soundtouch INTERFACE SoundTouch)
endif()

# --- Rubber Band (pitch / tempo, default engine) ------------------------------
# Built from its single-file compilation unit (built-in FFT and resampler, vDSP
# on Apple), so no FFTW or libsamplerate is needed.
option(SONKORE_USE_SYSTEM_RUBBERBAND "Link the system Rubber Band (pkg-config) instead of fetching it" OFF)

add_library(sonkore_rubberband INTERFACE)
add_library(sonkore::rubberband ALIAS sonkore_rubberband)

if(SONKORE_USE_SYSTEM_RUBBERBAND)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(RUBBERBAND REQUIRED IMPORTED_TARGET rubberband>=3.0)
    target_link_libraries(sonkore_rubberband INTERFACE PkgConfig::RUBBERBAND)
else()
    FetchContent_Declare(rubberband
        URL      https://breakfastquay.com/files/releases/rubberband-4.0.0.tar.bz2
        URL_HASH SHA256=af050313ee63bc18b35b2e064e5dce05b276aaf6d1aa2b8a82ced1fe2f8028e9
        SOURCE_SUBDIR _single_file_)
    FetchContent_MakeAvailable(rubberband)

    add_library(sonkore_rubberband_single STATIC ${rubberband_SOURCE_DIR}/single/RubberBandSingle.cpp)
    target_include_directories(sonkore_rubberband_single SYSTEM PUBLIC ${rubberband_SOURCE_DIR})
    set_target_properties(sonkore_rubberband_single PROPERTIES POSITION_INDEPENDENT_CODE ON)
    if(MSVC)
        target_compile_definitions(sonkore_rubberband_single PRIVATE _USE_MATH_DEFINES NOMINMAX)
        target_compile_options(sonkore_rubberband_single PRIVATE /W0)
    else()
        target_compile_options(sonkore_rubberband_single PRIVATE -w)
    endif()
    if(APPLE)
        target_link_libraries(sonkore_rubberband_single PUBLIC "-framework Accelerate")
    endif()
    target_link_libraries(sonkore_rubberband INTERFACE sonkore_rubberband_single)
endif()

# --- SoX Resampler (only used when the device refuses the track's rate) -------
option(SONKORE_USE_SYSTEM_SOXR "Link the system libsoxr (pkg-config) instead of fetching it" OFF)

add_library(sonkore_soxr INTERFACE)
add_library(sonkore::soxr ALIAS sonkore_soxr)

if(SONKORE_USE_SYSTEM_SOXR)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(SOXR REQUIRED IMPORTED_TARGET soxr>=0.1.2)
    target_link_libraries(sonkore_soxr INTERFACE PkgConfig::SOXR)
else()
    # Static, single-threaded, no libsamplerate bindings, tests or trace code.
    set(BUILD_TESTS        OFF CACHE BOOL "" FORCE)
    set(BUILD_EXAMPLES     OFF CACHE BOOL "" FORCE)
    set(BUILD_SHARED_LIBS  OFF CACHE BOOL "" FORCE)
    set(WITH_OPENMP        OFF CACHE BOOL "" FORCE)
    set(WITH_LSR_BINDINGS  OFF CACHE BOOL "" FORCE)
    set(WITH_VR32          OFF CACHE BOOL "" FORCE)
    set(WITH_AVFFT         OFF CACHE BOOL "" FORCE)
    set(WITH_DEV_TRACE     OFF CACHE BOOL "" FORCE)

    set(SONKORE_SOXR_SOURCE_DIR ${CMAKE_BINARY_DIR}/_deps/soxr-src)
    FetchContent_Declare(soxr
        URL      https://downloads.sourceforge.net/project/soxr/soxr-0.1.3-Source.tar.xz
        URL_HASH SHA256=b111c15fdc8c029989330ff559184198c161100a59312f5dc19ddeb9b5a15889
        SOURCE_DIR ${SONKORE_SOXR_SOURCE_DIR}
        PATCH_COMMAND ${CMAKE_COMMAND} -P ${CMAKE_CURRENT_LIST_DIR}/patches/soxr-paths-with-spaces.cmake)
    # soxr looks for its CMake modules under CMAKE_SOURCE_DIR (ours), lists
    # its sources without extensions and predates CMP0127.
    list(APPEND CMAKE_MODULE_PATH ${SONKORE_SOXR_SOURCE_DIR}/cmake/Modules)
    set(CMAKE_POLICY_DEFAULT_CMP0115 OLD)
    set(CMAKE_POLICY_DEFAULT_CMP0127 OLD)
    FetchContent_MakeAvailable(soxr)

    target_include_directories(sonkore_soxr SYSTEM INTERFACE ${SONKORE_SOXR_SOURCE_DIR}/src)
    target_link_libraries(sonkore_soxr INTERFACE soxr)
endif()

# --- FFmpeg (optional: ALAC, AAC, Opus, Vorbis, AIFF, APE, WavPack, DSD...) ---
# Only its headers are used at build time: the system's libraries (on Flathub,
# the runtime's; LGPL) are loaded when a file needs them, so FLAC/WAV/MP3
# sessions never map libavcodec and its dependencies. FLAC, WAV and MP3 are
# always decoded by dr_libs.
set(SONKORE_WITH_FFMPEG AUTO CACHE STRING "Decode extra formats with FFmpeg: AUTO, ON or OFF")
set_property(CACHE SONKORE_WITH_FFMPEG PROPERTY STRINGS AUTO ON OFF)

add_library(sonkore_ffmpeg INTERFACE)
add_library(sonkore::ffmpeg ALIAS sonkore_ffmpeg)
set(SONKORE_HAVE_FFMPEG OFF)

# Windows: FFmpeg's include folder can be given directly (only the headers are
# used; the DLLs are found next to Sonkore.exe at run time).
set(SONKORE_FFMPEG_INCLUDE_DIR "" CACHE PATH "FFmpeg headers, instead of pkg-config")

if(NOT SONKORE_WITH_FFMPEG STREQUAL "OFF" AND SONKORE_FFMPEG_INCLUDE_DIR)
    if(NOT EXISTS "${SONKORE_FFMPEG_INCLUDE_DIR}/libavformat/avformat.h")
        message(FATAL_ERROR "No FFmpeg headers in SONKORE_FFMPEG_INCLUDE_DIR=${SONKORE_FFMPEG_INCLUDE_DIR}")
    endif()
    target_include_directories(sonkore_ffmpeg SYSTEM INTERFACE ${SONKORE_FFMPEG_INCLUDE_DIR})
    target_link_libraries(sonkore_ffmpeg INTERFACE ${CMAKE_DL_LIBS})
    target_compile_definitions(sonkore_ffmpeg INTERFACE SONKORE_HAVE_FFMPEG)
    set(SONKORE_HAVE_FFMPEG ON)
elseif(NOT SONKORE_WITH_FFMPEG STREQUAL "OFF")
    find_package(PkgConfig)
    if(PkgConfig_FOUND)
        # FFmpeg 5.1+ (AVChannelLayout API).
        if(SONKORE_WITH_FFMPEG STREQUAL "ON")
            pkg_check_modules(FFMPEG REQUIRED IMPORTED_TARGET
                libavformat>=59.27.100 libavcodec>=59.37.100 libavutil>=57.28.100)
        else()
            pkg_check_modules(FFMPEG IMPORTED_TARGET
                libavformat>=59.27.100 libavcodec>=59.37.100 libavutil>=57.28.100)
        endif()
    elseif(SONKORE_WITH_FFMPEG STREQUAL "ON")
        message(FATAL_ERROR "SONKORE_WITH_FFMPEG=ON needs pkg-config to find FFmpeg")
    endif()
    if(FFMPEG_FOUND)
        target_include_directories(sonkore_ffmpeg SYSTEM INTERFACE ${FFMPEG_INCLUDE_DIRS})
        target_link_libraries(sonkore_ffmpeg INTERFACE ${CMAKE_DL_LIBS})
        target_compile_definitions(sonkore_ffmpeg INTERFACE SONKORE_HAVE_FFMPEG)
        set(SONKORE_HAVE_FFMPEG ON)
    endif()
endif()
message(STATUS "Sonkore: FFmpeg decoding ${SONKORE_HAVE_FFMPEG}")
