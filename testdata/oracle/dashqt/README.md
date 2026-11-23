# dash-qt session envelope vectors (MG-04b M9)

`testdata/shared_envelopes.json` records what dash-qt's own `MnShareSession`
(Dash Core v24.0.0-rc.3 `src/qt/mnsharesession.cpp`, linked unmodified) writes
and reads:

- `cases`: the two registrations of `testdata/shared_registration.json`
  (dashd's `shared_register_prepare`, `shared_sign`, `shared_combine` and the
  signed funding) replayed through the session as the creation dialog does it:
  the share table and terms with `noteDraftChange`, one `addContribution` per
  participant, `freeze`, `addSignature` per share (out of order),
  `setCombinedTx`, `setFundingSignedTx` and `setBroadcast`. After each step it
  records `toJsonString()` and `fingerprint()`. The session id is
  `SHA-256("MG-04b M9 <case name>")`, because the constructor draws a random one.
- `corpus`: texts made from those envelopes, each with one change (types,
  number forms, ranges, missing and repeated members, labels, a frozen
  transaction that doesn't match its envelope, the `coreP2PAddrs` forms
  `NetInfoMatches` reads through `ExtNetInfo::AddEntry`, approvals and funding
  signatures). Each text carries `fromJson`'s verdict: its `error`, or
  `fingerprint_ok` and the `read_back` text of the session it read.
  `wallet_refuses` marks a text dash-qt reads that the wallet refuses on
  purpose, naming the D-row.

`rust/crates/dw-protx/tests/oracle_envelopes.rs` replays every case on
`shared::session` and requires dash-qt's text and code at every step. It also
requires every text, read back, to be the session that wrote it. It reads
every corpus text and requires dash-qt's verdict, `read_back` and fingerprint
check, or a refusal where `wallet_refuses` says so. The corpus size is pinned
(`EXPECTED_CORPUS`). Tests and CI read only the JSON; nothing compiles C++.

| File | What it is |
|---|---|
| `mnshare_oracle.cpp` | The driver: `stages <cases.json>` and `verdicts <corpus.json>`, JSON on stdout. |
| `build.sh` | Extracts the tag with `git archive`, configures it against a depends prefix, makes the libraries the driver links, and links it with rc.3's `qt/mnsharesession.cpp` and `qt/bitcoinunits.cpp` (moc'd). |
| `gen_envelopes.py` | Writes the driver's inputs from `shared_registration.json`, runs it twice, and writes the JSON. The corpus is defined here. |

## Regenerating

You need a Dash Core git checkout with the tag and a built depends prefix
that includes Qt (`make -C depends`; the driver needs `lib/libQt5Core.a` and
`native/bin/moc`). The depends tree is only read. The committed vectors used
a prefix built at develop 15db4787500: `git diff v24.0.0-rc.3 15db4787500
-- depends/` touches only the Android build (the `android.mk` host, `qt.mk`'s
Android options and `fix_android_jni_static.patch`), so the Linux Qt 5.15 is
rc.3's.

```sh
testdata/oracle/dashqt/build.sh <dash checkout> <dash checkout>/depends/x86_64-pc-linux-gnu <work dir>
python3 -I testdata/oracle/dashqt/gen_envelopes.py --oracle <work dir>/mnshare_oracle
cd rust && cargo test -p dw-protx --test oracle_envelopes
```

The build configures Core once in `<work dir>/core` (about 150 MB). Delete the
work dir afterwards; nothing from it is committed. To add a corpus text,
append an `add(...)` in `gen_envelopes.py`, regenerate, and update
`EXPECTED_CORPUS`.
