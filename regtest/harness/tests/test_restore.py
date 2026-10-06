"""Regtest suite `restore` (DESIGN-opus §4.3) and the dash-qt compatibility items §4.4 1-3 that
need a node (R2, M2). Drives a host-built `dwcli` (DWCLI=/path/to/dwcli; skipped when absent).

Import (§4.4 item 1, §4.3 `restore`): dashd v24 wallets with a known phrase and BIP39 passphrase
(ASCII, non-ASCII with an encrypted wallet, a weak-checksum phrase with a 300-byte passphrase) are
funded, including an address 25 past the last used one (a gap over 20) and change from a spend.
Each is restored through every path that applies:

* the phrase and passphrase (`dwcli import --core-compat`);
* the descriptor `wallet.dat` (`import-wallet-dat`, with the wallet passphrase when encrypted);
* `dumpwallet` of a legacy wallet with the same phrase (`import-dump`);
* `listdescriptors true` (`import-key --descriptors-file`).

Every restore must give the same wallet id, the first 1000 receive and change addresses dashd
derives, dashd's balance after an SPV sync, and the labels.

Export (§4.4 item 2): a dwcli wallet funded by dashd is rebuilt in dashd from our phrase +
passphrase with `upgradetohd`, from our `dumpwallet` with `importwallet` on a legacy wallet, and
from our `importdescriptors` JSON; addresses and balance must match.

PSBT: dwcli "Create Unsigned" is signed by dashd `walletprocesspsbt` and broadcast by dwcli; a
dashd `walletcreatefundedpsbt` is signed by dwcli (signatures equal dashd's) and sent by dashd.

Signatures (§4.4 item 3) are covered by the `l1-send` suite (sign/verify both ways).
Not covered here: legacy BDB wallet.dat (M6), wallets mixed by Core's CoinJoin.

    DWD_COMPOSE_PROJECT=dwd-r2 DWD_REGTEST_BUILD=0 DWCLI=$CARGO_TARGET_DIR/debug/dwcli \\
        .venv/bin/python -m pytest -v tests/test_restore.py
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from decimal import Decimal
from pathlib import Path

import pytest

from dwd_regtest.node import wait_until

DWCLI = os.environ.get("DWCLI")
pytestmark = pytest.mark.skipif(not DWCLI, reason="set DWCLI to the dwcli binary to run the restore suite")

COIN = Decimal(100_000_000)
ADDRESS_COUNT = 1000
GAP_INDEX = 25

ABANDON = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
LETTER = "letter advice cage absurd amount doctor acoustic avoid letter advice cage above"
# Passes only Dash Core's weak checksum (testdata/oracle/bip39/weak_sample.txt line 1).
WEAK = "alter undo step harbor skate color young picture sand chef goat ordinary"
LEGAL = "legal winner thank year wave sausage worth useful legal winner thank yellow"
NON_ASCII = "pässwörd ñ € 🔑"
LONG_PASS = ("long passphrase é " * 16)[:290] + "0123456789"

VARIANTS = {
    # name: (phrase, BIP39 passphrase, wallet.dat passphrase, legacy dump too)
    "plain": (ABANDON, "TREZOR", None, True),
    "encrypted": (LETTER, NON_ASCII, "wallet pass é 🔐", True),
    # Legacy wallets refuse BIP39 passphrases over 256 characters (`SetMnemonic`), so this one
    # has no dumpwallet path; descriptor wallets take it and cut the salt at 256 bytes.
    "weak_longpass": (WEAK, LONG_PASS, None, False),
}


def duffs(amount: Decimal) -> int:
    return int((Decimal(amount) * COIN).to_integral_exact())


def fields(line: str) -> dict[str, str]:
    return dict(re.findall(r"(\w+)=(\S+)", line))


class Dwcli:
    """One dwcli data directory with an encrypted vault."""

    def __init__(self, root: Path, peer_port: int):
        root.mkdir(parents=True, exist_ok=True)
        self.root = root
        passfile = root / "vault-pass"
        passfile.write_text("restore suite vault passphrase\n")
        self.base = [
            DWCLI,
            "--datadir", str(root / "data"),
            "--network", "regtest",
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            "--peer", f"127.0.0.1:{peer_port}",
            "--passphrase-file", str(passfile),
        ]
        self.run("init-vault")

    def proc(self, *args: str, stdin: str | None = None, timeout: float = 300) -> subprocess.CompletedProcess:
        return subprocess.run([*self.base, *args], input=stdin, capture_output=True, text=True, timeout=timeout)

    def run(self, *args: str, stdin: str | None = None, timeout: float = 300) -> str:
        p = self.proc(*args, stdin=stdin, timeout=timeout)
        if p.returncode != 0:
            raise AssertionError(f"dwcli {' '.join(args)} failed ({p.returncode}):\n{p.stdout}\n{p.stderr[-4000:]}")
        print(f"$ dwcli {' '.join(args)}\n{p.stdout[:2000]}")
        return p.stdout

    def secret_file(self, name: str, text: str) -> str:
        path = self.root / name
        path.write_bytes(text.encode() + b"\n")
        return str(path)

    def wallet_id(self, out: str) -> str:
        return re.search(r"wallet_id ([0-9a-f]{64})", out).group(1)

    def addresses(self, wallet: str, chain: str, count: int) -> list[str]:
        out = self.run("addresses", wallet, "--chain", chain, "--count", str(count))
        return [line.split()[2] for line in out.splitlines() if line.startswith("addr ")]

    def total(self, height: int) -> int:
        out = self.run("sync", "--min-height", str(height), "--timeout-secs", "240")
        lines = [line for line in out.splitlines() if " name=" in line]
        assert len(lines) == 1, out
        return int(fields(lines[0])["total"])


# --- dashd helpers ------------------------------------------------------------------------


def chain_addresses(wallet, internal: bool, count: int, start: int = 0) -> list[str]:
    for d in wallet.listdescriptors(False)["descriptors"]:
        if d.get("active") and bool(d.get("internal")) == internal:
            return wallet.deriveaddresses(d["desc"], [start, start + count - 1])
    raise AssertionError("no active descriptor")


def balance(wallet) -> int:
    mine = wallet.getbalances()["mine"]
    return duffs(mine["trusted"] + mine["untrusted_pending"] + mine["immature"])


def create_hd(node, name: str, phrase: str, passphrase: str, walletpass: str | None, descriptors: bool):
    node.rpc.createwallet(name, False, True, "", False, descriptors, False)
    w = node.wallet(name)
    if walletpass:
        w.upgradetohd(phrase, passphrase, walletpass)
    else:
        w.upgradetohd(phrase, passphrase)
    return w


def copy_wallet_dat(node, name: str) -> bytes:
    node.rpc.unloadwallet(name)
    try:
        return node.read_file(node.node_path(f"wallets/{name}/wallet.dat"))
    finally:
        node.rpc.loadwallet(name)


def unlocked(wallet, walletpass: str | None):
    if walletpass:
        wallet.walletpassphrase(walletpass, 120)
    return wallet


@pytest.fixture(scope="module")
def chain(regtest_node, funded_miner):
    """Creates and funds every dashd wallet once; the tests restore them."""
    node = regtest_node
    wallets = {}
    for name, (phrase, passphrase, walletpass, legacy) in VARIANTS.items():
        desc = create_hd(node, f"r_{name}", phrase, passphrase, walletpass, descriptors=True)
        if legacy:
            create_hd(node, f"r_{name}_legacy", phrase, passphrase, None, descriptors=False)
        receive = chain_addresses(desc, False, 1) + chain_addresses(desc, False, 1, start=GAP_INDEX)
        unlocked(desc, walletpass).setlabel(receive[0], f"label {name} é")
        for address, amount in zip(receive, ("1.25", "0.75")):
            funded_miner.sendtoaddress(address, Decimal(amount))
        wallets[name] = desc
    node.mine(1)
    # A spend from each wallet puts change on its internal chain.
    for name, desc in wallets.items():
        unlocked(desc, VARIANTS[name][2]).sendtoaddress(funded_miner.getnewaddress(), Decimal("0.3"))
    node.mine(1)
    return {"wallets": wallets, "height": node.rpc.getblockcount()}


def expected(desc) -> dict:
    return {
        "receive": chain_addresses(desc, False, ADDRESS_COUNT),
        "change": chain_addresses(desc, True, ADDRESS_COUNT),
        "balance": balance(desc),
    }


def check_restore(cli: Dwcli, wallet_id: str, want: dict, height: int):
    assert cli.addresses(wallet_id, "receive", ADDRESS_COUNT) == want["receive"]
    assert cli.addresses(wallet_id, "change", ADDRESS_COUNT) == want["change"]
    assert cli.total(height) == want["balance"]


@pytest.mark.parametrize("name", list(VARIANTS))
def test_restore_dashd_wallet_every_path(name, tmp_path, regtest_node, chain):
    phrase, passphrase, walletpass, legacy = VARIANTS[name]
    node = regtest_node
    desc = chain["wallets"][name]
    want = expected(desc)
    assert want["balance"] > 0
    ids = set()

    # 1. Phrase + passphrase, Dash Core BIP39 rules.
    cli = Dwcli(tmp_path / "mnemonic", node.p2p_port)
    out = cli.run("import", "--core-compat", "--birth-height", "0",
                  "--bip39-passphrase-file", cli.secret_file("bip39", passphrase), stdin=phrase)
    ids.add(cli.wallet_id(out))
    check_restore(cli, cli.wallet_id(out), want, chain["height"])

    # 2. The descriptor wallet.dat.
    cli = Dwcli(tmp_path / "walletdat", node.p2p_port)
    dat = tmp_path / "wallet.dat"
    dat.write_bytes(copy_wallet_dat(node, f"r_{name}"))
    args = ["import-wallet-dat", str(dat)]
    if walletpass:
        missing = cli.proc(*args)
        assert missing.returncode != 0 and "passphrase required" in missing.stderr, missing.stderr
        args += ["--walletpass-file", cli.secret_file("walletpass", walletpass)]
    out = cli.run(*args)
    wid = cli.wallet_id(out)
    ids.add(wid)
    assert fields(out)["labels"] != "0", out
    check_restore(cli, wid, want, chain["height"])
    book = cli.run("book", wid)
    assert f"label {name} é" in book, book

    # 3. dumpwallet of a legacy wallet with the same phrase.
    if legacy:
        cli = Dwcli(tmp_path / "dump", node.p2p_port)
        target = node.node_path(f"dump_{name}.txt")
        node.wallet(f"r_{name}_legacy").dumpwallet(target)
        dump = tmp_path / "dump.txt"
        dump.write_bytes(node.read_file(target))
        out = cli.run("import-dump", str(dump))
        ids.add(cli.wallet_id(out))
        check_restore(cli, cli.wallet_id(out), want, chain["height"])

    # 4. listdescriptors true.
    if not walletpass:
        cli = Dwcli(tmp_path / "descriptors", node.p2p_port)
        listed = tmp_path / "listdescriptors.json"
        listed.write_text(json.dumps(desc.listdescriptors(True), default=str))
        out = cli.run("import-key", "--descriptors-file", str(listed))
        ids.add(cli.wallet_id(out))
        assert cli.addresses(cli.wallet_id(out), "receive", 30) == want["receive"][:30]

    assert len(ids) == 1, ids


@pytest.fixture(scope="module")
def ours(tmp_path_factory, regtest_node, funded_miner):
    """A dwcli wallet (Core-compatible phrase, non-ASCII passphrase) funded by dashd."""
    node = regtest_node
    cli = Dwcli(tmp_path_factory.mktemp("ours"), node.p2p_port)
    out = cli.run("import", "--core-compat", "--birth-height", "0",
                  "--bip39-passphrase-file", cli.secret_file("bip39", NON_ASCII), stdin=LEGAL)
    wid = cli.wallet_id(out)
    receive = cli.addresses(wid, "receive", 4)
    txids = [funded_miner.sendtoaddress(receive[0], Decimal("2")),
             funded_miner.sendtoaddress(receive[3], Decimal("1.5"))]
    node.mine(1)
    height = node.rpc.getblockcount()
    total = cli.total(height)
    assert total == duffs(Decimal("3.5"))
    return {"cli": cli, "id": wid, "receive": receive, "txids": txids, "total": total}


def test_export_mnemonic_upgradetohd(regtest_node, ours):
    node, cli = regtest_node, ours["cli"]
    out = cli.run("core-compat", ours["id"])
    assert out.startswith("core_compatible 1"), out
    revealed = dict(line.split() for line in cli.run("reveal-mnemonic", ours["id"]).splitlines())
    phrase = bytes.fromhex(revealed["mnemonic_hex"]).decode()
    passphrase = bytes.fromhex(revealed["passphrase_hex"]).decode()
    assert (phrase, passphrase) == (LEGAL, NON_ASCII)
    w = create_hd(node, "x_upgradetohd", phrase, passphrase, None, descriptors=True)
    w.rescanblockchain()
    assert balance(w) == ours["total"]
    seen = {t["txid"] for t in w.listtransactions("*", 1000)}
    assert set(ours["txids"]) <= seen
    assert chain_addresses(w, False, 4) == ours["receive"]


def test_export_dumpwallet_importwallet(tmp_path, regtest_node, ours):
    node, cli = regtest_node, ours["cli"]
    dest = tmp_path / "ours.dump"
    out = cli.run("export-core", ours["id"], "--format", "dumpwallet", str(dest))
    assert "exported" in out
    assert oct(dest.stat().st_mode & 0o777) == "0o600"
    again = cli.proc("export-core", ours["id"], "--format", "dumpwallet", str(dest))
    assert again.returncode != 0 and "cannot write" in again.stderr, again.stderr
    target = node.node_path("ours_import.dump")
    node.write_file(target, dest.read_bytes())
    node.rpc.createwallet("x_importwallet", False, True, "", False, False, False)
    w = node.wallet("x_importwallet")
    w.importwallet(target)
    for address in ours["receive"]:
        info = w.getaddressinfo(address)
        assert info["ismine"] and info["solvable"], info
    assert balance(w) == ours["total"]


def test_export_importdescriptors(tmp_path, regtest_node, ours):
    node, cli = regtest_node, ours["cli"]
    dest = tmp_path / "ours.json"
    cli.run("export-core", ours["id"], "--format", "descriptors", str(dest))
    requests = json.loads(dest.read_text())
    node.rpc.createwallet("x_descriptors", False, True, "", False, True, False)
    w = node.wallet("x_descriptors")
    results = w.importdescriptors(requests)
    assert all(r["success"] for r in results), results
    assert chain_addresses(w, False, 20) == cli.addresses(ours["id"], "receive", 20)
    assert chain_addresses(w, True, 20) == cli.addresses(ours["id"], "change", 20)
    assert balance(w) == ours["total"]


def run_mining(node, cmd: list[str], txid: str) -> subprocess.CompletedProcess:
    """Runs a dwcli broadcast in the background and mines its transaction once dashd has it (a
    single peer without InstantSend accepts a broadcast only once it is mined)."""
    env = dict(os.environ, DWCLI_LOG=os.environ.get("DWCLI_LOG", "warn,dash_spv=info"))
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)

    def in_mempool() -> bool:
        if proc.poll() is not None:
            out, err = proc.communicate()
            seen = [line for line in node.logs().splitlines()
                    if txid[:16] in line or "not accepted" in line or "reject" in line.lower()]
            spv = [line for line in err.splitlines() if "roadcast" in line or "empool" in line or "peer" in line]
            raise AssertionError(f"dwcli exited ({proc.returncode}) before dashd saw {txid}:\n{out}\n"
                                 + "\n".join(spv[-60:])
                                 + "\n--- dashd lines about it ---\n" + "\n".join(seen[-40:]))
        return txid in node.rpc.getrawmempool()

    wait_until(in_mempool, 200, what=f"{txid} in dashd's mempool")
    node.mine(1)
    out, err = proc.communicate(timeout=400)
    print(f"$ dwcli psbt-broadcast\n{out}{err[-2000:]}")
    return subprocess.CompletedProcess(cmd, proc.returncode, out, err)


def test_psbt_round_trips_with_dashd(tmp_path, regtest_node, funded_miner, ours):
    node, cli = regtest_node, ours["cli"]
    # Same keys in dashd: the descriptor import of the previous test.
    if "x_descriptors" not in node.rpc.listwallets():
        test_export_importdescriptors(tmp_path, node, ours)
    dashd = node.wallet("x_descriptors")
    height = node.rpc.getblockcount()

    # dwcli Create Unsigned -> dashd walletprocesspsbt -> dwcli broadcast.
    pay_to = funded_miner.getnewaddress()
    out = cli.run("psbt-create", ours["id"], "--to", f"{pay_to}:50000000", "--sync-height", str(height))
    unsigned = out.split()[1]
    decoded = node.rpc.decodepsbt(unsigned)
    assert all(i.get("non_witness_utxo") for i in decoded["inputs"]), decoded
    assert all(i.get("bip32_derivs") for i in decoded["inputs"]), decoded
    processed = dashd.walletprocesspsbt(unsigned, True, "ALL", True)
    assert processed["complete"], processed
    signed_file = tmp_path / "signed.psbt"
    signed_file.write_text(processed["psbt"])
    analysis = cli.run("psbt-analyze", str(signed_file), "--wallet", ours["id"])
    assert "status=Complete" in analysis, analysis
    # Dash has no witness: the signed transaction's id differs from the unsigned one's.
    txid = node.rpc.decoderawtransaction(node.rpc.finalizepsbt(processed["psbt"])["hex"])["txid"]
    assert txid != decoded["tx"]["txid"]
    p = run_mining(node, [*cli.base, "psbt-broadcast", str(signed_file), "--wallet", ours["id"],
                          "--sync-height", str(height)], txid)
    assert p.returncode == 0 or "outcome unknown" in p.stderr, p.stderr[-3000:]
    assert node.wait_for_tx(txid, 1)["confirmations"] >= 1

    # dashd walletcreatefundedpsbt -> dwcli sign (dashd's signatures) -> dashd finalize + send.
    created = dashd.walletcreatefundedpsbt([], [{funded_miner.getnewaddress(): Decimal("0.2")}], 0,
                                           {"fee_rate": 2}, True)
    theirs = dashd.walletprocesspsbt(created["psbt"], True, "ALL", True, False)
    unsigned_file = tmp_path / "theirs.psbt"
    unsigned_file.write_text(created["psbt"])
    signed = cli.run("psbt-sign", ours["id"], str(unsigned_file)).split()[1]
    ours_inputs = node.rpc.decodepsbt(signed)["inputs"]
    their_inputs = node.rpc.decodepsbt(theirs["psbt"])["inputs"]
    assert [i["partial_signatures"] for i in ours_inputs] == [i["partial_signatures"] for i in their_inputs]
    final = node.rpc.finalizepsbt(signed)
    assert final["complete"], final
    sent = node.rpc.sendrawtransaction(final["hex"])
    node.mine(1)
    assert node.wait_for_tx(sent, 1)["confirmations"] >= 1
