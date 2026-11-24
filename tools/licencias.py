#!/usr/bin/env python3
"""The licenses of every crate built into the app (Linux and Windows), from
`cargo metadata`, grouped by license: for THIRD-PARTY.md.
    tools/licencias.py
"""
import collections
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def runtime(platform):
    out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--offline", "--filter-platform", platform],
                         cwd=ROOT, capture_output=True, text=True, check=True).stdout
    m = json.loads(out)
    pkgs = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    seen, todo = set(), [next(p["id"] for p in m["packages"] if p["name"] == "sonkore")]
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        todo += [d["pkg"] for d in nodes[i]["deps"] if any(k["kind"] is None for k in d["dep_kinds"])]
    return {pkgs[i]["name"]: pkgs[i].get("license") for i in seen if not pkgs[i]["name"].startswith("sonkore")}


crates = {**runtime("x86_64-unknown-linux-gnu"), **runtime("x86_64-pc-windows-msvc")}
by = collections.defaultdict(list)
for name, lic in sorted(crates.items()):
    by[lic].append(name)
print(f"{len(crates)} crates")
for lic, names in sorted(by.items(), key=lambda x: (-len(x[1]), str(x[0]))):
    print(f"| {lic} | {len(names)} | {', '.join(names)} |")
