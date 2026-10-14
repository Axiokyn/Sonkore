#include "SpectrumView.hpp"

#include <QLinearGradient>
#include <QPainter>
#include <QPainterPath>
#include <QQuickWindow>

#include <algorithm>
#include <cmath>

namespace {

constexpr double kRise = 30.0;      // per second: the attack, close to immediate
constexpr double kFall = 6.0;       // per second: the release
constexpr double kPeakHold = 0.4;   // seconds a peak cap waits before it falls
constexpr double kGravity = 2.5;    // its fall's acceleration, in heights per second²
constexpr double kRest = 0.002;     // below this nothing shows
constexpr int kDotRows = 6;

QColor withAlpha(QColor c, double alpha)
{
    c.setAlphaF(static_cast<float>(std::clamp(alpha, 0.0, 1.0) * c.alphaF()));
    return c;
}

} // namespace

SpectrumView::SpectrumView(QQuickItem* parent)
    : QQuickPaintedItem(parent)
{
    setAntialiasing(true);
    setOpaquePainting(false);
    // The picture is drawn at the device's resolution and on its pixel grid:
    // taken pixel for pixel. Smooth sampling would blur it by half a pixel
    // wherever the layout puts the item between two device pixels.
    setSmooth(false);
}

void SpectrumView::setStyle(const QString& style)
{
    if (style == m_style)
        return;
    m_style = style;
    emit styleChanged();
    update();
}

void SpectrumView::setColor(const QColor& color)
{
    if (color == m_color)
        return;
    m_color = color;
    emit colorChanged();
    update();
}

void SpectrumView::setPeaks(bool on)
{
    if (on == m_peaksOn)
        return;
    m_peaksOn = on;
    emit peaksChanged();
    update();
}

void SpectrumView::setShaded(bool on)
{
    if (on == m_shaded)
        return;
    m_shaded = on;
    emit shadedChanged();
    update();
}

void SpectrumView::setBarFraction(qreal fraction)
{
    fraction = std::clamp<qreal>(fraction, 0.05, 1.0);
    if (fraction == m_barFraction)
        return;
    m_barFraction = fraction;
    emit barFractionChanged();
    update();
}

void SpectrumView::setMinimumHeight(qreal height)
{
    if (height == m_minimumHeight)
        return;
    m_minimumHeight = std::max<qreal>(0.0, height);
    emit minimumHeightChanged();
    update();
}

void SpectrumView::resize(std::size_t bands)
{
    if (bands == m_levels.size())
        return;
    m_levels.resize(bands, 0.0);
    m_peaks.resize(bands, 0.0);
    m_peakHold.resize(bands, 0.0);
    m_peakSpeed.resize(bands, 0.0);
}

void SpectrumView::setTarget(const QList<qreal>& target)
{
    m_target = target;
    resize(static_cast<std::size_t>(target.size()));
    emit targetChanged();
    updateResting();
}

QList<qreal> SpectrumView::levels() const
{
    return QList<qreal>(m_levels.begin(), m_levels.end());
}

void SpectrumView::setLevels(const QList<qreal>& levels)
{
    resize(static_cast<std::size_t>(levels.size()));
    for (std::size_t i = 0; i < m_levels.size(); ++i) {
        m_levels[i] = std::clamp<double>(levels[static_cast<qsizetype>(i)], 0.0, 1.0);
        m_peaks[i] = m_levels[i];
    }
    emit levelsChanged();
    update();
}

void SpectrumView::advance(double dt)
{
    dt = std::clamp(dt, 0.0, 0.25); // a stalled frame does not make everything leap
    const double rise = std::min(1.0, dt * kRise), fall = std::min(1.0, dt * kFall);
    for (std::size_t i = 0; i < m_levels.size(); ++i) {
        const double t = i < static_cast<std::size_t>(m_target.size()) ? std::clamp<double>(m_target[static_cast<qsizetype>(i)], 0.0, 1.0) : 0.0;
        double& l = m_levels[i];
        l += (t - l) * (t > l ? rise : fall);
        // A cap rests on the highest level a moment, then falls faster and faster.
        double& peak = m_peaks[i];
        if (l >= peak) {
            peak = l;
            m_peakHold[i] = kPeakHold;
            m_peakSpeed[i] = 0.0;
        } else if (m_peakHold[i] > 0.0) {
            m_peakHold[i] -= dt;
        } else {
            m_peakSpeed[i] += kGravity * dt;
            peak = std::max(l, peak - m_peakSpeed[i] * dt);
        }
    }
    emit levelsChanged();
    update();
    updateResting();
}

void SpectrumView::updateResting()
{
    bool rest = std::all_of(m_target.begin(), m_target.end(), [](qreal t) { return t <= kRest; });
    for (std::size_t i = 0; rest && i < m_levels.size(); ++i)
        rest = m_levels[i] <= kRest && (!m_peaksOn || m_peaks[i] <= kRest);
    if (rest != m_resting) {
        m_resting = rest;
        emit restingChanged();
    }
}

void SpectrumView::paint(QPainter* p)
{
    const std::size_t n = m_levels.size();
    const double w = width(), h = height();
    if (n == 0 || w <= 0.0 || h <= 0.0)
        return;
    p->setRenderHint(QPainter::Antialiasing);
    p->setPen(Qt::NoPen);

    // Bars on the device's pixel grid: the same whole number of pixels wide
    // and starting on a pixel edge, so their sides are sharp and alike; their
    // heights stay fractional (anti-aliased tops), so they move smoothly.
    const double dpr = window() ? window()->effectiveDevicePixelRatio() : 1.0;
    const auto snap = [dpr](double v) { return std::round(v * dpr) / dpr; };
    const double step = w / static_cast<double>(n);
    const double barW = std::max(1.0, std::round(step * m_barFraction * dpr)) / dpr;
    const auto barX = [&](std::size_t i) { return snap(static_cast<double>(i) * step + (step - barW) / 2.0); };
    const QColor top = m_color.lighter(115);

    if (m_style == QLatin1String("line")) {
        // A smooth curve through the bands' tops (Catmull-Rom as Béziers,
        // kept inside the item), over a fill that fades downwards.
        std::vector<QPointF> pts(n);
        for (std::size_t i = 0; i < n; ++i)
            pts[i] = {(static_cast<double>(i) + 0.5) * step, h - 1.0 - m_levels[i] * (h - 2.0)};
        QPainterPath curve;
        curve.moveTo(pts[0]);
        for (std::size_t i = 0; i + 1 < n; ++i) {
            const QPointF& p0 = pts[i > 0 ? i - 1 : 0];
            const QPointF& p1 = pts[i];
            const QPointF& p2 = pts[i + 1];
            const QPointF& p3 = pts[std::min(i + 2, n - 1)];
            QPointF c1 = p1 + (p2 - p0) / 6.0, c2 = p2 - (p3 - p1) / 6.0;
            c1.setY(std::clamp(c1.y(), 0.5, h - 0.5));
            c2.setY(std::clamp(c2.y(), 0.5, h - 0.5));
            curve.cubicTo(c1, c2, p2);
        }
        QPainterPath area = curve;
        area.lineTo(pts.back().x(), h);
        area.lineTo(pts.front().x(), h);
        area.closeSubpath();
        QLinearGradient fade(0.0, 0.0, 0.0, h);
        fade.setColorAt(0.0, withAlpha(m_color, 0.32));
        fade.setColorAt(1.0, withAlpha(m_color, 0.04));
        p->fillPath(area, fade);
        QPen pen(m_color, 1.6);
        pen.setCapStyle(Qt::RoundCap);
        pen.setJoinStyle(Qt::RoundJoin);
        p->strokePath(curve, pen);
        return;
    }

    if (m_style == QLatin1String("dots")) {
        // LED columns: the top lit dot glows in proportion, so the column
        // rises smoothly instead of a dot at a time.
        const double d = std::min(barW, h / (kDotRows + (kDotRows - 1) * 0.25));
        const double gap = kDotRows > 1 ? (h - kDotRows * d) / (kDotRows - 1) : 0.0;
        for (std::size_t i = 0; i < n; ++i) {
            const double x = barX(i) + (barW - d) / 2.0;
            for (int r = 0; r < kDotRows; ++r) {
                const double lit = std::clamp(m_levels[i] * kDotRows - r, 0.0, 1.0);
                p->setBrush(withAlpha(r == kDotRows - 1 && lit > 0 ? top : m_color, 0.14 + 0.86 * lit));
                p->drawEllipse(QRectF(x, snap(h - d - r * (d + gap)), d, d));
            }
        }
        return;
    }

    const bool mirror = m_style == QLatin1String("mirror");
    const bool caps = m_peaksOn && !mirror;
    const double capH = caps ? std::max(1.5, barW * 0.32) : 0.0, capGap = caps ? 1.5 : 0.0;
    const double room = h - capH - capGap; // the bars leave room for their caps
    QPainterPath bars, peaks;
    for (std::size_t i = 0; i < n; ++i) {
        const double x = barX(i);
        const double bh = std::clamp(m_levels[i] * room, std::min(m_minimumHeight, room), room);
        const double y = mirror ? (h - bh) / 2.0 : h - bh;
        const double r = std::min(barW, bh) / 2.0;
        bars.addRoundedRect(QRectF(x, y, barW, bh), r, r);
        if (caps && m_peaks[i] > kRest) {
            const double py = std::max(0.0, h - std::max(m_peaks[i] * room, bh) - capGap - capH);
            peaks.addRoundedRect(QRectF(x, py, barW, capH), capH / 2.0, capH / 2.0);
        }
    }
    if (m_shaded) {
        // Brighter towards the top (from the middle for the mirror), as a
        // level meter: quiet bands are dim, loud ones bright at the tip.
        QLinearGradient shade(0.0, 0.0, 0.0, h);
        if (mirror) {
            shade.setColorAt(0.0, top);
            shade.setColorAt(0.5, withAlpha(m_color, 0.55));
            shade.setColorAt(1.0, top);
        } else {
            shade.setColorAt(0.0, top);
            shade.setColorAt(1.0, withAlpha(m_color, 0.5));
        }
        p->fillPath(bars, shade);
    } else {
        p->fillPath(bars, m_color);
    }
    if (caps)
        p->fillPath(peaks, withAlpha(top, 0.9));
}
