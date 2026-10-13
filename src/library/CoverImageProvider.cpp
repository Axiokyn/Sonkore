#include "CoverImageProvider.hpp"

#include "Metadata.hpp"

#include <QBuffer>
#include <QFile>
#include <QFileInfo>
#include <QImageReader>
#include <QPainter>
#include <QPainterPath>
#include <QUrl>

#include <algorithm>
#include <cmath>

namespace {

// The image format from the first bytes, so the reader opens that one
// decoder instead of asking every image plugin in turn (its default even
// when a format is given). Empty: let it detect.
QByteArray formatOf(QIODevice* device)
{
    if (!device)
        return {};
    const QByteArray head = device->peek(16);
    if (head.startsWith("\xFF\xD8\xFF"))
        return "jpeg";
    if (head.startsWith("\x89PNG\r\n\x1a\n"))
        return "png";
    if (head.startsWith("GIF8"))
        return "gif";
    if (head.startsWith("BM"))
        return "bmp";
    if (head.startsWith("RIFF") && head.mid(8, 4) == "WEBP")
        return "webp";
    return {};
}

// Decodes `reader` so that its shorter side is at least `side` pixels,
// without decoding the full-size picture when the format can scale.
QImage readAtLeast(QImageReader& reader, int side)
{
    if (reader.format().isEmpty())
        if (const QByteArray format = formatOf(reader.device()); !format.isEmpty()) {
            reader.setFormat(format);
            reader.setAutoDetectImageFormat(false);
        }
    reader.setAutoTransform(true);
    const QSize full = reader.size();
    if (full.isValid() && std::min(full.width(), full.height()) > side) {
        const double scale = static_cast<double>(side) / std::min(full.width(), full.height());
        reader.setScaledSize(QSize(std::max(side, static_cast<int>(std::ceil(full.width() * scale))),
                                   std::max(side, static_cast<int>(std::ceil(full.height() * scale)))));
    }
    return reader.read();
}

QImage loadSource(const QString& path, int side)
{
    QByteArray embedded = sonkore::Metadata::embeddedCover(path);
    if (!embedded.isEmpty()) {
        QBuffer buffer(&embedded);
        buffer.open(QIODevice::ReadOnly);
        QImageReader reader(&buffer);
        const QImage image = readAtLeast(reader, side);
        if (!image.isNull())
            return image;
    }
    const QString folderImage = sonkore::Metadata::folderCover(QFileInfo(path).absolutePath());
    if (folderImage.isEmpty())
        return {};
    QFile file(folderImage);
    if (!file.open(QIODevice::ReadOnly))
        return {};
    QImageReader reader(&file);
    return readAtLeast(reader, side);
}

// Centre square of `source` at side x side with rounded corners.
QImage squareCover(const QImage& source, int side, int radius)
{
    const QImage scaled = source.scaled(side, side, Qt::KeepAspectRatioByExpanding, Qt::SmoothTransformation);
    const QImage square = scaled.copy((scaled.width() - side) / 2, (scaled.height() - side) / 2, side, side);
    if (radius <= 0)
        return square.convertToFormat(QImage::Format_ARGB32_Premultiplied);

    QImage result(side, side, QImage::Format_ARGB32_Premultiplied);
    result.fill(Qt::transparent);
    QPainter painter(&result);
    painter.setRenderHint(QPainter::Antialiasing);
    painter.setPen(Qt::NoPen);
    painter.setBrush(QBrush(square));
    painter.drawRoundedRect(QRectF(0, 0, side, side), radius, radius);
    return result;
}

} // namespace

CoverImageProvider::CoverImageProvider()
    : QQuickImageProvider(QQuickImageProvider::Image, QQmlImageProviderBase::ForceAsynchronousImageLoading)
{
    m_cache.setMaxCost(16 * 1024 * 1024); // bytes
}

QImage CoverImageProvider::requestImage(const QString& id, QSize* size, const QSize& requestedSize)
{
    // "r<radius>/<encoded path>"; a bare encoded path means square corners.
    int radius = 0;
    QString encoded = id;
    if (id.startsWith(u'r')) {
        const qsizetype slash = id.indexOf(u'/');
        bool ok = false;
        const int r = slash > 1 ? id.mid(1, slash - 1).toInt(&ok) : 0;
        if (ok) {
            radius = r;
            encoded = id.mid(slash + 1);
        }
    }
    const QString path = QUrl::fromPercentEncoding(encoded.toUtf8());
    const int side = std::clamp(requestedSize.isValid() ? std::max(requestedSize.width(), requestedSize.height()) : 512, 8, 2048);
    radius = std::clamp(radius, 0, side / 2);
    const QString key = path + u'@' + QString::number(side) + u'r' + QString::number(radius);

    {
        QMutexLocker lock(&m_mutex);
        if (const QImage* hit = m_cache.object(key)) {
            if (size)
                *size = hit->size();
            return *hit;
        }
    }

    const QImage source = loadSource(path, side);
    QImage result;
    if (source.isNull()) {
        result = QImage(1, 1, QImage::Format_ARGB32_Premultiplied); // QML shows its placeholder
        result.fill(Qt::transparent);
    } else {
        result = squareCover(source, side, radius);
    }

    {
        QMutexLocker lock(&m_mutex);
        m_cache.insert(key, new QImage(result), std::max<qsizetype>(1, result.sizeInBytes()));
    }
    if (size)
        *size = result.size();
    return result;
}
