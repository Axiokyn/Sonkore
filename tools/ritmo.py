#!/usr/bin/env python3
"""How fast the track advances (seconds of track per real second), from the
app's probe (SONKORE_STATE_FILE: position=), checked from a scenario:

    sh python3 {repo}/tools/ritmo.py {rt}/state 1.5

Measures over 4 s (a track that loops in between is measured again) and
fails unless the speed is within 10 % of the one given (the probe rounds
the position to 0.1 s; a loaded machine lags a little).
"""
import sys
import time


def position(path):
    for line in open(path):
        if line.startswith("position="):
            return float(line.split("=", 1)[1])
    return 0.0


path, want = sys.argv[1], float(sys.argv[2])
for _ in range(3):
    t0, p0 = time.time(), position(path)
    time.sleep(4)
    t1, p1 = time.time(), position(path)
    if p1 >= p0:
        break
speed = (p1 - p0) / (t1 - t0)
if abs(speed - want) > 0.10 * want:
    sys.exit(f"avanza a {speed:.2f}×, se esperaba {want}×")
