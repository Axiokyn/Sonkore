#!/usr/bin/env python3
"""Refreshes i18n/sonkore_<lang>.ts from the sources (lupdate) and fills
them from translations_*.py. Run after changing UI text:
    python3 i18n/update.py [path/to/lupdate]
Reports strings that still need a translation."""
import importlib.util, pathlib, re, subprocess, sys
import xml.etree.ElementTree as ET

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
LANGS = ["es", "pt_BR", "fr", "de", "it"]
LUPDATE = sys.argv[1] if len(sys.argv) > 1 else str(pathlib.Path.home() / "Qt/6.11.3/gcc_64/bin/lupdate")

table = {}
for part in sorted(HERE.glob("translations_*.py")):
    spec = importlib.util.spec_from_file_location(part.stem, part)
    mod = importlib.util.module_from_spec(spec); spec.loader.exec_module(mod)
    table.update(mod.T)

placeholder = re.compile(r"%\d")
problems = 0
for src, tr in table.items():
    assert len(tr) == len(LANGS), src
    for lang, text in zip(LANGS, tr):
        if sorted(placeholder.findall(src)) != sorted(placeholder.findall(text)):
            print(f"placeholder mismatch [{lang}]: {src!r} -> {text!r}"); problems += 1

missing = set()
for i, lang in enumerate(LANGS):
    ts = HERE / f"sonkore_{lang}.ts"
    subprocess.run([LUPDATE, "-silent", "-locations", "none", "-no-obsolete", "-source-language", "en",
                    "-target-language", lang, str(ROOT / "qml"), str(ROOT / "src"), "-ts", str(ts)], check=True)
    tree = ET.parse(ts)
    for msg in tree.getroot().iter("message"):
        src = msg.find("source").text
        t = msg.find("translation")
        if src in table:
            t.text = table[src][i]
            t.attrib.pop("type", None)
        else:
            missing.add(src)
    tree.write(ts, encoding="utf-8", xml_declaration=True)

for m in sorted(missing):
    print("untranslated:", repr(m))
unused = set(table) - {m.find("source").text for m in ET.parse(HERE / "sonkore_es.ts").getroot().iter("message")}
for u in sorted(unused):
    print("unused:", repr(u))
print(f"{len(table)} strings, {len(missing)} untranslated, {problems} placeholder problems")
sys.exit(1 if missing or problems else 0)
