# regtest — Dash Core regtest harness

Regtest infrastructure for dashwallet-desktop (DESIGN-opus.md §4.3, workstream WS-13). It has
three parts:

| Part | Path | What it does |
|---|---|---|
| Single-node regtest | `Dockerfile`, `docker-compose.yml`, `scripts/` | One official `dashd` (regtest, `-txindex`, BIP157 filters, RPC + P2P published on loopback) and a shell helper to mine and fund. |
| pytest harness | `harness/` | Fixtures that start/stop a node (Docker or local release binaries), mine, send, wait for a tx; smoke tests. |
| Masternode networks | `functional/` | Dash Core's functional `test_framework` (`DashTestFramework`), vendored, running against the release binaries: masternodes, quorums, ChainLocks, InstantSend, and a CoinJoin mixing probe. |

Measured results are in [RESULTS.md](RESULTS.md).

## Pins

| What | Pin | Source |
|---|---|---|
| Dash Core binaries | **24.0.0-rc.2** (newest v24 build on 2026-10-05; there is no final v24.0.0 yet) | `https://github.com/dashpay/dash/releases/tag/v24.0.0-rc.2` |
| SHA-256 | `x86_64-linux-gnu` 626840b4…f908, `aarch64-linux-gnu` 05a97c68…1a1b, `arm64-apple-darwin` c0e1aa38…5dcb, `x86_64-apple-darwin` 285fbb4f…f42e | release `SHA256SUMS.asc`; full values in `scripts/fetch-dashcore.sh` |
| test_framework | tag `v24.0.0-rc.2` = commit `1e239d4b05d582762aceea6ed728076ab3c38979` | `test/functional/test_framework/` (MIT, `functional/test_framework/COPYING`; `authproxy.py` is LGPL-2.1+, see below) |
| dash_hash (X11 for the framework) | 1.4.0, tag archive SHA-256 `2490feb0…7e5a` | `https://github.com/dashpay/dash_hash` (MIT) |
| Base image | `python:3.12-slim` (Debian 13, glibc 2.41) | `ARG BASE_IMAGE` in the Dockerfile |
| pytest | 8.3.4 | `harness/requirements.txt` |

The Dockerfile picks the tarball from `uname -m` at build time, so the same file builds natively on
amd64 and arm64 hosts. `scripts/fetch-dashcore.sh` is the single place that holds the version and
hashes; it is used by the Dockerfile and for host-local installs. To bump Dash Core, replace the
whole block of values in that script, re-vendor `functional/test_framework/` from the matching tag
(below), rebuild the image and rerun everything in RESULTS.md.

## Quick start

All commands below run from the repo root. Docker commands need the Claude sandbox disabled.

```sh
# start a node (build the image on first use) and wait for RPC
regtest/scripts/regtest.sh up
regtest/scripts/regtest.sh mine 101                       # pays the `miner` wallet
regtest/scripts/regtest.sh fund yXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX 2.5   # send + mine 1 block
regtest/scripts/regtest.sh status
regtest/scripts/regtest.sh cli getpeerinfo
regtest/scripts/regtest.sh down                           # discards the chain (tmpfs datadir)
regtest/scripts/selftest.sh                               # end-to-end check of regtest.sh
```

Connection details for an SPV client (e.g. `dwcli`):

| | |
|---|---|
| P2P | `127.0.0.1:19899` (`DWD_P2P_HOST_PORT`) |
| RPC | `http://127.0.0.1:19898`, user `dwd`, password `dwd` (`DWD_RPC_HOST_PORT`, `DWD_RPC_USER`, `DWD_RPC_PASSWORD`) |
| Flags | `-regtest -txindex=1 -blockfilterindex=1 -peerblockfilters=1 -peerbloomfilters=1 -listen=1 -fallbackfee=0.00001` (`scripts/node-entrypoint.sh`) |

`-peerblockfilters` serves BIP157/158 compact filters, which `dash-spv` uses; `-peerbloomfilters`
keeps BIP37 for older clients. The datadir is a tmpfs, so every `up` starts at genesis.

### pytest harness

```sh
cd regtest/harness
uv venv .venv --python 3.11 && uv pip install --python .venv/bin/python -r requirements.txt
.venv/bin/python -m pytest -v                                   # Docker backend (default)

# Local backend: run the release binaries directly on this host
sh ../scripts/fetch-dashcore.sh /path/to/dashcore               # verifies SHA-256; ad-hoc signs on macOS
DWD_REGTEST_BACKEND=local DASHCORE_DIR=/path/to/dashcore .venv/bin/python -m pytest -v
```

| Fixture | Scope | Provides |
|---|---|---|
| `regtest_node` | session | started `RegtestNode` (`.rpc`, `.wallet(name)`, `.p2p_port`, `.rpc_url`, `.ensure_wallet()`); stopped and discarded at the end; on failure the tail of the dashd log is printed |
| `funded_miner` | session | `miner` wallet with mature coins (mines 101 blocks on first use) |
| `mine` | function | `mine(blocks, address=None)` |
| `send_to_address` | function | `send_to_address(address, amount, from_wallet="miner")` → txid |
| `wait_for_tx` | function | `wait_for_tx(txid, confirmations=0, timeout=30)` → decoded tx (uses `-txindex`) |

Environment: `DWD_REGTEST_BACKEND` (`docker`|`local`), `DWD_REGTEST_BUILD=0` (Docker: skip `--build`),
`DASHCORE_DIR` (local), `DWD_KEEP_DATADIR=1` (local: keep the temp datadir). The Docker backend uses a
unique compose project and free loopback ports per session, so runs do not collide;
`DWD_COMPOSE_PROJECT=<name>` fixes the project name instead.

### L1 suites (engine against dashd)

`tests/test_l1_sync.py` (`l1-sync`) drives the `dwcli` binary: vault + import of a known mnemonic,
SPV sync, a dashd payment, history type/amount/status and balance, and a restart that keeps the
history. It is skipped unless `DWCLI` points at a built `dwcli`:

```sh
(cd rust && cargo build -p dwcli)
DWCLI=$CARGO_TARGET_DIR/debug/dwcli .venv/bin/python -m pytest -v tests/test_l1_sync.py
```

On a single node dash-spv never reports `caught_up`: there are no quorums, so its masternode phase
stalls on `getqrinfo` ("Cannot find quorum snapshot"). The suite waits on scanned heights and txids
instead (`dwcli sync --min-height` / `--txid`).

**l1-send** (`tests/test_l1_send.py`) drives a host-built `dwcli` (`DWCLI=/path/to/dwcli`; skipped when
absent) over SPV against the node: receive and sync, a custom-fee payment, coin control, a locked coin left
out of selection, subtract-fee, a foreign change address, a re-broadcast after an unknown outcome, exact
amounts at dashd, and sign/verify both ways. With one peer and no InstantSend quorum, dash-spv accepts a
broadcast only once it is mined, so the suite mines each payment from dashd's mempool while `dwcli send`
waits; the re-broadcast test leaves one unmined past dash-spv's 60 s acceptance timeout to get
`send.broadcast_unknown`, then sends the same prepared transaction again (`dwcli send --rebroadcast-unknown`).

```sh
DWD_COMPOSE_PROJECT=dwd-e2 DWD_REGTEST_BUILD=0 DWCLI=$CARGO_TARGET_DIR/debug/dwcli \
    .venv/bin/python -m pytest -v tests/test_l1_send.py
```

The harness has no dependency beyond pytest: `dwd_regtest/rpc.py` is a small stdlib JSON-RPC client
that decodes amounts as `Decimal` and sends them as strings.

## Dash Core functional test_framework from release binaries

### What is vendored

`functional/test_framework/` is a verbatim copy of `test/functional/test_framework/` at tag
`v24.0.0-rc.2` (26 top-level files, 11 in `crypto/`, including the CSV test vectors), with the
repo's `COPYING` added. `diff -r` against the tag's tree shows only the added `COPYING`.

Licensing: every Python file carries the MIT header except the empty `__init__.py` and
**`authproxy.py`, which is LGPL-2.1-or-later** (Jeff Garzik's python-jsonrpc lineage, unchanged
from Bitcoin Core). It is unmodified, test-only, and never linked into or shipped with the app. If
the project ever needs an MIT-only test tree, replace it with a small JSON-RPC client such as
`harness/dwd_regtest/rpc.py`. Also vendored:
`functional/feature_llmq_chainlocks.py` (upstream test, unmodified) as a regression check that the
framework works unchanged.

Files that are **not** needed and were left out: `test_runner.py` (we run single scripts),
`combine_logs.py`, `test/config.ini.in` (replaced by `run.sh`), `test/lint`, `test/util`.

Runtime requirements of the framework:

* Python ≥ 3.9 (the image has 3.12).
* `dash_hash` (C extension, X11) — imported by `messages.py`; installed from the pinned 1.4.0 archive.
  The upstream CI image installs the same version (`contrib/containers/ci/ci-slim.Dockerfile`).
* `config.ini`. A source build generates it from `test/config.ini.in`; release binaries have no
  build tree, so `functional/run.sh` writes one at run time (wallet, SQLite, BDB, CLI, util, wallet
  tool and ZMQ declared enabled, matching the official release build) and exports
  `DASHD`/`DASHCLI`/`DASHUTIL`/`DASHWALLET` pointing at the release `bin/`. `BUILDDIR` is only used
  for the default binary paths and `PATH`, both overridden.
* `pyzmq` is not installed; tests that need it (`interface_zmq.py`) would skip themselves.

### Running

```sh
# in Docker (recommended; this is what CI should run)
regtest/scripts/run-functional.sh                      # dwd_mn_chainlock.py + dwd_coinjoin_probe.py
docker compose -f regtest/docker-compose.yml --profile functional run --rm functional \
    feature_llmq_chainlocks.py --timeout-factor=3             # any single test with any options

# on a macOS host with the darwin release (ad-hoc signed by fetch-dashcore.sh)
uv venv /path/to/func-venv && uv pip install --python /path/to/func-venv/bin/python \
    https://github.com/dashpay/dash_hash/archive/refs/tags/1.4.0.tar.gz
DWD_PYTHON=/path/to/func-venv/bin/python DASHCORE_DIR=/path/to/dashcore \
    regtest/functional/run.sh dwd_mn_chainlock.py
```

`run.sh` puts config.ini, the framework cache and each run's node datadirs (`--tmpdir`, unique per
run) under `DWD_FUNC_TMP` (default `$TMPDIR/dwd-functional`). The `functional` service bind-mounts
`functional/` read-only over the copy baked into the image, so test edits need no rebuild; inside
the container everything lives in a tmpfs `/tmp`. Any test_framework option can follow the script
name and overrides run.sh's defaults (`--nocleanup`, `--tmpdir=`, `--portseed=`,
`--timeout-factor=`, `--loglevel=DEBUG`, …).

The upstream `feature_llmq_chainlocks.py` passed 2 of 4 runs on a heavily loaded laptop, with
timing failures (RESULTS.md §6). Use `--timeout-factor` on shared machines. Our own `dwd_*` tests
passed every run after their initial fixes.

### Masternode network facts (regtest, v24.0.0-rc.2)

* `set_dash_test_params(num_nodes, mn_count)` creates the **simple nodes first**: `nodes[0]` is the
  controller/miner, `nodes[1 .. num_nodes-mn_count-1]` are plain wallet nodes, the last `mn_count`
  are masternodes. Masternodes run in masternode mode, which has **no wallet** (wallet RPCs return
  `Method not found`).
* `setup_network` registers the MNs (1000 DASH collateral each), forces `mnsync` to finished on
  every node and enables `SPORK_2_INSTANTSEND_ENABLED` and `SPORK_19_CHAINLOCKS_ENABLED`. DKG is
  off until the test sets `SPORK_17_QUORUM_DKG_ENABLED` to 0.
* LLMQs on regtest (`src/chainparams.cpp` `CRegTestParams`): ChainLocks and MNHF use `llmq_test`
  (type 100, 3 members, threshold 2); InstantSend uses `llmq_test_dip0024` (type 103, 4 members,
  rotating); Platform uses `llmq_test_platform`. `mine_cycle_quorum()` mines the rotating pair and,
  in practice, also yields an `llmq_test` quorum at the same height; `mine_quorum()` covers
  `llmq_test` alone. So **4 masternodes** is the minimum for both ChainLocks and InstantSend
  (verified). From the quorum sizes, 3 should suffice for ChainLocks alone, and
  `set_dash_llmq_test_params(1, 1)` + `mine_quorum_single_member()` gives ChainLocks with 1 MN, as
  upstream `rpc_verifychainlock.py` does. Neither was run here.
* **Mining gate**: with ChainLocks enabled, block templates only include a transaction that has an
  InstantSend lock or has been in the mempool for 10 minutes (`WAIT_FOR_ISLOCK_TIMEOUT` in
  `src/test/miner_tests.cpp`). Without an InstantSend quorum, wallet payments stay in the mempool and
  balances stay `untrusted_pending`. Mine quorums before funding anything.
* The framework runs every node with mocktime that starts near the regtest genesis time (2014) and
  advances only via `bump_mocktime`. An external client using the wall clock (e.g. `dwcli` speaking
  P2P to these nodes) sees old block timestamps; set `self.disable_mocktime = True` in such tests if
  that matters.
* P2P ports inside the container are `11000 + n + (20 * portseed) % 9979` (`util.p2p_port`); pass
  `--portseed=<k>` to make them deterministic. RPC listens on 127.0.0.1 only. The intended way to
  test `dwcli` against a masternode network is to run `dwcli` (Linux build) inside the same
  container from a `DashTestFramework` subclass and connect it to `p2p_port(i)`; this is not built
  yet.

## CoinJoin on regtest

Constants at v24.0.0-rc.2:

| | regtest | testnet | mainnet | source |
|---|---|---|---|---|
| `nPoolMinParticipants` | **2** | 2 | 3 | `src/chainparams.cpp` (regtest line 863) |
| `nPoolMaxParticipants` | 20 | 20 | 20 | same |
| `COINJOIN_QUEUE_TIMEOUT` / `COINJOIN_SIGNING_TIMEOUT` | 30 s / 15 s | | | `src/coinjoin/coinjoin.h` |
| client auto-denominate cadence | every 5–15 maintenance ticks (≈ s) | | | `COINJOIN_AUTO_TIMEOUT_MIN/MAX` |

Session rules that decide whether a regtest network can mix (`src/coinjoin/server.cpp`
`IsSessionReady`):

1. A queue starts mixing immediately only when it has **20** participants (max). Otherwise it
   starts once the queue has timed out (30 s of mocktime) **and** has ≥ 2 participants.
2. New in v24 (protocol `COINJOIN_REBALANCE_VERSION = 70241`): `dsa` carries flags
   (`FLAG_PROMOTION`, `FLAG_DEMOTION`), and the masternode counts participants per "side" of the
   session denomination (`CoinJoin::MixSideCounts`, `src/coinjoin/common.h`). Each side must hold 0
   or ≥ 2 participants. Two plain (standard) participants satisfy this. This matters for WS-06: a
   client speaking protocol ≥ 70241 must serialize the dsa flags field.
3. A masternode cannot host queues back to back: `IsMixingThresholdExceeded` allows a new `dsq`
   from the same MN only after `mn_count / 5` other queues (0 with < 5 MNs, so no limit on small
   regtest networks).
4. The client wallet needs `-keypool` well above 1 and `-createwalletbackups` > 0, or its automatic
   backup check stops mixing (the framework's defaults are `keypool=1`, backups 0; see upstream
   `rpc_coinjoin.py`).
5. Mixing transactions are ordinary transactions for the mining gate above, so InstantSend quorums
   are needed for them to be mined promptly.

No upstream functional test runs a real mixing session (`rpc_coinjoin.py` fabricates mixed outputs
locally). `functional/dwd_coinjoin_probe.py` tries it with 4 masternodes and 3 mixing dashd
wallets: it drives real time (client cadence) and mocktime (queue timeouts) together and passes once
two wallets own outputs with `coinjoin_rounds >= 1` and a confirmed transaction spends inputs from
two different wallets. **Result: it works.** 6 of 6 runs after the quorum fix passed, with the first
mixed outputs 21–44 s after `coinjoin start`. Each mixing transaction had 2–6 inputs from 2–3
wallets and got InstantSend- and ChainLocked (RESULTS.md §5). A CoinJoin interop suite for our
client can therefore use one `DashTestFramework` network: 4 MNs, our client, and at least one dashd
wallet as the counterparty.

## Layout

```
regtest/
├── README.md, RESULTS.md    this file; measured results
├── Dockerfile               image: python:3.12-slim + dash_hash + Dash Core release + functional/
├── docker-compose.yml       services: dashd (single node), functional (profile; runs one test script)
├── scripts/
│   ├── fetch-dashcore.sh    download + SHA-256 verify + unpack (+ ad-hoc codesign on macOS)
│   ├── node-entrypoint.sh   dashd regtest command line used by the image
│   ├── regtest.sh           up/down/status/mine/fund/cli/wcli
│   ├── selftest.sh          end-to-end check of regtest.sh
│   └── run-functional.sh    run functional tests in the compose service, one summary line each
├── harness/                 pytest: dwd_regtest/ (rpc.py, node.py), conftest.py,
│                            tests/test_smoke.py (node), tests/test_harness.py (failure handling)
└── functional/
    ├── run.sh               writes config.ini for release binaries, runs one test
    ├── test_framework/      vendored from dash v24.0.0-rc.2 (MIT)
    ├── feature_llmq_chainlocks.py   upstream, unmodified
    ├── dwd_mn_chainlock.py  4 MNs → quorums → ChainLock + InstantSend
    └── dwd_coinjoin_probe.py        4 MNs + 3 mixing wallets → real CoinJoin rounds
```
