#include "WindowFrame.hpp"

#include <QPainter>
#include <QPainterPath>
#include <QQuickWindow>
#include <QSGRenderNode>
#include <QSGRendererInterface>

#include <algorithm>
#include <array>
#include <cmath>

namespace {

enum class Side { Top, Bottom, Left, Right };

// One side of the frame: draws the whole frame, clipped to its strip.
class SideNode : public QSGRenderNode {
public:
    SideNode(QQuickItem* item, Side side)
        : m_item(item)
        , m_side(side)
    {
    }

    void setLook(const WindowFrame::Look& look)
    {
        m_look = look;
        // The strip, in the item's logical pixels: the top and bottom ones as
        // deep as the corners, the side ones between them as wide as the
        // rings. One extra pixel past the far edges, where a rounded-up
        // buffer may have a row or a column more.
        const qreal rings = look.lineWidth * static_cast<qreal>(look.rings.size());
        const qreal deep = std::ceil(std::max(look.radius, rings)) + 2.0;
        const qreal wide = std::ceil(rings) + 2.0;
        const qreal w = look.width, h = look.height;
        switch (m_side) {
        case Side::Top: m_rect = QRectF(0, 0, w + 1, deep); break;
        case Side::Bottom: m_rect = QRectF(0, h - deep, w + 1, deep + 1); break;
        case Side::Left: m_rect = QRectF(0, deep, wide, std::max<qreal>(0, h - 2 * deep)); break;
        case Side::Right: m_rect = QRectF(w - wide, deep, wide + 1, std::max<qreal>(0, h - 2 * deep)); break;
        }
        markDirty(QSGNode::DirtyMaterial);
    }

    void render(const RenderState* state) override
    {
        QQuickWindow* window = m_item->window();
        if (!window || m_rect.isEmpty())
            return;
        auto* p = static_cast<QPainter*>(window->rendererInterface()->getResource(window, QSGRendererInterface::PainterResource));
        if (!p || !p->device())
            return;

        // The region to repaint, in the window's logical pixels (as the
        // painter is now: set before the transform below).
        // (Nothing to repaint: none.)
        const QRegion* clip = state->clipRegion();
        if (!clip || clip->isEmpty())
            return;
        p->setClipRegion(*clip, Qt::ReplaceClip);

        // From here on in the buffer's own pixels.
        const qreal dpr = p->device()->devicePixelRatio();
        const QTransform toWindow = matrix()->toTransform();
        const QPointF origin = toWindow.map(QPointF(0, 0)) * dpr;
        p->setTransform(QTransform::fromScale(1.0 / dpr, 1.0 / dpr));
        p->translate(std::round(origin.x()), std::round(origin.y()));
        const QRectF strip = toWindow.mapRect(m_rect);
        p->setClipRect(QRectF(strip.topLeft() * dpr, strip.size() * dpr).translated(-std::round(origin.x()), -std::round(origin.y())),
                       Qt::IntersectClip);
        p->setOpacity(inheritedOpacity());
        p->setRenderHint(QPainter::Antialiasing, true);

        // The frame: from the window's first pixel to the last it surely
        // shows. The buffer holds the item's logical size × scale, rounded.
        const int w = WindowFrame::shownPixels(m_look.width * dpr, p->device()->width() - static_cast<int>(std::round(origin.x())));
        const int h = WindowFrame::shownPixels(m_look.height * dpr, p->device()->height() - static_cast<int>(std::round(origin.y())));
        if (w <= 0 || h <= 0)
            return;
        const qreal radius = m_look.radius * dpr;
        const auto rounded = [&](qreal inset) {
            QPainterPath path;
            const qreal r = std::max<qreal>(0.0, radius - inset);
            path.addRoundedRect(QRectF(inset, inset, w - 2 * inset, h - 2 * inset), r, r);
            return path;
        };

        if (m_look.clipCorners) {
            // Everything outside the outline's outer edge, the corners
            // included, cleared (anti-aliased along the arcs).
            QPainterPath outside;
            outside.addRect(QRectF(-1, -1, p->device()->width() + 2, p->device()->height() + 2));
            outside = outside.subtracted(rounded(0));
            p->setCompositionMode(QPainter::CompositionMode_DestinationOut);
            p->fillPath(outside, Qt::black);
            p->setCompositionMode(QPainter::CompositionMode_SourceOver);
        }

        // Each ring a whole number of device pixels wide, the next inside it.
        const qreal ring = std::max<qreal>(1.0, std::round(m_look.lineWidth * dpr));
        qreal inset = 0;
        for (const QColor& color : std::as_const(m_look.rings)) {
            if (w - 2 * (inset + ring) <= 0 || h - 2 * (inset + ring) <= 0)
                break;
            QPainterPath band = rounded(inset);
            band.addPath(rounded(inset + ring));
            band.setFillRule(Qt::OddEvenFill);
            p->fillPath(band, color);
            inset += ring;
        }
    }

    StateFlags changedStates() const override { return {}; }
    RenderingFlags flags() const override { return BoundedRectRendering; }
    QRectF rect() const override { return m_rect; }

private:
    QQuickItem* m_item;
    Side m_side;
    WindowFrame::Look m_look;
    QRectF m_rect;
};

} // namespace

WindowFrame::WindowFrame(QQuickItem* parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
    connect(this, &WindowFrame::changed, this, &QQuickItem::update);
}

void WindowFrame::setRadius(qreal radius)
{
    if (qFuzzyCompare(radius, m_radius))
        return;
    m_radius = radius;
    emit changed();
}

void WindowFrame::setLineWidth(qreal width)
{
    if (qFuzzyCompare(width, m_lineWidth))
        return;
    m_lineWidth = width;
    emit changed();
}

void WindowFrame::setColors(const QVariantList& colors)
{
    QList<QColor> rings;
    for (const QVariant& c : colors)
        rings.append(c.value<QColor>());
    if (rings == m_rings)
        return;
    m_rings = rings;
    m_colorList = colors;
    emit changed();
}

void WindowFrame::setClipCorners(bool on)
{
    if (on == m_clipCorners)
        return;
    m_clipCorners = on;
    emit changed();
}

int WindowFrame::shownPixels(double extent, int buffer)
{
    // A size × scale such as 933.33: the buffer has 933 rows and, placed at
    // a fractional position, the last one may fall outside what the
    // compositor shows. 933.6 rounds up to 934 rows and keeps 933 whole.
    const double whole = std::floor(extent + 0.01);
    const double part = extent - whole;
    const int shown = static_cast<int>(whole) - (part > 0.01 && part < 0.5 ? 1 : 0);
    return std::clamp(shown, 0, std::max(0, buffer));
}

void WindowFrame::geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry)
{
    QQuickItem::geometryChange(newGeometry, oldGeometry);
    if (newGeometry.size() != oldGeometry.size())
        update();
}

QSGNode* WindowFrame::updatePaintNode(QSGNode* old, UpdatePaintNodeData*)
{
    // Only the software renderer hands a QPainter to render nodes.
    const bool software = window() && window()->rendererInterface()->graphicsApi() == QSGRendererInterface::Software;
    if (!software || width() <= 0 || height() <= 0 || (m_rings.isEmpty() && !m_clipCorners)) {
        delete old;
        return nullptr;
    }
    QSGNode* root = old;
    if (!root) {
        root = new QSGNode;
        for (Side side : {Side::Top, Side::Bottom, Side::Left, Side::Right})
            root->appendChildNode(new SideNode(this, side));
    }
    const Look look{width(), height(), m_radius, m_lineWidth, m_rings, m_clipCorners};
    for (QSGNode* n = root->firstChild(); n; n = n->nextSibling())
        static_cast<SideNode*>(n)->setLook(look);
    return root;
}
