#pragma once

#include <QAbstractNativeEventFilter>
#include <QObject>

class DspController;
class QWindow;

// Windows taskbar integration: previous / play-pause / next buttons in the
// window's thumbnail preview, and the track progress on the taskbar button.
// A no-op on other platforms.
class WinTaskbar : public QObject, public QAbstractNativeEventFilter {
    Q_OBJECT
public:
    WinTaskbar(DspController* player, QObject* parent = nullptr);
    ~WinTaskbar() override;

    void attach(QWindow* window);
    bool nativeEventFilter(const QByteArray& eventType, void* message, qintptr* result) override;

private:
    void createButtons();
    void updatePlayButton();
    void updateProgress();

    DspController* m_player;
    struct Impl;
    Impl* d = nullptr;
};
