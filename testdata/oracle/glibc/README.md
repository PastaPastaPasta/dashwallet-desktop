# `coreP2PAddrs` reading against glibc (MG-04b D78)

Core reads each entry of a shared session's `coreP2PAddrs` with
`ExtNetInfo::AddEntry` (Dash Core v24.0.0-rc.3 `src/evo/netinfo.cpp:500-551`).
For an IP entry that ends in the platform's `getaddrinfo(AI_NUMERICHOST)`.
The wallet's reader, `dw_protx::shared::netinfo::parse_core_service`, is a
pure-Rust port of that path as glibc answers it, the same on every target.
This harness compares the two on this machine's glibc.

- `core_read.py` ports `AddEntry`'s IP path line by line: `SplitHostPort`
  (`util/strencodings.cpp:101-126`) with `ParseUInt16` (`:234-270`), the
  `SAFE_CHARS_IPV4_6` filter (`evo/netinfo.cpp:28,512`), `Lookup`'s second
  split (`netbase.cpp:194-209`), `LookupIntern` (`:147-174`) over the real
  `getaddrinfo` with `WrappedGetAddrInfo`'s hints (`:44-59`),
  `SetLegacyIPv6` (`netaddress.cpp:135-162`) and `MaybeFlipIPv6toCJDNS`
  (`netbase.cpp:833-840`). It prints the service Core goes on to validate,
  or `-` for none.
- `rust/crates/dw-protx/examples/core_service_read.rs` prints, for the same
  entries, what `parse_core_service` reads and
  `ServiceAddr::from_socket_addr` keeps.
- The entries are the hand-picked `EDGES` (the D78 corpus forms, the unit
  test's forms, review r1's n-1 forms) plus `--count` generated ones from
  `--seed`: IPv4 forms of 1 to 5 parts (octal, hex, overflowing,
  bracketed), IPv6 forms (`::`, embedded IPv4, the TORv2, internal and
  CJDNS prefixes, too many groups), port forms (`+`, `++`, `+-`, leading
  zeros, 0, 65535, 65536) and noise over the characters these use.

Run (Linux with glibc; nothing is written):

```sh
cd rust && cargo build -p dw-protx --example core_service_read
python3 -I ../testdata/oracle/glibc/core_read.py --reader target/debug/examples/core_service_read
```

The committed run (2026-10-29, glibc 2.43, seed 20261029, the default):
300,075 entries (75 hand-picked), 14,895 read as a service, 285,180 refused,
0 mismatches. The host goes to `getaddrinfo` as bytes: as a str, CPython's
IDNA codec refuses some names (a label over 63 characters) before glibc
reads them (review r2 of 1c, n-1). The script exits 1 on any mismatch and
lists the first 20.

Another libc may read an unusual form differently (v24 defines no text form
beyond `AddEntry`). A coordinator on such a libc then freezes a payload the
wallet's frozen check refuses before anything is signed; the participant's
reservation is released by discarding the session (D17).
