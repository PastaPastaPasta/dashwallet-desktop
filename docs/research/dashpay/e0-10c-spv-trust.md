# E0-10c: dash-spv trust for Platform proofs

Status: implementation notes for DEC-168. Measured 2026-10-10 on agentbox; testnet and mainnet read-only.
Follows [E0-10a](trust-spike.md), whose §6 items 3 and 4 this closes. Item 5, chain-anchored lists
(dashpay/rust-dashcore#1117), is outside E0-10c (DEC-168 (4)).

| What | Revision |
|---|---|
| platform #5307 (v5.1 move, open) | `f475f72a11`, base v5.1-dev |
| rust-dashcore that #5307 pins | `8fe0a381` (head of rust-dashcore #1149, open) = `dev` `c19973ab` + 5 serde commits |
| rust-dashcore `dev` for the fix | `0eaf0284c` |
| fix, on `dev` (rust-dashcore) | branch `fix/spv-first-quorum-after-start`: `4c142bafc`, `9564a6b56`, `962654cbc` |
| fix, on `8fe0a381` | branch `fix/spv-first-quorum-after-start-on-1149`: `de7e1e04f`, `d3c9ad562`, `67770b9fc` (cherry-picks) |
| platform fork branch | `dw/e0-10c-v5.1-spv-trust` on `f475f72a11`: `ad601d1d8b` (#4978), `ebe37f8a67` (status lookup) |

None of these is pushed. The desktop pins below are the target once pasta approves the pushes.

---

## 0. Summary

- **The pin move costs 14 mechanical commits** (36 files, +158/−212 outside `Cargo.lock` and the workspace manifest),
  mostly the secp256k1 0.33 port, **plus a decision on PSBT**: rust-dashcore #1041 removed `key_wallet::psbt`, which
  `dw-psbt` is built on (§1).
- **DEC-18 picks:** #5307 already contains #4623, #4997, #4764, #5206, #5294 and #5305. Of the fixes the desktop gets
  from v5.0-dev today, only #4978 is missing; it cherry-picks cleanly onto #5307 (§2).
- **The first quorum after the start is fixed.** Root cause: a tip update never fetched the work-block list a new quorum
  needs; only the QRInfo path did, once per rotation cycle. A tip update now fetches it, and the quorum is `Verified` in
  the first list that carries it. Regtest: fails before, passes after. Testnet and mainnet, fix vs `dev` side by side:
  every Platform quorum that entered after the start was `Verified` on arrival with the fix (4/4 testnet, 3/3 mainnet,
  3/3 for a second testnet client on the final head); on `dev` the first one on each network stayed
  `Skipped(MissedList)` until the run ended, 3.3 h and 2.7 h later (§3.4).
- **The status-carrying lookup** is one additive method on platform-wallet's `SpvRuntime`,
  `get_quorum_public_key_with_status`, returning the key and dash-spv's `LLMQEntryVerificationStatus` (§4).
- **What the desktop graph needs for E0-10b to require `Verified`:** the fork pin (#5307 + #4978 + the status lookup)
  and the dash-spv fix, which #5307's rust-dashcore pin does not carry (§5).

---

## 1. Pin move: `bc41f1bc23` / `40268cc0` → #5307 `f475f72a11` / `8fe0a381`

Measured on `dw/e0-10c-spv-trust` (`aba0c63`..`74fa523`, one commit per cause). The lock was re-seeded from #5307's `Cargo.lock`, as R3 does
for every pin, which resolves the `zeroize` / `bitcoin_hashes` conflicts a plain re-pin hits.

### 1.1 Mechanical breaks

| Cause (upstream) | Crates | Lines (+/−) |
|---|---|---|
| The re-pin: platform #5307, rust-dashcore `8fe0a381`, base-sdk `dash-pkc` `e6402ced` | workspace manifest, lock | 17/12, lock 563/401 |
| Crate-local dashcore pins in dw-compat, dw-uri, dw-message (else two dashcore copies in the graph) | 3 manifests | 8/8 |
| #5307's platform-wallet uses `dpp::ed25519_dalek` without enabling it (fine inside platform's workspace, not for dependents) | workspace manifest | 4/1 |
| rust-dashcore #1042: secp256k1 0.33, no context argument, `Message` by value, `SecretKey::from_secret_bytes`, `RecoverableSignature`/`RecoveryId`/`SharedSecret` renames | dw-vault, dw-psbt, dw-message, dw-compat, dw-engine, dw-coinjoin (test) | 120/174 |
| rust-dashcore #1036: `blsful` replaced by `dash-pkc` behind `bls_sig_utils` | dw-coinjoin | 24/25 |
| rust-dashcore #1108: `dashcore::base58` is `base58ck` 0.5 | dw-uri | 3/2 |
| rust-dashcore #1056 / #1108: ed25519 via `dashcore::eddsa` (dash-pkc) | dw-engine | 3/3 |

dw-ffi, dw-appdb, dw-console, dw-desktop, dw-fs, dw-p2p, dw-units and dwcli needed no change.

Checked against the fork pin (`ebe37f8a67`, used through a local `file://` URL, not committed) with a type-only PSBT
stand-in: `cargo check` and `cargo clippy -D warnings` pass on the whole workspace. `cargo test --workspace`: every
failure is either a PSBT test hitting the stand-in (8 in dw-psbt, 3 in dw-engine `send::flow_tests`, 2 in dw-ffi) or
one of 4 wall-clock tests in dwcli (`dashpay_r3`, `dashpay_r5`, 10–60 s bounds) that failed at load average 250.
The wall-clock failures are load, not the move: `dashpay_r3` passes run serially, and desktop main's own binary misses
the same 10 s bound in `dashpay_r5` at that load (19.1 s and 10.9 s). Totals: 834 passed, 17 failed (13 stand-in, 4
wall-clock), 4 ignored. At the committed pin (`f475f72a11`, no #4978, `names_tests` disabled): 798 passed, 15 failed
(13 stand-in, 2 wall-clock).

### 1.2 Removed upstream without a replacement

**`key_wallet::psbt`** (rust-dashcore #1041, `65f11cd2e`, "drop the unused vendored PSBT implementation"). The whole
vendored BIP174 module is gone, and `8fe0a381` has nothing in its place. The desktop's `dw-psbt` is built on it, and
`dw-engine` send/psbt, `dw-ffi` and `dwcli` on `dw-psbt` (QT-076..079). Vendoring means 9 files, 4 091 lines from
`40268cc0` `key-wallet/src/psbt/` (plus 482 test lines and 13 vector files), ported to secp256k1 0.33 like the rest. The desktop
uses only the container: parse/serialize, `from_unsigned_tx`, `extract_tx`, and the input/output fields. It signs
through `dw-psbt`'s own `sign`, which allows only `SIGHASH_ALL`, not through the module's `sign`. Three import sites,
all in `dw-psbt`. Options:

1. vendor the container from `40268cc0` into `dw-psbt`, without the module's signing paths (the code #1041 cites as its
   reason; `dw-psbt` already signs `SIGHASH_ALL` only);
2. move `dw-psbt` to another PSBT implementation;
3. drop PSBT from 1.0.

Default: option 1, as its own task before the pin move merges; the manager decides. The measurement used an
uncommitted type-only stand-in.

### 1.3 Before this branch merges

The branch pins the plain #5307 head, an open PR's head that a force-push can drop, and that head lacks #4978 (§2).
Merging waits for:

- the re-pin to the fork branch (§5 step 1), once pasta approves the push;
- the PSBT decision (§1.2);
- the fixtures and vectors that name `bc41f1bc23` as the desktop's pin (`dw-engine/tests/fixtures/README.md`, the
  DP1-01 and DP1-03 vectors and their generators, `keys_policy.rs`, and the string `dp1_03_names.rs` asserts). Upstream
  identity key derivation and `identity_public_key` changed between the pins (4 files), so those vectors' rules need a
  re-check at the new pin, not only a new revision string. They pass at the fork pin today.

The port review found the signing paths equivalent: low-R signing, recoverable signatures, the compact-signature
header and the digests are unchanged. One behaviour change in BLS: the dash-pkc legacy decoder masks two flag bits in
the first byte, as Core's does, where blsful rejected them. It accepts a few more encodings of the same points. The new
test `verifies_with_the_legacy_form_of_the_operator_key` covers the legacy path. A known-answer vector from a real
DSQ or DSTX is still missing.

### 1.4 Not broken

No desktop crate calls `masternode_list_engine` (#1094) or the SPV runtime errors #5307 changed. platform-wallet,
dash-sdk and dpp needed nothing beyond the `dpp/ed25519-dalek` feature (#5307's platform-wallet uses it but relies on
workspace feature unification to enable it).

---

## 2. DEC-18 cherry-picks

Desktop main pins upstream `bc41f1bc23` and carries no fork patches. DASHPAY R3 planned #4623 + #4997 on a fork branch
until the v5.1 move; #5307 contains both, so they drop out.

| Fix | On v5.0-dev (`bc41f1bc23`) | In #5307 | Fork branch |
|---|---|---|---|
| #4623, #4997, #4764 | — (v5.1-dev only) | yes | — |
| #5206, #5294, #5305 | yes | yes | — |
| #4978 (keep the chosen DPNS name across wallet sync) | yes | **no** | cherry-pick `ad601d1d8b`, clean |
| status lookup (§4) | — | — | `ebe37f8a67`, once its upstream PR is public |

The other 17 v5.0-dev commits #5307 lacks are PV14 consensus rules, docs and CI; the desktop does not need them.
Without #4978, `dw-engine`'s `names_tests` stop compiling (`DpnsFetch` does not exist at #5307). Against the fork pin,
which carries #4978, all 38 compile and pass.

---

## 3. The first quorum after the start

### 3.1 Root cause

A non-rotating quorum is verified against the masternode list at its work block, `quorum height − 8`
(`QUORUM_MEMBER_LIST_OFFSET`). Once synced, dash-spv follows the tip with one `GetMnListDiff` per block, and asks for
missing work-block lists only after a QRInfo, which fires once per rotation cycle (288 blocks on mainnet and testnet).

The first Platform quorum committed after a start has its work block at or below the height the client synced to. The
client never built that list: it synced by QRInfo plus a few diffs, not block by block. So the tip update that brings
the commitment leaves the quorum `Skipped(MissedList(work block))` until the next QRInfo. That is E0-10a's 7–11 h. A
batched catch-up that jumps several blocks leaves the same gap.

### 3.2 Fix

When a tip update (a diff whose base is the newest list) applies, dash-spv now asks for the work-block lists of the
quorums that diff adds, and the update completes once they are applied. The quorum is re-verified in the same pass and
is `Verified` in the first list the client publishes with it. Each new quorum is asked for once. A work-block list
that fails does not hold back the tip update; a tip update that fails still publishes nothing.

Code: `dash-spv/src/sync/masternodes/sync_manager.rs` (MnListDiff handler) and `dash-spv/src/sml_engine.rs`
(`missing_work_block_list_requests_for`). It cherry-picks unchanged onto `8fe0a381`.

### 3.3 Tests

- Unit (`sync_manager.rs`): the work-block list is asked for and the quorum re-checked; a failed work-block list still
  completes; a rejected tip update publishes nothing. Mutation checks: removing the fix fails the first two; asking for
  every unverified quorum fails both; dropping either half of the completion guard fails one.
- dashd regtest (`dashd_masternode::test_quorum_mined_after_start_is_verified_on_arrival`): starts the client 4 blocks
  before a DKG cycle, mines the cycle, and records the quorum's status in every list the client publishes, at the
  moment it publishes it. On `dev`: `[(443, Skipped(MissedList(424)))]` (synced at 428). With the fix: passes.
- Suites on the final head: dash-spv lib 624/624; `dashd_masternode` 11/11. `dashd_sync` and dash-spv-ffi `dashd_sync`
  had 2–3 timeouts each at load 100–170 on 32 vCPU; both run with masternode sync off, so the change is not on their
  path. The `8fe0a381` backport: masternode unit tests 31/31, `dashd_masternode` 11/11 (regression test included).

### 3.4 Testnet and mainnet

E0-10a's dash-spv probe (`tools/trust-spike/`, the `spv` mode fed by the pin's `capture` tuples), read-only, started
side by side at 12:33 UTC on 2026-10-10 for 3.5 h: `dev` `0eaf0284c` ("ctl") and `dev` + the fix's first commit
`4c142bafc` ("fix") on both networks. A third testnet client on the final head `962654cbc` ("fin") started at 13:12 and
ran to the same end. Data: `/work/scratch/e0-10c/probe/run1/` (agentbox, not tracked).

Platform quorums that entered the newest list after each client's initial sync:

| Client | Network | Arrived after the start | Status on arrival | Status at the end of the run |
|---|---|---|---|---|
| fix | testnet | 4 | 4 `Verified` | 4 `Verified` |
| ctl | testnet | 4 | first `Skipped(MissedList(1569760))`, 3 `Verified` | unchanged: the first stayed `Skipped` 3.3 h |
| fin | testnet | 3 | 3 `Verified` | 3 `Verified` |
| fix | mainnet | 3 | 3 `Verified` | 3 `Verified` |
| ctl | mainnet | 3 | first `Skipped(MissedList(2552944))`, 2 `Verified` | unchanged: the first stayed `Skipped` 2.7 h |

The first quorum after the start is the case under test in every client. ctl and fix synced to 1569778 (testnet) and
2552953 (mainnet); the first quorum's work blocks are 1569760 and 2552944. fin synced to 1569792; its first quorum's
work block is 1569784. The quorums after the first are `Verified` in both builds: by then the client holds a list for
every block.

Every live tuple found once synced was `Verified` on first ask, in all clients (52–53 per client). Platform did not cite
the first quorum while ctl held it `Skipped`, as in E0-10a §4.2, so the lookups do not show the difference. On mainnet
one tuple, cited at ChainLock height 2532096 (about 20 800 blocks below the tip, a stale evonode, E0-10a §2), was not
found 120 times in both ctl and fix. During initial sync each client had a lookup answered `Skipped(MissedList)` that
was `Verified` seconds later (one in fix testnet, one in ctl mainnet).

---

## 4. A lookup that returns the status

E0-10a §6 item 4: `SpvRuntime::get_quorum_public_key` returns only the key, from any entry that is not `Invalid`.
`SpvRuntime` keeps its client private, so `trust.rs` cannot require `Verified`.

```rust
pub struct SpvQuorumPublicKey {
    pub public_key: [u8; 48],
    /// Never `Invalid`: the lookup skips invalid entries.
    pub status: LLMQEntryVerificationStatus,
}

impl SpvRuntime {
    pub async fn get_quorum_public_key_with_status(
        &self, quorum_type: u32, quorum_hash: [u8; 32], height: u32,
    ) -> Result<SpvQuorumPublicKey, PlatformWalletError>;
    // `get_quorum_public_key` keeps its signature and delegates to it.
}
```

Additive: no signature changes. A struct, not a `require_verified` flag: the trust policy stays in `trust.rs`, and the
`Skipped` reason stays available for the Tools ▸ Information card (DP6-03). Nothing else in the platform repo calls the
method (FFI, wasm and the context providers do not wrap `SpvRuntime`), so it is the only change needed.

Test (`spv::runtime::tests::should_return_the_quorum_status_with_its_key`): seeds dash-spv's engine through its
`test-utils` feature (dev-dependency only) and checks `Unknown`, `Skipped(MissedList)` and `Verified` with a hash that
is not its own reverse, and that neither lookup returns an `Invalid` entry. Removing the byte-order reversal fails it.
platform-wallet `spv::runtime`: 31/31; clippy clean.

DEC-18 allows the fork to carry this only as a cherry-pick of a public upstream PR, so the order is: the manager
approves the PR text, the PR opens against v5.1-dev stacked on #5307 (its test needs #5307's rust-dashcore), then the
fork carries the commit.

---

## 5. Route to the desktop

1. Push `dw/e0-10c-v5.1-spv-trust` to `PastaPastaPasta/platform` (after approval and the item-4 PR) and pin every
   platform crate to `ebe37f8a67` on the fork. This brings #1094, #1073 and #1102 (DEC-168 (1)), #4978 and the status
   lookup.
2. The dash-spv fix is not in `8fe0a381`. Until it merges upstream and platform re-pins, the desktop graph needs a
   `[patch."https://github.com/dashpay/rust-dashcore"]` to the `8fe0a381`-based branch (`67770b9fc`) on pasta's
   rust-dashcore fork. Publishing it is a manager decision (ROADMAP E0-10c, DEC-09).
3. The PSBT decision and the vector re-check (§1.2, §1.3) gate the pin move merging.
4. E0-10b's `trust.rs` then requires `status == Verified`. Enforcement (DP3-04 / H-09 full mode) still waits for #1117.

## 6. Open questions (defaults in force)

| Question | Default |
|---|---|
| PSBT after #1041 | vendor the container into `dw-psbt`, without the module's signing paths; separate task (§1.2) |
| Desktop graph gets the dash-spv fix how? | `[patch]` to the `8fe0a381` backport on pasta's fork after approval; drop it when platform re-pins past the upstream merge |
| Upstream PRs | rust-dashcore fix against `dev`; platform status lookup against v5.1-dev, stacked on #5307; both await the manager's approval of their text |
| #5307 or #1149 change before merge | re-run §1 and re-cherry-pick; the fix and the status lookup touch neither PR's files |

## 7. Limits

- The testnet probes ran `dev` `0eaf0284c` with and without the fix, not `8fe0a381`. The commits between them (#1144 and
  #1149's serde changes) do not touch masternode sync.
- One start per probe: each run covers the first quorum after one start, not a distribution.
- Desktop tests with the pin move ran with a type-only PSBT stand-in.
