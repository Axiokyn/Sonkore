#include "FrameAnimationDriver.hpp"

#include <QEvent>
#include <QGuiApplication>
#include <QQuickWindow>
#include <QSGRendererInterface>

namespace {
constexpr int kWatchdogMs = 50; // ~20 steps a second while no frame comes
}

FrameAnimationDriver::FrameAnimationDriver(QQuickWindow* window)
    : QAnimationDriver(window)
    , m_window(window)
{
    m_clock.start();
    m_watchdog.setInterval(kWatchdogMs);
    connect(&m_watchdog, &QTimer::timeout, this, [this] {
        if (isRunning() && m_clock.elapsed() - m_lastStep >= kWatchdogMs)
            step();
    });
    window->installEventFilter(this);
}

bool FrameAnimationDriver::wanted(QQuickWindow* window)
{
    if (qEnvironmentVariableIsSet("SONKORE_FRAME_DRIVER") && qEnvironmentVariableIntValue("SONKORE_FRAME_DRIVER") == 0)
        return false; // SONKORE_FRAME_DRIVER=0: Qt's own timer, as before
    // Asked of the window: the renderer may have been chosen through the
    // environment (QT_QUICK_BACKEND), which QQuickWindow::graphicsApi() misses.
    const QSGRendererInterface* renderer = window ? window->rendererInterface() : nullptr;
    return renderer && renderer->graphicsApi() == QSGRendererInterface::Software
        && QGuiApplication::platformName().startsWith(QLatin1String("wayland"));
}

bool FrameAnimationDriver::eventFilter(QObject* watched, QEvent* event)
{
    // The compositor's turn: the frame about to be drawn shows the
    // animations as they are now.
    if (event->type() == QEvent::UpdateRequest && watched == m_window && isRunning())
        step();
    return false;
}

void FrameAnimationDriver::step()
{
    m_lastStep = m_clock.elapsed();
    advance();
    // Animations still running: the next turn is asked for (it comes with the
    // next frame callback, not before).
    if (isRunning() && m_window)
        m_window->requestUpdate();
}

void FrameAnimationDriver::start()
{
    QAnimationDriver::start();
    m_lastStep = m_clock.elapsed();
    if (m_window)
        m_window->requestUpdate();
    m_watchdog.start();
}

void FrameAnimationDriver::stop()
{
    m_watchdog.stop();
    QAnimationDriver::stop();
}
