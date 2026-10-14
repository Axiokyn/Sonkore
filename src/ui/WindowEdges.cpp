#include "WindowEdges.hpp"

#include <QImage>
#include <QPainter>
#include <QQuickWindow>
#include <QSGRenderNode>
#include <QSGRendererInterface>

#include <cmath>

namespace {

// One edge, bottom or right: its node covers the window's last two logical
// rows (or columns), so it is redrawn whenever they are.
class EdgeNode : public QSGRenderNode {
public:
    EdgeNode(QQuickItem* item, bool bottom)
        : m_item(item)
        , m_bottom(bottom)
    {
    }

    void setSize(qreal width, qreal height)
    {
        m_rect = m_bottom ? QRectF(0, height - 2, width, 2) : QRectF(width - 2, 0, 2, height);
        markDirty(QSGNode::DirtyMaterial);
    }

    void render(const RenderState* state) override
    {
        QQuickWindow* window = m_item->window();
        if (!window || m_rect.isEmpty() || !state->clipRegion() || state->clipRegion()->isEmpty())
            return;
        auto* p = static_cast<QPainter*>(window->rendererInterface()->getResource(window, QSGRendererInterface::PainterResource));
        auto* image = p ? dynamic_cast<QImage*>(p->device()) : nullptr;
        if (!image)
            return;
        // The device pixels inside the window as the renderer reckons it
        // (its whole-number logical size, truncated) are painted; past them
        // the last painted row (column) is repeated.
        const qreal dpr = p->device()->devicePixelRatio();
        const int w = image->width(), h = image->height();
        const int painted = m_bottom ? static_cast<int>(std::floor(static_cast<int>(h / dpr) * dpr))
                                     : static_cast<int>(std::floor(static_cast<int>(w / dpr) * dpr));
        const int size = m_bottom ? h : w;
        if (painted < 1 || painted >= size)
            return;
        const QImage line = m_bottom ? image->copy(0, painted - 1, w, 1) : image->copy(painted - 1, 0, 1, h);
        p->setClipping(false);
        p->setTransform(QTransform::fromScale(1.0 / dpr, 1.0 / dpr)); // device pixels
        p->setOpacity(1.0);
        p->setCompositionMode(QPainter::CompositionMode_Source);
        p->setRenderHint(QPainter::SmoothPixmapTransform, false);
        p->drawImage(m_bottom ? QRectF(0, painted, w, h - painted) : QRectF(painted, 0, w - painted, h), line);
    }

    StateFlags changedStates() const override { return {}; }
    RenderingFlags flags() const override { return BoundedRectRendering; }
    QRectF rect() const override { return m_rect; }

private:
    QQuickItem* m_item;
    bool m_bottom;
    QRectF m_rect;
};

} // namespace

WindowEdges::WindowEdges(QQuickItem* parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
}

void WindowEdges::geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry)
{
    QQuickItem::geometryChange(newGeometry, oldGeometry);
    if (newGeometry.size() != oldGeometry.size())
        update();
}

QSGNode* WindowEdges::updatePaintNode(QSGNode* old, UpdatePaintNodeData*)
{
    // Only the software renderer hands a QPainter to render nodes.
    const bool software = window() && window()->rendererInterface()->graphicsApi() == QSGRendererInterface::Software;
    if (!software || width() <= 0 || height() <= 0) {
        delete old;
        return nullptr;
    }
    QSGNode* root = old;
    if (!root) {
        root = new QSGNode;
        root->appendChildNode(new EdgeNode(this, true));
        root->appendChildNode(new EdgeNode(this, false));
    }
    for (QSGNode* n = root->firstChild(); n; n = n->nextSibling())
        static_cast<EdgeNode*>(n)->setSize(width(), height());
    return root;
}
