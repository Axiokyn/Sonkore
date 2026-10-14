#pragma once

#include <QColor>
#include <QList>
#include <QQuickItem>
#include <QVariantList>
#include <QtQml/qqmlregistration.h>

// The edge of a borderless window for the software renderer: its outline
// (Settings › Appearance › Window outline) and the cut-out of its rounded
// corners, drawn with QPainter straight into the window's buffer, on its
// device pixels.
//
// Made of Rectangles, the software renderer draws a rounded border as straight
// edges filled at whole pixels plus corner pictures blitted on their own; at a
// fractional scale (125 %, 150 %, 166 %...) the two round differently, and the
// bottom and right edges step away from their corners. And there the window is
// not a whole number of device pixels: the buffer is rounded and the
// compositor may drop its last, partial row or column, so an edge drawn on it
// disappears. Here each ring of the outline is one anti-aliased path, its
// straight sides on whole device pixels, inside the pixels the window surely
// shows; outside it the corners are cleared, anti-aliased.
//
// Drawn by four render nodes, one per side (the top and bottom with their
// corners), so a change elsewhere in the window redraws none of them.
// The GPU renderers draw the Rectangles instead (WindowOutline.qml).
class WindowFrame : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(qreal radius READ radius WRITE setRadius NOTIFY changed)
    // Width of each ring, in logical pixels (rounded to whole device pixels).
    Q_PROPERTY(qreal lineWidth READ lineWidth WRITE setLineWidth NOTIFY changed)
    // The rings' colours, from the edge inwards; none for no outline.
    Q_PROPERTY(QVariantList colors READ colors WRITE setColors NOTIFY changed)
    // Clears what lies outside the rounded corners (a window with an alpha
    // channel), anti-aliased.
    Q_PROPERTY(bool clipCorners READ clipCorners WRITE setClipCorners NOTIFY changed)

public:
    explicit WindowFrame(QQuickItem* parent = nullptr);

    qreal radius() const { return m_radius; }
    void setRadius(qreal radius);
    qreal lineWidth() const { return m_lineWidth; }
    void setLineWidth(qreal width);
    QVariantList colors() const { return m_colorList; }
    void setColors(const QVariantList& colors);
    bool clipCorners() const { return m_clipCorners; }
    void setClipCorners(bool on);

    // What the nodes draw, in logical pixels of the item.
    struct Look {
        qreal width = 0, height = 0, radius = 0, lineWidth = 1;
        QList<QColor> rings;
        bool clipCorners = false;
    };

    // The pixels the window surely shows along one side: `extent` device
    // pixels (logical size × scale) in a buffer of `buffer` pixels. A partial
    // last pixel of less than a half may not be shown at all.
    static int shownPixels(double extent, int buffer);

signals:
    void changed();

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData* data) override;
    void geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry) override;

private:
    qreal m_radius = 0, m_lineWidth = 1;
    QVariantList m_colorList;
    QList<QColor> m_rings;
    bool m_clipCorners = false;
};
