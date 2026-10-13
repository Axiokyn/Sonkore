#include "DeviceReservation.hpp"

#include <QCoreApplication>
#include <QFileInfo>
#include <QRegularExpression>
#include <QThread>

#if defined(SONKORE_HAVE_DBUS)
#include <QDBusAbstractAdaptor>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusInterface>
#include <QDBusReply>

namespace {

QString serviceName(int card) { return QStringLiteral("org.freedesktop.ReserveDevice1.Audio%1").arg(card); }
QString objectPath(int card) { return QStringLiteral("/org/freedesktop/ReserveDevice1/Audio%1").arg(card); }

} // namespace

// The object other programs talk to while Sonkore holds a card.
class ReserveDeviceAdaptor : public QDBusAbstractAdaptor {
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.freedesktop.ReserveDevice1")
    Q_PROPERTY(int Priority READ priority)
    Q_PROPERTY(QString ApplicationName READ applicationName)
    Q_PROPERTY(QString ApplicationDeviceName READ applicationDeviceName)
public:
    ReserveDeviceAdaptor(QObject* exported, DeviceReservation* owner, int card)
        : QDBusAbstractAdaptor(exported), m_owner(owner), m_card(card) {}
    int priority() const { return DeviceReservation::kPriority; }
    QString applicationName() const { return QStringLiteral("Sonkore"); }
    QString applicationDeviceName() const { return QStringLiteral("hw:%1").arg(m_card); }

public slots:
    bool RequestRelease(int priority) { return m_owner->handleReleaseRequest(priority); }

private:
    DeviceReservation* m_owner;
    int m_card;
};
#endif

DeviceReservation::DeviceReservation(QObject* parent)
    : QObject(parent)
{
}

DeviceReservation::~DeviceReservation()
{
    release();
}

int DeviceReservation::cardIndexOf(const QString& alsaDevice)
{
    static const QRegularExpression byName(QStringLiteral("CARD=([^,]+)"));
    static const QRegularExpression byIndex(QStringLiteral("^(?:plug)?hw:(\\d+)"));
    if (const auto m = byIndex.match(alsaDevice); m.hasMatch())
        return m.captured(1).toInt();
    if (const auto m = byName.match(alsaDevice); m.hasMatch()) {
        const QString card = m.captured(1);
        bool numeric = false;
        const int n = card.toInt(&numeric);
        if (numeric)
            return n;
        // /proc/asound/<id> links to card<N>.
        const QString target = QFileInfo(QStringLiteral("/proc/asound/") + card).symLinkTarget();
        static const QRegularExpression cardDir(QStringLiteral("card(\\d+)$"));
        if (const auto c = cardDir.match(target); c.hasMatch())
            return c.captured(1).toInt();
    }
    return -1;
}

bool DeviceReservation::acquire(int card, QString* holder)
{
    if (card == m_card)
        return true;
    release();
    if (card < 0)
        return true;
#if defined(SONKORE_HAVE_DBUS)
    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected())
        return true;
    QDBusConnectionInterface* dbus = bus.interface();
    const QString name = serviceName(card);

    m_exported = new QObject(this);
    new ReserveDeviceAdaptor(m_exported, this, card);
    bus.registerObject(objectPath(card), m_exported);

    auto request = [&](QDBusConnectionInterface::ServiceQueueOptions queue) {
        const auto r = dbus->registerService(name, queue, QDBusConnectionInterface::AllowReplacement);
        return r.isValid() && r.value() == QDBusConnectionInterface::ServiceRegistered;
    };

    bool ok = request(QDBusConnectionInterface::DontQueueService);
    if (!ok) {
        QDBusInterface owner(name, objectPath(card), QStringLiteral("org.freedesktop.ReserveDevice1"), bus);
        if (holder)
            *holder = owner.property("ApplicationName").toString();
        owner.setTimeout(3000);
        const QDBusReply<bool> agreed = owner.call(QStringLiteral("RequestRelease"), kPriority);
        if (agreed.isValid() && agreed.value()) {
            // The holder may drop the name a moment after answering.
            for (int i = 0; i < 20 && !ok; ++i) {
                ok = request(QDBusConnectionInterface::ReplaceExistingService);
                if (!ok)
                    QThread::msleep(25);
            }
        }
    }
    if (!ok) {
        bus.unregisterObject(objectPath(card));
        m_exported->deleteLater();
        m_exported = nullptr;
        if (holder && holder->isEmpty())
            *holder = QCoreApplication::translate("DeviceReservation", "another application");
        return false;
    }
    m_card = card;
#else
    Q_UNUSED(holder);
    m_card = card;
#endif
    return true;
}

void DeviceReservation::release()
{
    if (m_card < 0)
        return;
#if defined(SONKORE_HAVE_DBUS)
    QDBusConnection bus = QDBusConnection::sessionBus();
    bus.interface()->unregisterService(serviceName(m_card));
    bus.unregisterObject(objectPath(m_card));
    m_exported->deleteLater(); // may be inside its own RequestRelease call
    m_exported = nullptr;
#endif
    m_card = -1;
}

bool DeviceReservation::handleReleaseRequest(int priority)
{
    if (m_card < 0 || priority <= kPriority)
        return false;
    emit releaseRequested(); // receiver closes the device
    release();
    return true;
}

#if defined(SONKORE_HAVE_DBUS)
#include "DeviceReservation.moc"
#endif
