#include "EqController.hpp"
#include "SettingsLock.hpp"
#include "AppPaths.hpp"

#include "Biquad.hpp"

#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QNetworkReply>
#include <QRegularExpression>
#include <QSettings>
#include <QStandardPaths>
#include <QTextStream>

#include <algorithm>
#include <cmath>
#include <complex>
#include <numbers>

namespace {

const double kGraphicFreqs[] = {31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000};
constexpr double kGraphicQ = 1.41421356237;
constexpr double kResponseRate = 48000.0;
constexpr int kResponsePoints = 192;

const char* kAutoEqRaw = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/";

struct Builtin {
    const char* name;
    double gains[10];
};
const Builtin kBuiltins[] = {
    {"Flat", {0, 0, 0, 0, 0, 0, 0, 0, 0, 0}},
    {"Bass Boost", {6, 5, 4, 2, 0.5, 0, 0, 0, 0, 0}},
    {"Bass Reducer", {-6, -5, -4, -2, -0.5, 0, 0, 0, 0, 0}},
    {"Treble Boost", {0, 0, 0, 0, 0, 1, 2, 4, 5, 6}},
    {"Treble Reducer", {0, 0, 0, 0, 0, -1, -2, -4, -5, -6}},
    {"Vocal", {-2, -2, -1, 1, 3, 3, 2, 1, 0, -1}},
    {"Rock", {4, 3, 2, 0, -1, -1, 1, 2, 3, 4}},
    {"Pop", {-1, 1, 3, 4, 3, 0, -1, -1, 0, 1}},
    {"Jazz", {3, 2, 1, 2, -1, -1, 0, 1, 2, 3}},
    {"Classical", {0, 0, 0, 0, 0, 0, -3, -3, -3, -5}},
    {"Electronic", {4, 3, 1, 0, -2, 2, 1, 1, 3, 4}},
    {"Acoustic", {4, 4, 3, 1, 2, 1, 3, 3, 3, 2}},
    {"Loudness", {5, 4, 1, 0, -1, 0, 0, 1, 4, 5}},
};

QString dataPath(const QString& file)
{
    return sonkore::paths::dataFile(file);
}

QString localPath(const QUrl& url)
{
    return url.isLocalFile() ? url.toLocalFile() : url.toString();
}

} // namespace

EqController::EqController(sonkore::SuperEQ& eq, QObject* parent)
    : QObject(parent)
    , m_eq(eq)
{
    m_bands = graphicLayout();
    loadUserProfiles();
    loadState();
    apply(false);
}

EqController::~EqController()
{
    saveState();
}

// --- Band model ----------------------------------------------------------------

QVector<EqController::Band> EqController::graphicLayout(const QVector<double>& gains)
{
    QVector<Band> bands;
    for (int i = 0; i < 10; ++i) {
        Band b;
        b.type = QStringLiteral("PK");
        b.freq = kGraphicFreqs[i];
        b.q = kGraphicQ;
        b.gain = i < gains.size() ? gains[i] : 0.0;
        bands << b;
    }
    return bands;
}

sonkore::FilterType EqController::toFilterType(const QString& type)
{
    const QString t = type.toUpper();
    if (t == "LSC" || t == "LS" || t == "LSQ") return sonkore::FilterType::LowShelf;
    if (t == "HSC" || t == "HS" || t == "HSQ") return sonkore::FilterType::HighShelf;
    if (t == "LPQ" || t == "LP") return sonkore::FilterType::LowPass;
    if (t == "HPQ" || t == "HP") return sonkore::FilterType::HighPass;
    if (t == "BP") return sonkore::FilterType::BandPass;
    if (t == "NO" || t == "NOTCH") return sonkore::FilterType::Notch;
    return sonkore::FilterType::Peaking;
}

QStringList EqController::filterTypes() const
{
    return {"PK", "LSC", "HSC", "LPQ", "HPQ", "BP", "NO"};
}

QString EqController::typeLabel(const QString& type) const
{
    switch (toFilterType(type)) {
    case sonkore::FilterType::LowShelf: return tr("Low shelf");
    case sonkore::FilterType::HighShelf: return tr("High shelf");
    case sonkore::FilterType::LowPass: return tr("Low pass");
    case sonkore::FilterType::HighPass: return tr("High pass");
    case sonkore::FilterType::BandPass: return tr("Band pass");
    case sonkore::FilterType::Notch: return tr("Notch");
    default: return tr("Peak");
    }
}

QString EqController::frequencyLabel(double hz) const
{
    if (hz >= 1000.0) {
        const double k = hz / 1000.0;
        return (std::fmod(k, 1.0) == 0.0 ? QString::number(k, 'f', 0) : QString::number(k, 'g', 3)) + "k";
    }
    return QString::number(hz, 'g', 4);
}

QVariantMap EqController::bandToMap(const Band& b)
{
    return {{"type", b.type}, {"freq", b.freq}, {"gain", b.gain}, {"q", b.q}, {"enabled", b.enabled}};
}

EqController::Band EqController::bandFromMap(const QVariantMap& m)
{
    Band b;
    b.type = m.value("type", "PK").toString();
    b.freq = m.value("freq", 1000.0).toDouble();
    b.gain = m.value("gain", 0.0).toDouble();
    b.q = m.value("q", kGraphicQ).toDouble();
    b.enabled = m.value("enabled", true).toBool();
    return b;
}

QVariantList EqController::bands() const
{
    QVariantList list;
    for (const Band& b : m_bands)
        list << bandToMap(b);
    return list;
}

void EqController::setEnabled(bool on)
{
    if (on == m_enabled)
        return;
    m_enabled = on;
    m_eq.setEnabled(on);
    saveState();
    emit enabledChanged();
}

void EqController::setLinearPhase(bool on)
{
    if (on == m_linearPhase)
        return;
    m_linearPhase = on;
    m_eq.setLinearPhase(on);
    saveState();
    emit linearPhaseChanged();
}

void EqController::setMode(const QString& mode)
{
    if (mode == m_mode || (mode != "graphic" && mode != "parametric"))
        return;
    if (mode == "graphic") {
        // Approximate the current curve on the 10 graphic bands.
        QVector<double> gains;
        for (double f : kGraphicFreqs)
            gains << std::round((responseAt(f) - m_preamp) * 10.0) / 10.0;
        m_bands = graphicLayout(gains);
    }
    m_mode = mode;
    apply();
}

void EqController::setPreamp(double db)
{
    db = std::clamp(db, -30.0, 12.0);
    if (db == m_preamp)
        return;
    m_preamp = db;
    apply();
}

void EqController::setGain(int index, double db)
{
    if (index < 0 || index >= m_bands.size())
        return;
    db = std::clamp(db, -24.0, 24.0);
    if (m_bands[index].gain == db)
        return;
    m_bands[index].gain = db;
    apply();
}

double EqController::graphicFrequency(int index) const
{
    return index >= 0 && index < static_cast<int>(std::size(kGraphicFreqs)) ? kGraphicFreqs[index] : 0.0;
}

void EqController::setBand(int index, const QString& type, double freq, double gain, double q, bool enabled)
{
    if (index < 0 || index >= m_bands.size())
        return;
    Band& b = m_bands[index];
    b.type = type;
    b.freq = std::clamp(freq, 10.0, 24000.0);
    b.gain = std::clamp(gain, -24.0, 24.0);
    b.q = std::clamp(q, 0.05, 30.0);
    b.enabled = enabled;
    apply();
}

void EqController::addBand()
{
    if (m_bands.size() >= maxBands())
        return;
    if (m_mode == "graphic")
        m_mode = QStringLiteral("parametric");
    Band b;
    b.freq = 1000.0;
    b.q = 1.0;
    m_bands << b;
    apply();
}

void EqController::removeBand(int index)
{
    if (index < 0 || index >= m_bands.size())
        return;
    if (m_mode == "graphic")
        m_mode = QStringLiteral("parametric");
    m_bands.removeAt(index);
    apply();
}

void EqController::reset()
{
    m_mode = QStringLiteral("graphic");
    m_bands = graphicLayout();
    m_preamp = 0.0;
    apply(false);
    setCurrentProfile(QStringLiteral("Flat"));
}

void EqController::autoPreamp()
{
    const double peak = peakGain() - m_preamp; // curve without the current preamp
    m_preamp = peak > 0.0 ? -std::ceil(peak * 10.0) / 10.0 : 0.0;
    apply();
}

// --- Response --------------------------------------------------------------------

namespace {

double sectionsDb(const std::vector<sonkore::BiquadCoeffs>& sections, double hz)
{
    const std::complex<double> z1 = std::polar(1.0, -2.0 * std::numbers::pi * hz / kResponseRate);
    const std::complex<double> z2 = z1 * z1;
    double db = 0.0;
    for (const auto& c : sections) {
        const std::complex<double> h = (c.b0 + c.b1 * z1 + c.b2 * z2) / (1.0 + c.a1 * z1 + c.a2 * z2);
        db += 20.0 * std::log10(std::max(std::abs(h), 1e-9));
    }
    return db;
}

} // namespace

QVector<EqController::Band> EqController::engineBands() const
{
    if (m_mode != "graphic" || m_bands.size() != 10)
        return m_bands;
    std::array<double, 10> sliders{};
    for (int i = 0; i < 10; ++i)
        sliders[i] = m_bands[i].enabled ? m_bands[i].gain : 0.0;
    const auto gains = sonkore::SuperEQ::graphicGains(sliders);
    QVector<Band> bands = m_bands;
    for (int i = 0; i < 10; ++i)
        bands[i].gain = std::round(gains[i] * 1000.0) / 1000.0;
    return bands;
}

void EqController::updateSections()
{
    m_sections.clear();
    for (const Band& b : engineBands())
        if (b.enabled)
            m_sections.push_back(sonkore::designBiquad(toFilterType(b.type), kResponseRate, b.freq, b.q, b.gain));
}

double EqController::responseAt(double hz) const
{
    return m_preamp + sectionsDb(m_sections, hz);
}

QVariantList EqController::response() const
{
    QVariantList points;
    const double lo = std::log10(20.0), hi = std::log10(20000.0);
    for (int i = 0; i < kResponsePoints; ++i) {
        const double f = std::pow(10.0, lo + (hi - lo) * i / (kResponsePoints - 1));
        points << QVariantMap{{"f", f}, {"db", responseAt(f)}};
    }
    return points;
}

double EqController::peakGain() const
{
    double peak = -1e9;
    const double lo = std::log10(20.0), hi = std::log10(20000.0);
    for (int i = 0; i < 400; ++i)
        peak = std::max(peak, responseAt(std::pow(10.0, lo + (hi - lo) * i / 399.0)));
    return peak;
}

// --- Engine sync -------------------------------------------------------------------

void EqController::apply(bool markCustom)
{
    sonkore::SuperEQ::Curve curve;
    for (const Band& b : engineBands()) {
        sonkore::SuperEQ::Band eb;
        eb.type = toFilterType(b.type);
        eb.freqHz = b.freq;
        eb.gainDb = b.gain;
        eb.q = b.q;
        eb.enabled = b.enabled;
        curve.bands.push_back(eb);
    }
    curve.preampDb = m_preamp;
    m_eq.setCurve(std::move(curve));
    m_eq.setEnabled(m_enabled);
    updateSections();
    m_eq.setLinearPhase(m_linearPhase);
    if (markCustom)
        setCurrentProfile(QString());
    saveState();
    emit bandsChanged();
}

void EqController::setCurrentProfile(const QString& name)
{
    if (name == m_currentProfile)
        return;
    m_currentProfile = name;
    saveState();
    emit currentProfileChanged();
}

// --- Profiles ------------------------------------------------------------------------

QStringList EqController::builtinProfiles() const
{
    QStringList names;
    for (const Builtin& b : kBuiltins)
        names << QString::fromLatin1(b.name);
    return names;
}

QStringList EqController::userProfiles() const
{
    QStringList names;
    for (const Profile& p : m_userProfiles)
        names << p.name;
    return names;
}

void EqController::loadProfile(const QString& name)
{
    for (const Builtin& b : kBuiltins) {
        if (name == QLatin1String(b.name)) {
            m_mode = QStringLiteral("graphic");
            m_bands = graphicLayout(QVector<double>(std::begin(b.gains), std::end(b.gains)));
            m_preamp = 0.0;
            const double peak = peakGain();
            if (peak > 0.0)
                m_preamp = -std::ceil(peak * 10.0) / 10.0; // headroom against clipping
            apply(false);
            setCurrentProfile(name);
            return;
        }
    }
    for (const Profile& p : m_userProfiles) {
        if (p.name == name) {
            m_mode = p.mode;
            m_preamp = p.preamp;
            m_bands = p.bands;
            apply(false);
            setCurrentProfile(name);
            return;
        }
    }
}

QString EqController::saveProfile(const QString& wanted)
{
    QString name = wanted.trimmed();
    if (name.isEmpty())
        name = tr("My profile");
    if (builtinProfiles().contains(name))
        name += tr(" (custom)");
    Profile p{name, m_mode, m_preamp, m_bands};
    bool replaced = false;
    for (Profile& existing : m_userProfiles)
        if (existing.name == name) {
            existing = p;
            replaced = true;
        }
    if (!replaced)
        m_userProfiles << p;
    saveUserProfiles();
    emit profilesChanged();
    setCurrentProfile(name);
    return name;
}

void EqController::deleteProfile(const QString& name)
{
    for (int i = 0; i < m_userProfiles.size(); ++i) {
        if (m_userProfiles[i].name == name) {
            m_userProfiles.removeAt(i);
            saveUserProfiles();
            emit profilesChanged();
            if (m_currentProfile == name)
                setCurrentProfile(QString());
            return;
        }
    }
}

bool EqController::parseParametric(const QString& text, double& preamp, QVector<Band>& bands)
{
    static const QRegularExpression preampRe(QStringLiteral("^\\s*Preamp\\s*:\\s*([-+]?[\\d.]+)\\s*dB"),
                                             QRegularExpression::CaseInsensitiveOption);
    static const QRegularExpression filterRe(
        QStringLiteral("^\\s*Filter\\s*\\d*\\s*:\\s*(ON|OFF)\\s+([A-Z]+)(?:\\s+([\\d.]+)\\s*dB)?\\s+Fc\\s+([\\d.]+)\\s*Hz"
                       "(?:\\s+Gain\\s+([-+]?[\\d.]+)\\s*dB)?(?:\\s+Q\\s+([\\d.]+))?"),
        QRegularExpression::CaseInsensitiveOption);

    preamp = 0.0;
    bands.clear();
    for (const QString& line : text.split('\n')) {
        if (const auto m = preampRe.match(line); m.hasMatch()) {
            preamp = m.captured(1).toDouble();
            continue;
        }
        const auto m = filterRe.match(line);
        if (!m.hasMatch())
            continue;
        Band b;
        b.enabled = m.captured(1).compare("ON", Qt::CaseInsensitive) == 0;
        b.type = m.captured(2).toUpper();
        if (b.type == "PEQ") b.type = "PK";
        if (b.type == "LS" || b.type == "LSQ") b.type = "LSC";
        if (b.type == "HS" || b.type == "HSQ") b.type = "HSC";
        if (b.type == "LP") b.type = "LPQ";
        if (b.type == "HP") b.type = "HPQ";
        b.freq = m.captured(4).toDouble();
        b.gain = m.captured(5).isEmpty() ? 0.0 : m.captured(5).toDouble();
        b.q = m.captured(6).isEmpty() ? 0.7071 : m.captured(6).toDouble();
        // Shelves given by slope (Equalizer APO: "LSC 10.8 dB", "LS 12dB",
        // or a plain "LS"/"HS", whose slope is S = 0.9) instead of Q: the
        // RBJ shelf slope S, x dB per octave being S = x / 12, as Q.
        if ((b.type == "LSC" || b.type == "HSC") && m.captured(6).isEmpty()) {
            const QString t = m.captured(2).toUpper();
            const double slope = !m.captured(3).isEmpty() ? std::clamp(m.captured(3).toDouble() / 12.0, 0.05, 1.0)
                                 : (t == "LS" || t == "HS") ? 0.9 : 0.0;
            if (slope > 0.0) {
                const double A = std::pow(10.0, std::fabs(b.gain) / 40.0);
                b.q = 1.0 / std::sqrt((A + 1.0 / A) * (1.0 / slope - 1.0) + 2.0);
            }
        }
        if (bands.size() < sonkore::SuperEQ::kMaxBands)
            bands << b;
    }
    return !bands.isEmpty();
}

bool EqController::importFile(const QUrl& file)
{
    QFile f(localPath(file));
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text))
        return false;
    double preamp = 0.0;
    QVector<Band> bands;
    if (!parseParametric(QString::fromUtf8(f.readAll()), preamp, bands)) {
        emit message(tr("No parametric EQ filters found in that file"));
        return false;
    }
    m_mode = QStringLiteral("parametric");
    m_preamp = preamp;
    m_bands = bands;
    apply(false);
    saveProfile(QFileInfo(f.fileName()).completeBaseName());
    emit message(tr("Imported %1 filters").arg(bands.size()));
    return true;
}

bool EqController::exportFile(const QUrl& file) const
{
    QFile f(localPath(file));
    if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate | QIODevice::Text))
        return false;
    QTextStream out(&f);
    out << "Preamp: " << QString::number(m_preamp, 'f', 1) << " dB\n";
    int n = 1;
    for (const Band& b : m_bands)
        out << "Filter " << n++ << ": " << (b.enabled ? "ON " : "OFF ") << b.type << " Fc "
            << QString::number(b.freq, 'g', 6) << " Hz Gain " << QString::number(b.gain, 'f', 1) << " dB Q "
            << QString::number(b.q, 'f', 2) << '\n';
    return true;
}

// --- AutoEQ --------------------------------------------------------------------------

void EqController::setAutoEqState(bool busy, const QString& status)
{
    m_autoEqBusy = busy;
    m_autoEqStatus = status;
    emit autoEqStateChanged();
}

void EqController::parseAutoEqIndex(const QByteArray& markdown)
{
    // "- [<Model>](./<source>/<type>/<Model, URL-encoded>) by <source> on ..."
    static const QRegularExpression re(QStringLiteral("^- \\[(.+)\\]\\(\\./(.+)\\) by (\\S+)(?: on (.+))?$"));
    m_autoEqIndex.clear();
    for (const QByteArray& raw : markdown.split('\n')) {
        const auto m = re.match(QString::fromUtf8(raw).trimmed());
        if (!m.hasMatch())
            continue;
        AutoEqEntry e{m.captured(1), m.captured(2), m.captured(3), m.captured(4), {}};
        e.searchText = (e.name + ' ' + e.source + ' ' + e.rig).toLower();
        m_autoEqIndex << e;
    }
}

void EqController::autoEqRefreshIndex()
{
    if (m_autoEqBusy)
        return;
    setAutoEqState(true, tr("Downloading AutoEQ database…"));
    QNetworkReply* reply = m_network.get(QNetworkRequest(QUrl(QString::fromLatin1(kAutoEqRaw) + "INDEX.md")));
    connect(reply, &QNetworkReply::finished, this, [this, reply] {
        reply->deleteLater();
        if (reply->error() != QNetworkReply::NoError) {
            setAutoEqState(false, tr("Could not reach AutoEQ: %1").arg(reply->errorString()));
            return;
        }
        const QByteArray data = reply->readAll();
        parseAutoEqIndex(data);
        QDir().mkpath(QFileInfo(dataPath("autoeq-index.md")).path());
        QFile cache(dataPath("autoeq-index.md"));
        if (cache.open(QIODevice::WriteOnly | QIODevice::Truncate))
            cache.write(data);
        setAutoEqState(false, tr("%1 headphone profiles available").arg(m_autoEqIndex.size()));
        autoEqSearch(m_pendingSearch);
    });
}

void EqController::autoEqSearch(const QString& text)
{
    m_pendingSearch = text;
    if (m_autoEqIndex.isEmpty()) {
        QFile cache(dataPath("autoeq-index.md"));
        const bool fresh = cache.exists()
            && QFileInfo(cache).lastModified().daysTo(QDateTime::currentDateTime()) < 14;
        if (fresh && cache.open(QIODevice::ReadOnly)) {
            parseAutoEqIndex(cache.readAll());
            setAutoEqState(false, tr("%1 headphone profiles available").arg(m_autoEqIndex.size()));
        } else {
            autoEqRefreshIndex();
            return;
        }
    }

    const QStringList words = text.toLower().split(' ', Qt::SkipEmptyParts);
    m_autoEqMatches.clear();
    m_autoEqResults.clear();
    if (!words.isEmpty()) {
        for (int i = 0; i < m_autoEqIndex.size() && m_autoEqMatches.size() < 150; ++i) {
            const AutoEqEntry& e = m_autoEqIndex[i];
            if (std::all_of(words.begin(), words.end(), [&](const QString& w) { return e.searchText.contains(w); })) {
                m_autoEqMatches << i;
                m_autoEqResults << QVariantMap{{"name", e.name}, {"source", e.source}, {"rig", e.rig}};
            }
        }
    }
    emit autoEqResultsChanged();
}

void EqController::autoEqApply(int resultIndex)
{
    if (resultIndex < 0 || resultIndex >= m_autoEqMatches.size() || m_autoEqBusy)
        return;
    const AutoEqEntry e = m_autoEqIndex[m_autoEqMatches[resultIndex]];
    const QString leaf = e.path.section('/', -1); // already percent-encoded
    const QByteArray url = QByteArray(kAutoEqRaw) + e.path.toUtf8() + '/' + leaf.toUtf8() + "%20ParametricEQ.txt";

    setAutoEqState(true, tr("Downloading %1…").arg(e.name));
    QNetworkReply* reply = m_network.get(QNetworkRequest(QUrl::fromEncoded(url)));
    connect(reply, &QNetworkReply::finished, this, [this, reply, e] {
        reply->deleteLater();
        if (reply->error() != QNetworkReply::NoError) {
            setAutoEqState(false, tr("Download failed: %1").arg(reply->errorString()));
            return;
        }
        double preamp = 0.0;
        QVector<Band> bands;
        if (!parseParametric(QString::fromUtf8(reply->readAll()), preamp, bands)) {
            setAutoEqState(false, tr("No parametric profile published for %1").arg(e.name));
            return;
        }
        m_mode = QStringLiteral("parametric");
        m_preamp = preamp;
        m_bands = bands;
        m_enabled = true;
        emit enabledChanged();
        apply(false);
        const QString saved = saveProfile(QStringLiteral("AutoEQ · %1 (%2)").arg(e.name, e.source));
        setAutoEqState(false, tr("Applied and saved as “%1”").arg(saved));
    });
}

// --- Persistence ---------------------------------------------------------------------

void EqController::loadState()
{
    QSettings s;
    s.beginGroup("equalizer");
    m_enabled = s.value("enabled", false).toBool(); // a fresh install starts bit-perfect
    m_linearPhase = s.value("linearPhase", false).toBool();
    m_mode = s.value("mode", "graphic").toString();
    m_preamp = s.value("preamp", 0.0).toDouble();
    m_currentProfile = s.value("profile", "Flat").toString();
    const QVariantList bands = QJsonDocument::fromJson(s.value("bands").toByteArray()).toVariant().toList();
    if (!bands.isEmpty()) {
        m_bands.clear();
        for (const QVariant& v : bands)
            m_bands << bandFromMap(v.toMap());
    }
    s.endGroup();
}

void EqController::saveState() const
{
    if (sonkore::g_settingsFrozen.load())
        return;
    QVariantList bands;
    for (const Band& b : m_bands)
        bands << bandToMap(b);
    QSettings s;
    s.beginGroup("equalizer");
    s.setValue("enabled", m_enabled);
    s.setValue("linearPhase", m_linearPhase);
    s.setValue("mode", m_mode);
    s.setValue("preamp", m_preamp);
    s.setValue("profile", m_currentProfile);
    s.setValue("bands", QJsonDocument::fromVariant(bands).toJson(QJsonDocument::Compact));
    s.endGroup();
}

void EqController::loadUserProfiles()
{
    QFile f(dataPath("eq-profiles.json"));
    if (!f.open(QIODevice::ReadOnly))
        return;
    for (const QVariant& v : QJsonDocument::fromJson(f.readAll()).toVariant().toList()) {
        const QVariantMap m = v.toMap();
        Profile p;
        p.name = m.value("name").toString();
        p.mode = m.value("mode", "parametric").toString();
        p.preamp = m.value("preamp").toDouble();
        for (const QVariant& b : m.value("bands").toList())
            p.bands << bandFromMap(b.toMap());
        if (!p.name.isEmpty())
            m_userProfiles << p;
    }
}

void EqController::saveUserProfiles() const
{
    QVariantList list;
    for (const Profile& p : m_userProfiles) {
        QVariantList bands;
        for (const Band& b : p.bands)
            bands << bandToMap(b);
        list << QVariantMap{{"name", p.name}, {"mode", p.mode}, {"preamp", p.preamp}, {"bands", bands}};
    }
    QDir().mkpath(QFileInfo(dataPath("eq-profiles.json")).path());
    QFile f(dataPath("eq-profiles.json"));
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(QJsonDocument::fromVariant(list).toJson(QJsonDocument::Indented));
}
