#include "CueSheet.hpp"

#include <algorithm>
#include <cctype>
#include <charconv>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <sstream>

namespace sonkore {

namespace {

constexpr std::string_view kMarker = ".cue#";

std::string lower(std::string s)
{
    std::transform(s.begin(), s.end(), s.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return s;
}

bool validUtf8(const std::string& s)
{
    for (std::size_t i = 0; i < s.size();) {
        const auto c = static_cast<unsigned char>(s[i]);
        const int extra = c < 0x80 ? 0 : (c >> 5) == 0x6 ? 1 : (c >> 4) == 0xE ? 2 : (c >> 3) == 0x1E ? 3 : -1;
        if (extra < 0 || i + extra >= s.size())
            return false;
        for (int k = 1; k <= extra; ++k)
            if ((static_cast<unsigned char>(s[i + k]) >> 6) != 0x2)
                return false;
        i += extra + 1;
    }
    return true;
}

// Windows-1252 (a superset of Latin-1 for text) to UTF-8.
std::string fromCp1252(const std::string& s)
{
    static const char16_t high[32] = {0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
                                      0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
                                      0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178};
    std::string out;
    for (unsigned char c : s) {
        const char16_t u = c >= 0x80 && c < 0xA0 ? high[c - 0x80] : c;
        if (u < 0x80) {
            out += static_cast<char>(u);
        } else if (u < 0x800) {
            out += static_cast<char>(0xC0 | (u >> 6));
            out += static_cast<char>(0x80 | (u & 0x3F));
        } else {
            out += static_cast<char>(0xE0 | (u >> 12));
            out += static_cast<char>(0x80 | ((u >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (u & 0x3F));
        }
    }
    return out;
}

// Splits a line into words; "quoted strings" stay whole.
std::vector<std::string> words(const std::string& line)
{
    std::vector<std::string> out;
    std::size_t i = 0;
    while (i < line.size()) {
        while (i < line.size() && std::isspace(static_cast<unsigned char>(line[i])))
            ++i;
        if (i >= line.size())
            break;
        std::string w;
        if (line[i] == '"') {
            const std::size_t end = line.find('"', i + 1);
            w = line.substr(i + 1, end == std::string::npos ? std::string::npos : end - i - 1);
            i = end == std::string::npos ? line.size() : end + 1;
        } else {
            while (i < line.size() && !std::isspace(static_cast<unsigned char>(line[i])))
                w += line[i++];
        }
        out.push_back(std::move(w));
    }
    return out;
}

// "mm:ss:ff" (75 frames per second) to CD frames.
bool cdTime(const std::string& s, uint64_t& frames)
{
    unsigned m = 0, sec = 0, f = 0;
    char a = 0, b = 0;
    std::istringstream in(s);
    if (!(in >> m >> a >> sec >> b >> f) || a != ':' || b != ':' || sec >= 60 || f >= 75)
        return false;
    frames = (static_cast<uint64_t>(m) * 60 + sec) * 75 + f;
    return true;
}

// "-4.20" whatever the locale (strtod would read "-4" where the decimal
// separator is a comma).
double number(const std::string& s)
{
    double v = 0.0;
    const char* first = s.data();
    if (first != s.data() + s.size() && *first == '+')
        ++first;
    const auto [end, ec] = std::from_chars(first, s.data() + s.size(), v);
    return ec == std::errc() && end != first ? v : CueSheet::kNoGain;
}

// The FILE a sheet names, or the same image under another audio extension.
std::filesystem::path resolve(const std::filesystem::path& dir, const std::string& name)
{
    std::error_code ec;
    std::filesystem::path direct = dir / std::filesystem::path(name).filename();
    if (std::filesystem::is_regular_file(direct, ec))
        return direct;
    const std::string stem = lower(std::filesystem::path(name).stem().string());
    static const char* const kAudio[] = {".flac", ".wav", ".wv", ".ape", ".tta", ".m4a", ".aiff", ".aif",
                                         ".dsf", ".dff", ".mp3", ".ogg", ".opus", ".tak"};
    for (const auto& entry : std::filesystem::directory_iterator(dir, ec)) {
        if (!entry.is_regular_file(ec))
            continue;
        const std::filesystem::path& p = entry.path();
        if (lower(p.stem().string()) != stem)
            continue;
        const std::string ext = lower(p.extension().string());
        if (std::find(std::begin(kAudio), std::end(kAudio), ext) != std::end(kAudio))
            return p;
    }
    return {};
}

} // namespace

bool CueSheet::parse(const std::filesystem::path& sheet, CueSheet& out)
{
    std::ifstream in(sheet, std::ios::binary);
    if (!in)
        return false;
    std::string text((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    if (text.size() > 1 << 20)
        return false; // not a cue sheet
    if (text.rfind("\xEF\xBB\xBF", 0) == 0)
        text.erase(0, 3);
    if (!validUtf8(text))
        text = fromCp1252(text);

    out = {};
    const std::filesystem::path dir = sheet.has_parent_path() ? sheet.parent_path() : std::filesystem::path(".");
    std::filesystem::path file;
    Track* current = nullptr;
    std::istringstream lines(text);
    std::string line;
    while (std::getline(lines, line)) {
        const auto w = words(line);
        if (w.empty())
            continue;
        const std::string key = lower(w[0]);
        const std::string arg = w.size() > 1 ? w[1] : std::string();
        if (key == "file") {
            file = resolve(dir, arg);
            current = nullptr;
        } else if (key == "track") {
            out.tracks.push_back({});
            current = &out.tracks.back();
            current->number = std::atoi(arg.c_str());
            current->file = file;
            current->start = UINT64_MAX; // until its INDEX 01
        } else if (key == "index" && current && w.size() > 2 && std::atoi(arg.c_str()) == 1) {
            cdTime(w[2], current->start);
        } else if (key == "title") {
            (current ? current->title : out.title) = arg;
        } else if (key == "performer") {
            (current ? current->performer : out.performer) = arg;
        } else if (key == "rem" && w.size() > 2) {
            const std::string what = lower(arg);
            const double v = number(w[2]);
            if (what == "genre") out.genre = w[2];
            else if (what == "date") out.date = w[2];
            else if (what == "replaygain_album_gain") out.albumGain = v;
            else if (what == "replaygain_album_peak") out.albumPeak = v;
            else if (what == "replaygain_track_gain" && current) current->gain = v;
            else if (what == "replaygain_track_peak" && current) current->peak = v;
        }
    }
    // Tracks without a playable file or an INDEX 01 cannot be played.
    out.tracks.erase(std::remove_if(out.tracks.begin(), out.tracks.end(),
                                    [](const Track& t) { return t.file.empty() || t.start == UINT64_MAX; }),
                     out.tracks.end());
    // A track ends where the next one in the same file begins (its pregap,
    // INDEX 00, stays with the track before it: playing them in turn is the
    // whole disc without a gap).
    for (std::size_t i = 0; i + 1 < out.tracks.size(); ++i)
        if (out.tracks[i + 1].file == out.tracks[i].file && out.tracks[i + 1].start > out.tracks[i].start)
            out.tracks[i].end = out.tracks[i + 1].start;
    return !out.tracks.empty();
}

const CueSheet::Track* CueSheet::track(int number) const
{
    for (const Track& t : tracks)
        if (t.number == number)
            return &t;
    return nullptr;
}

bool CueSheet::isTrackPath(std::string_view path)
{
    const auto at = lower(std::string(path)).rfind(kMarker);
    return at != std::string::npos && at + kMarker.size() < path.size();
}

bool CueSheet::splitTrackPath(std::string_view path, std::filesystem::path& sheet, int& number)
{
    const auto at = lower(std::string(path)).rfind(kMarker);
    if (at == std::string::npos)
        return false;
    const std::string digits(path.substr(at + kMarker.size()));
    if (digits.empty() || !std::all_of(digits.begin(), digits.end(), [](unsigned char c) { return std::isdigit(c); }))
        return false;
    sheet = std::filesystem::path(std::string(path.substr(0, at + 4)));
    number = std::atoi(digits.c_str());
    return true;
}

std::string CueSheet::trackPath(const std::filesystem::path& sheet, int number)
{
    return sheet.string() + "#" + std::to_string(number);
}

bool CueSheet::isSheet(const std::filesystem::path& path)
{
    return lower(path.extension().string()) == ".cue";
}

} // namespace sonkore
