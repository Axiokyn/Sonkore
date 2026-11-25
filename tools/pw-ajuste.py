#!/usr/bin/env python3
"""A key of the test PipeWire's "settings" metadata (clock.force-rate…),
checked from a scenario (tools/escenarios/sonido-*.txt):

    sh XDG_RUNTIME_DIR={rt} pw-dump | python3 {repo}/tools/pw-ajuste.py clock.force-rate 44100

Fails (exit 1, saying what it found) unless the key has that value; "0" also
matches a key that is not there.
"""
import json
import sys

key, want = sys.argv[1], sys.argv[2]
got = None
for obj in json.load(sys.stdin):
    if obj.get("type", "").endswith("Metadata") and obj.get("props", {}).get("metadata.name") == "settings":
        for entry in obj.get("metadata") or []:
            if entry.get("subject") == 0 and entry.get("key") == key:
                got = str(entry.get("value"))
if (got if got is not None else "0") != want:
    sys.exit(f"{key}={got}, se esperaba {want}")
