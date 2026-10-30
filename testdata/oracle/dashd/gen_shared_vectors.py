#!/usr/bin/env python3
"""Writes the MG-04b shared-masternode vectors from a v24 regtest dashd
(v24.0.0-rc.3), through dashd's own shared RPCs only:

- testdata/shared_registration.json: two shared registrations.
  A has three shares (400/300/300 DASH, one P2SH refund, one share with no
  reward, one whose reward is its refund written out), an early period and
  penalty. B has two shares (500/500), no early period (so `warning` is
  set) and no Core address. For each: the funding template
  (`createrawtransaction`, change outputs only), the `shared_register_prepare`
  arguments and result, every party's `shared_sign` result, the
  `shared_combine` output, the transaction after every funding input is
  signed, `testmempoolaccept` on it and on mutated join signatures, and
  `protx info` after one block.
- testdata/shared_maintenance.json: on A, `shared_update_share` (a new
  reward, and the refund written out as the reward, MG-04b D31),
  `shared_update_registrar_prepare` + `shared_sign` + `shared_combine` (new
  operator and voting keys), unilateral `shared_dissolve` standbys
  (payPenalty true and false) and `shared_dissolve_prepare` + `shared_sign` +
  `shared_combine`. Each has the transaction, dashd's sign hash where an RPC
  returns one, `testmempoolaccept` at the height it was asked (and mutated
  variants), and `protx info` at that time. The zero-penalty standby is
  tested again after the early period and then mined.

The regtest keys in these files belong to throwaway regtest wallets; the
operator BLS secrets are not written out.

Usage (a fresh regtest dashd with v24 activated by miners):
  dashd -regtest -datadir=$DIR -daemon -listen=0 -rpcport=29681 -port=29682 -rpcuser=u -rpcpassword=p \\
        -fallbackfee=0.00001 -txindex=1 -vbparams=v24:0:999999999999:0:10:8:6:5:0
  python3 testdata/oracle/dashd/gen_shared_vectors.py --rpc http://u:p@127.0.0.1:29681

With --dissolve-shapes (on another fresh dashd) it writes only
testdata/shared_dissolve_shapes.json: a registration whose early penalty is
its smallest share less 50,000 duffs, and unilateral `shared_dissolve`
standbys in the shapes the files above lack (MG-04b 2a review r1 n-3): the
actor holding the last share, the actor's output omitted (penalty and fee
take its whole share), and the actor's output left dust.
"""
import argparse
import base64
import os
import sys
from decimal import Decimal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_vectors import TESTDATA, Rpc, write_json  # noqa: E402

COIN = 100_000_000
MINER = "miner"
PARTIES = ["s0", "s1", "s2"]
# secp256k1's order: a high-S twin of a signature is (r, N - s).
N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141


def mine(rpc, n):
    return rpc.call("generatetoaddress", n, rpc.call("getnewaddress", wallet=MINER))


def height(rpc):
    return rpc.call("getblockcount")


def v24_active(rpc):
    return rpc.call("getdeploymentinfo")["deployments"]["v24"]["active"]


def finish_mnsync(rpc):
    for _ in range(20):
        if rpc.call("mnsync", "status")["IsSynced"]:
            return
        rpc.call("mnsync", "next")
    raise RuntimeError("mnsync did not finish")


def addr(rpc, wallet):
    return rpc.call("getnewaddress", wallet=wallet)


def wif(rpc, wallet, address):
    return rpc.call("dumpprivkey", address, wallet=wallet)


def coin(rpc, wallet, dash):
    """One confirmed `dash` output to a fresh address of `wallet`."""
    address = addr(rpc, wallet)
    txid = rpc.call("sendtoaddress", address, str(dash), wallet=MINER)
    mine(rpc, 1)
    tx = rpc.call("getrawtransaction", txid, True)
    out = next(o for o in tx["vout"] if o["scriptPubKey"].get("address") == address)
    return {"txid": txid, "vout": out["n"], "address": address, "amount": int(Decimal(str(out["value"])) * COIN)}


def accept(rpc, tx_hex):
    """`testmempoolaccept` at the current tip: the verdict and Core's reason."""
    r = rpc.call("testmempoolaccept", [tx_hex])[0]
    out = {"tip_height": height(rpc), "allowed": r["allowed"]}
    if not r["allowed"]:
        out["reject_reason"] = r["reject-reason"]
    return out


def high_s(sig):
    """The same signature with S = N - S and the recovery id flipped: valid
    ECDSA, refused by `VerifyHashCanonical`."""
    s = int.from_bytes(sig[33:65], "big")
    header = 27 + ((sig[0] - 27) & 4) + (((sig[0] - 27) & 3) ^ 1)
    return bytes([header]) + sig[1:33] + (N - s).to_bytes(32, "big")


def replace_once(tx_hex, old, new):
    raw = bytes.fromhex(tx_hex)
    assert raw.count(old) == 1, "the signature occurs once"
    return raw.replace(old, new).hex()


def sig_mutations(rpc, tx_hex, sigs):
    """`testmempoolaccept` on the transaction with its share signatures
    swapped (when there are two), the first one high-S, and the last zeroed.
    The bytes are replaced in place, so any input signature over the old
    bytes is stale too: Core's reason says which check ran first."""
    cases = []
    if len(sigs) > 1:
        raw = bytes.fromhex(tx_hex)
        a, b = sigs[0], sigs[1]
        i, j = raw.index(a), raw.index(b)
        swapped = bytearray(raw)
        swapped[i:i + 65], swapped[j:j + 65] = b, a
        cases.append(("swap_0_1", swapped.hex()))
    cases.append(("high_s_0", replace_once(tx_hex, sigs[0], high_s(sigs[0]))))
    cases.append(("zero_last", replace_once(tx_hex, sigs[-1], bytes(65))))
    return [{"mutation": name, "tx_hex": h, **accept(rpc, h)} for name, h in cases]


def script_of(rpc, address):
    return bytes.fromhex(rpc.call("validateaddress", address)["scriptPubKey"])


def key_id_of(rpc, address):
    script = script_of(rpc, address)
    assert len(script) == 25 and script[:3] == b"\x76\xa9\x14", "P2PKH"
    return script[3:23]


def payload_mutations(rpc, tx_hex, replacements):
    """`testmempoolaccept` on the transaction with one field's bytes
    replaced by others of the same length (`(name, old, new)`): Core runs
    its payload checks before the inputs' scripts, so the reason is the
    payload's even though the input signatures are stale."""
    cases = [(name, replace_once(tx_hex, old, new)) for name, old, new in replacements]
    return [{"mutation": name, "tx_hex": h, **accept(rpc, h)} for name, h in cases]


def output_bytes(rpc, tx_hex, n):
    """Output `n` as serialized: value (8 bytes LE), script length, script."""
    out = rpc.call("decoderawtransaction", tx_hex)["vout"][n]
    script = bytes.fromhex(out["scriptPubKey"]["hex"])
    value = int(Decimal(str(out["value"])) * COIN)
    return value.to_bytes(8, "little") + bytes([len(script)]) + script, value, script


def output_mutations(rpc, tx_hex):
    """A dissolution with its first output paying one duff less, and with
    its first two outputs swapped."""
    raw = bytes.fromhex(tx_hex)
    first, value, script = output_bytes(rpc, tx_hex, 0)
    second, _, _ = output_bytes(rpc, tx_hex, 1)
    less = (value - 1).to_bytes(8, "little") + first[8:]
    i = raw.index(first + second)
    swapped = raw[:i] + second + first + raw[i + len(first) + len(second):]
    return payload_mutations(rpc, tx_hex, [("first_output_less", first, less)]) + [
        {"mutation": "outputs_swapped", "tx_hex": swapped.hex(), **accept(rpc, swapped.hex())}]


def sign_all(rpc, tx_hex):
    """`protx shared_sign` on every party's wallet, and the signature list
    `shared_combine` takes, in share order."""
    results = {}
    entries = []
    for w in PARTIES:
        try:
            r = rpc.call("protx", "shared_sign", tx_hex, wallet=w)
        except RuntimeError as e:
            results[w] = {"error": str(e)}
            continue
        results[w] = r
        entries += r["signatures"]
    entries.sort(key=lambda e: e["shareIndex"])
    return results, entries


def sig_bytes(entries):
    return [base64.b64decode(e["signature"]) for e in entries]


def party(rpc, wallet, amount, refund="p2pkh", reward="other"):
    """A share: its owner key, refund and reward, and a funding coin of
    `amount` + 1 DASH."""
    owner = addr(rpc, wallet)
    if refund == "p2sh":
        keys = [rpc.call("getaddressinfo", addr(rpc, wallet), wallet=wallet)["pubkey"] for _ in range(2)]
        refund_address = rpc.call("addmultisigaddress", 1, keys, wallet=wallet)["address"]
    else:
        refund_address = addr(rpc, wallet)
    share = {"amount": amount, "refundAddress": refund_address, "ownerAddress": owner}
    if reward == "other":
        share["rewardAddress"] = addr(rpc, wallet)
    elif reward == "refund":
        share["rewardAddress"] = refund_address
    return {
        "wallet": wallet,
        "share": share,
        "owner_wif": wif(rpc, wallet, owner),
        "funding_coin": coin(rpc, wallet, Decimal(amount) / COIN + 1),
        "change_address": addr(rpc, wallet),
    }


def register(rpc, name, parties, service, voting, operator_reward, early_blocks, early_penalty, fee):
    """A shared registration through dashd's RPCs (rpc.md §10.1): template,
    prepare, every owner signs, combine, every funding input signs in turn,
    broadcast, one block. Party 0 pays `fee` from its change."""
    inputs = [{"txid": p["funding_coin"]["txid"], "vout": p["funding_coin"]["vout"]} for p in parties]
    change = {}
    for i, p in enumerate(parties):
        left = p["funding_coin"]["amount"] - p["share"]["amount"] - (fee if i == 0 else 0)
        change[p["change_address"]] = str(Decimal(left) / COIN)
    funding = rpc.call("createrawtransaction", inputs, change)
    shares = [p["share"] for p in parties]
    operator = rpc.call("bls", "generate")["public"]
    args = [funding, shares, service, operator, voting, operator_reward, early_blocks, early_penalty]
    prep = rpc.call("protx", "shared_register_prepare", *args)
    signed, entries = sign_all(rpc, prep["tx"])
    combined = rpc.call("protx", "shared_combine", prep["tx"], entries)
    funded = combined
    for p in parties:
        funded = rpc.call("signrawtransactionwithwallet", funded, wallet=p["wallet"])["hex"]
    sigs = sig_bytes(entries)
    case = {
        "name": name,
        "parties": parties,
        "funding_tx_hex": funding,
        "prepare_args": args,
        "operator_public": operator,
        "voting_address": voting,
        "prepare": prep,
        "shared_sign": signed,
        "signatures": entries,
        "combined_tx_hex": combined,
        "tx_hex": funded,
        "accept": accept(rpc, funded),
        "mutations": sig_mutations(rpc, funded, sigs),
    }
    pro_tx_hash = rpc.call("sendrawtransaction", funded)
    mine(rpc, 1)
    case["pro_tx_hash"] = pro_tx_hash
    case["protx_info"] = rpc.call("protx", "info", pro_tx_hash)
    return case


def fee_address(rpc, wallet):
    return coin(rpc, wallet, 1)["address"]


def update_share(rpc, name, reg, index, reward_address):
    p = reg["parties"][index]
    fee = fee_address(rpc, p["wallet"])
    before = rpc.call("protx", "info", reg["pro_tx_hash"])
    tx = rpc.call("protx", "shared_update_share", reg["pro_tx_hash"], index, reward_address, fee, False,
                  wallet=p["wallet"])
    sig = bytes.fromhex(tx)[-65:]
    case = {
        "name": name,
        "kind": "update_share",
        "share_index": index,
        "reward_address": reward_address,
        "fee_address": fee,
        "tx_hex": tx,
        "protx_info": before,
        "accept": accept(rpc, tx),
        "mutations": sig_mutations(rpc, tx, [sig]) + payload_mutations(rpc, tx, [
            ("reward_is_voting", script_of(rpc, reward_address),
             script_of(rpc, before["state"]["votingAddress"])),
            ("reward_is_owner", script_of(rpc, reward_address),
             script_of(rpc, reg["parties"][2]["share"]["ownerAddress"])),
        ]),
    }
    rpc.call("sendrawtransaction", tx)
    mine(rpc, 1)
    case["protx_info_after"] = rpc.call("protx", "info", reg["pro_tx_hash"])
    return case


def update_registrar(rpc, name, reg, operator, voting):
    p = reg["parties"][0]
    fee = fee_address(rpc, p["wallet"])
    before = rpc.call("protx", "info", reg["pro_tx_hash"])
    prep = rpc.call("protx", "shared_update_registrar_prepare", reg["pro_tx_hash"], operator, voting, fee,
                    wallet=p["wallet"])
    signed, entries = sign_all(rpc, prep["tx"])
    tx = rpc.call("protx", "shared_combine", prep["tx"], entries, False, wallet=p["wallet"])
    case = {
        "name": name,
        "kind": "update_registrar",
        "operator_public": operator,
        "voting_address": voting,
        "fee_address": fee,
        "prepare": prep,
        "shared_sign": signed,
        "signatures": entries,
        "tx_hex": tx,
        "protx_info": before,
        "accept": accept(rpc, tx),
        "mutations": sig_mutations(rpc, tx, sig_bytes(entries)) + payload_mutations(rpc, tx, [
            ("voting_is_refund", key_id_of(rpc, voting),
             key_id_of(rpc, reg["parties"][0]["share"]["refundAddress"])),
            # Share 0's reward at registration, replaced by its refund since
            # (`reward_refund`): no longer a payee.
            ("voting_is_old_reward", key_id_of(rpc, voting),
             key_id_of(rpc, reg["parties"][0]["share"]["rewardAddress"])),
        ]),
    }
    rpc.call("sendrawtransaction", tx)
    mine(rpc, 1)
    case["protx_info_after"] = rpc.call("protx", "info", reg["pro_tx_hash"])
    return case


def standby(rpc, name, reg, actor, fee, pay_penalty):
    p = reg["parties"][actor]
    tx = rpc.call("protx", "shared_dissolve", reg["pro_tx_hash"], actor, fee, False, pay_penalty,
                  wallet=p["wallet"])
    sig = bytes.fromhex(tx)[-65:]
    return {
        "name": name,
        "kind": "dissolve_unilateral",
        "actor_index": actor,
        "fee": fee,
        "pay_penalty": pay_penalty,
        "tx_hex": tx,
        "protx_info": rpc.call("protx", "info", reg["pro_tx_hash"]),
        "accept": accept(rpc, tx),
        "mutations": sig_mutations(rpc, tx, [sig]) + output_mutations(rpc, tx),
    }


def unanimous(rpc, name, reg, actor, fee):
    prep = rpc.call("protx", "shared_dissolve_prepare", reg["pro_tx_hash"], actor, fee)
    signed, entries = sign_all(rpc, prep["tx"])
    tx = rpc.call("protx", "shared_combine", prep["tx"], entries, False)
    return {
        "name": name,
        "kind": "dissolve_unanimous",
        "actor_index": actor,
        "fee": fee,
        "prepare": prep,
        "shared_sign": signed,
        "signatures": entries,
        "tx_hex": tx,
        "protx_info": rpc.call("protx", "info", reg["pro_tx_hash"]),
        "accept": accept(rpc, tx),
        "mutations": sig_mutations(rpc, tx, sig_bytes(entries)) + output_mutations(rpc, tx),
    }


def setup(rpc):
    """The wallets, v24 active and mnsync done; returns the height."""
    for w in [MINER] + PARTIES:
        if w not in rpc.call("listwallets"):
            # Legacy wallets: dumpprivkey hands the test the owner keys.
            rpc.call("createwallet", w, False, False, "", False, False, False)
    if height(rpc) < 600:
        mine(rpc, 600 - height(rpc))
    while not v24_active(rpc):
        mine(rpc, 10)
    finish_mnsync(rpc)
    return height(rpc)


def dissolve_shapes(rpc, source):
    """shared_dissolve_shapes.json: three shares 400/300/300, the early
    penalty 300 DASH less 50,000 duffs, and early standbys paying it."""
    parties = [party(rpc, w, amount) for w, amount in zip(PARTIES, [400 * COIN, 300 * COIN, 300 * COIN])]
    reg = register(rpc, "penalty_near_a_share", parties, "127.0.0.1:19903", addr(rpc, "s0"), "0",
                   20, 300 * COIN - 50_000, 30_000)
    cases = [
        # The actor holds the last share: 40,000 duffs back, last.
        standby(rpc, "last_actor_penalty", reg, 2, 10_000, True),
        # The penalty and fee take the whole share: no actor output.
        standby(rpc, "last_actor_omitted", reg, 2, 50_000, True),
        standby(rpc, "middle_actor_omitted", reg, 1, 50_000, True),
        # 100 duffs back: valid, but dust to relay (MG-04b D83).
        standby(rpc, "last_actor_dust", reg, 2, 49_900, True),
    ]
    write_json(os.path.join(TESTDATA, "shared_dissolve_shapes.json"), {
        "source": source,
        "network": "regtest",
        "registration": reg,
        "cases": cases,
    })


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--dissolve-shapes", action="store_true")
    args = ap.parse_args()
    rpc = Rpc(args.rpc)
    info = rpc.call("getnetworkinfo")
    source = f"regtest dashd {info.get('buildversion', info['subversion'])}, gen_shared_vectors.py"

    activation = setup(rpc)
    if args.dissolve_shapes:
        dissolve_shapes(rpc, source + " --dissolve-shapes")
        return

    a_parties = [
        party(rpc, "s0", 400 * COIN),
        party(rpc, "s1", 300 * COIN, refund="p2sh", reward="none"),
        party(rpc, "s2", 300 * COIN, reward="refund"),
    ]
    a = register(rpc, "three_shares_early", a_parties, "127.0.0.1:19901", addr(rpc, "s0"), "5.25",
                 20, 1 * COIN, 30_000)
    b_parties = [
        party(rpc, "s0", 500 * COIN, reward="none"),
        party(rpc, "s1", 500 * COIN, reward="none"),
    ]
    b = register(rpc, "two_shares_no_early", b_parties, "", addr(rpc, "s1"), "0", 0, 0, 20_000)
    write_json(os.path.join(TESTDATA, "shared_registration.json"), {
        "source": source,
        "network": "regtest",
        "v24_active_at_or_before": activation,
        "cases": [a, b],
    })

    early = [
        standby(rpc, "standby_penalty_early", a, 1, 100_000, True),
        standby(rpc, "standby_no_penalty_early", a, 1, 100_000, False),
        unanimous(rpc, "unanimous_early", a, 2, 50_000),
    ]
    updates = [
        update_share(rpc, "reward_new", a, 1, addr(rpc, "s1")),
        update_share(rpc, "reward_refund", a, 0, a["parties"][0]["share"]["refundAddress"]),
        update_registrar(rpc, "operator_and_voting", a, rpc.call("bls", "generate")["public"],
                         addr(rpc, "s2")),
    ]
    # The standby signed in the early period is valid from the first block
    # past it (rpc.md §4.4), and forever after.
    registered = a["protx_info"]["state"]["registeredHeight"]
    while height(rpc) + 1 < registered + 20:
        mine(rpc, 1)
    late = dict(early[1], name="standby_no_penalty_late", accept=accept(rpc, early[1]["tx_hex"]))
    late["protx_info"] = rpc.call("protx", "info", a["pro_tx_hash"])
    del late["mutations"]
    dissolved = rpc.call("sendrawtransaction", late["tx_hex"])
    mine(rpc, 1)
    late["mined_txid"] = dissolved
    late["listed_after"] = any(m["proTxHash"] == a["pro_tx_hash"]
                               for m in rpc.call("protx", "list", "registered", True))
    b_standby = standby(rpc, "standby_no_early", b, 0, 1_000, False)
    write_json(os.path.join(TESTDATA, "shared_maintenance.json"), {
        "source": source,
        "network": "regtest",
        "pro_tx_hash": a["pro_tx_hash"],
        "cases": early + updates + [late, b_standby],
    })


if __name__ == "__main__":
    main()
