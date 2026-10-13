#pragma once

#include <QObject>
#include <QString>

#include <memory>
#include <vector>

class QSocketNotifier;
typedef struct _snd_mixer snd_mixer_t;
typedef struct _snd_mixer_elem snd_mixer_elem_t;

// The playback volume control of an ALSA card (the DAC's own attenuator), so
// the volume can change while the samples reach the DAC untouched. Picks the
// card's main playback control and follows changes made elsewhere
// (alsamixer, another program, a knob the driver reports).
class AlsaMixer : public QObject {
    Q_OBJECT
public:
    explicit AlsaMixer(QObject* parent = nullptr);
    ~AlsaMixer() override;

    // `card`: "hw:CARD=DX1" or "hw:1". False when the card has no usable
    // playback volume control.
    bool open(const QString& card);
    void close();
    bool isOpen() const { return m_elem != nullptr; }

    // Linear amplitude 0..1, where 1 is the top of the control (capped at
    // 0 dB: a control that can amplify is never pushed above unity). 0 mutes
    // when the control has a switch, otherwise sets its minimum.
    double volume() const;
    void setVolume(double linear);

    QString controlName() const { return m_name; }
    QString card() const { return m_card; }
    bool hasDbScale() const { return m_hasDb; }
    double minDb() const { return m_minDb; }
    double topDb() const { return m_topDb; }

signals:
    void volumeChanged();

private:
    void handleEvents();

    snd_mixer_t* m_mixer = nullptr;
    snd_mixer_elem_t* m_elem = nullptr;
    QString m_card;
    QString m_name;
    bool m_hasDb = false;
    double m_minDb = 0.0, m_topDb = 0.0;
    long m_minRaw = 0, m_maxRaw = 0;
    std::vector<std::unique_ptr<QSocketNotifier>> m_notifiers;
};
