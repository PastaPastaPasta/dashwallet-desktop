"""The DashPay suites' harness (DASHPAY §6 tiers T2 and T3): `dwcli` wallets on a network with Platform.

Every DashPay action goes through `dwcli` (ROADMAP E0-09). Each member of a run is a fresh wallet on
its own fresh data root (its own device) under `DW_DP_STATE/runs/<run>`; runs older than three days
are deleted when a run starts. The funder is a `dwcli` wallet restored from a phrase file kept
outside the repository, on a persistent data root `DW_DP_STATE/funder`.

Environment:

| Variable | Meaning |
|---|---|
| `DW_DP_NET` | `testnet` (T3) or `devnet` (T2); the suites are skipped when unset |
| `DW_DP_NET_ARGS` | extra `dwcli` network arguments (T2: `--network regtest --dapi … --ca-cert … --quorum-url … --peer …`) |
| `DW_DP_STATE` | persistent directory (created 0700): the data roots and the vault passphrase |
| `DW_DP_FUNDER` | file holding the funder's BIP39 phrase (first line) |
| `DW_DP_FUNDER_BIRTH` | the funder's birth height |
| `DW_DP_LOGS` | where the run's dwcli logs go (default `<DW_DP_STATE>/logs/<run>`) |
| `DWCLI` | the `dwcli` binary (default `rust/target/debug/dwcli`) |
"""

from __future__ import annotations

import json
import os
import queue
import re
import secrets
import shlex
import shutil
import subprocess
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterator

REPO = Path(__file__).resolve().parents[3]


def dwcli_path() -> Path | None:
    candidates = []
    if os.environ.get("DWCLI"):
        candidates.append(Path(os.environ["DWCLI"]))
    if os.environ.get("CARGO_TARGET_DIR"):
        candidates.append(Path(os.environ["CARGO_TARGET_DIR"]) / "debug" / "dwcli")
    candidates.append(REPO / "rust" / "target" / "debug" / "dwcli")
    return next((c for c in candidates if c.exists()), None)


def wait_until(check: Callable[[], Any], timeout: float, what: str, interval: float = 5.0) -> Any:
    deadline = time.monotonic() + timeout
    while True:
        value = check()
        if value:
            return value
        if time.monotonic() > deadline:
            raise AssertionError(f"timed out after {timeout:.0f} s waiting for {what}")
        time.sleep(interval)


class Redactor:
    """The run's secrets (phrases, passphrases): masked in everything the harness prints, and
    looked for in the logs at the end."""

    def __init__(self) -> None:
        self.values: list[str] = []

    def add(self, value: str) -> None:
        value = value.strip()
        if value and value not in self.values:
            self.values.append(value)

    def mask(self, text: str) -> str:
        text = re.sub(r"(?m)^mnemonic .*$", "mnemonic <redacted>", text)
        for value in self.values:
            text = text.replace(value, "<redacted>")
        return text

    def leaks(self, root: Path) -> list[str]:
        found = []
        for path in sorted(p for p in root.rglob("*") if p.is_file()):
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            found.extend(f"{path}" for value in self.values if value in text)
        return found


@dataclass
class Net:
    """The network the suites run on."""

    tier: str
    args: list[str]

    @classmethod
    def from_env(cls) -> Net | None:
        net = os.environ.get("DW_DP_NET")
        extra = shlex.split(os.environ.get("DW_DP_NET_ARGS", ""))
        if net == "testnet":
            return cls("T3", ["--network", "testnet", *extra])
        if net == "devnet":
            if not extra:
                raise RuntimeError("DW_DP_NET=devnet needs DW_DP_NET_ARGS (the devnet's --dapi, --ca-cert, …)")
            return cls("T2", extra)
        return None

    def without_peers(self) -> list[str]:
        out, skip = [], False
        for arg in self.args:
            if skip:
                skip = False
            elif arg == "--peer":
                skip = True
            elif not arg.startswith("--peer="):
                out.append(arg)
        return out


class DwcliError(AssertionError):
    """A DashPay command that answered `{"ok":false}`."""

    def __init__(self, args: list[str], error: dict, text: str):
        self.code = error.get("code")
        self.params = error.get("params") or {}
        self.error = error
        super().__init__(f"dwcli {' '.join(args)}: {self.code}: {error.get('message')}\n{text}")


@dataclass
class Wallet:
    id: str
    name: str
    mnemonic: str | None = None


@dataclass
class Root:
    """One persistent dwcli data root with its vault."""

    path: Path
    passfile: Path
    net: Net
    redactor: Redactor
    logs: Path
    peers: list[str] | None = None

    def base(self, peers: list[str] | None = None) -> list[str]:
        peers = peers if peers is not None else self.peers
        net = self.net.args if peers is None else self.net.without_peers()
        out = [str(dwcli_path()), "--datadir", str(self.path), *net, "--passphrase-file", str(self.passfile),
               "--shutdown-timeout", os.environ.get("DW_DP_SHUTDOWN_SECS", "120")]
        for peer in peers or []:
            out += ["--peer", peer]
        return out

    def run(self, *args: str, stdin: str | None = None, timeout: float = 900, check: bool = True,
            peers: list[str] | None = None) -> subprocess.CompletedProcess:
        started = time.strftime("%H:%M:%S")
        proc = subprocess.run([*self.base(peers), *args], input=stdin, capture_output=True, text=True,
                              timeout=timeout, check=False)
        with open(self.logs / f"{self.path.name}.log", "a") as log:
            log.write(f"--- {started} $ dwcli {' '.join(args)} -> {proc.returncode}\n")
            log.write(self.redactor.mask(proc.stdout))
            log.write(self.redactor.mask(proc.stderr[-20000:]))
        if check and proc.returncode != 0:
            raise AssertionError(
                f"dwcli {' '.join(args)} exited {proc.returncode}\n"
                f"stdout:\n{self.redactor.mask(proc.stdout)[-4000:]}\nstderr (tail):\n{self.redactor.mask(proc.stderr)[-4000:]}"
            )
        return proc

    def json(self, *args: str, stdin: str | None = None, timeout: float = 900) -> Any:
        proc = self.run(*args, stdin=stdin, timeout=timeout, check=False)
        line = next((ln for ln in reversed(proc.stdout.splitlines()) if ln.startswith("{")), None)
        if line is None:
            raise AssertionError(f"dwcli {' '.join(args)} printed no JSON ({proc.returncode})\n"
                                 f"{self.redactor.mask(proc.stderr)[-4000:]}")
        answer = json.loads(line)
        if not answer.get("ok"):
            raise DwcliError(list(args), answer.get("error") or {}, self.redactor.mask(proc.stderr)[-2000:])
        return answer.get("result")

    def ensure_vault(self) -> None:
        if not (self.path / "vault-created").exists():
            self.run("init-vault")
            (self.path / "vault-created").touch()

    def create(self, name: str) -> Wallet:
        out = self.run("create").stdout
        wallet_id = re.search(r"^wallet_id (\S+)", out, re.M).group(1)
        phrase = re.search(r"^mnemonic (.+)$", out, re.M).group(1).strip()
        self.redactor.add(phrase)
        return Wallet(wallet_id, name, phrase)

    def restore(self, mnemonic: str, birth: int, name: str) -> Wallet:
        self.redactor.add(mnemonic)
        out = self.run("import", "--birth-height", str(birth), "--name", name, stdin=mnemonic + "\n").stdout
        wallet_id = re.search(r"^wallet_id (\S+)", out, re.M)
        wallet_id = wallet_id.group(1) if wallet_id else self.find(name)
        return Wallet(wallet_id, name, mnemonic)

    def find(self, name: str) -> str:
        for line in self.run("list").stdout.splitlines():
            m = re.match(r'(\S+) name="([^"]*)"', line)
            if m and m.group(2) == name:
                return m.group(1)
        raise AssertionError(f"no wallet named {name} in {self.path}")

    def address(self, wallet: str) -> str:
        line = next(ln for ln in self.run("address", wallet).stdout.splitlines() if ln.startswith("address "))
        return line.split()[1]

    def tip(self, timeout: int = 1800) -> int:
        out = self.run("sync", "--timeout-secs", str(timeout), timeout=timeout + 120).stdout
        return int(re.search(r"tip=(\d+)", out).group(1))

    def sync_wallet(self, wallet: str, height: int, timeout: int = 900) -> dict[str, str]:
        out = self.run("sync-wallet", wallet, "--height", str(height), "--timeout-secs", str(timeout),
                       timeout=timeout + 120).stdout
        line = next(ln for ln in out.splitlines() if ln.startswith("synced"))
        return dict(re.findall(r"(\w+)=(\S+)", line))

    def session(self, wallet: str | None = None, peers: list[str] | None = None, name: str = "session") -> Session:
        return Session(self, wallet, peers, name)


class Session:
    """`dwcli dashpay session --spv`: one engine with SPV running, as the app keeps it; one JSON
    request per line, answered in order."""

    def __init__(self, root: Root, wallet: str | None, peers: list[str] | None, name: str):
        self.root = root
        self.wallet = wallet
        self.next_id = 0
        self.log = open(root.logs / f"{root.path.name}-{name}.stderr", "a")
        args = [*root.base(peers), "dashpay", "session", "--spv"]
        if wallet:
            args += ["--wallet", wallet]
        self.proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                     text=True, bufsize=1)
        self.lines: queue.Queue[str | None] = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.transcript = open(root.logs / f"{root.path.name}-{name}.jsonl", "a")

    def _read(self) -> None:
        for line in self.proc.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def call(self, *args: str, wallet: str | None = None, input: str | None = None, timeout: float = 900) -> dict:
        self.next_id += 1
        argv = list(args)
        if (wallet or self.wallet) and "--wallet" not in argv:
            argv += ["--wallet", wallet or self.wallet]
        request: dict[str, Any] = {"args": argv, "id": self.next_id}
        if input is not None:
            request["input"] = input
        self.transcript.write(json.dumps({"request": argv, "at": time.strftime("%H:%M:%S")}) + "\n")
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()
        deadline = time.monotonic() + timeout
        while True:
            try:
                line = self.lines.get(timeout=max(1.0, deadline - time.monotonic()))
            except queue.Empty:
                raise AssertionError(f"dwcli session: no answer to {argv} within {timeout:.0f} s") from None
            if line is None:
                raise AssertionError(f"dwcli session ended ({self.proc.wait()}) before answering {argv}")
            if not line.startswith("{"):
                continue
            answer = json.loads(line)
            if answer.get("id") == self.next_id:
                self.transcript.write(self.root.redactor.mask(line))
                self.transcript.flush()
                return answer

    def ok(self, *args: str, **kw: Any) -> Any:
        answer = self.call(*args, **kw)
        if not answer.get("ok"):
            raise DwcliError(list(args), answer.get("error") or {}, "")
        return answer.get("result")

    def err(self, *args: str, **kw: Any) -> dict:
        answer = self.call(*args, **kw)
        assert not answer.get("ok"), f"{args} succeeded: {answer.get('result')}"
        return answer["error"]

    def close(self, timeout: float = 120) -> None:
        if self.proc.poll() is None:
            self.proc.stdin.close()
            try:
                self.proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.log.close()
        self.transcript.close()

    def __enter__(self) -> Session:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


@dataclass
class Harness:
    """A run: the network, the state directory and the roots in it."""

    net: Net
    state: Path
    logs: Path
    redactor: Redactor
    roots: dict[str, Root] = field(default_factory=dict)
    run_id: str = field(default_factory=lambda: time.strftime("%m%d%H%M") + secrets.token_hex(2))

    @classmethod
    def from_env(cls) -> Harness | None:
        net = Net.from_env()
        if net is None:
            return None
        state = Path(os.environ.get("DW_DP_STATE", "")).expanduser()
        if not str(state) or str(state) == ".":
            raise RuntimeError("set DW_DP_STATE to a persistent directory")
        state.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(state, 0o700)
        redactor = Redactor()
        harness = cls(net, state, Path("."), redactor)
        runs = state / "runs"
        runs.mkdir(mode=0o700, exist_ok=True)
        for old in runs.iterdir():
            if old.is_dir() and time.time() - old.stat().st_mtime > 3 * 86400:
                shutil.rmtree(old, ignore_errors=True)
        logs = Path(os.environ.get("DW_DP_LOGS") or state / "logs" / harness.run_id)
        logs.mkdir(parents=True, exist_ok=True)
        harness.logs = logs
        passfile = state / "pass"
        if not passfile.exists():
            fd = os.open(passfile, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            os.write(fd, (secrets.token_urlsafe(24) + "\n").encode())
            os.close(fd)
        redactor.add(passfile.read_text())
        return harness

    def root(self, role: str) -> Root:
        if role not in self.roots:
            parent = self.state if role == "funder" else self.state / "runs" / self.run_id
            parent.mkdir(mode=0o700, exist_ok=True)
            path = parent / role
            path.mkdir(mode=0o700, exist_ok=True)
            root = Root(path, self.state / "pass", self.net, self.redactor, self.logs)
            root.ensure_vault()
            self.roots[role] = root
        return self.roots[role]

    def label(self, prefix: str) -> str:
        """A non-contested label (it holds a digit 2-9, DIP-23) nobody holds yet."""
        digits = "".join(secrets.choice("23456789") for _ in range(6))
        return f"{prefix}{digits}"

    def funder(self) -> Funder:
        phrase_file = os.environ.get("DW_DP_FUNDER")
        birth = os.environ.get("DW_DP_FUNDER_BIRTH")
        if not phrase_file or not birth:
            raise RuntimeError("set DW_DP_FUNDER (a phrase file outside the repository) and DW_DP_FUNDER_BIRTH")
        root = self.root("funder")
        phrase = Path(phrase_file).expanduser().read_text().splitlines()[0].strip()
        self.redactor.add(phrase)
        try:
            wallet = root.find("funder")
        except AssertionError:
            wallet = root.restore(phrase, int(birth), "funder").id
        return Funder(root, wallet)

    def close(self) -> list[str]:
        return self.redactor.leaks(self.logs)


@dataclass
class Funder:
    """The run's source of Core funds. Its own change is spendable only once mined (a restarted
    process no longer knows the change's InstantSend lock), so it keeps several mined coins and
    each payment spends one of them."""

    root: Root
    wallet: str
    coin_duffs: int = 50_000_000
    coins: int = 12

    def mined_coins(self) -> list[int]:
        out = []
        for line in self.root.run("utxos", self.wallet).stdout.splitlines():
            m = re.match(r"utxo \S+ (\d+) conf=(\d+) locked=0 reserved=0", line)
            if m and int(m.group(2)) > 0:
                out.append(int(m.group(1)))
        return out

    def ensure_coins(self, duffs: int) -> int:
        """Splits the funder's balance into mined coins when fewer than four can pay `duffs`;
        returns the scanned tip."""
        tip = self.root.tip()
        if sum(1 for c in self.mined_coins() if c >= duffs) >= 4:
            return tip
        each = max(self.coin_duffs, duffs + 100_000)
        to = []
        for _ in range(self.coins):
            to += ["--to", f"{self.root.address(self.wallet)}:{each}"]
        wait_until(lambda: self._try_send(to, tip), 1800, "a mined coin to split", interval=30)
        return wait_until(lambda: self._mined(duffs), 1800, "the split coins to be mined", interval=30)

    def _try_send(self, to: list[str], height: int) -> str | None:
        """The txid, or `None` when nothing was sent and asking again may help: no mined coin
        yet, or SPV has not connected a peer."""
        proc = self.root.run("send", self.wallet, *to, "--sync-height", str(height), "--rebroadcast-unknown", "3",
                             check=False)
        if proc.returncode == 0:
            return re.search(r"^broadcast (\S+)$", proc.stdout, re.M).group(1)
        if "exceeds the available" in proc.stderr or "no connected peers" in proc.stderr:
            return None
        raise AssertionError(f"funder send failed: {self.root.redactor.mask(proc.stderr)[-2000:]}")

    def _mined(self, duffs: int) -> int | None:
        tip = self.root.tip()
        return tip if sum(1 for c in self.mined_coins() if c >= duffs) >= 4 else None

    def pay(self, to: Root, wallet: str, duffs: int, coins: int = 1) -> tuple[str, int]:
        """Pays `duffs` to `wallet` as `coins` equal outputs and waits until `to` has scanned a
        block that holds them; returns the txid and the height `to` scanned."""
        tip = self.ensure_coins(duffs)
        outputs = []
        for _ in range(coins):
            outputs += ["--to", f"{to.address(wallet)}:{duffs // coins}"]
        txid = wait_until(lambda: self._try_send(outputs, tip), 600, "the funder's payment to go out", interval=15)
        duffs = duffs // coins * coins
        height = tip
        while True:
            height += 1
            balances = to.sync_wallet(wallet, height, timeout=1200)
            if int(balances.get("total", "0")) >= duffs:
                return txid, height
            if height > tip + 6:
                raise AssertionError(f"{wallet} saw no funding {txid} by height {height}: {balances}")


def ids(rows: list[dict], key: str = "identity") -> list[str]:
    return [row[key] for row in rows]


FINAL_PHASES = ("done", "failed")


def registration_row(session: Session, wallet: str, draft: str) -> dict:
    rows = session.ok("identity", "registrations", wallet=wallet)
    return next(r for r in rows if r["draft"] == draft)


def register(session: Session, wallet: str, label: str, timeout: float = 2400) -> tuple[dict, dict, list[dict]]:
    """`identity register --label` from the Core balance, driven to its end in the session: the
    quote, the final row, and every distinct (phase, waiting) the row went through."""
    started = session.ok("identity", "register", "--label", label, wallet=wallet, timeout=1200)
    quote, draft = started["quote"], started["draft"]
    seen: list[dict] = []

    def final() -> dict | None:
        row = registration_row(session, wallet, draft)
        step = {"phase": row["phase"], "waiting": row["waiting"]}
        if not seen or seen[-1] != step:
            seen.append(step)
        if row["phase"] in FINAL_PHASES:
            return row
        if row["waiting"] in ("unlock", "authorize") and not row["running"]:
            grant = quote["grant"]
            session.ok("identity", "resume", draft, "--max-duffs", str(grant["max_duffs"]),
                       "--max-credits", str(grant["max_credits"]), wallet=wallet)
        elif not row["running"]:
            session.ok("identity", "resume", draft, wallet=wallet)
        return None

    row = wait_until(final, timeout, f"registration {draft} ({label}) to end", interval=10)
    return quote, row, seen


def wait_pass(session: Session, loops: tuple[str, ...] = ("dash_pay_sync",), timeout: float = 900) -> dict:
    """Waits until each of the engine's Platform sync loops has run once after now (`dashpay sync`
    is not implemented: `DashPay.sync_now`); returns the sync status."""
    since = time.time()

    def done() -> dict | None:
        status = session.ok("dashpay", "sync-status")
        ran = {loop["sync_loop"]: loop["last_run_at"] for loop in status["loops"]}

        def after(at: int | None) -> bool:
            return at is not None and (at / 1000 if at > 10**11 else at) >= since
        return status if all(after(ran.get(loop)) for loop in loops) else None

    return wait_until(done, timeout, f"a pass of {', '.join(loops)}", interval=10)


def iter_contacts(page: dict) -> Iterator[tuple[str, dict]]:
    for section in page["sections"]:
        for row in section["contacts"]:
            yield section["section"], row


@dataclass
class Member:
    """A fresh wallet with a registered identity, on its own data root (its own device)."""

    root: Root
    wallet: Wallet
    identity: str
    label: str
    birth: int

    def session(self, name: str, peers: list[str] | None = None) -> Session:
        return self.root.session(self.wallet.id, peers=peers, name=name)


def make_member(dp: Harness, funder: Funder, role: str, prefix: str, duffs: int, coins: int = 8) -> Member:
    """A fresh wallet funded with `coins` mined coins, and a registered identity. Several coins,
    as for the funder: a payment's change is spendable only once mined in the next process."""
    root = dp.root(role)
    birth = root.tip()
    wallet = root.create(f"{prefix}-{dp.run_id}")
    funder.pay(root, wallet.id, duffs, coins)
    label = dp.label(prefix)
    with root.session(wallet.id, name=f"{prefix}-register") as s:
        _, row, steps = register(s, wallet.id, label)
    assert row["phase"] == "done" and row["name_failure"] is None, (row, steps)
    return Member(root, wallet, row["identity"], label, birth)


def contact_page(session: Session, *args: str) -> dict:
    """`contact list`, read again while a sync pass holds the contacts state (`updating`)."""
    return wait_until(lambda: (p := session.ok("contact", "list", *args)) and not p["updating"] and p, 300,
                      "a contacts page no sync pass holds", interval=3)


def find_contact(session: Session, contact: str) -> tuple[str, dict] | None:
    page = contact_page(session)
    return next(((sec, row) for sec, row in iter_contacts(page) if row["contact"] == contact), None)


def try_pay(session: Session, contact: str, duffs: int, *extra: str, timeout: float = 900) -> dict:
    """`pay-contact`'s answer, asked again while nothing could be sent yet: the session's SPV has
    not connected a peer (the engine reports it as a rejection, see test_dp3_payments) or the
    wallet is still rescanning."""

    def once() -> dict | None:
        answer = session.call("pay-contact", contact, "--amount", str(duffs), *extra, timeout=900)
        error = answer.get("error") or {}
        if error.get("code") == "rescan_in_progress" or "no connected peers" in error.get("message", ""):
            return None
        return answer

    return wait_until(once, timeout, "a connected peer and no rescan to pay through", interval=10)


def send_kind(error: dict) -> str:
    """The `SendFailure` a `send` error is (dwcli's DashPay commands give every one the code
    `send`; the message names it)."""
    message = error.get("message", "")
    for prefix, kind in (("send: no peers", "no_peers"), ("send: broadcast rejected", "rejected"),
                         ("send: broadcast outcome unknown", "unknown"), ("send: cancelled", "cancelled")):
        if message.startswith(prefix):
            return kind
    return error.get("code", "")


def pay_contact(session: Session, contact: str, duffs: int, *extra: str) -> dict:
    """A payment that went out. An unknown outcome (on a loaded host the lease's deadline can cut
    the broadcast's hand-off) is settled as the app does, through the contact's lock: `sent` is
    the payment, under the lock's txid; `not_sent` pays again."""
    answer = try_pay(session, contact, duffs, *extra)
    if answer.get("ok"):
        return answer["result"]
    error = answer["error"]
    lock = session.ok("contact", "lock", contact) if send_kind(error) == "unknown" else None
    if not lock:
        raise DwcliError(["pay-contact", contact], error, "")
    print(f"pay-contact {duffs}: {error['message']}; resolving the lock on {lock['txid']}")
    outcome = wait_until(lambda: (o := session.ok("contact", "resolve-lock", contact)) != "unknown" and o, 900,
                         f"the lock on {lock['txid']} to resolve", interval=15)
    if outcome == "sent":
        return {"txid": lock["txid"], "resolved": outcome}
    answer = try_pay(session, contact, duffs, *extra)
    if not answer.get("ok"):
        raise DwcliError(["pay-contact", contact], answer["error"], "")
    return answer["result"]


def synced_until(session: Session, check: Callable[[], Any], timeout: float, what: str) -> Any:
    """Polls `check` while the session's DashPay sync loops run."""
    return wait_until(check, timeout, what, interval=15)


def befriend(a: Member, b: Member, timeout: float = 1200, live: float = 300) -> dict:
    """A asks B, B accepts: the round trip, with what each side saw on the way. A's session stays
    open while B accepts, as an app left unlocked; `a_live` is A's row in that session `live`
    seconds after it lists B as a contact, `a_contact` A's row once ready in a new process."""
    seen: dict[str, Any] = {}

    def contact_of(s: Session, other: Member, ready: bool) -> Callable[[], Any]:
        def check() -> Any:
            r = find_contact(s, other.identity)
            done = r and r[1]["relation"] == "contact" and r[1]["username"]
            return r if done and (r[1]["channel"] == "ready" or not ready) else None
        return check

    with a.session("befriend-a") as sa:
        seen["eligibility"] = sa.ok("contact", "eligibility", b.identity)
        seen["request"] = sa.ok("contact", "request", b.identity, timeout=1200)
        seen["a_pending"] = find_contact(sa, b.identity)
        with b.session("befriend-b") as sb:
            seen["b_incoming"] = synced_until(
                sb, lambda: (r := find_contact(sb, a.identity)) and r[1]["relation"] == "pending_incoming"
                and r[1]["username"] and r, timeout, "B to list A's request with A's username")
            seen["accept"] = sb.ok("contact", "accept", a.identity, timeout=1200)
            seen["b_contact"] = synced_until(sb, contact_of(sb, a, True), timeout, "B's contact A to be ready")
        row = synced_until(sa, contact_of(sa, b, False), timeout, "A to list B as a contact")
        deadline = time.monotonic() + live
        while row[1]["channel"] != "ready" and time.monotonic() < deadline:
            time.sleep(15)
            row = find_contact(sa, b.identity)
        seen["a_live"] = row
    with a.session("befriend-a2") as sa:
        seen["a_contact"] = synced_until(sa, contact_of(sa, b, True), timeout, "A's contact B to be ready")
    return seen
