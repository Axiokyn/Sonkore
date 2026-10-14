#pragma once

#include <QColor>
#include <QList>
#include <QQuickPaintedItem>
#include <QString>
#include <QtQml/qqmlregistration.h>

#include <vector>

// The spectrum visualizer (and the small "playing" bars), drawn on the CPU in
// one picture with QPainter, as desktop players draw theirs (foobar2000,
// MusicBee, Strawberry, Audacious): anti-aliased shapes at fractional
// positions, so a bar grows by fractions of a pixel instead of jumping from
// one pixel row to the next, and every bar the same width on the device's
// pixel grid. Made of separate rectangles, the software renderer snaps each
// to whole pixels with no anti-aliasing (bars of 6 and 7 px, jagged ends,
// steps in the motion); the GPU renderers get the same picture.
//
// Styles: bars (rounded, with peak caps that hold and then fall), mirror
// (bars grown from the middle), dots (LED columns, the top one lit in part)
// and line (a smooth curve over a fill). The levels move towards `target`
// in advance(): fast attack, gentle release.
class SpectrumView : public QQuickPaintedItem {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QString style READ style WRITE setStyle NOTIFY styleChanged)
    Q_PROPERTY(QColor color READ color WRITE setColor NOTIFY colorChanged)
    // What the levels move towards (0..1 per band; DspController.spectrum).
    Q_PROPERTY(QList<qreal> target READ target WRITE setTarget NOTIFY targetChanged)
    // The levels drawn; set directly to draw fixed levels (previews, the
    // playing bars).
    Q_PROPERTY(QList<qreal> levels READ levels WRITE setLevels NOTIFY levelsChanged)
    Q_PROPERTY(bool peaks READ peaks WRITE setPeaks NOTIFY peaksChanged)          // peak caps (bars)
    Q_PROPERTY(bool shaded READ shaded WRITE setShaded NOTIFY shadedChanged)       // brighter towards the top
    Q_PROPERTY(qreal barFraction READ barFraction WRITE setBarFraction NOTIFY barFractionChanged) // bar width / step
    Q_PROPERTY(qreal minimumHeight READ minimumHeight WRITE setMinimumHeight NOTIFY minimumHeightChanged) // a silent bar
    // Nothing moves any more: the caller can stop calling advance().
    Q_PROPERTY(bool resting READ resting NOTIFY restingChanged)

public:
    explicit SpectrumView(QQuickItem* parent = nullptr);

    QString style() const { return m_style; }
    void setStyle(const QString& style);
    QColor color() const { return m_color; }
    void setColor(const QColor& color);
    QList<qreal> target() const { return m_target; }
    void setTarget(const QList<qreal>& target);
    QList<qreal> levels() const;
    void setLevels(const QList<qreal>& levels);
    bool peaks() const { return m_peaksOn; }
    void setPeaks(bool on);
    bool shaded() const { return m_shaded; }
    void setShaded(bool on);
    qreal barFraction() const { return m_barFraction; }
    void setBarFraction(qreal fraction);
    qreal minimumHeight() const { return m_minimumHeight; }
    void setMinimumHeight(qreal height);
    bool resting() const { return m_resting; }

    // Moves the levels `dt` seconds towards the target and repaints.
    Q_INVOKABLE void advance(double dt);

    void paint(QPainter* painter) override;

signals:
    void styleChanged();
    void colorChanged();
    void peaksChanged();
    void shadedChanged();
    void barFractionChanged();
    void minimumHeightChanged();
    void targetChanged();
    void levelsChanged();
    void restingChanged();

private:
    void resize(std::size_t bands);
    void updateResting();

    QString m_style = QStringLiteral("bars");
    QColor m_color = QColor(0x1d, 0xb9, 0x54);
    QList<qreal> m_target;
    std::vector<double> m_levels, m_peaks, m_peakHold, m_peakSpeed;
    bool m_peaksOn = false;
    bool m_shaded = true;
    qreal m_barFraction = 0.62;
    qreal m_minimumHeight = 2.0;
    bool m_resting = true;
};
