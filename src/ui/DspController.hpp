#pragma once

#include "DeviceReservation.hpp"
#include "QmlSingleton.hpp"

#include <QFileSystemWatcher>
#include <QHash>
#include <QObject>
#include <QSettings>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QUrl>
#include <QVariant>
#include <QVariantList>
#include <QVariantMap>
#include <QVector>
#include <QtQml/qqmlregistration.h>

namespace sonkore {
class DSPManager;
}

// The DAC's own volume control on this platform (see updateMixer()).
#if defined(_WIN32)
class WinEndpointVolume;
using HardwareMixer = WinEndpointVolume;
#else
class AlsaMixer;
using HardwareMixer = AlsaMixer;
#endif
class PipeWireVolume;

// QML-facing bridge to the audio engine: transport, play queue, output
// devices and the pitch / tempo processor (SoundTouch).
//
// Precision contract: every continuous parameter is a C++ `double` and QML's
// `real`/`number` is an IEEE-754 binary64 too, so values cross the boundary
// bit-for-bit. Text from the UI is parsed with parseDecimal() (QLocale::C,
// correctly rounded) and setters compare with `==`, never qFuzzyCompare, so a
// 1e-7 semitone edit is never swallowed.
class DspController : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(DspController)

    // --- Pitch / tempo / rate ------------------------------------------------
    Q_PROPERTY(double pitchSemitones READ pitchSemitones WRITE setPitchSemitones NOTIFY pitchSemitonesChanged)
    Q_PROPERTY(double tempo READ tempo WRITE setTempo NOTIFY tempoChanged)
    Q_PROPERTY(double rate READ rate WRITE setRate NOTIFY rateChanged)


    // --- SoundTouch: advanced ---------------------------------------------
    Q_PROPERTY(bool useAAFilter READ useAAFilter WRITE setUseAAFilter NOTIFY soundTouchSettingsChanged)
    Q_PROPERTY(int aaFilterLength READ aaFilterLength WRITE setAAFilterLength NOTIFY soundTouchSettingsChanged)
    Q_PROPERTY(bool useQuickSeek READ useQuickSeek WRITE setUseQuickSeek NOTIFY soundTouchSettingsChanged)
    Q_PROPERTY(int sequenceMs READ sequenceMs WRITE setSequenceMs NOTIFY soundTouchSettingsChanged)
    Q_PROPERTY(int seekWindowMs READ seekWindowMs WRITE setSeekWindowMs NOTIFY soundTouchSettingsChanged)
    Q_PROPERTY(int overlapMs READ overlapMs WRITE setOverlapMs NOTIFY soundTouchSettingsChanged)
    // The pitch / tempo processor is processing audio.
    Q_PROPERTY(bool soundTouchActive READ soundTouchActive NOTIFY soundTouchActiveChanged)
    // On / off switch: off keeps the values but bypasses the processor.
    Q_PROPERTY(bool soundTouchEnabled READ soundTouchEnabled WRITE setSoundTouchEnabled NOTIFY soundTouchActiveChanged)
    // Source seconds per wall-clock second (tempo × rate), for smooth progress.
    Q_PROPERTY(double speed READ speed NOTIFY soundTouchActiveChanged)

    // --- Output -----------------------------------------------------------
    Q_PROPERTY(double volume READ volume WRITE setVolume NOTIFY volumeChanged)
    // Where the volume acts (VolumeMode): Sonkore's digital gain, or the DAC's
    // own control (ALSA mixer of a direct hw: output), which keeps bit-perfect.
    Q_PROPERTY(int volumeMode READ volumeMode WRITE setVolumeMode NOTIFY volumeModeChanged)
    Q_PROPERTY(bool hardwareVolume READ hardwareVolume NOTIFY volumeModeChanged) // the DAC's control is in use now
    // The volume is the PipeWire output's (the desktop's volume for it), the
    // DAC's own control where the device has one: systemVolumeInHardware.
    Q_PROPERTY(bool systemVolume READ systemVolume NOTIFY volumeModeChanged)
    Q_PROPERTY(bool systemVolumeInHardware READ systemVolumeInHardware NOTIFY volumeChanged)
    Q_PROPERTY(bool volumeFixed READ volumeFixed NOTIFY volumeModeChanged) // at 100 %, set on the DAC / amplifier
    Q_PROPERTY(bool hardwareVolumeSupported READ hardwareVolumeSupported CONSTANT)
    Q_PROPERTY(QString hardwareVolumeInfo READ hardwareVolumeInfo NOTIFY volumeModeChanged)
    // Requantisation dither when DSP is active (DitherMode: off, TPDF, noise shaped).
    Q_PROPERTY(int ditherMode READ ditherMode WRITE setDitherMode NOTIFY ditherChanged)
    // Attenuation ahead of active DSP, dB (<= 0), and samples clipped since the track started.
    Q_PROPERTY(double headroomDb READ headroomDb WRITE setHeadroomDb NOTIFY headroomChanged)
    Q_PROPERTY(bool headroomAuto READ headroomAuto WRITE setHeadroomAuto NOTIFY headroomChanged)
    Q_PROPERTY(double effectiveHeadroomDb READ effectiveHeadroomDb NOTIFY headroomChanged)
    Q_PROPERTY(double clippedSamples READ clippedSamples NOTIFY signalPathChanged)
    // Headphone crossfeed: 0 off, 1 default (700 Hz, 4.5 dB), 2 Chu Moy, 3 Jan Meier.
    Q_PROPERTY(int crossfeed READ crossfeed WRITE setCrossfeed NOTIFY crossfeedChanged)
    // True-peak limiter at the end of the processed path (never on a
    // bit-perfect one): peaks above the ceiling (dBTP) are brought down to it.
    Q_PROPERTY(bool limiter READ limiter WRITE setLimiter NOTIFY limiterChanged)
    Q_PROPERTY(double limiterCeilingDb READ limiterCeilingDb WRITE setLimiterCeilingDb NOTIFY limiterChanged)
    // PCM upsampling: 0 off, 1 x2, 2 x4, 3 highest the DAC takes; and the
    // resampling filter: 0 linear, 1 intermediate, 2 minimum phase, 3 linear steep.
    Q_PROPERTY(int upsampling READ upsampling WRITE setUpsampling NOTIFY resamplingChanged)
    // FIR convolution: the impulse response file (kept while switched off).
    Q_PROPERTY(QString convolutionFile READ convolutionFile NOTIFY convolutionChanged)
    Q_PROPERTY(bool convolutionEnabled READ convolutionEnabled WRITE setConvolutionEnabled NOTIFY convolutionChanged)
    Q_PROPERTY(int resampleFilter READ resampleFilter WRITE setResampleFilter NOTIFY resamplingChanged)
    // ReplayGain (ReplayGainMode). Off by default: any gain leaves bit-perfect.
    Q_PROPERTY(int replayGainMode READ replayGainMode WRITE setReplayGainMode NOTIFY replayGainChanged)
    Q_PROPERTY(double replayGainPreamp READ replayGainPreamp WRITE setReplayGainPreamp NOTIFY replayGainChanged)
    Q_PROPERTY(bool replayGainPreventClipping READ replayGainPreventClipping WRITE setReplayGainPreventClipping NOTIFY replayGainChanged)
    // Gain applied to the current track in dB, NaN when it has no loudness data.
    Q_PROPERTY(double currentGainDb READ currentGainDb NOTIFY replayGainChanged)
    Q_PROPERTY(bool exclusiveMode READ exclusiveMode WRITE setExclusiveMode NOTIFY exclusiveModeChanged)
    // PipeWire: the graph follows each track's rate while Sonkore plays.
    Q_PROPERTY(bool followTrackRate READ followTrackRate WRITE setFollowTrackRate NOTIFY followTrackRateChanged)
    // DSD files (DsdMode): converted to PCM, or sent as DSD (native / DoP) to a DAC that takes it.
    Q_PROPERTY(int dsdMode READ dsdMode WRITE setDsdMode NOTIFY dsdModeChanged)
    // PCM converted to DSD for the DSD output: 0 off, 1 DSD64, 2 DSD128, 3 DSD256, 4 highest the DAC takes.
    Q_PROPERTY(int pcmToDsd READ pcmToDsd WRITE setPcmToDsd NOTIFY dsdModeChanged)
    // Each file is read into RAM before it plays: no disk access during playback.
    Q_PROPERTY(bool playFromMemory READ playFromMemory WRITE setPlayFromMemory NOTIFY playFromMemoryChanged)
    // HDCD CDs decoded to 20 bits (only where HDCD codes are found).
    Q_PROPERTY(bool hdcd READ hdcd WRITE setHdcd NOTIFY hdcdChanged)
    Q_PROPERTY(QVariantList outputDevices READ outputDevices NOTIFY outputDevicesChanged)
    Q_PROPERTY(QString currentDeviceKey READ currentDeviceKey NOTIFY outputStatusChanged)
    Q_PROPERTY(QString outputDescription READ outputDescription NOTIFY outputStatusChanged)
    Q_PROPERTY(bool bitPerfect READ bitPerfect NOTIFY outputStatusChanged)
    // A few words for the player bar: "Bit-perfect · 96 kHz", "Resampled ·
    // 48 kHz", "Processed · 44.1 kHz", "DSD64 native"; "" with no output.
    Q_PROPERTY(QString outputSummary READ outputSummary NOTIFY outputStatusChanged)
    // Every stage between the file and the DAC, as Roon shows it: a list of
    // {stage, detail, quality}, quality being "lossless", "high", "enhanced"
    // (DSP chosen on purpose) or "low".
    Q_PROPERTY(QVariantList signalPath READ signalPath NOTIFY signalPathChanged)

    // --- Transport / queue --------------------------------------------------
    Q_PROPERTY(bool playing READ playing NOTIFY stateChanged)
    Q_PROPERTY(bool loaded READ loaded NOTIFY stateChanged)
    Q_PROPERTY(double position READ position NOTIFY positionChanged)
    Q_PROPERTY(double duration READ duration NOTIFY trackChanged)
    Q_PROPERTY(QVariantMap currentTrack READ currentTrack NOTIFY trackChanged)
    Q_PROPERTY(QString currentPath READ currentPath NOTIFY trackChanged)
    Q_PROPERTY(QString trackDescription READ trackDescription NOTIFY trackChanged)
    Q_PROPERTY(QStringList queue READ queue NOTIFY queueChanged)
    Q_PROPERTY(int currentIndex READ currentIndex NOTIFY trackChanged)
    Q_PROPERTY(bool shuffle READ shuffle WRITE setShuffle NOTIFY shuffleChanged)
    Q_PROPERTY(int repeatMode READ repeatMode WRITE setRepeatMode NOTIFY repeatModeChanged)
    Q_PROPERTY(QString lastError READ lastError NOTIFY errorOccurred)
    Q_PROPERTY(bool resumeSession READ resumeSession WRITE setResumeSession NOTIFY preferencesChanged)
    Q_PROPERTY(int bufferMs READ bufferMs WRITE setBufferMs NOTIFY preferencesChanged)
    Q_PROPERTY(int resyncDelayMs READ resyncDelayMs WRITE setResyncDelayMs NOTIFY preferencesChanged)
    Q_PROPERTY(int fixedRate READ fixedRate WRITE setFixedRate NOTIFY preferencesChanged)
    // A direct (hw:) DAC is taken from the desktop sound server while Sonkore
    // uses it; `releaseWhenIdle` hands it back after a minute in pause/stop.
    Q_PROPERTY(bool outputReserved READ outputReserved NOTIFY outputStatusChanged)
    Q_PROPERTY(bool outputExclusiveCapable READ outputExclusiveCapable NOTIFY outputStatusChanged)
    // Audio libraries in use, with versions (About).
    Q_PROPERTY(QString audioComponents READ audioComponents CONSTANT)
    Q_PROPERTY(bool releaseWhenIdle READ releaseWhenIdle WRITE setReleaseWhenIdle NOTIFY preferencesChanged)
    // Each output keeps its own sound settings (equaliser, convolution,
    // crossfeed, headroom, dither, upsampling and filter, DSD): headphones
    // and speakers each come back with theirs when the output is switched.
    Q_PROPERTY(bool soundPerOutput READ soundPerOutput WRITE setSoundPerOutput NOTIFY preferencesChanged)
    Q_PROPERTY(bool testSignalPlaying READ testSignalPlaying NOTIFY trackChanged)
    Q_PROPERTY(bool dtsTestAvailable READ dtsTestAvailable CONSTANT)

    // --- Visualiser: spectrum of what is playing ----------------------------
    // While active (a visualiser is on screen), `spectrum` holds 24 levels
    // (0..1, 40 Hz – 16 kHz, log spaced) about 30 times a second. Each
    // visualiser says when it is in view (watchSpectrum); active while any is.
    Q_PROPERTY(bool visualizerActive READ visualizerActive NOTIFY visualizerActiveChanged)
    Q_PROPERTY(QList<qreal> spectrum READ spectrum NOTIFY spectrumChanged)

public:
    enum RepeatMode { RepeatOff = 0, RepeatAll = 1, RepeatOne = 2 };
    Q_ENUM(RepeatMode)
    enum ReplayGainMode { ReplayGainOff = 0, ReplayGainTrack = 1, ReplayGainAlbum = 2 };
    Q_ENUM(ReplayGainMode)
    enum VolumeMode { VolumeDigital = 0, VolumeHardware = 1, VolumeAuto = 2 };
    Q_ENUM(VolumeMode)
    enum DsdMode { DsdToPcm = 0, DsdNative = 1, DsdDop = 2 };
    Q_ENUM(DsdMode)
    enum DitherModeValue { DitherOff = 0, DitherTpdf = 1, DitherShaped = 2 };
    Q_ENUM(DitherModeValue)

    explicit DspController(sonkore::DSPManager& engine, QObject* parent = nullptr);
    ~DspController() override;
    // Writes settings and the session (queue, track, position) right away.
    void flush();

    double pitchSemitones() const;
    double tempo() const;
    double rate() const;
    void setPitchSemitones(double semitones);
    void setTempo(double ratio);
    void setRate(double ratio);

    bool useAAFilter() const;
    int aaFilterLength() const;
    bool useQuickSeek() const;
    int sequenceMs() const;
    int seekWindowMs() const;
    int overlapMs() const;
    void setUseAAFilter(bool on);
    void setAAFilterLength(int taps);
    void setUseQuickSeek(bool on);
    void setSequenceMs(int ms);
    void setSeekWindowMs(int ms);
    void setOverlapMs(int ms);
    bool soundTouchActive() const;
    bool soundTouchEnabled() const;
    void setSoundTouchEnabled(bool on);
    double speed() const;

    double volume() const;
    void setVolume(double v);
    int volumeMode() const { return m_volumeMode; }
    void setVolumeMode(int mode);
    bool hardwareVolume() const { return m_hwVolumeActive; }
    bool systemVolume() const { return m_pwVolumeActive; }
    bool systemVolumeInHardware() const;
    bool volumeFixed() const { return m_volumeFixed; }
    QVariantList signalPath() const { return m_signalPath; }
    bool hardwareVolumeSupported() const;
    QString hardwareVolumeInfo() const;
    bool directOutput() const; // a hw: device: nothing mixes in, so the DAC's volume can be used
    int ditherMode() const { return m_ditherMode; }
    void setDitherMode(int mode);
    double headroomDb() const { return m_headroomDb; }
    void setHeadroomDb(double db);
    bool headroomAuto() const { return m_headroomAuto; }
    void setHeadroomAuto(bool on);
    double effectiveHeadroomDb() const { return m_effectiveHeadroomDb; }
    double clippedSamples() const;
    int crossfeed() const { return m_crossfeed; }
    bool limiter() const { return m_limiter; }
    void setLimiter(bool on);
    double limiterCeilingDb() const { return m_limiterCeilingDb; }
    void setLimiterCeilingDb(double db);
    int upsampling() const { return m_upsampling; }
    QString convolutionFile() const { return m_convolutionFile; }
    bool convolutionEnabled() const { return m_convolutionEnabled; }
    void setConvolutionEnabled(bool on);
    // Loads an impulse response (WAV / FLAC...) and switches convolution on.
    Q_INVOKABLE bool loadConvolution(const QUrl& file);
    Q_INVOKABLE void clearConvolution();
    // The volume control's scale: linear in dB over the last 60 dB (each
    // pixel and step the same size wherever it is set), the far left muted.
    Q_INVOKABLE double volumeToPosition(double gain) const;
    Q_INVOKABLE double positionToVolume(double position) const;
    // Up or down by `db` (wheel, Ctrl+Up/Down); from mute, up starts at -60 dB.
    Q_INVOKABLE void stepVolume(double db);
    // A gain as "-12.0 dB", "-inf dB" (tooltips).
    Q_INVOKABLE QString gainText(double gain) const;
    void setUpsampling(int mode);
    int resampleFilter() const { return m_resampleFilter; }
    void setResampleFilter(int filter);
    void setCrossfeed(int preset);
    int replayGainMode() const { return m_rgMode; }
    void setReplayGainMode(int mode);
    double replayGainPreamp() const { return m_rgPreamp; }
    void setReplayGainPreamp(double db);
    bool replayGainPreventClipping() const { return m_rgPreventClipping; }
    void setReplayGainPreventClipping(bool on);
    double currentGainDb() const { return m_currentGainDb; }
    bool exclusiveMode() const { return m_exclusive; }
    bool followTrackRate() const { return m_followTrackRate; }
    int dsdMode() const { return m_dsdMode; }
    int pcmToDsd() const { return m_pcmToDsd; }
    void setPcmToDsd(int mode);
    bool playFromMemory() const { return m_playFromMemory; }
    bool hdcd() const { return m_hdcd; }
    void setHdcd(bool on);
    void setPlayFromMemory(bool on);
    void setDsdMode(int mode);
    void setExclusiveMode(bool on);
    void setFollowTrackRate(bool on);
    QVariantList outputDevices() const { return m_devices; }
    QString currentDeviceKey() const { return m_deviceKey; }
    QString outputDescription() const;
    QString outputSummary() const;
    bool bitPerfect() const;

    bool playing() const;
    bool loaded() const;
    double position() const { return m_position; }
    double duration() const;
    QVariantMap currentTrack() const { return m_currentTrack; }
    QString currentPath() const;
    QString trackDescription() const;
    QStringList queue() const { return m_queue; }
    int currentIndex() const { return m_currentIndex; }
    bool shuffle() const { return m_shuffle; }
    void setShuffle(bool on);
    int repeatMode() const { return m_repeat; }
    void setRepeatMode(int mode);
    QString lastError() const { return m_lastError; }
    bool resumeSession() const { return m_resume; }
    void setResumeSession(bool on);
    int bufferMs() const;
    int resyncDelayMs() const;
    void setResyncDelayMs(int ms);
    int fixedRate() const;
    void setFixedRate(int hz);
    // Fixed rate and upsampling together (they exclude each other), reopened once.
    Q_INVOKABLE void setRateMode(int fixedHz, int upsampling);
    void setBufferMs(int ms);
    // Sonkore has the DAC to itself right now (a reserved card, or an output
    // the system cannot share: WASAPI exclusive, ASIO, hw:, native DSD).
    bool outputReserved() const;
    // The chosen output is taken from the system whenever something plays.
    bool outputExclusiveCapable() const;
    QString audioComponents() const;
    bool releaseWhenIdle() const { return m_releaseWhenIdle; }
    bool soundPerOutput() const { return m_soundPerOutput; }
    void setSoundPerOutput(bool on);
    void setReleaseWhenIdle(bool on);
    bool visualizerActive() const { return m_visualizerActive; }
    // `viewer` (a visualiser) is in view, or no longer. One that is destroyed
    // stops watching by itself.
    Q_INVOKABLE void watchSpectrum(QObject* viewer, bool on);
    QList<qreal> spectrum() const { return m_spectrum; }

    // Locale-independent, correctly rounded decimal -> double. Returns an
    // invalid QVariant (undefined in QML) when the text is not a number.
    Q_INVOKABLE QVariant parseDecimal(const QString& text) const;
    // Fixed-point formatting used by the UI ('.' separator, no exponent).
    Q_INVOKABLE QString formatDecimal(double value, int decimals) const;
    // value + direction*step, snapped to the `decimals` grid and clamped, so
    // repeated +/- clicks never accumulate binary rounding drift.
    Q_INVOKABLE double stepValue(double value, double step, int direction, int decimals,
                                 double minimum, double maximum) const;
    // 2^(semitones/12), for display.
    Q_INVOKABLE double pitchRatio(double semitones) const;
    Q_INVOKABLE QString formatTime(double seconds) const;

    Q_INVOKABLE void resetSoundTouch();
    Q_INVOKABLE void refreshDevices();
    Q_INVOKABLE void selectOutputDevice(const QString& key);
    // Closes the output and gives a reserved DAC back to the system; the
    // next play() takes it again and resumes at the same position.
    Q_INVOKABLE void releaseOutput();
    // Test signals (Settings › Output): made on the spot and played in place
    // of the queue, which resumes where it was when the signal ends or is
    // stopped. kind: "tone" (rate in Hz), "dsd" (rate = 64 / 128 / 256),
    // "channels" (a tone on each channel in turn), "dts" (bit-perfect check).
    Q_INVOKABLE void playTestSignal(const QString& kind, int rate, int channels);
    Q_INVOKABLE void stopTestSignal();
    bool testSignalPlaying() const { return !m_previewPath.isEmpty(); }
    bool dtsTestAvailable() const;
    // Same without notifying the UI; used when quitting.
    void closeOutput();

    Q_INVOKABLE void playPaths(const QStringList& paths, int startIndex = 0);
    Q_INVOKABLE void enqueue(const QStringList& paths);
    Q_INVOKABLE void playIndex(int index);
    Q_INVOKABLE void clearQueue();
    Q_INVOKABLE void removeFromQueue(const QList<int>& indices);
    // A folder left the library: its tracks leave the queue, and if one of
    // them is the current track it is stopped and forgotten (not resumed).
    Q_INVOKABLE void forgetFolder(const QString& folder);
    // Moves the queue entries `indices` so they start at `target` (an index
    // in the queue before the move); the current track keeps playing.
    Q_INVOKABLE void moveInQueue(const QList<int>& indices, int target);
    // Sorts the queue by a track field ("path", "title", "artist", "album",
    // "duration", "year", "trackNo", "added", "favorite"); albums keep disc / track
    // order. The current track keeps playing.
    Q_INVOKABLE void sortQueue(const QString& field, bool descending = false);
    Q_INVOKABLE void reverseQueue();

public slots:
    bool load(const QUrl& url);
    void play();
    void pause();
    // A pause asked by the desktop (MPRIS), which may come from the sound
    // server's own policy rather than the listener.
    void pauseFromDesktop();
    void togglePlayPause();
    void stop();
    void next();
    void previous();
    void seek(double seconds);

signals:
    void pitchSemitonesChanged();
    void tempoChanged();
    void rateChanged();
    void soundTouchSettingsChanged();
    void soundTouchActiveChanged();
    void volumeChanged();
    void volumeModeChanged();
    void signalPathChanged();
    void ditherChanged();
    void headroomChanged();
    void crossfeedChanged();
    void limiterChanged();
    void resamplingChanged();
    void convolutionChanged();
    void replayGainChanged();
    void exclusiveModeChanged();
    void followTrackRateChanged();
    void dsdModeChanged();
    void playFromMemoryChanged();
    void hdcdChanged();
    void outputDevicesChanged();
    void stateChanged();
    void positionChanged();
    void trackChanged();
    void queueChanged();
    void shuffleChanged();
    void repeatModeChanged();
    void outputStatusChanged();
    void trackFinished();
    void errorOccurred();
    // The selected direct output belongs to `holder`, which refused to give it up.
    void outputBusy(const QString& holder);
    void seeked(double seconds);
    void preferencesChanged();
    void visualizerActiveChanged();
    void spectrumChanged();
    // Another program asked for the DAC and Sonkore let it go.
    void outputTakenByOther();
    // A sound device was plugged in / removed (names as listed).
    void outputDevicesHotplugged(const QStringList& added, const QStringList& removed);

private:
    // Puts the queue in `order` (old indices, a permutation of them all).
    void applyQueueOrder(const QVector<int>& order);
    void poll();
    void devicesChangedOnSystem();
    void outputGraphChanged();
    void updateSpectrum();
    void updateSpectrumTimer();
    void setVisualizerActive(bool on);
    void reportError(const QString& message);
    bool openIndex(int index, bool autoplay, double startAt = 0.0);
    bool reserveOutput(bool* justTaken);
    void updateIdleRelease();
    void advance(bool userInitiated);
    void rebuildShuffleOrder();
    int orderPosition() const;
    int upcomingIndex() const;
    void armGapless();
    double gainFor(const QString& path, QVariantMap info = {}, bool* known = nullptr) const;
    void updateMixer();
    QVariantList buildSignalPath() const;
    void applyGains();
    void reopenCurrent();
    void scheduleSave();
    void saveSettings() const;
    void savePreferences() const;
    void saveSession() const;
    void loadSettings();
    void loadSession(QSettings& legacy);

    sonkore::DSPManager& m_engine;
    QTimer m_pollTimer;
    QTimer m_saveTimer; // coalesces setting changes into one write
    QFileSystemWatcher m_deviceWatcher; // /dev/snd: DACs plugged in or out
    QTimer m_hotplugTimer;
    QTimer m_spectrumTimer;
    QHash<QObject*, QMetaObject::Connection> m_spectrumViewers; // and their destroyed()
    bool m_visualizerActive = false;
    QList<qreal> m_spectrum;
    mutable QByteArray m_savedSession;
    double m_position = 0.0;
    double m_resumePosition = 0.0;
    bool m_lastBitPerfect = false;
    int m_ditherMode = DitherTpdf;
    double m_headroomDb = 0.0;          // manual setting
    bool m_headroomAuto = false;        // manual, 0 dB by default
    double m_effectiveHeadroomDb = 0.0; // what the engine applies
    void applyHeadroom();
    int m_crossfeed = 0;
    int m_upsampling = 0;
    QString m_convolutionFile;
    bool m_convolutionEnabled = false;
    bool applyConvolution(QString* error = nullptr);
    int m_resampleFilter = 0;
    bool m_limiter = false;
    double m_limiterCeilingDb = -1.0;
    int m_volumeMode = VolumeAuto;
    bool m_soundPerOutput = true;
    // The sound settings as one map (what soundPerOutput stores per device).
    QVariantMap soundSnapshot() const;
    void restoreSound(const QVariantMap& state);
    void storeSoundFor(const QString& deviceKey) const;
    bool loadSoundFor(const QString& deviceKey);
    bool m_hwVolumeActive = false;
    bool m_pwVolumeActive = false;    // Linux: the PipeWire sink's volume (PipeWireVolume)
    PipeWireVolume* m_pwVolume = nullptr;
    double m_digitalVolume = 1.0; // kept while the engine is held at unity gain
    bool m_volumeFixed = false;   // Automatic on a direct output
    bool m_engineAtUnity = false;
    HardwareMixer* m_mixer = nullptr; // the DAC's own volume control (ALSA mixer / WASAPI endpoint)
    int m_rgMode = ReplayGainOff;
    double m_rgPreamp = 0.0;
    bool m_rgPreventClipping = true;
    double m_currentGainDb = 0.0;
    bool m_exclusive = true;
    bool m_followTrackRate = true;
    int m_dsdMode = DsdNative;
    int m_pcmToDsd = 0;
    bool m_playFromMemory = true;
    bool m_hdcd = false;
    QString m_lastError;

    QVariantList m_devices;
    QString m_deviceKey;
    DeviceReservation m_reservation;
    QTimer m_idleReleaseTimer;
    bool m_releaseWhenIdle = true;
    // A test signal plays instead of the queue; where the queue was.
    void endPreview(bool resumePlaying);
    QString m_previewPath;
    int m_previewIndex = -1;
    double m_previewPosition = 0.0;
    bool m_previewPlaying = false;

    QStringList m_queue;
    int m_currentIndex = -1;
    QString m_currentPath; // stays valid when its queue entry is removed
    bool m_resume = true;
    QVariantMap m_currentTrack;
    QVector<int> m_order; // shuffle order (indices into m_queue)
    bool m_shuffle = false;
    int m_repeat = RepeatOff;
    QString m_armedPath; // handed to the engine for a gapless join
    QVariantList m_signalPath;
    qint64 m_rtThread = 0;  // audio thread last handed to rtkit
    int m_rtPriority = 0;   // its real-time priority, 0 = normal scheduling
};
