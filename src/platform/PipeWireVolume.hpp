#pragma once

#include <QObject>
#include <QString>

#include <memory>

namespace sonkore {
class PipeWireOutput;
}

// The volume of a PipeWire output (sink), moved from Sonkore: on a device's
// sink it is the DAC's own control (PipeWire sets the ALSA mixer and leaves
// the samples alone where the control covers the whole range), so the
// desktop's volume and Sonkore's slider are one and the same, and the
// bit-perfect path survives any volume. Follows changes made elsewhere (the
// desktop's volume keys, pavucontrol, wpctl). A virtual sink (an effects
// chain, a null sink) scales the samples in software: hardware() says so.
class PipeWireVolume : public QObject {
    Q_OBJECT
public:
    explicit PipeWireVolume(QObject* parent = nullptr);
    ~PipeWireVolume() override;

    // `sink`: a node.name, or "" for the desktop's default output (followed
    // when it changes). False when PipeWire is not there or has no such sink.
    bool open(const QString& sink);
    void close();
    bool isOpen() const;

    // Linear amplitude 0..1 (0 while muted).
    double volume() const;
    void setVolume(double linear); // 0 mutes, anything above unmutes
    // The sink is a device's and its control carries the volume (no software
    // scaling in PipeWire): the samples reach the DAC untouched.
    bool hardware() const;
    QString sinkName() const; // what the desktop calls it

signals:
    void volumeChanged();

private:
    std::unique_ptr<sonkore::PipeWireOutput> m_pw;
    bool m_open = false;
};
