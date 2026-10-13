#include "DspController.hpp"
#include "TrackSort.hpp"
#include "SettingsLock.hpp"
#include "AppPaths.hpp"

#include "DSPManager.hpp"
#include "LibraryModel.hpp"
#include "DsdDecoder.hpp"

#if defined(SONKORE_HAVE_ALSA_MIXER)
#include "AlsaMixer.hpp"
#define SONKORE_HAVE_HW_VOLUME
#elif defined(SONKORE_HAVE_WIN_ENDPOINT_VOLUME)
#include "WinEndpointVolume.hpp"
#define SONKORE_HAVE_HW_VOLUME
#endif
#if defined(SONKORE_HAVE_DBUS)
#include "RealtimeKit.hpp"
#endif

#include <QCollator>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocale>
#include <QSaveFile>
#include <QStandardPaths>
#include <QRandomGenerator>
#include <QSettings>
#include <QThread>

#include <algorithm>
#include <complex>
#include <vector>
#include <functional>
#include <cmath>
#include <numbers>
#include <filesystem>
#include <limits>
#include <numeric>
#include <string>

namespace {

std::filesystem::path toFsPath(const QString& path)
{
#if defined(_WIN32)
    return std::filesystem::path(path.toStdWString());
#else
    return std::filesystem::path(QFile::encodeName(path).toStdString());
#endif
}

QString fromFsPath(const std::filesystem::path& path)
{
#if defined(_WIN32)
    return QString::fromStdWString(path.wstring());
#else
    return QFile::decodeName(QByteArray::fromStdString(path.string()));
#endif
}

QString sessionPath()
{
    return sonkore::paths::dataFile(QStringLiteral("session.json"));
}

// Doubles are stored as text with 17 significant digits: the exact binary64
// value comes back, so a -0.3176660 semitone setting survives a restart.
QString exactText(double value)
{
    return QString::number(value, 'g', 17);
}

double exactValue(const QVariant& stored, double fallback)
{
    bool ok = false;
    const double v = stored.toString().toDouble(&ok);
    return ok && std::isfinite(v) ? v : fallback;
}

constexpr int kSpectrumBands = 24;
constexpr std::size_t kFftSize = 2048;

// In-place radix-2 FFT (size is a power of two).
void fft(std::vector<std::complex<float>>& a)
{
    const std::size_t n = a.size();
    for (std::size_t i = 1, j = 0; i < n; ++i) {
        std::size_t bit = n >> 1;
        for (; j & bit; bit >>= 1)
            j ^= bit;
        j ^= bit;
        if (i < j)
            std::swap(a[i], a[j]);
    }
    for (std::size_t len = 2; len <= n; len <<= 1) {
        const float angle = -2.0f * std::numbers::pi_v<float> / static_cast<float>(len);
        const std::complex<float> wlen(std::cos(angle), std::sin(angle));
        for (std::size_t i = 0; i < n; i += len) {
            std::complex<float> w(1.0f, 0.0f);
            for (std::size_t k = 0; k < len / 2; ++k) {
                const std::complex<float> u = a[i + k];
                const std::complex<float> v = a[i + k + len / 2] * w;
                a[i + k] = u + v;
                a[i + k + len / 2] = u - v;
                w *= wlen;
            }
        }
    }
}

QVariantMap trackInfo(const QString& path)
{
    if (LibraryModel* lib = LibraryModel::instance())
        return lib->track(path);
    return {{"path", path}, {"title", path.section('/', -1)}};
}

} // namespace

DspController::DspController(sonkore::DSPManager& engine, QObject* parent)
    : QObject(parent)
    , m_engine(engine)
{
    loadSettings();
    refreshDevices();

    // USB DACs and other cards appear in /dev/snd; PipeWire publishes its
    // sink a moment later, hence the delay before enumerating again.
    m_hotplugTimer.setSingleShot(true);
    m_hotplugTimer.setInterval(1500);
    connect(&m_hotplugTimer, &QTimer::timeout, this, &DspController::devicesChangedOnSystem);
    if (QFileInfo(QStringLiteral("/dev/snd")).isDir()) {
        m_deviceWatcher.addPath(QStringLiteral("/dev/snd"));
        connect(&m_deviceWatcher, &QFileSystemWatcher::directoryChanged, &m_hotplugTimer, qOverload<>(&QTimer::start));
    }

    m_spectrumTimer.setInterval(33);
    connect(&m_spectrumTimer, &QTimer::timeout, this, &DspController::updateSpectrum);
    connect(this, &DspController::stateChanged, this, &DspController::updateSpectrumTimer);
    m_spectrum = QList<qreal>(kSpectrumBands, 0.0);

    m_saveTimer.setSingleShot(true);
    m_saveTimer.setInterval(500);
    connect(&m_saveTimer, &QTimer::timeout, this, &DspController::saveSettings);

    // A paused player should not keep other programs off the DAC forever.
    m_idleReleaseTimer.setSingleShot(true);
    m_idleReleaseTimer.setInterval(60 * 1000);
    connect(&m_idleReleaseTimer, &QTimer::timeout, this, &DspController::releaseOutput);
    connect(this, &DspController::stateChanged, this, &DspController::updateIdleRelease);
    connect(&m_reservation, &DeviceReservation::releaseRequested, this, [this] {
        releaseOutput();
        emit outputTakenByOther();
    });

    m_pollTimer.setInterval(50);
    connect(&m_pollTimer, &QTimer::timeout, this, &DspController::poll);
    // Whatever changes what plays next re-arms the gapless join.
    connect(this, &DspController::queueChanged, this, &DspController::armGapless);
    connect(this, &DspController::shuffleChanged, this, &DspController::armGapless);
    connect(this, &DspController::repeatModeChanged, this, &DspController::armGapless);
    if (LibraryModel* lib = LibraryModel::instance()) {
        // Gains measured for the track playing (or the queued one) apply at once.
        connect(lib, &LibraryModel::loudnessUpdated, this, [this](const QStringList& paths) {
            if (paths.contains(m_currentPath) || paths.contains(m_armedPath)) {
                m_currentTrack = trackInfo(m_currentPath);
                applyGains();
            }
        });
        lib->setAutoAnalyzeLoudness(m_rgMode != ReplayGainOff);
    }
#if defined(SONKORE_HAVE_HW_VOLUME)
    m_mixer = new HardwareMixer(this);
    connect(m_mixer, &HardwareMixer::volumeChanged, this, &DspController::volumeChanged);
#endif
    updateMixer();
    m_pollTimer.start();
}

void DspController::flush()
{
    m_resumePosition = loaded() ? m_engine.positionSeconds() : m_resumePosition;
    m_saveTimer.stop();
    saveSettings();
}

DspController::~DspController()
{
    m_resumePosition = loaded() ? m_engine.positionSeconds() : m_resumePosition;
    m_saveTimer.stop();
    saveSettings();
    closeOutput();
}

void DspController::closeOutput()
{
    if (loaded()) {
        m_resumePosition = m_engine.positionSeconds();
        m_engine.unload();
    }
    // Only now: the sound server re-reads the card's profiles as soon as it
    // gets it back, and a card still open by us would look half-usable to
    // it (it would remember the DAC as "off").
    m_reservation.release();
}

// --- Pitch / tempo / rate ---------------------------------------------------

double DspController::pitchSemitones() const { return m_engine.timeStretch().pitchSemiTones(); }
double DspController::tempo() const { return m_engine.timeStretch().tempo(); }
double DspController::rate() const { return m_engine.timeStretch().rate(); }
bool DspController::soundTouchActive() const { return !m_engine.timeStretch().isNeutral(); }
double DspController::speed() const { return m_engine.timeStretch().speedFactor(); }
bool DspController::soundTouchEnabled() const { return m_engine.timeStretch().enabled(); }

void DspController::setSoundTouchEnabled(bool on)
{
    if (on == soundTouchEnabled())
        return;
    m_engine.timeStretch().setEnabled(on);
    emit soundTouchActiveChanged();
    scheduleSave();
}

void DspController::setPitchSemitones(double semitones)
{
    if (!std::isfinite(semitones) || semitones == pitchSemitones())
        return;
    m_engine.timeStretch().setPitchSemiTones(semitones);
    emit pitchSemitonesChanged();
    emit soundTouchActiveChanged();
    scheduleSave();
}

void DspController::setTempo(double ratio)
{
    if (!std::isfinite(ratio) || ratio == tempo())
        return;
    m_engine.timeStretch().setTempo(ratio);
    emit tempoChanged();
    emit soundTouchActiveChanged();
    scheduleSave();
}

void DspController::setRate(double ratio)
{
    if (!std::isfinite(ratio) || ratio == rate())
        return;
    m_engine.timeStretch().setRate(ratio);
    emit rateChanged();
    emit soundTouchActiveChanged();
    scheduleSave();
}

// --- Engine choice -------------------------------------------------------------

int DspController::pitchEngine() const { return static_cast<int>(m_engine.timeStretch().engine()); }

QString DspController::pitchEngineName() const
{
    return pitchEngine() == SoundTouch ? QStringLiteral("SoundTouch") : QStringLiteral("Rubber Band");
}

void DspController::setPitchEngine(int engine)
{
    engine = std::clamp(engine, static_cast<int>(RubberBand), static_cast<int>(SoundTouch));
    if (engine == pitchEngine())
        return;
    m_engine.timeStretch().setEngine(static_cast<sonkore::TimeStretch::Engine>(engine));
    emit pitchEngineChanged();
    scheduleSave();
}

// --- Rubber Band ----------------------------------------------------------------

bool DspController::rubberBandFiner() const { return m_engine.timeStretch().rubberBandSettings().finer; }
bool DspController::preserveFormants() const { return m_engine.timeStretch().rubberBandSettings().preserveFormants; }
bool DspController::channelsTogether() const { return m_engine.timeStretch().rubberBandSettings().channelsTogether; }
int DspController::pitchMethod() const { return m_engine.timeStretch().rubberBandSettings().pitchMethod; }
int DspController::rubberBandWindow() const { return m_engine.timeStretch().rubberBandSettings().window; }
int DspController::transients() const { return m_engine.timeStretch().rubberBandSettings().transients; }
int DspController::detector() const { return m_engine.timeStretch().rubberBandSettings().detector; }
bool DspController::phaseIndependent() const { return m_engine.timeStretch().rubberBandSettings().phaseIndependent; }
bool DspController::rubberBandSmoothing() const { return m_engine.timeStretch().rubberBandSettings().smoothing; }

void DspController::setRubberBandFiner(bool on)
{
    if (on == rubberBandFiner())
        return;
    m_engine.timeStretch().setRubberBandFiner(on);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setPreserveFormants(bool on)
{
    if (on == preserveFormants())
        return;
    m_engine.timeStretch().setPreserveFormants(on);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setPitchMethod(int method)
{
    if (method == pitchMethod())
        return;
    m_engine.timeStretch().setPitchMethod(method);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setRubberBandWindow(int window)
{
    if (window == rubberBandWindow())
        return;
    m_engine.timeStretch().setRubberBandWindow(window);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setTransients(int transients)
{
    if (transients == this->transients())
        return;
    m_engine.timeStretch().setTransients(transients);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setDetector(int detector)
{
    if (detector == this->detector())
        return;
    m_engine.timeStretch().setDetector(detector);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setPhaseIndependent(bool on)
{
    if (on == phaseIndependent())
        return;
    m_engine.timeStretch().setPhaseIndependent(on);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setRubberBandSmoothing(bool on)
{
    if (on == rubberBandSmoothing())
        return;
    m_engine.timeStretch().setRubberBandSmoothing(on);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

void DspController::setChannelsTogether(bool on)
{
    if (on == channelsTogether())
        return;
    m_engine.timeStretch().setChannelsTogether(on);
    emit rubberBandSettingsChanged();
    scheduleSave();
}

// --- SoundTouch advanced ----------------------------------------------------

bool DspController::useAAFilter() const { return m_engine.timeStretch().soundTouchSettings().useAAFilter; }
int DspController::aaFilterLength() const { return m_engine.timeStretch().soundTouchSettings().aaFilterLength; }
bool DspController::useQuickSeek() const { return m_engine.timeStretch().soundTouchSettings().useQuickSeek; }
int DspController::sequenceMs() const { return m_engine.timeStretch().soundTouchSettings().sequenceMs; }
int DspController::seekWindowMs() const { return m_engine.timeStretch().soundTouchSettings().seekWindowMs; }
int DspController::overlapMs() const { return m_engine.timeStretch().soundTouchSettings().overlapMs; }

void DspController::setUseAAFilter(bool on)
{
    if (on == useAAFilter())
        return;
    m_engine.timeStretch().setUseAAFilter(on);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::setAAFilterLength(int taps)
{
    if (taps == aaFilterLength())
        return;
    m_engine.timeStretch().setAAFilterLength(taps);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::setUseQuickSeek(bool on)
{
    if (on == useQuickSeek())
        return;
    m_engine.timeStretch().setUseQuickSeek(on);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::setSequenceMs(int ms)
{
    if (ms == sequenceMs())
        return;
    m_engine.timeStretch().setSequenceMs(ms);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::setSeekWindowMs(int ms)
{
    if (ms == seekWindowMs())
        return;
    m_engine.timeStretch().setSeekWindowMs(ms);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::setOverlapMs(int ms)
{
    if (ms == overlapMs())
        return;
    m_engine.timeStretch().setOverlapMs(ms);
    emit soundTouchSettingsChanged();
    scheduleSave();
}

void DspController::resetSoundTouch()
{
    setPitchSemitones(0.0);
    setTempo(1.0);
    setRate(1.0);
    const sonkore::TimeStretch::SoundTouchSettings defaults;
    setUseAAFilter(defaults.useAAFilter);
    setAAFilterLength(defaults.aaFilterLength);
    setUseQuickSeek(defaults.useQuickSeek);
    setSequenceMs(defaults.sequenceMs);
    setSeekWindowMs(defaults.seekWindowMs);
    setOverlapMs(defaults.overlapMs);
    resetRubberBand();
}

void DspController::resetRubberBand()
{
    const sonkore::TimeStretch::RubberBandSettings rb;
    setRubberBandFiner(rb.finer);
    setPreserveFormants(rb.preserveFormants);
    setChannelsTogether(rb.channelsTogether);
    setPitchMethod(rb.pitchMethod);
    setRubberBandWindow(rb.window);
    setTransients(rb.transients);
    setDetector(rb.detector);
    setPhaseIndependent(rb.phaseIndependent);
    setRubberBandSmoothing(rb.smoothing);
}

// --- Output -----------------------------------------------------------------

double DspController::volume() const
{
    if (m_volumeFixed)
        return 1.0;
#if defined(SONKORE_HAVE_HW_VOLUME)
    if (m_hwVolumeActive)
        return m_mixer->volume();
#endif
    return m_engine.volume();
}

void DspController::setVolume(double v)
{
    if (m_volumeFixed) {
        emit volumeChanged(); // set on the DAC or amplifier: the slider goes back to 100 %
        return;
    }
    if (!std::isfinite(v) || v == volume())
        return;
#if defined(SONKORE_HAVE_HW_VOLUME)
    if (m_hwVolumeActive) {
        m_mixer->setVolume(v); // the DAC keeps its own setting: nothing to save
        emit volumeChanged();
        return;
    }
#endif
    m_engine.setVolume(v);
    emit volumeChanged();
    scheduleSave();
}

// --- Hardware volume ------------------------------------------------------------

bool DspController::hardwareVolumeSupported() const
{
#if defined(SONKORE_HAVE_HW_VOLUME)
    return true;
#else
    return false;
#endif
}

void DspController::setVolumeMode(int mode)
{
    mode = std::clamp(mode, 0, 2);
    if (mode == m_volumeMode)
        return;
    m_volumeMode = mode;
    updateMixer();
    scheduleSave();
}

// Where the volume is set. DAC: Sonkore moves the DAC's own control, and
// where there is none (or the output is shared) the volume stays fixed at
// 100 %: DAC never falls back to digital volume. Automatic on a direct
// output: fixed at 100 %, set on the DAC or amplifier. Either way the engine
// stays at unity gain, so the volume never costs bit-perfect. Digital, or
// Automatic on a shared output: Sonkore's own.
void DspController::updateMixer()
{
    bool active = false;
#if defined(SONKORE_HAVE_ALSA_MIXER)
    if (m_volumeMode == VolumeHardware && m_deviceKey.startsWith(QLatin1String("alsa|")))
        active = m_mixer->open(m_deviceKey.mid(5).section(QLatin1Char(','), 0, 0)); // "hw:CARD=X"
    else
        m_mixer->close();
#elif defined(SONKORE_HAVE_WIN_ENDPOINT_VOLUME)
    // The device's own control, in exclusive mode (shared mode goes through
    // the Windows mixer, where Sonkore's digital volume is the honest one).
    // (ASIO drivers have no volume of their own here: fixed at 100 %.)
    if (m_volumeMode == VolumeHardware && directOutput() && !m_deviceKey.startsWith(QLatin1String("asio|")))
        active = m_mixer->open(QString::fromStdWString(m_engine.wasapiEndpointId(m_deviceKey.toStdString())));
    else
        m_mixer->close();
#endif
    const bool fixed = (m_volumeMode == VolumeAuto && directOutput()) || (m_volumeMode == VolumeHardware && !active);
    const bool unity = active || fixed;
    if (unity != m_engineAtUnity) {
        if (unity) {
            m_digitalVolume = m_engine.volume();
            m_engine.setVolume(1.0);
        } else {
            m_engine.setVolume(m_digitalVolume);
        }
        m_engineAtUnity = unity;
    }
    m_hwVolumeActive = active;
    m_volumeFixed = fixed;
    emit volumeModeChanged();
    emit volumeChanged();
    emit outputStatusChanged();
}

bool DspController::directOutput() const
{
#if defined(_WIN32)
    // An ASIO driver, or WASAPI in exclusive mode: no Windows mixer in the path.
    return m_exclusive || m_deviceKey.startsWith(QLatin1String("asio|"));
#else
    return m_deviceKey.startsWith(QLatin1String("alsa|hw:")) || m_deviceKey.startsWith(QLatin1String("alsa|plughw:"));
#endif
}

QString DspController::hardwareVolumeInfo() const
{
    if (m_volumeMode == VolumeDigital)
        return {};
    const bool automatic = m_volumeMode == VolumeAuto;
    if (m_volumeFixed && automatic)
        return tr("Automatic, direct output: Sonkore sends the signal at 100 % and the volume is set on the DAC or the amplifier, with its own knob. The audio stays bit-perfect. Choose Digital to set it from Sonkore, or DAC to move the DAC's control from here.");
    if (m_volumeFixed) // DAC chosen, but no control to move
        return directOutput()
            ? tr("This DAC has no volume control of its own: the volume stays fixed at 100 %, set it on the DAC or the amplifier. Choose Digital to set it from Sonkore.")
#if defined(_WIN32)
            : tr("Only in exclusive mode: with a shared output the volume stays fixed at 100 %. Choose Digital to set it from Sonkore.");
#else
            : tr("Only for direct (hw:) outputs: with this one the volume stays fixed at 100 %. Choose Digital to set it from Sonkore.");
#endif
#if defined(SONKORE_HAVE_HW_VOLUME)
    if (m_hwVolumeActive) {
        QString control = tr("Using the DAC's \"%1\" control.").arg(m_mixer->controlName());
        if (m_mixer->hasDbScale())
            control = tr("Using the DAC's \"%1\" control (%2 to %3 dB).")
                          .arg(m_mixer->controlName(), QLocale().toString(std::max(m_mixer->minDb(), -150.0), 'f', 1),
                               QLocale().toString(m_mixer->topDb(), 'f', 1));
        return automatic ? tr("Automatic, direct output: %1 The audio stays bit-perfect at any volume.").arg(control) : control;
    }
#if defined(_WIN32)
    if (automatic && !directOutput())
        return tr("Automatic, shared output: Sonkore's digital volume is used. In exclusive mode the volume is set on the DAC or the amplifier instead.");
#else
    if (automatic && !directOutput())
        return tr("Automatic, shared output: Sonkore's digital volume is used. With a direct (hw:) output, the DAC's own volume is used instead.");
    if (m_deviceKey.startsWith(QLatin1String("alsa|")))
        return tr("Automatic: this DAC has no volume control of its own, so Sonkore's digital volume is used.");
#endif
#endif
    return tr("Automatic: Sonkore's digital volume is used with this output.");
}

// --- ReplayGain -----------------------------------------------------------------

// Linear gain for a track under the current settings (1.0 = untouched). `info`
// is its library entry; files outside the library have their tags read here.
double DspController::gainFor(const QString& path, QVariantMap info, bool* known) const
{
    if (known)
        *known = false;
    if (m_rgMode == ReplayGainOff || path.isEmpty())
        return 1.0;
    if (!info.contains(QStringLiteral("rgTrackGain"))) {
        sonkore::TrackMeta meta;
        sonkore::Metadata::read(path, meta);
        info = meta.toVariantMap();
    }
    const double nan = std::numeric_limits<double>::quiet_NaN();
    double gain = info.value(QStringLiteral("rgTrackGain"), nan).toDouble();
    double peak = info.value(QStringLiteral("rgTrackPeak"), 0.0).toDouble();
    const double albumGain = info.value(QStringLiteral("rgAlbumGain"), nan).toDouble();
    if (m_rgMode == ReplayGainAlbum && !std::isnan(albumGain)) {
        gain = albumGain;
        peak = info.value(QStringLiteral("rgAlbumPeak"), 0.0).toDouble();
    }
    if (std::isnan(gain))
        return 1.0; // no loudness data (yet): played as is
    if (known)
        *known = true;
    double linear = std::pow(10.0, (gain + m_rgPreamp) / 20.0);
    if (m_rgPreventClipping && peak > 0.0)
        linear = std::min(linear, 1.0 / peak); // the loudest peak lands at full scale, not beyond
    return linear;
}

// Hands the engine the gains for the current and the queued track.
void DspController::applyGains()
{
    bool known = false;
    const double current = gainFor(m_currentPath, m_currentTrack, &known);
    if (loaded())
        m_engine.setGains(current, gainFor(m_armedPath, m_armedPath.isEmpty() ? QVariantMap{} : trackInfo(m_armedPath)));
    m_currentGainDb = known ? 20.0 * std::log10(current) : std::numeric_limits<double>::quiet_NaN();
    emit replayGainChanged();
    emit outputStatusChanged(); // a gain other than 1 leaves the bit-perfect path
}

void DspController::setReplayGainMode(int mode)
{
    mode = std::clamp(mode, 0, 2);
    if (mode == m_rgMode)
        return;
    m_rgMode = mode;
    if (LibraryModel* lib = LibraryModel::instance())
        lib->setAutoAnalyzeLoudness(mode != ReplayGainOff);
    applyGains();
    scheduleSave();
}

void DspController::setReplayGainPreamp(double db)
{
    db = std::clamp(db, -15.0, 15.0);
    if (db == m_rgPreamp)
        return;
    m_rgPreamp = db;
    applyGains();
    scheduleSave();
}

void DspController::setReplayGainPreventClipping(bool on)
{
    if (on == m_rgPreventClipping)
        return;
    m_rgPreventClipping = on;
    applyGains();
    scheduleSave();
}

void DspController::setDitherMode(int mode)
{
    mode = std::clamp(mode, 0, 2);
    if (mode == m_ditherMode)
        return;
    m_ditherMode = mode;
    m_engine.setDither(static_cast<sonkore::DitherMode>(mode));
    emit ditherChanged();
    scheduleSave();
}

void DspController::setCrossfeed(int preset)
{
    preset = std::clamp(preset, 0, 3);
    if (preset == m_crossfeed)
        return;
    m_crossfeed = preset;
    m_engine.setCrossfeed(static_cast<sonkore::Crossfeed::Preset>(preset));
    emit crossfeedChanged();
    scheduleSave();
}

void DspController::setUpsampling(int mode)
{
    mode = std::clamp(mode, 0, 3);
    if (mode == m_upsampling)
        return;
    m_upsampling = mode;
    auto cfg = m_engine.outputConfig();
    cfg.upsampling = static_cast<sonkore::DSPManager::Upsampling>(mode);
    m_engine.setOutputConfig(cfg);
    emit resamplingChanged();
    scheduleSave();
    reopenCurrent(); // the device opens at the new rate
}

// Hands the engine the impulse response (or none) per the current settings.
bool DspController::applyConvolution(QString* error)
{
    std::string why;
    const bool on = m_convolutionEnabled && !m_convolutionFile.isEmpty();
    if (m_engine.setConvolution(on ? toFsPath(m_convolutionFile) : std::filesystem::path(), &why))
        return true;
    if (error)
        *error = QString::fromStdString(why);
    m_engine.setConvolution({});
    return false;
}

bool DspController::loadConvolution(const QUrl& file)
{
    const QString path = file.isLocalFile() ? file.toLocalFile() : file.toString();
    const QString previous = m_convolutionFile;
    const bool wasEnabled = m_convolutionEnabled;
    m_convolutionFile = path;
    m_convolutionEnabled = true;
    QString error;
    if (!applyConvolution(&error)) {
        m_convolutionFile = previous;
        m_convolutionEnabled = wasEnabled;
        applyConvolution();
        reportError(tr("Could not use this impulse response: %1").arg(error));
        return false;
    }
    emit convolutionChanged();
    scheduleSave();
    return true;
}

void DspController::clearConvolution()
{
    m_convolutionFile.clear();
    m_convolutionEnabled = false;
    applyConvolution();
    emit convolutionChanged();
    scheduleSave();
}

void DspController::setConvolutionEnabled(bool on)
{
    if (on == m_convolutionEnabled)
        return;
    m_convolutionEnabled = on;
    QString error;
    if (!applyConvolution(&error)) {
        m_convolutionEnabled = false;
        reportError(tr("Could not use this impulse response: %1").arg(error));
    }
    emit convolutionChanged();
    scheduleSave();
}

void DspController::setResampleFilter(int filter)
{
    filter = std::clamp(filter, 0, 3);
    if (filter == m_resampleFilter)
        return;
    m_resampleFilter = filter;
    auto cfg = m_engine.outputConfig();
    cfg.filter = static_cast<sonkore::Resampler::Filter>(filter);
    m_engine.setOutputConfig(cfg);
    emit resamplingChanged();
    scheduleSave();
    if (m_engine.outputStatus().resampled)
        reopenCurrent();
}

double DspController::clippedSamples() const
{
    return static_cast<double>(m_engine.clippedSamples());
}

void DspController::setHeadroomDb(double db)
{
    db = std::clamp(std::isfinite(db) ? db : 0.0, -24.0, 0.0);
    if (db == m_headroomDb)
        return;
    m_headroomDb = db;
    applyHeadroom();
    emit headroomChanged();
    scheduleSave();
}

void DspController::setHeadroomAuto(bool on)
{
    if (on == m_headroomAuto)
        return;
    m_headroomAuto = on;
    applyHeadroom();
    emit headroomChanged();
    scheduleSave();
}

// Automatic headroom: 3 dB for the inter-sample peaks that filtering, pitch
// shifting and resampling bring out of masters cut at 0 dBFS (the margin Roon
// uses), plus whatever the EQ curve lifts above 0 dB. The engine applies it
// only while some DSP runs; a bit-perfect path is never touched.
void DspController::applyHeadroom()
{
    constexpr double kAutoMarginDb = 3.0;
    const double db = m_headroomAuto
        ? std::clamp(-(kAutoMarginDb + std::max(0.0, m_engine.equalizer().peakGainDb())), -24.0, 0.0)
        : m_headroomDb;
    const double rounded = std::round(db * 10.0) / 10.0;
    if (rounded == m_effectiveHeadroomDb)
        return;
    m_effectiveHeadroomDb = rounded;
    m_engine.setHeadroomDb(rounded);
    m_engine.resetClippedSamples();
    emit headroomChanged();
}

void DspController::setExclusiveMode(bool on)
{
    if (on == m_exclusive)
        return;
    m_exclusive = on;
    auto cfg = m_engine.outputConfig();
    cfg.exclusiveMode = on;
    m_engine.setOutputConfig(cfg);
    emit exclusiveModeChanged();
    scheduleSave();
    updateMixer(); // Windows: exclusive mode is what makes the output direct
    reopenCurrent();
}

namespace {
sonkore::DSPManager::DsdOutput toEngineDsd(int mode)
{
    using D = sonkore::DSPManager::DsdOutput;
    return mode == DspController::DsdNative ? D::Native : mode == DspController::DsdDop ? D::Dop : D::Pcm;
}
} // namespace

void DspController::setDsdMode(int mode)
{
    mode = std::clamp(mode, 0, 2);
    if (mode == m_dsdMode)
        return;
    m_dsdMode = mode;
    auto cfg = m_engine.outputConfig();
    cfg.dsd = toEngineDsd(mode);
    m_engine.setOutputConfig(cfg);
    emit dsdModeChanged();
    scheduleSave();
    reopenCurrent();
}

void DspController::setPcmToDsd(int mode)
{
    mode = std::clamp(mode, 0, 4);
    if (mode == m_pcmToDsd)
        return;
    m_pcmToDsd = mode;
    auto cfg = m_engine.outputConfig();
    cfg.pcmToDsd = static_cast<sonkore::DSPManager::PcmToDsd>(mode);
    m_engine.setOutputConfig(cfg);
    emit dsdModeChanged();
    scheduleSave();
    reopenCurrent();
}

void DspController::setPlayFromMemory(bool on)
{
    if (on == m_playFromMemory)
        return;
    m_playFromMemory = on;
    auto cfg = m_engine.outputConfig();
    cfg.playFromMemory = on;
    m_engine.setOutputConfig(cfg);
    emit playFromMemoryChanged();
    scheduleSave();
    reopenCurrent(); // the current file is loaded (or released) right away
}

void DspController::setHdcd(bool on)
{
    if (on == m_hdcd)
        return;
    m_hdcd = on;
    auto cfg = m_engine.outputConfig();
    cfg.hdcd = on;
    m_engine.setOutputConfig(cfg);
    emit hdcdChanged();
    scheduleSave();
    reopenCurrent();
}

void DspController::refreshDevices()
{
    m_devices.clear();
    const auto devices = m_engine.playbackDevices();
    // "System default" opens the sound server's default output, so it heads
    // that group (PipeWire first); without a sound server it gets its own.
    QString defaultGroup = tr("System");
    for (const auto& d : devices)
        if (d.key.rfind("pulse|", 0) == 0) {
            defaultGroup = QString::fromStdString(d.backend);
            break;
        }
    m_devices << QVariantMap{
        {"key", QString()}, {"backend", defaultGroup}, {"name", tr("System default")},
        {"detail", tr("Follows the desktop's sound settings")}, {"isDefault", true}, {"direct", false},
    };
    for (const auto& d : devices) {
        m_devices << QVariantMap{
            {"key", QString::fromStdString(d.key)},
            {"backend", QString::fromStdString(d.backend)},
            {"name", QString::fromStdString(d.name)},
            {"detail", QString::fromStdString(d.detail)},
            {"isDefault", d.isDefault},
            {"direct", d.direct},
        };
    }
    emit outputDevicesChanged();
}

// --- Visualiser ----------------------------------------------------------------

void DspController::setVisualizerActive(bool on)
{
    if (on == m_visualizerActive)
        return;
    m_visualizerActive = on;
    m_engine.setAnalysisEnabled(on);
    updateSpectrumTimer();
    emit visualizerActiveChanged();
}

void DspController::updateSpectrumTimer()
{
    const bool run = m_visualizerActive && playing();
    if (run && !m_spectrumTimer.isActive()) {
        m_spectrumTimer.start();
    } else if (!run && m_spectrumTimer.isActive()) {
        m_spectrumTimer.stop();
        m_spectrum.fill(0.0); // the bars fall back to rest
        emit spectrumChanged();
    }
}

void DspController::updateSpectrum()
{
    static std::vector<float> samples(kFftSize);
    static std::vector<std::complex<float>> bins(kFftSize);
    static std::vector<float> window = [] {
        std::vector<float> w(kFftSize);
        for (std::size_t i = 0; i < kFftSize; ++i) // Hann
            w[i] = 0.5f - 0.5f * std::cos(2.0f * std::numbers::pi_v<float> * i / (kFftSize - 1));
        return w;
    }();

    // The newest samples are still in the device buffer: look one buffer back
    // so the bars move with what is heard.
    if (!m_engine.recentOutput(samples.data(), kFftSize, m_engine.outputLatencyFrames()))
        return;
    for (std::size_t i = 0; i < kFftSize; ++i)
        bins[i] = {samples[i] * window[i], 0.0f};
    fft(bins);

    const double rate = std::max<uint32_t>(8000, m_engine.outputSampleRate());
    const double lo = 40.0, hi = std::min(16000.0, rate / 2.0 - 1.0);
    const double binHz = rate / kFftSize;
    for (int b = 0; b < kSpectrumBands; ++b) {
        const double f0 = lo * std::pow(hi / lo, static_cast<double>(b) / kSpectrumBands);
        const double f1 = lo * std::pow(hi / lo, static_cast<double>(b + 1) / kSpectrumBands);
        const std::size_t i0 = std::clamp<std::size_t>(static_cast<std::size_t>(f0 / binHz), 1, kFftSize / 2 - 1);
        const std::size_t i1 = std::clamp<std::size_t>(static_cast<std::size_t>(f1 / binHz) + 1, i0 + 1, kFftSize / 2);
        float peak = 0.0f;
        for (std::size_t i = i0; i < i1; ++i)
            peak = std::max(peak, std::abs(bins[i]));
        // Full-scale sine with a Hann window ~ N/4; map -66..0 dBFS to 0..1.
        const double db = 20.0 * std::log10(std::max(1e-9, peak / (kFftSize / 4.0)));
        m_spectrum[b] = std::clamp((db + 66.0) / 66.0, 0.0, 1.0);
    }
    emit spectrumChanged();
}

void DspController::devicesChangedOnSystem()
{
    auto names = [](const QVariantList& list) {
        QStringList out;
        for (const QVariant& v : list)
            if (!v.toMap().value("key").toString().isEmpty())
                out << v.toMap().value("name").toString();
        return out;
    };
    const QStringList before = names(m_devices);
    refreshDevices();
    const QStringList after = names(m_devices);
    QStringList added, removed;
    for (const QString& n : after)
        if (!before.contains(n))
            added << n;
    for (const QString& n : before)
        if (!after.contains(n))
            removed << n;
    added.removeDuplicates();
    removed.removeDuplicates();
    if (!added.isEmpty() || !removed.isEmpty())
        emit outputDevicesHotplugged(added, removed);
}

void DspController::selectOutputDevice(const QString& key)
{
    if (key == m_deviceKey)
        return;
    m_deviceKey = key;
    auto cfg = m_engine.outputConfig();
    cfg.deviceKey = key.toStdString();
    m_engine.setOutputConfig(cfg);
    scheduleSave();
    if (!loaded())
        m_reservation.release(); // taken again when something plays
    emit outputStatusChanged();
    updateMixer();
    reopenCurrent();
}

// The card behind the selected output when it is a direct ALSA device.
static int directCardOf(const QString& deviceKey)
{
    if (!deviceKey.startsWith(QLatin1String("alsa|")))
        return -1;
    return DeviceReservation::cardIndexOf(deviceKey.mid(5));
}

bool DspController::reserveOutput(bool* justTaken)
{
    *justTaken = false;
    const int card = directCardOf(m_deviceKey);
    if (card == m_reservation.card())
        return true;
    if (m_reservation.held()) {
        m_engine.unload(); // close our handle before the card goes back
        m_reservation.release();
        emit outputStatusChanged();
    }
    if (card < 0)
        return true;
    QString holder;
    if (!m_reservation.acquire(card, &holder)) {
        reportError(tr("The DAC is in use by %1 and it did not let go of it.").arg(holder));
        return false;
    }
    *justTaken = true;
    emit outputStatusChanged();
    return true;
}

void DspController::releaseOutput()
{
    m_idleReleaseTimer.stop();
    closeOutput();
    emit stateChanged();
    emit outputStatusChanged();
    poll();
}

void DspController::updateIdleRelease()
{
    if (m_reservation.held() && m_releaseWhenIdle && !playing())
        m_idleReleaseTimer.start();
    else
        m_idleReleaseTimer.stop();
}

void DspController::setReleaseWhenIdle(bool on)
{
    if (on == m_releaseWhenIdle)
        return;
    m_releaseWhenIdle = on;
    updateIdleRelease();
    emit preferencesChanged();
    scheduleSave();
}

// Re-opens the current track on the (new) output, keeping position and state.
void DspController::reopenCurrent()
{
    if (!loaded() || m_currentIndex < 0)
        return;
    const bool wasPlaying = playing();
    openIndex(m_currentIndex, wasPlaying, m_engine.positionSeconds());
}

QString DspController::outputDescription() const
{
    const auto s = m_engine.outputStatus();
    if (s.backend.empty())
        return tr("Output idle");
    if (!s.dsd.empty()) // "ALSA · Topping DX1 · DSD64 native · 2.8224 MHz"
        return tr("%1 · %2 · %3 · %4 MHz")
            .arg(QString::fromStdString(s.backend), QString::fromStdString(s.deviceName), QString::fromStdString(s.dsd))
            .arg((s.pcmToDsdRate ? s.pcmToDsdRate : m_engine.trackInfo().dsdRate) / 1e6, 0, 'g', 6);
    QString text = tr("%1 · %2 · %3 kHz · %4%5%6")
        .arg(QString::fromStdString(s.backend), QString::fromStdString(s.deviceName))
        .arg(s.deviceSampleRate / 1000.0, 0, 'g', 6)
        .arg(s.deviceFloat ? QStringLiteral("f") : QStringLiteral("s"))
        .arg(s.deviceBits)
        .arg(s.exclusive ? tr(" · exclusive") : QString());
    if (s.resampled)
        text += tr(" · resampled from %1 kHz (SoX VHQ)").arg(s.sourceSampleRate / 1000.0, 0, 'g', 6);
    return text;
}

bool DspController::bitPerfect() const { return m_engine.outputStatus().bitPerfectNow; }

QString DspController::audioComponents() const
{
    return QString::fromStdString(sonkore::DSPManager::componentVersions());
}

// --- Precision helpers ------------------------------------------------------

QVariant DspController::parseDecimal(const QString& text) const
{
    bool ok = false;
    const double v = QLocale::c().toDouble(text.trimmed(), &ok);
    if (!ok || !std::isfinite(v))
        return {};
    return v;
}

QString DspController::formatDecimal(double value, int decimals) const
{
    QString s = QString::number(value, 'f', std::clamp(decimals, 0, 15));
    if (s.startsWith(u'-') && s.toDouble() == 0.0) // "-0.0000000" -> "0.0000000"
        s.remove(0, 1);
    return s;
}

double DspController::stepValue(double value, double step, int direction, int decimals,
                                double minimum, double maximum) const
{
    const double raw = value + static_cast<double>(direction) * step;
    // Round-trip through the decimal text the user would see, so the result
    // is exactly the double that typing that text would have produced.
    const double snapped = QString::number(raw, 'f', std::clamp(decimals, 0, 15)).toDouble();
    return std::clamp(snapped == 0.0 ? 0.0 : snapped, minimum, maximum); // no -0.0
}

double DspController::pitchRatio(double semitones) const
{
    return std::exp2(semitones / 12.0);
}

QString DspController::formatTime(double seconds) const
{
    const int s = std::max(0, static_cast<int>(seconds));
    if (s >= 3600)
        return QStringLiteral("%1:%2:%3").arg(s / 3600).arg((s / 60) % 60, 2, 10, QChar('0')).arg(s % 60, 2, 10, QChar('0'));
    return QStringLiteral("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QChar('0'));
}

// --- Queue --------------------------------------------------------------------

void DspController::playPaths(const QStringList& paths, int startIndex)
{
    if (paths.isEmpty())
        return;
    m_queue = paths;
    startIndex = std::clamp(startIndex, 0, static_cast<int>(paths.size()) - 1);
    m_currentIndex = startIndex;
    rebuildShuffleOrder();
    emit queueChanged();
    openIndex(startIndex, true);
}

void DspController::enqueue(const QStringList& paths)
{
    if (paths.isEmpty())
        return;
    const bool wasEmpty = m_queue.isEmpty();
    m_queue << paths;
    rebuildShuffleOrder();
    emit queueChanged();
    scheduleSave();
    if (wasEmpty)
        openIndex(0, true);
}

void DspController::removeFromQueue(const QList<int>& indices)
{
    QList<int> sorted = indices;
    std::sort(sorted.begin(), sorted.end(), std::greater<int>());
    sorted.erase(std::unique(sorted.begin(), sorted.end()), sorted.end());
    bool changed = false;
    for (int i : sorted) {
        if (i < 0 || i >= m_queue.size())
            continue;
        m_queue.removeAt(i);
        changed = true;
        // Keep pointing at the entry before the removed one, so "next" plays
        // whatever followed it; the loaded track keeps playing.
        if (i <= m_currentIndex)
            --m_currentIndex;
    }
    if (!changed)
        return;
    m_currentIndex = std::min(m_currentIndex, static_cast<int>(m_queue.size()) - 1);
    rebuildShuffleOrder();
    emit queueChanged();
    emit trackChanged();
    scheduleSave();
}

void DspController::forgetFolder(const QString& folder)
{
    QString prefix = QDir::cleanPath(folder);
    if (prefix.isEmpty())
        return;
    if (!prefix.endsWith('/'))
        prefix += '/';
    const auto inside = [&](const QString& path) { return path.startsWith(prefix); };

    if (inside(m_currentPath)) {
        m_engine.unload();
        m_reservation.release();
        m_currentIndex = -1;
        m_currentPath.clear();
        m_currentTrack.clear();
        m_position = 0.0;
        m_resumePosition = 0.0;
        m_queue.removeIf(inside);
        rebuildShuffleOrder();
        emit queueChanged();
        emit trackChanged();
        emit stateChanged();
        emit positionChanged();
        emit outputStatusChanged();
        scheduleSave();
        return;
    }
    QList<int> indices;
    for (int i = 0; i < m_queue.size(); ++i)
        if (inside(m_queue[i]))
            indices << i;
    removeFromQueue(indices);
}

void DspController::moveInQueue(const QList<int>& indices, int target)
{
    const int size = static_cast<int>(m_queue.size());
    QList<int> moved;
    for (int i : indices)
        if (i >= 0 && i < size)
            moved << i;
    std::sort(moved.begin(), moved.end());
    moved.erase(std::unique(moved.begin(), moved.end()), moved.end());
    if (moved.isEmpty())
        return;
    target = std::clamp(target, 0, size);

    // New order as old indices: the untouched rows with the moved block
    // inserted where `target` was.
    QVector<bool> isMoved(size, false);
    for (int i : moved)
        isMoved[i] = true;
    QVector<int> order;
    order.reserve(size);
    for (int i = 0; i < size; ++i) {
        if (i == target)
            order += QVector<int>(moved.begin(), moved.end());
        if (!isMoved[i])
            order << i;
    }
    if (target == size)
        order += QVector<int>(moved.begin(), moved.end());
    applyQueueOrder(order);
}

void DspController::applyQueueOrder(const QVector<int>& order)
{
    const int size = static_cast<int>(m_queue.size());
    if (order.size() != size)
        return;
    QVector<int> newIndexOf(size);
    QStringList queue;
    queue.reserve(size);
    for (int n = 0; n < size; ++n) {
        newIndexOf[order[n]] = n;
        queue << m_queue[order[n]];
    }
    if (queue == m_queue)
        return;
    m_queue = queue;
    if (m_currentIndex >= 0 && m_currentIndex < size)
        m_currentIndex = newIndexOf[m_currentIndex];
    for (int& i : m_order) // the shuffle order follows the entries
        if (i >= 0 && i < size)
            i = newIndexOf[i];
    emit queueChanged();
    emit trackChanged();
    scheduleSave();
}

void DspController::sortQueue(const QString& field, bool descending)
{
    const int size = static_cast<int>(m_queue.size());
    if (size < 2)
        return;
    // The same order as the track lists (TrackSort): the field, then its
    // tie-breakers, blanks last, paths folder by folder.
    const QStringList keys = TrackSort::keysFor(field);
    const LibraryModel* library = LibraryModel::instance();
    QVector<QVariantList> values(size);
    for (int i = 0; i < size; ++i) {
        const QVariantMap t = trackInfo(m_queue[i]);
        for (const QString& k : keys) {
            if (k == u"path")
                values[i] << QVariant(m_queue[i]);
            else if (k == u"fileName")
                values[i] << QVariant(m_queue[i].section(u'/', -1));
            else if (k == u"favorite") // kept by the library, not in the track's tags
                values[i] << QVariant(library && library->isFavorite(m_queue[i]) ? 1 : 0);
            else
                values[i] << t.value(k);
        }
    }
    const QCollator collator = TrackSort::collator();
    QVector<int> order(size);
    std::iota(order.begin(), order.end(), 0);
    std::stable_sort(order.begin(), order.end(), [&](int a, int b) {
        for (int k = 0; k < keys.size(); ++k) {
            const bool down = k == 0 && descending;
            if (const int c = TrackSort::compareKey(collator, keys[k], values[a][k], values[b][k], down); c != 0)
                return down ? c > 0 : c < 0;
        }
        return false;
    });
    applyQueueOrder(order);
}

void DspController::reverseQueue()
{
    QVector<int> order(m_queue.size());
    std::iota(order.rbegin(), order.rend(), 0);
    applyQueueOrder(order);
}

void DspController::setResumeSession(bool on)
{
    if (on == m_resume)
        return;
    m_resume = on;
    scheduleSave();
    emit preferencesChanged();
}

int DspController::bufferMs() const
{
    return static_cast<int>(m_engine.outputConfig().bufferMs);
}

void DspController::setBufferMs(int ms)
{
    ms = std::clamp(ms, 60, 2000);
    if (ms == bufferMs())
        return;
    auto cfg = m_engine.outputConfig();
    cfg.bufferMs = static_cast<uint32_t>(ms);
    m_engine.setOutputConfig(cfg);
    scheduleSave();
    emit preferencesChanged();
    reopenCurrent();
}

int DspController::resyncDelayMs() const
{
    return static_cast<int>(m_engine.outputConfig().resyncDelayMs);
}

// Applies from the next rate change: nothing to reopen.
void DspController::setResyncDelayMs(int ms)
{
    ms = std::clamp(ms, 0, 3000);
    if (ms == resyncDelayMs())
        return;
    auto cfg = m_engine.outputConfig();
    cfg.resyncDelayMs = static_cast<uint32_t>(ms);
    m_engine.setOutputConfig(cfg);
    scheduleSave();
    emit preferencesChanged();
}

int DspController::fixedRate() const
{
    return static_cast<int>(m_engine.outputConfig().fixedRate);
}

void DspController::setFixedRate(int hz)
{
    hz = std::clamp(hz, 0, 768000);
    if (hz == fixedRate())
        return;
    auto cfg = m_engine.outputConfig();
    cfg.fixedRate = static_cast<uint32_t>(hz);
    m_engine.setOutputConfig(cfg);
    scheduleSave();
    emit preferencesChanged();
    reopenCurrent();
}

void DspController::setRateMode(int fixedHz, int upsampling)
{
    fixedHz = std::clamp(fixedHz, 0, 768000);
    upsampling = std::clamp(upsampling, 0, 3);
    if (fixedHz == fixedRate() && upsampling == m_upsampling)
        return;
    m_upsampling = upsampling;
    auto cfg = m_engine.outputConfig();
    cfg.fixedRate = static_cast<uint32_t>(fixedHz);
    cfg.upsampling = static_cast<sonkore::DSPManager::Upsampling>(upsampling);
    m_engine.setOutputConfig(cfg);
    scheduleSave();
    emit resamplingChanged();
    emit preferencesChanged();
    reopenCurrent();
}

void DspController::playIndex(int index)
{
    if (index < 0 || index >= m_queue.size())
        return;
    openIndex(index, true);
}

void DspController::clearQueue()
{
    m_engine.unload();
    m_reservation.release();
    m_queue.clear();
    m_order.clear();
    m_currentIndex = -1;
    m_currentPath.clear();
    m_currentTrack.clear();
    m_position = 0.0;
    emit queueChanged();
    emit trackChanged();
    emit stateChanged();
    emit positionChanged();
    emit outputStatusChanged();
    scheduleSave();
}

bool DspController::load(const QUrl& url)
{
    const QString path = url.isLocalFile() ? url.toLocalFile() : url.toString();
    playPaths({path}, 0);
    return loaded();
}

bool DspController::openIndex(int index, bool autoplay, double startAt)
{
    if (index < 0 || index >= m_queue.size())
        return false;
    m_currentIndex = index;
    const QString path = m_queue[index];
    m_currentPath = path;
    m_currentTrack = trackInfo(path);

    std::string error;
    bool justTaken = false;
    if (!reserveOutput(&justTaken)) {
        m_engine.unload();
        m_resumePosition = startAt;
        emit trackChanged();
        emit stateChanged();
        emit outputStatusChanged();
        return false;
    }
    const double gain = gainFor(path, m_currentTrack);
    bool ok = m_engine.load(toFsPath(path), &error, gain);
    // The sound server closes the card a moment after giving it up.
    for (int i = 0; justTaken && !ok && i < 40; ++i) {
        QThread::msleep(50);
        ok = m_engine.load(toFsPath(path), &error, gain);
    }
    if (ok) {
        if (startAt > 0.0)
            m_engine.seek(startAt);
        if (autoplay)
            m_engine.play();
        m_lastError.clear();
        emit errorOccurred();
    } else {
        reportError(QString::fromStdString(error));
    }
    m_resumePosition = 0.0;
    m_armedPath.clear(); // a fresh load forgets the queued track
    armGapless();
    applyGains();
    emit trackChanged();
    emit stateChanged();
    emit outputStatusChanged();
    poll();
    scheduleSave();
    return ok;
}

void DspController::rebuildShuffleOrder()
{
    m_order.resize(m_queue.size());
    for (int i = 0; i < m_order.size(); ++i)
        m_order[i] = i;
    if (!m_shuffle || m_order.isEmpty())
        return;
    std::shuffle(m_order.begin(), m_order.end(), *QRandomGenerator::global());
    // The current track becomes the first entry of the shuffled order.
    const auto it = std::find(m_order.begin(), m_order.end(), std::max(0, m_currentIndex));
    if (it != m_order.end())
        std::iter_swap(m_order.begin(), it);
}

// The track that follows the current one when it ends on its own.
int DspController::upcomingIndex() const
{
    if (m_queue.isEmpty() || m_currentIndex < 0)
        return -1;
    if (m_repeat == RepeatOne)
        return m_currentIndex;
    int pos = orderPosition() + 1;
    if (pos >= m_order.size()) {
        if (m_repeat == RepeatOff)
            return -1;
        pos = 0;
    }
    return pos < m_order.size() ? m_order[pos] : -1;
}

void DspController::armGapless()
{
    const int next = loaded() ? upcomingIndex() : -1;
    const QString path = next >= 0 ? m_queue[next] : QString();
    if (path == m_armedPath)
        return;
    m_armedPath = path;
    m_engine.setNext(path.isEmpty() ? std::filesystem::path() : toFsPath(path),
                     gainFor(path, path.isEmpty() ? QVariantMap{} : trackInfo(path)));
}

int DspController::orderPosition() const
{
    const auto it = std::find(m_order.begin(), m_order.end(), m_currentIndex);
    return it == m_order.end() ? -1 : static_cast<int>(it - m_order.begin());
}

void DspController::advance(bool userInitiated)
{
    if (m_queue.isEmpty())
        return;
    if (!userInitiated && m_repeat == RepeatOne) {
        m_engine.seek(0.0);
        m_engine.play();
        emit stateChanged();
        return;
    }
    // Skip over unreadable files, but never loop forever.
    int pos = orderPosition();
    for (int attempt = 0; attempt < m_order.size(); ++attempt) {
        ++pos;
        if (pos >= m_order.size()) {
            if (m_repeat == RepeatOff && !userInitiated) {
                m_engine.stop();
                emit stateChanged();
                emit trackFinished();
                return;
            }
            pos = 0;
        }
        if (openIndex(m_order[pos], true))
            return;
    }
}

void DspController::next()
{
    advance(true);
}

void DspController::previous()
{
    if (m_queue.isEmpty())
        return;
    if (m_engine.positionSeconds() > 3.0) {
        m_engine.seek(0.0);
        poll();
        emit seeked(0.0);
        return;
    }
    int pos = orderPosition() - 1;
    if (pos < 0)
        pos = m_repeat == RepeatAll ? static_cast<int>(m_order.size()) - 1 : 0;
    openIndex(m_order[pos], playing() || !loaded());
}

void DspController::setShuffle(bool on)
{
    if (on == m_shuffle)
        return;
    m_shuffle = on;
    rebuildShuffleOrder();
    scheduleSave();
    emit shuffleChanged();
}

void DspController::setRepeatMode(int mode)
{
    mode = std::clamp(mode, 0, 2);
    if (mode == m_repeat)
        return;
    m_repeat = mode;
    scheduleSave();
    emit repeatModeChanged();
}

// --- Transport ----------------------------------------------------------------

void DspController::play()
{
    if (!loaded()) {
        // Restored queue from the last session: open lazily on first play.
        if (m_currentIndex >= 0)
            openIndex(m_currentIndex, true, m_resumePosition);
        return;
    }
    m_engine.play();
    emit stateChanged();
}

void DspController::pause()
{
    m_engine.pause();
    emit stateChanged();
    scheduleSave(); // remember where we paused
}

void DspController::togglePlayPause()
{
    playing() ? pause() : play();
}

void DspController::stop()
{
    m_engine.stop();
    emit stateChanged();
    poll();
}

void DspController::seek(double seconds)
{
    if (!loaded()) {
        m_resumePosition = seconds;
        m_position = seconds;
        emit positionChanged();
        return;
    }
    m_engine.seek(seconds);
    poll();
    emit seeked(m_position);
}

// --- Status -----------------------------------------------------------------

bool DspController::playing() const { return m_engine.state() == sonkore::DSPManager::State::Playing; }
bool DspController::loaded() const { return m_engine.state() != sonkore::DSPManager::State::Empty; }

double DspController::duration() const
{
    return loaded() ? m_engine.durationSeconds() : m_currentTrack.value("duration").toDouble();
}

QString DspController::currentPath() const
{
    return m_currentPath;
}

QString DspController::trackDescription() const
{
    const auto t = m_engine.trackInfo();
    if (t.sampleRate == 0)
        return m_currentTrack.value("format").toString();
    QString s = QString::fromStdString(t.codec);
    if (t.bitsPerSample > 0)
        s += QStringLiteral(" %1-bit").arg(t.bitsPerSample);
    s += QStringLiteral(" · %1 kHz").arg(t.sampleRate / 1000.0, 0, 'g', 6);
    if (t.channels != 2)
        s += QStringLiteral(" · %1 ch").arg(t.channels);
    return s;
}

void DspController::poll()
{
    if (m_headroomAuto)
        applyHeadroom(); // follows EQ edits
    const double pos = loaded() ? m_engine.positionSeconds() : m_resumePosition;
    if (pos != m_position) {
        m_position = pos;
        emit positionChanged();
    }
    const bool bp = bitPerfect();
    if (bp != m_lastBitPerfect) {
        m_lastBitPerfect = bp;
        emit outputStatusChanged();
    }
    std::filesystem::path spliced;
    if (m_engine.takeTrackChange(&spliced)) {
        // The next track joined without a gap and is now being heard.
        const QString path = fromFsPath(spliced);
        const int upcoming = upcomingIndex();
        const int index = upcoming >= 0 && m_queue[upcoming] == path ? upcoming : static_cast<int>(m_queue.indexOf(path));
        if (index >= 0)
            m_currentIndex = index;
        m_currentPath = path;
        m_currentTrack = trackInfo(path);
        m_armedPath.clear();
        armGapless();
        applyGains(); // the engine already plays it at its gain; this refreshes the display
        emit trackChanged();
        scheduleSave();
    }
#if defined(SONKORE_HAVE_DBUS)
    // A new output means a new device thread: real-time scheduling for it,
    // through rtkit (SCHED_FIFO is not allowed to ordinary users directly).
    if (const qint64 tid = m_engine.audioThreadId(); tid != 0 && tid != m_rtThread) {
        m_rtThread = tid;
        m_rtPriority = RealtimeKit::currentPriority(tid);
        if (m_rtPriority == 0)
            m_rtPriority = RealtimeKit::makeRealtime(tid);
    }
#endif
    // Rebuilt every tick but only announced when a stage changed (EQ, pitch,
    // volume, ReplayGain, output or track).
    QVariantList path = buildSignalPath();
    if (path != m_signalPath) {
        m_signalPath = std::move(path);
        emit signalPathChanged();
    }
    if (playing() && m_engine.reachedEnd())
        advance(false);
}

QVariantList DspController::buildSignalPath() const
{
    QVariantList out;
    if (!loaded())
        return out;
    const auto add = [&out](const QString& stage, const QString& detail, const char* quality) {
        out << QVariantMap{{"stage", stage}, {"detail", detail}, {"quality", QString::fromLatin1(quality)}};
    };
    const auto signedDb = [](double db) { return (db >= 0 ? QStringLiteral("+") : QString()) + QString::number(db, 'f', 1); };
    const sonkore::StreamInfo t = m_engine.trackInfo();
    const auto s = m_engine.outputStatus();
    const bool dsd = t.dsd != sonkore::StreamInfo::Dsd::None;

    if (dsd)
        add(tr("Source"), tr("%1 · %2 MHz").arg(QString::fromStdString(sonkore::DsdDecoder::rateLabel(t.dsdRate))).arg(t.dsdRate / 1e6, 0, 'g', 6), "lossless");
    else
        add(tr("Source"), trackDescription(), t.lossless ? "lossless" : "low");
    add(tr("Decoder"), QString::fromStdString(t.decoder), t.lossless ? "lossless" : "high");
    switch (m_engine.memoryState()) {
    case 2: add(tr("Memory"), tr("Played from RAM (locked): no disk access"), "lossless"); break;
    case 1: add(tr("Memory"), tr("Played from RAM: no disk access"), "lossless"); break;
    default: break;
    }

    if (s.hdcd)
        add(tr("HDCD"), tr("Decoded to 20 bits (peak extend and gain control), at -6 dB to leave room for the extended peaks"), "enhanced");
    if (dsd) {
        add(tr("DSD"), t.dsd == sonkore::StreamInfo::Dsd::Native ? tr("Sent as native DSD") : tr("Sent as DoP (DSD over PCM)"), "lossless");
    } else {
        const sonkore::TimeStretch& st = m_engine.timeStretch();
        const bool crossfeedOn = m_crossfeed != 0 && t.channels == 2;
        const auto conv = m_engine.convolutionStatus();
        if (conv.active) {
            const QString layout = conv.irChannels == 4 ? tr("true stereo") : conv.irChannels == 2 ? tr("stereo") : tr("mono");
            QString detail = tr("%1 · %2 taps · %3").arg(QString::fromStdString(conv.file.filename().string())).arg(conv.taps).arg(layout);
            if (conv.resampledFrom)
                detail += tr(" · resampled from %1 kHz").arg(conv.resampledFrom / 1000.0, 0, 'g', 6);
            add(tr("Convolution"), detail, "enhanced");
        } else if (!conv.file.empty()) {
            add(tr("Convolution"), tr("Not applied to this track: %1").arg(QString::fromStdString(conv.problem)), "high");
        }
        const bool dspActive = !st.isNeutral() || !m_engine.equalizer().isNeutral() || s.resampled || crossfeedOn || conv.active
                            || (!m_hwVolumeActive && m_engine.volume() != 1.0)
                            || (m_rgMode != ReplayGainOff && !std::isnan(m_currentGainDb) && m_currentGainDb != 0.0);
        if (dspActive && m_effectiveHeadroomDb < 0.0)
            add(tr("Headroom"), (m_headroomAuto ? tr("%1 dB ahead of the DSP (automatic)") : tr("%1 dB ahead of the DSP"))
                                    .arg(signedDb(m_effectiveHeadroomDb)), "high");
        if (!st.isNeutral())
            add(tr("Pitch / tempo"), tr("%1 · pitch %2 st · tempo %3%").arg(pitchEngineName()).arg(signedDb(st.pitchSemiTones()))
                                         .arg(st.tempo() * st.rate() * 100.0, 0, 'f', 1), "enhanced");
        sonkore::SuperEQ& eq = m_engine.equalizer();
        if (!eq.isNeutral())
            add(tr("Equalizer"), tr("%1 bands · preamp %2 dB").arg(eq.bands().size()).arg(signedDb(eq.preampDb())), "enhanced");
        if (crossfeedOn) {
            double cutoff = 0.0, feed = 0.0;
            sonkore::Crossfeed::parameters(static_cast<sonkore::Crossfeed::Preset>(m_crossfeed), cutoff, feed);
            const QString names[] = {QString(), tr("Default"), QStringLiteral("Chu Moy"), QStringLiteral("Jan Meier")};
            add(tr("Crossfeed"), tr("%1 · %2 Hz · %3 dB").arg(names[m_crossfeed]).arg(cutoff).arg(feed, 0, 'f', 1), "enhanced");
        }
        if (m_rgMode != ReplayGainOff && !std::isnan(m_currentGainDb) && m_currentGainDb != 0.0)
            add(tr("ReplayGain"), tr("%1 · %2 dB").arg(m_rgMode == ReplayGainAlbum ? tr("Album") : tr("Track"), signedDb(m_currentGainDb)), "enhanced");
        if (!m_hwVolumeActive && m_engine.volume() != 1.0)
            add(tr("Volume"), m_engine.volume() > 0.0 ? tr("Digital · %1 dB").arg(signedDb(20.0 * std::log10(m_engine.volume()))) : tr("Digital · muted"), "high");
        if (s.pcmToDsdRate) {
            const QString filters[] = {tr("linear phase"), tr("intermediate phase"), tr("minimum phase"), tr("linear phase, steep")};
            add(tr("PCM → DSD"), tr("SoX VHQ, %1 · %2 kHz → %3 MHz · 7th-order sigma-delta modulator")
                                     .arg(filters[m_resampleFilter]).arg(s.sourceSampleRate / 1000.0, 0, 'g', 6).arg(s.pcmToDsdRate / 1e6, 0, 'g', 6), "enhanced");
            if (s.modulatorResets)
                add(tr("Modulator"), tr("%1 overloads: lower the headroom").arg(s.modulatorResets), "low");
        } else if (s.resampled) {
            const QString filters[] = {tr("linear phase"), tr("intermediate phase"), tr("minimum phase"), tr("linear phase, steep")};
            add(s.upsampled ? tr("Upsampling") : tr("Sample rate conversion"),
                tr("SoX VHQ, %1 · %2 → %3 kHz").arg(filters[m_resampleFilter]).arg(s.sourceSampleRate / 1000.0, 0, 'g', 6).arg(s.deviceSampleRate / 1000.0, 0, 'g', 6)
                    + (fixedRate() != 0 && !s.upsampled ? tr(" · fixed output rate") : QString()),
                s.upsampled ? "enhanced" : "high");
        }
        if (!s.bitPerfectNow && !s.hdcd && !s.pcmToDsdRate && (s.resampled || crossfeedOn || conv.active || !m_engine.timeStretch().isNeutral() || !eq.isNeutral() || m_engine.volume() != 1.0
                                 || (m_rgMode != ReplayGainOff && !std::isnan(m_currentGainDb) && m_currentGainDb != 0.0))) {
            const int bits = s.deviceFloat ? 24 : std::min(s.deviceBits, 24);
            add(tr("Requantisation"), m_ditherMode == DitherShaped ? tr("%1-bit with noise-shaped dither").arg(bits)
                                      : m_ditherMode == DitherTpdf ? tr("%1-bit with TPDF dither").arg(bits) : tr("%1-bit").arg(bits), "high");
        }
    }
    if (m_volumeFixed)
        add(tr("Volume"), tr("Fixed at 100 %: set on the DAC or the amplifier"), "lossless");
#if defined(SONKORE_HAVE_HW_VOLUME)
    if (m_hwVolumeActive)
        add(tr("Volume"), m_mixer->volume() > 0.0
                ? tr("DAC control \"%1\" · %2 dB").arg(m_mixer->controlName(), signedDb(20.0 * std::log10(m_mixer->volume())))
                : tr("DAC control \"%1\" · muted").arg(m_mixer->controlName()), "lossless");
#endif
    if (!dsd && s.deviceChannels != 0 && s.deviceChannels != t.channels) {
        if (t.channels == 1 && s.deviceChannels == 2)
            add(tr("Channels"), tr("Mono copied to both channels, sample for sample"), "lossless");
        else if (s.deviceChannels < t.channels)
            add(tr("Channels"), tr("%1 channels mixed down to %2: this output takes no more").arg(t.channels).arg(s.deviceChannels), "high");
    }
    // Only whether it clips: a running count would climb for the whole track.
    // A warning, not "low quality": one setting fixes it, and the row offers
    // it (automatic headroom) while the headroom is manual.
    if (m_engine.clippedSamples() > 0) {
        out << QVariantMap{{"stage", tr("Clipping")},
                           {"detail", m_headroomAuto ? tr("This track is clipping even with the automatic headroom: set a lower one by hand.")
                                                     : tr("This track is clipping: lower the headroom.")},
                           {"quality", QStringLiteral("warning")},
                           {"action", m_headroomAuto ? QString() : QStringLiteral("headroomAuto")}};
    }
    const bool mixer = !s.exclusive && !s.pathCanBeBitPerfect;
    QString output = outputDescription() + (mixer ? tr(" · through the system mixer") : QString());
    if (m_rtPriority > 0 && m_engine.audioThreadId() == m_rtThread)
        output += tr(" · real-time audio thread (priority %1)").arg(m_rtPriority);
    add(tr("Output"), output, s.pathCanBeBitPerfect ? "lossless" : "high");
    return out;
}

void DspController::reportError(const QString& message)
{
    m_lastError = message;
    emit errorOccurred();
}

// --- Persistence ----------------------------------------------------------------

void DspController::loadSettings()
{
    QSettings s;
    s.beginGroup("playback");
    m_deviceKey = s.value("device").toString();
    m_exclusive = s.value("exclusive", true).toBool();
    // Older settings stored a plain on/off "dither".
    m_ditherMode = std::clamp(s.value("ditherMode", s.value("dither", true).toBool() ? DitherTpdf : DitherOff).toInt(), 0, 2);
    m_headroomDb = std::clamp(s.value("headroomDb", 0.0).toDouble(), -24.0, 0.0);
    m_headroomAuto = s.value("headroomAuto", false).toBool(); // default: manual, 0 dB
    m_crossfeed = std::clamp(s.value("crossfeed", 0).toInt(), 0, 3);
    m_upsampling = std::clamp(s.value("upsampling", 0).toInt(), 0, 3);
    m_convolutionFile = s.value("convolutionFile").toString();
    m_convolutionEnabled = s.value("convolution", false).toBool();
    m_resampleFilter = std::clamp(s.value("resampleFilter", 0).toInt(), 0, 3);
    m_volumeMode = std::clamp(s.value("volumeMode", VolumeAuto).toInt(), 0, 2);
    m_dsdMode = std::clamp(s.value("dsdOutput", DsdNative).toInt(), 0, 2);
    m_pcmToDsd = std::clamp(s.value("pcmToDsd", 0).toInt(), 0, 4);
    m_playFromMemory = s.value("playFromMemory", true).toBool();
    m_hdcd = s.value("hdcd", false).toBool(); // decoding changes the samples: only on request
    m_rgMode = std::clamp(s.value("replayGain", ReplayGainOff).toInt(), 0, 2);
    m_rgPreamp = std::clamp(s.value("replayGainPreamp", 0.0).toDouble(), -15.0, 15.0);
    m_rgPreventClipping = s.value("replayGainPreventClipping", true).toBool();
    m_shuffle = s.value("shuffle", false).toBool();
    m_repeat = s.value("repeat", RepeatOff).toInt();
    m_resume = s.value("resume", true).toBool();
    m_releaseWhenIdle = s.value("releaseDacWhenIdle", true).toBool();
    const int buffer = s.value("bufferMs", 150).toInt();
    const int resync = std::clamp(s.value("resyncDelayMs", 250).toInt(), 0, 3000);
    const int fixedRate = std::clamp(s.value("fixedRate", 0).toInt(), 0, 768000);
    m_engine.setVolume(s.value("volume", 1.0).toDouble());
    loadSession(s);
    s.endGroup();

    auto cfg = m_engine.outputConfig();
    cfg.deviceKey = m_deviceKey.toStdString();
    cfg.exclusiveMode = m_exclusive;
    cfg.dsd = toEngineDsd(m_dsdMode);
    cfg.pcmToDsd = static_cast<sonkore::DSPManager::PcmToDsd>(m_pcmToDsd);
    cfg.playFromMemory = m_playFromMemory;
    cfg.hdcd = m_hdcd;
    cfg.upsampling = static_cast<sonkore::DSPManager::Upsampling>(m_upsampling);
    cfg.filter = static_cast<sonkore::Resampler::Filter>(m_resampleFilter);
    cfg.bufferMs = static_cast<uint32_t>(std::clamp(buffer, 60, 2000));
    cfg.resyncDelayMs = static_cast<uint32_t>(resync);
    cfg.fixedRate = static_cast<uint32_t>(fixedRate);
    m_engine.setOutputConfig(cfg);
    m_engine.setDither(static_cast<sonkore::DitherMode>(m_ditherMode));
    m_effectiveHeadroomDb = 1.0; // forces the first apply
    applyHeadroom();
    m_engine.setCrossfeed(static_cast<sonkore::Crossfeed::Preset>(m_crossfeed));
    if (!applyConvolution())
        m_convolutionEnabled = false; // the file is gone or unreadable

    // Pitch / tempo: engine settings first, then the ratios.
    sonkore::TimeStretch& st = m_engine.timeStretch();
    s.beginGroup("soundtouch");
    const sonkore::TimeStretch::SoundTouchSettings defaults;
    st.setUseAAFilter(s.value("aaFilter", defaults.useAAFilter).toBool());
    st.setAAFilterLength(s.value("aaFilterLength", defaults.aaFilterLength).toInt());
    st.setUseQuickSeek(s.value("quickSeek", defaults.useQuickSeek).toBool());
    st.setSequenceMs(s.value("sequenceMs", defaults.sequenceMs).toInt());
    st.setSeekWindowMs(s.value("seekWindowMs", defaults.seekWindowMs).toInt());
    st.setOverlapMs(s.value("overlapMs", defaults.overlapMs).toInt());
    // Older versions kept the ratios here, when SoundTouch was the only engine.
    const QVariant legacyPitch = s.value("pitch"), legacyTempo = s.value("tempo"), legacyRate = s.value("rate");
    const QVariant legacyEnabled = s.value("enabled", false); // a fresh install starts bit-perfect
    s.remove("pitch");
    s.remove("tempo");
    s.remove("rate");
    s.remove("enabled");
    s.endGroup();

    s.beginGroup("rubberband");
    const sonkore::TimeStretch::RubberBandSettings rb;
    st.setRubberBandFiner(s.value("finer", rb.finer).toBool());
    st.setPreserveFormants(s.value("preserveFormants", rb.preserveFormants).toBool());
    st.setChannelsTogether(s.value("channelsTogether", rb.channelsTogether).toBool());
    // Before the pitch method there was only "smooth pitch changes" (consistency).
    const int legacyMethod = s.value("smoothPitchChanges", false).toBool() ? sonkore::TimeStretch::PitchConsistency
                                                                           : rb.pitchMethod;
    s.remove("smoothPitchChanges");
    st.setPitchMethod(s.value("pitchMethod", legacyMethod).toInt());
    st.setRubberBandWindow(s.value("window", rb.window).toInt());
    // "transients" held the library's default (crisp) before smooth became
    // Sonkore's: read under a new name so that default reaches everyone.
    s.remove("transients");
    st.setTransients(s.value("r2Transients", rb.transients).toInt());
    st.setDetector(s.value("detector", rb.detector).toInt());
    st.setPhaseIndependent(s.value("phaseIndependent", rb.phaseIndependent).toBool());
    st.setRubberBandSmoothing(s.value("smoothing", rb.smoothing).toBool());
    s.endGroup();

    s.beginGroup("pitch");
    const int engine = s.value("engine", static_cast<int>(RubberBand)).toInt();
    st.setEngine(engine == SoundTouch ? sonkore::TimeStretch::Engine::SoundTouch : sonkore::TimeStretch::Engine::RubberBand);
    st.setPitchSemiTones(exactValue(s.value("pitch", legacyPitch), 0.0));
    st.setTempo(exactValue(s.value("tempo", legacyTempo), 1.0));
    st.setRate(exactValue(s.value("rate", legacyRate), 1.0));
    st.setEnabled(s.value("enabled", legacyEnabled).toBool());
    s.endGroup();

    rebuildShuffleOrder();
    if (m_currentIndex >= 0) {
        m_currentPath = m_queue[m_currentIndex];
        m_currentTrack = trackInfo(m_currentPath);
        m_position = m_resumePosition;
    }
}

// The play queue lives in its own file: thousands of paths in the INI would
// be rewritten on every preference change.
void DspController::loadSession(QSettings& legacy)
{
    QStringList queue;
    int index = 0;
    double position = 0.0;
    QFile file(sessionPath());
    if (file.open(QIODevice::ReadOnly)) {
        const QJsonObject o = QJsonDocument::fromJson(file.readAll()).object();
        for (const QJsonValue& v : o.value("queue").toArray())
            queue << v.toString();
        index = o.value("index").toInt();
        position = o.value("position").toDouble();
    } else if (legacy.contains("queue")) { // older versions kept the queue in the settings
        queue = legacy.value("queue").toStringList();
        index = legacy.value("index", 0).toInt();
        position = legacy.value("position", 0.0).toDouble();
    }
    legacy.remove("queue");
    legacy.remove("index");
    legacy.remove("position");

    if (!m_resume)
        return;
    m_queue = queue;
    m_currentIndex = m_queue.isEmpty() ? -1 : std::clamp(index, 0, static_cast<int>(m_queue.size()) - 1);
    m_resumePosition = std::max(0.0, position);
}

void DspController::scheduleSave()
{
    m_saveTimer.start();
}

void DspController::saveSettings() const
{
    if (!sonkore::g_settingsFrozen.load())
        savePreferences();
    saveSession();
}

void DspController::savePreferences() const
{
    QSettings s;
    s.beginGroup("playback");
    s.setValue("device", m_deviceKey);
    s.setValue("exclusive", m_exclusive);
    s.remove("dither");
    s.setValue("ditherMode", m_ditherMode);
    s.setValue("headroomDb", m_headroomDb);
    s.setValue("headroomAuto", m_headroomAuto);
    s.setValue("crossfeed", m_crossfeed);
    s.setValue("upsampling", m_upsampling);
    s.setValue("convolutionFile", m_convolutionFile);
    s.setValue("convolution", m_convolutionEnabled);
    s.setValue("resampleFilter", m_resampleFilter);
    s.setValue("replayGain", m_rgMode);
    s.setValue("replayGainPreamp", m_rgPreamp);
    s.setValue("replayGainPreventClipping", m_rgPreventClipping);
    s.setValue("shuffle", m_shuffle);
    s.setValue("repeat", m_repeat);
    s.setValue("volume", m_engineAtUnity ? m_digitalVolume : m_engine.volume());
    s.setValue("volumeMode", m_volumeMode);
    s.setValue("dsdOutput", m_dsdMode);
    s.setValue("pcmToDsd", m_pcmToDsd);
    s.setValue("playFromMemory", m_playFromMemory);
    s.setValue("hdcd", m_hdcd);
    s.setValue("resume", m_resume);
    s.setValue("releaseDacWhenIdle", m_releaseWhenIdle);
    s.setValue("bufferMs", static_cast<int>(m_engine.outputConfig().bufferMs));
    s.setValue("resyncDelayMs", static_cast<int>(m_engine.outputConfig().resyncDelayMs));
    s.setValue("fixedRate", static_cast<int>(m_engine.outputConfig().fixedRate));
    s.endGroup();

    const sonkore::TimeStretch& st = m_engine.timeStretch();
    s.beginGroup("pitch");
    s.setValue("engine", static_cast<int>(st.engine()));
    s.setValue("pitch", exactText(st.pitchSemiTones()));
    s.setValue("tempo", exactText(st.tempo()));
    s.setValue("rate", exactText(st.rate()));
    s.setValue("enabled", st.enabled());
    s.endGroup();

    const sonkore::TimeStretch::RubberBandSettings rb = st.rubberBandSettings();
    s.beginGroup("rubberband");
    s.setValue("finer", rb.finer);
    s.setValue("preserveFormants", rb.preserveFormants);
    s.setValue("channelsTogether", rb.channelsTogether);
    s.setValue("pitchMethod", rb.pitchMethod);
    s.setValue("window", rb.window);
    s.setValue("r2Transients", rb.transients);
    s.setValue("detector", rb.detector);
    s.setValue("phaseIndependent", rb.phaseIndependent);
    s.setValue("smoothing", rb.smoothing);
    s.endGroup();

    s.beginGroup("soundtouch");
    const sonkore::TimeStretch::SoundTouchSettings advanced = st.soundTouchSettings();
    s.setValue("aaFilter", advanced.useAAFilter);
    s.setValue("aaFilterLength", advanced.aaFilterLength);
    s.setValue("quickSeek", advanced.useQuickSeek);
    s.setValue("sequenceMs", advanced.sequenceMs);
    s.setValue("seekWindowMs", advanced.seekWindowMs);
    s.setValue("overlapMs", advanced.overlapMs);
    s.endGroup();
}

void DspController::saveSession() const
{
    QJsonObject session;
    session.insert("queue", QJsonArray::fromStringList(m_queue));
    session.insert("index", m_currentIndex);
    session.insert("position", loaded() ? m_engine.positionSeconds() : m_resumePosition);
    const QByteArray json = QJsonDocument(session).toJson(QJsonDocument::Compact);
    if (json == m_savedSession)
        return;
    QDir().mkpath(QFileInfo(sessionPath()).path());
    QSaveFile file(sessionPath());
    if (file.open(QIODevice::WriteOnly) && file.write(json) == json.size() && file.commit())
        m_savedSession = json;
}
