# E0-10c: dash-spv trust for Platform proofs

Status: implementation notes for DEC-168. Measured 2026-10-10 on agentbox; testnet and mainnet read-only.
Follows [E0-10a](trust-spike.md), whose §6 items 3 and 4 this closes. Item 5, chain-anchored lists
(dashpay/rust-dashcore#1117), is outside E0-10c (DEC-168 (4)).

| What | Revision |
|---|---|
| platform #5307 (v5.1 move, open) | `f475f72a11`, base v5.1-dev |
| rust-dashcore that #5307 pins | `8fe0a381` (head of rust-dashcore #1149, open) = `dev` `c19973ab` + 5 serde commits |
| rust-dashcore `dev` for the fix | `0eaf0284c` |
| fix, on `dev` (rust-dashcore) | dashpay/rust-dashcore#1150, branch `fix/spv-first-quorum-after-start`: `4c142bafc`, `9564a6b56`, `962654cbc`, `3e60a819d` |
| fix, on `8fe0a381` | branch `fix/spv-first-quorum-after-start-on-1149`: `de7e1e04f`, `d3c9ad562`, `67770b9fc`, `d3d50520a` (cherry-picks) |
| status lookup, upstream | dashpay/platform#5366, stacked on #5307: `738f83395d` |
| platform fork branch | `dw/e0-10c-v5.1-spv-trust` on `f475f72a11`: `ad601d1d8b` (#4978), `ebe37f8a67` (#5366) |

All are pushed: the rust-dashcore branches to `PastaPastaPasta/rust-dashcore-dashpay`, the platform branches to
`PastaPastaPasta/platform`. The desktop pins the platform fork branch and patches rust-dashcore to the `8fe0a381`
backport (§5).

---

## 0. Summary

- **The pin move costs 14 mechanical commits** (36 files, +158/−212 outside `Cargo.lock` and the workspace manifest),
  mostly the secp256k1 0.33 port, **plus PSBT**: rust-dashcore #1041 removed `key_wallet::psbt`, which `dw-psbt` is
  built on; E0-10d vendored the container (§1).
- **DEC-18 picks:** #5307 already contains #4623, #4997, #4764, #5206, #5294 and #5305. Of the fixes the desktop gets
  from v5.0-dev today, only #4978 is missing; it cherry-picks cleanly onto #5307 (§2).
- **The first quorum after the start is fixed.** Root cause: a tip update never fetched the work-block list a new quorum
  needs; only the QRInfo path did, once per rotation cycle. A tip update now fetches it, and the quorum is `Verified` in
  the first list that carries it. Regtest: fails before, passes after. Testnet and mainnet, fix vs `dev` side by side:
  every Platform quorum that entered after the start was `Verified` on arrival with the fix (4/4 testnet, 3/3 mainnet,
  3/3 for a client on `962654cbc` and 3/3 for one on the final head `3e60a819d`); on `dev` the first one on each network stayed
  `Skipped(MissedList)` until the run ended, 3.3 h and 2.7 h later (§3.4).
- **The status-carrying lookup** is one additive method on platform-wallet's `SpvRuntime`,
  `get_quorum_public_key_with_status`, returning the key and dash-spv's `LLMQEntryVerificationStatus` (§4).
- **What the desktop graph needs for E0-10b to require `Verified`:** the fork pin (#5307 + #4978 + the status lookup)
  and the dash-spv fix, which #5307's rust-dashcore pin does not carry. This branch has both (§5).

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

The gate at the committed pins (platform fork `ebe37f8a67`, rust-dashcore patched to `d3d50520a`), on desktop main
`bdccfa4` with E0-10d's vendored PSBT container: `cargo fmt --check` and
`cargo clippy --workspace --all-targets -D warnings` pass; `cargo test --workspace`: 857 passed, 1 failed, 4 ignored.
The failure is dw-desktop's `test_QT_031_a_failing_notify_send_is_an_os_error`, which got `ETXTBSY` executing its fake
`notify-send` script while other tests forked; dw-desktop's tests pass 3 of 3 runs alone, and this branch does not
touch the crate. Before E0-10d, with a type-only PSBT stand-in: 839 passed, 13 failed (all 13 the stand-in).

Earlier, against the fork pin through a local `file://` URL and without the dash-spv patch, every `cargo test` failure
was either a PSBT test hitting the stand-in (8 in dw-psbt, 3 in dw-engine `send::flow_tests`, 2 in dw-ffi) or
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

Done: option 1, as E0-10d (desktop main `bdccfa4`). This branch rebased onto it and deleted E0-10d's temporary
comparison with key-wallet's module (`dw-psbt/src/old_vs_vendored.rs`), which cannot build at the new pin.

### 1.3 Before this branch merges

Done: the re-pin to the fork branch (§5) and the vector re-check. DP1-01: the FFI harness at `ebe37f8a67` and the
independent Python derivation agree on all 86 keys, and the rebuilt fixture equals the `bc41f1bc23` one apart from its
platform field (`identity_derive_and_persist.rs` changed only for the secp256k1 0.33 API). DP1-03: the four cited
upstream files are identical at `bc41f1bc23`, `f475f72a11` and `ebe37f8a67`, so only the pin string changed.

E0-10d is merged and this branch is rebased onto it; the gate passes without the stand-in, apart from one known
test race outside this branch (§1.1).

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
| status lookup (§4), #5366 | — | — | cherry-pick `ebe37f8a67` (= `738f83395d`) |

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
is `Verified` in the first list the client publishes with it. Requests are capped per tip update, and the lists they
bring are kept only until their quorums are validated. A work-block list that fails does not hold back the tip update;
a tip update that fails still publishes nothing.

Code: `dash-spv/src/sync/masternodes/sync_manager.rs` (MnListDiff handler) and `dash-spv/src/sml_engine.rs`
(`missing_work_block_list_requests_for`, `drop_requested_work_lists_once_validated`). It cherry-picks unchanged onto
`8fe0a381`.

### 3.3 Tests

- Unit (`sync_manager.rs`): the work-block list is asked for and the quorum re-checked; a failed work-block list still
  completes; a rejected tip update publishes nothing. Mutation checks: removing the fix fails the first two; asking for
  every unverified quorum fails both; dropping either half of the completion guard fails one.
- dashd regtest (`dashd_masternode::test_quorum_mined_after_start_is_verified_on_arrival`): starts the client 4 blocks
  before a DKG cycle, mines the cycle, and records the quorum's status in every list the client publishes, at the
  moment it publishes it. On `dev`: `[(443, Skipped(MissedList(424)))]` (synced at 428). With the fix: passes.
- Unit (`sml_engine.rs`): the cap per update and per mining window, and when a fetched list is dropped.
- Suites on the final head (`3e60a819d`): dash-spv lib 628/628; `dashd_masternode` 11/11 (10 in the full run, where
  dashd's own DKG timed out at load 240 for the regression test, which then passed alone); clippy clean. `dashd_sync` and dash-spv-ffi `dashd_sync`
  had 2–3 timeouts each at load 100–170 on 32 vCPU; both run with masternode sync off, so the change is not on their
  path. The `8fe0a381` backport (`d3d50520a`): dash-spv lib 628/628, `dashd_masternode` 11/11 the same way, clippy clean.

### 3.4 Testnet and mainnet

E0-10a's dash-spv probe (`tools/trust-spike/`, the `spv` mode fed by the pin's `capture` tuples), read-only, started
side by side at 12:33 UTC on 2026-10-10 for 3.5 h: `dev` `0eaf0284c` ("ctl") and `dev` + the fix's first commit
`4c142bafc` ("fix") on both networks. A third testnet client on `962654cbc` ("fin", before the request cap) started at 13:12 and
ran to the same end. A fourth testnet client on #1150's final head `3e60a819d` ("cap", with the request cap) ran
20:00–22:50 UTC. Data: `/work/scratch/e0-10c/probe/run1/` (agentbox, not tracked).

Platform quorums that entered the newest list after each client's initial sync:

| Client | Network | Arrived after the start | Status on arrival | Status at the end of the run |
|---|---|---|---|---|
| fix | testnet | 4 | 4 `Verified` | 4 `Verified` |
| ctl | testnet | 4 | first `Skipped(MissedList(1569760))`, 3 `Verified` | unchanged: the first stayed `Skipped` 3.3 h |
| fin | testnet | 3 | 3 `Verified` | 3 `Verified` |
| cap | testnet | 3 | 3 `Verified` | 3 `Verified` |
| fix | mainnet | 3 | 3 `Verified` | 3 `Verified` |
| ctl | mainnet | 3 | first `Skipped(MissedList(2552944))`, 2 `Verified` | unchanged: the first stayed `Skipped` 2.7 h |

The first quorum after the start is the case under test in every client. ctl and fix synced to 1569778 (testnet) and
2552953 (mainnet); the first quorum's work blocks are 1569760 and 2552944. fin synced to 1569792; its first quorum's
work block is 1569784. cap synced to 1569984; its first quorum's block is 1569984, work block 1569976. The quorums after the first are `Verified` in both builds: by then the client holds a list for
every block.

Every live tuple found once synced was `Verified` on first ask, in the clients started with the capture (52–53 per client). cap started 7.5 h after the capture, so its tuples cited heights below its sync height; the quorums among them that had rotated out came back `Skipped(NotMarkedForVerification)` (§5.1). Platform did not cite
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

DEC-18 allows the fork to carry this only as a cherry-pick of a public upstream PR: it is dashpay/platform#5366,
based on #5307's branch (its test needs #5307's rust-dashcore), and the fork carries the same commit.

---

## 5. Route to the desktop

1. Done: every platform crate pins `ebe37f8a67` on `PastaPastaPasta/platform`. This brings #1094, #1073 and #1102
   (DEC-168 (1)), #4978 and the status lookup (#5366). Move back to upstream once #5307 and #5366 are on v5.1-dev
   and #4978 reaches it.
2. Done: the dash-spv fix is not in `8fe0a381`, so the workspace carries a
   `[patch."https://github.com/dashpay/rust-dashcore"]` to the backport `d3d50520a` on
   `PastaPastaPasta/rust-dashcore-dashpay` (DEC-09). It patches seven crates, and all twelve rust-dashcore packages
   in the lock then come from the fork, one copy each. Drop it when platform re-pins to a rust-dashcore with #1150.
3. Done: E0-10d (§1.2).
4. E0-10b's `trust.rs` then requires `status == Verified`. Enforcement (DP3-04 / H-09 full mode) still waits for #1117.

### 5.1 Note for E0-10b: quorums that rotated out before the client started

A lookup below the height the client started at can return `Skipped(NotMarkedForVerification)` for a quorum that has
since left the active set. dash-spv verifies the quorums of its newest list. The walk-back
(`quorum_entry_for_hash_at_or_before_height`) finds such a quorum only in a list from the client's initial sync,
which still carries the diff's default status. A quorum still in the newest list gets its status copied to
every list holding it, so this affects only quorums that are no longer active.

Seen in the probe of #1150's head (§3.4): its lookups replayed ChainLock heights captured 7.5 h earlier, below its
sync height, and the 4 quorums cited there that had since rotated out came back `Skipped(NotMarkedForVerification)`.
The 24 quorums of the newest list were all `Verified`. The answer errs strict: never `Verified` without verification.

For E0-10b: a `trust.rs` that requires `Verified` refuses a proof citing such a quorum. Live proofs cite recent
ChainLock heights, so this needs a proof older than the client's start whose quorum has also rotated out. The
default is to refuse and retry, and to show the status (Tools ▸ Information, DP6-03) rather than accept it.

## 6. Open questions (defaults in force)

| Question | Default |
|---|---|
| PSBT after #1041 | decided: vendor the container into `dw-psbt`, without the module's signing paths, as E0-10d (§1.2) |
| Desktop graph gets the dash-spv fix how? | decided: `[patch]` to the `8fe0a381` backport on pasta's fork (§5); drop it when platform re-pins past #1150 |
| Upstream PRs | open: rust-dashcore#1150 against `dev`; platform#5366 stacked on #5307 |
| #5307 or #1149 change before merge | re-run §1 and re-cherry-pick; the fix and the status lookup touch neither PR's files |

## 7. Limits

- The testnet probes ran `dev` `0eaf0284c` with and without the fix, not `8fe0a381`. The commits between them (#1144 and
  #1149's serde changes) do not touch masternode sync.
- One start per probe: each run covers the first quorum after one start, not a distribution.

## 8. Follow-ups

- **No desktop consumer reads the status yet.** `SharedContext::get_quorum_public_key`
  (`dw-engine/src/context.rs`) delegates to `TrustedHttpContextProvider`; nothing in the desktop calls `SpvRuntime`'s
  lookup. E0-10b's `trust.rs` is where it will: a `ContextProvider` whose `get_quorum_public_key` calls
  `SpvRuntime::get_quorum_public_key_with_status` and requires `Verified` (§5 step 4, §5.1).
- **Possible upstream cleanup in platform-wallet:** `SpvRuntime`'s lookup narrows the `u32` `quorum_type` to `u8`
  (`LLMQType::from(quorum_type as u8)`), so values above 255 wrap and unmapped values become `LlmqtypeUnknown`
  instead of an error. This predates #5366, which keeps the existing conversion.
