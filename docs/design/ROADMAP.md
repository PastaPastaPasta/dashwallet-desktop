# Roadmap: DashPay on the desktop to 1.0, and the 1.1 train

Status: **authoritative**, 2026-10-08. This is the task plan for [`DASHPAY.md`](DASHPAY.md). It applies the manager's
decisions DEC-01, DEC-09, DEC-12, DEC-13 and DEC-15 (listed in DASHPAY.md). External blockers (B1–B4) are in
[`DECISIONS-PENDING.md`](DECISIONS-PENDING.md).

It replaces the M4–M7 plans in `DASHPAY-fable.md` §6 and `DASHPAY-opus.md` §5 (on branches `dw/dashpay-design-fable`
and `dw/dashpay-design-opus`), and the M4 "Platform parity" workstream in DESIGN-opus.

## 0. Conventions

- **One task is one PR**, with a conventional commit, its contract update and its `docs/parity.md` rows in the same
  PR. Each milestone's contract work lands first (E0-08 for DashPay, MG-02/MG-03 for masternodes and governance).
- **Sizes** in agent-days: **S** ≤ 1, **M** 2–3, **L** 4–5. Anything larger is split.
- **Owner types:**
  - `engine`: Rust in dw-engine, dw-vault, dw-appdb, dwcli;
  - `UI`: view models and screens in the stack G-04 selects;
  - `infra`: environments, harnesses, CI, packaging, documents;
  - `manager`: the program manager's decision (DEC-12);
  - `external`: a blocker in `DECISIONS-PENDING.md`.
- **Test tiers** are defined in DASHPAY §6:
  - **T0**: unit tests, view-model tests and fixture UI tests;
  - **T1**: dashd regtest;
  - **T2**: the dashmate devnet on agentbox;
  - **T3**: testnet nightly;
  - **T4**: the mainnet canary;
  - **CI**: GitHub Actions on Linux, macOS and Windows (DEC-13);
  - **GL / GW / GM**: GUI runs on Linux, Windows and macOS.
- **Stack-indep**: **Y** means the task does not depend on the UI-stack decision and can be scheduled whenever its
  dependencies are met. **N** waits for G-04.
- **UI tasks in the U, DP, X and MG tracks** (not the G spike tasks) depend on E0-13 (the facade binding) and U-07 (the
  fixture backend), in addition to their listed dependencies.
- **At most four agents at once**, organised as four lanes: E1 and E2 (engine), UI, and L4 (infra, or a second UI
  agent once infra work thins out).
  - A wave never schedules a task before its dependencies' waves. Within a wave a lane runs its tasks in order.
  - The stabiliser runs at each wave boundary, in one lane's slot.
  - Second-agent reviews are subagents of the authoring agent and run while it waits, so they do not add a parallel
    agent.
- **Merge discipline.**
  - Engine PRs run T0 and T1, and CI, before merging. DashPay engine PRs also run T2 once T-01 is green.
  - `signers.rs`, `registration.rs`, `payments.rs`, `invitations.rs`, `trust.rs` and the MG signing code need a
    second-agent review that cites the counterpart (`rs-platform-wallet-ffi`, or the parked branch for MG).
  - After large changes, run `code-review-validator`.

## 1. Already running

| ID | Task | Branch | Notes |
|---|---|---|---|
| **D1** | **agentbox setup**: a portable deps dir; per-checkout target dirs and Docker volumes; `cargo test --workspace`, the Linux `swift test` targets (Docker), the regtest suites and the CrossUI Xvfb demo green on agentbox | `dw/agentbox-setup` | **Do not duplicate.** Every task that needs a full regression run or the regtest harness depends on D1. If D1 merges without a *completed* CoinJoin regtest run (the laptop run was cut off), E0-01's regression run finishes it before E0-01 merges. |

## 2. Tasks

### S — Scope

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| S-01 | **Update `CLAUDE.md` "Product scope" for DEC-01**. In: the wallet-holder masternode and governance flows (voting, proposals, my masternodes, the registration wizard, v24 shared masternodes), after the DashPay core. Out (the node and its CLI): operator and server tasks such as running a node, operator BLS operations (ProUpServTx, ProUpRevTx) and node administration. Also point `README.md` at DASHPAY.md and ROADMAP.md. | — | S | infra | — | Y | The two lists match DASHPAY §5a. Agents no longer refuse MG tasks under the old wording. |

### E0 — Engine foundations

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| E0-01 | **Pin bump** to v5.0-dev head (DEC-15): every platform crate at one revision (≥ `bc41f1bc23`), rust-dashcore `40268cc0` and grovedb `9791d277` as in platform's lock | D1 (for the regression run; the edit can start now) | M | engine | T0, T1, CI | Y | `cargo test --workspace`; regtest `l1-sync`, `l1-send`, `l2-tools`, `restore` and a **completed** `coinjoin` run; Linux `swift test` and `build-core.sh --check-bindings` are all green. `dw-engine/src/error.rs` maps `InsufficientIdentityCredits`. DESIGN.md R3 pins are updated. |
| E0-02 | **Runtime hygiene**: 8 MiB stacks for workers and blocking threads; `SessionOptions.ca_cert_path` → `SdkBuilder::with_ca_certificate_file`; `initial_protocol_version` → `with_initial_version`; a minimal `dwcli platform-status` that fetches the DPNS contract | — | S | engine | T0, T2 smoke | Y | A test that needs more than 2 MiB of stack passes on an engine worker. Once T-01 is green, `dwcli platform-status` verifies the DPNS contract through dashmate's self-signed gateway. |
| E0-03 | **Signer glue**: `VaultIdentitySigner`, `VaultContactCrypto`, `VaultScanKey`; the scopes `PlatformIdentity`, `DashPayCrypto` (limited to exactly the contactInfo children `65536'`/`65537'` under the identity-auth root) and `PlatformFunding{max_duffs}`; DIP-14 256-bit children through the vault's path checks; the `platform-encryption` dependency | — (the traits are unchanged between the pin and head, so it starts on the pin and rebases over `Cargo.lock`) | L | engine | T0 | Y | On fixed seeds, outputs are byte-identical to the library's test `SeedCryptoProvider` (`contact_requests.rs:171-345`, `#[cfg(test)] pub(crate)`, so it cannot be called from here). The vectors come from a one-off test run inside the local platform checkout (never pushed) and are committed as fixtures: receiving xpub, ECDH, account reference and unmask, contactInfo seal and open, auto-accept key. Signatures from keys 0–5 verify against the derived public keys. Every scope refuses paths outside it. A second-agent review cites `PWF/dashpay.rs` and `mnemonic_resolver_core_signer.rs`. |
| E0-04 | **Grants and leases** (DASHPAY §2.6): `PlatformOp{max_duffs, max_credits}`; flow leases, including one lease that carries several purposes with separate caps (for "Accept and pay"); a locked-vault lease keeps its key through the InstantSend window (≤ 300 s after broadcast) and drops it if the flow falls back to the ChainLock wait; the background `DashPayCrypto` lease (unlocked or unencrypted vault only); a lease state the UI can show | E0-03 | M | engine | T0 | Y | Tests in the style of the vault race tests: a lease cannot sign a BIP44 spend above its cap; `DashPayCrypto` signs no transaction; lock revokes every lease and parks the flow; a flow paused (barrier) after its signature was released and before its commit, then resumed after a lock, broadcasts or submits nothing (review DW-E0-03 r2 M2); `UnlockedMixingOnly` and `Locked` get no background lease. The m1-engine §2.2 credential table is updated. |
| E0-05 | **Engine-owned bring-up** (DASHPAY §3.2): `start_spv` returns at once and runs bring-up then SPV in a task that close can cancel; `start_wallet_subsystems` first (20 s, or our 3 s for a wallet created here); loop start/stop/quiesce; a cadence API; unlock drain + `reconcile_dashpay_rescan`; the status snapshot, with `Starting`; watch-only wallets are read-only | E0-02, E0-03 | L | engine | T0, T3 (T2 once available) | Y | `startSPV` returns within 100 ms. With DAPI blackholed, SPV starts within the budget + 1 s. Closing during bring-up is clean. A restored testnet wallet that has an identity reports `Ready` and has its contact accounts before SPV's first filter scan. The close report is `all_clean`. |
| E0-06 | **Changeset tap**: `WalletStore::store` → debounced `EngineEvent::Platform` signals + the `dp_events` journal; catch-up silence after a restore; `dp_trust_unverified` rows while the trust fallback is in use | E0-05, E0-07 | M | engine | T0 | Y | Every row of DASHPAY §3.5 has a test; 4 Hz debounce with the trailing edge kept; idempotent journal writes; events from a restore pass are stored read; with the fallback flag on, every touched entity gets a `dp_trust_unverified` row. |
| E0-07 | **dw-appdb migrations**: the `dp_*` tables of DASHPAY §3.4, including `dp_trust_unverified`; `<network>/avatars/` with mode 0700 | — | S | engine | T0 | Y | Append-only migrations with tests. |
| E0-08 | **DashPay facade skeleton + contract**: `NetworkSession::dashpay(wallet)`; every record, enum and per-domain error code of DASHPAY §3.6 as stubs returning `platform.not_implemented{call}`; `docs/contracts/m4-dashpay-engine.md` | E0-01 | M | engine | T0 | Y | A code-table test matches the contract. DP engine tasks fill in bodies without changing signatures, or update the contract in the same PR. |
| E0-09 | **dwcli DashPay commands** over the facade only: `dashpay status\|sync`, `identity …`, `name …`, `contact …`, `pay-contact`, `profile set`, `invite claim`; JSON output | E0-08 (the commands fill in as DP tasks land) | M | engine | T2, T3 | Y | The T2/T3 suites drive DashPay through `dwcli` only. |
| E0-10a | **Trust spike** (DASHPAY §2.2 gate): measure at platform's pin and capture the (type, hash, height) tuples real proofs cite; then feed those tuples to a standalone dash-spv probe at rust-dashcore `dev` (#1072, #1075) | E0-01 | M | engine | T3 + mainnet read-only | Y | `docs/research/dashpay/trust-spike.md` with numbers from testnet and mainnet, a pass or fail for R7 at the pin, and whether `dev` closes the gap. |
| E0-10b | **Layered quorum provider** (`trust.rs`), rules 1–4 and 6: the SPV cache; the trusted fallback before masternode sync, for reads only; after sync, a bounded wait then fail-closed for an unknown quorum; refusal on mismatch; setting the fallback flag for E0-06; re-fetching and clearing `dp_trust_unverified` entities after sync; the quorum source in `sync_status` | E0-10a, E0-05, E0-06 | M | engine | T0, T2, T3 | Y | A forged quorum key makes every DashPay read refuse. The fallback is used only before sync. A developer toggle forces trusted-only. If E0-10a failed, the provider ships in the degraded mode of DASHPAY §2.2. |
| E0-10c | *Only if E0-10a fails at the pin; scheduled right away (W3–W5).* Fix dash-spv in rust-dashcore, or adopt #1072/#1075. Prepare it as a backport onto platform's pinned rust-dashcore revision. | E0-10a | M–L | engine | T0, T3 | Y | The spike passes with the fix. Enforcement ships when the desktop's graph carries it (platform's pin, or a graph-wide `[patch]` once the manager allows publishing under DEC-09). |
| E0-11 | **Carried branch for #4623 + #4997** on pasta's fork `PastaPastaPasta/platform`, on v5.0-dev head; the first push waits for the manager's recorded decision (§5) | E0-01 | S | engine | platform-wallet tests | Y | The branch compiles and passes `rs-platform-wallet`'s tests. A tracking issue exists. The desktop pins the whole platform set to it only when DP3-01 starts, unless the v5.1 move (E0-14) has happened. No upstream PR unless pasta asks. |
| E0-12 | **Shielded build measurement**: a non-default `shielded` feature that enables `platform-wallet/shielded` + `platform-wallet-storage/shielded` | E0-01 | S | engine | T0, CI | Y | Clean build time, binary-size delta and prover warm-up are recorded in `docs/research/shielded-cost.md`, and the feature build is kept green in nightly CI. |
| E0-13 | **Bind the facades** for the chosen stack. Tauri: `dw-app` commands, generated TypeScript types and the event bridge. SwiftCrossUI: `dw-ffi` (UniFFI) wrappers and DashKit. The same task binds `Governance` and `Masternodes` later (MG). | G-04, E0-08 | M | engine / UI | T0, CI | N | Every facade call is reachable from the UI layer with generated types; a bindings check runs in CI. |
| E0-14 | **Pin train**: one bump per milestone, rebasing E0-11's carried branch at each bump; the v5.1 move when DASHPAY §3.9's criteria hold (which retires the branch) | E0-01 | M per bump | engine | T0, T1, T2, CI | Y | As E0-01, plus the DashPay suites green. |

### T — Test environments and CI

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| T-01 | **dashmate devnet on agentbox**: `scripts/devnet.sh up\|env\|down\|reset`, using dashmate 5.0.0-beta.2 (beta.1 until E0-01 merges); the Docker data root has ≥ 40 GB free (moved to `/work` if needed) | — (engine smoke: E0-02) | M | infra | T2 | Y | A cold `up` takes under 60 min. `env` prints DAPI, the quorum URL (`:22444`), SPV peers and the CA path. The contest voting period on `local` is measured and recorded. `regtest/platform/README.md` is written. |
| T-02 | **Testnet harness**: two persistent funded wallets (seeds in agentbox's secret store, never in the repo); top-ups through the `dash-faucet` skill's helper within its limits; a scheduled nightly run | D1 | M | infra | T3 | Y | The nightly run reports balances and Platform reachability, and runs the T3 suites as they land. |
| T-03 | **GitHub Actions** (DEC-13): push `main`. Linux (fmt, clippy, cargo test, UI T0), Windows and macOS (engine build and tests; app build, install, launch and screenshot as artifacts), nightly regtest, and `workflow_dispatch` jobs for the gate branches | D1 | M | infra | CI | Y | Green on `main`. Windows and macOS screenshots can be downloaded from a run. |
| T-04 | *Contingency.* A Windows 11 evaluation VM under KVM on agentbox, only if the hosted runners cannot do what a task needs (interactive Narrator, Windows Hello) | — | M | infra | GW | Y | An agent builds and runs the app on it over SSH and fetches a screenshot. |

### G — UI-stack gate (DASHPAY §2.1, a head-to-head)

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| G-01 | **Tauri spike app** (the G-01 + G-02 timebox is 5 agent-days + ≤ 3 for fixes): `apps/desktop` + `rust/crates/dw-app`; Overview, Transactions, transaction detail and unlock; generated tokens; event bridge; fixture backend; `--selftest` | — (it uses the fixture backend until D1's harness gives it a regtest wallet) | M | UI | T0, GL | Y (it is the decision's input) | The DASHPAY §2.1 scope is built on a branch. |
| G-02 | **Tauri measurements**: U1–U8 on agentbox and the CI runners, with the 10k-transaction regtest wallet | G-01, T-03, D1 | M | UI | GL, GW, GM | Y | Every criterion measured with raw data in a draft `docs/adr/0003-ui-stack.md`. |
| G-03 | **SwiftCrossUI measurements**: the existing `dash-wallet` with AppKitBackend on `macos-latest`, with WinUIBackend (and GtkBackend if WinUI fails U4) on `windows-latest`, and AT-SPI on Linux; the same U1–U8, with U3 judged against UX-SPEC's Cross column | T-03, D1 | L (4 + ≤ 2 for fixes) | UI / infra | GL, GW, GM | Y | Same ADR, same table, including U3's recorded fidelity gap. |
| G-04 | **ADR 0003: the manager's decision** (DEC-12) under DASHPAY §2.1's rule | G-02, G-03 | S | manager | — | — | ADR merged with the numbers and the reasons. pasta can revisit it. |

### U — One UI codebase (after G-04; Tauri variant shown)

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| — | **Freeze, effective now**: no new screens in `MacUI`, `DashUIMac`, `CrossUI` or `DashUICross`; bug fixes only | — | — | — | — | — | — |
| U-07 | Fixture backend + UI test harness: Playwright light and dark, tauri-driver end-to-end, axe-core in CI; replaces `WalletDemo` for UI tests | G-04, E0-13 | M | UI | T0, CI | N | Playwright runs without an engine. |
| U-01 | Shell and information architecture (DASHPAY §4): window, sidebar, toolbar (chip, bell, wallet picker, discreet, lock), status bar, native menus, single instance, deep links (`dash:`, `pay:`, `dashwallet:`, `dashpay:`), tray | G-04, E0-13 | L | UI | T0, GL, CI | N | Every route renders; links route on all three OSes (CI). |
| U-02 | Onboarding (create, verify, restore with live checks, passphrase), lock screen, unlock sheet, recovery phrase, Forgot passphrase | U-01 | L | UI | T0, T1, GL | N | The ported view-model tests (by parity ID) pass; the IOS-002/004/007/010/013/014 flows pass on T1. |
| U-03 | Home, Pay (address tab, coin-control panel, confirm, result), Receive, Activity (list, table, filters, detail) | U-01 | L | UI | T0, T1, GL | N | A real regtest send through the UI. |
| U-04 | Settings and Options, Security, Wallets, About, backups, imports and exports | U-01 | L | UI | T0, T1 | N | QT and IOS rows keep their status. |
| U-05 | Tools (Information, Console, Peers, Repair), PSBT, Sign/Verify, address book, Coin Selection | U-01 | L | UI | T0, T1 | N | as U-04 |
| U-06 | CoinJoin card, page and settings | U-03 | M | UI | T0, T1 | N | Mixing starts and stops from the UI. |
| U-08 | Retire Swift after the `swift-final` tag: the Swift targets, UniFFI generation, `Vendor/swift-cross-ui`, `Apps/macOS`; README update | U-02…U-07, PK-02 | S | UI / infra | CI | N | The repo builds without Swift. On macOS, Touch ID quick unlock stays off until B4 verifies it. |

If **SwiftCrossUI** is chosen, the U track becomes:

| ID | Task | Size | Stack-indep |
|---|---|---|---|
| UB-0 | Shell and information architecture in CrossUI (DASHPAY §4, R11). It replaces U-01, and the DP tasks' dependencies on U-01 and U-03 point at UB-0 and the existing CrossUI screens. | L | N |
| UB-1 | Move the MacUI-only behaviours behind `PlatformServicesMac` and ship the Cross app in the macOS bundle | L | N |
| UB-2 | Windows build and installer of the Cross app with the backend that passed U4 | L | N |
| U-07 | Extend `WalletDemo` with DashPay fixtures | M | N |
| U-08 | Delete `MacUI` + `DashUIMac` | S | N |

UB-0, UB-1 and UB-2 take the slots of U-01…U-06 in the waves below.

### DP1 — Identity and username

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP1-01 | Port the identity key policy from iOS `DWDashPayIdentityKeys.swift` | E0-03 | M | engine | T0 | Y | For the same seed and index, the key set equals iOS's: purposes, security levels and contract bounds. |
| DP1-02 | Registration flow: quote, start, the persisted state machine (`build_asset_lock_transaction`, then tracked lock, then `FromExistingAssetLock`), events, resume (session open, unlock, "Finish registration"), parking without keys on the ChainLock fallback, recover, discard, `finish_asset_locks` | E0-04, E0-05, E0-07, E0-08, DP1-01 | L | engine | T1, T3, T2 (kill matrix) | Y | Kill -9 at **every** transition of DASHPAY §3.4 loses no funds, and every case ends registered or resumable. On a locked vault a registration with a timely InstantSend lock needs one prompt; on the ChainLock fallback it parks keyless and asks again at identity creation. The funding cap holds. A non-contested registration succeeds on testnet. |
| DP1-03 | Names: `check_username` (dash-platform-queries rules, 23-character cap), availability, contest precheck, extra names from credits, the temporary-name policy, the main name | E0-08, DP1-01 | M | engine | T0, T2, T3 | Y | Rule vectors include homographs. Availability agrees with the explorer for 10 sample names. The main name survives sync (#4978). |
| DP1-04 | Contest watch: cadence, outcomes, journal, OS-notification signal | DP1-03, E0-06 | M | engine | T2 | Y | Won, lost and locked outcomes are seen on T2 (short period or harness votes). |
| DP1-05 | Same-seed recovery on restore + main identity | E0-05 | M | engine | T2, T3 | Y | Restoring into a fresh datadir brings back the identity, names and main name within 2 passes, and the contact payment history after SPV. |
| DP1-06 | Credits: refresh, top-up, cost table, low-credit thresholds | E0-04, DP1-01 | M | engine | T2, T3 | Y | The balance rises by the funded credits minus fees; the cost table matches the network's fees. |
| DP1-07 | Suites: `test_dp1_identity.py` (T2) and the nightly testnet registration (T3) | E0-09, T-01, T-02, DP1-02, DP1-03 | M | infra | T2, T3 | Y | Green 3 nights running. |
| DP1-08 | UI: DashPay status card, identity chip, Join intro, FAQ, voting info | U-03 | M | UI | T0 | N | Every banner state rendered in light and dark. |
| DP1-09 | UI: registration wizard, progress, resume | DP1-02, DP1-03, DP1-08 | L | UI | T0, GL on T2 | N | Register `alice` on T2 from the UI; close it mid-flow and resume. |
| DP1-10 | UI: My Profile sheet, username request status, top-up sheet | DP1-04, DP1-06, DP1-08 | M | UI | T0 | N | — |

### DP2 — Contacts

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP2-01 | Contacts read model: in, out, established, ignored, hidden, channel state, profile join, sort, search | E0-06, E0-08 | M | engine | T0 | Y | Sections match `established_contacts` + the request rows over fixtures. |
| DP2-02 | Send, accept, ignore, unignore; eligibility; Enable DashPay keys | DP2-01, DP1-01, E0-04 | M | engine | T2 | Y | With two wallets, A→B then accept: both see Established within 2 passes. Eligibility refuses before any prompt. The key-upgrade quote equals the fee charged. |
| DP2-03 | User search with relation; user links (build, parse, verify); the `dapk` scan path | DP2-01 | M | engine | T0, T2 | Y | An iOS-made link verifies (fixture); an expired `dapk` is refused; a valid one sends the request with the proof. |
| DP2-04 | Private details (alias, note, hidden), deferred until ≥ 2 contacts | DP2-02 | S | engine | T2 | Y | A restored wallet gets the alias back. |
| DP2-05 | Notifications engine: feed, unread count, OS-notification signal, catch-up silence | E0-06 | M | engine | T0, T2 | Y | One event per request received or accepted; no notification storm after a restore. |
| DP2-06 | Suites: round trip, ignore and unignore, hidden, restore with contact accounts before SPV; the one-way contact case once the pin has #5256 (E0-14's v5.1 move) | E0-09, T-01, DP2-02, DP2-04 | M | infra | T2, T3 | Y | Green. |
| DP2-07 | UI: Contacts split view (groups, sort, local and network search, hidden, pending-setup hint) | DP2-01, U-01 | L | UI | T0, GL | N | — |
| DP2-08 | UI: Add Contact (search, My QR, scan from a file, drop, clipboard or screen region) | DP2-03, DP2-07 | M | UI | T0, GL | N | — |
| DP2-09 | UI: contact pane (actions, activity, private details, Enable DashPay banner), notifications popover and page | DP2-02, DP2-05, DP2-07 | L | UI | T0, GL on T2 | N | Accept a request from the bell on T2. |
| DP2-10 | Receiving accounts while locked, from a cached `15'/0'` xpub | DP2-01 | M | engine | T2 | Y | A new contact's payment is seen while locked. After 1.0. |

### DP3 — Paying contacts

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP3-01 | `Recipient::Contact` in `TxDraft` (DASHPAY §2.3): same-size estimate; the host authorizes after Confirm; prepare redeems, then reserves, then signs; the reserved address is reused on a re-prepare; per-contact lock; CoinJoin source allowed; "Accept and pay" | E0-11, DP2-02, E0-04 | L | engine | T0 (send flow tests), T2 | Y | The m1-engine §2.7.1 rules hold for contact payments. A failure before the reservation consumes no address. A `send.no_peers` re-prepare prompts again and reuses the reserved address. A fee that grows → `send.grant_exceeded`. An unknown outcome locks the contact. "Accept and pay" validates the pending request, takes one credential prompt (one lease), and spends only after the accept succeeded. m1-swift is updated for the contact branch. |
| DP3-02 | Counterparty on history and transaction detail; contact activity; frequent contacts | DP2-01 | M | engine | T0, T2 | Y | Attribution both ways, also after a restore. |
| DP3-03 | Username in Receive and in payment requests | DP1-05 | S | engine | T0 | Y | IOS-055. |
| DP3-04 | Money-move trust gate (DASHPAY §2.2 rule 5, provenance) | E0-10b, DP3-01, DP1-02 | M | engine | T0, T2 | Y | With the fallback in use, registration, top-up and a payment to an entity with a `dp_trust_unverified` row wait until it is re-fetched and verified by SPV. In the degraded mode (E0-10a failed), they proceed on trusted-provider data, and a mismatch with SPV still refuses. |
| DP3-05 | Suites: pay both ways; unknown outcome (no peers) → lock → resolve; 25 cancelled confirms (more than the 20-address gap), then a payment the recipient still sees | DP3-01, E0-09, T-01 | M | infra | T2 | Y | Green. |
| DP3-06 | UI: Pay ▸ To a contact (frequent strip, picker, confirm with avatar, "Accept and pay", lock banner) | DP3-01, DP2-07, U-03 | M | UI | T0, GL on T2 | N | Pay `bob` from the UI on T2. |
| DP3-07 | UI: history rows with avatar and name; transaction detail → contact pane | DP3-02, DP2-07 | M | UI | T0 | N | — |

### DP4 — Profile and avatars

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP4-01 | Profile read and update (limits from the contract: 25 / 140) | DP2-01 | S | engine | T2 | Y | A contact sees the change within 2 passes. |
| DP4-02 | Avatar pipeline (DASHPAY §3.8): fetch guard, verification, re-encoding, cache, privacy toggle; URL, Gravatar and file sources; Imgur only with B1 | DP4-01 | L | engine | T0, T2 | Y | Fixtures: oversize, wrong hash, redirect to 10.0.0.1, GIF. A mismatch shows initials; no fetch reaches a private IP; hash and dHash equal the library's. |
| DP4-03 | UI: Edit Profile (crop, unsaved-changes guard); avatars in every list | DP4-02, DP1-10 | M | UI | T0 | N | — |

### DP5 — Invitation claim

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP5-01 | Link intake and vault stash (DASHPAY §2.9, last row); replay after onboarding; log redaction | E0-08 | M | engine | T0 (parser fuzz seeds) | Y | A link opened before any wallet exists survives onboarding; a redaction test passes. |
| DP5-02 | Claim: status (#4997 when carried, else the prospective-id check), inviter preview, invitation-funded registration, request to the inviter | DP1-02, DP2-02, DP5-01 | M | engine | T2 (an invitation made by a test-only patched `dwcli`) | Y | An identity and a name with zero Core balance; the inviter gets the request; an already-claimed invitation is detected. (The mainnet claim is H-07.) |
| DP5-03 | UI: claim screen (welcome and Join entries) | DP5-02, DP1-09 | M | UI | T0, GL | N | — |

### DP6 — Identity tools and diagnostics

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| DP6-01 | Identities page (advanced): list, set main, keys, refresh, find | DP1-05 (engine); U-04 (UI) | M | engine + UI | T0, T2 | engine Y / UI N | — |
| DP6-02 | Withdraw credits with a fee reserve on Max | DP1-06 (engine); DP1-10 (UI) | M | engine + UI | T2 | engine Y / UI N | Max leaves enough for the network to accept the withdrawal. |
| DP6-03 | DashPay card in Tools ▸ Information (loops, last pass, pending crypto, scan verdict, quorum source, "Sync now") | E0-05, E0-10b, U-05 | S | UI | T0 | N | — |

### X — Platform extras (DASHPAY §2.10)

| ID | Task | Deps | Size | Stack-indep | Release |
|---|---|---|---|---|---|
| X1a | Read-only Platform-address balance (`platform_address_sync`), a breakdown line with info sheet (MP-02), and the restore notice for shielded balances | E0-05, E0-08; UI: U-03 | M + S | engine Y / UI N | 1.0 |
| X1b | Platform-address transfers, receive, advanced mode, Internal Transfer with a non-Core confirm, registration funded from addresses | X1a, DP1-02 | 2 × L | engine Y / UI N | 1.1 |
| X2 | Shielded: feature on, balance, shield/unshield, shielded registration funding with the readiness checklist, "move mixed coins to Shielded" | E0-12, X1b | 3 × L | engine Y / UI N | 1.1 |
| X3 | Invitation creation, share and history (Android flow), after the storage patch on our fork (loaders + `WALLET_RESTORE`) | E0-14 (the v5.1 move brings the restore stack), DP5-02 | patch 3–5 d + M + L | engine Y / UI N | 1.1 (DEC-15) |
| X4 | Username marketplace (with a dw-appdb mirror of the name states) | DP1 | 2 × L | engine Y / UI N | 1.2 |

### MG — Masternodes and governance for wallet holders (DEC-01, DASHPAY §5a, release 1.1)

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| MG-00 | UX spec for the Masternodes sidebar item (My masternodes, Governance, the vote sheet, the registration and shared-masternode wizards), mobile-inspired; replaces UX-SPEC §4.22–4.24 | — | M | UI | — | Y | A UX-SPEC section with states, copy and light/dark mocks, reviewed against §5a's in/out split. |
| MG-01 | **Salvage**: partly revert `aab64fc` (dw-p2p governance wire commands, `PeerPicker::full_nodes`, events, error mapping, crate base files), then apply `dw-governance`, `dw-protx` and the vault's masternode keys from both parked branches, resolving the conflict with `374c883`. **Leave out** the operator-signed builders (`prepare_update_service`, `prepare_revoke`) and their CLI/console commands; the M3 FFI and Swift adapters stay out too. | E0-01 | L | engine | T0 | Y | The crates' unit and oracle tests are green (object hashes and collateral scripts equal dashd's). No operator-key signing path remains. Overlap with platform-wallet's masternode code is listed, with the library preferred. |
| MG-02 | `Governance` facade: govsync over `dw-p2p`, proposals, votes, clock, info, superblock budget; console commands back; contract `docs/contracts/mg-engine.md` | MG-01, E0-08 | L | engine | T1, mainnet read-only | Y | The governance regtest suite (4 masternodes) is green, and the G7 measurement on mainnet matches Core's Y/N counts on the bumped pin. |
| MG-03 | `Masternodes` facade + contract: my masternodes (collateral, owner, voting or payout key in the wallet); tracking any masternode and attaching keys (IOS-082, the library's tracked masternodes); status from the SPV list; payouts attributed in history; evonode status and claimable credits (IOS-080) | MG-01, E0-08 | M | engine | T1, T2 | Y | A regtest masternode funded from the wallet shows its status and payouts; an evonode on T2 shows claimable credits. |
| MG-04 | ProTx owner flows: ProRegTx (regular and evonode; `FundNew`, `ExistingUtxo` and the new `External` collateral with a signed-message proof; version-2 payloads, and version 3 where the network requires it); ProUpRegTx; the operator key taken as given, or from the wallet's keychain | MG-03, MG-05 | L | engine | T1 (ProTx suite; operator legs via dashd RPC), T2 (evonode) | Y | Each payload equals dashd's for the same inputs (oracle); registrations are accepted on regtest and dashmate; no operator-key signing. |
| MG-04b | **v24 shared masternodes** (QT-126/127): the session protocol for creation (2–8 shares), reward-address change, key rotation, dissolve now and together, standby; on top of the existing payload codecs | MG-04 | 2 × L | engine | T1 (v24) | Y | A three-party shared masternode is created, its reward address changed, and it is dissolved on regtest. Payloads equal dashd's. |
| MG-05 | Authorization: scopes `MasternodeVoting` and `MasternodeOwner`; grant purposes `Governance` and `MasternodeOp{max_duffs}` restored, where the cap covers the net debit and fees | E0-04, MG-01 | M | engine | T0 | Y | A voting lease cannot sign a transaction; a `FundNew` collateral does not count against the cap, and fees do. |
| MG-06 | Proposal creation and resume: collateral transaction, confirmations, submit, validation, superblock dates | MG-02, MG-05 | M | engine | T1 | Y | A proposal created on regtest appears in govsync and can be voted on. |
| MG-07 | Optional: voting on contested DPNS names with the wallet's masternode voting keys (IOS-079) | MG-05, DP1-03 | M | engine | T2 | Y | A vote cast on dashmate changes the contest tally. |
| MG-08 | `dwcli gov …` / `mn …` (owner commands only) and the regtest suites in nightly CI | MG-02, MG-04, MG-06 | M | infra | T1, CI | Y | Green 3 nights running. |
| MG-09 | UI: Governance tab (list, filters, detail, vote sheet over my masternodes, create/resume wizard) | MG-00, MG-02, MG-06, E0-13 | L | UI | T0, GL | N | Vote and create a proposal on regtest from the UI. |
| MG-10 | UI: My masternodes (list, detail, keys, payouts, tracking, evonode credits), the registration wizard (regular, evonode), update registrar | MG-00, MG-03, MG-04, MG-13, E0-13 | L | UI | T0, GL | N | Register a regtest masternode from the UI. |
| MG-10b | UI: the shared-masternode wizards and maintenance | MG-04b, MG-10 | M | UI | T0, GL | N | Create and dissolve a shared masternode on regtest from the UI. |
| MG-11 | UI: contested-name voting | MG-07, MG-09 | M | UI | T0 | N | — |
| MG-12 | Security review of the MG signing paths (owner and voting keys, ProTx payloads, collateral proofs, shared sessions, proposal fee, credit withdrawal) | MG-04b, MG-06, MG-13 | M | engine | — | Y | Findings fixed or accepted in writing. |
| MG-13 | Evonode Platform credit withdrawal to the payout address (IOS-081 owner part, the library's `masternode_withdraw`) | MG-03 | M | engine | T2 | Y | Credits are withdrawn on dashmate. |

The parity rows move as DASHPAY §5a's table says, each by the task that delivers it. Operator-only rows (QT-125's Update
Service and Revoke, the Unban parts of IOS-081/082) are marked "node/CLI, out of scope".

### MP — Remaining mobile parity (iOS rows still "not started")

| ID | Task (IOS rows) | Deps | Size | Stack-indep | When |
|---|---|---|---|---|---|
| MP-01 | Rates service and fiat everywhere (024, 038, 044, 104) | — (engine); U-03 (UI) | M | engine Y / UI N | 1.0 |
| MP-02 | Balance breakdown card + info sheets (021) | — | S | N | 1.0, inside X1a |
| MP-03 | Time-skew detection from the HTTPS `Date` header (026) | — | S | engine Y | 1.0 |
| MP-04 | Tax categories, CSV tax export (036, 037, 039) | — | M | engine Y / UI N | 1.2 |
| MP-05 | BIP70/72 (049) | — | M | engine Y | 1.2 (needed by DashSpend) |
| MP-06 | Phrase repair (008) | — | M | engine Y / UI N | 1.2 |
| MP-07 | Sweep and private-key import incl. BIP38 (056) | — | M | engine Y / UI N | 1.2 |
| MP-08 | CoinJoin "move mixed coins" to the wallet (057, Core part) | — | S | engine Y / UI N | 1.2 |
| MP-09 | Explore: DB sync (HTTPS + AES zip), merchants, ATMs, filters, details, map links (095–100) | — | L + M | engine Y / UI N | 1.2 |
| MP-10 | DashSpend CTX + PiggyCards (101–103) | MP-05, MP-09, **B1** | L + M | — | when B1 arrives (ships disabled before) |
| MP-11 | Buy & Sell: Topper, Uphold, Coinbase, SwapKit (087–094) | **B1** | 4 × L | — | per partner as B1 arrives |
| MP-12 | ZenLedger (040) | **B1** | S | — | when B1 arrives |
| MP-13 | CrowdNode, withdraw-only (091) | — | M | engine Y / UI N | 1.2, or drop |
| MP-14 | Localization infrastructure, Transifex keys, plurals, RTL check (118) | U-01 | M | N | 1.0 (infrastructure) |
| MP-15 | Storage explorer / developer tools (115) | — | S | engine Y | 1.0 |

### W — Windows

| ID | Task | Deps | Size | Owner | Tier | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|---|
| W-01 | Windows build and installer of the chosen stack in CI | G-04, T-03 | M | infra | CI, GW | N | Installs and opens a testnet wallet on `windows-latest`. |
| W-02 | OS services: DPAPI store for unencrypted mode, Windows Hello quick unlock, URL schemes, single instance, toasts, autostart, tray | W-01 | L | engine + UI | GW (Hello on T-04 or a real PC) | engine Y / UI N | IOS-011 and QT-001/009/150 rows on Windows. |
| W-03 | Code signing | W-01, **B2** | S | infra | CI | — | Signed installer. |

### PK — Packaging and release

| ID | Task | Deps | Size | Owner | Stack-indep | Acceptance |
|---|---|---|---|---|---|---|
| PK-01 | Linux: Flatpak (GNOME runtime; Flathub manifest), AppImage or portable tarball, `.deb`, a `.desktop` file with scheme handlers, AppStream metadata | G-04 | M | infra | N | Installs on Ubuntu 24.04, Fedora and Debian 12 containers or VMs; URL schemes work. |
| PK-02 | macOS: universal `.app` + dmg built in CI; URL types; keychain access group. Signed and notarized once **B2** arrives. | G-04, T-03 | M | infra | N | The unsigned dmg installs and launches on `macos-latest`. With B2: notarized, with the first launch checked in the B4 session. |
| PK-03 | Updates: a signed feed, off for Flatpak | PK-01, PK-02, B2 | M | infra | N | N−1 → N on all three OSes. 1.1. |
| PK-04 | Release engineering: versioning, SBOM, checksums, release checklist | PK-01, PK-02 | M | infra | N | The checklist is used for RC1. |

### H — Hardening and QA (1.0)

| ID | Task | Deps | Size | Stack-indep | Acceptance |
|---|---|---|---|---|---|
| H-01 | Security review: vault, scopes, leases, caps, IPC allowlist and CSP (webview XSS through DashPay text), avatar fetcher, deep links, invitation stash, renderer secret handling | DP3, DP4, DP5 (engine and UI) | L | N | Findings fixed or accepted in writing. |
| H-02 | Fuzzing: `dw-uri` (payment URIs, user and invitation links, `dapk`), QR payloads, PSBT, avatar decoding | DP2-03, DP5-01 | M | Y | 24 h without a crash; 10 min nightly. |
| H-03 | Kill -9 equivalence (DESIGN G5) across registration, request, payment and profile | DP3-01, DP4-01 | M | Y | Equal state ≤ 60 s after restart. |
| H-04 | Real-node GUI flows: Linux (T2/T3), Windows and macOS (CI runners) | U-01…U-07, the DP UI tasks | L | N | Every DP story passes on all three OSes. |
| H-05 | Accessibility: Orca, Narrator (CI or T-04), VoiceOver checks in the B4 session; keyboard-only DashPay flows | H-04 | M | N | No blockers. |
| H-06 | Performance: a large wallet with 500 contacts; a 10k-row history; cold restore with DashPay; contacts query < 100 ms at 1k contacts | DP3-02 | M | Y | The U5 budgets still hold. |
| H-07 | Mainnet canary (T4): a non-contested name, a contact round trip **with an iOS user**, payments both ways, one faucet invitation claim | DP5-02, **B3** | M | Y | Interoperability confirmed. |
| H-08 | `agentic-qa` full pass with evidence | H-04 | L | N | Report published; no open blocker or critical defect. |
| H-09 | Trust release gate: the layered provider on by default; the DP3-04 gate; mismatch handling | E0-10b, DP3-04 | M | Y | A forged quorum key makes every DashPay read and money-moving write refuse (or, in the degraded mode, a mismatch refuses). |
| H-10 | DAPI healthy-node pinning (3–5 nodes, ban on proof failure) | E0-05 | M | Y | Fewer reconnects in a sync burst (measured). |

## 3. Waves

Each wave is about 1–1.5 weeks. Each lane is one agent and runs its tasks in order. No task is scheduled before the
waves of its dependencies. Where a task depends on one in another lane of the same wave, the dependency is listed
first in its lane, and is noted.

| Wave | E1 (engine) | E2 (engine) | UI | L4 (infra, later a second UI agent) |
|---|---|---|---|---|
| **W1 — now** | **E0-01** pin bump (edit now, regression after D1) → **E0-02** → **E0-07** | **E0-03** signer glue (on the pin) | **G-01** Tauri spike | **D1** (running) → **S-01** → **T-03** CI |
| W2 — after D1, E0-01, T-03 | E0-08 → E0-10a | E0-03 (cont.) → E0-04 | G-02 Tauri measurements | G-03 SwiftCrossUI measurements |
| W3 | E0-05 (L) | DP1-01 → E0-11 → E0-12 *(if E0-10a failed: E0-10c first; E0-11/E0-12 move to L4 in W4)* | **G-04 (manager)** → E0-13 → U-07 | T-01 → T-02 |
| W4 | E0-06 → E0-10b | DP1-02 (L) | U-01 → U-03 | E0-09 → MP-03 → MP-15 |
| W5 | DP1-03 → DP1-04 | DP1-05 → DP1-06 | U-03 (cont.) → DP1-08 | U-02 → U-04 |
| W6 | DP2-01 → DP2-02 → DP2-04 | DP2-05 → DP2-03 → DP3-02 (both after E1's DP2-01) | DP1-09 → DP1-10 | DP1-07 → U-05 → U-06 |
| W7 | DP3-01 (L) | X1a (engine) → DP4-01 → DP4-02 | DP2-07 → DP2-08 | DP2-06 → E0-14 (milestone bump; rebases E0-11's branch) → W-01 |
| W8 | DP5-01 → DP5-02 → DP3-04 | DP4-02 (cont.) → DP3-03 → DP6-01/02 (engine) → MP-01 (engine) | DP2-09 → DP3-06 → DP3-07 | DP3-05 → PK-01 → X1a + MP-02 (UI) |
| W9 | H-02 → H-10 | MG-01 (L; the first 1.1 task, engine-only) | DP4-03 → DP5-03 → DP6-01/02/03 (UI) → MP-01 (UI) | W-02 → PK-02 → MP-14 |
| W10 — hardening | H-01 → H-09 | H-03 → H-06 | H-04 → H-05 → U-08 | H-07 (B3) → PK-04 |
| W11 — QA | fixes from H-01, H-08 | fixes from H-03, H-06, H-08 | fixes from H-04, H-05, H-08 | H-08 → **RC1** |
| W12 — 1.1 | MG-02 (L) | MG-05 → MG-03 | MG-00 → PK-03 | E0-14 (v5.1 move, or the X3 storage patch on the carried branch if the move is not possible yet) |
| W13 | MG-06 → MG-07 | MG-04 (L) | E0-13 (MG facades) → MG-09 (after E1's MG-06) | X3 storage patch + X3 (engine) → X1b (engine, L) |
| W14 | MG-04b (L, part 1) | MG-13 → MG-08 | MG-10 (after E2's MG-13) → MG-11 | X2 (engine, L) |
| W15 | MG-04b (L, part 2) | X2 (engine, cont.) → DP2-10 | X3 (UI) → X1b (UI) | X2 testnet shield/unshield suite → E0-10c follow-up if needed |
| W16 | MG-12 → fixes | fixes | MG-10b → X2 (UI) | H-08-style QA pass for 1.1 → **1.1 RC** |

**Start now (wave 1).** Four agents, with D1 already running in L4:

- E0-01 → E0-02 → E0-07 (lane E1);
- E0-03 (lane E2);
- G-01 (UI lane);
- after D1 merges, S-01 then T-03 (L4).

None of them depends on the UI-stack decision.

**Not scheduled, on purpose:**

- T-04 (contingency);
- W-03 and the signed halves of PK-02 and PK-03 (they wait for B2);
- MP-10…12 (they wait for B1);
- X4 and MP-04…09 and 13, which are 1.2.

**Critical path.**

- **Engine:** E0-03 (W1–W2) → E0-05 (W3) → E0-06 (W4) → DP2-01 → DP2-02 (W6) → DP3-01 (W7) → DP3-04 (W8) → H-01/H-09
  (W10) → H-08 (W11) → RC1. Registration runs alongside it: E0-04 → DP1-02 (W4) → DP5-02 (W8).
- **UI:** G-01 → G-02/G-03 → **G-04** (start of W3) → E0-13 → U-01 → U-03 → the DP UI tasks → H-04 (W10). A late G-04
  delays only the UI lane.

## 4. Effort

The sums use S = 0.5–1, M = 2–3 and L = 4–5 agent-days.

| Part | Agent-days |
|---|---|
| E0 (incl. one pin-train bump) + S-01 | 29–42 |
| T | 6–9 |
| G (incl. fix budgets) | 10–15 |
| U, Tauri (about 10 if SwiftCrossUI) | 25–32 |
| DP1–DP6 | 76–109 |
| X1a + MP-01/03/14/15 | 8–12 |
| W-01/02 + PK-01/02/04 | 12–17 |
| H | 26–36 |
| **To RC1** | **about 190–270 agent-days, 12–16 calendar weeks at four agents**, plus review latency |
| **1.1**: MG 48–68 (incl. MG-01 in W9 and the MG binding in E0-13), X1b 8–10, X2 12–15, X3 9–13, the v5.1 move 2–3, PK-03 2–3, DP2-10 2–3, QA 4–5 | about **90–120 agent-days, W12–W16, 6–8 weeks** |
| 1.2: X4, MP-04…09 and 13, MP-10…12 as B1 arrives | to be planned after 1.1 |

## 5. Gates and decision points

| When | Gate | Owner |
|---|---|---|
| start of W3 | **G-04**: UI stack (DASHPAY §2.1 rule) | manager (DEC-12) |
| W2 | **E0-10a**: does SPV serve Platform quorums? A pass keeps R7 as written. A fail → the degraded mode for 1.0, and E0-10c from W3. | engine, decided rule (DASHPAY §2.2) |
| if E0-10a fails | publishing a rust-dashcore backport on pasta's fork for a graph-wide `[patch]` (DEC-09) | manager |
| before the first push to `PastaPastaPasta/platform` (E0-11, W3) | record the decision to carry platform patches on pasta's fork (DASHPAY §2.3) | manager |
| before W7 (DP3-01) | E0-11's branch compiled and pinned, unless the v5.1 move happened | engine |
| W7 and W12 | E0-14: milestone pin bump; the v5.1 move when DASHPAY §3.9's criteria hold | engine |
| W9–W10 | a real-Mac session (B4): Touch ID, notarized first launch, VoiceOver | external |
| W10 | the mainnet canary budget (B3) | external |

## 6. Cutting for 1.0

**Cut in this order if the schedule slips:**

1. H-10 healthy-node pinning
2. MP-15 and MP-03
3. X1a (keep the shielded notice)
4. DP6-01/02, the Identities page and withdraw (keep the profile sheet)
5. Windows Hello in W-02 (passphrase only on Windows)
6. the tray companion on Windows and Linux (part of U-01)
7. MP-01 fiat
8. MP-14 (English only)

**Never cut:**

- resumable registration and asset-lock recovery (DP1-02);
- DashPay restore correctness (DP1-05, DP2-06);
- contact payments (DP3-01);
- invitation claim (DP5-02);
- the kill -9 matrix (H-03);
- the security review (H-01);
- the trust gate H-09, in whichever mode E0-10a decided.
