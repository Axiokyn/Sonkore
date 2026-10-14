#include "RoundBox.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QQuickWindow>
#include <QSGRenderNode>
#include <QSGRendererInterface>

#include <algorithm>
#include <cmath>

namespace {

// A rounded rectangle with a radius per corner (each at most half the
// shorter side).
QPainterPath roundedPath(const QRectF& r, qreal tl, qreal tr, qreal br, qreal bl)
{
    const qreal most = std::min(r.width(), r.height()) / 2;
    tl = std::clamp<qreal>(tl, 0, most), tr = std::clamp<qreal>(tr, 0, most);
    br = std::clamp<qreal>(br, 0, most), bl = std::clamp<qreal>(bl, 0, most);
    QPainterPath path;
    if (tl == tr && tr == br && br == bl) {
        path.addRoundedRect(r, tl, tl);
        return path;
    }
    path.moveTo(r.left() + tl, r.top());
    path.lineTo(r.right() - tr, r.top());
    if (tr > 0)
        path.arcTo(r.right() - 2 * tr, r.top(), 2 * tr, 2 * tr, 90, -90);
    path.lineTo(r.right(), r.bottom() - br);
    if (br > 0)
        path.arcTo(r.right() - 2 * br, r.bottom() - 2 * br, 2 * br, 2 * br, 0, -90);
    path.lineTo(r.left() + bl, r.bottom());
    if (bl > 0)
        path.arcTo(r.left(), r.bottom() - 2 * bl, 2 * bl, 2 * bl, 270, -90);
    path.lineTo(r.left(), r.top() + tl);
    if (tl > 0)
        path.arcTo(r.left(), r.top(), 2 * tl, 2 * tl, 180, -90);
    path.closeSubpath();
    return path;
}

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
        // (Nothing to repaint: none.)
        const QRegion* clip = state->clipRegion();
        if (!clip || clip->isEmpty())
            return;
        p->setClipRegion(*clip, Qt::ReplaceClip);

        // The box on the buffer's own pixels: its edges rounded to whole
        // device pixels, so straight sides are sharp and alike.
        const qreal dpr = p->device()->devicePixelRatio();
        const QRectF r = matrix()->toTransform().mapRect(QRectF(0, 0, m_look.width, m_look.height));
        const qreal x0 = std::round(r.left() * dpr), y0 = std::round(r.top() * dpr);
        const qreal x1 = std::max(x0 + 1, std::round(r.right() * dpr)), y1 = std::max(y0 + 1, std::round(r.bottom() * dpr));
        const QRectF box(x0, y0, x1 - x0, y1 - y0);
        const qreal scale = r.width() > 0 ? box.width() / r.width() : dpr; // logical px → device px, item scale included
        const qreal tl = m_look.topLeft * scale, tr = m_look.topRight * scale;
        const qreal br = m_look.bottomRight * scale, bl = m_look.bottomLeft * scale;
        const qreal border = m_look.borderWidth > 0 && m_look.borderColor.alpha() > 0
                                 ? std::min(std::max<qreal>(1.0, std::round(m_look.borderWidth * scale)), std::min(box.width(), box.height()) / 2)
                                 : 0.0;

        p->setTransform(QTransform::fromScale(1.0 / dpr, 1.0 / dpr));
        p->setOpacity(inheritedOpacity());
        p->setRenderHint(QPainter::Antialiasing, true);
        p->setPen(Qt::NoPen);

        const QPainterPath outer = roundedPath(box, tl, tr, br, bl);
        if (border <= 0) {
            p->fillPath(outer, m_look.color);
            return;
        }
        // The border inside the box, as a Rectangle's; the colour inside it.
        const auto in = [border](qreal radius) { return std::max<qreal>(0.0, radius - border); };
        const QPainterPath inner = roundedPath(box.adjusted(border, border, -border, -border), in(tl), in(tr), in(br), in(bl));
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

void RoundBox::setCorner(qreal& corner, qreal radius)
{
    if (qFuzzyCompare(radius, corner))
        return;
    corner = radius;
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
    const auto corner = [this](qreal r) { return r < 0 ? m_radius : r; };
    node->setLook({width(), height(), m_borderWidth, corner(m_topLeft), corner(m_topRight), corner(m_bottomRight),
                   corner(m_bottomLeft), m_color, m_borderColor});
    return node;
}
