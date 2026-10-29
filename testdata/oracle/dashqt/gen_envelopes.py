#!/usr/bin/env python3
"""Writes testdata/shared_envelopes.json (MG-04b M9) with dash-qt's own
session code: Dash Core v24.0.0-rc.3's src/qt/mnsharesession.cpp, linked
unmodified into mnshare_oracle (build.sh).

Usage: gen_envelopes.py --oracle <work dir>/mnshare_oracle

1. `cases`: the two registrations of testdata/shared_registration.json
   (dashd's `shared_register_prepare`, `shared_sign`, `shared_combine` and
   signed funding) replayed through MnShareSession as the creation dialog
   does; toJsonString() and fingerprint() after every step.
2. `corpus`: texts made from those envelopes, each with one change, and
   fromJson's verdict on each: its error, or whether the fingerprint matched
   and toJsonString() of the session it read. `wallet_refuses` marks a text
   dash-qt reads but the wallet refuses on purpose, with the D-row.

dw-protx's tests/oracle_envelopes.rs checks shared::session against both.
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
TESTDATA = os.path.dirname(os.path.dirname(HERE))
SOURCE = os.path.join(TESTDATA, "shared_registration.json")
OUT = os.path.join(TESTDATA, "shared_envelopes.json")
LABELS = ["Alice", "Bob", "Carol"]
# Envelope fields the coordinator's dialog sets; one case leaves them empty.
SIDE_FIELDS = [("s0", "Alice", "Alice"), ("", "", "")]
# The order the signatures arrive in; addSignature keeps them ascending.
SIGNATURE_ORDER = [[2, 0, 1], [1, 0]]

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
REGTEST_P2PKH, REGTEST_P2SH = 140, 19


def script_of(address):
    n = 0
    for ch in address:
        n = n * 58 + B58.index(ch)
    raw = n.to_bytes(25, "big")
    body, check = raw[:21], raw[21:]
    assert hashlib.sha256(hashlib.sha256(body).digest()).digest()[:4] == check, address
    if body[0] == REGTEST_P2PKH:
        return "76a914" + body[1:].hex() + "88ac"
    assert body[0] == REGTEST_P2SH, address
    return "a914" + body[1:].hex() + "87"


def parse_tx(hex_tx):
    """A plain (type 0) transaction: inputs and outputs."""
    b = bytes.fromhex(hex_tx)
    pos = 0

    def take(n):
        nonlocal pos
        pos += n
        return b[pos - n:pos]

    def varint():
        first = take(1)[0]
        return first if first < 0xFD else int.from_bytes(take({0xFD: 2, 0xFE: 4, 0xFF: 8}[first]), "little")

    assert int.from_bytes(take(4), "little") == 2
    vin = []
    for _ in range(varint()):
        txid = take(32)[::-1].hex()
        vout = int.from_bytes(take(4), "little")
        take(varint())
        vin.append({"txid": txid, "vout": vout, "sequence": int.from_bytes(take(4), "little")})
    vout = []
    for _ in range(varint()):
        value = int.from_bytes(take(8), "little")
        vout.append({"value": value, "script": take(varint()).hex()})
    assert take(4) == bytes(4) and pos == len(b)
    return vin, vout


def case_inputs(c, i):
    args = c["prepare_args"]
    vin, vout = parse_tx(args[0])
    parties = c["parties"]
    assert len(vin) == len(vout) == len(parties)
    shares, contributions = [], []
    for label, party, inp, out in zip(LABELS, parties, vin, vout):
        s = party["share"]
        shares.append({
            "label": label,
            "amount": s["amount"],
            "ownerAddress": s["ownerAddress"],
            "refundAddress": s["refundAddress"],
            "rewardAddress": s.get("rewardAddress", ""),
        })
        coin = party["funding_coin"]
        assert (inp["txid"], inp["vout"]) == (coin["txid"], coin["vout"])
        assert out["script"] == script_of(party["change_address"])
        contributions.append({
            "label": label,
            "inputs": [inp],
            "change": {"address": party["change_address"], "amount": out["value"]},
        })
    prepare_wallet, secret_holder, coordinator = SIDE_FIELDS[i]
    by_index = {s["shareIndex"]: s for s in c["signatures"]}
    return {
        "name": c["name"],
        "sessionId": hashlib.sha256(f"MG-04b M9 {c['name']}".encode()).hexdigest(),
        "shares": shares,
        "terms": {
            "coreP2PAddrs": args[2],
            "operatorPubKey": args[3],
            "votingAddress": args[4],
            "operatorReward": round(float(args[5]) * 100),
            "earlyPeriodBlocks": args[6],
            "earlyPenalty": args[7],
        },
        "prepareWallet": prepare_wallet,
        "operatorSecretHolder": secret_holder,
        "coordinatorLabel": coordinator,
        "contributions": contributions,
        "protx": c["prepare"]["tx"],
        "consentHash": c["prepare"]["consentHash"],
        "collateralIndex": c["prepare"]["collateralIndex"],
        "signatures": [by_index[k] for k in SIGNATURE_ORDER[i]],
        "combinedTx": c["combined_tx_hex"],
        "fundingSignedTx": c["tx_hex"],
    }


def run(oracle, mode, data):
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(data, f)
    try:
        out = subprocess.run([oracle, mode, f.name], check=True, capture_output=True, text=True).stdout
    finally:
        os.unlink(f.name)
    return json.loads(out)


# --- the corpus -------------------------------------------------------------

def raw(token):
    """A JSON number written as is (Python would normalize it)."""
    return f"@@RAW:{token}@@"


def dump(doc):
    text = json.dumps(doc, ensure_ascii=False, separators=(",", ":"))
    while "\"@@RAW:" in text:
        start = text.index("\"@@RAW:")
        end = text.index("@@\"", start)
        text = text[:start] + text[start + 7:end] + text[end + 3:]
    return text


def corpus(steps, other_steps):
    base = {s["step"]: s["text"] for s in steps}
    other = {s["step"]: s["text"] for s in other_steps}
    entries = []

    def add(name, step, change=None, text=None, wallet_refuses=None):
        if text is None:
            doc = json.loads(base[step])
            change(doc)
            text = dump(doc)
        entry = {"name": name, "base": step, "text": text}
        if wallet_refuses:
            entry["wallet_refuses"] = wallet_refuses
        entries.append(entry)

    def setter(*path_value):
        *path, value = path_value

        def change(doc):
            node = doc
            for key in path[:-1]:
                node = node[key]
            node[path[-1]] = value
        return change

    def deleter(*path):
        def change(doc):
            node = doc
            for key in path[:-1]:
                node = node[key]
            del node[path[-1]]
        return change

    draft, frozen = "contribution Carol", "freeze"
    signing, combined, funded, broadcast = "signature 0", "combine", "sign funding", "broadcast"

    # Unchanged, and the envelope as a whole.
    for step in base:
        add(f"as written: {step}", step, text=base[step])
    add("not JSON", draft, text=base[draft][:-2])
    add("an array", draft, text="[" + base[draft] + "]")
    add("a string", draft, text=json.dumps(base[draft]))
    add("type missing", draft, deleter("type"))
    add("type of the signature envelope", draft, setter("type", "dash-shared-mn-sigs"))
    add("type a number", draft, setter("type", 1))
    add("version 2", draft, setter("version", 2))
    add("version as a string", draft, setter("version", "1"))
    add("version 1.0", draft, setter("version", raw("1.0")))
    add("version 1e0", draft, setter("version", raw("1e0")))
    add("version past int", draft, setter("version", raw("4294967297")))
    add("network main", draft, setter("network", "main"))
    add("network missing", draft, deleter("network"))
    add("session id empty", draft, setter("sessionId", ""))
    add("session id a number", draft, setter("sessionId", 7))
    add("session id upper-case", draft, lambda d: d.update(sessionId=d["sessionId"].upper()),
        wallet_refuses="D28")
    add("session id short", draft, setter("sessionId", "abc"), wallet_refuses="D28")
    add("revision 0", draft, setter("revision", 0))
    add("revision -1", draft, setter("revision", -1))
    add("revision 2.5", draft, setter("revision", raw("2.5")))
    add("revision as a string", draft, setter("revision", "4"))
    add("revision past int", draft, setter("revision", raw("2147483648")))
    add("stage missing", draft, deleter("stage"))
    add("stage unknown", draft, setter("stage", "signed"))
    add("stage capitalized", draft, setter("stage", "Draft"))
    add("stage a number", draft, setter("stage", 0))
    add("stage repeated", draft, text=base[draft].replace('"stage": "draft",', '"stage": "draft",\n  "stage": "broadcast",', 1))
    add("fundingTx a number", draft, setter("fundingTx", 5))
    add("fundingTx missing", draft, deleter("fundingTx"))
    add("fingerprint stale", draft, setter("coordinatorLabel", "Mallory"))
    add("fingerprint lower-case", draft, lambda d: d.update(fingerprint=d["fingerprint"].lower()))
    add("fingerprint missing", draft, deleter("fingerprint"))
    add("fingerprint a number", draft, setter("fingerprint", 1))
    add("fingerprint not last", draft,
        lambda d: d.update({"fingerprint": d.pop("fingerprint"), **{k: d.pop(k) for k in list(d) if k != "fingerprint"}}))

    # Shares.
    add("shares not an array", draft, setter("shares", {}), wallet_refuses="D28")
    add("nine shares", draft, lambda d: d["shares"].extend([d["shares"][0]] * 6))
    add("eight shares", draft,
        lambda d: d["shares"].extend([dict(d["shares"][0], label=f"P{i}") for i in range(5)]))
    add("share not an object", draft, setter("shares", 0, "Alice"))
    add("share amount -1", draft, setter("shares", 0, "amount", -1))
    add("share amount past MAX_MONEY", draft, setter("shares", 0, "amount", 2_100_000_000_000_001))
    add("share amount MAX_MONEY", draft, setter("shares", 0, "amount", 2_100_000_000_000_000))
    add("share amount as a string", draft, setter("shares", 0, "amount", "40000000000"))
    add("share amount 4e10", draft, setter("shares", 0, "amount", raw("4e10")))
    add("share amount 400.5", draft, setter("shares", 0, "amount", raw("400.5")))
    add("share amount past int64", draft, setter("shares", 0, "amount", raw("9223372036854775808")))
    add("share owner missing", draft, deleter("shares", 0, "ownerAddress"))
    add("share owner a number", draft, setter("shares", 0, "ownerAddress", 1))
    add("share refund missing", draft, deleter("shares", 0, "refundAddress"))
    add("share reward missing", draft, deleter("shares", 0, "rewardAddress"))
    add("share reward a number", draft, setter("shares", 0, "rewardAddress", 1))
    add("share label missing", draft, deleter("shares", 0, "label"), wallet_refuses="D28")
    add("share label repeated", draft, setter("shares", 1, "label", "Alice"), wallet_refuses="D28")
    add("share label repeated with a space", draft, setter("shares", 1, "label", "Alice "), wallet_refuses="D28")
    add("share label blank", draft, setter("shares", 2, "label", " "), wallet_refuses="D28")

    # Terms.
    add("terms not an object", draft, setter("terms", []))
    add("terms missing", draft, deleter("terms"))
    add("operatorReward 5.25", draft, setter("terms", "operatorReward", raw("5.25")))
    add("operatorReward past int", draft, setter("terms", "operatorReward", raw("2147483648")))
    add("operatorReward -1", draft, setter("terms", "operatorReward", -1))
    add("operatorReward 10001", draft, setter("terms", "operatorReward", 10001))
    add("operatorReward as a string", draft, setter("terms", "operatorReward", "525"))
    add("earlyPeriodBlocks -1", draft, setter("terms", "earlyPeriodBlocks", -1))
    add("earlyPeriodBlocks 2^32", draft, setter("terms", "earlyPeriodBlocks", 2**32))
    add("earlyPeriodBlocks 2^32-1", draft, setter("terms", "earlyPeriodBlocks", 2**32 - 1))
    add("earlyPeriodBlocks 20.0", draft, setter("terms", "earlyPeriodBlocks", raw("20.0")))
    add("earlyPenalty -1", draft, setter("terms", "earlyPenalty", -1))
    add("earlyPenalty past MAX_MONEY", draft, setter("terms", "earlyPenalty", 2_100_000_000_000_001))
    add("coreP2PAddrs a number", draft, setter("terms", "coreP2PAddrs", 1))
    add("operatorPubKey missing", draft, deleter("terms", "operatorPubKey"))

    # Contributions.
    add("contributions not an array", draft, setter("contributions", "x"))
    add("nine contributions", draft, lambda d: d["contributions"].extend([d["contributions"][0]] * 6))
    add("contribution not an object", draft, setter("contributions", 0, []))
    add("contribution label missing", draft, deleter("contributions", 0, "label"))
    add("contribution label unknown", draft, setter("contributions", 0, "label", "Mallory"), wallet_refuses="D28")
    add("contribution label repeated", draft, setter("contributions", 1, "label", "Alice"), wallet_refuses="D28")
    add("contribution inputs missing", draft, deleter("contributions", 0, "inputs"))
    add("contribution inputs empty", draft, setter("contributions", 0, "inputs", []))
    add("contribution with 64 inputs", draft,
        lambda d: d["contributions"][0].update(inputs=[dict(d["contributions"][0]["inputs"][0], vout=v) for v in range(64)]))
    add("contribution with 65 inputs", draft,
        lambda d: d["contributions"][0].update(inputs=[dict(d["contributions"][0]["inputs"][0], vout=v) for v in range(65)]))
    add("input not an object", draft, setter("contributions", 0, "inputs", 0, 1))
    add("input vout -1", draft, setter("contributions", 0, "inputs", 0, "vout", -1))
    add("input vout 2^32", draft, setter("contributions", 0, "inputs", 0, "vout", 2**32))
    add("input vout 1.0", draft, setter("contributions", 0, "inputs", 0, "vout", raw("1.0")))
    add("input vout missing", draft, deleter("contributions", 0, "inputs", 0, "vout"))
    add("input sequence 2^32", draft, setter("contributions", 0, "inputs", 0, "sequence", 2**32))
    add("input sequence 0", draft, setter("contributions", 0, "inputs", 0, "sequence", 0))
    add("input sequence missing", draft, deleter("contributions", 0, "inputs", 0, "sequence"))
    add("input txid short", draft, lambda d: d["contributions"][0]["inputs"][0].update(txid="ab" * 31))
    add("input txid upper-case", draft,
        lambda d: d["contributions"][0]["inputs"][0].update(txid=d["contributions"][0]["inputs"][0]["txid"].upper()))
    add("input txid not hex", draft, lambda d: d["contributions"][0]["inputs"][0].update(txid="zz" * 32))
    add("input txid a number", draft, setter("contributions", 0, "inputs", 0, "txid", 1))
    add("change not an object", draft, setter("contributions", 0, "change", "x"))
    add("change missing", draft, deleter("contributions", 0, "change"))
    add("change address missing", draft, deleter("contributions", 0, "change", "address"))
    add("change address invalid", draft, setter("contributions", 0, "change", "address", "x"))
    add("change amount -1", draft, setter("contributions", 0, "change", "amount", -1))
    add("change amount 0", draft, setter("contributions", 0, "change", "amount", 0))
    add("change amount as a string", draft, setter("contributions", 0, "change", "amount", "1"))
    add("changeIndex as a string", draft, setter("contributions", 0, "changeIndex", "0"))
    add("changeIndex 0.5", draft, setter("contributions", 0, "changeIndex", raw("0.5")))
    add("changeIndex 7", draft, setter("contributions", 0, "changeIndex", 7))

    # The frozen fields, read at any stage.
    add("consentHash short", draft, setter("consentHash", "abc"))
    add("consentHash at draft", draft, setter("consentHash", "AB" * 32))
    add("consentHash a number", draft, setter("consentHash", 1))
    add("collateralIndex as a string", draft, setter("collateralIndex", "3"))
    add("collateralIndex 3.5", draft, setter("collateralIndex", raw("3.5")))
    add("protx a number", draft, setter("protx", 1))
    add("prepareWallet a number", draft, setter("prepareWallet", 1))
    add("operatorSecretHolder missing", draft, deleter("operatorSecretHolder"))
    add("coordinatorLabel a number", draft, setter("coordinatorLabel", 1))

    # Signature entries.
    add("sigs not an array", signing, setter("sigs", {}))
    add("sig not an object", signing, setter("sigs", 0, 1))
    add("sig index as a string", signing, setter("sigs", 0, "shareIndex", "0"))
    add("sig index 0.0", signing, setter("sigs", 0, "shareIndex", raw("0.0")))
    add("sig signature a number", signing, setter("sigs", 0, "signature", 1))
    add("sig repeated", signing, lambda d: d["sigs"].append(d["sigs"][0]))
    add("sig at draft", draft, setter("sigs", [{"shareIndex": 9, "signature": "AA=="}]))

    # Past draft: the frozen transaction.
    for step in (frozen, combined):
        add(f"{step}: protx empty", step, setter("protx", ""))
        add(f"{step}: consentHash empty", step, setter("consentHash", ""))
        add(f"{step}: consentHash upper-case", step, lambda d: d.update(consentHash=d["consentHash"].upper()))
        add(f"{step}: consentHash of another", step, setter("consentHash", "11" * 32))
        add(f"{step}: protx not hex", step, setter("protx", "zz"))
        add(f"{step}: protx the funding template", step, lambda d: d.update(protx=d["fundingTx"]))
        add(f"{step}: protx of the other case", step,
            lambda d: d.update(protx=json.loads(other[frozen])["protx"], consentHash=json.loads(other[frozen])["consentHash"]))
        add(f"{step}: back to draft", step, setter("stage", "draft"))
    add("frozen: share amount", frozen, lambda d: d["shares"][0].update(amount=d["shares"][0]["amount"] - 1))
    add("frozen: owners swapped", frozen,
        lambda d: (d["shares"][0].update(ownerAddress=d["shares"][1]["ownerAddress"]),
                   d["shares"][1].update(ownerAddress=json.loads(base[frozen])["shares"][0]["ownerAddress"])))
    add("frozen: refund changed", frozen,
        lambda d: d["shares"][0].update(refundAddress=d["shares"][2]["refundAddress"]))
    add("frozen: reward cleared", frozen, setter("shares", 0, "rewardAddress", ""))
    add("frozen: reward set to the refund", frozen,
        lambda d: d["shares"][1].update(rewardAddress=d["shares"][1]["refundAddress"]))
    add("frozen: share removed", frozen, lambda d: d["shares"].pop())
    add("frozen: share label changed", frozen, setter("shares", 0, "label", "Alicia"), wallet_refuses="D28")
    add("frozen: voting changed", frozen, lambda d: d["terms"].update(votingAddress=d["shares"][0]["ownerAddress"]))
    add("frozen: operator key upper-case", frozen,
        lambda d: d["terms"].update(operatorPubKey=d["terms"]["operatorPubKey"].upper()))
    add("frozen: operator key changed", frozen,
        lambda d: d["terms"].update(operatorPubKey=d["terms"]["operatorPubKey"][:-2] + "00"))
    add("frozen: operatorReward changed", frozen, setter("terms", "operatorReward", 0))
    add("frozen: earlyPeriodBlocks changed", frozen, setter("terms", "earlyPeriodBlocks", 21))
    add("frozen: earlyPenalty changed", frozen, setter("terms", "earlyPenalty", 0))
    add("frozen: change amount", frozen,
        lambda d: d["contributions"][0]["change"].update(amount=d["contributions"][0]["change"]["amount"] + 1))
    add("frozen: change removed", frozen, deleter("contributions", 2, "change"))
    add("frozen: contribution removed", frozen, lambda d: d["contributions"].pop())
    add("frozen: contributions reordered", frozen,
        lambda d: d.update(contributions=[d["contributions"][1], d["contributions"][0], d["contributions"][2]]))
    add("frozen: input sequence", frozen, setter("contributions", 0, "inputs", 0, "sequence", 0))
    add("frozen: input txid upper-case", frozen,
        lambda d: d["contributions"][0]["inputs"][0].update(txid=d["contributions"][0]["inputs"][0]["txid"].upper()))
    add("frozen: collateralIndex 0", frozen, setter("collateralIndex", 0))
    add("frozen: collateralIndex missing", frozen, deleter("collateralIndex"))
    add("frozen: fundingTx changed", frozen, setter("fundingTx", "00"))
    add("frozen: sig for share 3", frozen, setter("sigs", [{"shareIndex": 3, "signature": "AA=="}]))
    add("frozen: sig for share -1", frozen, setter("sigs", [{"shareIndex": -1, "signature": "AA=="}]))
    add("frozen: sig not checked yet", frozen, setter("sigs", [{"shareIndex": 0, "signature": "AA=="}]))
    add("signing: sigs reversed", signing, lambda d: d["sigs"].reverse())
    add("signing: as combined", signing, setter("stage", "combined"))

    # The service addresses: NetInfoMatches runs both sides through AddEntry.
    for name, addrs in [
        ("spaces around", " 127.0.0.1:19901 "),
        ("a trailing comma", "127.0.0.1:19901,"),
        ("a tab", "127.0.0.1:19901\t"),
        ("repeated", "127.0.0.1:19901,127.0.0.1:19901"),
        ("IPv4-mapped IPv6", "[::ffff:127.0.0.1]:19901"),
        ("IPv4 in brackets", "[127.0.0.1]:19901"),
        ("short IPv4", "127.1:19901"),
        ("hex IPv4", "0x7f.0.0.1:19901"),
        ("octal IPv4", "0177.0.0.1:19901"),
        ("zero-padded IPv4", "127.000.000.001:19901"),
        ("zero-padded port", "127.0.0.1:019901"),
        ("IPv4 as one number", "2130706433:19901"),
        ("IPv4 in three parts", "127.0.1:19901"),
        ("IPv4 in two parts past a byte", "127.256:19901"),
        ("octal 01", "127.0.0.01:19901"),
        ("octal 08", "127.0.0.08:19901"),
        ("IPv4 part past 255", "127.0.0.256:19901"),
        ("IPv4 with a trailing dot", "127.0.0.1.:19901"),
        ("IPv4 with an empty part", "127..0.1:19901"),
        ("IPv4 with five parts", "127.0.0.0.1:19901"),
        ("IPv4-mapped IPv6 in hex", "[::ffff:7f00:1]:19901"),
        ("IPv4-mapped IPv6 in full", "[0:0:0:0:0:ffff:127.0.0.1]:19901"),
        ("IPv4-compatible IPv6", "[::127.0.0.1]:19901"),
        ("IPv6 loopback", "[::1]:19901"),
        ("IPv6 without brackets", "::ffff:127.0.0.1:19901"),
        ("a plus-signed port", "127.0.0.1:+19901"),
        ("a minus-signed port", "127.0.0.1:-19901"),
        ("port 0", "127.0.0.1:0"),
        ("port past 65535", "127.0.0.1:84437"),
        ("brackets twice", "[[127.0.0.1]]:19901"),
        ("the port inside the brackets", "[127.0.0.1:19901]"),
        ("two ports", "127.0.0.1:19901:19901"),
        ("an empty host", ":19901"),
        ("no port", "127.0.0.1"),
        ("another port", "127.0.0.1:19902"),
        ("empty", ""),
        ("a host name", "localhost:19901"),
        ("a semicolon", "127.0.0.1:19901;"),
    ]:
        add(f"frozen: coreP2PAddrs {name}", frozen, setter("terms", "coreP2PAddrs", addrs))

    # Combined and later: every approval verified, every input signed.
    add("combined: sig removed", combined, lambda d: d["sigs"].pop())
    add("combined: sigs swapped", combined,
        lambda d: (d["sigs"][0].update(signature=d["sigs"][1]["signature"]),
                   d["sigs"][1].update(signature=json.loads(base[combined])["sigs"][0]["signature"])))
    add("combined: sig not base64", combined, setter("sigs", 0, "signature", "!!"))
    add("combined: sig for share 3 too", combined, lambda d: d["sigs"].append({"shareIndex": 3, "signature": "AA=="}))
    add("combined: sigs reversed", combined, lambda d: d["sigs"].reverse())
    add("combined: protx with signed inputs", combined, lambda d: d.update(protx=json.loads(base[funded])["protx"]))
    add("combined: protx before combining", combined, lambda d: d.update(protx=json.loads(base[frozen])["protx"]))
    add("sign funding: protx with unsigned inputs", funded, lambda d: d.update(protx=json.loads(base[combined])["protx"]))
    add("broadcast: protx with unsigned inputs", broadcast, lambda d: d.update(protx=json.loads(base[combined])["protx"]))
    add("broadcast: network test", broadcast, setter("network", "test"))

    names = [e["name"] for e in entries]
    assert len(names) == len(set(names)), "corpus names must be unique"
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--oracle", required=True, help="mnshare_oracle built by build.sh")
    args = parser.parse_args()

    with open(SOURCE) as f:
        source = json.load(f)
    assert source["network"] == "regtest" and "v24.0.0-rc.3" in source["source"]
    inputs = [case_inputs(c, i) for i, c in enumerate(source["cases"])]
    stages = run(args.oracle, "stages", inputs)
    entries = corpus(stages[0]["steps"], stages[1]["steps"])
    verdicts = run(args.oracle, "verdicts", [{"name": e["name"], "text": e["text"]} for e in entries])
    for entry, verdict in zip(entries, verdicts):
        assert verdict.pop("name") == entry["name"]
        if "wallet_refuses" in entry:
            assert verdict["ok"], f"{entry['name']}: dash-qt refuses it too"
        if verdict["ok"]:
            verdict["read_back"] = verdict.pop("text")
        entry.update(verdict)

    doc = {
        "source": "dash-qt v24.0.0-rc.3 src/qt/mnsharesession.cpp (MnShareSession), "
                  "testdata/oracle/dashqt/gen_envelopes.py",
        "network": "regtest",
        "cases": [dict(i, steps=s["steps"]) for i, s in zip(inputs, stages)],
        "corpus": entries,
    }
    with open(OUT, "w") as f:
        json.dump(doc, f, ensure_ascii=False)
    subprocess.run([sys.executable, "-I", os.path.join(os.path.dirname(HERE), "jsonfmt.py"), OUT], check=True)
    print(f"wrote {OUT}: {len(inputs)} cases, {len(entries)} corpus texts")


if __name__ == "__main__":
    main()
