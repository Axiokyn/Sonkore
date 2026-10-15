#include "SoftwareRepaint.hpp"

#include <QQuickWindow>
#include <QSGRendererInterface>
#include <QtQuick/private/qquickwindow_p.h>
#include <QtQuick/private/qsgsoftwarerenderer_p.h>

#include <algorithm>

namespace {
constexpr int kProbeEvery = 8;      // frames repainted whole between two left to the renderer
constexpr double kLargeShare = 0.5; // of the window's area: "the whole window changes"
constexpr int kManyPieces = 64;     // a region in more rectangles than this costs more than a whole repaint

QSGSoftwareRenderer* softwareRenderer(QQuickWindow* window)
{
    if (!window)
        return nullptr;
    return dynamic_cast<QSGSoftwareRenderer*>(QQuickWindowPrivate::get(window)->renderer);
}

// Spread over most of the window, or cut in many pieces (a scrolling list
// gives hundreds of slivers): each piece is a clip and a damage rectangle
// for the compositor, and together they cost more than one whole repaint.
bool large(const QRegion& region, const QSize& window)
{
    const QRect box = region.boundingRect();
    const double share = double(box.width()) * box.height() / std::max(1.0, double(window.width()) * window.height());
    return share > kLargeShare || region.rectCount() > kManyPieces;
}
} // namespace

void SoftwareRepaint::prepare()
{
    if (qEnvironmentVariableIsEmpty("QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES"))
        qputenv("QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES", "1");
}

bool SoftwareRepaint::wanted(QQuickWindow* window)
{
    const QSGRendererInterface* renderer = window ? window->rendererInterface() : nullptr;
    return renderer && renderer->graphicsApi() == QSGRendererInterface::Software
        && qEnvironmentVariableIntValue("QSG_SOFTWARE_RENDERER_FORCE_PARTIAL_UPDATES") == 1;
}

SoftwareRepaint::SoftwareRepaint(QQuickWindow* window)
    : QObject(window)
    , m_window(window)
{
    connect(window, &QQuickWindow::beforeRendering, this, &SoftwareRepaint::beforeFrame, Qt::DirectConnection);
    connect(window, &QQuickWindow::afterRendering, this, &SoftwareRepaint::afterFrame, Qt::DirectConnection);
}

void SoftwareRepaint::beforeFrame()
{
    m_forced = false;
    if (!m_wholeFrames.load(std::memory_order_relaxed))
        return;
    if (++m_sinceProbe >= kProbeEvery) { // left to the renderer: is it still moving?
        m_sinceProbe = 0;
        return;
    }
    if (QSGSoftwareRenderer* renderer = softwareRenderer(m_window)) {
        renderer->markDirty();
        m_forced = true;
    }
}

void SoftwareRepaint::afterFrame()
{
    if (m_forced)
        return; // whole by our own doing: says nothing about what changed
    QSGSoftwareRenderer* renderer = softwareRenderer(m_window);
    if (!renderer || !m_window)
        return;
    const bool whole = large(renderer->flushRegion(), m_window->size());
    if (whole != m_wholeFrames.load(std::memory_order_relaxed)) {
        m_wholeFrames.store(whole, std::memory_order_relaxed);
        m_sinceProbe = 0;
    }
}
