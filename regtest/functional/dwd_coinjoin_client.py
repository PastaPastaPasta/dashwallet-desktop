#!/usr/bin/env python3
# Copyright (c) 2026 The Dash Wallet Desktop developers
# Distributed under the MIT software license, see test_framework/COPYING.
"""dwd_coinjoin_client.py — regtest suite `coinjoin` (M3 R1, QT-041…051, QT-112).

Our CoinJoin client (`dwcli`, built on dw-engine/dw-coinjoin) mixes against 4 regtest
masternodes. Regtest needs 2 participants per session (chainparams.cpp
`nPoolMinParticipants`). Counterparties: two dashd wallets and a second dwcli wallet. A dashd
client stops finding masternodes after its first sessions on a small regtest network: it keeps
the masternode connections it opened for mixing (CMasternodeUtils::DoMaintenance does not
disconnect them while it has fewer than its maximum outbound peers) and then skips every
masternode it is connected to (IsMasternodeOrDisconnectRequested). The second dwcli wallet keeps
sessions going; the dashd wallets show the protocol works with Core clients too.

Network: node0 controller/miner and the SPV peer of dwcli (BIP157 filters), nodes 1..2 the dashd
mixing counterparties, nodes 3..6 masternodes. Real time (no mocktime): our client checks
`dsq` timestamps against the wall clock as Core checks them against adjusted time.

Checks, in order:
 1. stop releases reservations: mixing stops while a session holds coins and no coin stays
    reserved afterwards (`after_stop reserved=0`);
 2. mixing reaches 1 round;
 3. restart resumes: a new dwcli process (new session, same data dir) continues from the rounds
    already earned and reaches 2 rounds with a fully mixed balance, the progress above what
    denominating alone gives;
 4. fully mixed coins are spendable through the CoinJoin send path only: an ordinary payment of
    more than the non-CoinJoin balance fails, `send --coinjoin` pays from fully mixed coins with
    no change output and history shows it as CoinJoinSend.

Run (host, darwin release binaries; DWCLI = a built dwcli):
  DWCLI=<target>/debug/dwcli DWD_PYTHON=<venv>/bin/python DASHCORE_DIR=<dashcore> \
      regtest/functional/run.sh dwd_coinjoin_client.py
"""

import os
import re
import subprocess
import threading
import time
from decimal import Decimal

from test_framework.test_framework import DashTestFramework
from test_framework.util import assert_equal, assert_greater_than, p2p_port

MIXERS = (1, 2)
MIXER_ARGS = [
    "-keypool=500",
    "-createwalletbackups=10",
    "-coinjoinamount=8",
    "-coinjoinrounds=8",
    "-coinjoinsessions=1",
    "-coinjoindenomsgoal=10",
    "-coinjoinmultisession=0",
    "-debug=coinjoin",
]
CONTROLLER_ARGS = ["-peerblockfilters=1", "-blockfilterindex=1", "-debug=coinjoin"]
PASSPHRASE = "dwd coinjoin regtest"


class Dwcli:
    def __init__(self, binary, datadir, peer_port, log):
        self.binary = binary
        self.datadir = datadir
        self.peer_port = peer_port
        self.log = log
        self.passfile = os.path.join(datadir, "pass")
        os.makedirs(datadir, exist_ok=True)
        with open(self.passfile, "w", encoding="utf8") as f:
            f.write(PASSPHRASE + "\n")

    def base(self):
        return [
            self.binary,
            "--datadir", os.path.join(self.datadir, "data"),
            "--network", "regtest",
            "--dapi", "http://127.0.0.1:1",
            "--quorum-url", "http://127.0.0.1:1",
            # No Platform here: no DashPay bring-up before SPV.
            "--no-platform",
            "--peer", f"127.0.0.1:{self.peer_port}",
            "--passphrase-file", self.passfile,
        ]

    def run(self, *args, timeout=600, check=True):
        cmd = self.base() + list(args)
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        self.log.info(f"dwcli {' '.join(args)} -> {p.returncode}\n{p.stdout}{p.stderr[-4000:]}")
        if check and p.returncode != 0:
            raise AssertionError(f"dwcli {args} failed: {p.stderr[-2000:]}")
        return p

    def popen(self, *args, stderr_path):
        cmd = self.base() + list(args)
        env = dict(os.environ)
        env.setdefault("DWCLI_LOG", "warn,dw_engine::coinjoin=debug,dw_coinjoin=debug")
        err = open(stderr_path, "a", encoding="utf8")
        return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=err, text=True, env=env)


def field(line, key):
    m = re.search(rf"\b{key}=(\S+)", line)
    return m.group(1) if m else None


class DwdCoinJoinClient(DashTestFramework):
    def add_options(self, parser):
        self.add_wallet_options(parser)

    def set_test_params(self):
        extra_args = [CONTROLLER_ARGS] + [MIXER_ARGS] * len(MIXERS) + [["-debug=coinjoin"]] * 4
        self.set_dash_test_params(1 + len(MIXERS) + 4, 4, extra_args)
        self.disable_mocktime = True

    def skip_test_if_missing_module(self):
        self.skip_if_no_wallet()
        if not os.environ.get("DWCLI"):
            raise SystemExit("set DWCLI to the dwcli binary")

    # --- helpers -----------------------------------------------------------------------

    def start_miner(self):
        """Mines a block every 8 s while dwcli runs (mixing and funding transactions confirm;
        the ChainLocks mining gate needs their InstantSend locks first)."""
        self.mining = True

        def loop():
            while self.mining:
                time.sleep(8)
                try:
                    self.generate(self.nodes[0], 1, sync_fun=self.no_op)
                except Exception as e:  # node shutting down at the end
                    self.log.info(f"miner: {e}")
                    return

        self.miner = threading.Thread(target=loop, daemon=True)
        self.miner.start()

    def stop_miner(self):
        self.mining = False
        self.miner.join(timeout=30)

    def mix(self, *conditions, timeout=1200):
        """Runs `coinjoin-mix` until the conditions hold; returns its output lines."""
        height = self.nodes[0].getblockcount()
        stderr_path = os.path.join(self.options.tmpdir, "dwcli-mix.log")
        p = self.dw.popen(
            "coinjoin-mix", self.wallet, "--mixing-only", "--sync-height", str(height),
            "--timeout-secs", str(timeout), *conditions, stderr_path=stderr_path,
        )
        lines = []
        for line in p.stdout:
            line = line.rstrip("\n")
            lines.append(line)
            self.log.info(f"dwcli: {line}")
        p.wait(timeout=300)
        self.log.info(f"coinjoin-mix {conditions} -> {p.returncode} (stderr in {stderr_path})")
        assert_equal(p.returncode, 0)
        return lines

    # --- test ----------------------------------------------------------------------------

    def run_test(self):
        controller = self.nodes[0]
        controller.sporkupdate("SPORK_17_QUORUM_DKG_ENABLED", 0)
        self.wait_for_sporks_same()
        self.mine_cycle_quorum()
        if len(controller.quorum("list")["llmq_test"]) == 0:
            self.mine_quorum(llmq_type_name="llmq_test", llmq_type=100)

        def new_client(name, rounds):
            dw = Dwcli(os.environ["DWCLI"], os.path.join(self.options.tmpdir, name), p2p_port(0), self.log)
            dw.run("init-vault")
            wallet = re.search(r"wallet_id (\w+)", dw.run("create").stdout).group(1)
            dw.run("coinjoin-settings", "--set", f"rounds={rounds}", "--set", "amount=4",
                   "--set", "goal=10", "--set", "multi=0")
            return dw, wallet

        self.dw, self.wallet = new_client("dwcli", 2)
        # The second dwcli wallet mixes 8 rounds so it keeps going all test long.
        helper, helper_wallet = new_client("dwcli-helper", 8)

        # Fund both dwcli wallets and the dashd counterparties.
        funding = []
        for dw, wallet in ((self.dw, self.wallet), (helper, helper_wallet)):
            address = dw.run("address", wallet).stdout.split()[1]
            funding += [controller.sendtoaddress(address, Decimal("3.3")) for _ in range(3)]
        for idx in MIXERS:
            mixer = self.nodes[idx]
            funding += [controller.sendtoaddress(mixer.getnewaddress(), Decimal("3.3")) for _ in range(3)]
        self.wait_for_instantlock(*funding, timeout=60)
        self.generate(controller, 2)
        for idx in MIXERS:
            self.nodes[idx].coinjoin("start")

        status = self.dw.run("coinjoin-status", self.wallet).stdout
        self.log.info(status)
        self.start_miner()
        helper_log = os.path.join(self.options.tmpdir, "dwcli-helper.log")
        helper_proc = helper.popen(
            "coinjoin-mix", helper_wallet, "--mixing-only", "--sync-height", str(controller.getblockcount()),
            "--timeout-secs", "5400", "--until-rounds", "99", stderr_path=helper_log,
        )

        def drain():
            with open(helper_log + ".out", "a", encoding="utf8") as out:
                for line in helper_proc.stdout:
                    out.write(line)
                    out.flush()

        threading.Thread(target=drain, daemon=True).start()
        try:
            def progress(lines):
                return [float(field(l, "progress")) for l in lines if l.startswith("cjstatus")]

            # 1. stop releases reservations (stopped while a session holds coins).
            lines = self.mix("--until-session-entry", timeout=900)
            after = [l for l in lines if l.startswith("after_stop")][-1]
            assert_equal(field(after, "reserved"), "0")
            # Denominating alone is 1 of (3 + rounds) weighted parts of the progress.
            denominated_only = max(progress(lines))

            # 2. one round.
            self.mix("--until-rounds", "1", timeout=1200)

            # 3. a new process resumes from the rounds earned and reaches 2 with fully mixed
            #    coins; the progress has grown past what denominating alone gives.
            lines = self.mix("--until-rounds", "2", "--until-fully-mixed", "1", timeout=1800)
            first = [l for l in lines if l.startswith("cjcoins")][0]
            assert int(field(first, "max_rounds")) >= 1, first
            final = [l for l in lines if l.startswith("mixed")][-1]
            assert int(field(final, "fully_mixed")) > 0, final
            assert_greater_than(max(progress(lines)), denominated_only)
        finally:
            helper_proc.terminate()
            helper_proc.wait(timeout=60)
            self.stop_miner()
        self.generate(controller, 2)

        # 4. fully mixed coins: the CoinJoin send path only.
        height = controller.getblockcount()
        self.dw.run("sync-wallet", self.wallet, "--height", str(height))
        out = self.dw.run("coinjoin-status", self.wallet).stdout
        spend = [l for l in out.splitlines() if l.startswith("cjspendable")][0]
        any_spendable = int(field(spend, "any"))
        mixed_spendable = int(field(spend, "fully_mixed"))
        assert_greater_than(mixed_spendable, 0)
        payee = controller.getnewaddress()
        too_much = any_spendable + 1
        height = str(controller.getblockcount())
        assert too_much <= any_spendable + mixed_spendable
        p = self.dw.run("send", self.wallet, "--sync-height", height, "--to", f"{payee}:{too_much}", check=False)
        assert p.returncode != 0, "an ordinary payment must not spend mixed coins"
        assert "exceeds" in p.stderr, p.stderr
        coins = self.dw.run("coinjoin-utxos", self.wallet, "--fully-mixed").stdout
        values = sorted(int(l.split()[2]) for l in coins.splitlines() if l.startswith("cjutxo"))
        amount = values[0] - 2000
        # dash-spv accepts a broadcast once it sees it in a block (it cannot verify regtest
        # InstantSend locks of the rotating quorum), so blocks keep coming while dwcli waits.
        self.start_miner()
        try:
            out = self.dw.run(
                "send", self.wallet, "--sync-height", height, "--coinjoin", "--to", f"{payee}:{amount}"
            ).stdout
        finally:
            self.stop_miner()
        txid = re.search(r"broadcast (\w+)", out).group(1)
        assert "change=1" not in out, "the CoinJoin page pays no change"
        tx = controller.getrawtransaction(txid, True)
        assert_equal(len(tx["vout"]), 1)
        assert_equal(tx["vout"][0]["valueSat"], amount)
        self.generate(controller, 1)
        self.dw.run("sync-wallet", self.wallet, "--height", str(controller.getblockcount()))
        history = self.dw.run("history").stdout
        assert any(txid in l and "CoinJoinSend" in l for l in history.splitlines()), history
        self.log.info("CoinJoin client suite passed")


if __name__ == "__main__":
    DwdCoinJoinClient().main()
