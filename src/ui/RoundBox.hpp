#pragma once

#include <QColor>
#include <QQuickItem>
#include <QtQml/qqmlregistration.h>

// A rounded box (colour, radius, border) for the software renderer, drawn
// with QPainter straight into the window's buffer: one anti-aliased path, its
// straight sides on whole device pixels. The software renderer draws a
// Rectangle's rounded corners as small pictures scaled into place, and at a
// fractional scale (125 %, 150 %, 166 %...) they come out stepped or
// speckled: a slider's round knob jagged, a square knob's border with dark
// dots in its corners. Used through RoundRect.qml, which draws a Rectangle
// with the GPU renderers; this item draws nothing there.
class RoundBox : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QColor color READ color WRITE setColor NOTIFY changed)
    Q_PROPERTY(qreal radius READ radius WRITE setRadius NOTIFY changed)
    Q_PROPERTY(qreal borderWidth READ borderWidth WRITE setBorderWidth NOTIFY changed)
    Q_PROPERTY(QColor borderColor READ borderColor WRITE setBorderColor NOTIFY changed)

public:
    explicit RoundBox(QQuickItem* parent = nullptr);

    QColor color() const { return m_color; }
    void setColor(const QColor& color);
    qreal radius() const { return m_radius; }
    void setRadius(qreal radius);
    qreal borderWidth() const { return m_borderWidth; }
    void setBorderWidth(qreal width);
    QColor borderColor() const { return m_borderColor; }
    void setBorderColor(const QColor& color);

    struct Look {
        qreal width = 0, height = 0, radius = 0, borderWidth = 0;
        QColor color, borderColor;
    };

signals:
    void changed();

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData* data) override;
    void geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry) override;

private:
    QColor m_color = Qt::white;
    qreal m_radius = 0, m_borderWidth = 0;
    QColor m_borderColor = Qt::transparent;
};
