#include "PipeWireVolume.hpp"

#include "PipeWireOutput.hpp"

#include <QMetaObject>

#include <algorithm>
#include <cmath>

PipeWireVolume::PipeWireVolume(QObject* parent)
    : QObject(parent)
    , m_pw(std::make_unique<sonkore::PipeWireOutput>())
{
    // PipeWire's thread reports; the signal goes out on this object's thread.
    m_pw->setSinkChangedCallback([this] { QMetaObject::invokeMethod(this, &PipeWireVolume::volumeChanged, Qt::QueuedConnection); });
    m_pw->setGraphChangedCallback([this] { QMetaObject::invokeMethod(this, &PipeWireVolume::graphChanged, Qt::QueuedConnection); });
}

PipeWireVolume::~PipeWireVolume()
{
    m_pw->setSinkChangedCallback({});
    m_pw->setGraphChangedCallback({});
    close();
    m_pw->release();
}

bool PipeWireVolume::watchGraph()
{
    std::string error;
    return m_pw->watchGraph(&error);
}

std::vector<sonkore::PipeWireOutput::Sink> PipeWireVolume::sinks() const
{
    return m_pw->currentSinks();
}

QStringList PipeWireVolume::sinkNames() const
{
    QStringList out;
    for (const auto& s : m_pw->currentSinks())
        out << QString::fromStdString(s.name);
    return out;
}

bool PipeWireVolume::open(const QString& sink)
{
    std::string error;
    m_open = m_pw->watch(sink.toStdString(), &error);
    if (!m_open)
        m_pw->unwatch();
    return m_open;
}

void PipeWireVolume::close()
{
    if (!m_open)
        return;
    m_open = false;
    m_pw->unwatch();
}

bool PipeWireVolume::isOpen() const
{
    return m_open && m_pw->watching();
}

double PipeWireVolume::volume() const
{
    float v = 1.0f;
    bool muted = false;
    if (!m_pw->sinkVolume(&v, &muted))
        return 1.0;
    return muted ? 0.0 : std::clamp<double>(v, 0.0, 1.0);
}

void PipeWireVolume::setVolume(double linear)
{
    if (!std::isfinite(linear))
        return;
    const double v = std::clamp(linear, 0.0, 1.0);
    m_pw->setSinkVolume(static_cast<float>(v), v <= 0.0);
}

bool PipeWireVolume::hardware() const
{
    return m_pw->sinkVolumeInHardware();
}

QString PipeWireVolume::sinkName() const
{
    return QString::fromStdString(m_pw->watchedSinkDescription());
}
