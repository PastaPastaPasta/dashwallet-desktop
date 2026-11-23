"""The DashPay harness itself, with no network and no real dwcli: secrets kept out of every
artifact, the configuration rules, run cleanup, the funder's and a contact payment's retry rules,
and the relay's txid check."""

from __future__ import annotations

import hashlib
import json
import os
import resource
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from dwd_regtest import dashpay
from dwd_regtest.dashpay import (
    REPO,
    RUN_MARKER,
    SCAN_OK,
    DwcliError,
    Funder,
    Harness,
    HarnessConfigError,
    HoldingPeer,
    Net,
    Redactor,
    Root,
    Secret,
    clean_runs,
    pay_contact,
    try_pay,
    txid_of,
)

HARNESS = Path(__file__).resolve().parents[1]
FUNDER_PHRASE = "zebra quokka walrus meadow pigeon candle sorrow ribbon thunder velvet glacier orbit"
CREATED_PHRASE = "lantern pepper mosaic falcon ripple saddle tunnel beacon harvest copper violin jungle"

FAKE_DWCLI = r'''#!{python}
import hashlib, os, sys
args = sys.argv[1:]
if "init-vault" in args:
    sys.exit(0)
if "list" in args:
    print(os.environ.get("FAKE_LIST", ""))
    sys.exit(0)
if "import" in args:
    phrase = sys.stdin.readline().strip()
    if os.environ.get("FAKE_IMPORT_OK"):
        print("wallet_id " + hashlib.sha256(phrase.encode()).hexdigest()[:16])
        sys.exit(0)
    print("echo " + phrase)
    print("hint: " + " ".join(phrase.split()[2:6]))
    print("fragment " + " ".join(phrase.split()[5:9]).upper(), file=sys.stderr)
    print("words: " + "\n".join(f"{{i}}. {{w}}" for i, w in enumerate(phrase.split(), 1)), file=sys.stderr)
    print("error: could not import " + phrase, file=sys.stderr)
    sys.exit(3)
if "create" in args:
    print(os.environ.get("FAKE_CREATE", "wallet_id " + "ab" * 32 + "\nmnemonic {created}"))
    print("seed: {created}", file=sys.stderr)
    sys.exit(0)
if "send" in args:
    with open(os.environ["FAKE_SEND_OUT"]) as f:
        sys.stdout.write(f.read())
    print("error: canned failure: no connected peers", file=sys.stderr)
    sys.exit(int(os.environ.get("FAKE_SEND_RC", "1")))
sys.exit(9)
'''


@pytest.fixture
def fake_dwcli(tmp_path: Path) -> Path:
    path = tmp_path / "bin" / "dwcli"
    path.parent.mkdir()
    path.write_text(FAKE_DWCLI.format(python=sys.executable, created=CREATED_PHRASE))
    path.chmod(0o700)
    return path


@pytest.fixture
def keep_umask():
    old = os.umask(0o022)
    os.umask(old)
    yield
    os.umask(old)


def private(path: Path, text: str | None = None, mode: int = 0o600) -> Path:
    if text is None:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.chmod(0o700)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(mode)
    return path


def dp_env(tmp_path: Path, dwcli: Path, **extra: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if not k.startswith("DW_DP_")}
    env.update(DW_DP_NET="testnet", DW_DP_STATE=str(tmp_path / "state"), DWCLI=str(dwcli),
               DW_DP_FUNDER=str(private(tmp_path / "secret" / "funder.phrase", FUNDER_PHRASE + "\n")),
               DW_DP_FUNDER_BIRTH="1", DW_DP_LOGS=str(tmp_path / "logs"))
    env.update(extra)
    return env


class Run:
    """An inner pytest run: its exit status, and its output only through checks and a masked
    tail, so no local that pytest prints holds it."""

    __slots__ = ("returncode", "_stdout", "_stderr")

    def __init__(self, proc: subprocess.CompletedProcess):
        self.returncode, self._stdout, self._stderr = proc.returncode, proc.stdout, proc.stderr

    def __repr__(self) -> str:
        return f"Run(returncode={self.returncode})"

    def out(self, text: str) -> bool:
        return text in self._stdout

    def err(self, text: str) -> bool:
        return text in self._stderr

    def leaks(self) -> dict[str, list[str]]:
        return {name: found for name, text in (("stdout", self._stdout), ("stderr", self._stderr))
                if (found := leaks(text))}

    def tail(self, secrets: tuple[str, ...] = ()) -> str:
        r = Redactor()
        for value in (FUNDER_PHRASE, CREATED_PHRASE, *secrets):
            r.add(Secret(value))
        return r.mask(self._stdout[-3000:] + self._stderr[-3000:])


def run_cases(tmp_path: Path, env: dict[str, str], select: str, junit: str | None = None) -> Run:
    return Run(subprocess.run(
        [sys.executable, "-m", "pytest", "-l", "--tb=long", "-p", "no:cacheprovider", "-o", "addopts=",
         "-o", "junit_logging=all", f"--junitxml={junit or tmp_path / 'junit.xml'}", "-k", select,
         "tests/dp_secret_cases.py"],
        cwd=HARNESS, env=env, capture_output=True, text=True, timeout=300))


def leaks(text: str) -> list[str]:
    found = []
    for name, phrase in (("funder", FUNDER_PHRASE), ("created", CREATED_PHRASE)):
        words = phrase.split()
        if phrase in text or " ".join(words[:4]) in text or Secret(phrase).found_in(text):
            found.append(name)
    return found


def artifact_verdicts(tmp_path: Path, run: Run) -> dict[str, list[str]]:
    """Each artifact holding a secret (the fixed phrases, the run's vault passphrase), and what;
    reads everything here, so no caller's local holds an artifact or the passphrase."""
    passphrase = (tmp_path / "state" / "pass").read_text().strip()
    verdicts = {name: found for name, found in run.leaks().items()}
    files = [tmp_path / "junit.xml", *sorted(p for p in (tmp_path / "logs").rglob("*") if p.is_file())]
    for path in files:
        text = path.read_text()
        found = leaks(text) + (["passphrase"] if passphrase in text else [])
        if found:
            verdicts[str(path)] = found
    return verdicts


def test_secrets_stay_out_of_every_artifact(tmp_path, fake_dwcli):
    env = dp_env(tmp_path, fake_dwcli)
    run = run_cases(tmp_path, env, "funder_import_fails or created_wallet_then_failure")
    assert run.returncode == 1, run.tail()
    assert run.out("dwcli import --birth-height 1 --name funder exited 3"), run.tail()
    assert run.out("a failure after a create")
    assert run.out("1 failed, 2 deselected, 1 error"), run.tail()
    assert "<system-out>" in (tmp_path / "junit.xml").read_text(), "junit_logging captured nothing"
    assert any(p.name.endswith(".log") for p in (tmp_path / "logs").rglob("*"))
    assert artifact_verdicts(tmp_path, run) == {}
    assert (tmp_path / "logs" / SCAN_OK).exists(), run.tail()
    assert run.err("DashPay secret scan: no secret")


@pytest.mark.parametrize("create", [
    f"wallet_id {'ab' * 32}\nmnemonic {' '.join(CREATED_PHRASE.split()[:7])}",
    f"wallet_id {'ab' * 32}\nmnemonic {CREATED_PHRASE}\nmnemonic {CREATED_PHRASE}",
    f"wallet_id {'ab' * 31}\nmnemonic {CREATED_PHRASE}",
    f"mnemonic {CREATED_PHRASE}",
    f"wallet_id {'ab' * 32}\nMnemonic: {CREATED_PHRASE}",
    f"wallet_id {'ab' * 32}\nmnemonic {CREATED_PHRASE}\nbackup {CREATED_PHRASE.upper().replace(' ', '-')}",
])
def test_a_malformed_create_withholds_both_streams(tmp_path, fake_dwcli, monkeypatch, create):
    monkeypatch.setenv("FAKE_CREATE", create)
    root = funder_with(tmp_path, fake_dwcli, monkeypatch, "").root
    with pytest.raises(AssertionError, match="both its streams were withheld"):
        root.create("m")
    assert not leaks((root.logs / "funder.log").read_text())


def test_a_well_formed_create_keeps_only_its_public_fields(tmp_path, fake_dwcli, monkeypatch):
    root = funder_with(tmp_path, fake_dwcli, monkeypatch, "").root
    wallet = root.create("m")
    assert wallet.id == "ab" * 32 and wallet.mnemonic.reveal() == CREATED_PHRASE
    log = (root.logs / "funder.log").read_text()
    assert f"wallet_id {'ab' * 32}\nmnemonic <redacted>\n" in log and not leaks(log)


def test_session_answers_are_masked_after_decoding():
    r = Redactor()
    r.add(Secret(FUNDER_PHRASE))
    escaped = "".join(f"\\u{ord(c):04x}" for c in FUNDER_PHRASE)
    answer = dashpay.session_answer('{"id": 1, "error": {"message": "%s", "%s": 1}}\n' % (escaped, escaped), r)
    assert answer["id"] == 1 and not leaks(repr(answer)), answer
    assert dashpay.session_answer("not json " + FUNDER_PHRASE, r) is None
    assert dashpay.session_answer('{"id": 1, "x": "' + escaped, r) is None


def test_stderr_is_masked_across_lines():
    r = Redactor()
    r.add(Secret(FUNDER_PHRASE))
    written: list[str] = []
    masker = dashpay.LineMasker(r, written.append, window=8)
    words = FUNDER_PHRASE.split()
    for i in range(0, 12, 3):
        masker.feed(f"log {i}: {' '.join(words[i:i + 3])}\n")
    for i in range(20):
        masker.feed(f"line {i}\n")
    masker.flush()
    assert not leaks("".join(written)), written
    assert written[-1] == "line 19\n"


def test_the_scan_fails_on_a_secret_or_an_unreadable_file(tmp_path, fake_dwcli):
    planted = tmp_path / "out" / "planted.log"
    planted.parent.mkdir()
    planted.write_text("seed\n" + "\n".join(f"{i}. {w}" for i, w in enumerate(FUNDER_PHRASE.split()[3:9], 1)))
    run = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=str(planted)), "harness_only")
    assert run.returncode == 1, run.tail()
    assert run.err(f"DashPay secret scan FAILED: a secret in ['{planted}']")
    assert not (tmp_path / "logs" / SCAN_OK).exists()
    assert run.leaks() == {}

    planted.write_text("nothing here\n")
    planted.chmod(0)
    missing = tmp_path / "out" / "missing.log"
    run = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=f"{planted}:{missing}"), "harness_only")
    assert run.returncode == 1, run.tail()
    assert run.err("unreadable") and run.err(f"{planted} (PermissionError)"), run.tail()
    assert run.err(f"{missing} (missing)")
    assert not (tmp_path / "logs" / SCAN_OK).exists()

    planted.chmod(0o600)
    run = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=str(planted)), "harness_only")
    assert run.returncode == 0, run.tail()
    assert (tmp_path / "logs" / SCAN_OK).exists()


def test_the_scan_runs_without_the_dp_fixture_and_reads_the_junit_pytest_writes(tmp_path, fake_dwcli):
    env = dp_env(tmp_path, fake_dwcli, HOME=str(tmp_path / "home"), DP_JUNIT_DIR="out")
    (tmp_path / "home" / "out").mkdir(parents=True)
    run = run_cases(tmp_path, env, "without_the_harness", junit="~/$DP_JUNIT_DIR/junit.xml")
    assert run.returncode == 0, run.tail()
    assert (tmp_path / "logs" / SCAN_OK).exists()
    assert run.err(f"no secret in {tmp_path / 'logs'}, {tmp_path / 'home' / 'out' / 'junit.xml'}")


def test_an_undecodable_secret_file_fails_the_run_after_a_scan(tmp_path, fake_dwcli):
    env = dp_env(tmp_path, fake_dwcli)
    funder = Path(env["DW_DP_FUNDER"])
    funder.write_bytes(b"\xff\xfe " + FUNDER_PHRASE.encode() + b"\n")
    run = run_cases(tmp_path, env, "harness_only")
    assert run.returncode == 1, run.tail()
    assert run.out("its first line could not be read as UTF-8 text"), run.tail()
    assert run.err(f"DashPay secret scan: no secret in {tmp_path / 'junit.xml'}"), run.tail()
    assert artifact_verdicts(tmp_path, run) == {}
    assert not run.out("UnicodeDecodeError") and not run.out("\\xff")


def test_a_secret_file_is_read_by_its_first_line_on_the_checked_descriptor(tmp_path):
    path = private(tmp_path / "phrase", "")
    path.write_bytes(FUNDER_PHRASE.encode() + b"\n\xff\xfe trailing\n")
    assert dashpay.read_secret(path, "a phrase").reveal() == FUNDER_PHRASE
    path.write_bytes(b"\xffbad\n")
    with pytest.raises(HarnessConfigError, match="not be read as UTF-8") as e:
        dashpay.read_secret(path, "a phrase")
    assert e.value.__context__ is None and e.value.__cause__ is None
    link = tmp_path / "link"
    link.symlink_to(path)
    with pytest.raises(HarnessConfigError, match="a phrase .*link"):
        dashpay.read_secret(link, "a phrase")
    path.chmod(0o640)
    with pytest.raises(HarnessConfigError, match="open to group or others"):
        dashpay.read_secret(link, "a phrase", follow=True)


def test_the_scan_never_follows_a_link_or_waits_on_a_fifo(tmp_path):
    logs = private(tmp_path / "logs")
    (logs / "plain.log").write_text("fine\n")
    os.mkfifo(logs / "fifo")
    secret = private(tmp_path / "secret.txt", FUNDER_PHRASE)
    (logs / "link.log").symlink_to(secret)
    (logs / "linked-dir").symlink_to(tmp_path)
    r = Redactor()
    r.add(Secret(FUNDER_PHRASE))
    result: list = []
    worker = threading.Thread(target=lambda: result.append(r.scan([logs, logs / "fifo"])), daemon=True)
    worker.start()
    worker.join(30)
    assert result, "the scan blocked"
    leaked, unreadable = result[0]
    assert leaked == []
    assert sorted(unreadable) == sorted(f"{logs / n} ({why})" for n, why in (
        ("fifo", "not a regular file"), ("fifo", "not a regular file"), ("link.log", "OSError"),
        ("linked-dir", "OSError")))


def test_redactor_masks_a_phrase_in_any_layout():
    r = Redactor()
    r.add(Secret(FUNDER_PHRASE))
    numbered = "\n".join(f"{i}. {w}" for i, w in enumerate(FUNDER_PHRASE.split(), 1))
    for text in (FUNDER_PHRASE, numbered, str(FUNDER_PHRASE.split()), f"mnemonic {CREATED_PHRASE}",
                 f"seed: {CREATED_PHRASE}", FUNDER_PHRASE.upper(), FUNDER_PHRASE.replace(" ", " ;-; " * 20),
                 FUNDER_PHRASE.replace(" ", "\\n")):
        assert not leaks(r.mask(text)), text
    for text in (FUNDER_PHRASE.title(), FUNDER_PHRASE.replace(" ", "." * 40), FUNDER_PHRASE.replace(" ", "\\n"),
                 "".join(f"\\u{ord(c):04x}" for c in FUNDER_PHRASE)):
        assert Secret(FUNDER_PHRASE).found_in(text), text
    words = FUNDER_PHRASE.split()
    for fragment in (" ".join(words[3:7]), "\n".join(f"{i}. {w}" for i, w in enumerate(words[5:11], 6)),
                     f"{words[0]} x {' '.join(words[2:6])}"):
        masked = Secret(FUNDER_PHRASE).masked_in(fragment)
        assert not Secret(FUNDER_PHRASE).found_in(masked), (fragment, masked)
    assert repr(Secret(FUNDER_PHRASE)) == str(Secret(FUNDER_PHRASE)) == "Secret(<redacted>)"
    assert FUNDER_PHRASE not in repr(dashpay.Wallet("w", "n", Secret(FUNDER_PHRASE)))


@pytest.mark.parametrize("net,args,error", [
    ("mainnet", "", "not one of"),
    ("testnett", "", "not one of"),
    ("testnet", "--network mainnet", "mainnet"),
    ("testnet", "--network testnet", "may not set --network"),
    ("testnet", "--network=regtest", "may not set --network"),
    ("testnet", "--peer=mainnet-seed.example:9999", "mainnet"),
    ("testnet", "--datadir /x", "not one of"),
    ("testnet", "--peer", "needs a value"),
    ("testnet", "--peer=", "needs a value"),
    ("testnet", "--peer=-x", "needs a value"),
    ("testnet", "--peer -x", "needs a value"),
    ("testnet", "--dapi=--network", "needs a value"),
    ("devnet", "--peer 127.0.0.1:20001", "needs DW_DP_NET_ARGS with the devnet's --dapi"),
])
def test_network_configuration_is_refused(monkeypatch, net, args, error):
    monkeypatch.setenv("DW_DP_NET", net)
    monkeypatch.setenv("DW_DP_NET_ARGS", args)
    with pytest.raises(HarnessConfigError, match=error):
        Net.from_env()


def test_network_configuration(monkeypatch):
    monkeypatch.delenv("DW_DP_NET", raising=False)
    assert Net.from_env() is None
    monkeypatch.setenv("DW_DP_NET", "")
    assert Net.from_env() is None
    monkeypatch.setenv("DW_DP_NET", "testnet")
    monkeypatch.setenv("DW_DP_NET_ARGS", "--peer 1.2.3.4:19999")
    assert Net.from_env() == Net("T3", ["--network", "testnet", "--peer", "1.2.3.4:19999"])
    monkeypatch.setenv("DW_DP_NET", "devnet")
    monkeypatch.setenv("DW_DP_NET_ARGS", "--dapi https://127.0.0.1:2443 --ca-cert=/x/ca.pem --peer 127.0.0.1:20001")
    net = Net.from_env()
    assert net.tier == "T2"
    assert net.args == ["--network", "regtest", "--dapi", "https://127.0.0.1:2443", "--ca-cert", "/x/ca.pem",
                        "--peer", "127.0.0.1:20001"]
    assert net.without_peers() == net.args[:6]


@pytest.fixture
def state_env(tmp_path, monkeypatch, keep_umask):
    for key in [k for k in os.environ if k.startswith("DW_DP_")]:
        monkeypatch.delenv(key)
    monkeypatch.setenv("DW_DP_NET", "testnet")
    monkeypatch.setenv("DW_DP_STATE", str(tmp_path / "state"))
    return tmp_path


def test_state_directory_rules(state_env, monkeypatch):
    tmp = state_env
    monkeypatch.setenv("DW_DP_STATE", "relative/state")
    with pytest.raises(HarnessConfigError, match="absolute"):
        Harness.from_env()
    inside = REPO / "regtest" / "dw-dp-state-test"
    monkeypatch.setenv("DW_DP_STATE", str(inside))
    with pytest.raises(HarnessConfigError, match="inside the checkout"):
        Harness.from_env()
    assert not inside.exists()
    loose = tmp / "loose"
    loose.mkdir(mode=0o750)
    loose.chmod(0o750)
    monkeypatch.setenv("DW_DP_STATE", str(loose))
    with pytest.raises(HarnessConfigError, match="open to group or others"):
        Harness.from_env()

    real = private(tmp / "real-state")
    (tmp / "state-link").symlink_to(real)
    monkeypatch.setenv("DW_DP_STATE", str(tmp / "state-link"))
    with pytest.raises(HarnessConfigError, match="DW_DP_STATE .* is a symlink"):
        Harness.from_env()

    state = tmp / "state"
    monkeypatch.setenv("DW_DP_STATE", str(state))
    harness = Harness.from_env()
    assert resource.getrlimit(resource.RLIMIT_CORE) == (0, 0)
    assert stat.S_IMODE(state.stat().st_mode) == 0o700
    assert stat.S_IMODE((state / "pass").stat().st_mode) == 0o600
    assert (harness.run_dir / RUN_MARKER).read_text().strip() == harness.run_id
    assert stat.S_IMODE(harness.logs.stat().st_mode) == 0o700

    (state / "pass").chmod(0o640)
    with pytest.raises(HarnessConfigError, match="passphrase file .* open to group or others"):
        Harness.from_env()
    (state / "pass").chmod(0o600)

    funder = private(tmp / "secret" / "funder", FUNDER_PHRASE, mode=0o644)
    monkeypatch.setenv("DW_DP_FUNDER", str(funder))
    with pytest.raises(HarnessConfigError, match="DW_DP_FUNDER .* open to group or others"):
        Harness.from_env()
    funder.chmod(0o600)
    link = tmp / "secret" / "funder-link"
    link.symlink_to(funder)
    monkeypatch.setenv("DW_DP_FUNDER", str(link))
    Harness.from_env()

    elsewhere = private(tmp / "elsewhere")
    for checkout in (tmp / "co", tmp / "co" / "inner" / "state"):
        (tmp / "co" / ".git").mkdir(parents=True, exist_ok=True)
        monkeypatch.setenv("DW_DP_STATE", str(private(checkout)))
        with pytest.raises(HarnessConfigError, match="git checkout or inside one"):
            Harness.from_env()
    monkeypatch.setenv("DW_DP_STATE", str(state))

    (state / "funder").symlink_to(elsewhere)
    with pytest.raises(HarnessConfigError, match="funder data root .* is a symlink"):
        Harness.from_env().root("funder")
    (state / "funder").unlink()

    (state / "runs").rename(tmp / "old-runs")
    (state / "runs").symlink_to(elsewhere)
    with pytest.raises(HarnessConfigError, match="runs directory .* is a symlink"):
        Harness.from_env()


def test_cleanup_deletes_only_old_marked_runs(tmp_path, capsys):
    runs = private(tmp_path / "runs")
    outside = private(tmp_path / "outside")
    (outside / "keep.txt").write_text("keep")
    (outside / RUN_MARKER).write_text("link\n")
    old = time.time() - 4 * 86400

    def run(name: str, marker: str | None, age: float) -> Path:
        d = private(runs / name)
        if marker is not None:
            (d / RUN_MARKER).write_text(marker + "\n")
            os.utime(d / RUN_MARKER, (age, age))
        os.utime(d, (age, age))
        return d

    doomed = run("old-marked", "old-marked", old)
    (doomed / "escape").symlink_to(outside)
    run("old-unmarked", None, old)
    run("new-marked", "new-marked", time.time())
    run("wrong-marker", "someone-else", old)
    (runs / "link").symlink_to(outside)
    os.utime(outside / RUN_MARKER, (old, old))

    assert clean_runs(runs) == ["old-marked"]
    assert sorted(p.name for p in runs.iterdir()) == ["link", "new-marked", "old-unmarked", "wrong-marker"]
    assert (outside / "keep.txt").read_text() == "keep"
    assert capsys.readouterr().err == ""


def wallet_id(phrase: str) -> str:
    return hashlib.sha256(phrase.encode()).hexdigest()[:16]


def test_the_funder_wallet_must_be_the_configured_phrase(state_env, fake_dwcli, monkeypatch, capsys):
    tmp = state_env
    monkeypatch.setenv("DWCLI", str(fake_dwcli))
    monkeypatch.setenv("DW_DP_FUNDER", str(private(tmp / "secret" / "funder", FUNDER_PHRASE + "\n")))
    monkeypatch.setenv("DW_DP_FUNDER_BIRTH", "1")
    monkeypatch.setenv("FAKE_IMPORT_OK", "1")
    monkeypatch.setenv("FAKE_LIST", f'{wallet_id(FUNDER_PHRASE)} name="funder"')
    assert Harness.from_env().funder().wallet == wallet_id(FUNDER_PHRASE)
    monkeypatch.setenv("FAKE_LIST", f'{wallet_id(CREATED_PHRASE)} name="funder"')
    with pytest.raises(HarnessConfigError, match="not DW_DP_FUNDER's; set DW_DP_STATE to a new directory") as e:
        Harness.from_env().funder()
    assert not leaks(str(e.value) + capsys.readouterr().out)
    monkeypatch.setenv("FAKE_LIST", "")
    assert Harness.from_env().funder().wallet == wallet_id(FUNDER_PHRASE)


def funder_with(tmp_path: Path, fake: Path, monkeypatch, stdout: str, rc: int = 1) -> Funder:
    out = tmp_path / "send.out"
    out.write_text(stdout)
    monkeypatch.setenv("DWCLI", str(fake))
    monkeypatch.setenv("FAKE_SEND_OUT", str(out))
    monkeypatch.setenv("FAKE_SEND_RC", str(rc))
    logs = private(tmp_path / "logs")
    root = Root(private(tmp_path / "funder"), tmp_path / "pass", Net("T3", ["--network", "testnet"]), Redactor(), logs)
    return Funder(root, "w")


TX = "aa" * 32
OTHER_TX = "bb" * 32
PREPARED = f"estimate fee=1\nprepared txid={TX} fee=1\n"


@pytest.mark.parametrize("stdout,rc,expected", [
    ("failed outcome=not_sent kind=amount_exceeds_balance txid=-\n", 1, None),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={TX}\n", 1, None),
    (PREPARED + f"unknown {TX} reason=x\nrebroadcast 1\nfailed outcome=unknown kind=broadcast_unknown txid={TX} "
     "reason=not sent this time: transaction broadcast rejected: SPV broadcast not sent: no connected peers\n", 1, TX),
    (PREPARED + f"unknown {TX} reason=x\nfailed outcome=unknown kind=storage txid={TX}\n", 1, TX),
    (PREPARED + f"output yX 1 change=0 mine=0\nbroadcast {TX}\n", 0, TX),
])
def test_funder_retries_only_what_was_definitely_not_sent(tmp_path, fake_dwcli, monkeypatch, stdout, rc, expected):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, stdout, rc)
    assert funder._try_send(["--to", "x:1"], 1) == expected


@pytest.mark.parametrize("stdout,rc", [
    (PREPARED + f"failed outcome=not_sent kind=broadcast_rejected txid={TX} "
     "reason=SPV broadcast not sent: no connected peers\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=broadcast_rejected txid={TX} "
     "reason=bad-txns-inputs-missingorspent\n", 1),
    (PREPARED, 1),
    ("estimate fee=1\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={TX[:20]}\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={OTHER_TX}\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={TX}\nheld reserved=1 unlisted=0\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={TX}\nfailed outcome=unknown kind=x txid={TX}\n", 1),
    (PREPARED + "failed outcome=unknown kind=broadcast_unknown txid=-\n", 1),
    (PREPARED + f"broadcast {OTHER_TX}\n", 0),
    (PREPARED + f"broadcast {TX}\n", 1),
    (PREPARED + f"failed outcome=not_sent kind=no_peers txid={TX}\n", 0),
])
def test_funder_never_resends_an_unclassified_failure(tmp_path, fake_dwcli, monkeypatch, stdout, rc):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, stdout, rc)
    with pytest.raises(AssertionError, match="not sending again"):
        funder._try_send(["--to", "x:1"], 1)


def test_funder_waits_for_its_own_transaction_s_outputs(tmp_path, fake_dwcli, monkeypatch):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, "")
    utxos = (f"utxo {OTHER_TX}:0 900000000 conf=5 locked=0 reserved=0 change=0 address=yA\n"
             f"utxo {TX}:3 50000000 conf=0 locked=0 reserved=0 change=0 address=yA\n")
    monkeypatch.setattr(funder.root, "tip", lambda timeout=1800: 7)
    monkeypatch.setattr(funder.root, "run", lambda *args, **kw: dashpay.Output(0, utxos, ""))
    assert funder._mined(TX) is None
    utxos = utxos.replace("conf=0", "conf=1")
    assert funder._mined(TX) == 7


def test_funder_reads_the_tip_again_for_each_attempt(tmp_path, fake_dwcli, monkeypatch, no_sleep):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, "")
    tips = iter(range(100, 200))
    heights: list[int] = []

    def try_send(to: list[str], height: int) -> str | None:
        heights.append(height)
        return "t1" if len(heights) == 3 else None

    monkeypatch.setattr(funder.root, "tip", lambda timeout=1800: next(tips))
    monkeypatch.setattr(funder.root, "address", lambda wallet: "yF")
    monkeypatch.setattr(funder, "mined_coins", lambda: [])
    monkeypatch.setattr(funder, "_try_send", try_send)
    monkeypatch.setattr(funder, "_mined", lambda duffs: 150)
    assert funder.ensure_coins(1) == 150
    assert heights == [101, 102, 103]


def test_funder_pay_waits_for_its_own_txid(tmp_path, fake_dwcli, monkeypatch, no_sleep):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, "")
    to = funder.root
    monkeypatch.setattr(funder, "ensure_coins", lambda duffs: 100)
    monkeypatch.setattr(funder, "_try_send", lambda to, height: "t1")
    monkeypatch.setattr(to, "tip", lambda timeout=1800: 100)
    monkeypatch.setattr(to, "address", lambda wallet: "yT")
    monkeypatch.setattr(to, "sync_wallet", lambda wallet, height, timeout=900: {"total": "900"})
    ran: list[tuple] = []
    monkeypatch.setattr(to, "run", lambda *args, **kw: ran.append(args) or dashpay.Output(0, "", ""))
    credits = iter([0, 0, 500])
    monkeypatch.setattr(dashpay, "credited", lambda root, wallet, txid: next(credits))
    assert funder.pay(to, "w", 500) == ("t1", 103, 900)
    assert ran == [("sync", "--txid", "t1", "--confirmations", "1", "--timeout-secs", "1800")]
    monkeypatch.setattr(dashpay, "credited", lambda root, wallet, txid: 0)
    with pytest.raises(AssertionError, match="saw no funding t1"):
        funder.pay(to, "w", 500)


class FakeSession:
    """Answers `pay-contact`, `contact lock` and `contact resolve-lock` from scripts."""

    def __init__(self, pays: list[dict], lock: dict | list | None = None, resolutions: list = ()):
        self.pays, self.lock, self.resolutions = list(pays), lock, list(resolutions)
        self.paid = 0

    def call(self, *args: str, **kw) -> dict:
        if args[0] == "pay-contact":
            self.paid += 1
            return self.pays.pop(0)
        if args[:2] == ("contact", "lock"):
            return {"ok": True, "result": self.lock.pop(0) if isinstance(self.lock, list) else self.lock}
        if args[:2] == ("contact", "resolve-lock"):
            answer = self.resolutions.pop(0)
            return answer if isinstance(answer, dict) else {"ok": True, "result": answer}
        raise AssertionError(args)

    def ok(self, *args: str, **kw):
        answer = self.call(*args, **kw)
        if not answer.get("ok"):
            raise DwcliError(list(args), answer["error"], "")
        return answer["result"]


def send_error(kind: str, outcome: str | None = None, reason: str | None = None, **params) -> dict:
    p = {"kind": kind, **params}
    if outcome:
        p["outcome"] = outcome
    if reason:
        p["reason"] = reason
    return {"ok": False, "error": {"code": f"send.{kind}", "message": f"send: {kind}", "params": p}}


PAID = {"ok": True, "result": {"paid": True, "txid": "t2", "address": "yA"}}
UNKNOWN = send_error("broadcast_unknown", "unknown", "x", prepared_txid="t1")


@pytest.fixture
def no_sleep(monkeypatch):
    monkeypatch.setattr(dashpay.time, "sleep", lambda _: None)


def test_send_kind_reads_only_send_errors():
    assert dashpay.send_kind(send_error("no_peers", "not_sent")["error"]) == "no_peers"
    assert dashpay.send_kind(send_error("broadcast_unknown", "unknown", "x")["error"]) == "broadcast_unknown"
    for error in ({"code": "send", "params": {"kind": "no_peers"}},
                  {"code": "contact.payment_locked", "params": {"kind": "no_peers", "txid": "t0"}},
                  {"code": "coinjoin.send.no_peers", "params": {"kind": "no_peers"}},
                  {"code": None}, {}):
        assert dashpay.send_kind(error) is None, error
    with pytest.raises(AssertionError, match="disagree"):
        dashpay.send_kind({"code": "send.no_peers", "params": {"kind": "broadcast_unknown"}})


def test_only_a_definite_no_peer_failure_with_no_lock_allows_the_next_payment():
    assert dashpay.unsent_for_no_peers(send_error("no_peers", "not_sent")["error"], None)
    for error, lock in ((send_error("no_peers", "not_sent")["error"], {"txid": "t1"}),
                        (send_error("no_peers", "unknown")["error"], None),
                        (send_error("no_peers")["error"], None),
                        (send_error("broadcast_unknown", "unknown", "x")["error"], None),
                        (send_error("broadcast_rejected", "not_sent", "no connected peers")["error"], None),
                        ({"code": "rescan_in_progress", "params": {}}, None)):
        assert not dashpay.unsent_for_no_peers(error, lock), (error, lock)


def test_try_pay_retries_only_a_definitely_unsent_no_peer_failure(no_sleep):
    rescanning = {"ok": False, "error": {"code": "rescan_in_progress", "message": "rescan", "params": {}}}
    s = FakeSession([send_error("no_peers", "not_sent"), rescanning, PAID])
    assert try_pay(s, "c", 1) == PAID and s.paid == 3
    old_no_peers = "SPV broadcast not sent: no connected peers"
    for answer in (send_error("broadcast_rejected", "not_sent", "bad-txns"),
                   send_error("broadcast_rejected", "not_sent", old_no_peers),
                   send_error("no_peers", "unknown"),
                   send_error("no_peers"),
                   {"ok": False, "error": {"code": "send", "message": "send: no peers",
                                           "params": {"kind": "no_peers", "outcome": "not_sent"}}},
                   {"ok": False, "error": {"code": "contact.payment_locked", "message": "locked",
                                           "params": {"kind": "no_peers", "outcome": "not_sent", "txid": "t0"}}}):
        s = FakeSession([answer, PAID])
        assert try_pay(s, "c", 1) == answer and s.paid == 1
    s = FakeSession([send_error("no_peers", "not_sent")], lock={"txid": "t0"})
    with pytest.raises(AssertionError, match="locked to t0"):
        try_pay(s, "c", 1)


def test_pay_contact_settles_an_unknown_outcome_through_the_lock(no_sleep):
    s = FakeSession([UNKNOWN, PAID], lock={"txid": "t1"}, resolutions=["unknown", "sent"])
    assert pay_contact(s, "c", 1)["txid"] == "t1" and s.paid == 1
    s = FakeSession([UNKNOWN, PAID], lock={"txid": "t1"}, resolutions=["not_sent"])
    assert pay_contact(s, "c", 1) == PAID["result"] and s.paid == 2
    gone = {"ok": False, "error": {"code": "invalid_argument", "message": "no payment lock for this contact"}}
    for resolutions, message in (([gone], "resolving the lock on t1"), (["maybe"], "t1 resolved 'maybe'")):
        s = FakeSession([UNKNOWN, PAID], lock={"txid": "t1"}, resolutions=resolutions)
        with pytest.raises(AssertionError, match=message):
            pay_contact(s, "c", 1)
        assert s.paid == 1
    unknown2 = send_error("broadcast_unknown", "unknown", "x", prepared_txid="t2")
    s = FakeSession([UNKNOWN, unknown2, PAID], lock=[{"txid": "t1"}, {"txid": "t2"}],
                    resolutions=["not_sent", "unknown", "sent"])
    assert pay_contact(s, "c", 1)["txid"] == "t2" and s.paid == 2 and not s.resolutions
    s = FakeSession([UNKNOWN, unknown2, PAID], lock=[{"txid": "t1"}, {"txid": "t2"}],
                    resolutions=["not_sent", "not_sent"])
    with pytest.raises(AssertionError, match="2 payments resolved not_sent; not paying again"):
        pay_contact(s, "c", 1)
    assert s.paid == 2 and not s.resolutions
    s = FakeSession([UNKNOWN, PAID], lock=None)
    with pytest.raises(AssertionError, match="t1: outcome unknown, yet the contact has no lock"):
        pay_contact(s, "c", 1)
    s = FakeSession([send_error("broadcast_rejected", "not_sent", "bad-txns"), PAID])
    with pytest.raises(DwcliError):
        pay_contact(s, "c", 1)
    assert s.paid == 1


def test_a_payment_settled_through_its_lock_is_checked_by_its_history_output(no_sleep, monkeypatch):
    s = FakeSession([UNKNOWN], lock={"txid": "t1"}, resolutions=["sent"])
    paid = pay_contact(s, "c", 1)
    assert (paid["txid"], paid["address"]) == ("t1", None)
    history = ("t1 0 RecvWithAddress 150000 Confirmed 1 yContact\n"
               "t0 0 RecvWithAddress 150000 Confirmed 9 yOther\n"
               "t1 1 SendToAddress -5 Confirmed 1 ySelf\n"
               "garbled t1 line\n")
    root = Root(Path("/nonexistent"), Path("/nonexistent"), Net("T3", []), Redactor(), Path("/nonexistent"))
    calls: list[tuple] = []
    monkeypatch.setattr(root, "run", lambda *args, **kw: calls.append(args) or dashpay.Output(0, history, ""))
    assert dashpay.received_at(root, paid["txid"]) == ["yContact"]
    assert calls == [("history", "--limit", "1000")]
    assert dashpay.received_at(root, "t9") == []


GENESIS_COINBASE = bytes.fromhex(
    "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d01"
    "04455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f6620"
    "7365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548"
    "271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b"
    "8d578a4c702b6bf11d5fac00000000")


def p2p(command: bytes, payload: bytes) -> bytes:
    return (b"\xce\xe2\xca\xff" + command.ljust(12, b"\0") + len(payload).to_bytes(4, "little")
            + b"\0\0\0\0" + payload)


def test_relay_holds_a_tx_by_its_txid_until_released():
    assert txid_of(GENESIS_COINBASE) == "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
    upstream = socket.socket()
    upstream.bind(("127.0.0.1", 0))
    upstream.listen(1)
    got = bytearray()

    def serve() -> None:
        conn, _ = upstream.accept()
        conn.settimeout(10)
        try:
            while chunk := conn.recv(65536):
                got.extend(chunk)
        except OSError:
            pass

    threading.Thread(target=serve, daemon=True).start()
    relay = HoldingPeer(f"127.0.0.1:{upstream.getsockname()[1]}")
    try:
        client = socket.create_connection(("127.0.0.1", relay.port), timeout=10)
        version, tx = p2p(b"version", b"v" * 10), p2p(b"tx", GENESIS_COINBASE)
        client.sendall(version + tx)
        txid = txid_of(GENESIS_COINBASE)
        deadline = time.monotonic() + 10
        while (bytes(got) != version or not relay.held_txids()) and time.monotonic() < deadline:
            time.sleep(0.05)
        assert bytes(got) == version
        assert relay.held_txids() == [txid]
        assert relay.release() == {txid: True}
        while bytes(got) != version + tx and time.monotonic() < deadline:
            time.sleep(0.05)
        assert bytes(got) == version + tx
        assert relay.release() == {}
        relay.close()
        client.settimeout(3)
        assert client.recv(1) == b""
        client.close()
    finally:
        relay.close()
        upstream.close()


def test_the_failure_notice_goes_out_before_any_run_left_a_directory(tmp_path):
    """The notifier's install-time test, and a first run that fails early, have no nightly directory."""
    notifier = tmp_path / "notify"
    notifier.write_text('#!/bin/sh\nprintf %s "$1" > "$(dirname "$0")/sent"\n')
    notifier.chmod(0o700)
    env = {**os.environ, "DW_DP_NOTIFY": str(notifier), "DW_DP_STATE": str(tmp_path / "state")}
    notify = ["bash", str(REPO / "regtest/scripts/dashpay-nightly-notify.sh"), "dw-no-such.service"]
    subprocess.run(notify, env=env, check=True, timeout=60)
    assert (tmp_path / "sent").read_text().endswith("artifacts: none yet")
    run = tmp_path / "state" / "nightly" / "20261120"
    run.mkdir(parents=True)
    subprocess.run(notify, env=env, check=True, timeout=60)
    assert (tmp_path / "sent").read_text().endswith(f"artifacts: {run}")


def test_a_green_run_tells_the_notifier_so(tmp_path):
    """The nightly unit's ExecStartPost= passes --green, and the notifier gets result=success."""
    notifier = tmp_path / "notify"
    notifier.write_text('#!/bin/sh\nprintf %s "$1" > "$(dirname "$0")/sent"\n')
    notifier.chmod(0o700)
    env = {**os.environ, "DW_DP_NOTIFY": str(notifier), "DW_DP_STATE": str(tmp_path / "state")}
    notify = ["bash", str(REPO / "regtest/scripts/dashpay-nightly-notify.sh"), "--green", "dw-x.service"]
    subprocess.run(notify, env=env, check=True, timeout=60)
    assert (tmp_path / "sent").read_text() == "dw-x.service: result=success"


def test_the_issue_notifier_opens_updates_and_closes_one_issue(tmp_path):
    """A red night opens the issue or comments on it, without the host's path; a green one closes it."""
    if shutil.which("jq") is None:
        pytest.skip("jq is not installed")
    for name in ("dashpay-nightly-issue.sh", "nightly-issue.sh"):
        source = REPO / ("ci/github" if name == "nightly-issue.sh" else "regtest/scripts") / name
        shutil.copy(source, tmp_path / name)
    fake = tmp_path / "bin"
    fake.mkdir()
    (fake / "gh").write_text(
        '#!/bin/sh\nprintf \'%s\\n\' "$GH_REPO $*" >> "$CALLS"\n'
        '[ "$1 $2" = "issue list" ] && cat "$OPEN"\nexit 0\n'
    )
    (fake / "gh").chmod(0o700)
    calls, opened = tmp_path / "calls", tmp_path / "open.json"
    env = {**os.environ, "PATH": f"{fake}:{os.environ['PATH']}", "CALLS": str(calls), "OPEN": str(opened)}
    env["DW_DP_ISSUE_REPO"] = "owner/name"
    title = "Nightly failure: DashPay suites on testnet"

    def notify(notice):
        calls.write_text("")
        subprocess.run(["bash", str(tmp_path / "dashpay-nightly-issue.sh"), notice], env=env, check=True, timeout=60)
        listed, *rest = calls.read_text().splitlines()
        assert listed == "owner/name issue list --state open --label nightly-failure --limit 100 --json number,title"
        return rest

    red = f"dw.service: result=exit-code exit=1; artifacts: {tmp_path}/state/nightly/20261120-021500"
    opened.write_text(json.dumps([{"number": 3, "title": "Nightly failure: GitHub Actions"}]))
    (create,) = notify(red)
    assert create.startswith(f"owner/name issue create --title {title} --label nightly-failure --body ")
    assert create.endswith("result=exit-code exit=1; artifacts: nightly/20261120-021500 on the nightly's host")
    assert str(tmp_path) not in create
    for odd in (f"{red}/7 x\ndw.service: result=success", "dw.service: result=success\n", "anything: result=success x"):
        (made,) = notify(odd)
        assert made.startswith("owner/name issue create ") and "7 x" not in made
    assert notify("dw.service: result=success") == []
    (never_ran,) = notify("dw.service: result=success exit=0; artifacts: none yet")
    assert never_ran.startswith("owner/name issue create ")
    opened.write_text(json.dumps([{"number": 3, "title": "Nightly failure: GitHub Actions"}, {"number": 7, "title": title}]))
    (comment,) = notify(red)
    assert comment.startswith("owner/name issue comment 7 --body ") and str(tmp_path) not in comment
    (close,) = notify("dw.service: result=success")
    assert close.startswith("owner/name issue close 7 --comment ") and close.endswith("dw.service: result=success")


@pytest.mark.parametrize("where", ["outside", "in the state root", "in the checkout"])
def test_the_nightlys_build_cannot_read_the_funder_or_the_state_root(tmp_path, where):
    """The fetch and the build run where the state root holds only the checkout and the phrase reads empty."""
    if shutil.which("bwrap") is None or subprocess.run(["bwrap", "--dev-bind", "/", "/", "true"], capture_output=True).returncode != 0:
        pytest.skip("bubblewrap cannot start a sandbox here")
    sandbox = ["bash", str(REPO / "regtest/scripts/dashpay-unfunded.sh")]
    state = tmp_path / "state"
    repo = state / "repo"
    repo.mkdir(parents=True)
    (state / "vault-passphrase").write_text("secret")
    funder = {"outside": tmp_path, "in the state root": state, "in the checkout": repo}[where] / "funder"
    funder.write_text("phrase")
    funder.chmod(0o600)
    probe = 'cat "$1"; ls "$2"; pwd; touch built'
    out = subprocess.run(
        [*sandbox, str(state), str(repo), str(funder), str(repo), "bash", "-c", probe, "_", str(funder), str(state)],
        capture_output=True, text=True, check=True, timeout=60,
    ).stdout.split()
    assert out == [*(["funder"] if where == "in the state root" else []), "repo", str(repo)]
    assert (repo / "built").exists()
    assert funder.read_text() == "phrase" and (state / "vault-passphrase").read_text() == "secret"

    def refused(*args: str) -> bool:
        return subprocess.run([*sandbox, *args, "true"], capture_output=True, timeout=60).returncode != 0

    not_in_state = tmp_path / "elsewhere"
    not_in_state.mkdir()
    assert refused(str(state), str(not_in_state), str(funder), str(not_in_state))
    assert refused(str(state), str(repo), "funder", str(repo))
    os.link(funder, tmp_path / "another-name")
    assert refused(str(state), str(repo), str(funder), str(repo))
