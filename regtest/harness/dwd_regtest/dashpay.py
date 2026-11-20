"""The DashPay suites' harness (DASHPAY §6 tiers T2 and T3): `dwcli` wallets on a network with Platform.

Every DashPay action goes through `dwcli` (ROADMAP E0-09). Each member of a run is a fresh wallet on
its own fresh data root (its own device) under `DW_DP_STATE/runs/<run>`; a run directory carries a
marker the harness wrote, and marked runs older than three days are deleted when a run starts. The
funder is a `dwcli` wallet restored from a phrase file kept outside the repository, on a persistent
data root `DW_DP_STATE/funder`.

Environment:

| Variable | Meaning |
|---|---|
| `DW_DP_NET` | `testnet` (T3) or `devnet` (T2, dashmate local); unset skips, any other value is an error |
| `DW_DP_NET_ARGS` | only `--dapi`, `--ca-cert`, `--quorum-url`, `--peer`; the harness owns `--network` |
| `DW_DP_STATE` | absolute, outside the checkout, this user's, 0700 (created so): data roots, vault passphrase |
| `DW_DP_FUNDER` | file holding the funder's BIP39 phrase (first line); owner-only (0600 or 0400) |
| `DW_DP_FUNDER_BIRTH` | the funder's birth height |
| `DW_DP_LOGS` | absolute directory for the run's dwcli logs (default `<DW_DP_STATE>/logs/<run>`), 0700 |
| `DW_DP_SCAN` | colon-separated files the end-of-run secret scan also reads (the nightly's pytest output) |
| `DW_DP_STRICT` | `1`: a case that would skip for want of configuration (the relay) fails instead |
| `DWCLI` | the `dwcli` binary (default `rust/target/debug/dwcli`) |
"""

from __future__ import annotations

import hashlib
import json
import os
import queue
import re
import secrets
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO, Any, Callable, Iterable, Iterator

REPO = Path(__file__).resolve().parents[3]
MASK = "<redacted>"
RUN_MARKER = ".dw-dp-run"
RUN_MAX_AGE = 3 * 86400
SCAN_OK = "secret-scan.ok"

NETWORKS = {"testnet": ("T3", "testnet"), "devnet": ("T2", "regtest")}
NET_OPTIONS = ("--dapi", "--ca-cert", "--quorum-url", "--peer")

FUNDER_RETRY_KINDS = ("no_peers", "amount_exceeds_balance", "amount_with_fee_exceeds_balance")

MNEMONIC_LINE = re.compile(r"(?m)^mnemonic .*$")
PHRASE_SHAPE = re.compile(r"\b[a-z]{3,8}(?:[\s,\"']+[a-z]{3,8}){11,}\b")


class HarnessConfigError(RuntimeError):
    """The DashPay suites' environment is set, but wrong."""


def dwcli_path() -> Path | None:
    candidates = []
    if os.environ.get("DWCLI"):
        candidates.append(Path(os.environ["DWCLI"]))
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


class Secret:
    """A phrase or passphrase. Its `repr` and `str` are a mask, so a traceback's locals, an
    f-string or a dataclass `repr` never show it; `reveal()` is the value."""

    __slots__ = ("_value",)

    def __init__(self, value: str):
        self._value = value.strip()

    def __repr__(self) -> str:
        return f"Secret({MASK})"

    __str__ = __repr__

    def __bool__(self) -> bool:
        return bool(self._value)

    def reveal(self) -> str:
        return self._value

    def _pattern(self, words: int | None = None) -> re.Pattern[str]:
        parts = self._value.split()
        if words is None or len(parts) < 2:
            return re.compile(r"[^a-z]{1,8}".join(map(re.escape, parts)) if len(parts) > 1 else re.escape(self._value))
        return re.compile("|".join(r"[^a-z]{1,8}".join(map(re.escape, parts[i:i + words]))
                                   for i in range(len(parts) - words + 1)))

    def masked_in(self, text: str) -> str:
        return self._pattern().sub(MASK, text.replace(self._value, MASK))

    def found_in(self, text: str) -> bool:
        """The whole value, or (for a phrase) any four of its words in a row, whatever separates
        them."""
        if self._value in text:
            return True
        return len(self._value.split()) >= 4 and self._pattern(4).search(text) is not None


class Redactor:
    """The run's secrets (phrases, passphrases): masked in everything the harness writes or
    raises, and looked for in every artifact at the end of the run."""

    def __init__(self) -> None:
        self._secrets: list[Secret] = []

    def __len__(self) -> int:
        return len(self._secrets)

    def add(self, secret: Secret) -> Secret:
        if secret and all(s.reveal() != secret.reveal() for s in self._secrets):
            self._secrets.append(secret)
        return secret

    def mask(self, text: str, shape: bool = True) -> str:
        """`text` with every registered secret masked; with `shape`, also anything shaped like a
        BIP39 phrase (twelve or more short lowercase words) and any `mnemonic …` line."""
        for secret in self._secrets:
            text = secret.masked_in(text)
        if shape:
            text = PHRASE_SHAPE.sub(MASK, MNEMONIC_LINE.sub("mnemonic " + MASK, text))
        return text

    def found_in(self, text: str) -> bool:
        return any(secret.found_in(text) for secret in self._secrets)

    def scan(self, paths: Iterable[Path]) -> tuple[list[str], list[str]]:
        """The files under `paths` holding a secret, and the ones that could not be read (a
        missing path counts as unreadable)."""
        leaked: list[str] = []
        unreadable: list[str] = []
        for path in _files(paths, unreadable):
            try:
                found = self.found_in(path.read_bytes().decode("utf-8", "replace"))
            except OSError as e:
                unreadable.append(f"{path} ({type(e).__name__})")
                continue
            if found:
                leaked.append(str(path))
        return leaked, unreadable


def _files(paths: Iterable[Path], unreadable: list[str]) -> Iterator[Path]:
    for path in paths:
        if path.is_dir() and not path.is_symlink():
            def failed(e: OSError) -> None:
                unreadable.append(f"{e.filename} ({type(e).__name__})")
            for top, _, names in os.walk(path, onerror=failed):
                for name in sorted(names):
                    yield Path(top) / name
        elif path.exists():
            yield path
        else:
            unreadable.append(f"{path} (missing)")


def _check_private(path: Path, what: str, st: os.stat_result, kind: int) -> None:
    if stat.S_IFMT(st.st_mode) != kind:
        raise HarnessConfigError(f"{what} {path} is not a {'directory' if kind == stat.S_IFDIR else 'regular file'}")
    if st.st_uid != os.geteuid():
        raise HarnessConfigError(f"{what} {path} is not owned by this user")
    if st.st_mode & 0o077:
        raise HarnessConfigError(
            f"{what} {path} is open to group or others (mode {stat.S_IMODE(st.st_mode):o}); make it owner-only")


def _outside_repo(path: Path, what: str) -> None:
    resolved = Path(os.path.realpath(path))
    if resolved == REPO or REPO in resolved.parents:
        raise HarnessConfigError(f"{what} {path} is inside the checkout {REPO}; keep it outside the repository")


def private_dir(path: Path, what: str) -> Path:
    """`path` (absolute, outside the checkout) as a directory owned by this user with no group
    or other access; created 0700 when missing. A symlink at `path` itself is refused."""
    if not path.is_absolute():
        raise HarnessConfigError(f"{what} must be an absolute path, not {str(path)!r}")
    _outside_repo(path, what)
    try:
        st = os.lstat(path)
    except FileNotFoundError:
        path.mkdir(mode=0o700, parents=True)
        st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode):
        raise HarnessConfigError(f"{what} {path} is a symlink")
    _check_private(path, what, st, stat.S_IFDIR)
    return path


def private_file(path: Path, what: str, follow: bool = False) -> Path:
    """An existing regular file owned by this user with no group or other access. `follow`:
    `path` may be a symlink (the secret store's), checked at its target."""
    st = os.stat(path) if follow else os.lstat(path)
    if not follow and stat.S_ISLNK(st.st_mode):
        raise HarnessConfigError(f"{what} {path} is a symlink")
    _check_private(path, what, st, stat.S_IFREG)
    return path


def read_secret(path: Path) -> Secret:
    """The first line of `path`."""
    return Secret(path.read_text().partition("\n")[0])


def clean_runs(runs: Path, now: float | None = None) -> list[str]:
    """Deletes the run directories under `runs` that carry the harness's marker and are older
    than three days; never follows a symlink. Returns what it deleted."""
    if not shutil.rmtree.avoids_symlink_attacks:
        return []
    now = time.time() if now is None else now
    deleted = []
    for entry in os.scandir(runs):
        if entry.is_symlink() or not entry.is_dir(follow_symlinks=False):
            continue
        marker = Path(entry.path) / RUN_MARKER
        if not os.path.lexists(marker):
            continue
        try:
            st = os.lstat(marker)
            if not stat.S_ISREG(st.st_mode) or st.st_uid != os.geteuid():
                continue
            if marker.read_text().strip() != entry.name or now - st.st_mtime <= RUN_MAX_AGE:
                continue
            shutil.rmtree(entry.path)
            deleted.append(entry.name)
        except OSError as e:
            print(f"DashPay run cleanup: kept {entry.path}: {type(e).__name__}: {e.strerror}", file=sys.stderr)
    return deleted


def net_args(text: str) -> list[str]:
    """`DW_DP_NET_ARGS`: only the options of NET_OPTIONS, each with a value; never `--network`
    or anything naming mainnet."""
    tokens = shlex.split(text)
    if any("mainnet" in t.lower() for t in tokens):
        raise HarnessConfigError("DW_DP_NET_ARGS names mainnet; the DashPay suites never run there")
    out: list[str] = []
    i = 0
    while i < len(tokens):
        option, eq, value = tokens[i].partition("=")
        if option == "--network":
            raise HarnessConfigError("DW_DP_NET_ARGS may not set --network; DW_DP_NET chooses the network")
        if option not in NET_OPTIONS:
            raise HarnessConfigError(f"DW_DP_NET_ARGS: {tokens[i]!r} is not one of {', '.join(NET_OPTIONS)}")
        if not eq:
            if i + 1 >= len(tokens) or tokens[i + 1].startswith("-"):
                raise HarnessConfigError(f"DW_DP_NET_ARGS: {option} needs a value")
            i += 1
            value = tokens[i]
        out += [option, value]
        i += 1
    return out


@dataclass
class Net:
    """The network the suites run on. The harness owns `--network`."""

    tier: str
    args: list[str]

    @classmethod
    def from_env(cls) -> Net | None:
        net = os.environ.get("DW_DP_NET", "")
        if not net:
            return None
        if net not in NETWORKS:
            raise HarnessConfigError(f"DW_DP_NET={net!r}: not one of {', '.join(NETWORKS)} (unset it to skip)")
        tier, network = NETWORKS[net]
        extra = net_args(os.environ.get("DW_DP_NET_ARGS", ""))
        if net == "devnet" and "--dapi" not in extra:
            raise HarnessConfigError(
                "DW_DP_NET=devnet needs DW_DP_NET_ARGS with the devnet's --dapi (and --ca-cert, …)")
        return cls(tier, ["--network", network, *extra])

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
        super().__init__(f"dwcli {' '.join(args)}: {self.code}: {error.get('message')} {self.params}\n{text}")


@dataclass
class Wallet:
    id: str
    name: str
    mnemonic: Secret | None = field(default=None, repr=False)


class _Buffer:
    """Raw subprocess output before it is masked; its `repr` is only a size."""

    __slots__ = ("_data",)

    def __init__(self) -> None:
        self._data = bytearray()

    def __repr__(self) -> str:
        return f"<{len(self._data)} bytes of unmasked output>"

    def extend(self, chunk: bytes) -> None:
        self._data.extend(chunk)

    def secrets(self, pattern: str) -> list[Secret]:
        return [Secret(m.group(1)) for m in re.finditer(pattern, self._data.decode("utf-8", "replace"), re.M)]

    def masked(self, redactor: Redactor) -> str:
        return redactor.mask(self._data.decode("utf-8", "replace"))

    def clear(self) -> None:
        self._data[:] = b""


def _drain(stream: IO[bytes], sink: _Buffer) -> None:
    for chunk in iter(lambda: stream.read1(65536), b""):
        sink.extend(chunk)
    stream.close()


def _secret_pipe(secret: Secret) -> int:
    """A pipe's read end holding `secret` and a newline: a child's stdin that puts the value on
    no command line, in no file and in no local of the caller."""
    r, w = os.pipe()
    try:
        os.write(w, (secret.reveal() + "\n").encode())
    finally:
        os.close(w)
    return r


@dataclass(repr=False)
class Output:
    """A finished dwcli run: its exit status and its output, masked."""

    returncode: int
    stdout: str
    stderr: str
    timed_out: bool = False
    secrets: list[Secret] = field(default_factory=list)

    def __repr__(self) -> str:
        return f"Output(returncode={self.returncode}, stdout={len(self.stdout)} chars, stderr={len(self.stderr)} chars)"


def execute(argv: list[str], redactor: Redactor, stdin: Secret | None = None, timeout: float = 900,
            secret_line: str | None = None) -> Output:
    """Runs `argv` with `stdin` on a pipe. Every secret its stdout prints on a `secret_line`
    match is registered with `redactor` before any of the output is masked; with a
    `secret_line` that matches nothing, the stdout is withheld."""
    stdin_fd = _secret_pipe(stdin) if stdin is not None else subprocess.DEVNULL
    try:
        proc = subprocess.Popen(argv, stdin=stdin_fd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    finally:
        if stdin is not None:
            os.close(stdin_fd)
    out, err = _Buffer(), _Buffer()
    readers = [threading.Thread(target=_drain, args=(proc.stdout, out), daemon=True),
               threading.Thread(target=_drain, args=(proc.stderr, err), daemon=True)]
    for reader in readers:
        reader.start()
    timed_out = False
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        proc.kill()
        proc.wait()
    for reader in readers:
        reader.join(30)
    found: list[Secret] = []
    if secret_line is not None:
        found = [redactor.add(s) for s in out.secrets(secret_line) if s]
    stdout = out.masked(redactor) if secret_line is None or found else "<stdout withheld: no secret line>\n"
    stderr = err.masked(redactor)
    out.clear()
    err.clear()
    return Output(proc.returncode, stdout, stderr, timed_out, found)


@dataclass
class Root:
    """One persistent dwcli data root with its vault."""

    path: Path
    passfile: Path
    net: Net
    redactor: Redactor = field(repr=False)
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

    def run(self, *args: str, stdin: Secret | None = None, timeout: float = 900, check: bool = True,
            peers: list[str] | None = None, secret_line: str | None = None) -> Output:
        started = time.strftime("%H:%M:%S")
        out = execute([*self.base(peers), *args], self.redactor, stdin=stdin, timeout=timeout,
                      secret_line=secret_line)
        with open(self.logs / f"{self.path.name}.log", "a") as log:
            log.write(f"--- {started} $ dwcli {' '.join(args)} -> {out.returncode}"
                      f"{' (timed out)' if out.timed_out else ''}\n")
            log.write(out.stdout)
            log.write(out.stderr[-20000:])
        if out.timed_out:
            raise AssertionError(f"dwcli {' '.join(args)} timed out after {timeout:.0f} s\n"
                                 f"stderr (tail):\n{out.stderr[-4000:]}")
        if secret_line is not None and not out.secrets:
            raise AssertionError(f"dwcli {' '.join(args)} printed no line matching {secret_line!r}; "
                                 "its output was withheld")
        if check and out.returncode != 0:
            raise AssertionError(
                f"dwcli {' '.join(args)} exited {out.returncode}\n"
                f"stdout:\n{out.stdout[-4000:]}\nstderr (tail):\n{out.stderr[-4000:]}"
            )
        return out

    def json(self, *args: str, timeout: float = 900) -> Any:
        out = self.run(*args, timeout=timeout, check=False)
        line = next((ln for ln in reversed(out.stdout.splitlines()) if ln.startswith("{")), None)
        if line is None:
            raise AssertionError(f"dwcli {' '.join(args)} printed no JSON ({out.returncode})\n{out.stderr[-4000:]}")
        answer = json.loads(line)
        if not answer.get("ok"):
            raise DwcliError(list(args), answer.get("error") or {}, out.stderr[-2000:])
        return answer.get("result")

    def ensure_vault(self) -> None:
        if not (self.path / "vault-created").exists():
            self.run("init-vault")
            (self.path / "vault-created").touch()

    def create(self, name: str) -> Wallet:
        out = self.run("create", secret_line=r"^mnemonic (\S.*?)\s*$")
        if len(out.secrets) != 1:
            raise AssertionError(f"dwcli create printed {len(out.secrets)} mnemonic lines")
        wallet_id = re.search(r"^wallet_id (\S+)", out.stdout, re.M)
        if wallet_id is None:
            raise AssertionError(f"dwcli create printed no wallet_id\n{out.stdout[-2000:]}")
        return Wallet(wallet_id.group(1), name, out.secrets[0])

    def restore(self, mnemonic: Secret, birth: int, name: str) -> Wallet:
        self.redactor.add(mnemonic)
        out = self.run("import", "--birth-height", str(birth), "--name", name, stdin=mnemonic)
        wallet_id = re.search(r"^wallet_id (\S+)", out.stdout, re.M)
        return Wallet(wallet_id.group(1) if wallet_id else self.find(name), name, mnemonic)

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

    def extras(self, wallet: str, txid: str) -> dict[str, str]:
        out = self.run("extras", wallet, txid).stdout
        line = next(ln for ln in out.splitlines() if ln.startswith("extras "))
        return dict(re.findall(r"(\w+)=(\S+)", line))

    def session(self, wallet: str | None = None, peers: list[str] | None = None, name: str = "session") -> Session:
        return Session(self, wallet, peers, name)


class Session:
    """`dwcli dashpay session --spv`: one engine with SPV running, as the app keeps it; one JSON
    request per line, answered in order. Its stderr is masked line by line on its way to the log."""

    def __init__(self, root: Root, wallet: str | None, peers: list[str] | None, name: str):
        self.root = root
        self.wallet = wallet
        self.next_id = 0
        self.log = open(root.logs / f"{root.path.name}-{name}.stderr", "a")
        args = [*root.base(peers), "dashpay", "session", "--spv"]
        if wallet:
            args += ["--wallet", wallet]
        self.proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     text=True, bufsize=1)
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.readers = [threading.Thread(target=self._read, daemon=True),
                        threading.Thread(target=self._stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        self.transcript = open(root.logs / f"{root.path.name}-{name}.jsonl", "a")

    def _read(self) -> None:
        for line in self.proc.stdout:
            self.lines.put(self.root.redactor.mask(line, shape=False))
        self.lines.put(None)

    def _stderr(self) -> None:
        for line in self.proc.stderr:
            self.log.write(self.root.redactor.mask(line))
        self.log.flush()

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
        for reader in self.readers:
            reader.join(30)
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
    redactor: Redactor = field(repr=False)
    run_dir: Path = Path(".")
    strict: bool = False
    roots: dict[str, Root] = field(default_factory=dict)
    run_id: str = field(default_factory=lambda: time.strftime("%m%d%H%M") + secrets.token_hex(2))

    @classmethod
    def from_env(cls) -> Harness | None:
        net = Net.from_env()
        if net is None:
            return None
        os.umask(0o077)
        raw = os.environ.get("DW_DP_STATE", "")
        if not raw:
            raise HarnessConfigError("set DW_DP_STATE to an absolute directory outside the checkout")
        state = Path(raw).expanduser()
        state = private_dir(Path(os.path.realpath(state)) if state.is_absolute() else state, "DW_DP_STATE")
        redactor = Redactor()
        passfile = state / "pass"
        if not os.path.lexists(passfile):
            fd = os.open(passfile, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            try:
                os.write(fd, (secrets.token_urlsafe(24) + "\n").encode())
            finally:
                os.close(fd)
        redactor.add(read_secret(private_file(passfile, "the vault passphrase file")))
        funder = os.environ.get("DW_DP_FUNDER")
        if funder:
            path = Path(funder).expanduser()
            _outside_repo(path, "DW_DP_FUNDER")
            redactor.add(read_secret(private_file(path, "DW_DP_FUNDER", follow=True)))
        harness = cls(net, state, Path("."), redactor, strict=os.environ.get("DW_DP_STRICT") == "1")
        runs = private_dir(state / "runs", "the runs directory")
        clean_runs(runs)
        harness.run_dir = runs / harness.run_id
        harness.run_dir.mkdir(mode=0o700)
        (harness.run_dir / RUN_MARKER).write_text(harness.run_id + "\n")
        logs = os.environ.get("DW_DP_LOGS")
        if logs:
            harness.logs = private_dir(Path(logs).expanduser(), "DW_DP_LOGS")
        else:
            if os.path.islink(state / "logs"):
                raise HarnessConfigError(f"{state / 'logs'} is a symlink")
            (state / "logs").mkdir(mode=0o700, exist_ok=True)
            harness.logs = private_dir(state / "logs" / harness.run_id, "the run's log directory")
        return harness

    def root(self, role: str) -> Root:
        if role not in self.roots:
            parent = self.state if role == "funder" else self.run_dir
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
            raise HarnessConfigError("set DW_DP_FUNDER (a phrase file outside the repository) and DW_DP_FUNDER_BIRTH")
        root = self.root("funder")
        try:
            wallet = root.find("funder")
        except AssertionError:
            phrase = self.redactor.add(read_secret(Path(phrase_file).expanduser()))
            wallet = root.restore(phrase, int(birth), "funder").id
        return Funder(root, wallet)

    def scan(self, extra: Iterable[Path] = ()) -> bool:
        """The end-of-run secret scan of the logs directory and `extra` (the junit file, the
        nightly's pytest output): prints its verdict, writes `secret-scan.ok` in the logs
        directory only when every file was read and none holds a secret."""
        paths = [self.logs, *extra]
        ok_file = self.logs / SCAN_OK
        if ok_file.exists():
            ok_file.unlink()
        leaked, unreadable = self.redactor.scan(paths)
        if leaked or unreadable:
            print(f"DashPay secret scan FAILED: a secret in {leaked or 'no file'}; unreadable: {unreadable or 'none'}",
                  file=sys.__stderr__, flush=True)
            return False
        ok_file.write_text(f"{len(self.redactor)} secrets, none in {', '.join(map(str, paths))}\n")
        print(f"DashPay secret scan: no secret in {', '.join(map(str, paths))}", file=sys.__stderr__, flush=True)
        return True


def parse_send_failure(stdout: str) -> dict[str, str] | None:
    """`dwcli send`'s last line on a failure: `failed outcome=… kind=… txid=…[ reason=…]`."""
    m = re.search(r"^failed outcome=(\S+) kind=(\S+) txid=(\S+)(?: reason=(.*))?$", stdout, re.M)
    if m is None:
        return None
    return {"outcome": m.group(1), "kind": m.group(2), "txid": "" if m.group(3) == "-" else m.group(3),
            "reason": m.group(4) or ""}


def funder_may_retry(failure: dict[str, str]) -> bool:
    """A send that definitely sent nothing and may go through if asked again: no mined coin yet,
    or no peer connected."""
    return failure["outcome"] == "not_sent" and failure["kind"] in FUNDER_RETRY_KINDS


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
        txid = wait_until(lambda: self._try_send(to, tip), 1800, "a mined coin to split", interval=30)
        return wait_until(lambda: self._mined(duffs), 1800, f"the split coins of {txid} to be mined", interval=30)

    def _try_send(self, to: list[str], height: int) -> str | None:
        """The txid, or `None` when dwcli reports that nothing was sent and asking again may
        help. Anything that may have reached the network keeps its txid: the caller waits for
        it, never pays again."""
        out = self.root.run("send", self.wallet, *to, "--sync-height", str(height), "--rebroadcast-unknown", "3",
                            check=False)
        if out.returncode == 0:
            return re.search(r"^broadcast (\S+)$", out.stdout, re.M).group(1)
        failure = parse_send_failure(out.stdout)
        if failure is not None and funder_may_retry(failure):
            return None
        prepared = re.search(r"^prepared txid=(\S+)", out.stdout, re.M)
        txid = (failure or {}).get("txid") or (prepared.group(1) if prepared else "")
        if failure is not None and failure["outcome"] == "unknown" and txid:
            print(f"funder send {txid}: outcome unknown ({failure['kind']}: {failure['reason']}); "
                  "waiting for it, not paying again")
            return txid
        raise AssertionError(f"funder send failed{f' (prepared {txid})' if txid else ''}: {failure}\n"
                             f"{out.stderr[-2000:]}")

    def _mined(self, duffs: int) -> int | None:
        tip = self.root.tip()
        return tip if sum(1 for c in self.mined_coins() if c >= duffs) >= 4 else None

    def pay(self, to: Root, wallet: str, duffs: int, coins: int = 1) -> tuple[str, int, int]:
        """Pays `duffs` to `wallet` as `coins` equal outputs and waits until `to` has scanned a
        block that holds them; returns the txid, the height `to` scanned and its total then."""
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
                return txid, height, int(balances["total"])
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


def _seconds(at: int | None) -> float | None:
    return None if at is None else (at / 1000 if at > 10**11 else at)


def wait_pass(session: Session, loops: tuple[str, ...] = ("dash_pay_sync",), timeout: float = 900) -> dict:
    """Waits until each of the engine's Platform sync loops has run once after now (`dashpay sync`
    is not implemented: `DashPay.sync_now`); returns the sync status."""
    since = time.time()

    def done() -> dict | None:
        status = session.ok("dashpay", "sync-status")
        ran = {loop["sync_loop"]: _seconds(loop["last_run_at"]) for loop in status["loops"]}
        return status if all((ran.get(loop) or 0) >= since for loop in loops) else None

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


def send_kind(error: dict) -> str | None:
    """The `SendFailure` kind of a send error: `params.kind` of an error whose code is
    `send.<kind>` (dwcli's per-variant code, `masternodes::send_code`); `None` for any other
    error. A code and kind that disagree fail."""
    code = error.get("code")
    if not isinstance(code, str) or not code.startswith("send."):
        return None
    kind = (error.get("params") or {}).get("kind")
    if kind != code.removeprefix("send."):
        raise AssertionError(f"a send error whose code and params.kind disagree: {error}")
    return kind


def definitely_unsent_no_peers(error: dict) -> bool:
    """A `pay-contact` error that sent nothing and may go through once a peer connects: the
    wallet still rescanning (refused before anything was prepared), or a broadcast that found no
    peer and that dwcli could abandon afterwards (`outcome: not_sent`)."""
    if error.get("code") == "rescan_in_progress":
        return True
    return send_kind(error) == "no_peers" and (error.get("params") or {}).get("outcome") == "not_sent"


def try_pay(session: Session, contact: str, duffs: int, *extra: str, timeout: float = 900) -> dict:
    """`pay-contact`'s answer, asked again only while it definitely sent nothing for want of a
    peer or a finished rescan (`definitely_unsent_no_peers`), and the contact is not locked."""

    def once() -> dict | None:
        answer = session.call("pay-contact", contact, "--amount", str(duffs), *extra, timeout=900)
        if answer.get("ok") or not definitely_unsent_no_peers(answer.get("error") or {}):
            return answer
        lock = session.ok("contact", "lock", contact)
        if lock:
            raise AssertionError(f"pay-contact sent nothing ({answer['error']}), yet the contact is locked to "
                                 f"{lock['txid']}")
        return None

    return wait_until(once, timeout, "a connected peer and no rescan to pay through", interval=10)


def pay_contact(session: Session, contact: str, duffs: int, *extra: str) -> dict:
    """A payment that went out. An unknown outcome (on a loaded host the lease's deadline can cut
    the broadcast's hand-off) is settled as the app does, through the contact's lock: `sent` is
    the payment, under the lock's txid (no `address`); only exactly `not_sent` pays again, and
    anything else fails with the txid."""
    answer = try_pay(session, contact, duffs, *extra)
    if answer.get("ok"):
        return answer["result"]
    error = answer["error"]
    if send_kind(error) != "broadcast_unknown":
        raise DwcliError(["pay-contact", contact], error, "")
    prepared = error["params"].get("prepared_txid")
    lock = session.ok("contact", "lock", contact)
    if not lock:
        raise AssertionError(f"pay-contact {prepared}: outcome unknown, yet the contact has no lock")
    txid = lock["txid"]
    if prepared and prepared != txid:
        raise AssertionError(f"pay-contact prepared {prepared}, but the contact is locked to {txid}")
    print(f"pay-contact {duffs}: {error['message']}; resolving the lock on {txid}")

    def resolved() -> str | None:
        try:
            outcome = session.ok("contact", "resolve-lock", contact)
        except DwcliError as e:
            raise AssertionError(f"resolving the lock on {txid}: {e}") from None
        return None if outcome == "unknown" else outcome

    outcome = wait_until(resolved, 900, f"the lock on {txid} to resolve", interval=15)
    if outcome == "sent":
        return {"paid": True, "txid": txid, "resolved": outcome, "address": None}
    if outcome != "not_sent":
        raise AssertionError(f"the lock on {txid} resolved {outcome!r}; not paying again")
    answer = try_pay(session, contact, duffs, *extra)
    if not answer.get("ok"):
        raise DwcliError(["pay-contact", contact], answer["error"], f"(after {txid} resolved not_sent)")
    return answer["result"]


def synced_until(session: Session, check: Callable[[], Any], timeout: float, what: str) -> Any:
    """Polls `check` while the session's DashPay sync loops run."""
    return wait_until(check, timeout, what, interval=15)


def befriend(a: Member, b: Member, timeout: float = 1200, live: float = 300) -> dict:
    """A asks B, B accepts: the round trip, with what each side saw on the way. A's session stays
    open while B accepts, as an app left unlocked; `a_live` is A's row in that session once its
    channel is ready or `live` seconds after it lists B as a contact, read after A's DashPay sync
    loop has run again since (`a_live_pass`: the engine reports no per-pass outcome, `last_pass`
    stays empty while `sync_now` is unimplemented); `a_contact` A's row once ready in a new
    process."""
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
        listed = time.time()
        deadline = time.monotonic() + live
        while row and row[1]["channel"] != "ready" and time.monotonic() < deadline:
            time.sleep(15)
            row = find_contact(sa, b.identity)
        status = wait_pass(sa)
        ran = {loop["sync_loop"]: _seconds(loop["last_run_at"]) for loop in status["loops"]}
        seen["a_live_pass"] = {"listed_at": listed, "ran_at": ran.get("dash_pay_sync"),
                               "startup": status["startup"]}
        seen["a_live"] = find_contact(sa, b.identity)
    with a.session("befriend-a2") as sa:
        seen["a_contact"] = synced_until(sa, contact_of(sa, b, True), timeout, "A's contact B to be ready")
    return seen


def txid_of(payload: bytes) -> str:
    """A non-witness transaction's txid: its double SHA-256, byte-reversed (Dash has no witness)."""
    return hashlib.sha256(hashlib.sha256(payload).digest()).digest()[::-1].hex()


class HoldingPeer:
    """A loopback P2P relay to one real peer that holds back every `tx` message the client sends
    until `release()`: the client's broadcast reaches no node, so its outcome is unknown; released,
    the same bytes reach the network."""

    def __init__(self, upstream: str):
        self.upstream = upstream
        self.server = socket.socket()
        self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server.bind(("127.0.0.1", 0))
        self.server.listen(4)
        self.port = self.server.getsockname()[1]
        self.held: list[tuple[Any, bytes, str]] = []
        self.holding = True
        self.lock = threading.Lock()
        self.closed = False
        threading.Thread(target=self._accept, daemon=True).start()

    @property
    def address(self) -> str:
        return f"127.0.0.1:{self.port}"

    def _accept(self) -> None:
        while not self.closed:
            try:
                client, _ = self.server.accept()
            except OSError:
                return
            host, port = self.upstream.rsplit(":", 1)
            try:
                up = socket.create_connection((host, int(port)), timeout=20)
                up.settimeout(None)
            except OSError:
                client.close()
                continue
            threading.Thread(target=self._pipe, args=(client, up, True), daemon=True).start()
            threading.Thread(target=self._pipe, args=(up, client, False), daemon=True).start()

    def _pipe(self, src: Any, dst: Any, from_client: bool) -> None:
        buf = b""
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                if not from_client:
                    dst.sendall(data)
                    continue
                buf += data
                while len(buf) >= 24:
                    length = int.from_bytes(buf[16:20], "little")
                    if len(buf) < 24 + length:
                        break
                    message, buf = buf[: 24 + length], buf[24 + length:]
                    command = message[4:16].rstrip(b"\0")
                    with self.lock:
                        if command == b"tx" and self.holding:
                            self.held.append((dst, message, txid_of(message[24:])))
                            continue
                    dst.sendall(message)
        except OSError:
            pass
        finally:
            for s in (src, dst):
                try:
                    s.close()
                except OSError:
                    pass

    def held_txids(self) -> list[str]:
        with self.lock:
            return [txid for _, _, txid in self.held]

    def release(self) -> dict[str, bool]:
        """Sends the held transactions on over the connections they came on; returns, for each
        held txid, whether one of its copies was written to the upstream peer."""
        with self.lock:
            self.holding = False
            held, self.held = self.held, []
        released: dict[str, bool] = {}
        for dst, message, txid in held:
            try:
                dst.sendall(message)
                released[txid] = True
            except OSError:
                released.setdefault(txid, False)
        return released

    def close(self) -> None:
        self.closed = True
        try:
            self.server.close()
        except OSError:
            pass
