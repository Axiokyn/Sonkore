#pragma once

#include <QCache>
#include <QImage>
#include <QMutex>
#include <QQuickImageProvider>

// image://cover/r<radius px>/<percent-encoded track path>
// Embedded artwork first (FLAC PICTURE / ID3 APIC), then cover.jpg & co. in
// the track's folder. The image is decoded straight at the requested size
// (JPEG scales inside the decoder), cropped square and given its rounded
// corners here, so QML shows a plain Image: no shader, no offscreen layer.
class CoverImageProvider : public QQuickImageProvider {
public:
    CoverImageProvider();
    QImage requestImage(const QString& id, QSize* size, const QSize& requestedSize) override;

private:
    QMutex m_mutex;
    QCache<QString, QImage> m_cache; // recently shown covers, by id and size
};
