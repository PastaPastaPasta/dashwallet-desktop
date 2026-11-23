#!/usr/bin/env python3
"""Writes the input lists that qt_oracle.cpp evaluates.

Outputs (next to this script): uri_inputs.json, amount_inputs.json.
The first block of URI inputs is Dash Core src/qt/test/uritests.cpp verbatim;
the rest probe QUrl/QUrlQuery decoding and BitcoinUnits::parse edge cases.
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

X = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg"  # mainnet P2PKH (uritests.cpp)
Y = "yf7WoPrbJCGhLLtpeBe7tteKEKwpvZ1w97"  # testnet P2PKH (key_io_valid.json)
S = "7XShCrc5u9rZZv7j18WqbUMZMxp8k1Hq4z"  # mainnet P2SH (key_io_valid.json)

parse = []
for q in [
    "?req-dontexist=", "?dontexist=", "?label=Some Example Address", "?amount=0.001", "?amount=1.001",
    "?amount=100&label=Some Example", "?message=Some Example Address", "?req-message=Some Example Address",
    "?amount=1,000&label=Some Example", "?amount=1,000.0&label=Some Example",
    "?amount=100&amount=200&label=Wikipedia Example", "?amount=100&amount=1,000&label=Wikipedia Example",
    "?amount=100&label=?", "?amount=100&label=%3F",
    "?amount=100&label=Some Example&message=Some Example Message",
    "?IS=1", "?req-IS=1", "",
]:
    parse.append("dash:" + X + q)

parse += [
    "DASH:" + X, "Dash:" + X + "?amount=1", "dash://" + X, "dash://" + X + "/?amount=1", "DASH://" + X,
    "dash:" + X + "/", "dash:" + X + "//", "dash:/" + X, "dash:", "dash:?amount=1", "dash", "bitcoin:" + X,
    "pay:" + X, "dashwallet:" + X, "dash:" + Y, "dash:" + S + "?amount=2",
    "dash:" + X + "?r=https://example.com/pr", "dash:" + X + "?r=https%3A%2F%2Fexample.com%2Fpr",
    "dash:?r=https%3A%2F%2Fexample.com%2Fpr",
    "dash:" + X + "&amount=1", "dash:" + X + "?amount=1#label=a", "dash:" + X + "#?amount=1",
    "dash:" + X + "?label=a=b", "dash:" + X + "?label=a;b", "dash:" + X + "?label=a&b", "dash:" + X + "?label",
    "dash:" + X + "?is=1", "dash:" + X + "?req-is=1", "dash:" + X + "?Label=x", "dash:" + X + "?req-label=x",
    "dash:" + X + "?req-amount=1.5", "dash:" + X + "?req-=1", "dash:" + X + "?=x", "dash:" + X + "?&&",
    "dash:" + X + "?&amount=1", "dash:" + X + "?amount", "dash:" + X + "?amount=",
    "dash:" + X + "?message=a&message=b", "dash:" + X + "?label=a&label=",
    "dash: " + X, " dash:" + X, "dash:" + X + " ", "dash:" + X + "?amount=1 ", "\tdash:" + X, "dash:" + X + "\n",
    "dash:%58" + X[1:], "dash:" + X[:5] + "%20" + X[5:], "dash:" + X + "%2F", "dash:" + X + "%3Famount=1",
    "dash:é" + X,
    "dash:" + X + "?amount=1&IS=1&label=x&req-message=m",
    "dash:" + X + "?message=Hello%20World&label=Shop", "dash:" + X + "?req-sender=foo",
    "dash:" + X + "?sender=foo&user=alice&currency=USD&local=12.5",
]

label_vals = [
    "%20", "+", "%2B", "%26", "%3D", "%25", "%", "%zz", "%4", "%41", "%61%62", "%C3%A9", "%c3%a9", "é",
    "%E9", "%00", "%09", "%0A", "%0D", "%7F", "%3C", "%3E", "%22", "%5C", "%5E", "%60", "%7B", "%7C", "%7D",
    "%23", "%2F", "%3A", "%40", "%21", "%24", "%27", "%28", "%29", "%2A", "%2C", "%3B", "%5B", "%5D", "%7E",
    "%2D", "%2E", "%5F", "<>", "a b", "a\tb", '"q"', "a\\b", "^`{|}", "[x]", "a:b@c/d", "%E2%82%AC",
    "€", "%F0%9F%98%80", "\U0001F600", "%ED%A0%80", "%C3", "%C3%28", "%80", "%FF", "100%", "%%", "%%41",
    "a%2", "%2", "%u0041", "ä%20ö", "%E2%80%89",
]
for v in label_vals:
    parse.append("dash:" + X + "?label=" + v)
parse += [
    "dash:" + X + "?message=%3F%26%3D%20x",
    "dash:" + X + "?l%61bel=key-encoded",
    "dash:" + X + "?label%3Dx=y",
    "dash:" + X + "?req%2Dfoo=1",
]
amt_vals = [
    "1", "1.", "0.5", ".5", ".", "0", "00000001", "1.123456789", "1.12345678", "-1", "+1", "1e3", " 1", "1 000",
    "1%20000", "1%E2%80%89000", "1%09", "%091", "1%0A", "9999999999.99999999", "99999999999.99999999",
    "92233720368.54775807", "1..2", "1.2.3", "0x10", "%D9%A1", "1,5", "+", "-", "--1", "1-", "%EF%BC%91",
    "1%C2%A0", "%C2%A01", "21000000", "21000001", "100000000000", "1+1", "%2B1",
]
for v in amt_vals:
    parse.append("dash:" + X + "?amount=" + v)
parse.append("dash:" + X + "?amount=1&amount=")
parse.append("dash:" + X + "?label=" + "a" * 300)


# Tolerant-mode percent fix-up scope, lowercase hex in kept-encoded
# sequences, path decoding, and authority validity.
parse += [
    "dash:%58" + X[1:] + "?label=%",
    "dash:" + X + "?label=%41&message=%",
    "dash:" + X + "?label=%41%",
    "dash:" + X + "?label=%2f%3a%e9%c3%28",
    "dash:" + X + "?label=%2F%3A&message=%zz",
    "dash:" + X + "%E9", "dash:" + X + "%00", "dash:" + X + "%zz", "dash:" + X + "%", "dash:" + X + "%2",
    "dash:" + X + "%C3%28", "dash:" + X + "%25", "dash:" + X + "%2f", "dash:" + X + "%23", "dash:" + X + "%09",
    "dash:" + X + "\t", "dash:" + X + "<>", "dash:" + X + "%3C",
    "dash://a b", "dash://[::1]/x", "dash://user@host:99/p", "dash://host:abc/p", "dash://host/p%20q",
    "dash://" + X + "?amount=1", "dash:///x", "dash://", "dash:?", "dash:#", "dash:X?#",
    "1dash:" + X, "da sh:" + X, "dash+x:" + X, "dAsH:" + X, "dash:" + X + "?label=a+b%20c",
    "dash:" + X + "?label=%7e%2d", "dash:" + X + "?label=%e2%82%ac",
]

# Invalid UTF-8 replacement policy in path (FullyDecoded) and query.
for bad in ["%F0%9FA", "%F0%9F", "%E2%82", "%E2%82A", "%ED%A0%80", "%C0%80", "%EF%BF%BE", "%F4%90%80%80",
            "%F8%88%80%80%80", "%E0%80%AF", "%F0%9F%98", "%80%80", "%C3%A9%C3", "%EF%B7%90", "%F0%9F%BF%BE", "%EF%BF%BF", "%F4%8F%BF%BD", "%EF%B7%AF", "%EF%B7%B0"]:
    parse.append("dash:" + X + bad)
    parse.append("dash:" + X + "?label=" + bad)

fmt = []
for addr in [X, Y, S, "", "not an address", "a b"]:
    fmt.append({"address": addr, "amount": 0, "label": "", "message": ""})
for amt in [1, 100000, 100000000, 123456789, -1, -100000000, 2100000000000000, 10000000000]:
    fmt.append({"address": X, "amount": amt, "label": "", "message": ""})
for lbl in ["Shop", "Some Example", "a&b=c?d#e", "100%", "é€\U0001F600", "a+b", "~-._",
            "!*'();:@&=+$,/?#[]", "<>\"\\^`{|}", "\t\n", "Ж"]:
    fmt.append({"address": X, "amount": 0, "label": lbl, "message": ""})
    fmt.append({"address": X, "amount": 150000000, "label": lbl, "message": "msg " + lbl})
fmt.append({"address": X, "amount": 0, "label": "", "message": "only message"})
fmt.append({"address": X, "amount": 1, "label": "L", "message": "M"})

with open(os.path.join(HERE, "uri_inputs.json"), "w", encoding="utf-8") as f:
    json.dump({"parse": parse, "format": fmt}, f, ensure_ascii=False, indent=1)
    f.write("\n")

amounts = [
    0, 1, 9, 10, 99, 100, 999, 1000, 9999, 10000, 99999, 100000, 123456, 999999, 1000000, 12345678, 99999999,
    100000000, 100000001, 123456789, 1234567890, 12345678901, 100000000000, 123456789012345, 2100000000000000,
    -1, -99999, -100000000, -123456789, -2100000000000000,
]
amount_parse = [
    "1", "1.", ".5", ".", "", " ", "0", "1.123456789", "1.12345678", "1.12345", "1.123456", "1.12", "1.123",
    "-1", "+1", "1e3", " 1", "1 000", "1 000", "1\t", "\t1", "1\t.5", "1..2", "1.2.3", "abc", "0x10",
    "١", "999999999999999999", "9999999999999999999", "99999999999.99999999", "1,5", "1.5 ", "1. 5", "+",
    "-", "--1", "+-1", "1-", "１", "1 ", " 1", " 1", "007", "0.00000001", "21000000",
    "92233720368.54775807", "1 000", "1　",
]
with open(os.path.join(HERE, "amount_inputs.json"), "w", encoding="utf-8") as f:
    json.dump({"amounts": amounts, "parse": amount_parse}, f, ensure_ascii=False, indent=1)
    f.write("\n")
print(len(parse), "uri parse inputs,", len(fmt), "format inputs")
