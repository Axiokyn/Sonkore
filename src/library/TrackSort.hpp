#pragma once

#include <QCollator>
#include <QString>
#include <QStringList>
#include <QVariant>

// The one way tracks are ordered by a field, shared by the track lists
// (TrackFilterModel) and the play queue (DspController::sortQueue), so the
// same choice gives the same order everywhere.
namespace TrackSort {

// Natural order: "2" before "10", case and accents aside.
QCollator collator();

// The field, then what settles ties: an artist's tracks by album, disc and
// track, an album's by disc and track…, and always the path last.
QStringList keysFor(const QString& field);

// Folder order: down the folders one by one, each folder's sub-folders (with
// all they hold) before the files beside them.
int comparePaths(const QCollator& collator, const QString& a, const QString& b);

// One key of two tracks: numbers as numbers, text in natural order, paths
// as folders. Blanks (and a year or number of 0) go last, either way, so
// `descending` only says which way "last" is.
int compareKey(const QCollator& collator, const QString& key, const QVariant& a, const QVariant& b, bool descending);

} // namespace TrackSort
