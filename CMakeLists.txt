cmake_minimum_required(VERSION 3.21)

project(Sonkore
    VERSION 1.0.0
    DESCRIPTION "Hi-Fi music player: bit-perfect output, SoundTouch, SuperEQ"
    LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_C_STANDARD 11)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "" FORCE)
endif()

find_package(Threads REQUIRED)
# Qt6::Gui passes OpenGL on to the executable. The legacy libGL.so.1 is on every
# desktop (Qt itself needs it); the GLVND split (libOpenGL.so.0 + libGLX.so.0)
# is not, e.g. Ubuntu without libopengl0, where the binary would not start.
set(OpenGL_GL_PREFERENCE LEGACY)
find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Widgets Qml Quick QuickControls2 Network Svg)
# The software renderer's repaint area (SoftwareRepaint) uses two exported
# members of its private class.
find_package(Qt6 6.5 REQUIRED COMPONENTS QuickPrivate)
if(UNIX AND NOT APPLE)
    find_package(Qt6 6.5 OPTIONAL_COMPONENTS DBus)
endif()
qt_standard_project_setup(REQUIRES 6.5)

include(cmake/Dependencies.cmake)

# -----------------------------------------------------------------------------
# sonkore_audio: Qt-free audio engine (decoder, DSP, output).
# -----------------------------------------------------------------------------
add_library(sonkore_audio STATIC
    src/audio/backend_impl.c
    src/audio/Biquad.hpp
    src/audio/Decoder.hpp
    src/audio/Decoder.cpp
    src/audio/CueSheet.hpp
    src/audio/CueSheet.cpp
    src/audio/DsdDecoder.hpp
    src/audio/DsdDecoder.cpp
    src/audio/DstDecoder.hpp
    src/audio/DstDecoder.cpp
    src/audio/DsdToPcm.hpp
    src/audio/DsdToPcm.cpp
    src/audio/SigmaDelta.hpp
    src/audio/SigmaDelta.cpp
    src/audio/TestSignals.hpp
    src/audio/TestSignals.cpp
    src/audio/FilePreload.hpp
    src/audio/FilePreload.cpp
    src/audio/SampleConvert.hpp
    src/audio/SpscRingBuffer.hpp
    src/audio/TimeStretch.hpp
    src/audio/TimeStretch.cpp
    src/audio/Resampler.hpp
    src/audio/Resampler.cpp
    src/audio/SuperEQ.hpp
    src/audio/SuperEQ.cpp
    src/audio/LinearPhaseEq.hpp
    src/audio/LinearPhaseEq.cpp
    src/audio/Crossfeed.hpp
    src/audio/Limiter.hpp
    src/audio/Limiter.cpp
    src/audio/Convolver.hpp
    src/audio/Convolver.cpp
    src/audio/DSPManager.hpp
    src/audio/DSPManager.cpp
    src/audio/AsioAbi.hpp
    src/audio/AsioConvert.hpp
    src/audio/AsioOutput.hpp
    src/audio/AsioOutput.cpp
    src/audio/PipeWireOutput.hpp
    src/audio/PipeWireOutput.cpp
)
if(SONKORE_HAVE_FFMPEG)
    target_sources(sonkore_audio PRIVATE src/audio/FfmpegDecoder.hpp src/audio/FfmpegDecoder.cpp)
endif()
target_include_directories(sonkore_audio PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src/audio)
target_link_libraries(sonkore_audio
    PUBLIC  sonkore::miniaudio sonkore::dr_flac sonkore::ffmpeg
    PRIVATE sonkore::soundtouch sonkore::soxr sonkore::hdcd sonkore::pocketfft Threads::Threads)

# Strict IEEE-754 semantics in our own DSP code. Fast-math would break the
# exact "is this parameter neutral?" comparisons that gate the bit-perfect
# bypass, and would flush/reorder operations in the biquads.
if(MSVC)
    target_compile_options(sonkore_audio PRIVATE /fp:precise /W4 /utf-8)
else()
    target_compile_options(sonkore_audio PRIVATE -fno-fast-math -Wall -Wextra)
endif()

if(WIN32)
    # WASAPI / Avrt / ole32 are loaded at runtime by miniaudio; ASIO drivers
    # are COM objects found in the registry.
    target_compile_definitions(sonkore_audio PUBLIC NOMINMAX WIN32_LEAN_AND_MEAN)
    target_link_libraries(sonkore_audio PRIVATE ole32 advapi32 user32)
elseif(APPLE)
    target_compile_definitions(sonkore_audio PRIVATE MA_NO_RUNTIME_LINKING)
    target_link_libraries(sonkore_audio PRIVATE
        "-framework CoreFoundation" "-framework CoreAudio" "-framework AudioToolbox")
else()
    # ALSA / PulseAudio (pipewire-pulse) / JACK (pipewire-jack) are dlopen()'d.
    target_link_libraries(sonkore_audio PRIVATE ${CMAKE_DL_LIBS} m)
    # Native DSD (DSD_U32_BE/LE on a hw: device) and the DAC's volume control
    # need alsa-lib itself.
    find_package(ALSA)
    if(ALSA_FOUND)
        target_sources(sonkore_audio PRIVATE src/audio/AlsaDsdOutput.hpp src/audio/AlsaDsdOutput.cpp)
        target_compile_definitions(sonkore_audio PRIVATE SONKORE_HAVE_ALSA_DSD)
        target_link_libraries(sonkore_audio PRIVATE ALSA::ALSA)
    endif()
    # Native PipeWire output: only its headers at build time; libpipewire is
    # dlopen()'d, so the program runs (without it) where it is missing.
    find_package(PkgConfig)
    if(PkgConfig_FOUND)
        pkg_check_modules(PIPEWIRE libpipewire-0.3>=0.3.40)
    endif()
    if(PIPEWIRE_FOUND)
        target_include_directories(sonkore_audio SYSTEM PRIVATE ${PIPEWIRE_INCLUDE_DIRS})
        target_compile_definitions(sonkore_audio PRIVATE SONKORE_HAVE_PIPEWIRE)
    endif()
    message(STATUS "Sonkore: native PipeWire output ${PIPEWIRE_FOUND}")
endif()

# Engine tests (tests/make_corpus.sh builds the audio they play).
option(SONKORE_BUILD_TESTS "Build the audio engine tests" OFF)
if(SONKORE_BUILD_TESTS)
    enable_testing()
    add_executable(engine_test tests/engine_test.cpp)
    target_link_libraries(engine_test PRIVATE sonkore_audio)
    if(MSVC)
        target_compile_options(engine_test PRIVATE /utf-8)
    endif()
    set(SONKORE_TEST_CORPUS "${CMAKE_BINARY_DIR}/test-corpus" CACHE PATH "Audio corpus for engine_test")
    add_test(NAME engine COMMAND engine_test ${SONKORE_TEST_CORPUS})
endif()

# -----------------------------------------------------------------------------
# Sonkore: Qt 6 / QML application
# -----------------------------------------------------------------------------
qt_add_executable(Sonkore src/main.cpp)

set_source_files_properties(
    qml/Theme.qml qml/AppActions.qml qml/Prefs.qml qml/TrackColumns.qml qml/Clock.qml
    PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

# Every icon.
file(GLOB SONKORE_ICONS RELATIVE "${CMAKE_CURRENT_SOURCE_DIR}" CONFIGURE_DEPENDS qml/icons/*.svg qml/icons/solid/*.svg)

qt_add_qml_module(Sonkore
    URI Sonkore
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/MainDialogs.qml
        qml/AppActions.qml
        qml/Clock.qml
        qml/EditSoundtouch.qml
        qml/PrecisionField.qml
        qml/Prefs.qml
        qml/Theme.qml
        qml/TrackColumns.qml
        qml/components/Badge.qml
        qml/components/Card.qml
        qml/components/ConfirmDialog.qml
        qml/components/CoverArt.qml
        qml/components/Icon.qml
        qml/components/IconButton.qml
        qml/components/InfoTip.qml
        qml/components/LCheckBox.qml
        qml/components/LComboBox.qml
        qml/components/LMenu.qml
        qml/components/StatusCard.qml
        qml/components/LMenuItem.qml
        qml/components/LMenuSeparator.qml
        qml/components/LScrollBar.qml
        qml/components/LSlider.qml
        qml/components/LockedVolume.qml
        qml/components/VolumeModeMenu.qml
        qml/components/LSwitch.qml
        qml/components/SubfolderLevels.qml
        qml/components/WindowButtons.qml
        qml/components/NameDialog.qml
        qml/components/NowPlayingBar.qml
        qml/components/NumField.qml
        qml/components/PillButton.qml
        qml/components/PlayingBars.qml
        qml/components/EdgeFade.qml
        qml/components/ResponseGraph.qml
        qml/components/SearchField.qml
        qml/components/SectionLabel.qml
        qml/components/Sidebar.qml
        qml/components/SignalPathPopup.qml
        qml/components/SignalPathList.qml
        qml/components/TrackMenu.qml
        qml/components/TrackRow.qml
        qml/components/TrackTable.qml
        qml/components/WheelScroll.qml
        qml/components/MiniPlayer.qml
        qml/components/AppWindow.qml
        qml/components/TitleBarButton.qml
        qml/components/FolderTree.qml
        qml/components/Visualizer.qml
        qml/components/WindowOutline.qml
        qml/components/RoundRect.qml
        qml/components/ColumnBrowser.qml
        qml/components/PlayButton.qml
        qml/components/Brand.qml
        qml/pages/AlbumsPage.qml
        qml/pages/BrowsePage.qml
        qml/pages/EqualizerDialogs.qml
        qml/pages/EqualizerPage.qml
        qml/pages/FoldersPage.qml
        qml/pages/PlaylistDialogs.qml
        qml/pages/PlaylistPage.qml
        qml/pages/QueuePage.qml
        qml/pages/NowPlayingPage.qml
        qml/pages/SettingsDialogs.qml
        qml/pages/SettingsPage.qml
        qml/pages/TracksPage.qml
    RESOURCES
        ${SONKORE_ICONS}
    SOURCES
        src/ui/QmlSingleton.hpp
        src/ui/AppController.hpp
        src/ui/AppController.cpp
        src/ui/DspController.hpp
        src/ui/DspController.cpp
        src/ui/EqController.hpp
        src/ui/EqController.cpp
        src/ui/FrameAnimationDriver.hpp
        src/ui/FrameAnimationDriver.cpp
        src/ui/SoftwareRepaint.hpp
        src/ui/SoftwareRepaint.cpp
        src/ui/WaveformController.hpp
        src/ui/WaveformController.cpp
        src/ui/BackdropItem.hpp
        src/ui/BackdropItem.cpp
        src/ui/SpectrumView.hpp
        src/ui/SpectrumView.cpp
        src/ui/WindowFrame.hpp
        src/ui/WindowFrame.cpp
        src/ui/RoundBox.hpp
        src/ui/RoundBox.cpp
        src/ui/WindowEdges.hpp
        src/ui/WindowEdges.cpp
        src/library/Metadata.hpp
        src/library/Metadata.cpp
        src/library/TrackSort.hpp
        src/library/TrackSort.cpp
        src/library/Loudness.hpp
        src/library/Loudness.cpp
        src/library/LibraryModel.hpp
        src/library/LibraryModel.cpp
        src/library/TrackFilterModel.hpp
        src/library/TrackFilterModel.cpp
        src/library/AlbumModel.hpp
        src/library/AlbumModel.cpp
        src/library/FolderTreeModel.hpp
        src/library/FolderTreeModel.cpp
        src/library/TrackListModel.hpp
        src/library/TrackListModel.cpp
        src/library/PlaylistManager.hpp
        src/library/PlaylistManager.cpp
        src/library/CoverImageProvider.hpp
        src/library/CoverImageProvider.cpp
        src/library/OnlineArt.hpp
        src/library/OnlineArt.cpp
)

# Precompiled shaders (sources and build.sh next to them; Qt Shader Tools is
# only needed to change them).
qt_add_resources(Sonkore "shaders"
    PREFIX "/shaders"
    BASE qml/shaders
    FILES
        qml/shaders/backdrop.frag.qsb
)

# Application icon (window / tray / about) from the generated set.
qt_add_resources(Sonkore "appicon"
    PREFIX "/icons"
    BASE packaging/icons
    FILES
        packaging/icons/sonkore.svg
        packaging/icons/png/16.png
        packaging/icons/png/22.png
        packaging/icons/png/24.png
        packaging/icons/png/32.png
        packaging/icons/png/48.png
        packaging/icons/png/64.png
        packaging/icons/png/128.png
        packaging/icons/png/256.png
        packaging/icons/png/512.png
)

# Interface fonts (SIL Open Font License; fonts/<id>/OFL.txt), regular to
# bold. Inter, the default, covers Latin (Vietnamese included), Greek
# and Cyrillic and is hinted for Windows; the others are latin subsets, and
# other scripts fall back to the system's fonts.
qt_add_resources(Sonkore "fonts"
    PREFIX "/fonts"
    BASE fonts
    FILES
        fonts/inter/400.ttf
        fonts/inter/500.ttf
        fonts/inter/600.ttf
        fonts/inter/700.ttf
        fonts/manrope/400.ttf
        fonts/manrope/500.ttf
        fonts/manrope/600.ttf
        fonts/manrope/700.ttf
        fonts/plus-jakarta-sans/400.ttf
        fonts/plus-jakarta-sans/500.ttf
        fonts/plus-jakarta-sans/600.ttf
        fonts/plus-jakarta-sans/700.ttf
        fonts/figtree/400.ttf
        fonts/figtree/500.ttf
        fonts/figtree/600.ttf
        fonts/figtree/700.ttf
        fonts/outfit/400.ttf
        fonts/outfit/500.ttf
        fonts/outfit/600.ttf
        fonts/outfit/700.ttf
        fonts/dm-sans/400.ttf
        fonts/dm-sans/500.ttf
        fonts/dm-sans/600.ttf
        fonts/dm-sans/700.ttf
        fonts/nunito-sans/400.ttf
        fonts/nunito-sans/500.ttf
        fonts/nunito-sans/600.ttf
        fonts/nunito-sans/700.ttf
        fonts/rubik/400.ttf
        fonts/rubik/500.ttf
        fonts/rubik/600.ttf
        fonts/rubik/700.ttf
        fonts/sora/700.ttf
)

target_sources(Sonkore PRIVATE src/platform/WinTaskbar.hpp src/platform/WinTaskbar.cpp
    src/platform/DeviceReservation.hpp src/platform/DeviceReservation.cpp)

# Interface translations (English is the source text). Edit
# i18n/translations_*.py and run i18n/update.py to refresh the .ts files;
# the compiled .qm files are embedded under :/i18n.
find_package(Qt6 6.5 REQUIRED COMPONENTS LinguistTools)
# (lrelease + resources directly: the project path has spaces, which the
# deferred lupdate setup of qt_add_translations cannot handle.)
qt_add_lrelease(
    TS_FILES
        i18n/sonkore_es.ts
        i18n/sonkore_pt_BR.ts
        i18n/sonkore_fr.ts
        i18n/sonkore_de.ts
        i18n/sonkore_it.ts
    OPTIONS -silent
    QM_FILES_OUTPUT_VARIABLE SONKORE_QM_FILES)
qt_add_resources(Sonkore "translations"
    PREFIX "/i18n"
    BASE ${CMAKE_CURRENT_BINARY_DIR}
    FILES ${SONKORE_QM_FILES})

target_compile_definitions(Sonkore PRIVATE PROJECT_VERSION_STRING="${PROJECT_VERSION}")
target_include_directories(Sonkore PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/src/ui
    ${CMAKE_CURRENT_SOURCE_DIR}/src/library
    ${CMAKE_CURRENT_SOURCE_DIR}/src/platform)
target_link_libraries(Sonkore PRIVATE
    sonkore_audio sonkore::ebur128 Qt6::Core Qt6::Gui Qt6::Widgets Qt6::Qml Qt6::Quick Qt6::QuickPrivate Qt6::QuickControls2 Qt6::Network Qt6::Svg)

# Linux: MPRIS media controls (GNOME/KDE media widgets, media keys, playerctl).
if(TARGET Qt6::DBus)
    target_sources(Sonkore PRIVATE src/platform/MprisService.hpp src/platform/MprisService.cpp
                                   src/platform/RealtimeKit.hpp src/platform/RealtimeKit.cpp)
    target_compile_definitions(Sonkore PRIVATE SONKORE_HAVE_MPRIS SONKORE_HAVE_DBUS)
    target_link_libraries(Sonkore PRIVATE Qt6::DBus)
endif()

# Linux: the DAC's own volume control (ALSA mixer), so the volume can change
# without leaving the bit-perfect path.
if(UNIX AND NOT APPLE)
    if(ALSA_FOUND)
        target_sources(Sonkore PRIVATE src/platform/AlsaMixer.hpp src/platform/AlsaMixer.cpp)
        target_compile_definitions(Sonkore PRIVATE SONKORE_HAVE_ALSA_MIXER)
        target_link_libraries(Sonkore PRIVATE ALSA::ALSA)
    endif()
    message(STATUS "Sonkore: native DSD and hardware (ALSA mixer) volume ${ALSA_FOUND}")
    # The PipeWire output's volume (the DAC's own control through PipeWire):
    # needs the headers only, like the output itself.
    if(PIPEWIRE_FOUND)
        target_sources(Sonkore PRIVATE src/platform/PipeWireVolume.hpp src/platform/PipeWireVolume.cpp)
        target_compile_definitions(Sonkore PRIVATE SONKORE_HAVE_PIPEWIRE_VOLUME)
    endif()
endif()

set_target_properties(Sonkore PROPERTIES
    # The QML module URI "Sonkore" creates build/Sonkore/ (qmldir), so the
    # binary must live elsewhere or the linker would collide with that folder.
    RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin
    WIN32_EXECUTABLE ON)

if(WIN32)
    # Exe icon + version info, the version taken from project().
    set(SONKORE_ICO "${CMAKE_CURRENT_SOURCE_DIR}/packaging/windows/sonkore.ico")
    configure_file(packaging/windows/sonkore.rc.in ${CMAKE_CURRENT_BINARY_DIR}/sonkore.rc @ONLY)
    target_sources(Sonkore PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/sonkore.rc)
    target_link_libraries(Sonkore PRIVATE ole32 uuid)               # ITaskbarList3
    # The DAC's own volume control (IAudioEndpointVolume), for exclusive mode.
    target_sources(Sonkore PRIVATE src/platform/WinEndpointVolume.hpp src/platform/WinEndpointVolume.cpp)
    target_compile_definitions(Sonkore PRIVATE SONKORE_HAVE_WIN_ENDPOINT_VOLUME)
endif()
if(MSVC)
    target_compile_options(Sonkore PRIVATE /utf-8)
endif()

# -----------------------------------------------------------------------------
# Install + Qt runtime deployment (windeployqt / macdeployqt / Linux layout)
# -----------------------------------------------------------------------------
include(GNUInstallDirs)
install(TARGETS Sonkore
    BUNDLE  DESTINATION .
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

if(UNIX AND NOT APPLE)
    install(FILES packaging/linux/sonkore.desktop DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
    install(FILES packaging/linux/sonkore.metainfo.xml DESTINATION ${CMAKE_INSTALL_DATADIR}/metainfo)
    install(FILES packaging/icons/sonkore.svg DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps)
    install(FILES packaging/icons/sonkore-symbolic.svg DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/symbolic/apps)
    foreach(size 16 22 24 32 48 64 128 256 512)
        install(FILES packaging/icons/png/${size}.png
                DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/${size}x${size}/apps
                RENAME sonkore.png)
    endforeach()
endif()

qt_generate_deploy_qml_app_script(
    TARGET Sonkore
    OUTPUT_SCRIPT sonkore_deploy_script
    NO_UNSUPPORTED_PLATFORM_ERROR)
install(SCRIPT ${sonkore_deploy_script})
