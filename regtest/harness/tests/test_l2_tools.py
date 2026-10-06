"""l2-tools: the M2 R1 engine tools against dashd over SPV (docs/contracts/m2-engine.md §2.1–2.5).

Covers:
* rescan from a height finds a payment that a wallet born above it missed (QT-117);
* abandoning a transaction dashd never accepted releases its inputs, and resend of an
  unconfirmed transaction reaches dashd again (QT-091, IOS-034);
* a watch-only wallet from an account xpub sees incoming funds and cannot sign or send (QT-114);
* the fee policy is SPV's honest answer and a payment at the minimum relay fee is accepted
  (QT-057/058); the coin-control summary follows dash-qt's estimate (QT-072);
* console commands answered from the engine with Core's shapes (QT-145);
* first-seen transactions arrive as one batched NewTransactions event (QT-031/032).

Needs a dwcli built from this tree (`DWCLI=/path/to/dwcli`; skipped when absent). Run it with
its own compose project:

    DWD_COMPOSE_PROJECT=dwd-r1 DWD_REGTEST_BUILD=0 DWCLI=... \\
        .venv/bin/python -m pytest -v tests/test_l2_tools.py

A single regtest node has no quorums, so dash-spv is never `caught_up` there: every
`NewTransactions` batch reports `catch_up=1`, which is what a host must treat as "initial sync,
no popups". With one peer and no InstantSend quorum dash-spv accepts a broadcast only once it
is mined.
"""

from __future__ import annotations

import re
import subprocess
import threading
import time
from decimal import Decimal
from pathlib import Path

import pytest

from dwd_regtest import wait_until

from test_l1_send import COIN, DWCLI, Dwcli, duffs, fields

pytestmark = pytest.mark.skipif(DWCLI is None, reason="no dwcli binary (set DWCLI)")


def new_cli(root: Path, node, name: str, encrypted: bool = True) -> Dwcli:
    passfile = root / f"{name}.pass"
    passfile.write_text(f"{name} passphrase\n")
    cli = Dwcli(root / name, f"{node.host}:{node.p2p_port}", passfile)
    if encrypted:
        cli.run("init-vault")
    return cli


def tip(node) -> int:
    return node.rpc.getblockcount()


def history(cli: Dwcli) -> list[list[str]]:
    return [line.split() for line in cli.run("history") if line.strip()]


@pytest.fixture(scope="module")
def root(tmp_path_factory) -> Path:
    return tmp_path_factory.mktemp("l2tools")


@pytest.fixture(scope="module")
def main(root, regtest_node, funded_miner) -> Dwcli:
    """A wallet with two confirmed coins (3 and 2 DASH)."""
    cli = new_cli(root, regtest_node, "main")
    cli.wallet = cli.line("wallet_id", cli.run("create")).split()[1]
    for amount in (Decimal(3), Decimal(2)):
        regtest_node.send_to_address(cli.address(), amount)
    regtest_node.mine(1)
    synced = cli.sync(tip(regtest_node))
    assert int(synced["total"]) == 5 * COIN, synced
    return cli


def test_rescan_from_height_finds_a_missed_payment(root, regtest_node, funded_miner):
    """QT-117: a wallet born above a payment's block does not see it; a rescan from below
    that block finds it."""
    phrase_cli = new_cli(root, regtest_node, "rescan-src")
    created = phrase_cli.run("create")
    mnemonic = phrase_cli.line("mnemonic", created).split(" ", 1)[1]
    phrase_cli.wallet = phrase_cli.line("wallet_id", created).split()[1]
    address = phrase_cli.address()
    txid = funded_miner.sendtoaddress(address, Decimal("1.25"))
    regtest_node.mine(1)
    paid_at = tip(regtest_node)
    regtest_node.mine(2)
    height = tip(regtest_node)

    # Same phrase, born two blocks after the payment.
    cli = new_cli(root, regtest_node, "rescan")
    proc = subprocess.run(
        [*cli.base, "import", "--birth-height", str(height - 1)],
        input=mnemonic,
        capture_output=True,
        text=True,
        timeout=120,
        check=True,
    )
    cli.wallet = proc.stdout.split()[1]
    cli.run("sync", "--min-height", str(height), "--timeout-secs", "240")
    assert txid not in [r[0] for r in history(cli)]

    out = cli.run("rescan", "--sync-height", str(height), "--from-height", str(paid_at - 1))
    assert cli.line("rescan", out) == f"rescan started from={paid_at - 1}", out
    assert "rescan done" in out, out
    rows = history(cli)
    assert [r[0] for r in rows] == [txid], rows
    assert int(rows[0][3]) == duffs(Decimal("1.25"))
    assert f"total={duffs(Decimal('1.25'))}" in cli.line(f"wallet {cli.wallet}", out)


def _send_in_background(cli: Dwcli, height: int, *args: str) -> tuple[subprocess.Popen, list[str]]:
    proc = subprocess.Popen(
        [*cli.base, "send", cli.wallet, "--sync-height", str(height), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    lines: list[str] = []
    threading.Thread(target=lambda: lines.extend(proc.stdout), daemon=True).start()
    return proc, lines


def _prepared_txid(lines: list[str], proc: subprocess.Popen, timeout: float = 300) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for line in list(lines):
            if line.startswith("prepared "):
                return fields(line)["txid"]
        if proc.poll() is not None and not any(line.startswith("prepared ") for line in lines):
            break
        time.sleep(0.25)
    raise AssertionError(f"send never prepared: {lines}\n{proc.stderr.read()[-4000:]}")


def test_abandon_never_relayed_releases_inputs_and_resend_reaches_dashd(main, regtest_node, funded_miner):
    """QT-091 / IOS-034.

    Abandon: the wallet's 2 DASH coin is first spent by a transaction A that dashd takes into
    its mempool (submitted by the test from a `--no-broadcast` build). A second payment B from
    the same coin is then broadcast by the engine: dashd refuses it (mempool conflict) and,
    with no reject message on the p2p network, dash-spv reports no verdict. B is in the wallet
    as unconfirmed and has never been relayed. Abandoning it marks it Abandoned and the coin is
    spendable again once the abandon's rescan passes the coin's block. Mining then confirms A.

    Resend: a payment C left unmined (no verdict) is resent; dashd receives another `tx`
    message from the SPV peer, and C confirms once mined. platform-wallet also re-sends
    unconfirmed own transactions when a session loads and dash-spv sends a txid once per
    session, so the dashd log cannot tell the resend's message from that re-dispatch: the
    test proves the call succeeds with a connected peer and the transaction reaches dashd."""
    height = tip(regtest_node)
    main.sync(height)
    coins = main.utxos()
    coin = next(o for o, amount, _ in coins if amount == 2 * COIN)
    recipient = funded_miner.getnewaddress()

    out = main.run(
        "send", main.wallet, "--sync-height", str(height), "--to", f"{recipient}:{COIN // 2}",
        "--coin", coin, "--no-broadcast",
    )
    raw_a = main.line("raw", out).split()[1]
    txid_a = regtest_node.rpc.sendrawtransaction(raw_a)

    proc, lines = _send_in_background(main, height, "--to", f"{recipient}:{COIN // 4}", "--coin", coin)
    txid_b = _prepared_txid(lines, proc)
    assert txid_b != txid_a
    proc.wait(timeout=180)  # send.broadcast_unknown after dash-spv's 60 s acceptance timeout
    assert proc.returncode != 0, lines
    assert txid_b not in regtest_node.rpc.getrawmempool()

    rows = {r[0]: r for r in history(main)}
    assert rows[txid_b][4] == "Unconfirmed", rows[txid_b]
    extras = fields(main.line("extras", main.run("extras", main.wallet, txid_b)))
    assert extras["in_mempool"] == "unknown", "SPV never claims mempool knowledge"
    assert extras["can_abandon"] == "1" and extras["can_resend"] == "1", extras

    out = main.run("abandon", main.wallet, txid_b, "--sync-height", str(height))
    assert f"abandoned {txid_b}" in out, out
    assert coin in [line.split()[1] for line in out if line.startswith("utxo ")], out
    assert {r[0]: r for r in history(main)}[txid_b][4] == "Abandoned"
    extras = fields(main.line("extras", main.run("extras", main.wallet, txid_b)))
    assert extras["abandoned"] == "1" and extras["can_abandon"] == "0" and extras["can_resend"] == "0"
    assert "refused AlreadyAbandoned" in main.run("abandon", main.wallet, txid_b, "--sync-height", str(height))
    # A restart keeps it abandoned.
    assert {r[0]: r for r in history(main)}[txid_b][4] == "Abandoned"

    # dashd mines A: the coin is spent by A, and A's change comes back.
    regtest_node.mine(1)
    height = tip(regtest_node)
    main.sync(height)
    assert coin not in [o for o, _, _ in main.utxos()]
    assert txid_a in {r[0] for r in history(main)}

    # Resend of an unmined payment C.
    proc, lines = _send_in_background(main, height, "--to", f"{recipient}:{COIN // 10}")
    txid_c = _prepared_txid(lines, proc)
    wait_until(lambda: txid_c in regtest_node.rpc.getrawmempool(), timeout=90, what="C in dashd's mempool")
    proc.wait(timeout=180)
    before = regtest_node.logs().count("received: tx (")
    out = main.run("resend", main.wallet, txid_c, "--sync-height", str(height))
    assert f"resent {txid_c}" in out, out
    after = regtest_node.logs().count("received: tx (")
    assert after > before, (before, after)
    regtest_node.mine(1)
    height = tip(regtest_node)
    main.sync(height)
    rows = {r[0]: r for r in history(main)}
    assert rows[txid_c][4] in ("Confirming", "Confirmed"), rows[txid_c]
    assert "refused Confirmed" in main.run("resend", main.wallet, txid_c, "--sync-height", str(height))
    assert "refused Confirmed" in main.run("abandon", main.wallet, txid_c, "--sync-height", str(height))


def test_watch_only_wallet_sees_funds_and_cannot_sign(root, main, regtest_node, funded_miner):
    """QT-114 / IOS-111: the account xpub of a seed wallet, imported into another data
    directory with no vault, watches the same addresses."""
    xpub_line = main.line("xpub", main.run("xpub", main.wallet))
    xpub = xpub_line.split()[1]
    assert xpub.startswith("tpub") and "path=m/44'/1'/0'" in xpub_line

    # The data directory has a vault (grants can be issued) but no secret for this wallet.
    watch = new_cli(root, regtest_node, "watch")
    watch.wallet = watch.line("wallet_id", watch.run("watch-only", xpub, "--birth-height", "0")).split()[1]
    states = [line for line in watch.run("load-states") if line.startswith("wallet ")]
    assert fields(states[0])["watch_only"] == "1", states

    address = main.address()
    txid = funded_miner.sendtoaddress(address, Decimal("0.75"))
    regtest_node.mine(1)
    height = tip(regtest_node)
    main_total = int(main.sync(height)["total"])
    watch_total = int(watch.sync(height)["total"])
    assert watch_total == main_total, (watch_total, main_total)
    assert txid in {r[0] for r in history(watch)}

    # The CSV of a watch-only wallet carries dash-qt's Watch-only column.
    csv = watch.run("export-csv", watch.wallet)
    assert csv[0] == '"Confirmed","Watch-only","Date","Type","Label","Address","Amount (tDASH)","ID"'
    assert any(txid in line and ',"1",' in line for line in csv[1:]), csv

    # No keys: sending and signing are refused.
    proc = subprocess.run(
        [*watch.base, "send", watch.wallet, "--to", f"{funded_miner.getnewaddress()}:{COIN // 10}"],
        capture_output=True, text=True, timeout=120,
    )
    assert proc.returncode != 0
    assert "watch-only wallet" in proc.stderr, proc.stderr[-2000:]
    out = watch.run("console", f"signmessage {address} hello", "--wallet", watch.wallet)
    assert any(line.startswith(("error ", "engine_error ")) for line in out), out


def test_fee_policy_is_honest_and_min_relay_is_accepted(main, regtest_node, funded_miner):
    """QT-057/058: SPV has no estimator; every dash-qt target is the minimum relay fee, and a
    payment at that rate is accepted and mined. QT-072: dash-qt's coin-control estimate."""
    out = main.run("fees")
    policy = fields(main.line("policy", out))
    assert policy == {
        "source": "MinimumRelay", "min": "1000", "max_custom": "10000000",
        "max_fee": "10000000", "max_rate": "10000000",
    }, policy
    targets = [fields(line) for line in out if line.startswith("target ")]
    assert [int(t["blocks"]) for t in targets] == [2, 4, 6, 12, 24, 48, 144, 504, 1008]
    assert all(t["rate"] == "1000" for t in targets)
    # dashd itself has no estimate on a fresh regtest chain either.
    assert "errors" in regtest_node.rpc.estimatesmartfee(6)

    height = tip(regtest_node)
    main.sync(height)
    coin, amount, _ = max(main.utxos(), key=lambda c: c[1])
    summary = fields(main.line("summary", main.run("summary", main.wallet, "--coin", coin, "--pay", str(COIN // 10))))
    # 148·1 + 34·(1+1) + 10 bytes at 1000 duff/kB.
    assert summary["bytes"] == "226" and summary["fee"] == "226", summary
    assert int(summary["change"]) == amount - COIN // 10 - 226

    lines, tx = main.pay(regtest_node, height, "--to", f"{funded_miner.getnewaddress()}:{COIN // 10}", "--fee-per-kb", "1000")
    prepared = fields(main.line("prepared", lines))
    assert int(prepared["rate"]) >= 1000
    assert tx["confirmations"] >= 1 or tx.get("blockhash"), tx


def test_console_answers_from_the_engine(main, regtest_node):
    """QT-145: Core-named commands with Core's result shapes; full-node commands say so."""
    height = tip(regtest_node)
    out = main.run("console", "getblockcount", "--sync-height", str(height))
    assert out[out.index("result json=1") + 1] == str(height), out

    out = main.run("console", "getwalletinfo", "--wallet", main.wallet)
    body = "\n".join(out[out.index("result json=1") + 1:])
    assert '"walletname": "Wallet 1"' in body and '"private_keys_enabled": true' in body, body

    out = main.run("console", "getblockhash 0")
    assert "not_available getblockhash" in out, out
    out = main.run("console", "getbalance")
    assert "wallet_required" in out, out

    # Nested call and result query, as dash-qt's console.
    out = main.run("console", "validateaddress(getnewaddress())[isvalid]", "--wallet", main.wallet)
    assert out[out.index("result json=1") + 1] == "true", out

    # signmessage needs a grant; dwcli authorizes with the passphrase file, dashd verifies.
    address = main.address()
    out = main.run("console", f"signmessage {address} \"l2 console\"", "--wallet", main.wallet, "--no-grant")
    assert "authorization_required sign_message" in out, out
    out = main.run("console", f"signmessage {address} \"l2 console\"", "--wallet", main.wallet)
    signature = out[out.index("result json=0") + 1]
    assert regtest_node.rpc.verifymessage(address, signature, "l2 console") is True

    # walletpassphrase is redacted in the history line.
    out = main.run("console", 'walletpassphrase "main passphrase" 60')
    assert "history walletpassphrase(…)" in out, out

    out = main.run("console", "listtransactions \"*\" 2", "--wallet", main.wallet)
    body = "\n".join(out[out.index("result json=1") + 1:])
    assert body.count('"txid"') == 2, body


def test_new_transactions_are_batched(main, regtest_node, funded_miner):
    """QT-031/032: three payments mined in one block reach the host as one NewTransactions
    event; the notification rows come from tx_notices."""
    height = tip(regtest_node)
    main.sync(height)
    txids = {funded_miner.sendtoaddress(main.address(), Decimal("0.1")) for _ in range(3)}
    regtest_node.mine(1)
    out = main.run("watch-events", "--sync-height", str(tip(regtest_node)))
    events = [fields(line) for line in out if line.startswith("newtx ")]
    seen = [set(e["txids"].split(",")) for e in events]
    batches = [s for s in seen if s & txids]
    assert len(batches) == 1 and txids <= batches[0], events
    assert batches[0] == txids or len(batches[0]) >= 3
    # A single regtest node never reaches dash-spv's steady state.
    assert all(e["catch_up"] == "1" for e in events), events

    notices = [fields(line) for line in main.run("notices", main.wallet, *sorted(txids)) if line.startswith("notice ")]
    assert {n["txid"] for n in notices} == txids
    assert all(int(n["amount"]) == duffs(Decimal("0.1")) and n["coinjoin_internal"] == "0" for n in notices)


def test_close_and_open_wallet(root, main, regtest_node):
    """QT-101: a closed wallet is not loaded at the next start and catches up when opened."""
    other = main.line("wallet_id", main.run("create")).split()[1]
    main.run("unload", other)
    states = {line.split()[1]: fields(line) for line in main.run("load-states") if line.startswith("wallet ")}
    assert states[other]["loaded"] == "0" and states[other]["startup"] == "0", states
    assert states[main.wallet]["loaded"] == "1"
    main.run("load", other)
    states = {line.split()[1]: fields(line) for line in main.run("load-states") if line.startswith("wallet ")}
    assert states[other]["loaded"] == "1" and states[other]["startup"] == "1", states
