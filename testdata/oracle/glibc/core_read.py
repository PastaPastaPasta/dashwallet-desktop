#!/usr/bin/env python3
"""MG-04b D78: the wallet's `coreP2PAddrs` reader against Core's, over glibc.

Core reads each entry with `ExtNetInfo::AddEntry` (v24.0.0-rc.3
`src/evo/netinfo.cpp:500-551`); for an IP entry that ends in glibc's
`getaddrinfo`. This script ports that path line by line and calls this
machine's real `getaddrinfo`, then runs the wallet's reader
(`rust/crates/dw-protx/examples/core_service_read.rs`) on the same entries
and compares the two. See README.md.

Usage: core_read.py --reader <core_service_read binary> [--seed N] [--count N]
"""

import argparse
import os
import platform
import random
import socket
import subprocess
import sys

# rc.3 `evo/netinfo.cpp:28`.
SAFE_CHARS_IPV4_6 = set("abcdefABCDEF1234567890.:[]")
# rc.3 `netaddress.h:68-86`.
IPV4_IN_IPV6_PREFIX = bytes([0] * 10 + [0xFF, 0xFF])
TORV2_IN_IPV6_PREFIX = bytes([0xFD, 0x87, 0xD8, 0x7E, 0xEB, 0x43])
INTERNAL_IN_IPV6_PREFIX = bytes([0xFD, 0x6B, 0x88, 0xC0, 0x87, 0x24])
BIP155_IPV4, BIP155_IPV6, BIP155_CJDNS = 1, 2, 6
TORV2 = "torv2"


def parse_uint16(s):
    """`ParseUInt16` (`util/strencodings.cpp:234-270`): `ParseIntegral`
    refuses `+-`, strips one `+`, then `ToIntegral` (`from_chars`) takes
    the whole rest as decimal digits."""
    if len(s) >= 2 and s[0] == "+" and s[1] == "-":
        return None
    if s and s[0] == "+":
        s = s[1:]
    if not s or not all("0" <= c <= "9" for c in s):
        return None
    n = int(s)
    return n if n <= 0xFFFF else None


def split_host_port(s, port):
    """`SplitHostPort` (`util/strencodings.cpp:101-126`); its return value
    is unused by both callers."""
    colon = s.rfind(":")
    have = colon != -1
    bracketed = have and s[0] == "[" and s[colon - 1] == "]"
    multi = have and colon != 0 and s.rfind(":", 0, colon) != -1
    if have and (colon == 0 or bracketed or not multi):
        n = parse_uint16(s[colon + 1 :])
        if n is not None:
            s, port = s[:colon], n
    if len(s) > 0 and s[0] == "[" and s[-1] == "]":
        s = s[1:-1]
    return s, port


def lookup_intern(name):
    """`LookupIntern` (`netbase.cpp:147-174`) over `WrappedGetAddrInfo`
    (`:44-59`, `AI_NUMERICHOST`), one solution: `SetSpecial` can't succeed
    on a name `SAFE_CHARS_IPV4_6` passed (no `.onion` or `.i2p`); a result
    read as internal is skipped. Returns (network, address bytes) as
    `SetLegacyIPv6` (`netaddress.cpp:135-162`) leaves it, or `TORV2` for a
    TORv2-in-IPv6 address, which it leaves as the invalid `::`."""
    try:
        infos = socket.getaddrinfo(
            name, None, socket.AF_UNSPEC, socket.SOCK_STREAM, socket.IPPROTO_TCP, socket.AI_NUMERICHOST
        )
    except (socket.gaierror, UnicodeError, ValueError):
        return None
    for family, _, _, _, sockaddr in infos:
        if family == socket.AF_INET:
            return BIP155_IPV4, socket.inet_pton(socket.AF_INET, sockaddr[0])
        if family == socket.AF_INET6:
            raw = socket.inet_pton(socket.AF_INET6, sockaddr[0].split("%")[0])
            if raw.startswith(IPV4_IN_IPV6_PREFIX):
                return BIP155_IPV4, raw[12:]
            if raw.startswith(TORV2_IN_IPV6_PREFIX):
                return TORV2
            if raw.startswith(INTERNAL_IN_IPV6_PREFIX):
                continue
            return BIP155_IPV6, raw
    return None


def core_reads(entry):
    """`AddEntry`'s IP path up to `MaybeFlipIPv6toCJDNS` (`netbase.cpp:833-840`):
    the service Core goes on to validate, as `network:address hex:port`, or
    `-` when it keeps none: a lookup that fails, or a TORv2 prefix, read as
    the invalid `::` that `ValidateService` refuses and that the wallet's
    `ServiceAddr::from_socket_addr` refuses at once. A written `::` is read
    as `::` by both and refused by `ValidateService`."""
    addr, port = split_host_port(entry, 0)
    if not set(addr) <= SAFE_CHARS_IPV4_6:
        return "-"  # a domain or bad input, not a service
    if not addr:
        return "-"  # `Lookup` of an empty name
    host, port = split_host_port(addr, port)
    read = lookup_intern(host)
    if read is None or read == TORV2:
        return "-"
    network, raw = read
    if network == BIP155_IPV6 and raw[0] == 0xFC:
        network = BIP155_CJDNS
    return f"{network}:{raw.hex()}:{port}"


# Hand-picked: the D78 corpus forms, the unit list, review r1's n-1 forms and
# the validator's edge cases.
EDGES = [
    "127.0.0.1:19901", "127.1:19901", "127.0.1:19901", "2130706433:19901", "0177.0.0.01:19901",
    "127.000.000.001:19901", "127.0.0.1:019901", "127.0.0.1:+19901", "[127.0.0.1]:19901",
    "[[127.0.0.1]]:19901", "[127.0.0.1:19901]", "[::ffff:127.0.0.1]:19901", "[::ffff:7f00:1]:19901",
    "[2001:db8::1]:9999", "127.0.0.1", "[::1]", "", ":19901", "0x7f.0.0.1:19901", "localhost:19901",
    "127.0.0.08:19901", "127.0.0.256:19901", "127.0.0.1.:19901", "127..0.1:19901", "127.0.0.0.1:19901",
    "127.16777216:19901", "4294967296:19901", "4294967295:19901", "127.0.0.1:19901:19901",
    "::ffff:127.0.0.1:19901", "[::1%1]:19901", "[127.0.0.1:+19901]", "127.0.0.1:-19901",
    "127.0.0.1:84437", "127.0.0.1:+-1", "127.0.0.1:++19901", "383.0.0.1:19901", "383.1:19901",
    "127.256:19901", ".1.2.3:9999", "1.2.3.:9999", "0x1.2.3.4:9999", "x1.2.3.4:9999", "010.0.0.1:9999",
    "08.0.0.1:9999", "0377.0377.0377.0377:9999", "0400.0.0.1:9999", "1.2.3.4:0", "1.2.3.4:65535",
    "1.2.3.4:65536", "1.2.3.4:+", "1.2.3.4:++", "1.2.3.4:00009999", "[1.2.3.4]:+9999", "1.2.772:9999",
    "[::FFFF:1.2.3.4]:9999", "[::1.2.3.4]:9999", "[::]:9999", "[1:2:3:4:5:6:7:8:9]:9999",
    "[1:2:3:4:5:6:7:8]:9999", "[fd87:d87e:eb43::1]:9999", "[fd6b:88c0:8724::1]:9999",
    "[fc00::1]:9999", "[fe80::1]:9999", "[::ffff:1.2.3.04]:9999", "[::ffff:1.2.3]:9999",
    "[[::1]]:9999", "[::1]:9999:", "[]:9999", "[:9999", "]:9999", "1.2.3.4]:9999", "[1.2.3.4:9999",
    "[f61:6263:6465:6667:6869:6a2e:6b6c:6d6e]:19999",
]

# Pieces the structured generator joins.
DEC_PARTS = ["0", "1", "7", "08", "09", "010", "0377", "0400", "00", "255", "256", "383", "65535",
             "65536", "16777215", "16777216", "4294967295", "4294967296", "0x1", "0X7f", "x1", "a", "",
             "127", "1e3", "00000001"]
PORTS = ["", ":", ":0", ":1", ":9999", ":19999", ":65535", ":65536", ":+9999", ":++9999", ":+-1", ":-1",
         ":009999", ":+", ":9999:", ":a", ":0x10", ":99999999999"]
HEX_GROUPS = ["0", "1", "ffff", "FFFF", "fc00", "fd87", "d87e", "eb43", "fd6b", "88c0", "8724", "2001",
              "db8", "00000", "12345", "g", "0f61", "7f00"]
NOISE = "0123456789abcdefABCDEFxX.:[]+-% "


def ipv4_form(r):
    host = ".".join(r.choice(DEC_PARTS) for _ in range(r.randint(1, 5)))
    return host


def ipv6_form(r):
    groups = [r.choice(HEX_GROUPS) for _ in range(r.randint(1, 9))]
    if r.random() < 0.4:
        groups.insert(r.randint(0, len(groups)), "")
        if r.random() < 0.3:
            groups.insert(r.randint(0, len(groups)), "")
    if r.random() < 0.25:
        groups.append(ipv4_form(r))
    return ":".join(groups)


def generated(seed, count):
    r = random.Random(seed)
    out = []
    while len(out) < count:
        kind = r.random()
        if kind < 0.40:
            host = ipv4_form(r)
            if r.random() < 0.2:
                host = "[" + host + "]"
            out.append(host + r.choice(PORTS))
        elif kind < 0.80:
            host = ipv6_form(r)
            port = r.choice(PORTS)
            shape = r.random()
            if shape < 0.6:
                out.append("[" + host + "]" + port)
            elif shape < 0.8:
                out.append(host + port)
            else:
                out.append("[" + host + port + "]")
        else:
            out.append("".join(r.choice(NOISE) for _ in range(r.randint(0, 24))))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reader", required=True, help="the built core_service_read example")
    ap.add_argument("--seed", type=int, default=20261029)
    ap.add_argument("--count", type=int, default=300_000, help="generated entries, besides EDGES")
    args = ap.parse_args()

    entries = EDGES + generated(args.seed, args.count)
    want = [core_reads(e) for e in entries]
    stdin = "".join(e.encode().hex() + "\n" for e in entries)
    got = subprocess.run([args.reader], input=stdin, capture_output=True, text=True, check=True).stdout.split("\n")[:-1]
    if len(got) != len(entries):
        sys.exit(f"the reader answered {len(got)} of {len(entries)} entries")

    mismatches = [(e, w, g) for e, w, g in zip(entries, want, got) if w != g]
    read = sum(w != "-" for w in want)
    libc = " ".join(platform.libc_ver())
    print(f"libc {libc or 'unknown'} ({os.uname().sysname}); seed {args.seed}")
    print(f"{len(entries)} entries ({len(EDGES)} hand-picked): {read} read as a service, "
          f"{len(entries) - read} refused; {len(mismatches)} mismatches")
    for e, w, g in mismatches[:20]:
        print(f"  {e!r}: Core {w}, wallet {g}")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
