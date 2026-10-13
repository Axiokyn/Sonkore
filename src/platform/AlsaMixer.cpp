#include "AlsaMixer.hpp"

#include <QSocketNotifier>

#include <alsa/asoundlib.h>

#include <algorithm>
#include <cmath>
#include <poll.h>

namespace {

// Usual names of a card's main playback control, best first. USB DACs often
// expose a single control named after the device: any playback volume
// control is taken when none of these exists.
const char* const kPreferred[] = {"PCM", "Master", "Digital", "Headphone", "Speaker", "Front", "DAC"};

// True for controls the hardware really has. alsa-lib's softvol plugin adds
// "user" controls (the "PCM" of most onboard HD-Audio chips) that only act on
// the "default" chain: on a direct hw: output they change nothing.
bool isHardwareControl(snd_ctl_t* ctl, snd_mixer_elem_t* e)
{
    if (!ctl)
        return true;
    const QByteArray wanted = QByteArray(snd_mixer_selem_get_name(e)) + " Playback Volume";
    const unsigned index = snd_mixer_selem_get_index(e);
    snd_ctl_elem_list_t* list = nullptr;
    snd_ctl_elem_list_alloca(&list);
    if (snd_ctl_elem_list(ctl, list) < 0)
        return true;
    const unsigned count = snd_ctl_elem_list_get_count(list);
    if (snd_ctl_elem_list_alloc_space(list, count) < 0)
        return true;
    bool hardware = true;
    if (snd_ctl_elem_list(ctl, list) == 0) {
        for (unsigned i = 0; i < snd_ctl_elem_list_get_used(list); ++i) {
            if (wanted != snd_ctl_elem_list_get_name(list, i) || snd_ctl_elem_list_get_index(list, i) != index)
                continue;
            snd_ctl_elem_id_t* id = nullptr;
            snd_ctl_elem_id_alloca(&id);
            snd_ctl_elem_list_get_id(list, i, id);
            snd_ctl_elem_info_t* info = nullptr;
            snd_ctl_elem_info_alloca(&info);
            snd_ctl_elem_info_set_id(info, id);
            if (snd_ctl_elem_info(ctl, info) == 0 && snd_ctl_elem_info_is_user(info))
                hardware = false;
            break;
        }
    }
    snd_ctl_elem_list_free_space(list);
    return hardware;
}

snd_mixer_elem_t* pickControl(snd_mixer_t* mixer, snd_ctl_t* ctl)
{
    const auto usable = [ctl](snd_mixer_elem_t* e) {
        return snd_mixer_selem_is_active(e) && snd_mixer_selem_has_playback_volume(e) && isHardwareControl(ctl, e);
    };
    for (const char* name : kPreferred)
        for (snd_mixer_elem_t* e = snd_mixer_first_elem(mixer); e; e = snd_mixer_elem_next(e))
            if (qstrcmp(snd_mixer_selem_get_name(e), name) == 0 && usable(e))
                return e;
    for (snd_mixer_elem_t* e = snd_mixer_first_elem(mixer); e; e = snd_mixer_elem_next(e))
        if (usable(e))
            return e;
    return nullptr;
}

} // namespace

AlsaMixer::AlsaMixer(QObject* parent)
    : QObject(parent)
{
}

AlsaMixer::~AlsaMixer()
{
    close();
}

bool AlsaMixer::open(const QString& card)
{
    if (isOpen() && card == m_card)
        return true;
    close();
    const QByteArray name = card.toUtf8();
    if (snd_mixer_open(&m_mixer, 0) < 0)
        return false;
    if (snd_mixer_attach(m_mixer, name.constData()) < 0 || snd_mixer_selem_register(m_mixer, nullptr, nullptr) < 0
        || snd_mixer_load(m_mixer) < 0) {
        close();
        return false;
    }
    snd_ctl_t* ctl = nullptr;
    if (snd_ctl_open(&ctl, name.constData(), SND_CTL_READONLY) < 0)
        ctl = nullptr;
    m_elem = pickControl(m_mixer, ctl);
    if (ctl)
        snd_ctl_close(ctl);
    if (!m_elem) {
        close();
        return false;
    }
    m_card = card;
    m_name = QString::fromUtf8(snd_mixer_selem_get_name(m_elem));
    snd_mixer_selem_get_playback_volume_range(m_elem, &m_minRaw, &m_maxRaw);
    long minDb = 0, maxDb = 0;
    m_hasDb = snd_mixer_selem_get_playback_dB_range(m_elem, &minDb, &maxDb) == 0 && maxDb > minDb;
    if (m_hasDb) {
        m_minDb = minDb / 100.0;
        m_topDb = std::min(maxDb / 100.0, 0.0);
    }

    // Changes made elsewhere arrive on the mixer's file descriptors.
    const int count = snd_mixer_poll_descriptors_count(m_mixer);
    std::vector<pollfd> fds(static_cast<std::size_t>(std::max(0, count)));
    if (count > 0 && snd_mixer_poll_descriptors(m_mixer, fds.data(), static_cast<unsigned>(count)) > 0)
        for (const pollfd& fd : fds) {
            auto notifier = std::make_unique<QSocketNotifier>(fd.fd, QSocketNotifier::Read);
            connect(notifier.get(), &QSocketNotifier::activated, this, &AlsaMixer::handleEvents);
            m_notifiers.push_back(std::move(notifier));
        }
    return true;
}

void AlsaMixer::close()
{
    m_notifiers.clear();
    if (m_mixer)
        snd_mixer_close(m_mixer);
    m_mixer = nullptr;
    m_elem = nullptr;
    m_card.clear();
    m_name.clear();
    m_hasDb = false;
}

void AlsaMixer::handleEvents()
{
    if (!m_mixer)
        return;
    snd_mixer_handle_events(m_mixer);
    emit volumeChanged();
}

double AlsaMixer::volume() const
{
    if (!m_elem)
        return 1.0;
    if (snd_mixer_selem_has_playback_switch(m_elem)) {
        int on = 1;
        if (snd_mixer_selem_get_playback_switch(m_elem, SND_MIXER_SCHN_FRONT_LEFT, &on) == 0 && !on)
            return 0.0;
    }
    if (m_hasDb) {
        long db = 0;
        if (snd_mixer_selem_get_playback_dB(m_elem, SND_MIXER_SCHN_FRONT_LEFT, &db) == 0)
            return std::min(1.0, std::pow(10.0, (db / 100.0 - m_topDb) / 20.0));
    }
    long raw = 0;
    snd_mixer_selem_get_playback_volume(m_elem, SND_MIXER_SCHN_FRONT_LEFT, &raw);
    return m_maxRaw > m_minRaw ? static_cast<double>(raw - m_minRaw) / static_cast<double>(m_maxRaw - m_minRaw) : 1.0;
}

void AlsaMixer::setVolume(double linear)
{
    if (!m_elem)
        return;
    linear = std::clamp(linear, 0.0, 1.0);
    const bool hasSwitch = snd_mixer_selem_has_playback_switch(m_elem);
    if (linear <= 0.0) {
        if (hasSwitch)
            snd_mixer_selem_set_playback_switch_all(m_elem, 0);
        else
            snd_mixer_selem_set_playback_volume_all(m_elem, m_minRaw);
        return;
    }
    if (hasSwitch)
        snd_mixer_selem_set_playback_switch_all(m_elem, 1);
    if (m_hasDb) {
        // Towards the quieter step when the control cannot hit the value.
        const double db = std::max(m_minDb, m_topDb + 20.0 * std::log10(linear));
        if (snd_mixer_selem_set_playback_dB_all(m_elem, std::lround(db * 100.0), -1) < 0)
            snd_mixer_selem_set_playback_dB_all(m_elem, std::lround(db * 100.0), 1);
        return;
    }
    snd_mixer_selem_set_playback_volume_all(m_elem, m_minRaw + std::lround(linear * static_cast<double>(m_maxRaw - m_minRaw)));
}
