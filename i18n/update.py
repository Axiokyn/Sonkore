#!/usr/bin/env python3
"""Brings i18n/<lang>.po up to date with the code and fills them from
translations_*.py. Run after changing interface text:
    python3 i18n/update.py
Every text the app translates (@tr("Context" => "Text") in the .slint files,
tr/trf("Context", "Text") in the Rust code) gets an entry under its context
(msgctxt) in each .po, and its translation from the tables (keyed by the
English text). Nothing is removed: some texts are translated from data
(column titles, theme hints…), not from a literal the code shows.
Placeholders are Slint's: {0}, {1}…; literal braces are doubled.
build.rs bundles the .po files; tools/textos-sin-traducir.py checks them."""
import glob
import importlib.util
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
LANGS = ["es", "pt_BR", "fr", "de", "it"]
HEADER = 'msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n\n'


def unescape(text):
    """A Slint or Rust string literal's body as the text it stands for."""
    return re.sub(r'\\(.)', lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)), text)


def used():
    """(context, msgid) of every translated text in the code, in a stable order."""
    found = []
    sources = [
        # Slint's @tr already writes literal braces doubled.
        (r'@tr\("([^"]+)"\s*=>\s*"((?:[^"\\]|\\.)*)"', "crates/app/ui/*.slint", lambda t: t),
        # Rust texts write them single.
        (r'\btrf?\(\s*"([^"]+)",\s*"((?:[^"\\]|\\.)*)"', "crates/app/src/*.rs",
         lambda t: re.sub(r"\{(?!\d+\})|(?<!\{\d)\}", lambda m: m.group(0) * 2, t)),
    ]
    for pattern, files, braces in sources:
        for f in sorted(glob.glob(str(ROOT / files))):
            for m in re.finditer(pattern, pathlib.Path(f).read_text(encoding="utf-8")):
                pair = (m.group(1), braces(unescape(m.group(2))))
                if pair not in found:
                    found.append(pair)
    return found


def po_string(lines):
    return "".join(unescape(re.match(r'^[a-z]*\s*"(.*)"$', l).group(1)) for l in lines)


def read_po(path):
    """[(msgctxt, msgid, msgstr)] in file order, without the header."""
    entries = []
    for block in path.read_text(encoding="utf-8").split("\n\n"):
        fields, key = {}, None
        for line in block.strip().splitlines():
            if line.startswith("#"):
                continue
            word = line.split(" ", 1)[0]
            if word in ("msgctxt", "msgid", "msgstr"):
                key = word
            fields.setdefault(key, []).append(line)
        if fields.get("msgid") and po_string(fields["msgid"]):
            entries.append((po_string(fields.get("msgctxt", [])), po_string(fields["msgid"]), po_string(fields.get("msgstr", []))))
    return entries


def quote(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


def write_po(path, entries):
    path.write_text(HEADER + "".join(f"msgctxt {quote(c)}\nmsgid {quote(i)}\nmsgstr {quote(t)}\n\n" for c, i, t in entries),
                    encoding="utf-8")


def main():
    table = {}
    for part in sorted(HERE.glob("translations_*.py")):
        spec = importlib.util.spec_from_file_location(part.stem, part)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        table.update(mod.T)

    placeholder = re.compile(r"\{\d+\}")
    problems = 0
    for src, tr in table.items():
        assert len(tr) == len(LANGS), src
        for lang, text in zip(LANGS, tr):
            if sorted(placeholder.findall(src)) != sorted(placeholder.findall(text)):
                print(f"placeholder mismatch [{lang}]: {src!r} -> {text!r}")
                problems += 1

    pairs = used()
    missing, added = set(), 0
    for i, lang in enumerate(LANGS):
        path = HERE / f"{lang}.po"
        entries = read_po(path)
        have = {(c, m) for c, m, _ in entries}
        for pair in pairs:
            if pair not in have:
                entries.append((*pair, ""))
                added += 1
        filled = []
        for ctx, msgid, text in entries:
            if msgid in table:
                text = table[msgid][i]
            elif (ctx, msgid) in pairs and not text:
                missing.add(msgid)
            filled.append((ctx, msgid, text))
        write_po(path, filled)

    for m in sorted(missing):
        print("untranslated:", repr(m))
    print(f"{len(pairs)} texts in the code, {added} entries added, {len(missing)} untranslated, {problems} placeholder problems")
    return 1 if missing or problems else 0


if __name__ == "__main__":
    sys.exit(main())
