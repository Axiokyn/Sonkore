#pragma once

#include <QObject>
#include <QString>

struct IAudioEndpointVolume;

// Windows: the volume control of the DAC itself (IAudioEndpointVolume on a
// device that implements it in hardware), so the volume can change while
// WASAPI exclusive mode hands the DAC untouched samples. Same interface as
// AlsaMixer; follows changes made elsewhere (the Windows volume flyout, the
// DAC's knob when its driver reports it).
class WinEndpointVolume : public QObject {
    Q_OBJECT
public:
    explicit WinEndpointVolume(QObject* parent = nullptr);
    ~WinEndpointVolume() override;

    // `endpointId`: the WASAPI endpoint id ("" = the default output). False
    // when the device has no volume control of its own: Windows would then
    // scale the samples in software, which exclusive mode bypasses anyway.
    bool open(const QString& endpointId);
    void close();
    bool isOpen() const { return m_volume != nullptr; }

    // Linear amplitude 0..1, where 1 is the top of the control (capped at
    // 0 dB). 0 mutes.
    double volume() const;
    void setVolume(double linear);

    QString controlName() const { return m_name; }
    bool hasDbScale() const { return true; }
    double minDb() const { return m_minDb; }
    double topDb() const { return m_topDb; }

signals:
    void volumeChanged();

private:
    class Callback;
    IAudioEndpointVolume* m_volume = nullptr;
    Callback* m_callback = nullptr;
    QString m_endpoint;
    QString m_name;
    double m_minDb = 0.0, m_topDb = 0.0;
    bool m_comInitialized = false;
};
