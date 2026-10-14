#include "RoundBox.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QQuickWindow>
#include <QSGRenderNode>
#include <QSGRendererInterface>

#include <algorithm>
#include <cmath>

namespace {

class BoxNode : public QSGRenderNode {
public:
    explicit BoxNode(QQuickItem* item)
        : m_item(item)
    {
    }

    void setLook(const RoundBox::Look& look)
    {
        m_look = look;
        markDirty(QSGNode::DirtyMaterial);
    }

    void render(const RenderState* state) override
    {
        QQuickWindow* window = m_item->window();
        if (!window || m_look.width <= 0 || m_look.height <= 0)
            return;
        auto* p = static_cast<QPainter*>(window->rendererInterface()->getResource(window, QSGRendererInterface::PainterResource));
        if (!p || !p->device())
            return;
        // The region to repaint, in the window's logical pixels (set before
        // the transform below).
        const QRegion* clip = state->clipRegion();
        if (clip && !clip->isEmpty())
            p->setClipRegion(*clip, Qt::ReplaceClip);

        // The box on the buffer's own pixels: its edges rounded to whole
        // device pixels, so straight sides are sharp and alike.
        const qreal dpr = p->device()->devicePixelRatio();
        const QRectF r = matrix()->toTransform().mapRect(QRectF(0, 0, m_look.width, m_look.height));
        const qreal x0 = std::round(r.left() * dpr), y0 = std::round(r.top() * dpr);
        const qreal x1 = std::max(x0 + 1, std::round(r.right() * dpr)), y1 = std::max(y0 + 1, std::round(r.bottom() * dpr));
        const QRectF box(x0, y0, x1 - x0, y1 - y0);
        const qreal scale = r.width() > 0 ? box.width() / r.width() : dpr; // logical px → device px, item scale included
        const qreal radius = std::clamp(m_look.radius * scale, 0.0, std::min(box.width(), box.height()) / 2);
        const qreal border = m_look.borderWidth > 0 && m_look.borderColor.alpha() > 0
                                 ? std::min(std::max<qreal>(1.0, std::round(m_look.borderWidth * scale)), std::min(box.width(), box.height()) / 2)
                                 : 0.0;

        p->setTransform(QTransform::fromScale(1.0 / dpr, 1.0 / dpr));
        p->setOpacity(inheritedOpacity());
        p->setRenderHint(QPainter::Antialiasing, true);
        p->setPen(Qt::NoPen);

        QPainterPath outer;
        outer.addRoundedRect(box, radius, radius);
        if (border <= 0) {
            p->fillPath(outer, m_look.color);
            return;
        }
        // The border inside the box, as a Rectangle's; the colour inside it.
        QPainterPath inner;
        const qreal innerRadius = std::max<qreal>(0.0, radius - border);
        inner.addRoundedRect(box.adjusted(border, border, -border, -border), innerRadius, innerRadius);
        QPainterPath ring = outer;
        ring.addPath(inner);
        ring.setFillRule(Qt::OddEvenFill);
        p->fillPath(ring, m_look.borderColor);
        if (m_look.color.alpha() > 0)
            p->fillPath(inner, m_look.color);
    }

    StateFlags changedStates() const override { return {}; }
    RenderingFlags flags() const override { return BoundedRectRendering; }
    // Rounded to device pixels the box may reach half a pixel further out.
    QRectF rect() const override { return QRectF(-1, -1, m_look.width + 2, m_look.height + 2); }

private:
    QQuickItem* m_item;
    RoundBox::Look m_look;
};

} // namespace

RoundBox::RoundBox(QQuickItem* parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
    connect(this, &RoundBox::changed, this, &QQuickItem::update);
}

void RoundBox::setColor(const QColor& color)
{
    if (color == m_color)
        return;
    m_color = color;
    emit changed();
}

void RoundBox::setRadius(qreal radius)
{
    if (qFuzzyCompare(radius, m_radius))
        return;
    m_radius = radius;
    emit changed();
}

void RoundBox::setBorderWidth(qreal width)
{
    if (qFuzzyCompare(width, m_borderWidth))
        return;
    m_borderWidth = width;
    emit changed();
}

void RoundBox::setBorderColor(const QColor& color)
{
    if (color == m_borderColor)
        return;
    m_borderColor = color;
    emit changed();
}

void RoundBox::geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry)
{
    QQuickItem::geometryChange(newGeometry, oldGeometry);
    if (newGeometry.size() != oldGeometry.size())
        update();
}

QSGNode* RoundBox::updatePaintNode(QSGNode* old, UpdatePaintNodeData*)
{
    // Only the software renderer hands a QPainter to render nodes.
    const bool software = window() && window()->rendererInterface()->graphicsApi() == QSGRendererInterface::Software;
    if (!software || width() <= 0 || height() <= 0 || (m_color.alpha() == 0 && (m_borderWidth <= 0 || m_borderColor.alpha() == 0))) {
        delete old;
        return nullptr;
    }
    auto* node = static_cast<BoxNode*>(old);
    if (!node)
        node = new BoxNode(this);
    node->setLook({width(), height(), m_radius, m_borderWidth, m_color, m_borderColor});
    return node;
}
