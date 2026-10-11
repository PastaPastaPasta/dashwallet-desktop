#!/usr/bin/env python3
"""Gate G-UI on Linux (G-03, docs/adr/0003-ui-stack.md): SwiftCrossUI's dash-wallet, GtkBackend,
from the release tarball, on the gate wallet. Started by linux-gate.sh inside a private D-Bus
session with AT-SPI; every launch sets DWD_GATE=1, so GateProbe writes its marks to stderr.

Phases (--phases, comma separated; all by default):
  start    U5 cold start: launch to first paint (demo, and the live wallet's lock screen), and
           unlock to Overview, --runs times each
  history  U5 history_page(200) + render of the 200 rows, and the FFI call alone (limit 1)
  pss      U5 PSS after idling on Overview, and on Transactions with 200 rows
  u1       U1 the hero balance and the first 200 rows against dwcli on the same datadir
  screens  U3 the four screens in light and dark; U4 the AT-SPI naming walk of each
  keys     U4 keyboard only: unlock, Tab through Overview, Transactions, a row's details, Load more
Results: results.json (every measurement), results.txt (one line per check), and the screens'
PNGs and AT-SPI dumps.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../linux/crossui"))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../../probes/crossui-linux/scripts"))
from atspi_smoke import children, find_all, find_app, name, role, safe, wait_for, walk  # noqa: E402

# Roles a user operates; each needs an accessible name (U4).
INTERACTIVE = {"push button", "button", "toggle button", "check box", "radio button", "text", "password text", "entry",
               "combo box", "list item", "menu item", "link", "spin button", "slider", "switch", "page tab"}
BUTTONS = ("push button", "button")
CLK_TCK = os.sysconf("SC_CLK_TCK")


class App:
    """One launch of dash-wallet, its GateProbe marks read from stderr as they arrive."""

    def __init__(self, gate, args, env=None, log=None):
        self.marks = []
        self.cond = threading.Condition()
        environment = dict(os.environ, DWD_GATE="1", **(env or {}))
        self.log = open(log or os.devnull, "w")
        self.started = time.monotonic()
        self.proc = subprocess.Popen([gate.binary, *args], env=environment, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.PIPE, text=True, bufsize=1)
        self.pid = self.proc.pid
        self._root = None
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stderr:
            self.log.write(line)
            self.log.flush()
            if line.startswith("gate-mark "):
                parts = line.split()
                fields = dict(p.split("=", 1) for p in parts[3:] if "=" in p)
                with self.cond:
                    self.marks.append((parts[1], float(parts[2]), fields))
                    self.cond.notify_all()

    def wait(self, mark, timeout=120, after=None):
        """The first mark named `mark` (after the mark `after`, if given) as (seconds since
        launch, fields), or None."""
        hit = self.wait_any([mark], timeout, after)
        return hit[1:] if hit else None

    def wait_any(self, marks, timeout=120, after=None):
        """The first mark named one of `marks` (after the mark `after`, if given) as (name,
        seconds since launch, fields), or None."""
        deadline = time.monotonic() + timeout
        with self.cond:
            while True:
                since = [m[1] for m in self.marks if m[0] == after][:1] if after else [0.0]
                hits = [m for m in self.marks if m[0] in marks and since and m[1] >= since[0]]
                if hits:
                    return hits[0][0], hits[0][1] - self.started, hits[0][2]
                left = deadline - time.monotonic()
                if left <= 0 or self.proc.poll() is not None:
                    return None
                self.cond.wait(min(left, 0.5))

    def all(self, mark):
        with self.cond:
            return [m[2] for m in self.marks if m[0] == mark]

    def memory(self):
        """smaps_rollup's Pss, Pss_Anon and Rss, in MiB."""
        values = {}
        with open(f"/proc/{self.pid}/smaps_rollup") as fh:
            for line in fh:
                key, _, rest = line.partition(":")
                if key in ("Pss", "Pss_Anon", "Pss_File", "Rss"):
                    values[key] = round(int(rest.split()[0]) / 1024, 1)
        return values

    def cpu(self):
        with open(f"/proc/{self.pid}/stat") as fh:
            fields = fh.read().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / CLK_TCK

    def root(self):
        """The app's AT-SPI root, looked up again if the cached one stopped answering."""
        if self._root is None or role(self._root) != "application":
            self._root = app_frame(self.pid, timeout=30)
        return self._root

    def alive(self):
        return self.proc.poll() is None

    def stop(self, timeout=60):
        """Closes the window as a user does (AT-SPI "window.close"), so the engine shuts down and
        the next launch finds a clean datadir; SIGTERM only if that fails. Returns the seconds the
        app took to exit after the close, or None if it had to be terminated."""
        took = None
        if self.alive():
            frame = app_frame(self.pid, timeout=10)
            window = next((c for c in children(frame) if role(c) == "frame"), None) if frame else None
            if window is not None and named_action(window, "window.close"):
                closed = time.monotonic()
                try:
                    self.proc.wait(timeout)
                    took = round(time.monotonic() - closed, 2)
                except subprocess.TimeoutExpired:
                    pass
        if self.alive():
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.log.close()
        return took


class Gate:
    def __init__(self, args):
        self.binary = args.binary
        self.out = args.out
        self.runs = args.runs
        self.dwcli = args.dwcli
        # The wallet's datadir, unpacked once; every live launch gets a fresh copy of it, so no
        # run sees another's leftovers (U1's dwcli reads the copy its launch used).
        self.fixture = os.path.join(args.out, "fixture")
        if not os.path.exists(self.fixture):
            os.makedirs(self.fixture, mode=0o700)
            subprocess.run(["tar", "-C", self.fixture, "-xzf", args.wallet], check=True)
        self.datadir = os.path.join(args.out, "datadir")
        self.passphrase_file = args.passphrase_file
        with open(self.passphrase_file) as fh:
            self.passphrase = fh.read().splitlines()[0]
        self.results = {"uptimeStart": uptime()}
        self.lines = []

    def live_args(self, appearance="light", page=None):
        """The live launch's arguments, on a fresh copy of the wallet."""
        subprocess.run(["rm", "-rf", self.datadir], check=True)
        subprocess.run(["cp", "-a", self.fixture, self.datadir], check=True)
        args = ["--network", "regtest", "--datadir", self.datadir, "--connect", "127.0.0.1:1",
                "--dapi", "http://127.0.0.1:1", "--appearance", appearance]
        return args + (["--page", page] if page else [])

    def unlock_env(self):
        """GateProbe's hooks for the timed runs: unlock, then hide the sync overlay."""
        return {"DWD_GATE_UNLOCK_FILE": self.passphrase_file, "DWD_GATE_HIDE_SYNC_OVERLAY": "1"}

    def launch(self, args, env=None, tag="app"):
        return App(self, args, env, os.path.join(self.out, "logs", f"{tag}.err.log"))

    def check(self, criterion, label, ok, detail=""):
        line = f"{criterion} {'PASS' if ok else 'FAIL'} {label}" + (f" ({detail})" if detail else "")
        print(line, flush=True)
        self.lines.append(line)
        return ok

    def note(self, criterion, label, detail):
        line = f"{criterion} INFO {label}: {detail}"
        print(line, flush=True)
        self.lines.append(line)

    def save(self):
        self.results["uptimeEnd"] = uptime()
        with open(os.path.join(self.out, "results.json"), "w") as fh:
            json.dump(self.results, fh, indent=1)
        with open(os.path.join(self.out, "results.txt"), "w") as fh:
            fh.write("\n".join(self.lines) + "\n")


def uptime():
    with open("/proc/loadavg") as fh:
        return " ".join(fh.read().split()[:3])


def summary(values):
    values = sorted(values)
    if not values:
        return {}
    p95 = values[min(len(values) - 1, max(0, round(0.95 * len(values) + 0.5) - 1))]
    return {"n": len(values), "min": round(values[0], 3), "median": round(statistics.median(values), 3),
            "p95": round(p95, 3), "max": round(values[-1], 3)}


# --- AT-SPI ---

def app_frame(pid, timeout=60):
    app = find_app(pid, timeout=timeout)
    if app is None:
        return None
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if any(role(c) == "frame" and name(c).startswith("Dash Wallet") for c in children(app)):
            return app
        time.sleep(0.25)
    return app


def interactive(nodes):
    return [(n, i) for n, i, _d in nodes if i["role"] in INTERACTIVE and "showing" in i["states"]]


def unnamed(nodes):
    return [i["role"] for _n, i in interactive(nodes) if not i["name"]]


def node_named(app, roles, title, timeout=15):
    hits = wait_for(app, lambda n, i: i["role"] in roles and i["name"] == title and "showing" in i["states"],
                    timeout=timeout)
    return hits[0][0] if hits else None


def named_action(node, action_name):
    action = safe(lambda: node.queryAction())
    names = [safe(lambda i=i: action.getName(i), "") for i in range(action.nActions)] if action else []
    return action_name in names and bool(safe(lambda: action.doAction(names.index(action_name)), False))


def do_action(node):
    action = safe(lambda: node.queryAction()) if node is not None else None
    return action is not None and action.nActions > 0 and bool(safe(lambda: action.doAction(0), False))


def unlock(gate, app):
    field = node_named(app, ("password text", "text", "entry"), "Passphrase", timeout=60)
    editable = safe(lambda: field.queryEditableText()) if field is not None else None
    typed = editable is not None and bool(safe(lambda: editable.setTextContents(gate.passphrase), False))
    # The button turns sensitive once the field has text.
    button = typed and wait_for(app, lambda n, i: i["role"] in BUTTONS and i["name"] == "Unlock"
                                and "sensitive" in i["states"], timeout=10)
    pressed = bool(button) and do_action(button[0][0])
    return typed and pressed


def past_sync_overlay(app, frame):
    """After an unlock through AT-SPI: the gate wallet has no peer, so the sync overlay covers
    the wallet; press its Hide button. True once Overview is up."""
    if app.wait("sync-overlay-paint", 180, after="lock-paint") is None:
        return app.wait("overview-paint", 5, after="lock-paint") is not None
    do_action(node_named(frame, BUTTONS, "Hide", timeout=15))
    return app.wait("overview-paint", 60, after="sync-overlay-paint") is not None


def sidebar_select(app, item):
    for node, _info in find_all(app, lambda n, i: i["role"] in ("list", "list box", "table", "tree table")):
        items = [c for c in children(node) if role(c) in ("list item", "table row", "tree item")]
        texts = [name(c) or " ".join(t for t in descendant_texts(c)) for c in items]
        if item in texts and "Transactions" in texts and "Overview" in texts:
            selection = safe(lambda: node.querySelection())
            return selection is not None and bool(safe(lambda: selection.selectChild(texts.index(item)), False))
    return False


def descendant_texts(node, depth=3):
    out = []
    if depth < 0:
        return out
    for child in children(node):
        if name(child):
            out.append(name(child))
        text = safe(lambda: child.queryText())
        if text is not None:
            value = safe(lambda: text.getText(0, -1), "")
            if value:
                out.append(value)
        out.extend(descendant_texts(child, depth - 1))
    return out


def dump(gate, app, tag):
    # A walk during a large rebuild (Transactions' rows) can see stale objects ("[?]"); retry.
    for _ in range(5):
        lines, nodes = walk(app)
        if nodes and nodes[0][1]["role"] == "application" and len(nodes) > 5 and all(
                i["role"] != "?" for _n, i, d in nodes if d <= 1):
            break
        time.sleep(2)
    with open(os.path.join(gate.out, f"atspi-{tag}.txt"), "w") as fh:
        fh.write("\n".join(lines) + "\n")
    return nodes


# --- phases ---

def phase_start(gate):
    """U5 cold start: spawn to GateProbe's marks, on CLOCK_MONOTONIC (the probe's clock), with the
    process's CPU time at each. The demo's Overview, with its balance and history, is the criterion
    (Tauri's fixture-mode first paint); its first frame (the window, Tauri's "shell") and the live
    wallet's lock screen, which waits for the engine to open the 40k wallet, are recorded."""
    rows = {"demo-first-paint": [], "demo-overview-paint": [], "live-lock-paint": [], "live-unlock-to-overview": []}
    cpu = {"demo-first-paint": [], "demo-overview-paint": [], "live-lock-paint": []}
    exits = []  # seconds from the window's close to the process's exit (None: it had to be killed)
    for run in range(gate.runs):
        app = gate.launch(["--demo", "--appearance", "light"], tag=f"start-demo-{run}")
        first, overview = app.wait("first-paint", 60), app.wait("overview-paint", 60)
        exits.append(app.stop())
        if first:
            rows["demo-first-paint"].append(first[0])
            cpu["demo-first-paint"].append(float(first[1]["cpu"]))
        if overview:
            rows["demo-overview-paint"].append(overview[0])
            cpu["demo-overview-paint"].append(float(overview[1]["cpu"]))
        app = gate.launch(gate.live_args(), env=gate.unlock_env(), tag=f"start-live-{run}")
        lock, requested = app.wait("lock-paint", 120), app.wait("unlock-requested", 120)
        overview = app.wait("overview-paint", 180, after="unlock-requested")
        exits.append(app.stop())
        if lock:
            rows["live-lock-paint"].append(lock[0])
            cpu["live-lock-paint"].append(float(lock[1]["cpu"]))
        if requested and overview:
            # Includes the sync overlay's frame and GateProbe hiding it (one frame).
            rows["live-unlock-to-overview"].append(overview[0] - requested[0])
    result = {"seconds": {k: summary(v) for k, v in rows.items()}, "raw": rows,
              "cpuSeconds": {k: summary(v) for k, v in cpu.items()}, "closeToExitSeconds": exits,
              "uptime": uptime()}
    gate.results["u5-start"] = result
    s, c = result["seconds"]["demo-overview-paint"], result["cpuSeconds"]["demo-overview-paint"]
    gate.check("U5", f"cold start to first paint (demo Overview): median {s.get('median')} s, max {s.get('max')} s "
               f"(<= 1.5 s)", bool(s) and s["max"] <= 1.5,
               f"{s.get('n')} runs; CPU median {c.get('median')} s; load {result['uptime']}")
    for key in ("demo-first-paint", "live-lock-paint"):
        gate.note("U5", f"spawn to {key} (s; CPU s)", f"{result['seconds'][key]}; {result['cpuSeconds'][key]}")
    gate.note("U5", "unlock to Overview (KDF + first page)", result["seconds"]["live-unlock-to-overview"])


def phase_history(gate):
    """U5 history_page(200) round trip: GateProbe's bench, after Transactions shows its rows."""
    renders, ffi200, ffi1, cpu200, cpu1 = [], [], [], [], []
    rounds = 20
    for run in range(max(1, gate.runs // 3)):
        app = gate.launch(gate.live_args(page="transactions"), tag=f"history-{run}",
                          env={**gate.unlock_env(), "DWD_GATE_HISTORY_BENCH": str(rounds)})
        done = app.wait("history-bench-done", 900)
        errors = app.all("history-bench-error")
        for fields in app.all("history-render"):
            if fields.get("renderMs") != "timeout":
                renders.append(float(fields["totalMs"]))
            ffi200.append(float(fields["ffiMs"]))
            cpu200.append(float(fields["cpuMs"]))
        for fields in app.all("history-ffi"):
            ffi1.append(float(fields["ffiMs"]))
            cpu1.append(float(fields["cpuMs"]))
        exited = None if app.alive() else app.proc.returncode
        app.stop()
        if not done:
            gate.note("U5", f"history bench run {run}",
                      f"incomplete: {errors or 'no history-bench-done'}; app exit status {exited}")
    result = {"totalMs": summary(renders), "ffiMs200": summary(ffi200), "ffiMs1": summary(ffi1),
              "cpuMs200": summary(cpu200), "cpuMs1": summary(cpu1),
              "raw": {"totalMs": renders, "ffiMs200": ffi200, "ffiMs1": ffi1}, "uptime": uptime()}
    gate.results["u5-history"] = result
    p95 = result["totalMs"].get("p95")
    gate.check("U5", f"history_page(200) + render p95 {p95} ms (<= 20 ms)", p95 is not None and p95 <= 20,
               f"FFI alone p95 {result['ffiMs200'].get('p95')} ms, limit 1 p95 {result['ffiMs1'].get('p95')} ms; "
               f"load {result['uptime']}")


def phase_pss(gate, idle=30):
    """U5 idle PSS, after `idle` s on each screen. The UI's own cost (DEC-36) is the demo's Overview,
    without the engine; the live runs, with the engine in the same process, show Overview after
    unlock, then Transactions with 200 rows."""
    demo = []
    for run in range(max(1, gate.runs // 3)):
        app = gate.launch(["--demo", "--appearance", "light"], tag=f"pss-demo-{run}")
        if app.wait("overview-paint", 120) is not None:
            time.sleep(idle)
            demo.append(app.memory())
        app.stop()
    samples = []
    for run in range(max(1, gate.runs // 3)):
        app = gate.launch(gate.live_args(), env=gate.unlock_env(), tag=f"pss-{run}")
        if app.wait("overview-paint", 300, after="unlock-requested") is None:
            app.stop()
            gate.note("U5", f"PSS run {run}", "no overview-paint")
            continue
        time.sleep(idle)
        overview = app.memory()
        cpu_idle = app.cpu()
        time.sleep(10)
        idle_cpu_pct = round((app.cpu() - cpu_idle) / 10 * 100, 1)
        transactions = None
        frame = app_frame(app.pid)
        if frame is not None and sidebar_select(frame, "Transactions") and app.wait("transactions-paint", 60):
            load_more(app, 200)
            time.sleep(idle)
            transactions = app.memory()
        samples.append({"overview": overview, "transactions": transactions, "idleCpuPercent": idle_cpu_pct})
        app.stop()
    gate.results["u5-pss"] = {"demo": demo, "samples": samples, "idleSeconds": idle, "uptime": uptime()}
    ui = max(s["Pss"] for s in demo) if demo else None
    gate.check("U5", f"idle PSS without the engine (demo Overview) max {ui} MiB (<= 400 MiB)",
               ui is not None and ui <= 400, json.dumps(demo))
    live = max((s["transactions"] or s["overview"])["Pss"] for s in samples) if samples else None
    gate.note("U5", f"idle PSS with the engine, 40k wallet, max {live} MiB", json.dumps(samples))


def transaction_rows(app, timeout=0, more_than=0):
    """Names of the Transactions list's rows (accessibleRowNames: "type, address, amount, date"),
    waiting up to `timeout` s for more than `more_than` of them: right after the page changes, the
    app's main thread is busy drawing and AT-SPI calls time out."""
    deadline = time.monotonic() + timeout
    while True:
        rows = [info["name"] for _n, info in find_all(
            app.root(), lambda n, i: i["role"] in ("list item", "table row") and i["name"].count(", ") >= 3)]
        if len(rows) > more_than or time.monotonic() >= deadline:
            return rows
        time.sleep(3)


def load_more(app, wanted):
    """Presses Load more until the list has `wanted` rows."""
    rows = len(transaction_rows(app, timeout=180))
    for _ in range(10):
        print(f"== Transactions: {rows} rows", flush=True)
        if rows >= wanted:
            return True
        if not do_action(node_named(app.root(), BUTTONS, "Load more", timeout=60)):
            print("== Load more: no button or its action failed", flush=True)
            return False
        rows = len(transaction_rows(app, timeout=180, more_than=rows))
    return rows >= wanted


def phase_u1(gate):
    app = gate.launch(gate.live_args(), tag="u1")
    frame = app_frame(app.pid)
    ok = frame is not None and app.wait("lock-paint", 120) and unlock(gate, frame)
    gate.check("U1", "unlock through AT-SPI (Passphrase field, Unlock button)", bool(ok))
    overview = ok and past_sync_overlay(app, frame)
    nodes = dump(gate, frame, "u1-overview") if overview else []
    texts = [i["name"] for _n, i, _d in nodes if i["name"]] + [i.get("text", "") for _n, i, _d in nodes]
    selected = sidebar_select(frame, "Transactions") and app.wait("transactions-paint", 60)
    loaded = selected and load_more(app, 200)
    rows = transaction_rows(app)[:200] if loaded else []
    with open(os.path.join(gate.out, "u1-app-rows.txt"), "w") as fh:
        fh.write("\n".join(rows) + "\n")
    app.stop()
    reference = dwcli_reference(gate)
    balance = reference["balance"]
    shown = [t for t in texts if t and balance in duff_amounts(t)]
    gate.check("U1", f"Overview shows dwcli's balance ({balance} duffs)", bool(shown), str(shown[:3]))
    mismatches = compare_rows(rows, reference["rows"])
    gate.results["u1"] = {"balanceDuffs": balance, "shownBalance": shown[:3], "appRows": len(rows),
                          "dwcliRows": len(reference["rows"]), "mismatches": mismatches[:20]}
    gate.check("U1", f"first 200 rows match dwcli history ({len(rows)} app rows, {len(reference['rows'])} dwcli)",
               len(rows) == 200 and len(reference["rows"]) == 200 and not mismatches,
               f"{len(mismatches)} mismatches; first {mismatches[:3]}")


def dwcli_reference(gate):
    base = [gate.dwcli, "--datadir", gate.datadir, "--network", "regtest", "--no-platform", "--dapi",
            "http://127.0.0.1:1",
            "--passphrase-file", gate.passphrase_file]
    listing = subprocess.run(base + ["list"], capture_output=True, text=True, timeout=600)
    history = subprocess.run(base + ["history", "--limit", "200"], capture_output=True, text=True, timeout=600)
    with open(os.path.join(gate.out, "u1-dwcli-list.txt"), "w") as fh:
        fh.write(listing.stdout + listing.stderr)
    with open(os.path.join(gate.out, "u1-dwcli-history.txt"), "w") as fh:
        fh.write(history.stdout + history.stderr)
    total = re.search(r"\btotal=(\d+)", listing.stdout)
    # txid, record index, type, amount (signed duffs), status, confirmations, address ("-" if none).
    rows = [line.split() for line in history.stdout.splitlines() if re.match(r"^[0-9a-f]{64} ", line)]
    return {"balance": int(total.group(1)) if total else None, "rows": rows}


def duff_amounts(text):
    """Every decimal number in `text`, in duffs. Digit groups may be split by commas or by spaces
    (the hero uses a narrow no-break space: "206 014.36392896")."""
    joined = re.sub(r"(?<=\d)[,\s\u00a0\u2009\u202f](?=\d{3}\b|\d{3}[,.\s\u00a0\u2009\u202f])", "", text)
    return {round(float(m) * 1e8) for m in re.findall(r"\d+\.\d+", joined)}


def compare_rows(app_rows, reference):
    """Row i of the app against dwcli's row i: the address and the amount."""
    mismatches = []
    for index, ref in enumerate(reference[:200]):
        shown = app_rows[index] if index < len(app_rows) else ""
        address, duffs = ref[6], abs(int(ref[3]))
        if (address != "-" and address not in shown) or duffs not in duff_amounts(shown):
            mismatches.append({"row": index, "app": shown, "dwcli": " ".join(ref)})
    return mismatches


def phase_screens(gate):
    """U3 screenshots (light and dark) and U4's AT-SPI naming walk of each screen."""
    naming = {}
    for appearance in ("light", "dark"):
        app = gate.launch(gate.live_args(appearance), tag=f"screens-{appearance}")
        frame = app_frame(app.pid)
        shots = {}
        if frame is not None and app.wait("lock-paint", 120):
            time.sleep(1)
            shots["unlock"] = capture(gate, frame, f"{appearance}-unlock", naming)
            if unlock(gate, frame) and past_sync_overlay(app, frame):
                time.sleep(1)
                shots["overview"] = capture(gate, frame, f"{appearance}-overview", naming)
                if sidebar_select(frame, "Transactions") and app.wait("transactions-paint", 60):
                    time.sleep(1)
                    shots["transactions"] = capture(gate, frame, f"{appearance}-transactions", naming)
                    if open_first_detail(frame) and app.wait("detail-paint", 30):
                        time.sleep(1)
                        shots["detail"] = capture(gate, frame, f"{appearance}-detail", naming)
        fonts = mapped_fonts(app.pid)
        app.stop()
        gate.results.setdefault("u3-fonts", {})[appearance] = fonts
        gate.check("U3", f"{appearance}: the four screens captured", len(shots) == 4, str(sorted(shots)))
        gate.check("U3", f"{appearance}: the bundled Inter is the font in use",
                   any("Inter" in os.path.basename(f) and ".resources" in f for f in fonts), str(fonts))
    gate.results["u4-naming"] = naming
    for screen, info in naming.items():
        gate.check("U4", f"AT-SPI walk {screen}: {info['interactive']} interactive, {info['unnamed']} unnamed",
                   info["unnamed"] == 0, str(info["unnamedRoles"][:10]))


def capture(gate, frame, tag, naming):
    screenshot(gate.out, tag)
    nodes = dump(gate, frame, tag)
    with open(os.path.join(gate.out, f"styles-{tag}.json"), "w") as fh:
        json.dump(text_styles(nodes), fh, indent=1)
    missing = unnamed(nodes)
    naming[tag] = {"nodes": len(nodes), "interactive": len(interactive(nodes)), "unnamed": len(missing),
                   "unnamedRoles": missing}
    return tag


def text_styles(nodes):
    """U3(a) on Linux: each visible text's font, size, weight and colours (AT-SPI text attributes)."""
    styles = []
    for node, info, _depth in nodes:
        if not info.get("text") or "showing" not in info["states"]:
            continue
        text = safe(lambda: node.queryText())
        run = safe(lambda: text.getAttributeRun(0, True)) if text is not None else None
        attributes = dict(a.split(":", 1) for a in (run[0] if run else []) if ":" in a)
        keep = ("family-name", "size", "weight", "fg-color", "bg-color", "style")
        styles.append({"role": info["role"], "text": info["text"][:60], "extents": info.get("extents"),
                       **{k: attributes[k] for k in keep if k in attributes}})
    return styles


def mapped_fonts(pid):
    """Font files the process has mapped (the bundled Inter shows up here once it is used)."""
    try:
        with open(f"/proc/{pid}/maps") as fh:
            paths = {line.split()[-1] for line in fh if re.search(r"\.(ttf|otf|ttc)$", line.strip())}
    except OSError:
        return []
    return sorted(paths)


def open_first_detail(frame):
    rows = find_all(frame, lambda n, i: i["role"] in ("list item", "table row") and i["name"].count(", ") >= 3)
    if not rows:
        return False
    node = rows[0][0]
    if do_action(node):
        return True
    parent = safe(lambda: node.parent)
    selection = safe(lambda: parent.querySelection()) if parent is not None else None
    index = safe(lambda: node.getIndexInParent(), 0)
    return selection is not None and bool(safe(lambda: selection.selectChild(index), False))


def screenshot(out_dir, tag):
    """PNG of the app window: the root window cropped to it (xwd -id fails with BadMatch on GTK
    4's ARGB windows)."""
    path = os.path.join(out_dir, f"{tag}.png")
    found = xdotool("search", "--onlyvisible", "--name", "^Dash Wallet").stdout.split()
    geometry = xdotool("getwindowgeometry", "--shell", found[-1]).stdout if found else ""
    box = dict(line.split("=", 1) for line in geometry.split() if "=" in line)
    crop = f"-crop {box['WIDTH']}x{box['HEIGHT']}+{box['X']}+{box['Y']} +repage" if box else ""
    ok = subprocess.run(["sh", "-c", f"import -window root {crop} '{path}'"]).returncode == 0
    print(f"== screenshot {path}: {'ok' if ok else 'FAILED'} {crop}", flush=True)
    return ok


def xdotool(*args):
    return subprocess.run(["xdotool", *args], capture_output=True, text=True, timeout=30)


def focused(frame):
    hits = find_all(frame, lambda n, i: "focused" in i["states"])
    return (hits[-1][1]["role"], hits[-1][1]["name"]) if hits else None


def tab_to(frame, predicate, limit):
    """Presses Tab until the focused element (role, name) satisfies `predicate`."""
    for _ in range(limit):
        now = focused(frame)
        if now and predicate(now):
            return True
        xdotool("key", "Tab")
        time.sleep(0.25)
    now = focused(frame)
    return bool(now and predicate(now))


def tab_cycle(frame, limit=80):
    """Tab until focus returns to where it started (or `limit` presses); the elements it visited."""
    seen = []
    start = focused(frame)
    for _ in range(limit):
        xdotool("key", "--clearmodifiers", "Tab")
        time.sleep(0.25)
        now = focused(frame)
        if now is not None and now not in seen:
            seen.append(now)
        elif now == start and seen:
            break
    return seen


def phase_keys(gate):
    """U4 keyboard only: X key events (xdotool) drive the app; AT-SPI only observes."""
    app = gate.launch(gate.live_args(), tag="keys")
    frame = app_frame(app.pid)
    steps = {}
    if frame is None or not app.wait("lock-paint", 120):
        gate.check("U4", "keyboard: lock screen up", False)
        app.stop()
        return
    xdotool("search", "--sync", "--name", "^Dash Wallet", "windowactivate", "--sync")
    time.sleep(1)
    steps["lockFocus"] = focused(frame)
    steps["lockTabs"] = tab_cycle(frame, 12)
    target = node_named(frame, ("password text", "text", "entry"), "Passphrase", timeout=5)
    for _ in range(12):
        if focused(frame) == ("password text", "Passphrase") or focused(frame) == (role(target), "Passphrase"):
            break
        xdotool("key", "Tab")
        time.sleep(0.25)
    xdotool("type", "--delay", "20", gate.passphrase)
    xdotool("key", "Return")
    wallet = ["sync-overlay-paint", "overview-paint"]
    hit = app.wait_any(wallet, 120, after="lock-paint")
    if hit is None:
        # Return in the field may not submit; Tab to Unlock and press Space.
        steps["unlockNeeded"] = "Tab to Unlock + Space"
        if tab_to(frame, lambda f: f[1] == "Unlock", 6):
            xdotool("key", "space")
        hit = app.wait_any(wallet, 120, after="lock-paint")
    gate.check("U4", "keyboard: unlock", hit is not None, str(steps.get("unlockNeeded", "Return in the field")))
    if hit and app.wait("sync-overlay-paint", 60, after="lock-paint"):
        # The sync overlay (the wallet has no peer): Tab to Hide, Space.
        hidden = tab_to(frame, lambda f: f[1] == "Hide", 20)
        if hidden:
            xdotool("key", "space")
        gate.check("U4", "keyboard: hide the sync overlay (Tab to Hide, Space)",
                   hidden and app.wait("overview-paint", 30, after="sync-overlay-paint") is not None)
    unlocked = app.wait("overview-paint", 5, after="sync-overlay-paint") is not None
    if unlocked:
        time.sleep(1)
        nodes = dump(gate, frame, "keys-overview")
        steps["overviewTabs"] = tab_cycle(frame)
        steps["overviewInteractive"] = [(i["role"], i["name"]) for _n, i in interactive(nodes)]
        missed = [c for c in steps["overviewInteractive"] if c not in steps["overviewTabs"]]
        gate.check("U4", f"keyboard: Tab reaches every Overview control ({len(steps['overviewTabs'])} stops)",
                   not missed, str(missed[:10]))
        # Sidebar: focus it, then Down to Transactions.
        moved = False
        for _ in range(40):
            now = focused(frame)
            if now and now[0] == "list item" and now[1] in ("Overview", "Transactions", "Send", "Receive"):
                for _ in range(6):
                    if app.wait("transactions-paint", 0.1):
                        break
                    xdotool("key", "Down")
                    time.sleep(0.4)
                    xdotool("key", "Return")
                    time.sleep(0.6)
                moved = app.wait("transactions-paint", 10) is not None
                break
            xdotool("key", "Tab")
            time.sleep(0.25)
        gate.check("U4", "keyboard: sidebar to Transactions (Tab, Down, Return)", moved)
        if moved:
            time.sleep(1)
            nodes = dump(gate, frame, "keys-transactions")
            steps["transactionsTabs"] = tab_cycle(frame, 300)
            names = {n for _r, n in steps["transactionsTabs"]}
            gate.check("U4", "keyboard: Tab reaches Load more", "Load more" in names)
            opened = False
            for _ in range(300):
                now = focused(frame)
                if now and now[1].count(", ") >= 3:
                    xdotool("key", "Return")
                    opened = app.wait("detail-paint", 10) is not None
                    break
                xdotool("key", "Tab")
                time.sleep(0.2)
            gate.check("U4", "keyboard: open a transaction's details (Tab to a row, Return)", opened)
            if opened:
                time.sleep(1)
                dump(gate, frame, "keys-detail")
                screenshot(gate.out, "keys-detail")
                steps["detailTabs"] = tab_cycle(frame, 40)
    gate.results["u4-keys"] = steps
    app.stop()


PHASES = {"start": phase_start, "history": phase_history, "pss": phase_pss, "u1": phase_u1,
          "screens": phase_screens, "keys": phase_keys}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--wallet", required=True, help="the wallet's datadir as a .tar.gz")
    parser.add_argument("--passphrase-file", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--dwcli", required=True)
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--phases", default=",".join(PHASES))
    args = parser.parse_args()
    os.makedirs(os.path.join(args.out, "logs"), exist_ok=True)
    gate = Gate(args)
    try:
        for phase in args.phases.split(","):
            print(f"== phase {phase} (load {uptime()})", flush=True)
            PHASES[phase](gate)
            gate.save()
    finally:
        gate.save()
    return 0 if not any(" FAIL " in line for line in gate.lines) else 1


if __name__ == "__main__":
    sys.exit(main())
