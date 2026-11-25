#!/usr/bin/env python3
"""Deletes what target/ keeps from old builds: cargo never cleans up.

Every change of features, flags or dependencies leaves another copy of the
crates it touches (a new hash), and the old ones stay there forever: target/
went past 59 GB. This runs the builds the tools use (check.sh, ui-tour.sh,
appimage.sh --rapido, check-windows.sh, clippy) with --message-format=json; cargo
lists every artifact it uses, also the fresh ones, and what is not on that list
is deleted. Deleting too much is never wrong, just slower: cargo rebuilds what
is missing.

    tools/limpiar-target.py             # build what is used, delete the rest
    tools/limpiar-target.py --probar    # only say what it would delete
    tools/limpiar-target.py --release   # also keep the release build (publishing)
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "target"
HASH = re.compile(r"-([0-9a-f]{16})(?:\.|$)")
DRY = "--probar" in sys.argv
KEEP_RELEASE = "--release" in sys.argv

BUILDS = [
    (["cargo", "build", "-p", "sonkore"], {}),
    (["cargo", "test", "--workspace", "--no-run"], {}),
    # check.sh's clippy step, with its exact arguments (others recheck it all).
    (["cargo", "clippy", "--keep-going", "--workspace", "--all-targets"], {}),
    (["cargo", "build", "-p", "sonkore-library", "--example", "scan"], {}),
    (["cargo", "build", "--profile", "diario", "-p", "sonkore"], {}),
    (["cargo", "build", "--release", "--manifest-path", str(ROOT / "tools/mkappimage/Cargo.toml")],
     {"CARGO_TARGET_DIR": str(TARGET)}),
    ([str(ROOT / "tools/check-windows.sh")], {}),
]
if KEEP_RELEASE:
    BUILDS.append((["cargo", "build", "--release", "-p", "sonkore"], {}))


def used_hashes():
    """Hashes of every artifact the builds use. A binary is reported by its
    uplifted name (target/debug/sonkore), a hard link to deps/sonkore-<hash>:
    the inodes catch those."""
    used = set()
    inodes = set()
    for cmd, env in BUILDS:
        start = time.time()
        cut = cmd.index("--") if "--" in cmd else len(cmd)  # cargo's options before "--"
        out = subprocess.run(cmd[:cut] + ["--message-format=json"] + cmd[cut:], cwd=ROOT, env={**os.environ, **env},
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        if out.returncode != 0:
            sys.exit(f"falló {' '.join(cmd)}: no borro nada")
        for line in out.stdout.splitlines():
            if not line.startswith("{"):
                continue
            msg = json.loads(line)
            paths = msg.get("filenames") or []
            if msg.get("executable"):
                paths.append(msg["executable"])
            if msg.get("out_dir"):
                paths.append(msg["out_dir"])
            for p in paths:
                try:
                    inodes.add(os.stat(p).st_ino)
                except OSError:
                    pass
                for part in Path(p).parts:
                    m = HASH.search(part)
                    if m:
                        used.add(m.group(1))
        print(f"  {time.time() - start:5.0f} s  {' '.join(Path(c).name if '/' in c else c for c in cmd)}")
    for sub in ("deps", "examples"):
        for f in TARGET.glob(f"**/{sub}/*"):
            m = HASH.search(f.name)
            if m and f.is_file() and f.stat().st_ino in inodes:
                used.add(m.group(1))
    return used


def size(path):
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    return sum(f.lstat().st_size for f in path.rglob("*") if not f.is_symlink() and f.is_file())


def stale_incremental(profile, started):
    """incremental/<crate>-<id>/ has no cargo hash: rustc names it. Inside,
    only the newest finished session is ever read (rustc leaves the previous
    one behind). A whole directory goes when this run did not touch it but
    rebuilt the same crate elsewhere (an old configuration), or after a week."""
    stale = []
    dirs = [d for d in (profile / "incremental").glob("*") if d.is_dir()]
    newest = {d: max([d.stat().st_mtime] + [s.stat().st_mtime for s in d.iterdir()]) for d in dirs}
    crate = lambda d: d.name.rsplit("-", 1)[0]
    touched = {crate(d) for d in dirs if newest[d] >= started}
    for d in dirs:
        if newest[d] < started and (crate(d) in touched or newest[d] < started - 7 * 86400):
            stale.append(d)
            continue
        sessions = sorted((s for s in d.iterdir() if s.is_dir() and not s.name.endswith("-working")),
                          key=lambda s: s.stat().st_mtime)
        for old in sessions[:-1]:
            stale.append(old)
            lock = d / (old.name.rsplit("-", 1)[0] + ".lock")
            if lock.exists():
                stale.append(lock)
    return stale


def main():
    print("compilando lo que usan las herramientas…")
    started = time.time() - 5
    used = used_hashes()
    profiles = sorted({fp.parent for fp in TARGET.glob("**/.fingerprint") if fp.is_dir()})
    freed = 0
    for profile in profiles:
        stale = []
        kept = 0
        for sub in ("deps", "build", ".fingerprint", "examples"):
            for entry in (profile / sub).glob("*"):
                m = HASH.search(entry.name)
                if not m:
                    continue
                if m.group(1) in used:
                    kept += 1
                else:
                    stale.append(entry)
        if stale and not kept:
            print(f"  ¡{profile.relative_to(TARGET)}: nada coincide, no lo toco!")
            continue
        stale += stale_incremental(profile, started)
        bytes_ = sum(size(e) for e in stale)
        freed += bytes_
        print(f"  {bytes_ / 2**30:5.1f} GB  {profile.relative_to(TARGET)}  ({len(stale)} viejos, {kept} en uso)")
        if not DRY:
            for e in stale:
                if e.is_dir() and not e.is_symlink():
                    shutil.rmtree(e)
                else:
                    e.unlink()
    print(f"{'se liberarían' if DRY else 'liberados'}: {freed / 2**30:.1f} GB")


main()
