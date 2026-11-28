#!/usr/bin/env python3
"""Drives Sonkore's real window without a display: a headless Wayland
compositor (tools/wl-headless), a PipeWire of its own with a null sink
(tests/pipewire: nothing reaches the desktop's sound) and the app, then runs
scenario files and checks what happened.

    tools/ui-live.py [options] scenario.txt...   (tools/escenarios/*.txt)
      --app PATH            the binary (default target/debug/sonkore, built first)
      --escalas 150,110     screen scales in % (default 150, the user's)
      --interfaz 1.1        Settings › Interface scale (default 1.0)
      --sin-audio           no PipeWire daemon (ALSA default silenced)
      --hilos N             runs at once (default: half the cores)

Scenario lines (coordinates in the app's logical pixels at Interface scale
100 %; multiplied by --interfaz):
    ajuste SECCION/CLAVE=VALOR  a setting in sonkore.conf before the app starts
    entorno CLAVE=VALOR         an environment variable for the app ({rt}: the
                                run's own folder, {repo}: the repository)
    preparar COMANDO…           a shell command before the app starts ({rt}, {repo})
    argumentos ARCHIVO…         files the app is started with (it plays them;
                                a path with spaces between quotes)
    requiere ARCHIVO            without it ({repo}…) the scenario is skipped
                                (an optional test corpus)
    pantalla W H                the screen's size in pixels (default 2546 1420):
                                with widgets/windows/settings=@Size(920 2700),
                                a whole Settings tab without scrolling
    sh COMANDO…                 a shell command, then and there ({rt}, {repo}
                                too, and {pid}: the app's process); it
                                failing fails the scenario
                                (any line of the file; e.g. appearance/backdrop=aurora)
    esperar S                   sleep
    mover X Y | clic X Y | dclic X Y | rclic X Y | rueda X Y DY
    arrastrar X Y               a press there must ask to move the window
    arrastre X0 Y0 X1 Y1        press, move, let go (sliders, the EQ's points)
    tecla [ctrl+][shift+]NAME   a key (names in wl-headless/headless.c)
    soltar-encima X Y RUTA…     files dragged over the window from a pretend file
                                manager ({rt}, {repo}; quotes for spaces); then
    soltar | soltar-fuera       drops them, or the drag leaves
    foco main|last|lasttop|N    which surface gets the input
    escala N                    the screen's scale, in %
    tamaño W H                  the compositor resizes the main window
    estado CLAVE=VALOR [S]      waits (S s, default 3) until the app's state
    estado CLAVE~REGEX [S]      (probe.rs) matches; CLAVE!=VALOR negates.
    estado CLAVE>N [S]          numeric (position>1.0)
    guardado SECCION/CLAVE=VALOR [S]  waits (S s, default 3) until sonkore.conf
                                holds it (what Settings saved; ~REGEX, !~REGEX
                                and != too)
    sonido [S] COND… [Ts]       waits (T s, default 5) until what the app sent to
                                its output over the last S s (default 1) meets
                                every COND: CLAVE=V, CLAVE=N±T, CLAVE<N, CLAVE>N,
                                CLAVE~REGEX, CLAVE!=V. Keys: salida tasa canales
                                bits flotante dsd dop salidas silencio duracion,
                                and nivel- pico- frec- desvio- preeco- with -i
                                (left) or -d (right); see sound() (DSD: nivel, pico and
                                frec from the 1-bit density, dsd_measures()).
                                A scenario using it gets SONKORE_OUTPUT_TAP
                                (crates/app/src/tap.rs).
                                Without COND it prints every measure.
    ventanas N [S]              waits until N toplevels are on screen
    latencia NOMBRE MAX_MS [ventana]  the next input must give a new frame
                                within MAX_MS (ventana: the first frame of a
                                new window or popup)
    cpu NOMBRE S [MAX]          the app's CPU over S seconds, in % of one core
                                (fails above MAX when given)
    perfil NOMBRE S             perf record of the app for S seconds; the top
                                functions in target/ui-live/<run>/NOMBRE.txt
    reiniciar                   waits for the app to quit (its close button
                                saves) and starts it again on the same profile
    se-reinicia                 the app restarts itself (Settings › Restart):
                                waits for it to quit cleanly and goes on with
                                the one it started
    captura NOMBRE              every surface to target/ui-live/<run>/NOMBRE-<k>.png
    centrado NOMBRE Y X0 X1     in capture NOMBRE (the main window), along row Y
                                from X0 to X1: what differs from the colour at
                                X0 must leave the same room on both sides
                                (±1 screen px). E.g. a menu row's highlight
                                inside the menu's border. With "bordes" after
                                X1, X0 and X1 are the ground's edges (a panel
                                on a ground of about its colour).
    carril NOMBRE Y X0 X1       in capture NOMBRE, along row Y from X0 to X1:
                                the scroll bar's thumb (the brightest run)
                                leaves as much of its track (the colour next
                                to it) on each side, to the screen pixel
                                (k: 1 = main window, then in order of creation)
    revisar NOMBRE              fails if a surface has see-through holes or
                                pixels outside the damage the app reported
    # comment
"""
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target" / "ui-live"
TOUR = REPO / "target" / "ui-tour" / "perfil"
WL = REPO / "target" / "wl-headless"


class Fail(Exception):
    pass


def load_pam(path):
    from PIL import Image
    data = path.read_bytes()
    i = data.index(b"ENDHDR\n") + 7
    head = data[:i].decode().split("\n")
    w, h = int(head[1].split()[1]), int(head[2].split()[1])
    return Image.frombytes("RGBA", (w, h), data[i:])


def build_tools(audio):
    subprocess.run([str(REPO / "tools/wl-headless/build.sh"), str(WL)], check=True)
    if audio:
        testd = WL / "testd"
        src = REPO / "tests/pipewire/testd.c"
        if not testd.exists() or testd.stat().st_mtime < src.stat().st_mtime:
            flags = subprocess.run(["pkg-config", "--cflags", "--libs", "libpipewire-0.3"], capture_output=True, text=True, check=True).stdout.split()
            subprocess.run(["gcc", "-O1", "-o", str(testd), str(src), *flags], check=True)
    if not (TOUR / "data").exists():
        # The tour makes the test library (and builds the app).
        subprocess.run([str(REPO / "tools/ui-tour.sh"), "pistas"], check=True, stdout=subprocess.DEVNULL)


class Adopted:
    """A process this script did not start (the app's own restart), with the
    bits of Popen's interface Session uses."""

    def __init__(self, pid):
        self.pid = pid

    def poll(self):
        try:
            with open(f"/proc/{self.pid}/stat") as f:
                return 0 if f.read().split(") ", 1)[1].startswith("Z") else None
        except (FileNotFoundError, IndexError):
            return 0

    def wait(self, timeout=None):
        end = time.time() + (timeout if timeout is not None else 1e9)
        while self.poll() is None:
            if time.time() > end:
                raise subprocess.TimeoutExpired(f"pid {self.pid}", timeout)
            time.sleep(0.02)
        return 0

    def terminate(self):
        try:
            os.kill(self.pid, 15)
        except ProcessLookupError:
            pass

    def kill(self):
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


class Session:
    def __init__(self, app, scale, ui_scale, audio, run_dir, say, settings=(), app_vars=(), prepare=(), screen=None, args=(), tap=False, dnd=False):
        self.app, self.audio, self.run_dir, self.say = app, audio, run_dir, say
        self.rt = Path(tempfile.mkdtemp(prefix="sk-live", dir="/tmp"))  # sockets: short paths
        self.prof = self.rt / "p"
        for d in ("config", "data"):
            shutil.copytree(TOUR / d, self.prof / d)
        (self.prof / "cache" / "fontconfig").mkdir(parents=True)
        conf = self.conf = self.prof / "config/sonkore/sonkore.conf"
        text = conf.read_text()
        extra = [("appearance", f"uiScale={ui_scale}"), ("playback", "device=pipewire|test-sink" if audio else "device=")]
        for spec in settings:
            section, line = spec.split("/", 1)
            extra.append((section, line))
        for section, line in extra:
            key = line.split("=", 1)[0]
            text = re.sub(rf"^{re.escape(key)}=.*\n", "", text, flags=re.M) if f"[{section}]" in text else text
            if f"[{section}]" in text:
                text = text.replace(f"[{section}]", f"[{section}]\n{line}", 1)
            else:
                text += f"\n[{section}]\n{line}\n"
        conf.write_text(text)
        self.state_file = self.rt / "state"
        self.result_file = self.rt / "result"
        self.result_file.touch()
        self.comp_log = self.rt / "comp.log"
        env = dict(os.environ, XDG_RUNTIME_DIR=str(self.rt), START_SCALE=str(round(scale * 1.2)),
                   RESULT_FILE=str(self.result_file), DUMP_DIR=str(run_dir))
        if screen:
            env["OUTPUT"] = screen
        if dnd:
            env["DND"] = "1"  # the compositor offers drag and drop (wl_data_device_manager)
        self.procs = []
        if audio:
            conf_dir = self.rt / "pwconf"
            conf_dir.mkdir()
            shutil.copy(REPO / "tests/pipewire/test.conf", conf_dir)
            self.procs.append(subprocess.Popen([str(WL / "testd"), "test.conf"], env=dict(env, PIPEWIRE_CONFIG_DIR=str(conf_dir)),
                                               stdout=subprocess.DEVNULL, stderr=open(self.rt / "pw.log", "w")))
        self.comp = subprocess.Popen([str(WL / "headless"), "wl-live"], env=env, stdin=subprocess.PIPE, text=True,
                                     stderr=open(self.comp_log, "w"))
        self.procs.append(self.comp)
        # Its socket, before the app looks for it (winit doesn't retry; a
        # fixed 0.3 s fell short with several sessions at once).
        end = time.time() + 10
        while not (self.rt / "wl-live").exists() and time.time() < end:
            if self.comp.poll() is not None:
                raise Fail(f"el compositor terminó al arrancar (ver {self.comp_log})")
            time.sleep(0.02)
        time.sleep(0.05)
        app_env = {
            "HOME": os.environ["HOME"], "PATH": os.environ["PATH"], "LANG": "es_AR.UTF-8",
            "XDG_RUNTIME_DIR": str(self.rt), "WAYLAND_DISPLAY": "wl-live",
            "XDG_CONFIG_HOME": str(self.prof / "config"), "XDG_DATA_HOME": str(self.prof / "data"),
            "XDG_CACHE_HOME": str(self.prof / "cache"),
            "ALSA_CONFIG_PATH": str(REPO / "tests/pipewire/alsa-null.conf"),
            "SONKORE_STATE_FILE": str(self.state_file),
            # The cards are the test's (the same three as the user's computer,
            # DX1 as card 2), so the outputs listed don't depend on the machine.
            "SONKORE_ASOUND_ROOT": str(REPO / "tests/asound"),
        }
        if tap:
            app_env["SONKORE_OUTPUT_TAP"] = str(self.rt / "tap")
        # SONKORE_* from the caller reach the app (SONKORE_FRAME_STATS=1…).
        app_env.update({k: v for k, v in os.environ.items() if k.startswith("SONKORE_") and k not in app_env})
        for spec in app_vars:
            key, value = spec.split("=", 1)
            app_env[key] = self.expand(value)
        for command in prepare:
            done = subprocess.run(self.expand(command), shell=True, capture_output=True, text=True)
            if done.returncode != 0:
                raise Fail(f"preparar terminó con {done.returncode}: {done.stderr.strip()[:200]}")
        if app == "rust":
            binary = REPO / "target/debug/sonkore"
        else:
            binary = Path(app)
        self.binary, self.app_env = binary, app_env
        self.launch([self.expand(a) for a in args])
        self.pending_lat = None
        self.waited_state = True
        self.ready()

    def expand(self, text):
        text = text.replace("{rt}", str(self.rt)).replace("{repo}", str(REPO))
        if "{pid}" in text and getattr(self, "app_proc", None):
            text = text.replace("{pid}", str(self.app_proc.pid))
        return text

    def launch(self, args=()):
        self.app_proc = subprocess.Popen([str(self.binary), *args], env=self.app_env, stdout=open(self.rt / "app.log", "a"), stderr=subprocess.STDOUT)
        self.procs.append(self.app_proc)

    def restart(self):
        """The app again on the same profile, once it has quit (its own close
        button saves; SIGTERM after 5 s)."""
        try:
            self.app_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.say("    reiniciar: la app no se cerró sola en 5 s (SIGTERM)")
            self.app_proc.terminate()
            self.app_proc.wait(timeout=3)
        self.procs.remove(self.app_proc)
        if self.state_file.exists():
            self.state_file.unlink()
        self.launch()
        self.ready()

    def restarted_by_itself(self):
        """The app quit by itself and started another (Settings › Restart):
        that one is the app from now on."""
        try:
            code = self.app_proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            raise Fail("se-reinicia: la app no se cerró en 8 s")
        if code != 0:
            raise Fail(f"se-reinicia: la app terminó con {code}")
        self.procs.remove(self.app_proc)
        mark = f"SONKORE_STATE_FILE={self.state_file}".encode()
        end = time.time() + 8
        while time.time() < end:
            for entry in os.listdir("/proc"):
                if not entry.isdigit() or int(entry) == self.app_proc.pid:
                    continue
                try:
                    with open(f"/proc/{entry}/environ", "rb") as f:
                        if mark not in f.read().split(b"\0"):
                            continue
                except OSError:
                    continue
                self.app_proc = Adopted(int(entry))
                self.procs.append(self.app_proc)
                self.ready()
                return
            time.sleep(0.05)
        raise Fail("se-reinicia: no arrancó otra app")

    def send(self, line):
        self.comp.stdin.write(line + "\n")
        self.comp.stdin.flush()

    def surfaces(self):
        before = self.comp_log.read_text().count("list-end")
        self.send("list")
        end = time.time() + 2
        while time.time() < end:
            log = self.comp_log.read_text()
            if log.count("list-end") > before:
                block = log.split("list-end")[before]
                return re.findall(r"surface (\d+) (\w+) (\d+)x(\d+)", block)
            time.sleep(0.02)
        raise Fail("el compositor no responde")

    def ready(self):
        end = time.time() + 15
        while time.time() < end:
            if self.app_proc.poll() is not None:
                raise Fail(f"la app terminó al arrancar (ver {self.rt}/app.log)")
            if any(role == "toplevel" and int(w) > 0 for _, role, w, _ in self.surfaces()):
                if not self.state_file.exists():
                    time.sleep(0.05)
                    continue
                time.sleep(0.8)  # first frames, the library model, covers
                return
            time.sleep(0.1)
        raise Fail("no apareció la ventana en 15 s")

    def state(self):
        try:
            return dict(l.split("=", 1) for l in self.state_file.read_text().splitlines() if "=" in l)
        except FileNotFoundError:
            return {}

    def lat_results(self):
        return dict((m[0], float(m[1])) for m in re.findall(r"^lat (\S+) (\S+)$", self.result_file.read_text(), re.M))

    def input(self, *lines):
        if self.pending_lat:
            name, _, new = self.pending_lat
            self.send(f"{'latnew' if new else 'lat'} {name}")
        for l in lines:
            self.send(l)
        if self.pending_lat:
            name, limit, _ = self.pending_lat
            self.pending_lat = None
            end = time.time() + max(2.0, limit / 1000 * 3)
            while time.time() < end and name not in self.lat_results():
                time.sleep(0.01)
            ms = self.lat_results().get(name)
            if ms is None:
                raise Fail(f"latencia {name}: ningún cuadro nuevo")
            mark = "" if ms <= limit else "  ← LENTO"
            self.say(f"    latencia {name}: {ms:.0f} ms (máx {limit:g}){mark}")
            if ms > limit:
                raise Fail(f"latencia {name}: {ms:.0f} ms > {limit} ms")

    def capture(self, name, prefix=""):
        stem = self.run_dir / f"{prefix}{name}"
        self.send(f"dump {stem}")
        self.surfaces()  # the dump is done once list answers
        files = []
        # Numbered by order (1 = the main window).
        pams = sorted(self.run_dir.glob(f"{prefix}{name}_*_screen.pam"),
                      key=lambda p: int(p.name[len(prefix + name) + 1:].split("_")[0]))
        for k, pam in enumerate(pams, 1):
            png = self.run_dir / f"{prefix}{name}-{k}.png"
            load_pam(pam).save(png)
            pam.unlink()
            files.append(png)
        return files

    def close(self):
        # The app first (a client whose compositor goes away exits with an error).
        died = self.app_proc.poll() is not None
        self.app_proc.terminate()
        try:
            self.app_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.app_proc.kill()
            self.say("    la app no terminó con SIGTERM en 3 s")
        try:
            self.send("quit")
        except BrokenPipeError:
            pass
        for p in self.procs:
            try:
                p.wait(timeout=2)
            except subprocess.TimeoutExpired:
                p.kill()
        log = (self.rt / "app.log").read_text(errors="replace")
        (self.run_dir / "app.log").write_text(log)  # always: debug() output, warnings
        crashed = died or "panicked" in log
        if crashed:
            keep = self.run_dir / "app.log"
            keep.write_text(log)
            self.say(f"    la app se cayó o entró en pánico: {keep}")
        shutil.rmtree(self.rt, ignore_errors=True)
        return not crashed


def check_state(sess, spec, timeout):
    m = re.match(r"([\w-]+)(!=|=|~|>|<)(.*)$", spec)
    if not m:
        raise Fail(f"estado mal escrito: {spec}")
    key, op, want = m.groups()
    end = time.time() + timeout
    got = None
    while True:
        got = sess.state().get(key)
        if got is not None:
            if op == "=" and got == want: return
            if op == "!=" and got != want: return
            if op == "~" and re.search(want, got): return
            try:
                if op == ">" and float(got) > float(want): return
                if op == "<" and float(got) < float(want): return
            except ValueError:
                pass
        if time.time() > end:
            raise Fail(f"estado {spec}: quedó {key}={got!r} tras {timeout} s")
        time.sleep(0.05)


def sound(sess, seconds):
    """What the app's last output got over its last `seconds` (crates/app/src/tap.rs):
    format, and per channel (i = left, d = right) the level (dB of a sine: RMS
    + 3.01 dB, so a -12 dBFS tone reads -12), the peak (dBFS), the frequency
    (Hz, from rising zero crossings), the spread in LSBs of the quantizer's
    grid (desvio: 0 for a constant without dither) and, for a click, the
    pre-ringing (preeco: dB of the energy ahead of its peak, -200 for none;
    the conversion filter's phase, escenarios/sonido-filtro.txt)."""
    import json
    from array import array
    folder = sess.rt / "tap"
    segments = sorted(int(p.stem) for p in folder.glob("*.json"))
    if not segments:
        return {}
    n = segments[-1]
    fmt = json.loads((folder / f"{n}.json").read_text())
    ch, rate = fmt["channels"], fmt["rate"]
    raw = folder / f"{n}.raw"
    frame = 4 * ch
    size = raw.stat().st_size // frame * frame
    out = {
        "salida": fmt["key"], "tasa": str(rate), "canales": str(ch), "bits": str(fmt["bits"]),
        "flotante": str(fmt["float"]).lower(), "dsd": fmt["dsd"] or "no", "salidas": str(len(segments)),
    }
    with open(raw, "rb") as f:
        # Silence at the start (the pause after a rate change), in ms.
        lead, step = 0, 65536 * frame
        while True:
            block = f.read(step)
            if not block:
                break
            a = array("i")
            a.frombytes(block[: len(block) // frame * frame])
            nz = next((i for i, v in enumerate(a) if v != 0), None)
            if nz is not None:
                lead += nz // ch
                break
            lead += len(a) // ch
        out["silencio"] = f"{lead * 1000 / rate:.0f}"
        want = min(size, int(seconds * rate) * frame)
        f.seek(size - want)
        a = array("i")
        a.frombytes(f.read(want))
    if not a:
        return out
    frames = len(a) // ch
    out["duracion"] = f"{frames / rate:.2f}"
    if fmt["dsd"]:
        # DoP: the marker byte (0x05 / 0xFA, in turn) atop each word.
        top = [(v >> 24) & 0xFF for v in a[0::ch]]
        out["dop"] = f"{sum(t in (0x05, 0xFA) for t in top) / len(top):.2f}"
        dsd_measures(out, a, ch, rate, fmt["dsd"] == "Dop")
        return out
    lsb = float(1 << (32 - fmt["quantizer"]))
    for c, name in zip(range(min(ch, 2)), "id"):
        x = a[c::ch]
        mean = sum(x) / frames
        rms = (sum(v * v for v in x) / frames) ** 0.5 / 2147483648.0
        peak = max(abs(v) for v in x) / 2147483648.0
        db = lambda v: 20 * __import__("math").log10(v) if v > 0 else -200.0
        out[f"nivel-{name}"] = f"{db(rms * 2 ** 0.5):.1f}"
        out[f"pico-{name}"] = f"{db(peak):.1f}"
        out[f"desvio-{name}"] = f"{(sum((v - mean) ** 2 for v in x) / frames) ** 0.5 / lsb:.2f}"
        ups = [i - x[i - 1] / (x[i] - x[i - 1]) for i in range(1, frames) if x[i - 1] < 0 <= x[i]]
        out[f"frec-{name}"] = f"{(len(ups) - 1) * rate / (ups[-1] - ups[0]):.1f}" if len(ups) > 2 and ups[-1] > ups[0] else "0"
        # Pre-ringing of a click: the energy from 20 ms to 0.2 ms before the
        # loudest sample, in dB of the energy within 20 ms of it.
        p = max(range(frames), key=lambda i: abs(x[i]))
        lo, hi, lobe = max(0, p - rate // 50), min(frames, p + rate // 50), round(rate * 0.0002)
        pre = sum(v * v for v in x[lo:max(lo, p - lobe)])
        total = sum(v * v for v in x[lo:hi])
        out[f"preeco-{name}"] = f"{10 * __import__('math').log10(pre / total):.1f}" if pre > 0 and total > 0 else "-200.0"
    return out


def dsd_measures(out, a, ch, rate, dop):
    """DSD words (native DSD_U32 or DoP) back to a level and a frequency per
    channel: the 1-bit density of each word (which order the bits go out in
    does not matter for it), a third-order CIC down to ~11 kHz, then the same
    measures as PCM. 50 % modulation reads -6 dB (all ones is full scale, as
    DSD to PCM converts); the CIC's droop at the measured frequency is taken
    back out. No desvio: there is no grid."""
    import math
    bits = 16 if dop else 32
    mask = 0x00FFFF00 if dop else 0xFFFFFFFF
    m = max(1, round(rate / 11025))  # words per output sample
    for c, name in zip(range(min(ch, 2)), "id"):
        x = [(2 * bin(v & mask).count("1") - bits) / bits for v in a[c::ch]]
        for _ in range(3):  # moving averages of m words
            run, acc = [], 0.0
            for i, v in enumerate(x):
                acc += v - (x[i - m] if i >= m else 0.0)
                run.append(acc / m)
            x = run
        y = x[3 * m::m]
        if len(y) < 4:
            continue
        out_rate = rate / m
        ups = [i - y[i - 1] / (y[i] - y[i - 1]) for i in range(1, len(y)) if y[i - 1] < 0 <= y[i]]
        freq = (len(ups) - 1) * out_rate / (ups[-1] - ups[0]) if len(ups) > 2 and ups[-1] > ups[0] else 0.0
        # Droop of three boxcars of m·bits bits at the 1-bit rate.
        span = m * bits
        w = math.pi * freq / (rate * bits)
        droop = (math.sin(w * span) / (span * math.sin(w))) ** 3 if freq > 0 else 1.0
        rms = (sum(v * v for v in y) / len(y)) ** 0.5 / max(droop, 1e-9)
        peak = max(abs(v) for v in y)
        db = lambda v: 20 * math.log10(v) if v > 0 else -200.0
        out[f"nivel-{name}"] = f"{db(rms * 2 ** 0.5):.1f}"
        out[f"pico-{name}"] = f"{db(peak):.1f}"
        out[f"frec-{name}"] = f"{freq:.1f}"


def check_sound(sess, seconds, specs, timeout):
    """Each spec: CLAVE=VALOR, CLAVE=N±T, CLAVE<N, CLAVE>N, CLAVE~REGEX,
    CLAVE!=VALOR; all have to hold on the same measure."""
    parsed = []
    for spec in specs:
        m = re.match(r"([\w-]+)(!=|=|~|>|<)(.*)$", spec)
        if not m:
            raise Fail(f"sonido mal escrito: {spec}")
        parsed.append(m.groups())

    def holds(got, op, want):
        if got is None:
            return False
        if op == "~":
            return re.search(want, got) is not None
        try:
            g = float(got)
            if op == "=" and "±" in want:
                w, tol = want.split("±")
                return abs(g - float(w)) <= float(tol)
            if op in "=<>" or op == "!=":
                w = float(want)
                return {"=": g == w, "!=": g != w, "<": g < w, ">": g > w}[op]
        except ValueError:
            pass
        return (got == want) if op == "=" else (got != want) if op == "!=" else False

    if not parsed:
        got = sound(sess, seconds)
        sess.say("    sonido: " + " ".join(f"{k}={v}" for k, v in got.items()))
        return
    end = time.time() + timeout
    while True:
        got = sound(sess, seconds)
        if all(holds(got.get(k), op, w) for k, op, w in parsed):
            sess.say("    sonido: " + " ".join(f"{k}={got.get(k)}" for k, _, _ in parsed))
            return
        if time.time() > end:
            shown = " ".join(f"{k}={got.get(k)}" for k in dict.fromkeys([k for k, _, _ in parsed] + ["salida", "tasa", "nivel-i", "nivel-d"]))
            raise Fail(f"sonido {' '.join(specs)}: medido {shown}")
        time.sleep(0.2)


def saved_settings(conf):
    """sonkore.conf as {"section/key": raw value, quotes taken off}."""
    out, section = {}, "General"
    try:
        text = conf.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return out
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
        elif "=" in line:
            k, v = line.split("=", 1)
            v = v.strip()
            if len(v) >= 2 and v[0] == v[-1] == '"':
                v = v[1:-1]
            out[k.strip() if section == "General" else f"{section}/{k.strip()}"] = v
    return out


def check_saved(sess, spec, timeout):
    m = re.match(r"([\w/%-]+)(!=|!~|=|~)(.*)$", spec)
    if not m:
        raise Fail(f"guardado mal escrito: {spec}")
    key, op, want = m.groups()
    end = time.time() + timeout
    while True:
        got = saved_settings(sess.conf).get(key)
        if op == "=" and got == want: return
        if op == "!=" and got != want: return
        if op == "~" and got is not None and re.search(want, got): return
        if op == "!~" and (got is None or not re.search(want, got)): return
        if time.time() > end:
            raise Fail(f"guardado {spec}: quedó {key}={got!r} tras {timeout} s")
        time.sleep(0.05)


def run_scenario(path, app, scale, ui_scale, audio, tag):
    """Returns (ok, report)."""
    lines = []
    say = lines.append
    run_dir = OUT / tag
    shutil.rmtree(run_dir, ignore_errors=True)
    run_dir.mkdir(parents=True)
    sess = None
    ok = False
    t0 = time.time()
    captures = []
    try:
        text = path.read_text().splitlines()
        # A file the scenario needs and the machine may lack (an optional
        # test corpus): without it the scenario is skipped, not failed.
        for l in text:
            if l.startswith("requiere "):
                need = Path(l.split(None, 1)[1].strip().replace("{repo}", str(REPO)))
                if not need.exists():
                    return True, f"  OMITIDO {tag}: falta {need}"
        settings = [l.split(None, 1)[1].strip() for l in text if l.startswith("ajuste ")]
        env = [l.split(None, 1)[1].strip() for l in text if l.startswith("entorno ")]
        prepare = [l.split(None, 1)[1].strip() for l in text if l.startswith("preparar ")]
        screen = [l.split()[1:3] for l in text if l.startswith("pantalla ")]
        # shlex: a path with spaces goes between quotes
        app_args = [a for l in text if l.startswith("argumentos ") for a in shlex.split(l)[1:]]
        sess = Session(app, scale, ui_scale, audio, run_dir, say, settings, env, prepare,
                       screen=f"{screen[-1][0]}x{screen[-1][1]}" if screen else None, args=app_args,
                       tap=any(l.split()[:1] == ["sonido"] for l in text),
                       dnd=any(l.split()[:1] in (["soltar-encima"], ["soltar"]) for l in text))
        for n, raw in enumerate(text, 1):
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            cmd, *a = line.split()
            if cmd in ("ajuste", "entorno", "preparar", "pantalla", "argumentos", "requiere"):
                continue
            try:
                if cmd == "esperar":
                    time.sleep(float(a[0]))
                elif cmd == "sh":
                    done = subprocess.run(sess.expand(line.split(None, 1)[1]), shell=True, capture_output=True, text=True)
                    if done.returncode != 0:
                        raise Fail(f"sh terminó con {done.returncode}: {done.stderr.strip()[:200]}")
                elif cmd in ("mover", "clic", "dclic", "rclic"):
                    x, y = float(a[0]) * ui_scale, float(a[1]) * ui_scale
                    extra = {"mover": [], "clic": ["click"], "dclic": ["dclick"], "rclic": ["rclick"]}[cmd]
                    sess.input(f"move {x} {y}", *extra)
                    time.sleep(0.15)
                elif cmd == "arrastrar":
                    # A press that should move the window (a title bar, the
                    # mini player's empty space): the compositor must get
                    # xdg_toplevel.move.
                    before = sess.result_file.read_text() if sess.result_file.exists() else ""
                    sess.input(f"move {float(a[0]) * ui_scale} {float(a[1]) * ui_scale}", "button 1")
                    time.sleep(0.3)
                    sess.input("button 0")
                    time.sleep(0.15)
                    now = sess.result_file.read_text() if sess.result_file.exists() else ""
                    if "moved toplevel" not in now[len(before):]:
                        raise Fail("arrastrar: la ventana no pidió moverse")
                elif cmd == "arrastre":
                    # Press at X0 Y0, move to X1 Y1 in steps, let go (a slider,
                    # a point of the equalizer's graph).
                    x0, y0, x1, y1 = (float(v) * ui_scale for v in a[:4])
                    sess.input(f"move {x0} {y0}", "button 1")
                    time.sleep(0.1)
                    for i in range(1, 11):
                        sess.input(f"move {x0 + (x1 - x0) * i / 10} {y0 + (y1 - y0) * i / 10}")
                        time.sleep(0.03)
                    sess.input("button 0")
                    time.sleep(0.15)
                elif cmd == "cpu":
                    ticks = lambda: sum(int(v) for v in Path(f"/proc/{sess.app_proc.pid}/stat").read_text().rsplit(")", 1)[1].split()[11:13])
                    hz = os.sysconf("SC_CLK_TCK")
                    t0, c0 = time.time(), ticks()
                    time.sleep(float(a[1]))
                    pct = (ticks() - c0) / hz / (time.time() - t0) * 100
                    say(f"    cpu {a[0]}: {pct:.1f} %")
                    if len(a) > 2 and pct > float(a[2]):
                        raise Fail(f"cpu {a[0]}: {pct:.1f} % > {a[2]} %")
                elif cmd == "perfil":
                    data = run_dir / f"{a[0]}.perf.data"
                    subprocess.run(["perf", "record", "-F", "999", "-g", "-o", str(data), "-p", str(sess.app_proc.pid), "--", "sleep", a[1]],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    top = subprocess.run(["perf", "report", "-i", str(data), "--stdio", "--no-children", "--percent-limit", "1.5",
                                          "--sort", "dso,symbol", "-g", "none"], capture_output=True, text=True).stdout
                    (run_dir / f"{a[0]}.txt").write_text(top)
                    say(f"    perfil {a[0]}: {run_dir / (a[0] + '.txt')}")
                elif cmd == "rueda":
                    sess.input(f"move {float(a[0]) * ui_scale} {float(a[1]) * ui_scale}", f"axis {a[2]}")
                    time.sleep(0.15)
                elif cmd == "soltar-encima":
                    # Files dragged from a "file manager" over the window
                    # (Wayland: wl_data_device), to X Y.
                    from urllib.parse import quote
                    paths = shlex.split(sess.expand(line.split(None, 3)[3]))
                    uris = " ".join("file://" + quote(p) for p in paths)
                    sess.send(f"dnd-enter {float(a[0]) * ui_scale} {float(a[1]) * ui_scale} {uris}")
                    time.sleep(0.3)
                elif cmd == "soltar":
                    sess.send("dnd-drop")
                    time.sleep(0.3)
                elif cmd == "soltar-fuera":
                    sess.send("dnd-leave")
                    time.sleep(0.2)
                elif cmd == "tecla":
                    sess.input(f"key {a[0]}")
                    time.sleep(0.15)
                elif cmd == "foco":
                    sess.send(f"focus {a[0]}")
                elif cmd == "escala":
                    sess.send(f"scale {round(float(a[0]) * 1.2)}")
                    time.sleep(0.5)
                elif cmd == "tamaño":
                    sess.send(f"resize {a[0]} {a[1]}")
                    time.sleep(0.5)
                elif cmd == "estado":
                    timeout = 3.0
                    if len(a) > 1 and re.fullmatch(r"[\d.]+", a[-1]):
                        timeout = float(a.pop())
                    check_state(sess, " ".join(a), timeout)
                elif cmd == "sonido":
                    seconds, timeout = 1.0, 5.0
                    if a and re.fullmatch(r"[\d.]+", a[0]):
                        seconds = float(a.pop(0))
                    if a and re.fullmatch(r"\d+(\.\d+)?s", a[-1]):
                        timeout = float(a.pop()[:-1])
                    check_sound(sess, seconds, a, timeout)
                elif cmd == "guardado":
                    timeout = 3.0
                    if len(a) > 1 and re.fullmatch(r"[\d.]+", a[-1]):
                        timeout = float(a.pop())
                    check_saved(sess, " ".join(a), timeout)
                elif cmd == "ventanas":
                    want, end = int(a[0]), time.time() + (float(a[1]) if len(a) > 1 else 3.0)
                    while True:
                        tops = [s for s in sess.surfaces() if s[1] == "toplevel" and int(s[2]) > 0]
                        if len(tops) == want:
                            break
                        if time.time() > end:
                            raise Fail(f"ventanas: hay {len(tops)}, se esperaban {want}")
                        time.sleep(0.1)
                elif cmd == "latencia":
                    sess.pending_lat = (a[0], float(a[1]), len(a) > 2 and a[2] == "ventana")
                elif cmd == "captura":
                    captures += sess.capture(a[0])
                elif cmd == "se-reinicia":
                    sess.restarted_by_itself()
                elif cmd == "carril":
                    # The track and the panel beside it differ by a few
                    # levels, too little for "centrado" (the mini's list frame is 3
                    # off): the track is the flat colour next to the thumb, ±1.
                    from PIL import Image
                    k = scale / 100 * ui_scale
                    img = Image.open(run_dir / f"{a[0]}-1.png").convert("RGB")
                    y, x0, x1 = round(float(a[1]) * k), round(float(a[2]) * k), round(float(a[3]) * k)
                    px = lambda x: img.getpixel((x, y))
                    near = lambda p, q, d: max(abs(u - v) for u, v in zip(p, q)) <= d
                    top = max(range(x0, x1), key=lambda x: sum(px(x)))
                    first = last = top
                    while first > x0 and near(px(first - 1), px(top), 8):
                        first -= 1
                    while last < x1 and near(px(last + 1), px(top), 8):
                        last += 1
                    track_l, track_r = px(first - 1), px(last + 1)
                    left = right = 0
                    while first - left - 1 > x0 and near(px(first - left - 1), track_l, 1):
                        left += 1
                    while last + right + 1 < x1 and near(px(last + right + 1), track_r, 1):
                        right += 1
                    say(f"    carril {a[0]}: barra de {last - first + 1} px, {left} px de carril a la izquierda, {right} a la derecha")
                    if left != right or not near(track_l, track_r, 3):
                        raise Fail(f"carril: {left} px a la izquierda y {right} a la derecha (en {run_dir / (a[0] + '-1.png')})")
                elif cmd == "reiniciar":
                    sess.restart()
                elif cmd == "centrado":
                    # X0 and X1 only need to be near the menu's inner edges: from
                    # the row's middle (inside the highlight) the highlight runs
                    # out to the menu's ground on each side, and the gap is the
                    # ground up to the border. A scale that rounds the menu a
                    # pixel over (1.65 = 150 % × 1.1) measures the same.
                    from PIL import Image
                    k = scale / 100 * ui_scale
                    img = Image.open(run_dir / f"{a[0]}-1.png").convert("RGB")
                    y, x0, x1 = round(float(a[1]) * k), round(float(a[2]) * k), round(float(a[3]) * k)
                    ground = img.getpixel((x0 + 1, y))
                    is_ground = lambda x: max(abs(u - v) for u, v in zip(img.getpixel((x, y)), ground)) <= 12
                    mid = (x0 + x1) // 2
                    if is_ground(mid):
                        raise Fail(f"centrado: nada resaltado en el medio de y={a[1]}")
                    first = mid
                    while first > 0 and not is_ground(first - 1):
                        first -= 1
                    last = mid
                    while last < img.width - 1 and not is_ground(last + 1):
                        last += 1
                    # "bordes": X0 and X1 are the ground's own edges (a panel
                    # on a ground of nearly its colour, nothing to stop at).
                    walls = len(a) > 4 and a[4] == "bordes"
                    lo, hi = (x0, x1 - 1) if walls else (0, img.width - 1)
                    edge0, edge1 = first, last
                    while edge0 > lo and is_ground(edge0 - 1):
                        edge0 -= 1
                    while edge1 < hi and is_ground(edge1 + 1):
                        edge1 += 1
                    left, right = first - edge0, edge1 - last
                    say(f"    centrado {a[0]}: {left} px a la izquierda, {right} a la derecha")
                    if abs(left - right) > 1:
                        raise Fail(f"centrado: {left} px a la izquierda y {right} a la derecha (en {run_dir / (a[0] + '-1.png')})")
                elif cmd == "revisar":
                    before = sess.result_file.read_text()
                    sess.send(f"check {a[0]}")
                    sess.surfaces()
                    bad = []
                    for l in sess.result_file.read_text()[len(before):].splitlines():
                        f = l.split()
                        if len(f) >= 7 and (int(f[-2]) or int(f[-1])):
                            bad.append(f"{f[-6]} {f[-5]} {f[-4]}x{f[-3]}: {f[-2]} px transparentes, {f[-1]} px fuera del daño")
                    if bad:
                        raise Fail("revisar: " + "; ".join(bad) + f" (volcados en {run_dir})")
                else:
                    raise Fail(f"comando desconocido: {cmd}")
            except Fail as e:
                raise Fail(f"{path.name}:{n} «{raw.strip()}»: {e}")
        ok = True
    except Fail as e:
        say(f"    FALLÓ {e}")
        try:
            if sess:
                captures += sess.capture("al-fallar")
                st = sess.state()
                if st:
                    say("    estado: " + ", ".join(f"{k}={v}" for k, v in st.items() if v))
        except Exception:
            pass
        ok = False
    finally:
        if sess and not ok:
            for log in ("app.log", "comp.log", "pw.log"):
                if (sess.rt / log).exists():
                    shutil.copy(sess.rt / log, run_dir / log)
        if sess and not sess.close():
            ok = False
    lines.insert(0, f"  {'OK   ' if ok else 'FALLÓ'} {tag} ({time.time() - t0:.1f} s)" + (f" — capturas en {run_dir}" if captures else ""))
    return ok, "\n".join(lines)


def main():
    args = sys.argv[1:]
    app, scales, ui_scale, audio, files = "rust", [150.0], 1.0, True, []
    workers = max(1, (os.cpu_count() or 4) // 2)
    while args:
        x = args.pop(0)
        if x == "--app": app = args.pop(0)
        elif x == "--escalas": scales = [float(s) for s in args.pop(0).split(",")]
        elif x == "--interfaz": ui_scale = float(args.pop(0))
        elif x == "--sin-audio": audio = False
        elif x == "--hilos": workers = int(args.pop(0))
        elif x in ("-h", "--help"): print(__doc__); return 0
        else: files.append(Path(x))
    if not files:
        files = sorted((REPO / "tools/escenarios").glob("*.txt"))
    build_tools(audio)
    if app == "rust":
        subprocess.run(["cargo", "build", "--quiet", "--manifest-path", str(REPO / "Cargo.toml"), "-p", "sonkore"], check=True)
    # Every run has its own runtime dir, sockets and profile: they go in
    # parallel.
    from concurrent.futures import ThreadPoolExecutor
    jobs = []
    for f in files:
        for scale in scales:
            label = f"{f.stem}-{scale:g}" + (f"-ui{ui_scale:g}" if ui_scale != 1.0 else "")
            jobs.append((f, app, scale, label))
    workers = max(1, min(len(jobs), workers))
    with ThreadPoolExecutor(workers) as pool:
        results = list(pool.map(lambda j: run_scenario(j[0], j[1], j[2], ui_scale, audio, j[3]), jobs))
    failed = 0
    for ok, report in results:
        print(report)
        failed += not ok
    print(f"{'todo bien' if not failed else f'{failed} fallaron'}")
    return 1 if failed else 0


sys.exit(main())
