#pragma once

#include <QNetworkReply>
#include <QString>

#include <algorithm>

namespace sonkore {

// How long to wait before asking again after a "too many requests" (429) or
// "busy" (503) answer: 1, 2, 4, 8, 16 s, or longer when the server's
// Retry-After (in seconds) says so. -1 for any other outcome, or once
// `maxAttempts` retries were made. (LRCLIB answers 429 with "Retry-After: 1"
// to a good share of single requests when it is loaded, and a burst can
// last several seconds.)
inline int retryDelayMs(QNetworkReply* reply, int attempt, int maxAttempts = 5)
{
    const int status = reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
    if ((status != 429 && status != 503) || attempt >= maxAttempts)
        return -1;
    bool ok = false;
    const int after = reply->rawHeader("Retry-After").trimmed().toInt(&ok);
    const int seconds = std::min(std::max(ok ? after : 0, 1 << attempt), 60);
    return seconds * 1000;
}

// The HTTP status of a failed reply, 0 when no answer came at all.
inline int httpStatus(QNetworkReply* reply)
{
    return reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
}

} // namespace sonkore
