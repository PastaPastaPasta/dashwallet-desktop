# Regtest harness — recorded results

These runs used the harness's old location `tests/regtest/`; it now lives at `regtest/` (moved so
the directory no longer collides with SwiftPM's `Tests/` on case-insensitive file systems).
Paths in the commands below are as they were run.

Run on 2026-10-05 on the macOS dev laptop (Apple Silicon, 14 cores, macOS 26 / Darwin 25.5,
OrbStack Docker 29.4.0, compose v5.1.2). The machine was shared with other agents during these runs:
load average 160–980, and OrbStack's daemon restarted once mid-run (see "Upstream
feature_llmq_chainlocks.py"). Times are wall-clock under that load.

Binaries: Dash Core **v24.0.0-rc.2** (`Dash Core version v24.0.0-rc.2`, protocol 70242,
subversion `/Dash Core:24.0.0/`). Raw logs were kept outside the repo in
`/Users/pasta/workspace/dashwallet-desktop-deps/regtest-cache/logs/`.

## 1. Image build and release verification

| Check | Result |
|---|---|
| `docker compose -f tests/regtest/docker-compose.yml build dashd` (linux/arm64, native) | built, 224 MB; `verified dashcore-24.0.0-rc.2-aarch64-linux-gnu.tar.gz sha256=05a97c68…1a1b` |
| `docker build --platform linux/amd64` (Rosetta emulation) | built; `verified dashcore-24.0.0-rc.2-x86_64-linux-gnu.tar.gz sha256=626840b4…f908`; container started, `createwallet`, `generatetoaddress 101`, `getbalance` = 500.00000000, `getnetworkinfo.buildversion` = v24.0.0-rc.2. Image removed afterwards. |
| `scripts/fetch-dashcore.sh <dir>` on macOS arm64 | `verified dashcore-24.0.0-rc.2-arm64-apple-darwin.tar.gz sha256=c0e1aa38…5dcb` |
| macOS binaries without signing | **killed at exec (exit 137)**; `codesign -dv` → "code object is not signed at all". Fixed: the script ad-hoc signs them (`codesign --force --sign -`); afterwards `dashd -version` → v24.0.0-rc.2. |
| `x86_64-apple-darwin` tarball | hash pinned from SHA256SUMS.asc, not downloaded or run. |

## 2. Single node + helper script

`tests/regtest/scripts/selftest.sh` (compose project `dwd-regtest-selftest`, ports 29898/29899):

```
mined 5 block(s) to yX5x9ZbwJJkih2rJ1PwrBrVxtqEhznhY7g; height 5
fund txid: 577ef8b7b01a5c781c54069f92aad7caee078afaf8b52970518109c98441abfe
height: 107
peers:  0
miner balance: 3496.74999775
PASS: regtest.sh selftest (height 107, ext balance 3.25000000)
```

Passed twice (before and after the review fixes).

## 3. pytest harness

`cd tests/regtest/harness && .venv/bin/python -m pytest -v` (Python 3.11.4, pytest 8.3.4)

Docker backend (default):

```
tests/test_harness.py::test_local_backend_reports_dashd_exit_quickly PASSED [ 20%]
tests/test_harness.py::test_wrong_rpc_credentials_raise_http_401 PASSED  [ 40%]
tests/test_smoke.py::test_node_is_fresh_regtest PASSED                   [ 60%]
tests/test_smoke.py::test_p2p_port_accepts_inbound_peers PASSED          [ 80%]
tests/test_smoke.py::test_mine_fund_and_balance PASSED                   [100%]
============================== 5 passed in 28.00s ==============================
```

Local backend (`DWD_REGTEST_BACKEND=local DASHCORE_DIR=<darwin release>`):

```
============================== 5 passed in 12.26s ==============================
```

History: the smoke file (3 tests) passed 2× on Docker and 4× on the local backend before the
final run. The first local run failed `test_mine_fund_and_balance`: `getbalances` was read before
the recipient wallet had processed the mempool transaction (dashd log showed `AddToWallet` after
the RPC). The test now waits for the pending balance. After each run, no `dwd-regtest-*`
containers and no harness-started dashd processes were left behind.

## 4. Masternode network (functional test_framework from release binaries)

`dwd_mn_chainlock.py`: 6 nodes (controller, 1 plain wallet node, 4 masternodes).

| Run | Where | Result | Time |
|---|---|---|---|
| 1 | Docker | FAIL — test bug: wallet RPC on node 5, which is a masternode (no wallet). Simple nodes come first in `DashTestFramework`; fixed to node 1. | 27 s |
| 2–4 | Docker | PASS | 22 s, 24 s, 27 s |
| 5 | macOS host, darwin binaries (`functional/run.sh`) | PASS | 28 s |
| 6 | Docker, final (after review fixes) | PASS | 43 s |

Final Docker run, abridged:

```
Deterministic MN list: {'cdeb5072…-1': 'ENABLED', '240aa8af…-1': 'ENABLED', 'ea37fbda…-1': 'ENABLED', '8a737297…-1': 'ENABLED'}
h(228) quorums: {'llmq_test': ['79a1f530…'], 'llmq_test_dip0024': ['79a1f530…', '4d9d9bdb…'], 'llmq_test_platform': []}
Best ChainLock on plain node: height=237 hash=6ff8ef560930de6439ec02b472ceea689cfff0ddc3e04bdb55d9d5f3a116e9f7
Plain node sees tx 3f01da01…50a4: confirmations=0 instantlock=True
Masternode network: ChainLocks and InstantSend verified
Tests successful
```

## 5. CoinJoin mixing on regtest

`dwd_coinjoin_probe.py`: controller, 3 mixing dashd wallets (9.9 DASH each, `-coinjoinrounds=2
-coinjoinamount=4 -coinjoinsessions=1`), 4 masternodes; quorums mined first.

| Run | Where | Result | Time to first mixed outputs | Mix transaction |
|---|---|---|---|---|
| 1 | Docker | FAIL — funding stayed `untrusted_pending`: with ChainLocks on and no InstantSend quorum, miners hold unlocked txs for 10 min. Fixed by mining quorums first. | — | — |
| 2 | Docker | PASS | 31 s | 4 in / 4 out, inputs from wallets 1+2 |
| 3 | Docker | PASS | 21 s | `a604b982…89ac` 3 in / 3 out of 0.10000100, wallets 1+2+3, confirmations=2, IS + CL |
| 4 | Docker | PASS | 34 s | `4c819295…b805` 2 in / 2 out of 0.10000100, wallets 2+3, IS + CL |
| 5 | Docker | PASS | 44 s | `8ea0fcdd…4c67` 2 in / 2 out of 0.10000100, wallets 1+3, IS + CL |
| 6 | macOS host | PASS | 33 s | `6e98c64e…9b01` 4 in / 4 out of 1.00001000, wallets 2+3, IS + CL |
| 7 | Docker, final | PASS | 30 s | `c2448fad…8da1` 6 in / 6 out of 1.00001000, wallets 1+2+3, IS + CL |

**Conclusion: real CoinJoin mixing sessions run on regtest with 4 masternodes and 2–3 dashd
participants.** Sessions start after the 30 s queue timeout with 2 participants (min participants =
2 on regtest). This is the basis for the WS-06 interop suite (our client as one participant, dashd
wallets as the others).

## 6. Upstream `feature_llmq_chainlocks.py` (unmodified)

| Run | Where | Result |
|---|---|---|
| 1 | Docker | FAIL at line 252: a newly added masternode already had a ChainLock (`assert_raises_rpc_error(... "Unable to find any ChainLock" ...)`), i.e. the CLSIG reached it before the assertion — a timing race |
| 2 | Docker | test PASSED (`Tests successful`), but `docker compose run` returned 125 because the OrbStack daemon dropped the connection (`error waiting for container: unexpected EOF`) and restarted |
| 3 | macOS host | FAIL: `wait_for_quorums_list` timed out in `mine_cycle_quorum` |
| 4 | Docker, `--timeout-factor=3` | PASS |

Two of four runs passed the test. Both failures are timing failures under a load average of 160–980
on 14 cores. Neither looks like a release-binary or harness problem: the same framework code paths
(DKG, quorum list, ChainLocks) pass on every run of our own tests. This is not verified on an idle
machine.

## 6a. Governance suite (`functional/dwd_governance.py`, M3 R2, 2026-10-06)

macOS host, darwin release, host-built debug `dwcli`, `--portseed=4712 --timeout-factor=3`.

| Run | Result | Cause / fix |
|---|---|---|
| 1, 2 | FAIL in setup | test bugs: the `skip_test_if_missing_module` override dropped `skip_if_no_wallet()` (no wallet loaded); `register_fund` pays the collateral from its funds address only |
| 3–7 | FAIL at the vote step: "no voting masternodes" | engine: the wallet's masternode had no collateral. platform-wallet reloads chainlocked provider records as bare txids in a new process, and `register_fund`'s collateral hash is null (the ProRegTx pays its own collateral). Fixed: collaterals also come from the history store, a null hash is the proTxHash. Run 4 also hit the known `mine_cycle_quorum` flake (one quorum for both indices); the test now mines one more cycle when that happens |
| 8 | **PASS**, 1 min 42 s | create (IS-locked collateral, `OP_RETURN` = object hash), resume in a new process (`Ready`, 6 confirmations), submit (node 0 lists it), govsync (1 object, 4 votes), our vote accepted by node 0 under the collateral with the hash dwcli computed, tally 5Y/0N/0A = node 0's `FundingResult`, `next`/`last`/budget = `getgovernanceinfo` |

The fifth masternode runs no node, so it is PoSe-banned after the next DKG and counts in neither
"eligible" nor "controlled" (dash-qt skips banned entries); its vote still counts, as in Core.

## 7. Not verified

* Windows: nothing was run. There is no Windows dashd path in the harness yet.
* The x86_64 Linux image ran only under Rosetta emulation, and only for a single-node smoke check.
  The functional tests were not run on amd64.
* `x86_64-apple-darwin` binaries: hash pinned, not executed.
* An SPV client (`dwcli`/dash-spv) talking to these nodes: not attempted; `dwcli` does not exist yet.
  Only a raw TCP connection to the published P2P port was checked.
* GPG verification of `SHA256SUMS.asc`: the file's signature was not checked. The pinned hashes
  were copied from it as downloaded over HTTPS from GitHub.
