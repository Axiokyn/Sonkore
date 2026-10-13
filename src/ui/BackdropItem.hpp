#pragma once

#include <QColor>
#include <QImage>
#include <QQuickPaintedItem>
#include <QVector2D>
#include <QVector4D>
#include <QtQml/qqmlregistration.h>

// The window backdrop (Settings › Appearance › Background) drawn on the CPU,
// for the software renderer, which runs no shaders: the same picture as
// qml/shaders/backdrop.frag (the skin's colour with up to three soft radial
// glows, or a diagonal three-stop wash; float maths, a triangular dither of
// one 8-bit step against banding, anti-aliased rounded corners). Computed
// only when an input or the size changes; the item keeps the result.
class BackdropItem : public QQuickPaintedItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(qreal radius MEMBER m_radius NOTIFY changed)
    Q_PROPERTY(qreal linearMode MEMBER m_linearMode NOTIFY changed)
    Q_PROPERTY(QVector4D bg MEMBER m_bg NOTIFY changed)
    Q_PROPERTY(QVector4D tone1 MEMBER m_tone1 NOTIFY changed)
    Q_PROPERTY(QVector4D tone2 MEMBER m_tone2 NOTIFY changed)
    Q_PROPERTY(QVector4D tone3 MEMBER m_tone3 NOTIFY changed)
    Q_PROPERTY(QVector4D blob1 MEMBER m_blob1 NOTIFY changed)
    Q_PROPERTY(QVector4D blob2 MEMBER m_blob2 NOTIFY changed)
    Q_PROPERTY(QVector4D blob3 MEMBER m_blob3 NOTIFY changed)

public:
    explicit BackdropItem(QQuickItem* parent = nullptr);
    void paint(QPainter* painter) override;

    // The picture for a `width` x `height` pixel area (also used by tests).
    struct Params {
        double radius = 0, linearMode = 0;
        QVector4D bg, tone1, tone2, tone3, blob1, blob2, blob3;
    };
    static QImage render(int width, int height, double scale, const Params& p);

signals:
    void changed();

private:
    Params params() const;

    qreal m_radius = 0, m_linearMode = 0;
    QVector4D m_bg, m_tone1, m_tone2, m_tone3, m_blob1, m_blob2, m_blob3;
};
