#include "WinEndpointVolume.hpp"

#include <QMetaObject>

#include <algorithm>
#include <atomic>
#include <cmath>

#include <windows.h>
#include <initguid.h> // defines PKEY_Device_FriendlyName here
#include <mmdeviceapi.h>
#include <endpointvolume.h>
#include <propsys.h>
#include <functiondiscoverykeys_devpkey.h>

namespace {

// Tags Sonkore's own changes, so their notifications are not mistaken for a
// change made elsewhere.
const GUID kSonkoreContext = {0x6b1f3c52, 0x9d0e, 0x4a7b, {0x8e, 0x21, 0x5c, 0x33, 0x0f, 0x7a, 0x94, 0xd1}};

template <typename T>
void release(T*& p)
{
    if (p) {
        p->Release();
        p = nullptr;
    }
}

} // namespace

// Called by Windows on a thread of its own: hands the news to the GUI thread.
class WinEndpointVolume::Callback final : public IAudioEndpointVolumeCallback {
public:
    explicit Callback(WinEndpointVolume* owner) : m_owner(owner) {}

    ULONG STDMETHODCALLTYPE AddRef() override { return ++m_refs; }
    ULONG STDMETHODCALLTYPE Release() override
    {
        const ULONG left = --m_refs;
        if (left == 0)
            delete this;
        return left;
    }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override
    {
        if (iid == __uuidof(IUnknown) || iid == __uuidof(IAudioEndpointVolumeCallback)) {
            *out = static_cast<IAudioEndpointVolumeCallback*>(this);
            AddRef();
            return S_OK;
        }
        *out = nullptr;
        return E_NOINTERFACE;
    }
    HRESULT STDMETHODCALLTYPE OnNotify(PAUDIO_VOLUME_NOTIFICATION_DATA data) override
    {
        if (data && IsEqualGUID(data->guidEventContext, kSonkoreContext))
            return S_OK;
        // Queued on the owner: close() unregisters before it goes away, and
        // Qt drops events still posted to a deleted object.
        WinEndpointVolume* owner = m_owner;
        QMetaObject::invokeMethod(owner, [owner] { emit owner->volumeChanged(); }, Qt::QueuedConnection);
        return S_OK;
    }

private:
    std::atomic<ULONG> m_refs{1};
    WinEndpointVolume* m_owner;
};

WinEndpointVolume::WinEndpointVolume(QObject* parent)
    : QObject(parent)
{
    // Qt has usually set up COM on the GUI thread already (S_FALSE, or
    // RPC_E_CHANGED_MODE for another apartment): either way it is usable.
    const HRESULT hr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    m_comInitialized = SUCCEEDED(hr);
}

WinEndpointVolume::~WinEndpointVolume()
{
    close();
    if (m_comInitialized)
        CoUninitialize();
}

bool WinEndpointVolume::open(const QString& endpointId)
{
    if (m_volume && endpointId == m_endpoint)
        return true;
    close();

    IMMDeviceEnumerator* enumerator = nullptr;
    IMMDevice* device = nullptr;
    IPropertyStore* props = nullptr;
    IAudioEndpointVolume* volume = nullptr;
    bool ok = false;
    do {
        if (FAILED(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, __uuidof(IMMDeviceEnumerator),
                                    reinterpret_cast<void**>(&enumerator))))
            break;
        const HRESULT got = endpointId.isEmpty()
            ? enumerator->GetDefaultAudioEndpoint(eRender, eConsole, &device)
            : enumerator->GetDevice(reinterpret_cast<LPCWSTR>(endpointId.utf16()), &device);
        if (FAILED(got))
            break;
        if (FAILED(device->Activate(__uuidof(IAudioEndpointVolume), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(&volume))))
            break;
        // Only a control the device implements: in software, Windows scales
        // the shared-mode mix, which exclusive mode never goes through.
        DWORD support = 0;
        if (FAILED(volume->QueryHardwareSupport(&support)) || !(support & ENDPOINT_HARDWARE_SUPPORT_VOLUME))
            break;
        float minDb = 0.0f, maxDb = 0.0f, step = 0.0f;
        if (FAILED(volume->GetVolumeRange(&minDb, &maxDb, &step)) || !(maxDb > minDb))
            break;
        m_minDb = minDb;
        m_topDb = std::min(0.0, static_cast<double>(maxDb)); // never pushed above unity
        if (SUCCEEDED(device->OpenPropertyStore(STGM_READ, &props))) {
            PROPVARIANT name;
            PropVariantInit(&name);
            if (SUCCEEDED(props->GetValue(PKEY_Device_FriendlyName, &name)) && name.vt == VT_LPWSTR)
                m_name = QString::fromWCharArray(name.pwszVal);
            PropVariantClear(&name);
        }
        if (m_name.isEmpty())
            m_name = tr("Output volume");
        m_callback = new Callback(this);
        if (FAILED(volume->RegisterControlChangeNotify(m_callback))) {
            m_callback->Release();
            m_callback = nullptr;
        }
        m_volume = volume;
        volume = nullptr;
        m_endpoint = endpointId;
        ok = true;
    } while (false);
    release(volume);
    release(props);
    release(device);
    release(enumerator);
    if (ok)
        emit volumeChanged();
    return ok;
}

void WinEndpointVolume::close()
{
    if (m_volume && m_callback)
        m_volume->UnregisterControlChangeNotify(m_callback);
    if (m_callback) {
        m_callback->Release();
        m_callback = nullptr;
    }
    release(m_volume);
    m_endpoint.clear();
    m_name.clear();
}

double WinEndpointVolume::volume() const
{
    if (!m_volume)
        return 1.0;
    BOOL muted = FALSE;
    if (SUCCEEDED(m_volume->GetMute(&muted)) && muted)
        return 0.0;
    float db = 0.0f;
    if (FAILED(m_volume->GetMasterVolumeLevel(&db)))
        return 1.0;
    if (db <= m_minDb)
        return 0.0;
    return std::clamp(std::pow(10.0, (db - m_topDb) / 20.0), 0.0, 1.0);
}

void WinEndpointVolume::setVolume(double linear)
{
    if (!m_volume || !std::isfinite(linear))
        return;
    linear = std::clamp(linear, 0.0, 1.0);
    if (linear <= 0.0) {
        m_volume->SetMute(TRUE, &kSonkoreContext);
        return;
    }
    const double db = std::max(m_minDb, m_topDb + 20.0 * std::log10(linear));
    m_volume->SetMasterVolumeLevel(static_cast<float>(db), &kSonkoreContext);
    BOOL muted = FALSE;
    if (SUCCEEDED(m_volume->GetMute(&muted)) && muted)
        m_volume->SetMute(FALSE, &kSonkoreContext);
}
