#pragma once

#include <QObject>
#include <QString>

// Takes a sound card away from the desktop sound server so Sonkore can open its
// raw hw: device, using the session-bus device reservation protocol
// (org.freedesktop.ReserveDevice1.Audio<card>) that PipeWire's session
// manager and other sound servers honour: they hold every card at a low
// priority, release it when asked by a higher one and take it back as soon
// as the name is free again.
//
// Without D-Bus (or on other systems) every call is a successful no-op.
class DeviceReservation : public QObject {
    Q_OBJECT
public:
    explicit DeviceReservation(QObject* parent = nullptr);
    ~DeviceReservation() override;

    // ALSA card index for a hw: device ("hw:CARD=DX1,DEV=0", "hw:0,0"), or -1.
    static int cardIndexOf(const QString& alsaDevice);

    // Reserves `card`, asking the current holder to let go. Returns false and
    // fills `holder` with the owner's name when it refuses.
    bool acquire(int card, QString* holder = nullptr);
    // Gives the card back (the sound server picks it up again by itself).
    void release();

    int card() const { return m_card; }
    bool held() const { return m_card >= 0; }

    static constexpr int kPriority = 10; // sound servers hold cards at < 0

signals:
    // Another program with a higher priority wants the card. The receiver
    // must close the device synchronously; the reservation is then dropped.
    void releaseRequested();

private:
    friend class ReserveDeviceAdaptor;
    bool handleReleaseRequest(int priority);

    int m_card = -1;
    QObject* m_exported = nullptr;
};
