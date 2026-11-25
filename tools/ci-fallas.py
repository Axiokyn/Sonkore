#!/usr/bin/env python3
"""Runs `cargo test` and turns each failure into a GitHub annotation.

The CI's logs need a signed-in account, but a run's annotations can be read
by anyone through the API (check-runs/<job>/annotations): with this, the
failing tests of a run, with their message and file:line, can be seen from
the sandbox without downloading the log.

    tools/ci-fallas.py -- cargo test --workspace --no-fail-fast
    tools/ci-fallas.py --log LOG          (only reads a saved log)
    tools/ci-fallas.py --corrida [ID] [--avisos]
                                          (a run's failed jobs and their
                                           annotations; the latest without ID;
                                           --avisos: also every job's notices
                                           and warnings)

The command's output goes through unchanged and its exit code is returned.
Without GITHUB_ACTIONS the annotations are printed all the same, to try it.
"""
import os
import re
import subprocess
import sys
from collections import deque

ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The time stamp a downloaded log puts on each line.
STAMP = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z ")
HEADER = re.compile(r"^---- (.+) stdout ----$")
PANIC = re.compile(r"panicked at ([^:]+(?::[\\/][^:]+)?):(\d+):\d+:?$")
RERUN = re.compile(r"^error: test failed, to rerun pass `([^`]*)`")
COMPILE = re.compile(r"^error(\[E\d+\])?: ")
# cargo's own lines about failed tests, not compile errors.
CARGO = re.compile(r"^error: (test failed|\d+ targets? failed|could not compile)")
BINARY = re.compile(r"^\s+(Running|Doc-tests) ")
DETAILED = 9  # GitHub keeps 10 error annotations per step: 1 summary + 9
BLOCK_LINES = 30


class Parser:
    def __init__(self):
        self.failures = []  # dicts: name, lines, rerun, file, line
        self.pending = []  # failures whose binary is not known yet
        self.block = None
        self.recent = deque(maxlen=15)
        self.compile = []  # (first line, following lines)
        self.in_compile = None
        self.follow = None  # lines that go on after a crashed binary

    def feed(self, raw):
        line = STAMP.sub("", ANSI.sub("", raw)).rstrip("\r\n")
        if BINARY.match(line):
            self.recent.clear()
            self.follow = None
        elif self.follow is not None and len(self.follow) < 25:
            self.follow.append(line)
        if self.in_compile is not None:
            if line.strip() and len(self.in_compile[1]) < 20:
                self.in_compile[1].append(line)
            else:
                self.in_compile = None
        m = HEADER.match(line)
        if m:
            self._close()
            self.block = {"name": m.group(1), "lines": [], "rerun": "", "file": "", "line": 0}
        elif self.block is not None and line in ("failures:", "successes:"):
            self._close()
        elif self.block is not None:
            self.block["lines"].append(line)
            p = PANIC.search(line)
            if p and not self.block["file"]:
                self.block["file"] = p.group(1).replace("\\", "/")
                self.block["line"] = int(p.group(2))
        m = RERUN.match(line)
        if m:
            self._close()
            if self.pending:
                for f in self.pending:
                    f["rerun"] = m.group(1)
                self.pending = []
            else:
                # The binary ended without a report: a crash or an abort.
                lines = list(self.recent) + [line]
                self.failures.append({"name": m.group(1), "lines": lines,
                                      "rerun": m.group(1), "file": "", "line": 0})
                self.follow = lines
        elif COMPILE.match(line) and not CARGO.match(line):
            self.in_compile = (line, [])
            self.compile.append(self.in_compile)
        self.recent.append(line)

    def _close(self):
        if self.block is None:
            return
        lines = self.block["lines"]
        while lines and not lines[0].strip():
            lines.pop(0)
        while lines and not lines[-1].strip():
            lines.pop()
        lines[:] = [l for l in lines if not l.startswith("note: run with `RUST_BACKTRACE")]
        if len(lines) > BLOCK_LINES:
            lines[BLOCK_LINES - 1:] = [f"… ({len(lines) - BLOCK_LINES + 1} more lines)"]
        self.failures.append(self.block)
        self.pending.append(self.block)
        self.block = None

    def finish(self):
        self._close()


def escape(s, prop=False):
    s = s.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    if prop:
        s = s.replace(":", "%3A").replace(",", "%2C")
    return s


def annotations(p):
    out = []
    compile_errors = p.compile
    if not p.failures and not compile_errors:
        return out
    summary = [f"{len(p.failures)} failing test(s)"]
    for f in p.failures:
        where = f" ({f['rerun']})" if f["rerun"] and f["rerun"] != f["name"] else ""
        summary.append(f"- {f['name']}{where}")
    if compile_errors:
        summary.append(f"{len(compile_errors)} compile error(s)")
        summary += [f"- {c[0]}" for c in compile_errors[:20]]
    out.append(f"::error title={escape('Tests: summary', True)}::{escape(chr(10).join(summary))}")
    items = [(f"{f['name']} [{f['rerun']}]" if f["rerun"] and f["rerun"] != f["name"] else f["name"],
              "\n".join(f["lines"]), f["file"], f["line"]) for f in p.failures]
    items += [(c[0][:120], "\n".join([c[0]] + c[1]), "", 0) for c in compile_errors]
    for title, body, path, line in items[:DETAILED]:
        loc = f"file={escape(path, True)},line={line}," if path else ""
        out.append(f"::error {loc}title={escape(title, True)}::{escape(body[:4000])}")
    return out


def api(path):
    import json
    import urllib.request
    req = urllib.request.Request("https://api.github.com/repos/" + REPO + path,
                                 headers={"Accept": "application/vnd.github+json",
                                          "User-Agent": "sonkore-ci-fallas"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


NOISE = ("Node.js 20 is deprecated", "No files were found with the provided path")


def show_run(run_id, notices=False):
    if run_id is None:
        run = api("/actions/runs?per_page=1")["workflow_runs"][0]
    else:
        run = api(f"/actions/runs/{run_id}")
    print(f"{run['id']} {run['head_sha'][:7]} {run['status']} {run['conclusion']}  "
          f"{run['display_title']}\n{run['html_url']}")
    jobs = api(f"/actions/runs/{run['id']}/jobs?per_page=100")["jobs"]
    for j in jobs:
        bad = [s["name"] for s in j.get("steps", [])
               if s["conclusion"] not in ("success", "skipped", None)]
        print(f"  {j['conclusion'] or j['status']:<10} {j['name'][:70]}"
              + (f"  ← {', '.join(bad)}" if bad else ""))
    for j in jobs:
        failed = j["conclusion"] in ("failure", "cancelled", "timed_out")
        if not failed and not notices:
            continue
        for a in api(f"/check-runs/{j['id']}/annotations?per_page=50"):
            if a["annotation_level"] != "failure" and not notices:
                continue
            if any(n in a["message"] for n in NOISE):
                continue
            where = f"{a['path']}:{a['start_line']}" if a["path"] != ".github" else ""
            print(f"\n── {j['name'][:40]}: {a.get('title') or ''} {where}")
            print(a["message"])


REPO = "Axiokyn/Sonkore"


def main():
    args = sys.argv[1:]
    if args[:1] == ["--corrida"]:
        rest = [a for a in args[1:] if a != "--avisos"]
        show_run(rest[0] if rest else None, notices="--avisos" in args)
        return
    p = Parser()
    out = sys.stdout.buffer
    if args[:1] == ["--log"] and len(args) == 2:
        with open(args[1], "rb") as f:
            for raw in f:
                p.feed(raw.decode("utf-8", "replace"))
        code = 0
    else:
        if args[:1] == ["--"]:
            args = args[1:]
        if not args:
            sys.exit(__doc__)
        proc = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        for raw in proc.stdout:
            out.write(raw)
            out.flush()
            p.feed(raw.decode("utf-8", "replace"))
        code = proc.wait()
    p.finish()
    for a in annotations(p):
        out.write((a + "\n").encode("utf-8"))
    out.flush()
    sys.exit(code)


if __name__ == "__main__":
    main()
