"""The DashPay harness itself, with no network and no real dwcli: secrets kept out of every
artifact, the configuration rules, run cleanup, the funder's and a contact payment's retry rules,
and the relay's txid check."""

from __future__ import annotations

import hashlib
import os
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
    print("words: " + "\n".join(f"{{i}}. {{w}}" for i, w in enumerate(phrase.split(), 1)), file=sys.stderr)
    print("error: could not import " + phrase, file=sys.stderr)
    sys.exit(3)
if "create" in args:
    print("wallet_id " + "ab" * 32)
    print("mnemonic {created}")
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


def run_cases(tmp_path: Path, env: dict[str, str], select: str,
              junit: str | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-m", "pytest", "-l", "--tb=long", "-p", "no:cacheprovider", "-o", "addopts=",
         f"--junitxml={junit or tmp_path / 'junit.xml'}", "-k", select, "tests/dp_secret_cases.py"],
        cwd=HARNESS, env=env, capture_output=True, text=True, timeout=300)


def leaks(text: str) -> list[str]:
    found = []
    for name, phrase in (("funder", FUNDER_PHRASE), ("created", CREATED_PHRASE)):
        words = phrase.split()
        if phrase in text or " ".join(words[:4]) in text or Secret(phrase).found_in(text):
            found.append(name)
    return found


def test_secrets_stay_out_of_every_artifact(tmp_path, fake_dwcli):
    env = dp_env(tmp_path, fake_dwcli)
    proc = run_cases(tmp_path, env, "funder_import_fails or created_wallet_then_failure")
    assert proc.returncode == 1, proc.stdout[-4000:] + proc.stderr[-4000:]
    assert "dwcli import --birth-height 1 --name funder exited 3" in proc.stdout, proc.stdout[-4000:]
    assert "a failure after a create" in proc.stdout
    assert "1 failed, 2 deselected, 1 error" in proc.stdout
    passphrase = (tmp_path / "state" / "pass").read_text().strip()
    artifacts = {"pytest stdout": proc.stdout, "pytest stderr": proc.stderr,
                 "junit": (tmp_path / "junit.xml").read_text()}
    logs = sorted(p for p in (tmp_path / "logs").rglob("*") if p.is_file())
    assert any(p.name.endswith(".log") for p in logs), logs
    artifacts.update({str(p): p.read_text() for p in logs})
    for name, text in artifacts.items():
        assert not leaks(text), (name, leaks(text))
        assert passphrase not in text, name
    assert (tmp_path / "logs" / SCAN_OK).exists(), proc.stderr[-2000:]
    assert "DashPay secret scan: no secret" in proc.stderr


def test_the_scan_fails_on_a_secret_or_an_unreadable_file(tmp_path, fake_dwcli):
    planted = tmp_path / "out" / "planted.log"
    planted.parent.mkdir()
    planted.write_text("seed\n" + "\n".join(f"{i}. {w}" for i, w in enumerate(FUNDER_PHRASE.split()[3:9], 1)))
    proc = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=str(planted)), "harness_only")
    assert proc.returncode == 1, proc.stdout[-2000:] + proc.stderr[-2000:]
    assert f"DashPay secret scan FAILED: a secret in ['{planted}']" in proc.stderr
    assert not (tmp_path / "logs" / SCAN_OK).exists()
    assert not leaks(proc.stdout + proc.stderr)

    planted.write_text("nothing here\n")
    planted.chmod(0)
    missing = tmp_path / "out" / "missing.log"
    proc = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=f"{planted}:{missing}"), "harness_only")
    assert proc.returncode == 1, proc.stdout[-2000:] + proc.stderr[-2000:]
    assert "unreadable" in proc.stderr and f"{planted} (PermissionError)" in proc.stderr, proc.stderr
    assert f"{missing} (missing)" in proc.stderr
    assert not (tmp_path / "logs" / SCAN_OK).exists()

    planted.chmod(0o600)
    proc = run_cases(tmp_path, dp_env(tmp_path, fake_dwcli, DW_DP_SCAN=str(planted)), "harness_only")
    assert proc.returncode == 0, proc.stdout[-2000:] + proc.stderr[-2000:]
    assert (tmp_path / "logs" / SCAN_OK).exists()


def test_the_scan_runs_without_the_dp_fixture_and_reads_the_junit_pytest_writes(tmp_path, fake_dwcli):
    env = dp_env(tmp_path, fake_dwcli, HOME=str(tmp_path / "home"), DP_JUNIT_DIR="out")
    (tmp_path / "home" / "out").mkdir(parents=True)
    proc = run_cases(tmp_path, env, "without_the_harness", junit="~/$DP_JUNIT_DIR/junit.xml")
    assert proc.returncode == 0, proc.stdout[-2000:] + proc.stderr[-2000:]
    assert (tmp_path / "logs" / SCAN_OK).exists()
    assert f"no secret in {tmp_path / 'logs'}, {tmp_path / 'home' / 'out' / 'junit.xml'}" in proc.stderr


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
                 f"seed: {CREATED_PHRASE}"):
        assert not leaks(r.mask(text)), text
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

    state = tmp / "state"
    monkeypatch.setenv("DW_DP_STATE", str(state))
    harness = Harness.from_env()
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
    with pytest.raises(HarnessConfigError, match="not DW_DP_FUNDER's .* set DW_DP_STATE to a new directory") as e:
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


PREPARED = "estimate fee=1\nprepared txid=aa11 fee=1\n"


@pytest.mark.parametrize("stdout,expected", [
    ("failed outcome=not_sent kind=amount_exceeds_balance txid=-\n", None),
    (PREPARED + "failed outcome=not_sent kind=no_peers txid=aa11\n", None),
    (PREPARED + "unknown aa11 reason=x\nrebroadcast 1\nfailed outcome=unknown kind=broadcast_unknown txid=aa11 "
     "reason=not sent this time: transaction broadcast rejected: SPV broadcast not sent: no connected peers\n", "aa11"),
])
def test_funder_retries_only_what_was_definitely_not_sent(tmp_path, fake_dwcli, monkeypatch, stdout, expected):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, stdout)
    assert funder._try_send(["--to", "x:1"], 1) == expected


@pytest.mark.parametrize("stdout,message", [
    (PREPARED + "failed outcome=not_sent kind=broadcast_rejected txid=aa11 "
     "reason=SPV broadcast not sent: no connected peers\n", "prepared aa11"),
    (PREPARED + "failed outcome=not_sent kind=broadcast_rejected txid=aa11 reason=bad-txns-inputs-missingorspent\n",
     "prepared aa11"),
    (PREPARED, "prepared aa11"),
    ("estimate fee=1\n", "funder send failed: None"),
])
def test_funder_never_resends_an_unclassified_failure(tmp_path, fake_dwcli, monkeypatch, stdout, message):
    funder = funder_with(tmp_path, fake_dwcli, monkeypatch, stdout)
    with pytest.raises(AssertionError, match=message):
        funder._try_send(["--to", "x:1"], 1)


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
    credits = iter([0, 0, 500])
    monkeypatch.setattr(dashpay, "credited", lambda root, wallet, txid: next(credits))
    assert funder.pay(to, "w", 500) == ("t1", 103, 900)
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
