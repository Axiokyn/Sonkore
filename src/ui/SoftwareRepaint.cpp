#include "SoftwareRepaint.hpp"

#include <QGuiApplication>
#include <QQuickWindow>
#include <QSGRendererInterface>
#include <QVersionNumber>
#include <QtGui/private/qhighdpiscaling_p.h>
#include <QtQuick/private/qquickwindow_p.h>
#include <QtQuick/private/qsgsoftwarerenderablenode_p.h>
#include <QtQuick/private/qsgsoftwarerenderer_p.h>

#include <algorithm>
#include <cmath>

// Two private members of the renderer: what it will repaint besides its
// changed nodes, and its nodes. The explicit instantiations below are the
// standard way to name them.
using RenderableNodes = QHash<QSGNode*, QSGSoftwareRenderableNode*>;
QRegion& pendingDirt(QSGAbstractSoftwareRenderer& renderer);
RenderableNodes& renderableNodes(QSGAbstractSoftwareRenderer& renderer);
template <QRegion QSGAbstractSoftwareRenderer::*Member>
struct PendingDirtAccess {
    friend QRegion& pendingDirt(QSGAbstractSoftwareRenderer& renderer) { return renderer.*Member; }
};
template struct PendingDirtAccess<&QSGAbstractSoftwareRenderer::m_dirtyRegion>;
template <RenderableNodes QSGAbstractSoftwareRenderer::*Member>
struct RenderableNodesAccess {
    friend RenderableNodes& renderableNodes(QSGAbstractSoftwareRenderer& renderer) { return renderer.*Member; }
};
template struct RenderableNodesAccess<&QSGAbstractSoftwareRenderer::m_nodes>;

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

// The smallest step, in the window's units, that is a whole number of the
// platform's units at Qt's own scale factor (4 at 1.25, 2 at 1.5); 1 when the
// factor is whole, 0 when no small step fits.
int gridFor(qreal factor)
{
    for (int step = 1; step <= 16; ++step)
        if (std::abs(step * factor - std::round(step * factor)) < 1e-4)
            return step;
    return 0;
}

int floorTo(int v, int step)
{
    return v >= 0 ? v / step * step : -((-v + step - 1) / step) * step;
}

QRegion snapped(const QRegion& region, int step, const QRect& bounds)
{
    QRegion out;
    for (const QRect& r : region)
        out += QRect(QPoint(floorTo(r.left(), step), floorTo(r.top(), step)),
                     QPoint(-floorTo(-(r.right() + 1), step) - 1, -floorTo(-(r.bottom() + 1), step) - 1));
    return out.intersected(bounds);
}

// The member offsets above come from the headers Sonkore was built with.
bool sameQtAsBuilt()
{
    const QVersionNumber running = QVersionNumber::fromString(QLatin1StringView(qVersion()));
    return running.majorVersion() == QT_VERSION_MAJOR && running.minorVersion() == QT_VERSION_MINOR;
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

namespace {
class WindowWatcher : public QObject {
public:
    using QObject::QObject;

protected:
    bool eventFilter(QObject* watched, QEvent* event) override
    {
        if (event->type() == QEvent::Show)
            if (auto* window = qobject_cast<QQuickWindow*>(watched))
                SoftwareRepaint::attach(window);
        return false;
    }
};
} // namespace

void SoftwareRepaint::watchWindows()
{
    for (QWindow* window : QGuiApplication::topLevelWindows())
        if (auto* quick = qobject_cast<QQuickWindow*>(window))
            attach(quick);
    qApp->installEventFilter(new WindowWatcher(qApp));
}

void SoftwareRepaint::attach(QQuickWindow* window)
{
    if (window && wanted(window) && !window->findChild<SoftwareRepaint*>(Qt::FindDirectChildrenOnly))
        new SoftwareRepaint(window);
}

SoftwareRepaint::SoftwareRepaint(QQuickWindow* window)
    : QObject(window)
    , m_window(window)
    , m_canSnap(sameQtAsBuilt())
{
    connect(window, &QQuickWindow::beforeRendering, this, &SoftwareRepaint::beforeFrame, Qt::DirectConnection);
    connect(window, &QQuickWindow::afterRendering, this, &SoftwareRepaint::afterFrame, Qt::DirectConnection);
}

void SoftwareRepaint::beforeFrame()
{
    m_forced = false;
    m_snapped = QRegion();
    QSGSoftwareRenderer* renderer = softwareRenderer(m_window);
    if (!renderer)
        return;
    m_grid = gridFor(QHighDpiScaling::factor(static_cast<QWindow*>(m_window.data())));
    if (m_grid != 1 && (m_grid == 0 || !m_canSnap)) { // as Qt itself would
        renderer->markDirty();
        m_forced = true;
        return;
    }
    if (m_wholeFrames.load(std::memory_order_relaxed)) {
        if (++m_sinceProbe < kProbeEvery) {
            renderer->markDirty();
            m_forced = true;
            m_mend = QRegion();
            return;
        }
        m_sinceProbe = 0; // left to the renderer: is it still moving?
    }
    if (m_grid > 1)
        snapToGrid(*renderer);
}

// At a fractional scale factor of Qt's own (Sonkore's interface scale, or
// Windows' display scaling) the edges of what is repainted fall inside the
// platform's pixels, and Qt rounds each edge three ways: the painter's clip,
// the backing store's clearing and record of what is fresh, and the damage
// sent to the compositor. A pixel along an edge can then be cleared and not
// painted, or painted and never shown: a stray line or a see-through hole.
// Widened to a step that is whole in the platform's units, all three agree.
// The renderer widens what it repaints on its own (an opaque item at a
// fractional position brings in the outline around it, a custom render node
// its whole box); those are added here too, and anything still missed is
// repainted on the next frame.
void SoftwareRepaint::snapToGrid(QSGSoftwareRenderer& renderer)
{
    QRegion seed = pendingDirt(renderer) + m_mend;
    m_mend = QRegion();
    // Nodes that widen what is repainted once it reaches them: an opaque one
    // at a fractional position (its outline) and a custom render node (its
    // whole box, which Qt reports as repainted).
    QVarLengthArray<QSGSoftwareRenderableNode*, 64> widening;
    for (QSGSoftwareRenderableNode* node : std::as_const(renderableNodes(renderer))) {
        if (node->isDirty()) {
            seed += node->dirtyRegion();
            seed += node->previousDirtyRegion();
        }
        if (node->type() == QSGSoftwareRenderableNode::RenderNode
            || (node->isOpaque() && node->boundingRectMax() != node->boundingRectMin()))
            widening.append(node);
    }
    if (seed.isEmpty())
        return;
    const QRect bounds(QPoint(), m_window->size());
    seed = snapped(seed, m_grid, bounds);
    for (int pass = 0; pass < 3; ++pass) {
        QRegion more;
        for (QSGSoftwareRenderableNode* node : widening) {
            const QRect box = node->boundingRectMax();
            if (!seed.intersects(box))
                continue;
            if (node->type() == QSGSoftwareRenderableNode::RenderNode)
                more += box;
            else
                more += QRegion(box).subtracted(node->boundingRectMin());
        }
        more = snapped(more.subtracted(seed), m_grid, bounds).subtracted(seed);
        if (more.isEmpty())
            break;
        seed += more;
    }
    if (large(seed, m_window->size())) {
        renderer.markDirty();
        m_forced = true;
        return;
    }
    pendingDirt(renderer) = seed;
    m_snapped = seed;
}

void SoftwareRepaint::afterFrame()
{
    if (m_forced)
        return; // whole by our own doing: says nothing about what changed
    QSGSoftwareRenderer* renderer = softwareRenderer(m_window);
    if (!renderer || !m_window)
        return;
    const QRegion painted = renderer->flushRegion();
    const bool whole = large(painted, m_window->size());
    if (whole != m_wholeFrames.load(std::memory_order_relaxed)) {
        m_wholeFrames.store(whole, std::memory_order_relaxed);
        m_sinceProbe = 0;
    }
    if (m_grid > 1 && !whole) {
        const QRegion missed = painted.subtracted(m_snapped);
        if (!missed.isEmpty()) { // one pixel out covers the rows rounded away
            QRegion around;
            for (const QRect& r : missed)
                around += r.adjusted(-1, -1, 1, 1);
            m_mend = snapped(around, m_grid, QRect(QPoint(), m_window->size()));
            QMetaObject::invokeMethod(m_window, &QQuickWindow::update, Qt::QueuedConnection);
        }
    }
}
