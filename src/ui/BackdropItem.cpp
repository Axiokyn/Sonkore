#include "BackdropItem.hpp"

#include <QPainter>
#include <QQuickWindow>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <vector>

namespace {

// The shader's hash (qml/shaders/backdrop.frag), for the same dither.
inline float hash(float x, float y)
{
    float a = x * 0.1031f, b = y * 0.1031f, c = x * 0.1031f;
    a -= std::floor(a);
    b -= std::floor(b);
    c -= std::floor(c);
    const float d = a * (b + 33.33f) + b * (c + 33.33f) + c * (a + 33.33f);
    a += d;
    b += d;
    c += d;
    const float r = (a + b) * c;
    return r - std::floor(r);
}

inline float mixf(float a, float b, float t) { return a + (b - a) * t; }

} // namespace

BackdropItem::BackdropItem(QQuickItem* parent)
    : QQuickPaintedItem(parent)
{
    setOpaquePainting(false);
    connect(this, &BackdropItem::changed, this, [this] { update(); });
}

BackdropItem::Params BackdropItem::params() const
{
    return {m_radius, m_linearMode, m_bg, m_tone1, m_tone2, m_tone3, m_blob1, m_blob2, m_blob3};
}

QImage BackdropItem::render(int width, int height, double scale, const Params& p)
{
    QImage image(width, height, QImage::Format_ARGB32_Premultiplied);
    if (width <= 0 || height <= 0)
        return image;
    const float s = static_cast<float>(scale);
    const float areaX = width / s, areaY = height / s; // item units, as the shader's `area`
    const float radius = static_cast<float>(p.radius);
    const QVector4D tones[3] = {p.tone1, p.tone2, p.tone3};
    const QVector4D blobs[3] = {p.blob1, p.blob2, p.blob3};
    const bool linear = p.linearMode > 0.5;
    const float areaDot = std::max(areaX * areaX + areaY * areaY, 1.0f);

    // The colour field is smooth (cones and a ramp): computed every kStep
    // pixels and interpolated, it differs from the exact one by well under
    // one 8-bit step, at a sixtieth of the cost (a 4K window: ~15 ms).
    constexpr int kStep = 8;
    const int gw = width / kStep + 2, gh = height / kStep + 2;
    std::vector<float> grid(static_cast<std::size_t>(gw) * gh * 3);
    for (int gy = 0; gy < gh; ++gy)
        for (int gx = 0; gx < gw; ++gx) {
            const float px = (gx * kStep + 0.5f) / s, py = (gy * kStep + 0.5f) / s;
            float r = p.bg.x(), g = p.bg.y(), b = p.bg.z();
            if (linear) {
                const float t = std::clamp((px * areaX + py * areaY) / areaDot, 0.0f, 1.0f);
                const QVector4D st = t < 0.55f ? tones[0] + (tones[1] - tones[0]) * (t / 0.55f)
                                               : tones[1] + (tones[2] - tones[1]) * ((t - 0.55f) / 0.45f);
                r = mixf(r, st.x(), st.w());
                g = mixf(g, st.y(), st.w());
                b = mixf(b, st.z(), st.w());
            } else {
                for (int i = 0; i < 3; ++i) {
                    if (tones[i].w() <= 0.0f)
                        continue;
                    const float dx = px - blobs[i].x(), dy = py - blobs[i].y();
                    const float t = std::clamp(std::sqrt(dx * dx + dy * dy) / std::max(blobs[i].z(), 1.0f), 0.0f, 1.0f);
                    const float k = tones[i].w() * (1.0f - t);
                    r = mixf(r, tones[i].x(), k);
                    g = mixf(g, tones[i].y(), k);
                    b = mixf(b, tones[i].z(), k);
                }
            }
            float* cell = &grid[(static_cast<std::size_t>(gy) * gw + gx) * 3];
            cell[0] = r;
            cell[1] = g;
            cell[2] = b;
        }

    // Triangular dither of about one 8-bit step (the shader's hash), from a
    // tile: at 1/255 a repeat every 256 pixels is invisible.
    // Fixed point from here on: 8-bit levels with 16 fraction bits.
    static const std::vector<int32_t> dither = [] {
        std::vector<int32_t> d(256 * 256);
        for (int y = 0; y < 256; ++y)
            for (int x = 0; x < 256; ++x) {
                const float fx = x + 0.5f, fy = y + 0.5f;
                const float n = hash(fx, fy) + hash(fx + 17.17f, fy + 17.17f) - 1.0f; // -1..1 levels
                d[static_cast<std::size_t>(y) * 256 + x] = static_cast<int32_t>(std::lround(n * 65536.0f));
            }
        return d;
    }();

    const float cornerBand = radius * s + 2.0f; // pixels from an edge where a corner can cut
    constexpr float kFixed = 255.0f * 65536.0f;
    std::vector<int32_t> row(static_cast<std::size_t>(gw) * 3);
    for (int y = 0; y < height; ++y) {
        const int gy = y / kStep;
        const float fy = static_cast<float>(y % kStep) / kStep;
        const float* a = &grid[static_cast<std::size_t>(gy) * gw * 3];
        const float* b = a + static_cast<std::size_t>(gw) * 3;
        for (int i = 0; i < gw * 3; ++i)
            row[i] = static_cast<int32_t>((a[i] + (b[i] - a[i]) * fy) * kFixed);
        const int32_t* noise = &dither[static_cast<std::size_t>(y & 255) * 256];
        const bool nearRow = y < cornerBand || y >= height - cornerBand;
        auto* line = reinterpret_cast<QRgb*>(image.scanLine(y));
        const auto level = [](int32_t v) { return static_cast<uint32_t>(std::clamp((v + 32768) >> 16, 0, 255)); };
        for (int x0 = 0; x0 < width; x0 += kStep) {
            // One cell: the colour steps evenly from this grid point to the next.
            const int32_t* c0 = &row[static_cast<std::size_t>(x0 / kStep) * 3];
            const int32_t dr = (c0[3] - c0[0]) / kStep, dg = (c0[4] - c0[1]) / kStep, db = (c0[5] - c0[2]) / kStep;
            const int x1 = std::min(width, x0 + kStep);
            if (nearRow && (x0 < cornerBand || x1 > width - cornerBand)) {
                for (int x = x0; x < x1; ++x) {
                    // Rounded window corners, anti-aliased (the shader's distance).
                    const int k = x - x0;
                    const float px = (x + 0.5f) / s, py = (y + 0.5f) / s;
                    const float qx = std::abs(px - 0.5f * areaX) - (0.5f * areaX - radius);
                    const float qy = std::abs(py - 0.5f * areaY) - (0.5f * areaY - radius);
                    const float d = std::hypot(std::max(qx, 0.0f), std::max(qy, 0.0f)) + std::min(std::max(qx, qy), 0.0f) - radius;
                    const float mask = std::clamp(0.5f - d * s, 0.0f, 1.0f);
                    const int32_t n = noise[x & 255];
                    const auto masked = [mask](uint32_t v) { return static_cast<int>(v * mask + 0.5f); };
                    line[x] = qRgba(masked(level(c0[0] + dr * k + n)), masked(level(c0[1] + dg * k + n)),
                                    masked(level(c0[2] + db * k + n)), static_cast<int>(mask * 255.0f + 0.5f));
                }
                continue;
            }
            int32_t r = c0[0], g = c0[1], bl = c0[2];
            for (int x = x0; x < x1; ++x, r += dr, g += dg, bl += db) {
                const int32_t n = noise[x & 255];
                line[x] = 0xFF000000u | level(r + n) << 16 | level(g + n) << 8 | level(bl + n);
            }
        }
    }
    return image;
}

void BackdropItem::paint(QPainter* painter)
{
    // Called only when an input or the size changed: the item keeps what was
    // painted, so the picture is not held a second time here.
    const double scale = window() ? window()->effectiveDevicePixelRatio() : 1.0;
    QImage image = render(static_cast<int>(std::ceil(width() * scale)), static_cast<int>(std::ceil(height() * scale)), scale, params());
    image.setDevicePixelRatio(scale);
    painter->drawImage(QPointF(0, 0), image);
}
