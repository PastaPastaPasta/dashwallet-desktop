#!/usr/bin/env python3
"""Writes the MG-01 masternode and governance vectors from a regtest dashd:

- testdata/protx_collateral.json: two registrations as dashd builds them,
  `protx register_fund` (the transaction pays its own collateral: a null
  collateral hash in the payload) and `protx register_prepare` +
  `register_submit` (an existing collateral output), with the raw ProRegTx,
  `protx info`'s collateral outpoint and address, and for the second the
  `signMessage` dashd asks the collateral key to sign and that signature.
- testdata/governance_votes.json: funding votes signed with `signmessage`
  by each masternode's voting key, accepted by `voteraw` (so dashd checked
  the signature), with the vote hashes `gobject getcurrentvotes` lists.
- testdata/governance_budget.json: `getgovernanceinfo` and
  `getsuperblockbudget` on regtest.

The regtest keys in these files belong to a throwaway regtest wallet.

Usage (a fresh regtest dashd with a legacy wallet available):
  dashd -regtest -datadir=$DIR -daemon -listen=0 -rpcport=29461 -rpcuser=u -rpcpassword=p \
        -fallbackfee=0.00001 -txindex=1
  python3 testdata/oracle/dashd/gen_masternode_vectors.py --rpc http://u:p@127.0.0.1:29461
"""
import argparse
import base64
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import TESTDATA, Rpc, write_json  # noqa: E402

WALLET = "mn"
COLLATERAL = 1000
# Superblocks on regtest: start 1500, cycle 20.
BUDGET_HEIGHTS = [1499, 1500, 1501, 1520, 1540, 1560, 2000]


def mine(rpc, n, to):
    return rpc.call("generatetoaddress", n, to)


def finish_mnsync(rpc):
    for _ in range(20):
        if rpc.call("mnsync", "status")["IsSynced"]:
            return
        rpc.call("mnsync", "next")
    raise RuntimeError("mnsync did not finish")


def new_keys(rpc):
    return {role: rpc.call("getnewaddress", wallet=WALLET) for role in ("owner", "voting", "payout")}


def funded_address(rpc, miner, amount):
    """A fresh address holding one confirmed `amount` output: the ProTx
    calls fund their fee (and a fund-new collateral) from one address."""
    address = rpc.call("getnewaddress", wallet=WALLET)
    rpc.call("sendtoaddress", address, amount, wallet=WALLET)
    mine(rpc, 1, miner)
    return address


def operator_keys(rpc):
    """A dashd-generated operator key: its basic and legacy public forms
    (`bls fromsecret <secret> true`). The secret is not written out."""
    bls = rpc.call("bls", "generate")
    legacy = rpc.call("bls", "fromsecret", bls["secret"], True)
    return bls["public"], legacy["public"]


def registration(rpc, pro_tx_hash):
    info = rpc.call("protx", "info", pro_tx_hash)
    raw = rpc.call("getrawtransaction", pro_tx_hash, True)
    return info, raw


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    info = rpc.call("getnetworkinfo")
    source = f"regtest dashd {info.get('buildversion', info['subversion'])}, gen_masternode_vectors.py"

    if WALLET not in rpc.call("listwallets"):
        # A legacy wallet: dumpprivkey hands the test the voting and
        # collateral keys.
        rpc.call("createwallet", WALLET, False, False, "", False, False, False)
    miner = rpc.call("getnewaddress", wallet=WALLET)
    if rpc.call("getblockcount") < 600:
        mine(rpc, 600 - rpc.call("getblockcount"), miner)
    finish_mnsync(rpc)

    regs = []

    # 1. register_fund: the ProRegTx pays its own collateral.
    k = new_keys(rpc)
    collateral_address = rpc.call("getnewaddress", wallet=WALLET)
    operator, operator_legacy = operator_keys(rpc)
    fund = funded_address(rpc, miner, COLLATERAL + 1)
    pro_tx_hash = rpc.call(
        "protx", "register_fund", collateral_address, ["127.0.0.1:19899"], k["owner"],
        operator, k["voting"], "0", k["payout"], fund, wallet=WALLET,
    )
    mine(rpc, 1, miner)
    info, raw = registration(rpc, pro_tx_hash)
    regs.append({
        "kind": "fund_new",
        "pro_tx_hash": pro_tx_hash,
        "tx_hex": raw["hex"],
        "collateral_hash": info["collateralHash"],
        "collateral_index": info["collateralIndex"],
        "collateral_address": info["collateralAddress"],
        "operator_public": operator,
        "operator_public_legacy": operator_legacy,
        "voting_address": info["state"]["votingAddress"],
        "voting_wif": rpc.call("dumpprivkey", k["voting"], wallet=WALLET),
    })

    # 2. register_prepare + register_submit: an existing collateral output.
    k = new_keys(rpc)
    collateral_address = rpc.call("getnewaddress", wallet=WALLET)
    coll_txid = rpc.call("sendtoaddress", collateral_address, COLLATERAL, wallet=WALLET)
    mine(rpc, 1, miner)
    coll_tx = rpc.call("getrawtransaction", coll_txid, True)
    coll_out = next(
        o for o in coll_tx["vout"]
        if o["scriptPubKey"].get("address") == collateral_address
    )
    coll_index = coll_out["n"]
    operator, operator_legacy = operator_keys(rpc)
    fund = funded_address(rpc, miner, 1)
    prepared = rpc.call(
        "protx", "register_prepare", coll_txid, coll_index, ["127.0.0.2:19899"], k["owner"],
        operator, k["voting"], "0", k["payout"], fund, wallet=WALLET,
    )
    signature = rpc.call("signmessage", prepared["collateralAddress"], prepared["signMessage"], wallet=WALLET)
    pro_tx_hash = rpc.call("protx", "register_submit", prepared["tx"], signature, wallet=WALLET)
    mine(rpc, 1, miner)
    info, raw = registration(rpc, pro_tx_hash)
    regs.append({
        "kind": "existing",
        "pro_tx_hash": pro_tx_hash,
        "tx_hex": raw["hex"],
        "collateral_hash": info["collateralHash"],
        "collateral_index": info["collateralIndex"],
        "collateral_address": info["collateralAddress"],
        "operator_public": operator,
        "operator_public_legacy": operator_legacy,
        "voting_address": info["state"]["votingAddress"],
        "voting_wif": rpc.call("dumpprivkey", k["voting"], wallet=WALLET),
        "collateral_script_hex": coll_out["scriptPubKey"]["hex"],
        "collateral_value": coll_out["value"],
        "sign_message": prepared["signMessage"],
        "collateral_signature_hex": base64.b64decode(signature).hex(),
        "collateral_wif": rpc.call("dumpprivkey", prepared["collateralAddress"], wallet=WALLET),
    })
    write_json(os.path.join(TESTDATA, "protx_collateral.json"), {
        "source": source,
        "network": "regtest",
        "cases": regs,
    })

    # 3. A proposal, then one funding vote per masternode through voteraw.
    now = int(time.time())
    payee = rpc.call("getnewaddress", wallet=WALLET)
    data = (
        f'{{"name":"mg01-votes","payment_address":"{payee}","payment_amount":5,'
        f'"url":"https://example.org/mg01","start_epoch":{now - 60},"end_epoch":{now + 86_400 * 30},"type":1}}'
    )
    data_hex = data.encode().hex()
    rpc.call("gobject", "check", data_hex)
    fee_txid = rpc.call("gobject", "prepare", "0", 1, now, data_hex, wallet=WALLET)
    mine(rpc, 6, miner)
    object_hash = rpc.call("gobject", "submit", "0", 1, now, data_hex, fee_txid)

    votes = []
    for reg, outcome in zip(regs, ("yes", "no")):
        outcome_code = {"yes": 1, "no": 2, "abstain": 3}[outcome]
        vote_time = int(time.time())
        message = (
            f'{reg["collateral_hash"]}-{reg["collateral_index"]}|{object_hash}|1|{outcome_code}|{vote_time}'
        )
        sig_b64 = rpc.call("signmessage", reg["voting_address"], message, wallet=WALLET)
        result = rpc.call(
            "voteraw", reg["collateral_hash"], reg["collateral_index"], object_hash,
            "funding", outcome, vote_time, sig_b64,
        )
        votes.append({
            "collateral_hash": reg["collateral_hash"],
            "collateral_index": reg["collateral_index"],
            "outcome": outcome_code,
            "signal": 1,
            "time": vote_time,
            "message": message,
            "voting_address": reg["voting_address"],
            "voting_wif": reg["voting_wif"],
            "signature_hex": base64.b64decode(sig_b64).hex(),
            "voteraw": result,
        })
    current = rpc.call("gobject", "getcurrentvotes", object_hash)
    write_json(os.path.join(TESTDATA, "governance_votes.json"), {
        "source": source,
        "network": "regtest",
        "object_hash": object_hash,
        "object": {"revision": 1, "time": now, "data_hex": data_hex},
        "votes": votes,
        "current_votes": current,
    })

    # 4. Treasury: mine past the first superblock, then read the limits.
    tip = rpc.call("getblockcount")
    if tip < max(BUDGET_HEIGHTS):
        mine(rpc, max(BUDGET_HEIGHTS) - tip, miner)
    write_json(os.path.join(TESTDATA, "governance_budget.json"), {
        "source": source,
        "networks": {
            "regtest": {
                "governanceinfo": rpc.call("getgovernanceinfo"),
                "budgets": {str(h): rpc.call("getsuperblockbudget", h) for h in BUDGET_HEIGHTS},
            },
        },
    })


if __name__ == "__main__":
    main()
