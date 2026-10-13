#pragma once

#include "QmlSingleton.hpp"
#include "SuperEQ.hpp"

#include <QNetworkAccessManager>
#include <QObject>
#include <QStringList>
#include <QUrl>
#include <QVariantList>
#include <QVector>
#include <QtQml/qqmlregistration.h>

// Equalizer state for QML: 10-band graphic or up to 32-band parametric,
// preamp, response curve, saved profiles and AutoEQ (github.com/jaakkopasanen/AutoEq)
// headphone correction profiles.
class EqController : public QObject {
    Q_OBJECT
    QML_NAMED_ELEMENT(Equalizer)
    QML_SINGLETON
    SONKORE_QML_SINGLETON_INSTANCE(EqController)

    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged)
    Q_PROPERTY(QString mode READ mode WRITE setMode NOTIFY bandsChanged)
    Q_PROPERTY(double preamp READ preamp WRITE setPreamp NOTIFY bandsChanged)
    Q_PROPERTY(QVariantList bands READ bands NOTIFY bandsChanged)
    Q_PROPERTY(QVariantList response READ response NOTIFY bandsChanged)
    Q_PROPERTY(double peakGain READ peakGain NOTIFY bandsChanged)
    Q_PROPERTY(int maxBands READ maxBands CONSTANT)
    Q_PROPERTY(QStringList filterTypes READ filterTypes CONSTANT)

    Q_PROPERTY(QStringList builtinProfiles READ builtinProfiles CONSTANT)
    Q_PROPERTY(QStringList userProfiles READ userProfiles NOTIFY profilesChanged)
    Q_PROPERTY(QString currentProfile READ currentProfile NOTIFY currentProfileChanged)

    Q_PROPERTY(QVariantList autoEqResults READ autoEqResults NOTIFY autoEqResultsChanged)
    Q_PROPERTY(bool autoEqBusy READ autoEqBusy NOTIFY autoEqStateChanged)
    Q_PROPERTY(QString autoEqStatus READ autoEqStatus NOTIFY autoEqStateChanged)

public:
    explicit EqController(sonkore::SuperEQ& eq, QObject* parent = nullptr);
    ~EqController() override;
    void flush() const { saveState(); }

    bool enabled() const { return m_enabled; }
    void setEnabled(bool on);
    QString mode() const { return m_mode; }
    void setMode(const QString& mode);
    double preamp() const { return m_preamp; }
    void setPreamp(double db);
    QVariantList bands() const;
    QVariantList response() const;
    double peakGain() const;
    int maxBands() const { return sonkore::SuperEQ::kMaxBands; }
    QStringList filterTypes() const;

    QStringList builtinProfiles() const;
    QStringList userProfiles() const;
    QString currentProfile() const { return m_currentProfile; }

    QVariantList autoEqResults() const { return m_autoEqResults; }
    bool autoEqBusy() const { return m_autoEqBusy; }
    QString autoEqStatus() const { return m_autoEqStatus; }

    Q_INVOKABLE void setGain(int index, double db);
    // Standard frequency of a graphic band (31 Hz .. 16 kHz), 0 for others.
    Q_INVOKABLE double graphicFrequency(int index) const;
    Q_INVOKABLE void setBand(int index, const QString& type, double freq, double gain, double q, bool enabled);
    Q_INVOKABLE void addBand();
    Q_INVOKABLE void removeBand(int index);
    Q_INVOKABLE void reset();
    Q_INVOKABLE void autoPreamp();
    Q_INVOKABLE QString typeLabel(const QString& type) const;
    Q_INVOKABLE QString frequencyLabel(double hz) const;

    Q_INVOKABLE void loadProfile(const QString& name);
    Q_INVOKABLE QString saveProfile(const QString& name);
    Q_INVOKABLE void deleteProfile(const QString& name);
    Q_INVOKABLE bool importFile(const QUrl& file);
    Q_INVOKABLE bool exportFile(const QUrl& file) const;

    Q_INVOKABLE void autoEqSearch(const QString& text);
    Q_INVOKABLE void autoEqRefreshIndex();
    Q_INVOKABLE void autoEqApply(int resultIndex);

signals:
    void enabledChanged();
    void bandsChanged();
    void profilesChanged();
    void currentProfileChanged();
    void autoEqResultsChanged();
    void autoEqStateChanged();
    void message(const QString& text);

private:
    struct Band {
        QString type = QStringLiteral("PK");
        double freq = 1000.0;
        double gain = 0.0;
        double q = 1.41421356237;
        bool enabled = true;
    };
    struct Profile {
        QString name;
        QString mode;
        double preamp = 0.0;
        QVector<Band> bands;
    };
    struct AutoEqEntry {
        QString name, path, source, rig, searchText;
    };

    static QVector<Band> graphicLayout(const QVector<double>& gains = {});
    static sonkore::FilterType toFilterType(const QString& type);
    static QVariantMap bandToMap(const Band& b);
    static Band bandFromMap(const QVariantMap& m);
    static bool parseParametric(const QString& text, double& preamp, QVector<Band>& bands);
    double responseAt(double hz) const;

    void apply(bool markCustom = true);
    void setCurrentProfile(const QString& name);
    void loadState();
    void saveState() const;
    void loadUserProfiles();
    void saveUserProfiles() const;
    void parseAutoEqIndex(const QByteArray& markdown);
    void setAutoEqState(bool busy, const QString& status);

    sonkore::SuperEQ& m_eq;
    bool m_enabled = true;
    QString m_mode = QStringLiteral("graphic");
    double m_preamp = 0.0;
    QVector<Band> m_bands;
    QString m_currentProfile;
    QVector<Profile> m_userProfiles;

    QNetworkAccessManager m_network;
    QVector<AutoEqEntry> m_autoEqIndex;
    QVector<int> m_autoEqMatches;
    QVariantList m_autoEqResults;
    QString m_pendingSearch;
    bool m_autoEqBusy = false;
    QString m_autoEqStatus;
};
