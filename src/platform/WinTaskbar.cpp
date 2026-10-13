#include "WinTaskbar.hpp"

#include "DspController.hpp"

#include <QCoreApplication>
#include <QWindow>

#if defined(Q_OS_WIN)
#include <QIcon>
#include <QImage>
#include <QPainter>
#include <QSvgRenderer>

#include <shobjidl.h>
#include <windows.h>

namespace {

enum ButtonId : UINT { PrevId = 1, PlayId = 2, NextId = 3 };

// Renders one of the app's monochrome SVG icons in white for the taskbar.
HICON makeIcon(const QString& name)
{
    QImage img(32, 32, QImage::Format_ARGB32_Premultiplied);
    img.fill(Qt::transparent);
    QSvgRenderer svg(QStringLiteral(":/qt/qml/Sonkore/qml/icons/%1.svg").arg(name));
    QPainter p(&img);
    svg.render(&p, QRectF(6, 6, 20, 20));
    p.setCompositionMode(QPainter::CompositionMode_SourceIn);
    p.fillRect(img.rect(), Qt::white);
    p.end();
    return img.toHICON();
}

} // namespace

struct WinTaskbar::Impl {
    HWND hwnd = nullptr;
    ITaskbarList3* taskbar = nullptr;
    UINT taskbarCreatedMsg = 0;
    bool buttonsAdded = false;
    HICON prev = nullptr, play = nullptr, pause = nullptr, next = nullptr;
};

WinTaskbar::WinTaskbar(DspController* player, QObject* parent)
    : QObject(parent)
    , m_player(player)
    , d(new Impl)
{
    d->taskbarCreatedMsg = RegisterWindowMessageW(L"TaskbarButtonCreated");
    d->prev = makeIcon(QStringLiteral("prev"));
    d->play = makeIcon(QStringLiteral("play"));
    d->pause = makeIcon(QStringLiteral("pause"));
    d->next = makeIcon(QStringLiteral("next"));
    QCoreApplication::instance()->installNativeEventFilter(this);
    connect(player, &DspController::stateChanged, this, [this] { updatePlayButton(); updateProgress(); });
    connect(player, &DspController::positionChanged, this, &WinTaskbar::updateProgress);
}

WinTaskbar::~WinTaskbar()
{
    QCoreApplication::instance()->removeNativeEventFilter(this);
    if (d->taskbar)
        d->taskbar->Release();
    for (HICON icon : {d->prev, d->play, d->pause, d->next})
        if (icon)
            DestroyIcon(icon);
    delete d;
}

void WinTaskbar::attach(QWindow* window)
{
    d->hwnd = reinterpret_cast<HWND>(window->winId());
}

void WinTaskbar::createButtons()
{
    if (!d->hwnd)
        return;
    if (!d->taskbar) {
        if (FAILED(CoCreateInstance(CLSID_TaskbarList, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&d->taskbar))))
            return;
        d->taskbar->HrInit();
    }
    THUMBBUTTON buttons[3] = {};
    const UINT ids[3] = {PrevId, PlayId, NextId};
    const HICON icons[3] = {d->prev, m_player->playing() ? d->pause : d->play, d->next};
    const wchar_t* tips[3] = {L"Previous", L"Play / Pause", L"Next"};
    for (int i = 0; i < 3; ++i) {
        buttons[i].dwMask = static_cast<THUMBBUTTONMASK>(THB_ICON | THB_TOOLTIP | THB_FLAGS);
        buttons[i].iId = ids[i];
        buttons[i].hIcon = icons[i];
        buttons[i].dwFlags = THBF_ENABLED;
        wcsncpy_s(buttons[i].szTip, tips[i], _TRUNCATE);
    }
    if (d->buttonsAdded)
        d->taskbar->ThumbBarUpdateButtons(d->hwnd, 3, buttons);
    else
        d->buttonsAdded = SUCCEEDED(d->taskbar->ThumbBarAddButtons(d->hwnd, 3, buttons));
}

void WinTaskbar::updatePlayButton()
{
    if (!d->taskbar || !d->buttonsAdded)
        return;
    THUMBBUTTON b = {};
    b.dwMask = static_cast<THUMBBUTTONMASK>(THB_ICON | THB_FLAGS);
    b.iId = PlayId;
    b.hIcon = m_player->playing() ? d->pause : d->play;
    b.dwFlags = THBF_ENABLED;
    d->taskbar->ThumbBarUpdateButtons(d->hwnd, 1, &b);
}

void WinTaskbar::updateProgress()
{
    if (!d->taskbar)
        return;
    const double duration = m_player->duration();
    if (!m_player->loaded() || duration <= 0) {
        d->taskbar->SetProgressState(d->hwnd, TBPF_NOPROGRESS);
        return;
    }
    d->taskbar->SetProgressState(d->hwnd, m_player->playing() ? TBPF_NORMAL : TBPF_PAUSED);
    d->taskbar->SetProgressValue(d->hwnd, static_cast<ULONGLONG>(m_player->position() * 1000),
                                 static_cast<ULONGLONG>(duration * 1000));
}

bool WinTaskbar::nativeEventFilter(const QByteArray& eventType, void* message, qintptr*)
{
    if (eventType != "windows_generic_MSG")
        return false;
    const MSG* msg = static_cast<const MSG*>(message);
    if (!d->hwnd || msg->hwnd != d->hwnd)
        return false;
    if (msg->message == d->taskbarCreatedMsg) {
        d->buttonsAdded = false;
        createButtons();
    } else if (msg->message == WM_COMMAND && HIWORD(msg->wParam) == THBN_CLICKED) {
        switch (LOWORD(msg->wParam)) {
        case PrevId: m_player->previous(); break;
        case PlayId: m_player->togglePlayPause(); break;
        case NextId: m_player->next(); break;
        default: return false;
        }
        return true;
    }
    return false;
}

#else // !Q_OS_WIN

struct WinTaskbar::Impl {};

WinTaskbar::WinTaskbar(DspController* player, QObject* parent)
    : QObject(parent)
    , m_player(player)
{
}
WinTaskbar::~WinTaskbar() = default;
void WinTaskbar::attach(QWindow*) {}
bool WinTaskbar::nativeEventFilter(const QByteArray&, void*, qintptr*) { return false; }
void WinTaskbar::createButtons() {}
void WinTaskbar::updatePlayButton() {}
void WinTaskbar::updateProgress() {}

#endif
