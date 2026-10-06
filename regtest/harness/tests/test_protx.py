"""protx: masternode provider transactions from dwcli (dw-engine over SPV) against dashd.

Covers (docs/contracts/m3-engine.md §2.3-2.5, R3):
* QT-123/124: register a regular masternode with a collateral the registration creates
  (`protx register_fund`) and with an existing exact-amount wallet UTXO (`protx register`),
  and an EvoNode (fund new); the operator-secret gate refuses the broadcast until the last
  four characters are confirmed; dashd's `protx info` shows the payload dwcli built;
* QT-125: Update Service, Update Registrar (new payout) and Revoke (reason 1), each accepted
  by dashd and visible in `protx info`;
* QT-118/120: the list shows the wallet's masternodes as owned, with their status;
* IOS-083: the keychain lists the owner key a registration used;
* QT-126/127: the v24 shared-masternode payload codecs (ProDisTx, ProUpShareTx,
  ProUpSharedRegTx) decode in dashd to the fields dw-protx wrote. The shared registration
  itself needs the v24 fork, which activates through an MNHF signal from a quorum and so
  never activates on a single node; it is not run here.

Needs a dwcli built from this tree (`DWCLI=/path/to/dwcli`; skipped when absent):

    DWD_COMPOSE_PROJECT=dwd-r3 DWD_REGTEST_BUILD=0 DWCLI=... \\
        .venv/bin/python -m pytest -v tests/test_protx.py

DIP3 activates at height 432 and is enforced from 500 on regtest (`src/chainparams.cpp`), so the
suite mines past 500 first. With one peer and no InstantSend quorum, dash-spv accepts a
broadcast only once it is mined, so every provider transaction runs in the background while
the test mines it from dashd's mempool.
"""

from __future__ import annotations

import subprocess
import threading
from decimal import Decimal

import pytest

from dwd_regtest import wait_until

from test_l1_send import COIN, DWCLI, Dwcli, fields

pytestmark = pytest.mark.skipif(DWCLI is None, reason="no dwcli binary (set DWCLI)")

DIP3_ENFORCED = 500
PLATFORM_NODE_ID = "0b1d2c3e4f5a6b7c8d9e0f1a2b3c4d5e6f708192"


def run_and_mine(dw: Dwcli, node, *args: str, timeout: float = 300) -> list[str]:
    """Runs a dwcli provider command in the background; once its `prepared txid=…` line is
    out and dashd has the transaction, mines a block so dash-spv accepts the broadcast."""
    proc = subprocess.Popen(
        [*dw.base, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    stderr: list[str] = []
    lines: list[str] = []
    threading.Thread(target=lambda: stderr.append(proc.stderr.read()), daemon=True).start()

    def read_stdout() -> None:
        for line in proc.stdout:
            lines.append(line.rstrip("\n"))

    reader = threading.Thread(target=read_stdout, daemon=True)
    reader.start()

    def prepared_txid() -> str | None:
        found = [line for line in lines if line.startswith("prepared ")]
        if found:
            return fields(found[0])["txid"]
        if proc.poll() is not None and not reader.is_alive():
            raise AssertionError(
                f"{args[0]} exited {proc.returncode} before preparing: {lines}\n"
                f"{''.join(stderr)[-4000:]}"
            )
        return None

    txid = wait_until(prepared_txid, timeout=timeout, what=f"{args[0]} prepared")
    wait_until(lambda: txid in node.rpc.getrawmempool(), timeout=120, what=f"{txid} in mempool")
    node.mine(1)
    proc.wait(timeout=240)
    reader.join(timeout=30)
    if proc.returncode != 0:
        raise AssertionError(f"{args[0]} exited {proc.returncode}: {lines}\n{''.join(stderr)[-4000:]}")
    assert any(line.startswith("broadcast ") for line in lines), lines
    return lines


@pytest.fixture(scope="module")
def chain(regtest_node, funded_miner):
    """Mines past DIP3 enforcement."""
    height = regtest_node.rpc.getblockcount()
    if height < DIP3_ENFORCED + 5:
        regtest_node.mine(DIP3_ENFORCED + 5 - height)
    return regtest_node


@pytest.fixture(scope="module")
def dw(chain, tmp_path_factory) -> Dwcli:
    root = tmp_path_factory.mktemp("dwcli-protx")
    passfile = root / "pass"
    passfile.write_text("protx passphrase\n")
    cli = Dwcli(root / "data", f"{chain.host}:{chain.p2p_port}", passfile)
    cli.run("init-vault")
    created = cli.run("create")
    cli.wallet = cli.line("wallet_id", created).split()[1]
    cli.root = root
    return cli


@pytest.fixture(scope="module")
def funded(dw, chain):
    """One address per purpose, each the fee source of its flows (the Register wizard's "Fee
    source"; change goes back to it): 1010 DASH for the fund-new regular masternode, 4010 DASH
    for the fund-new EvoNode, an exact 1000 DASH coin for the existing-collateral registration
    and 10 DASH for its fee and every maintenance transaction."""
    addresses = {}
    for purpose, amount in (("regular", 1010), ("evo", 4010), ("collateral", 1000), ("fees", 10)):
        addresses[purpose] = dw.address()
        chain.send_to_address(addresses[purpose], Decimal(amount))
    chain.mine(1)
    height = chain.rpc.getblockcount()
    synced = dw.sync(height)
    assert int(synced["total"]) == (1010 + 4010 + 1000 + 10) * COIN, synced
    return addresses


def payout_address(chain) -> str:
    return chain.ensure_wallet("miner").getnewaddress()


def core_service(state: dict) -> str:
    """The Core P2P address of a `protx info` state (v24 lists it under `addresses`)."""
    if "service" in state:
        return state["service"]
    return state["addresses"]["core_p2p"][0]


def register(dw, chain, *args: str) -> tuple[str, dict, list[str]]:
    height = chain.rpc.getblockcount()
    lines = run_and_mine(
        dw, chain, "mn-register", dw.wallet, "--sync-height", str(height), *args
    )
    prepared = fields(dw.line("prepared", lines))
    pro_tx_hash = dw.line("broadcast", lines).split()[1]
    assert pro_tx_hash == prepared["txid"]
    chain.wait_for_tx(pro_tx_hash, confirmations=1)
    return pro_tx_hash, prepared, lines


@pytest.fixture(scope="module")
def regular(dw, chain, funded):
    """A regular masternode registered with a collateral the ProRegTx creates."""
    payout = payout_address(chain)
    pro_tx_hash, prepared, lines = register(
        dw,
        chain,
        "--service",
        "127.0.0.1:19901",
        "--payout",
        payout,
        "--fee-source",
        funded["regular"],
        "--wrong-last4",
        "zzzz",
    )
    secret = dw.line("operator_secret", lines).split()[1]
    secret_file = dw.root / "operator-regular"
    secret_file.write_text(secret + "\n")
    return {
        "hash": pro_tx_hash,
        "prepared": prepared,
        "lines": lines,
        "payout": payout,
        "secret_file": secret_file,
    }


def test_QT_123_register_fund_new_matches_dashd(regular, chain):
    info = chain.rpc.protx("info", regular["hash"])
    prepared = regular["prepared"]
    assert info["collateralHash"] == regular["hash"]
    collateral_txid, collateral_vout = prepared["collateral"].split(":")
    assert collateral_txid == regular["hash"]
    assert info["collateralIndex"] == int(collateral_vout)
    tx = chain.rpc.getrawtransaction(regular["hash"], True)
    assert tx["vout"][int(collateral_vout)]["value"] == Decimal(1000)
    state = info["state"]
    assert core_service(state) == "127.0.0.1:19901"
    assert state["payoutAddress"] == regular["payout"]
    assert state["ownerAddress"] == prepared["owner"]
    assert state["votingAddress"] == prepared["voting"]
    assert state["pubKeyOperator"] == prepared["operator"]
    assert info["type"] == "Regular"


def test_QT_124_operator_secret_gate_comes_before_broadcast(regular):
    lines = regular["lines"]
    assert "gate closed" in lines
    assert "gate wrong_last4_accepted=0" in lines
    assert lines.index("gate open") < next(
        i for i, line in enumerate(lines) if line.startswith("broadcast ")
    )
    assert int(regular["prepared"]["secret_required"]) == 1


def test_QT_123_register_existing_collateral(dw, chain, funded, regular):
    height = chain.rpc.getblockcount()
    candidates = [
        line.split()
        for line in dw.run("mn-collaterals", dw.wallet, "--sync-height", str(height))
        if line.startswith("candidate ")
    ]
    usable = [c for c in candidates if c[4] == "refusal=-"]
    assert len(usable) == 1, candidates
    # The fund-new collateral is listed but refused.
    regular_collateral = regular["prepared"]["collateral"]
    assert any(
        c[1] == regular_collateral and c[4] == "refusal=AlreadyCollateral" for c in candidates
    ), candidates
    outpoint = usable[0][1]
    payout = payout_address(chain)
    # The collateral key signs MakeSignString; dashd checks it (CheckStringSig).
    pro_tx_hash, prepared, _ = register(
        dw,
        chain,
        "--collateral",
        outpoint,
        "--service",
        "127.0.0.1:19902",
        "--payout",
        payout,
        "--fee-source",
        funded["fees"],
    )
    info = chain.rpc.protx("info", pro_tx_hash)
    txid, vout = outpoint.split(":")
    assert info["collateralHash"] == txid
    assert info["collateralIndex"] == int(vout)
    assert info["collateralAddress"] == funded["collateral"]
    assert info["state"]["payoutAddress"] == payout
    assert prepared["collateral"] == outpoint
    assert int(prepared["total"]) == int(prepared["fee"]), "no collateral is spent"


@pytest.fixture(scope="module")
def evonode(dw, chain, funded, regular):
    payout = payout_address(chain)
    pro_tx_hash, prepared, lines = register(
        dw,
        chain,
        "--node-type",
        "evo",
        "--service",
        "127.0.0.1:19903",
        "--payout",
        payout,
        "--platform-node-id",
        PLATFORM_NODE_ID,
        "--platform-p2p-port",
        "22200",
        "--platform-http-port",
        "22201",
        "--fee-source",
        funded["evo"],
    )
    secret_file = dw.root / "operator-evo"
    secret_file.write_text(dw.line("operator_secret", lines).split()[1] + "\n")
    return {"hash": pro_tx_hash, "prepared": prepared, "secret_file": secret_file}


def test_QT_123_register_evonode(chain, evonode):
    info = chain.rpc.protx("info", evonode["hash"])
    assert info["type"] == "Evo"
    state = info["state"]
    assert state["platformNodeID"] == PLATFORM_NODE_ID
    if "addresses" in state:
        assert state["addresses"]["platform_p2p"] == ["127.0.0.1:22200"]
        assert state["addresses"]["platform_https"] == ["127.0.0.1:22201"]
    else:
        assert (state["platformP2PPort"], state["platformHTTPPort"]) == (22200, 22201)
    tx = chain.rpc.getrawtransaction(evonode["hash"], True)
    vout = int(evonode["prepared"]["collateral"].split(":")[1])
    assert tx["vout"][vout]["value"] == Decimal(4000)


def test_QT_118_list_shows_owned_masternodes_with_status(dw, chain, regular, evonode):
    height = chain.rpc.getblockcount()
    rows = [
        line
        for line in dw.run("mn-list", "--owned", "--sync-height", str(height))
        if line.startswith("mn ")
    ]
    by_hash = {line.split()[1]: fields(line) for line in rows}
    assert {regular["hash"], evonode["hash"]} <= set(by_hash), rows
    row = by_hash[regular["hash"]]
    owned = row["owned"].split(",")
    assert {"Owner", "Collateral", "Voting"} <= set(owned), row
    assert row["service"] == "127.0.0.1:19901"
    assert row["payout"] == regular["payout"]
    assert by_hash[evonode["hash"]]["type"] == "evo"
    # A single dashd has no quorums, so dash-spv's masternode phase never finishes there
    # (README: it stalls on getqrinfo) and the SPV list is unavailable: the status is the
    # honest "unknown". With a synced list it is "active".
    assert row["status"] in ("active", "unknown"), row
    registered = chain.rpc.getrawtransaction(regular["hash"], True)
    assert int(row["registered"]) == chain.rpc.getblock(registered["blockhash"])["height"]
    dashd = {e["proTxHash"] for e in chain.rpc.protx("list", "registered", True)}
    assert set(by_hash) <= dashd, (set(by_hash), dashd)
    state = fields(dw.line("mnstate", dw.run("mn-state")))
    print("list status:", {h: r["status"] for h, r in by_hash.items()}, "mn-state:", state)


def test_IOS_083_keychain_lists_the_owner_key_in_use(dw, regular):
    lines = [
        line
        for line in dw.run("mn-keys", dw.wallet, "--role", "owner", "--count", "3", "--reveal")
        if line.startswith(("key ", "revealed "))
    ]
    keys = [fields(line) for line in lines if line.startswith("key ")]
    assert len(keys) == 3
    assert keys[0]["path"] == "m/9'/1'/3'/2'/0"
    assert keys[0]["address"] == regular["prepared"]["owner"]
    assert regular["hash"] in keys[0]["used_by"].split(","), lines
    revealed = fields(next(line for line in lines if line.startswith("revealed ")))
    assert len(revealed["wif"]) == 52


def test_QT_125_update_service(dw, chain, funded, regular):
    height = chain.rpc.getblockcount()
    lines = run_and_mine(
        dw,
        chain,
        "mn-update-service",
        dw.wallet,
        regular["hash"],
        "--service",
        "127.0.0.1:19911",
        "--operator-secret-file",
        str(regular["secret_file"]),
        "--fee-source",
        funded["fees"],
        "--sync-height",
        str(height),
    )
    txid = dw.line("broadcast", lines).split()[1]
    tx = chain.wait_for_tx(txid, confirmations=1)
    assert tx["type"] == 2, tx
    info = chain.rpc.protx("info", regular["hash"])
    assert core_service(info["state"]) == "127.0.0.1:19911"


def test_IOS_082_tracked_masternode_signs_with_its_attached_operator_key(
    dw, chain, funded, evonode
):
    """Track the EvoNode, attach its operator key, then Update Service (the IOS-081 unban
    path) without typing the secret: the engine signs with the attached key."""
    lines = dw.run("mn-track", evonode["hash"], "--label", "evo-1")
    assert f"tracked {evonode['hash']} label=evo-1" in lines
    attached = dw.run(
        "mn-attach",
        evonode["hash"],
        "--role",
        "operator",
        "--key-file",
        str(evonode["secret_file"]),
        "--wallet",
        dw.wallet,
    )
    assert "attached" in attached
    tracked = fields(dw.line("tracked", dw.run("mn-tracked")))
    assert tracked["attached"] == "Operator" and tracked["update_service"] == "1", tracked
    height = chain.rpc.getblockcount()
    lines = run_and_mine(
        dw,
        chain,
        "mn-update-service",
        dw.wallet,
        evonode["hash"],
        "--service",
        "127.0.0.1:19913",
        "--platform-p2p-port",
        "22200",
        "--platform-http-port",
        "22201",
        "--fee-source",
        funded["fees"],
        "--sync-height",
        str(height),
    )
    txid = dw.line("broadcast", lines).split()[1]
    chain.wait_for_tx(txid, confirmations=1)
    state = chain.rpc.protx("info", evonode["hash"])["state"]
    assert core_service(state) == "127.0.0.1:19913"
    assert dw.line("untracked", dw.run("mn-untrack", evonode["hash"])) == "untracked 1"


def test_QT_125_update_registrar_changes_only_the_payout(dw, chain, funded, regular):
    before = chain.rpc.protx("info", regular["hash"])["state"]
    new_payout = payout_address(chain)
    height = chain.rpc.getblockcount()
    lines = run_and_mine(
        dw,
        chain,
        "mn-update-registrar",
        dw.wallet,
        regular["hash"],
        "--payout",
        new_payout,
        "--fee-source",
        funded["fees"],
        "--sync-height",
        str(height),
    )
    prepared = fields(dw.line("prepared", lines))
    assert prepared["bans"] == "0"
    txid = dw.line("broadcast", lines).split()[1]
    tx = chain.wait_for_tx(txid, confirmations=1)
    assert tx["type"] == 3, tx
    after = chain.rpc.protx("info", regular["hash"])["state"]
    assert after["payoutAddress"] == new_payout
    assert after["pubKeyOperator"] == before["pubKeyOperator"]
    assert after["votingAddress"] == before["votingAddress"]


def test_QT_125_revoke(dw, chain, funded, regular):
    height = chain.rpc.getblockcount()
    lines = run_and_mine(
        dw,
        chain,
        "mn-revoke",
        dw.wallet,
        regular["hash"],
        "--reason",
        "1",
        "--operator-secret-file",
        str(regular["secret_file"]),
        "--fee-source",
        funded["fees"],
        "--sync-height",
        str(height),
    )
    txid = dw.line("broadcast", lines).split()[1]
    tx = chain.wait_for_tx(txid, confirmations=1)
    assert tx["type"] == 4, tx
    state = chain.rpc.protx("info", regular["hash"])["state"]
    assert state["revocationReason"] == 1
    assert state["PoSeBanHeight"] > 0
    detail = fields(dw.line("detail", dw.run("mn-info", regular["hash"])))
    assert detail["revoked"] == "1"


def test_QT_127_shared_payload_codecs_decode_in_dashd(dw, chain):
    lines = dw.run("mn-shared-vectors")
    vectors = {line.split()[1]: line.split()[2] for line in lines if line.startswith("vector ")}
    expect = fields(dw.line("expect", lines))
    dis = chain.rpc.decoderawtransaction(vectors["prodistx"])
    assert dis["type"] == 10
    assert dis["proDisTx"] == {
        "version": 1,
        "proTxHash": expect["pro_tx_hash"],
        "actorIndex": 1,
        "sigCount": 1,
    }
    share = chain.rpc.decoderawtransaction(vectors["proupsharetx"])
    assert share["type"] == 11
    p = share["proUpShareTx"]
    assert (p["version"], p["proTxHash"], p["shareIndex"]) == (1, expect["pro_tx_hash"], 2)
    assert p["inputsHash"] == expect["inputs_hash"]
    assert p["rewardAddress"] == share["vout"][0]["scriptPubKey"]["address"]
    reg = chain.rpc.decoderawtransaction(vectors["proupsharedregtx"])
    assert reg["type"] == 12
    r = reg["proUpSharedRegTx"]
    assert r["pubKeyOperator"] == expect["operator"]
    assert r["sigCount"] == 2
    assert r["inputsHash"] == expect["inputs_hash"]
    assert r["proTxHash"] == expect["pro_tx_hash"]
