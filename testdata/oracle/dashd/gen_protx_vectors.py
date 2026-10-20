#!/usr/bin/env python3
"""Writes the MG-04 ProRegTx and ProUpRegTx vectors from a regtest dashd:

- testdata/protx_registration.json: one registration per `protx` flavour,
  regular and Evo masternodes: `register_fund[_evo]` (the transaction pays
  its own collateral), `register[_evo]` (an existing collateral output,
  signed by dashd) and `register_prepare[_evo]` + `register_submit` (an
  existing collateral output, signed with `signmessage`). Each case has the
  signed ProRegTx from `getrawtransaction`, the arguments given to the RPC,
  the owner and voting WIFs, and `protx info` after one confirmation. The
  prepared cases also have the unsigned transaction, the `signMessage`
  dashd asks the collateral key to sign and that signature.
- testdata/protx_update_registrar.json: `protx update_registrar` on four of
  those masternodes (new voting key, new payout address, new operator key,
  and all three on an Evo node) with the signed ProUpRegTx and `protx info`
  before and after.

The regtest keys in these files belong to a throwaway regtest wallet; the
operator BLS secrets are not written out.

Usage (a fresh regtest dashd with a legacy wallet available):
  dashd -regtest -datadir=$DIR -daemon -listen=0 -rpcport=29571 -port=29572 -rpcuser=u -rpcpassword=p \
        -fallbackfee=0.00001 -txindex=1
  python3 testdata/oracle/dashd/gen_protx_vectors.py --rpc http://u:p@127.0.0.1:29571
"""
import argparse
import base64
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import TESTDATA, Rpc, write_json  # noqa: E402

WALLET = "mn"
COLLATERAL = 1000
EVO_COLLATERAL = 4000
# The Platform ports Evo nodes announce (the regtest defaults).
PLATFORM_P2P_PORT = 22200
PLATFORM_HTTP_PORT = 22201
# Fixed, so a rerun picks the same Platform node IDs.
RNG = random.Random(0x4D4730345F)


def mine(rpc, n, to):
    return rpc.call("generatetoaddress", n, to)


def finish_mnsync(rpc):
    for _ in range(20):
        if rpc.call("mnsync", "status")["IsSynced"]:
            return
        rpc.call("mnsync", "next")
    raise RuntimeError("mnsync did not finish")


def new_address(rpc):
    return rpc.call("getnewaddress", wallet=WALLET)


def funded_address(rpc, miner, amount):
    """A fresh address holding one confirmed `amount` output: the ProTx
    calls fund their fee (and a fund-new collateral) from one address."""
    address = new_address(rpc)
    rpc.call("sendtoaddress", address, amount, wallet=WALLET)
    mine(rpc, 1, miner)
    return address


def operator_public(rpc):
    """A dashd-generated basic-scheme operator public key. The secret is
    not written out."""
    return rpc.call("bls", "generate")["public"]


def new_roles(rpc, operator_reward):
    """The addresses and key a registration is made of."""
    return {
        "owner_address": new_address(rpc),
        "voting_address": new_address(rpc),
        "payout_address": new_address(rpc),
        "operator_public": operator_public(rpc),
        "operator_reward": operator_reward,
    }


def existing_collateral(rpc, miner, amount):
    """A confirmed `amount` output to a fresh address, and what a test
    needs to spend or sign for it."""
    address = new_address(rpc)
    txid = rpc.call("sendtoaddress", address, amount, wallet=WALLET)
    mine(rpc, 1, miner)
    tx = rpc.call("getrawtransaction", txid, True)
    out = next(o for o in tx["vout"] if o["scriptPubKey"].get("address") == address)
    return {
        "collateral_hash": txid,
        "collateral_index": out["n"],
        "collateral_address": address,
        "collateral_wif": rpc.call("dumpprivkey", address, wallet=WALLET),
        "collateral_script_hex": out["scriptPubKey"]["hex"],
        "collateral_value": out["value"],
    }


def evo_platform():
    return {
        "platform_node_id": RNG.randbytes(20).hex(),
        "platform_p2p_port": PLATFORM_P2P_PORT,
        "platform_http_port": PLATFORM_HTTP_PORT,
    }


def case(rpc, kind, evo, service, roles, pro_tx_hash, miner, extra):
    """A finished registration: mines one block, then reads the signed
    transaction and `protx info` back."""
    mine(rpc, 1, miner)
    entry = {
        "kind": kind,
        "masternode_type": "evo" if evo else "regular",
        "pro_tx_hash": pro_tx_hash,
        "tx_hex": rpc.call("getrawtransaction", pro_tx_hash),
        "service": service,
        **roles,
        "owner_wif": rpc.call("dumpprivkey", roles["owner_address"], wallet=WALLET),
        "voting_wif": rpc.call("dumpprivkey", roles["voting_address"], wallet=WALLET),
    }
    entry.update(extra)
    entry["protx_info"] = rpc.call("protx", "info", pro_tx_hash)
    return entry


def platform_args(platform):
    return [platform["platform_node_id"], platform["platform_p2p_port"], platform["platform_http_port"]]


def fund_new(rpc, miner, kind, evo, service, operator_reward):
    roles = new_roles(rpc, operator_reward)
    collateral_address = new_address(rpc)
    platform = evo_platform() if evo else {}
    fund = funded_address(rpc, miner, (EVO_COLLATERAL if evo else COLLATERAL) + 1)
    args = [collateral_address, [service], roles["owner_address"], roles["operator_public"],
            roles["voting_address"], roles["operator_reward"], roles["payout_address"]]
    if evo:
        args += platform_args(platform)
    args.append(fund)
    pro_tx_hash = rpc.call("protx", "register_fund_evo" if evo else "register_fund", *args, wallet=WALLET)
    return case(rpc, kind, evo, service, roles, pro_tx_hash, miner,
                {"collateral_address": collateral_address, **platform})


def existing(rpc, miner, kind, evo, service, operator_reward):
    roles = new_roles(rpc, operator_reward)
    collateral = existing_collateral(rpc, miner, EVO_COLLATERAL if evo else COLLATERAL)
    platform = evo_platform() if evo else {}
    fund = funded_address(rpc, miner, 1)
    args = [collateral["collateral_hash"], collateral["collateral_index"], [service],
            roles["owner_address"], roles["operator_public"], roles["voting_address"],
            roles["operator_reward"], roles["payout_address"]]
    if evo:
        args += platform_args(platform)
    args.append(fund)
    pro_tx_hash = rpc.call("protx", "register_evo" if evo else "register", *args, wallet=WALLET)
    return case(rpc, kind, evo, service, roles, pro_tx_hash, miner, {**collateral, **platform})


def prepared(rpc, miner, kind, evo, service, operator_reward):
    roles = new_roles(rpc, operator_reward)
    collateral = existing_collateral(rpc, miner, EVO_COLLATERAL if evo else COLLATERAL)
    platform = evo_platform() if evo else {}
    fund = funded_address(rpc, miner, 1)
    args = [collateral["collateral_hash"], collateral["collateral_index"], [service],
            roles["owner_address"], roles["operator_public"], roles["voting_address"],
            roles["operator_reward"], roles["payout_address"]]
    if evo:
        args += platform_args(platform)
    args.append(fund)
    prep = rpc.call("protx", "register_prepare_evo" if evo else "register_prepare", *args, wallet=WALLET)
    if prep["collateralAddress"] != collateral["collateral_address"]:
        raise RuntimeError("register_prepare names another collateral address")
    signature = rpc.call("signmessage", prep["collateralAddress"], prep["signMessage"], wallet=WALLET)
    pro_tx_hash = rpc.call("protx", "register_submit", prep["tx"], signature, wallet=WALLET)
    return case(rpc, kind, evo, service, roles, pro_tx_hash, miner, {
        **collateral,
        **platform,
        "prepared_tx_hex": prep["tx"],
        "sign_address": prep["collateralAddress"],
        "sign_message": prep["signMessage"],
        "signature_b64": signature,
        "signature_hex": base64.b64decode(signature).hex(),
    })


def update_registrar(rpc, miner, kind, reg, operator, voting, payout):
    """`protx update_registrar` ("" keeps a field) on a registered
    masternode, paying its fee from one fresh single-UTXO address."""
    pro_tx_hash = reg["pro_tx_hash"]
    before = rpc.call("protx", "info", pro_tx_hash)
    fee = funded_address(rpc, miner, 1)
    txid = rpc.call("protx", "update_registrar", pro_tx_hash, operator, voting, payout, fee, wallet=WALLET)
    mine(rpc, 1, miner)
    after = rpc.call("protx", "info", pro_tx_hash)
    return {
        "kind": kind,
        "masternode_type": reg["masternode_type"],
        "pro_tx_hash": pro_tx_hash,
        "update_tx_hash": txid,
        "tx_hex": rpc.call("getrawtransaction", txid),
        "operator_public": operator or reg["operator_public"],
        "voting_address": voting or reg["voting_address"],
        "payout_address": payout or reg["payout_address"],
        "owner_wif": reg["owner_wif"],
        "protx_info_before": before,
        "protx_info_after": after,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    info = rpc.call("getnetworkinfo")
    source = f"regtest dashd {info.get('buildversion', info['subversion'])}, gen_protx_vectors.py"

    if WALLET not in rpc.call("listwallets"):
        # A legacy wallet: dumpprivkey hands the test the owner, voting and
        # collateral keys.
        rpc.call("createwallet", WALLET, False, False, "", False, False, False)
    miner = new_address(rpc)
    if rpc.call("getblockcount") < 600:
        mine(rpc, 600 - rpc.call("getblockcount"), miner)
    finish_mnsync(rpc)

    cases = [
        fund_new(rpc, miner, "fund_new_regular", False, "127.0.0.1:19899", "0"),
        existing(rpc, miner, "existing_regular", False, "127.0.0.2:19899", "0"),
        prepared(rpc, miner, "prepared_regular", False, "127.0.0.4:19899", "12.34"),
        fund_new(rpc, miner, "fund_new_evo", True, "127.0.0.3:19899", "0"),
        existing(rpc, miner, "existing_evo", True, "127.0.0.5:19899", "0"),
        prepared(rpc, miner, "prepared_evo", True, "127.0.0.6:19899", "0"),
    ]
    write_json(os.path.join(TESTDATA, "protx_registration.json"), {
        "source": source,
        "network": "regtest",
        "cases": cases,
    })

    by_kind = {c["kind"]: c for c in cases}
    updates = [
        # A new voting address only: "" keeps the operator key and payout.
        update_registrar(rpc, miner, "voting_regular", by_kind["existing_regular"],
                         "", new_address(rpc), ""),
        update_registrar(rpc, miner, "payout_regular", by_kind["fund_new_regular"],
                         "", "", new_address(rpc)),
        update_registrar(rpc, miner, "operator_regular", by_kind["prepared_regular"],
                         operator_public(rpc), "", ""),
        update_registrar(rpc, miner, "all_evo", by_kind["fund_new_evo"],
                         operator_public(rpc), new_address(rpc), new_address(rpc)),
    ]
    write_json(os.path.join(TESTDATA, "protx_update_registrar.json"), {
        "source": source,
        "network": "regtest",
        "cases": updates,
    })


if __name__ == "__main__":
    main()
