#include "RealtimeKit.hpp"

#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#include <QFile>
#include <QVariant>

#include <sched.h>
#include <sys/resource.h>
#include <unistd.h>

namespace RealtimeKit {

namespace {

// rtkit on the system bus, or inside a Flatpak sandbox (which cannot reach
// it) the Realtime portal on the session bus: same methods and properties,
// and the portal maps the sandbox's PIDs to the host's.
struct Service {
    QString name;
    QString path;
    QString interface;
    QDBusConnection bus;
};

Service service()
{
    if (QFile::exists(QStringLiteral("/.flatpak-info")))
        return {QStringLiteral("org.freedesktop.portal.Desktop"), QStringLiteral("/org/freedesktop/portal/desktop"),
                QStringLiteral("org.freedesktop.portal.Realtime"), QDBusConnection::sessionBus()};
    return {QStringLiteral("org.freedesktop.RealtimeKit1"), QStringLiteral("/org/freedesktop/RealtimeKit1"),
            QStringLiteral("org.freedesktop.RealtimeKit1"), QDBusConnection::systemBus()};
}

QVariant property(const Service& rt, const char* name)
{
    QDBusInterface props(rt.name, rt.path, QStringLiteral("org.freedesktop.DBus.Properties"), rt.bus);
    const QDBusReply<QVariant> reply = props.call(QStringLiteral("Get"), rt.interface, QString::fromLatin1(name));
    return reply.isValid() ? reply.value() : QVariant();
}

} // namespace

int currentPriority(qint64 tid)
{
    // rtkit sets SCHED_RR with SCHED_RESET_ON_FORK, which comes back or'ed in.
    const int policy = sched_getscheduler(static_cast<pid_t>(tid)) & ~SCHED_RESET_ON_FORK;
    if (policy != SCHED_FIFO && policy != SCHED_RR)
        return 0;
    sched_param param{};
    return sched_getparam(static_cast<pid_t>(tid), &param) == 0 ? param.sched_priority : 0;
}

int makeRealtime(qint64 tid)
{
    const Service rt = service();
    if (!rt.bus.isConnected())
        return 0;
    const int priority = property(rt, "MaxRealtimePriority").toInt();
    const qlonglong rtTime = property(rt, "RTTimeUSecMax").toLongLong();
    if (priority <= 0 || rtTime <= 0)
        return 0;
    // rtkit only serves processes whose real-time CPU time is capped: a
    // runaway real-time thread then gets SIGXCPU instead of freezing the system.
    rlimit limit{static_cast<rlim_t>(rtTime), static_cast<rlim_t>(rtTime)};
    if (setrlimit(RLIMIT_RTTIME, &limit) != 0)
        return 0;
    QDBusInterface rtkit(rt.name, rt.path, rt.interface, rt.bus);
    const QDBusMessage reply = rtkit.call(QStringLiteral("MakeThreadRealtimeWithPID"),
                                          QVariant::fromValue(static_cast<quint64>(getpid())),
                                          QVariant::fromValue(static_cast<quint64>(tid)),
                                          QVariant::fromValue(static_cast<quint32>(priority)));
    if (reply.type() == QDBusMessage::ErrorMessage)
        return 0;
    return currentPriority(tid);
}

} // namespace RealtimeKit
