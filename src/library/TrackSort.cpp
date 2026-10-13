#include "TrackSort.hpp"


namespace TrackSort {

QCollator collator()
{
    QCollator c;
    c.setNumericMode(true);
    c.setCaseSensitivity(Qt::CaseInsensitive);
    return c;
}

QStringList keysFor(const QString& field)
{
    QStringList keys{field};
    if (field == u"genre")
        keys << QStringLiteral("artist");
    if (field == u"artist" || field == u"albumArtist" || field == u"genre" || field == u"year")
        keys << QStringLiteral("album");
    if (field == u"artist" || field == u"albumArtist" || field == u"genre" || field == u"year" || field == u"album")
        keys << QStringLiteral("discNo") << QStringLiteral("trackNo");
    if (field != u"path")
        keys << QStringLiteral("path");
    return keys;
}

int comparePaths(const QCollator& collator, const QString& a, const QString& b)
{
    // Walks both paths a folder at a time, in place: no lists, no copies
    // (it runs for every comparison of a sort).
    const auto next = [](const QString& p, qsizetype& at) {
        while (at < p.size() && p[at] == u'/')
            ++at; // empty segments ("//", the leading "/")
        const qsizetype start = at;
        const qsizetype slash = p.indexOf(u'/', at);
        at = slash < 0 ? p.size() : slash;
        return QStringView(p).sliced(start, at - start);
    };
    const auto last = [](const QString& p, qsizetype at) {
        while (at < p.size() && p[at] == u'/')
            ++at;
        return at >= p.size();
    };
    qsizetype i = 0, j = 0;
    while (!last(a, i) && !last(b, j)) {
        const QStringView x = next(a, i), y = next(b, j);
        const bool xFile = last(a, i), yFile = last(b, j);
        if (xFile != yFile)
            return xFile ? 1 : -1; // a sub-folder before the files beside it
        if (const int c = collator.compare(x, y))
            return c;
    }
    return last(a, i) == last(b, j) ? 0 : (last(a, i) ? -1 : 1);
}

int compareKey(const QCollator& collator, const QString& key, const QVariant& a, const QVariant& b, bool descending)
{
    if (key == u"path")
        return comparePaths(collator, a.toString(), b.toString());
    const bool text = a.typeId() == QMetaType::QString || b.typeId() == QMetaType::QString;
    // A missing number is stored as 0 (no year, no track number).
    const bool zeroIsBlank = key == u"year" || key == u"trackNo" || key == u"discNo";
    const auto blank = [&](const QVariant& v) {
        return text ? v.toString().isEmpty() : !v.isValid() || (zeroIsBlank && v.toDouble() == 0.0);
    };
    const bool blankA = blank(a), blankB = blank(b);
    if (blankA != blankB)
        return blankA != descending ? 1 : -1;
    if (blankA)
        return 0;
    if (text)
        return collator.compare(a.toString(), b.toString());
    const double x = a.toDouble(), y = b.toDouble();
    return x < y ? -1 : (x > y ? 1 : 0);
}

} // namespace TrackSort
