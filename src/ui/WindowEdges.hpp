#pragma once

#include <QQuickItem>
#include <QtQml/qqmlregistration.h>

// The last rows and columns of a window's buffer that the software renderer
// never paints. It takes the window as QRect(0, 0, buffer / scale): at a
// fractional scale the division is truncated (1598 rows at 166 % become 958
// logical rows, 1596.7 device rows), and the buffer's last row or two stay as
// they were, transparent: on a maximized window the desktop showed through a
// line along the bottom (and the right). This item copies the last painted
// row and column over them, straight into the buffer, each time the edge is
// repainted. Square windows only (maximized, or without rounded corners: a
// rounded one has its edge drawn by WindowFrame). Draws nothing with the GPU
// renderers, which clear and draw the whole buffer.
class WindowEdges : public QQuickItem {
    Q_OBJECT
    QML_ELEMENT

public:
    explicit WindowEdges(QQuickItem* parent = nullptr);

protected:
    QSGNode* updatePaintNode(QSGNode* old, UpdatePaintNodeData* data) override;
    void geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry) override;
};
