# Golden-vector generators

The JSON files in `testdata/` that describe Dash Core / dash-qt behaviour are
not written by hand. Each comes from running Core's own code. Rust tests in
`rust/crates/dw-*` and, later, Swift tests read them.

| File | Produced by | What runs |
|---|---|---|
| `uri_cases.json`, `amount_format.json` | `qt/run.sh` | dash-qt's `parseBitcoinURI`, `formatBitcoinURI`, `BitcoinUnits`, `GUIUtil::formatAmount`, copied into `qt/qt_oracle.cpp` and linked against Qt 5.15 (`brew install qt@5`). Inputs: `qt/make_inputs.py`. |
| `bip39_core_quirks.json` | `bip39/make_vectors.py --core-src <dash>/src` | Dash Core's `src/wallet/bip39.cpp` compiled into `bip39/bip39_oracle.cpp`; the `dashd` section is copied from `dashd/dashd_bip39.json`. |
| `message_cases.json`, `dumpwallet/*`, `dashd/dashd_bip39.json` | `dashd/gen_vectors.py` | A regtest `dashd` (committed vectors: v24.0.0-rc.2) started with `-keypool=4`: `signmessagewithprivkey`, `verifymessage`, `upgradetohd`, `dumphdinfo`, `listdescriptors`, `dumpwallet`. |
| `address_cases.json` | `dashd/gen_address_cases.py` | `validateaddress` on a mainnet dashd (`-connect=0`, never syncs) and a regtest dashd. |
| `core/*.json` | copied | Dash Core `src/test/data/{key_io_valid,key_io_invalid,bip39_vectors}.json`. |
| `uri_ext_cases.json` | hand-written | Cases from the iOS wallet's unit tests plus the desktop deviations; see the `notes` key. |

Regenerating the dashd files needs a fresh datadir (wallet names are fixed):

```sh
dashd -regtest -datadir=$DIR -daemon -listen=0 -rpcport=29445 -rpcuser=u -rpcpassword=p -keypool=4
python3 testdata/oracle/dashd/gen_vectors.py --rpc http://u:p@127.0.0.1:29445 --dump-dir $DIR \
  --weak "$(sed -n 1p testdata/oracle/bip39/weak_sample.txt)" "$(sed -n 6p testdata/oracle/bip39/weak_sample.txt)"
python3 testdata/oracle/bip39/make_vectors.py --core-src <dash checkout>/src
```

Dump files and the `Created on` header change on every run; the tests check
structure and byte-identical rewriting, not timestamps.
