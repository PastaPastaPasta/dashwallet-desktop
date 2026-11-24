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
| Dash Core binaries | **24.0.0** (the final release, 2026-11-05) | `https://github.com/dashpay/dash/releases/tag/v24.0.0` |
| SHA-256 | `x86_64-linux-gnu` 1ffce395…2c5e, `aarch64-linux-gnu` 77783b23…b1cb, `arm64-apple-darwin` 93c69899…1e16, `x86_64-apple-darwin` e24380ba…a950, `win64` dc459ae5…9219 | release `SHA256SUMS.asc`; full values in `scripts/fetch-dashcore.sh` |
| test_framework | tag `v24.0.0` = commit `938fba502824b9d850b7c710601456dc3d71603e`, plus one local patch (below) | `test/functional/test_framework/` (MIT, `functional/test_framework/COPYING`; `authproxy.py` is LGPL-2.1+, see below) |
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
history. It is skipped unless `DWCLI` points at a built `dwcli`. The commands in this section run
from `regtest/harness`; `DW_TARGET` is the cargo target dir (`rust/target` unless `CARGO_TARGET_DIR`
says otherwise):

```sh
(cd ../../rust && cargo build -p dwcli)
DW_TARGET=${CARGO_TARGET_DIR:-$PWD/../../rust/target}
DWCLI=$DW_TARGET/debug/dwcli .venv/bin/python -m pytest -v tests/test_l1_sync.py
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
DWD_COMPOSE_PROJECT=dwd-e2 DWD_REGTEST_BUILD=0 DWCLI=$DW_TARGET/debug/dwcli \
    .venv/bin/python -m pytest -v tests/test_l1_send.py
```

**l2-tools** (`tests/test_l2_tools.py`, M2 R1) drives the same `dwcli` through the engine tools: a rescan
from a height below a wallet's birth finds a payment the wallet missed; a payment dashd refused (mempool
conflict with a transaction the test submitted, so never relayed) is abandoned and its coin comes back through
the abandon's rescan; an unmined payment is resent; a watch-only wallet from the account xpub sees the same
funds and cannot send or sign; the fee policy is the SPV minimum-relay answer and dash-qt's coin-control
estimate; console commands with Core result shapes, grants and redaction; one batched `NewTransactions`
event for three payments in a block; Close/Open wallet with the load-on-startup list. Two payments wait out
dash-spv's 60 s acceptance timeout, so the suite takes about 4 minutes.

```sh
DWD_COMPOSE_PROJECT=dwd-r1 DWD_REGTEST_BUILD=0 DWCLI=$DW_TARGET/debug/dwcli \
    .venv/bin/python -m pytest -v tests/test_l2_tools.py
```

**move-mixed** (`tests/test_move_mixed.py`, 0.2 MIX-SHIELD) funds 60 addresses of a wallet's CoinJoin account
directly (no masternodes here, so no mixing), consolidates them onto one fresh address of the same account
(`consolidate-mixed`: the move plan still finds the value there, as one coin), then moves that coin to a BIP44
receive address (`move-mixed`), after which the account has nothing to move. It needs a `dwcli` built with the
test-only `mix-shield-test` feature (`coinjoin-test-addresses`; debug builds only) and is skipped otherwise.

```sh
(cd rust && cargo build -p dwcli --features mix-shield-test)
DWD_COMPOSE_PROJECT=dwd-mm DWD_REGTEST_BUILD=0 DWCLI=$DW_TARGET/debug/dwcli \
    .venv/bin/python -m pytest -v tests/test_move_mixed.py
```

**restore** (`tests/test_restore.py`, DESIGN-opus §4.3 `restore` and §4.4 items 1-2) checks dash-qt
compatibility both ways with a host-built `dwcli`:

* dashd v24 descriptor wallets (a plain one, an encrypted one with a non-ASCII BIP39 and wallet
  passphrase, a weak-checksum phrase with a 256-byte passphrase), funded including an address 25
  past the last used one and change, are restored through the phrase, the `wallet.dat`, the
  `dumpwallet` of a legacy wallet with the same phrase and `listdescriptors true`; every path gives
  the same wallet id, dashd's first 1000 receive and change addresses, its balance and labels;
* a dwcli wallet is rebuilt in dashd from its phrase (`upgradetohd`), its `dumpwallet` export
  (`importwallet`) and its `importdescriptors` export; addresses and balance match;
* PSBT: dwcli "Create Unsigned" → dashd `walletprocesspsbt` → dwcli broadcast; dashd
  `walletcreatefundedpsbt` → dwcli sign (same signatures as dashd) → dashd `finalizepsbt`.

dashd v24.0.0 takes a 249- to 256-byte passphrase only with `-allow-deprecated-long-mnemonics`, which
the suite passes to its dashd. A passphrase over 256 bytes, which only 23.0.0 to 23.1.x took, is
covered by `rust/crates/dw-vault/tests/core_v23_long_passphrase.rs` against a vector from v23.1.10.
Not covered yet: BDB `wallet.dat` (M6), Core-mixed CoinJoin funds.

```sh
DWD_COMPOSE_PROJECT=dwd-r2 DWD_REGTEST_BUILD=0 DWCLI=$DW_TARGET/debug/dwcli \
    .venv/bin/python -m pytest -v tests/test_restore.py
```

The node helpers `node_path`, `read_file` and `write_file` move files in and out of the node's
datadir (through `docker compose exec` for the tmpfs datadir of the Docker backend).

The harness has no dependency beyond pytest: `dwd_regtest/rpc.py` is a small stdlib JSON-RPC client
that decodes amounts as `Decimal` and sends them as strings.

## DashPay suites (T2, T3)

`harness/tests/test_dp*.py` drive DashPay through `dwcli` only (ROADMAP E0-09) on a network with
Platform: testnet (DASHPAY §6 T3) or a dashmate devnet (T2). `dwd_regtest/dashpay.py` holds the
harness: one fresh wallet on its own fresh data root per member (its own device), JSON commands,
`dwcli dashpay session --spv` for everything that needs SPV running, and a funder wallet restored
from a phrase file outside the repository.

**Secrets.** Phrases and passphrases never go on a command line or into a file the harness
writes: `dwcli import` reads the phrase from a pipe; of `dwcli create`'s stdout only the
validated public fields are kept (one `wallet_id`, one well-formed `mnemonic` line, whose phrase
is registered before stderr is masked), and when its output is malformed or only partly matches,
both streams are withheld; a session's answers are decoded and then masked before the harness
sees them, and its stderr is masked across lines (a phrase split over several lines is masked
before any of it is written); every other dwcli output is masked before it is written or raised.
Secret files are read by a bounded first line, checked on the descriptor they are read from; one
that is not UTF-8 is an error that quotes none of it. Their `repr` is a mask, so a traceback
with locals (`-l --tb=long`) shows none. The harness sets its core-dump limit to zero (dwcli
inherits it), so a crash dumps no secret. After pytest has written its junit file and summary, a
scan reads the logs directory, the junit file and the files `DW_DP_SCAN` names for any secret of
the run (the whole value, or four phrase words in a row, in any case, whatever non-letters or
JSON escapes separate them, also after decoding JSON escapes; the masking hides the same); a hit,
or a file it cannot read (a symlink or anything but a regular file counts as unreadable), fails
the run, and only a clean scan writes `secret-scan.ok` into the logs directory. The paths it
prints are masked too. With `DW_DP_NET` set the scan runs whichever tests ran, even when none of
them used the harness, and even when the harness could not be set up (it then scans the junit
file and `DW_DP_SCAN`'s files for the secrets it had read, and the run fails).
`tests/test_dp_harness.py` checks all of this, and the configuration rules below, without a
network.

| Suite | ROADMAP | What |
|---|---|---|
| `test_dp1_identity.py` | DP1-07 | availability, offline name rules, a registration from the Core balance to `done`, its funding transaction's debit and the wallet's balance change equal, above the lock and within the grant; the identity, its name and keys, the name taken and resolved, the main name after a sync pass and a restart, a restore that finds the identity |
| `test_dp2_contacts.py` | DP2-06 | the request and accept round trip between two members (A's channel turning ready while its session stays open), the contact in a new process, ignore and unignore (hidden as `ignored` through a pass and a restart; back as a request on the next sweep), hide and show, a restore whose contact (ready) is listed while SPV can reach no peer, before any block is scanned, then the first scan attributing a payment to it; the one-way contact is skipped until a pin bump (E0-14) brings platform #4740: the pinned platform-wallet registers a receiving account when a request is sent, but discovery builds one again only for established contacts, so a restore loses a sent-only request's |
| `test_dp3_payments.py` | DP3-05 | contact payments both ways, a payment with no peer (reported as `send.no_peers`, definitely not sent: no lock; the next payment is made only after exactly that), an unknown outcome through a relay that holds the transaction (the contact locked to its txid, the held bytes hashing to it, a second payment refused, `resolve-lock` settling it `Sent` once the relay passed them on), 25 cancelled confirms past the 20-address gap, each naming the same address, then a payment the recipient sees, whose output in the recipient's `dwcli history` is that address (also when it settled through its lock) |

| Variable | Meaning |
|---|---|
| `DW_DP_NET` | `testnet` or `devnet` (a dashmate local network; the harness passes `--network regtest`); the suites are skipped when unset, and any other value is an error |
| `DW_DP_NET_ARGS` | extra network options, only `--dapi`, `--ca-cert`, `--quorum-url` and `--peer` (devnet: `--dapi … --ca-cert … --quorum-url … --peer …`); `--network` and anything naming mainnet are refused |
| `DW_DP_STATE` | absolute directory outside the checkout, owned by you, mode 0700 (created so; refused when open to group or others): the funder's data root, the vault passphrase `pass` (0600, refused otherwise), `runs/` and `logs/` |
| `DW_DP_FUNDER`, `DW_DP_FUNDER_BIRTH` | the funder's phrase file (owner-only, 0600 or 0400; refused otherwise) and birth height; on agentbox the phrase is the secret store's `dashwallet-desktop/dashpay-suites-funder`. A funder wallet already in `DW_DP_STATE` must be this phrase's (its wallet id is compared with the phrase's, imported into a scratch root of the run); another phrase is refused: use a new `DW_DP_STATE` |
| `DW_DP_LOGS` | absolute directory for the dwcli logs, 0700 (default `DW_DP_STATE/logs/<run>`) |
| `DW_DP_SCAN` | colon-separated extra files for the end-of-run secret scan |
| `DW_DP_STRICT` | `1`: the unknown-outcome case fails instead of skipping when the relay is not configured |
| `DWCLI` | the dwcli binary |
| `DW_DP_NOTIFY` | nightly only: the absolute path of the site's notifier, an executable that delivers its one argument (a failure message, or `<unit>: result=success` after a run that passed) |
| `DW_DP_ISSUE_REPO` | nightly only, for the issue notifier `scripts/dashpay-nightly-issue.sh`: the repository (`owner/name`) its issue lives in. That notifier also needs `gh`, signed in with issue access to that repository, and `jq` |
| `DW_DP_HOLD_UPSTREAM` | `ip:port` of a testnet peer serving BIP157 filters, for the unknown-outcome relay (DP3); that case is skipped when unset (fails with `DW_DP_STRICT=1`) |

The harness assumes `DW_DP_STATE` is private to it: mode 0700, owned by the user that runs the
suites, on one filesystem, and with no other process of that user working in it while a run
does. It refuses a state root that is a symlink, open to group or others, owned by someone
else, or a git checkout or inside one; it does not defend against a process of the same user changing the tree under a run.

Each run's member data roots live in `DW_DP_STATE/runs/<run>`, which holds a `.dw-dp-run` marker
naming the run. A run deletes the marked run directories older than three days; it refuses a
symlinked `runs`, never follows a symlink, and leaves any directory without its marker alone.

```sh
(cd ../../rust && cargo build -p dwcli)
DW_DP_NET=testnet DW_DP_STATE=/work/scratch/dw-suites/t3 \
  DW_DP_FUNDER=~/.config/dashwallet-desktop/dashpay-suites-funder.mnemonic DW_DP_FUNDER_BIRTH=1594390 \
  DWCLI=$DW_TARGET/debug/dwcli .venv/bin/python -m pytest -v tests/test_dp*.py
```

The funder keeps a dozen mined coins of 0.5 tDASH and splits its balance again when fewer than
four are left: a restarted dwcli no longer knows its own change's InstantSend lock, so change is
spendable only once mined. It asks again only when `dwcli send`'s last line reports
`outcome=not_sent` for want of a mined coin or a peer; a send whose outcome is unknown keeps its
txid and is waited for, never paid again; each attempt syncs to the current tip first. Its
result is the one complete terminal record on the last line (`broadcast <txid>` or
`failed …`), whose txid must be the prepared one; anything else stops the run without sending
again. The split waits for that transaction's own outputs to be mined. A payment
to a member counts once the member's wallet holds that txid with one confirmation (`dwcli sync --txid … --confirmations 1`) and credits it with the amount. A contact payment's failure is read from dwcli's JSON:
an error whose code is `send.<kind>` (one per `SendFailure` variant) and whose `params.kind`
names the same kind; `pay-contact` asks again only for `no_peers` with `params.outcome` `not_sent`
(or while the wallet rescans), and pays again after an unknown outcome only once
`contact resolve-lock` answers `not_sent`, two payments at most; every unknown outcome is resolved
through its lock before anything fails. Top it up with the `dash-faucet` skill when its balance
runs low. A run costs about 0.1 tDASH per registered member.

**Nightly (T3).** `scripts/dashpay-nightly.sh` builds dwcli at `origin/main` in its own checkout
under `DW_DP_STATE` (with `umask 077`, under a `flock` on `DW_DP_STATE/nightly.lock`, with
`CARGO_TARGET_DIR` unset and the binary path taken from that build) and runs every
`tests/test_dp*.py` on testnet with `DW_DP_STRICT=1`. It needs `DW_DP_HOLD_UPSTREAM` and
`DW_DP_NOTIFY`, fails when no suite matches, refuses any `DW_DP_NET` but `testnet`, and dumps no
core. Before it writes anything in the state root it checks it at its canonical path: absolute,
not a symlink, the user's own, 0700, no git checkout and inside none, and not where the script
itself is installed; the lock, the run directory and the checkout must not be symlinks. The
fetch, the build and the dependency install run code of the fetched revision, so they run before
the funder's variables are exported to anything and through `scripts/dashpay-unfunded.sh`: in a
bubblewrap mount namespace where the state root holds only the checkout and the funder's phrase
file reads empty. The launcher checks that at every run and fails without it; there is no
fallback. It refuses a funder file named by a relative path or with a second hard link,
which the sandbox could not hide. The sandbox hides files. It does not confine a build that asks this user's service
manager or a container daemon to run something outside it, and the suites then run the fetched
revision with the funder: the funder must be a dedicated testnet wallet with a small capped
balance, never one holding anything else.
pytest's output goes to `pytest.log` only; the script's log and the journal get its summary lines
once the secret scan has passed. Everything stays in `DW_DP_STATE/nightly/<date>`.

On agentbox it runs from the systemd user timer in `ci/agentbox/`, with the launcher,
`scripts/dashpay-unfunded.sh`, `scripts/dashpay-nightly-notify.sh`, `scripts/dashpay-nightly-issue.sh`
and `ci/github/nightly-issue.sh` installed outside `DW_DP_STATE` and outside any checkout
(`~/.local/libexec/dashwallet-desktop/`; install steps in the service file). It does not gate
anything; it alerts. A failed run fails the unit, and its `OnFailure=` unit
(`dw-dashpay-nightly-failed.service`) runs the site-provided notifier `DW_DP_NOTIFY` with the
unit's result and exit status and the run's artifact directory, never test output. The install
requires one test start of that unit, with its message seen to arrive, before the timer is
enabled. After a run that passed, the unit runs the notifier once more with `result=success`.

On agentbox the notifier is `scripts/dashpay-nightly-issue.sh`: it keeps one open issue labelled
`nightly-failure` ("Nightly failure: DashPay suites on testnet") in `DW_DP_ISSUE_REPO` while the
nightly is red, adds a comment for each further red night, and closes it after the next green
one. The issue is public, so it names the artifact directory by the launcher's name for it only, and publishes nothing from a notice of any other form. The scheduled
GitHub Actions nightly reports the same way ("Nightly failure: GitHub Actions", with links to
the failed jobs), through the same `ci/github/nightly-issue.sh`.

**T2 is blocked.** On the shared dashmate `local` devnet (3 evonodes) dash-spv never finishes
its masternode list: the network forms no `llmq_test_dip0024` quorum, so QRInfo is never
answered, `caught_up` never comes, and every Platform write waits on `sync` (DP6-01 D11, MG-07
D17, BL-316). The suites run there unchanged (`DW_DP_NET=devnet`) once the devnet has rotating
quorums.

## Dash Core functional test_framework from release binaries

### What is vendored

`functional/test_framework/` is a copy of `test/functional/test_framework/` at tag `v24.0.0` (27
top-level files, including the CSV test vectors, and 11 in `crypto/`), with the repo's `COPYING`
added and one local patch. `diff -r` against the tag's tree shows only the added `COPYING` and the
patch:

* `test_framework.py`, `mine_cycle_quorum`: before mining the final-commitment block, wait for both
  quorums' final commitments on the members and on the mining node. This ports Dash Core
  `73a69fa5b9b` (dashpay/dash#7892), which is not in v24.0.0; without it a commitment that hasn't
  reached the miner leaves the new quorum out of the list. Drop the patch when the vendored tag
  contains that commit.

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

# on the host with the matching release: Linux (x86_64/aarch64) or macOS (ad-hoc signed by
# fetch-dashcore.sh); building dash_hash needs a C compiler
sh regtest/scripts/fetch-dashcore.sh ~/.cache/dwd-dashcore/24.0.0
uv venv ~/.cache/dwd-dashcore/func-venv --python 3.12 && uv pip install --python ~/.cache/dwd-dashcore/func-venv/bin/python \
    https://github.com/dashpay/dash_hash/archive/refs/tags/1.4.0.tar.gz
DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 \
    regtest/functional/run.sh dwd_mn_chainlock.py
```

`run.sh` puts config.ini, the framework cache and each run's node datadirs (`--tmpdir`, unique per
run) under `DWD_FUNC_TMP` (default `$TMPDIR/dwd-functional`), all created owner-only (`umask 077`, and the
scratch root is reset to 0700): dwcli's engine refuses a database below a group- or world-writable
directory that is not sticky, so `DWD_FUNC_TMP` must not sit below one either. The `functional` service bind-mounts
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
  upstream `rpc_verifychainlock.py` does. Neither was run here. At v24.0.0,
  `mine_quorum_single_member()` first mines until the masternodes are confirmed for the next DKG,
  and syncs blocks while it mines.
* **Mining gate**: with ChainLocks enabled, block templates only include a transaction that has an
  InstantSend lock or has been in the mempool for 10 minutes (`WAIT_FOR_ISLOCK_TIMEOUT` in
  `src/test/miner_tests.cpp`). Without an InstantSend quorum, wallet payments stay in the mempool and
  balances stay `untrusted_pending`. Mine quorums before funding anything.
* The framework runs every node with mocktime that starts near the regtest genesis time (2014) and
  advances only via `bump_mocktime`. An external client using the wall clock (e.g. `dwcli` speaking
  P2P to these nodes) sees old block timestamps; set `self.disable_mocktime = True` in such tests if
  that matters.
* P2P ports inside the container are `11000 + n + (20 * portseed) % 9979` (`util.p2p_port`); pass
  `--portseed=<k>` to make them deterministic. At v24.0.0 the framework never hands out p2p port
  19896 (dashd's regtest onion-service target) and uses 20999 instead (`util._port`). RPC listens
  on 127.0.0.1 only. The intended way to
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

### Suite `coinjoin` (our client, M3 R1)

`functional/dwd_coinjoin_client.py` runs `dwcli` (set `DWCLI`) against 4 masternodes, two dashd
mixing wallets and a second `dwcli` wallet, in real time (`disable_mocktime`: our client checks
`dsq` timestamps against the wall clock). It checks that stopping releases a session's coins, one
round, that a restarted process resumes to 2 rounds with a fully mixed balance, and that fully mixed
coins pay only through `send --coinjoin` (no change output, `CoinJoinSend` in history). The `dwcli`
nodes connect to the framework's ports on the host, so it runs on the host with the host's release,
set up as in "Running" above (the Docker image would need a Linux `dwcli` inside the container):

```sh
(cd rust && cargo build -p dwcli)    # from the repo root
DWCLI=${CARGO_TARGET_DIR:-$PWD/rust/target}/debug/dwcli DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 regtest/functional/run.sh dwd_coinjoin_client.py --portseed=<n>
```

dashd counterparties alone are not enough on this network: after their first sessions they keep the
masternode connections they opened (`CMasternodeUtils::DoMaintenance` keeps them while the node has
fewer than its maximum outbound peers) and skip every connected masternode
(`IsMasternodeOrDisconnectRequested`), so most queues expire. The second `dwcli` wallet keeps
sessions going. One pass takes 8–15 minutes on the laptop (quorums about 2, mixing 5–12) and about
6–7 minutes on agentbox.

### Suite `masternodes` (MG-03)

`functional/dwd_masternodes.py` runs `dwcli mn` (set `DWCLI`) against Core's deterministic
masternode list: a masternode registered with an external collateral the wallet funds and signs
for, two more matched by owner key or payout, tracking and attached keys, payouts against Core's
coinbase outputs, and `enabled` → `pose_banned` (revoked) → `removed` (collateral spent). DKG stays
off, so the masternodes it registers run no node; before the first sync it mines whole DIP24
cycles until Core answers `quorum rotationinfo`, which dash-spv's list sync needs (MG-03 D16). Run
it on the host like `coinjoin`:

```sh
DWCLI=${CARGO_TARGET_DIR:-$PWD/rust/target}/debug/dwcli DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 regtest/functional/run.sh dwd_masternodes.py --portseed=<n>
```

`DWD_MG03_EVIDENCE=<dir>` also writes the list, A's payouts and its removed detail as JSON. One
pass takes about 3 minutes on agentbox.

### Suites `governance`, `protx` and `shared` (MG-02, MG-04, MG-04b, MG-06, MG-08)

`functional/dwd_governance.py`, `functional/dwd_protx.py` (plain and `DWD_MG04_V24=1`) and
`functional/dwd_shared.py` run `dwcli` on the host the same way:

```sh
DWCLI=${CARGO_TARGET_DIR:-$PWD/rust/target}/debug/dwcli DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 regtest/functional/run.sh dwd_governance.py --portseed=<n>
DWCLI=${CARGO_TARGET_DIR:-$PWD/rust/target}/debug/dwcli DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 regtest/functional/run.sh dwd_protx.py --portseed=<n>
DWCLI=${CARGO_TARGET_DIR:-$PWD/rust/target}/debug/dwcli DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 regtest/functional/run.sh dwd_shared.py --portseed=<n> --timeout-factor=4
```

### Suite `governance_ui` (MG-09)

`functional/dwd_governance_ui.py` is the Governance tab's acceptance on the real app: 3 masternodes,
one of them voting with the app wallet's DIP3 voting key 0 (`protx update_registrar`), a proposal
submitted by Core, and the wallet holding one 1,000 DASH coin. It runs
`apps/desktop/e2e-tauri/regtest-governance.ts` (tauri-driver), which turns governance on, votes Yes
and creates a proposal from the UI, then checks on every node that the vote and the proposal are
Core's. The app reads masternodes' ProRegTxs from dashd through `--collateral-rpc` (MG-09 D28).
`apps/desktop/scripts/e2e-regtest-governance.sh` builds the app and dwcli and runs it; it needs a
private X display, WebKitWebDriver and tauri-driver:

```sh
DISPLAY=:<n> TAURI_DRIVER=<tauri-driver> DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 apps/desktop/scripts/e2e-regtest-governance.sh
```

Screenshots, the app's log and `result.json` go to `DW_E2E_OUT`. One pass takes about 4 minutes on
agentbox once built.

### Suite `governance_added_key_ui` (MG-10 §1.4)

`functional/dwd_governance_added_key_ui.py` is the same network with a masternode the wallet did not
derive: Core makes a fresh voting key in a legacy wallet of node0 (`getnewaddress`, `dumpprivkey`)
and gives it to masternode 2 (`protx update_registrar`), then submits a proposal. It runs
`apps/desktop/e2e-tauri/regtest-vote-added-key.ts`, which tracks masternode 2 by ProTx hash, adds
the key in Add keys (passed by environment, typed only into the masked field, never in a capture),
checks the vote sheet lists it with "key you added", and votes Yes. Back in the suite, every node
must hold that masternode's Yes, and no file of the run or of the app's data may hold the key in
plain text. `apps/desktop/scripts/e2e-regtest-vote-added-key.sh` builds and runs it, with the same
needs as `governance_ui`:

```sh
DISPLAY=:<n> TAURI_DRIVER=<tauri-driver> DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 apps/desktop/scripts/e2e-regtest-vote-added-key.sh
```

### Suite `update_registrar_ui` (MG-10 S3)

`functional/dwd_update_registrar_ui.py` is Change keys or payout's acceptance on the real app
(MG-10 S3's GL `--part update`). The network is `dwd_protx.py`'s. dwcli registers masternode F
(`FundNew`, Core's operator key, the keychain's owner key 0), so the wallet holds F's registration.
The suite then runs `apps/desktop/e2e-tauri/regtest-update-registrar.ts` (tauri-driver), which opens
Change keys or payout on F, sets the keychain's next voting key and Core's address as the payout,
and sends. Back in the suite, `protx info` must show that payout and voting key 1, with the owner
and operator keys unchanged. `apps/desktop/scripts/e2e-regtest-update-registrar.sh` builds and runs
it, with the same needs as `governance_ui`:

```sh
DISPLAY=:<n> TAURI_DRIVER=<tauri-driver> DWD_PYTHON=~/.cache/dwd-dashcore/func-venv/bin/python \
    DASHCORE_DIR=~/.cache/dwd-dashcore/24.0.0 apps/desktop/scripts/e2e-regtest-update-registrar.sh
```

## Layout

```
regtest/
├── README.md, RESULTS.md    this file; measured results
├── Dockerfile               image: python:3.12-slim + dash_hash + Dash Core release + functional/
├── docker-compose.yml       services: dashd (single node), functional (profile; runs one test script)
├── scripts/
│   ├── dashpay-nightly.sh   the DashPay suites on testnet, nightly (agentbox)
│   ├── dashpay-unfunded.sh  the nightly's build sandbox: no funder file, no state root
│   ├── fetch-dashcore.sh    download + SHA-256 verify + unpack (+ ad-hoc codesign on macOS)
│   ├── node-entrypoint.sh   dashd regtest command line used by the image
│   ├── regtest.sh           up/down/status/mine/fund/cli/wcli
│   ├── selftest.sh          end-to-end check of regtest.sh
│   └── run-functional.sh    run functional tests in the compose service, one summary line each
├── harness/                 pytest: dwd_regtest/ (rpc.py, node.py, dashpay.py), conftest.py,
│                            tests/test_smoke.py (node), tests/test_harness.py (failure handling)
└── functional/
    ├── run.sh               writes config.ini for release binaries, runs one test
    ├── test_framework/      vendored from dash v24.0.0 (MIT), one local patch
    ├── feature_llmq_chainlocks.py   upstream, unmodified
    ├── dwd_mn_chainlock.py  4 MNs → quorums → ChainLock + InstantSend
    ├── dwd_coinjoin_probe.py        4 MNs + 3 mixing wallets → real CoinJoin rounds
    ├── dwd_masternodes.py   dwcli mn against the MN list: roles, tracking, payouts, status
    ├── dwd_governance.py    dwcli governance / gov against 4 MNs: govsync, proposals, votes
    ├── dwd_protx.py         dwcli mn ProRegTx / ProUpRegTx owner flows (regular + v24)
    ├── dwd_shared.py        dwcli mn shared v24 registration, key change, standby, dissolution
    ├── dwd_governance_ui.py the desktop app votes and creates a proposal through its UI
    └── dwd_update_registrar_ui.py   the desktop app changes a masternode's payout and voting key
```
