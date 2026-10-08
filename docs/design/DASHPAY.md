# DashPay on the desktop — reconciled design (authoritative)

Status: **authoritative**, 2026-10-08. This file reconciles two independent designs written on 2026-10-08:

- Fable's, `DASHPAY-fable.md`, on the local branch `dw/dashpay-design-fable` (`e1c7fce`);
- Opus's, `DASHPAY-opus.md`, on the local branch `dw/dashpay-design-opus` (`400e307`).

Neither source is on `main`. This file stands on its own and wins wherever they differ. The task plan is
[`ROADMAP.md`](ROADMAP.md). The external blockers (credentials, signing identities, money, hardware) are in
[`DECISIONS-PENDING.md`](DECISIONS-PENDING.md). This file amends `DESIGN.md` where §8 says so.

**Manager decisions applied.** These come from the program manager's decision log, kept outside this repository
(2026-10-08). pasta delegated product decisions to the program manager that day.

| Decision | What it settles |
|---|---|
| **DEC-01** | Wallet-holder masternode and governance flows come to the desktop after the DashPay core (§5a), and `CLAUDE.md` "Product scope" changes with them. |
| **DEC-12** | The manager picks the UI stack from the gate's data. |
| **DEC-13** | `main` is pushed to GitHub, and GitHub Actions runs on Linux, macOS and Windows runners. |
| **DEC-15** | Invitation claim in 1.0 and creation in 1.1; the `dashpay://user` QR, with DIP-15 accepted on scan; pin to v5.0-dev head now; integrations ship disabled until DCG supplies credentials. |
| **DEC-09** | rust-dashcore changes made in this program are not published without pasta's approval. |

The trust model and the timing of shielded and Platform-address support are decided here (§2.2, §2.10).

**(verified)** marks a claim re-checked for this reconciliation on agentbox. The checks ran against:

- this repo at `05161d9`;
- dashpay/platform at the pin `bc321362b9`, `origin/v5.0-dev` `bc41f1bc23` and `origin/v5.1-dev` `6499c680c6`, read
  with `git show <rev>:<path>` in `~/workspace/platform`;
- rust-dashcore at `e4208c90` (the pin) and `40268cc0` (what v5.0-dev head pins).

Path shorthands: `PW/` = `packages/rs-platform-wallet/src/`, `PWS/` = `packages/rs-platform-wallet-storage/src/` and
`PWF/` = `packages/rs-platform-wallet-ffi/src/`, all at the pin unless a revision is named.

---

## 0. Decisions at a glance

| # | Topic | Decision | Status |
|---|---|---|---|
| R1 | Engine | Keep the Rust engine on `platform-wallet` + `SqlitePersister`. DashPay is glue in `dw-engine/src/platform/`: signers, bring-up order, read models and flows. No protocol code of our own. | locked (both designs) |
| R2 | Engine API | A plain-Rust `DashPay` facade in dw-engine that knows no binding. The binding (UniFFI or Tauri commands) is a thin 1:1 wrapper added after the UI gate. | locked (§2.4) |
| R3 | Pin | Bump to v5.0-dev head as the first DashPay PR (DEC-15), then one bump per milestone. Move to v5.1-dev at the first milestone bump where it carries v5.0-dev's fixes plus the DashPay restore fixes that target only v5.1 (§3.9), or when iOS moves, whichever is first. Until then, carry #4623 + #4997 on a branch of pasta's fork `PastaPastaPasta/platform`; no upstream PR unless pasta asks. | locked |
| R4 | Runtime | Engine workers get 8 MiB stacks. The **engine** owns the bring-up order: host → Platform subsystems (≤ 20 s) → SPV → sync loops. It runs in a cancellable engine task, so `start_spv` returns at once. | locked; both gaps verified (§2.5) |
| R5 | Authorization | `PlatformOp{max_duffs, max_credits}`, three signer scopes and flow leases. Contact payments keep `Spend`. | locked (§2.6) |
| R6 | Contact payments | Through our own `TxDraft`. The confirm sheet shows a same-size estimate. After Confirm, the host authorizes, and `prepare` redeems the grant and only then reserves the DIP-15 address with `reserve_payment_address`; a retry reuses that address. Broadcast follows at once. | locked (§2.3) |
| R7 | Proof trust | 1.0 requires SPV-verified quorum keys from a layered provider; money never moves on trust that came only from the fallback. If spike E0-10a fails, 1.0 ships in a degraded mode instead: the trusted provider, cross-checked against SPV wherever SPV has the quorum, and the gap disclosed. Full enforcement follows when the desktop's graph carries the dash-spv fix. | locked (§2.2) |
| R8 | UI | One UI codebase from now on; `MacUI` and `CrossUI` are frozen. The stack is chosen at gate G-UI, a head-to-head measured on agentbox and on CI's macOS and Windows runners. Recommendation: Tauri 2 + TypeScript/React. | the manager decides from the gate data (DEC-12) |
| R9 | Releases | **1.0**: the DashPay core (identity, username with own-contest status and a temporary name, contacts, contact payments, notifications, profile and avatars, invitation claim), plus a read-only Platform-address balance (X1a). **1.1**: wallet-holder masternodes and governance (MG, DEC-01), invitation creation (X3), Platform-address transfers (X1b) and the shielded pool (X2). **1.2**: the username marketplace (X4). | locked (§2.10, §5a) |
| R10 | Testing | Tiers T0–T4: unit, regtest, a dashmate devnet on agentbox, testnet nightly, mainnet canary. GitHub Actions runs Linux, macOS and Windows (DEC-13). A KVM Windows VM on agentbox only if the hosted runners fall short. | locked |
| R11 | Shell | Mobile information architecture in a sidebar, window title "Dash Wallet", dash-qt tools under Tools and Settings ▸ Advanced. | locked |
| R12 | Smaller answers | Explore without the Firebase SDK. Emit the plain `dashpay://user` QR; scan both plain and `dapk`. Claim invitations in 1.0, create them in 1.1. Integrations ship disabled until DCG supplies credentials. | locked (DEC-15) |
| R13 | Masternodes and governance | Wallet-holder flows (voting, proposals, "my masternodes", the registration wizard, v24 shared masternodes) with mobile-inspired UX, salvaged from the parked branches, in milestone MG after the DashPay core. Operator and server work stays with the node and its CLI. | locked (DEC-01, §5a) |

---

## 1. What was verified, including corrections to both sources

| Claim | Result | Evidence |
|---|---|---|
| The engine runs on tokio's default 2 MiB worker stack | **true** (both sources found it). The library runs its own DashPay loop on 8 MiB because proof descent overflows smaller stacks, and both FFI runtimes use 8 MiB workers. It bites as soon as an on-demand DashPay call runs on an engine worker. | `rust/crates/dw-engine/src/engine.rs` `Engine::new` (no `thread_stack_size`); `PW/manager/dashpay_sync.rs:75-84`; `PWF/runtime.rs:24,36`; `packages/rs-sdk-ffi/src/runtime.rs:63` |
| Contact accounts must exist before the compact-filter scan passes their funding heights | **true**. The accounts come into being only when a signer-present drain runs. Our host starts SPV directly, so nothing runs first. `LifecycleQueue` awaits `startSPV` serially, so anything slow inside it blocks network switch, stop, import and remove. | `PW/manager/startup.rs:1-36`; `Sources/WalletRuntime/Host/LifecycleQueue.swift:154-165`; `dw-engine/src/session.rs` `start_spv_inner` |
| The library's startup outcomes | 7 `WalletStartupStatus` variants. The "3 s budget for a never-restored wallet" is ours; the library's only 3 s value is a discovery backoff. | `PW/manager/startup.rs:184-234` |
| `send_payment` derives, marks used, builds, signs and broadcasts in one call | **true** | `PW/wallet/identity/network/payments.rs:1080` |
| Contact address pools | DIP-15 recommends a gap of 10 (`DEFAULT_CONTACT_GAP_LIMIT`), but key-wallet builds the `DashpayReceivingFunds` and `DashpayExternalAccount` pools with a gap of **20**. The library documents the constant as unused for pools. | `PW/wallet/identity/crypto/dip14.rs:250-260`; rust-dashcore `e4208c90` `key-wallet/src/managed_account/managed_account_type.rs` |
| #4623 `reserve_payment_address` is only on v5.1-dev | **true**. It is the merge commit `6499c680c6`, the current head of v5.1-dev (5 files, +682/−98). The call drains contact crypto, takes a contact-payment gate, marks the next address used, persists and flushes, and never releases the address. | `git show --stat 6499c680c6` |
| #4623 can be carried on v5.0-dev | **applies as text**. Replaying the merge against its first parent onto v5.0-dev head gives a clean `git merge-tree`, and #4997 (`e52344c905`) stacks cleanly on top. It has **not been compiled** yet (E0-11). | `git merge-tree --write-tree` |
| The pin lets the engine reserve an address itself | **only partly**. `PlatformWallet::wallet_manager()` is public, but the pool changeset helper `account_address_pool_entries` is `pub(crate)` and the contact-payment gate does not exist. | `PW/wallet/platform_wallet.rs:431`; `PW/changeset/changeset.rs:1924` |
| M1 send order | `review → authorize(Spend) → prepare → .confirm(summary) → confirm() → broadcast`. The prompt and `prepare` (which reserves the inputs) come *before* the confirm sheet. | `docs/contracts/m1-swift.md:151-174`; `m1-engine.md` §2.7 |
| Grants and long flows | Grants are single-use with a 120 s TTL. A grant authorized by passphrase on a locked vault carries its own key, and `Vault::signer` moves that key into a **Full-scope** `VaultSigner` that lives until it is dropped or the vault locks. So a long flow *can* hold a signer today, but it is unscoped and uncapped. | `m1-engine.md` §2.2; `rust/crates/dw-vault/src/vault.rs` `Vault::signer` |
| Registration timing | Registration waits up to 300 s for InstantSend, then for a ChainLock **with no time limit** (`upgrade_to_chain_lock_proof(None)`). The 180 s bound is used only by the shielded seed pool. Both sources had "300 s + 180 s". | `PW/wallet/asset_lock/orchestration.rs:51-66`; `PW/wallet/identity/network/registration.rs:188,249` |
| The SDK's `ContextProvider` is synchronous | **true**. `SpvRuntime::get_quorum_public_key` is async, so an SPV provider needs a cache that a background task fills. | `packages/rs-context-provider/src/provider.rs:87`; `PW/spv/runtime.rs:340` |
| dash-spv can look up Platform signing quorums | **the mechanism exists**. `quorum_entry_for_hash_at_or_before_height` walks back through retained lists for quorums at lagged heights (its tests use `LlmqtypeDevnetPlatform`). It floors the walk at a fixed number of active windows and skips only `Invalid` entries, so unverified entries come back too. Whether the lists dash-spv keeps on testnet and mainnet cover Platform's proof heights is **unknown** (spike E0-10a). | rust-dashcore `e4208c90` `dash/src/sml/masternode_list_engine/helpers.rs:68,103`; `dash-spv/src/client/queries.rs:48` |
| rust-dashcore fixes reach the desktop directly | **no**. `40268cc0` is `e4208c90` plus 5 rpc-json commits. rust-dashcore `dev` has 88 commits that are not in it, including #1072 (keep masternode lists within a retention window) and #1075 (rebuild a pruned quorum lookup from storage). They reach the desktop only through platform's rust-dashcore pin, or through a `[patch]` that moves rust-dashcore for the whole graph. | `git log 40268cc0..dev` |
| Changes between the pin and head | Signer traits, `startup.rs` and the storage crate did not change between the pin and v5.0-dev head, so the signer glue can start before the bump. | `git diff bc321362b9 origin/v5.0-dev` (empty for those files) |
| What each platform branch carries | v5.0-dev head has #4978, #5206, #5294 and #5305; #4978 is not on v5.1-dev. #4997 and #4764 are on v5.1-dev only; the `v4.3-dev` branch they were merged into no longer exists. v5.0-dev head pins rust-dashcore `40268cc0` and grovedb `9791d277`. | `git log --grep` per branch; head `Cargo.toml` |
| Open DashPay fixes | Only #5026 targets v5.0-dev. #5256 (one-way contacts) and the restore stack (#5150, #5207, #5210, #5220, built on #5307's rust-dashcore update) target **v5.1-dev only**, as does #5288. | `gh pr view` |
| Main identity | Not a library concept, so the main-identity choice is ours, stored in dw-appdb. | no hits at the pin or at head |
| SwiftCrossUI backends | The vendored 0.10.0 has AppKit (4.5k lines, compile-checked `FullAppBackend`), GTK (3.3k) and WinUI (4.4k, never built; Fable said 5.3k). | `Vendor/swift-cross-ui/Sources/*Backend` |
| SwiftCrossUI on Windows | Issue #787 is **open** (filed 2026-09-25). UI Automation clients crash WinUI apps whose window contains List, NavigationSplitView, ScrollView or TextField, and windows that survive expose one element. The upstream WinUI bug behind it is microsoft-ui-xaml#11028. | `gh issue view 787 --repo stackotter/swift-cross-ui` |
| SwiftCrossUI bus factor | About one: the top contributor has 685 commits, the next 73. 0.10.0 was released 2026-09-30. | GitHub API |
| dashmate v5 images | `drive` 5.0.0-beta.1 and beta.2; `rs-dapi` and `dashmate-helper` 5.0.0-beta.2; `dashd` 24.0.0-rc.3; `tenderdash` 1.8.1; `quorum-list-server:latest` all exist. Head's dashmate is 5.0.0-beta.2. | `docker manifest inspect`; `packages/dashmate/package.json` |
| Explore database | Readable without auth; an AES zip whose password is the `Data-Checksum` metadata. Both sources checked this live on 2026-10-08. | sources (not re-run) |
| Repo state | No `.github/`; agentbox has no host Swift. Line counts: MacUI + DashUIMac 17.6k, CrossUI + DashUICross + DashWalletCross 8.4k, WalletFeatures 13.1k, WalletRuntime + DashKit 13.7k, WalletDemo 3.5k, Swift tests 14.8k, Rust crates 55.7k. | `ls -a`, `wc -l` |
| Parked masternode/governance work (DEC-01) | **`m3/r2-governance`** (10 commits, 39 files, +7,978): crate `dw-governance` (objects, votes, proposals, clock, tallies, govsync), `dw-engine/src/governance/`, FFI, dwcli `gov …`, console `gobject`/`getgovernanceinfo`/`getsuperblockbudget`, a regtest suite against 4 masternodes, oracle tests (object hashes and collateral scripts equal dashd's), and the G7 mainnet measurement (SPV govsync matches Core's Y/N counts). **`m3/r3-protx`** (10 commits, 38 files, +9,537): crate `dw-protx` (service rules, operator BLS keys, classic ProTx builders, v24 shared-masternode codecs), the masternode list model, ProTx flows, vault masternode keys, FFI, dwcli and console, and a regtest ProTx suite. Both have Swift runtime adapters and **no screens**, and both are based on pre-bump `main` and the M3 FFI. `main` keeps the masternode keychain (IOS-083, `374c883`). | `git log`/`git diff --stat main...<branch>` |
| pasta's platform fork | `PastaPastaPasta/platform` exists (a public fork of dashpay/platform), and this box's `gh` login is `PastaPastaPasta` | `gh repo view`, `gh api user` |

---

## 2. The disagreements, decided

### 2.1 UI stack (R8)

Both sources agree that the two UI trees end now. Fable wants SwiftCrossUI everywhere, with the AppKit backend on
macOS and MacUI retired after a half-day Mac gate. Opus wants Tauri 2 + TypeScript/React, retiring Swift, UniFFI and
SwiftCrossUI behind a one-week measured spike, with SwiftCrossUI everywhere as the fallback.

**Decision.**

- One UI codebase.
- From today no new screen is written in `MacUI` or `CrossUI`; they get bug fixes only.
- DashPay screens are written only in the stack that gate G-UI selects.
- The engine (§3) and the view-model design (§4) are stack-neutral, so nothing on the engine's critical path waits for
  the gate.

**Who decides.** The program manager, from the gate's data (DEC-12, under pasta's 2026-10-08 delegation). pasta can
revisit the choice; ADR 0003 records the numbers and the reasons.

**Recommendation: Tauri 2 + TypeScript/React, with the Rust engine linked in-process.**

Why, each point checked:

1. **Windows.**
   - SwiftCrossUI has no accessible native Windows path today (#787, verified open).
   - Its fallback is GTK on Windows over a Swift-on-Windows toolchain that this project has never built.
   - Tauri on Windows uses WebView2, which ships with Windows 10/11 and has mature UI Automation support.
2. **Day-to-day QA.** agentbox is Linux, with no host Swift and no Mac. CI adds hosted macOS and Windows runners
   (DEC-13), which serve both stacks equally for building and smoke tests. The difference is the daily loop:
   - Tauri's UI builds, runs and is tested on the agentbox host. Playwright drives Chromium (the engine inside
     WebView2) and WebKit (Playwright's own build, close to WKWebView and WebKitGTK), and tauri-driver runs the real
     WebKitGTK webview. So every UI change is seen in all three engine families before it reaches CI.
   - Under SwiftCrossUI everywhere, AppKit and Windows behaviour is visible only through CI round trips, and GTK under
     Xvfb does not predict AppKit layout.
3. **Fidelity.** UX-SPEC §3.0 lists what SwiftCrossUI 0.10 cannot draw: shadows, opacity and scale effects, context
   menus, continuous corners, SVG and charts. §2.6 adds that it has no usable animation API. CSS can do all of them.
4. **Long-term risk.** SwiftCrossUI's bus factor is about one. Tauri and React are mainstream, and agents are most
   fluent in TypeScript/React.
5. **Cost.**
   - Porting the M0–M3 screens and view models to TypeScript costs about 25–32 agent-days. Fable's path costs about
     5–10 agent-days.
   - The port runs in its own lane alongside the engine work, which is DashPay's critical path (about 8–10 weeks of
     engine tasks). Its calendar cost is therefore small; its agent cost is real.
6. **What we give up.**
   - SwiftUI's native feel on macOS. UX-SPEC already paints an iOS look, so this matters less than it sounds.
   - Free native-control accessibility; it has to be earned with ARIA and axe checks in CI.
   - About 52k lines of Swift (UI, view models, runtime) and their tests. The engine (55.7k lines), the contracts,
     `dwcli` and the regtest suites stay.
   - A webview renders attacker-controlled DashPay text: strangers' display names, bios and avatars. That is a new XSS
     surface, covered by gate criterion U6 and the security review H-01.

**Rejected alternatives.**

- **Keep two UIs.** Both sources reject it: every DashPay screen is written twice, and half of them wait for a scarce
  Mac.
- **Fable's sequence**: DashPay screens written in CrossUI at once, then a half-day manual Mac walk (G9), with
  Windows left to the later gate G3. It commits the DashPay UI before either of the stack's two open risks, AppKit
  quality and Windows (#787), has been measured. CI now makes measuring both cheap, so G-03 measures them up front, on
  the same criteria as Tauri.
- **egui, as in dash-evo-tool.** It was research 04's Windows escape hatch. It gives up iOS fidelity and offers
  nothing over Tauri for QA on the Linux box.

#### Gate G-UI: a head-to-head

**Where it runs.**

- agentbox: the host build (Rust, Node, WebKitGTK development packages) and Docker for Linux packaging;
- GitHub Actions: `ubuntu-latest`, `windows-latest` and `macos-latest` (DEC-13; T-03 sets up the workflows).

No Mac session and no Windows VM are needed.

**Both candidates are measured on the same criteria, in parallel, each with a timebox.** Going over the timebox counts
as a fail.

| Candidate | Tasks | Timebox |
|---|---|---|
| Tauri | G-01 (build), G-02 (measure) | 5 agent-days + ≤ 3 for fixes |
| SwiftCrossUI everywhere | G-03 | 4 agent-days + ≤ 2 for fixes |

**What the Tauri side builds (G-01).**

- `apps/desktop` (Tauri 2, React, TypeScript, Vite) and `rust/crates/dw-app`: Tauri commands over dw-engine, linked
  in-process, with TypeScript types generated from the Rust records.
- Commands: open a network, list wallets, unlock, balance, sync status, history page, transaction detail.
- Screens, in light and dark, per UX-SPEC §4.5 and §4.9: Overview (hero, shortcuts, history cards), Transactions,
  transaction detail and unlock.
- CSS custom properties generated from the same token source that `scripts/gen-tokens.swift` reads.
- An event bridge that turns engine signals into query invalidation.
- A TypeScript fixture backend for screenshot tests, and a `--selftest` mode that renders the four screens and writes
  the box of every `data-testid` element to a JSON file.

**What the SwiftCrossUI side uses (G-03).** It starts from the existing `dash-wallet` app, which already has Overview,
Transactions, the transaction detail and unlock.

- Built on `macos-latest` with AppKitBackend.
- Built on `windows-latest` with WinUIBackend, using the standard Swift-on-Windows setup action. If WinUI fails U4,
  GtkBackend is tried as well.
- On Linux, the existing AT-SPI harness (ADR 0002).

**Test data.** A regtest wallet from the existing harness with at least 10,000 generated transactions, copied to the
runners.

**Pass criteria.** All of them must pass, for each candidate.

| # | Criterion | Measure | Pass |
|---|---|---|---|
| U1 | Correct on the real engine | On Linux, Overview and Transactions against `dwcli` on the same datadir | identical balance and identical first 200 rows |
| U2 | Windows | On `windows-latest`: build; install (an MSI or NSIS installer for Tauri; an archive with the Swift runtime for SwiftCrossUI); launch; open the copied wallet; screenshot | Overview renders; no crash in 10 minutes |
| U3 | Fidelity | Screenshots of the four screens, light and dark, on Linux, Windows and macOS, with the fonts bundled. For Tauri also: Playwright on agentbox in Chromium and WebKit, tauri-driver on the real WebKitGTK and WebView2, and the `--selftest` boxes from the macOS runner. | **(a) Pass/fail:** every item of the UX-SPEC §2 checklist (colour tokens, radii, elevation, type ramp, spacing) is checked per screen against **the column that applies to the candidate**: the main spec for Tauri, UX-SPEC's Cross column (§2.4, §2.5, §3) for SwiftCrossUI. Tauri is checked by computed-style assertions, SwiftCrossUI by review of the screenshots. Pass = no item fails its own column. **(b) Recorded, not pass/fail:** the fidelity gap, meaning the number of items where the candidate's column falls short of the main spec. It feeds rule 3. **(c)** Tauri only: the box of every layout container (`data-testid` on containers, not on text runs) agrees within 2 px across the engines. |
| U4 | Accessibility | AT-SPI walk on Linux; the #787 UI Automation walk script on `windows-latest`; axe-core (Tauri); a keyboard-only run | 0 unnamed interactive elements; the UIA walk completes without a crash and names every control; 0 serious or critical axe issues; every action is reachable by keyboard |
| U5 | Performance (release build, agentbox) | `history_page(200)` round trip (Tauri: IPC; SwiftCrossUI: FFI call + render); cold start to first paint; PSS of all the app's processes, idle, on the 10k-transaction wallet | p95 ≤ 20 ms; ≤ 1.5 s; ≤ 400 MB. Opus measured RSS ≤ 350 MB on a bait9-sized wallet instead. The gate isolates the UI's own cost: engine memory on huge wallets is the same under both stacks and belongs to H-06, and PSS counts a webview's shared pages fairly. |
| U6 | Security (Tauri only; SwiftCrossUI has no renderer) | the capability allowlist; CSP; a navigation block; an XSS payload in a label and in a display-name fixture; the renderer calling an unlisted command | only listed commands can be called; nothing remote loads or navigates; payloads render as text; the unlisted call is refused. How the renderer handles secrets (the passphrase field, the phrase reveal) is written down for H-01. |
| U7 | Packaging | Linux on agentbox (Tauri: `.deb` + AppImage; SwiftCrossUI: tarball + a Flatpak manifest that builds); Windows installer or archive on `windows-latest`; unsigned `.app` / dmg on `macos-latest` | each one installs (or unpacks) and launches |
| U8 | macOS | On `macos-latest`: build, launch, a screenshot of each screen in light and dark | renders; no crash |

**Decision rule (the manager's).**

1. A candidate that still fails any must-pass criterion after its fix budget is out.
2. If only one candidate passes, it is chosen.
3. If both pass, **Tauri** is chosen, unless the recorded numbers (U3's fidelity gap, U5, the fix estimates) say
   otherwise. The port's 25–32 agent-days sit off the critical path, and they buy a mainstream stack, local QA of all
   three rendering engines and a large maintainer base. SwiftCrossUI's
   single-maintainer risk, and its WinUI path through #787, would remain for the life of the product. The manager may
   weigh the numbers differently; ADR 0003 records why.
4. If neither passes, the manager picks the candidate whose failures are cheaper to fix, using the fix estimates in
   ADR 0003.

**After the decision.**

- **Tauri.**
  - The port (U-01…U-07) starts.
  - Swift stays frozen. It is deleted only once the Tauri app reaches parity and the macOS checks pass (tag
    `swift-final`, U-08).
- **SwiftCrossUI everywhere.**
  - DashPay screens are written once, in CrossUI.
  - The MacUI-only behaviours move behind the existing `PlatformServicesMac` protocols: Dock menu, menu-bar companion,
    Touch ID quick unlock, saved window frames, backup reminder and `ScreenCaptureGuard`.
  - Windows uses whichever backend passed U4.
  - The facade is exposed through `dw-ffi` (UniFFI), 1:1.
  - MacUI is deleted once the Cross app ships as the macOS bundle.

### 2.2 Proof trust (R7)

Fable: the trusted quorum service at 1.0, as on iOS, with SPV-derived keys later. Opus: quorum keys cross-checked
against SPV for 1.0.

**Decision: 1.0 requires SPV-verified quorum keys.** The layered provider is built in the foundations phase (E0-10a,
E0-10b), not in hardening. Development uses the trusted provider until it lands.

The layered provider's policy (`platform/trust.rs`):

1. **Quorum keys come from dash-spv's masternode state** (`SpvRuntime::get_quorum_public_key`). They reach the
   synchronous `ContextProvider` through a cache that a background task fills.
2. **The trusted HTTPS service is consulted only for reads, and only while dash-spv's masternode state is not
   synced yet** (first launch, or after a long time offline). Results verified that way are marked unverified.
   Platform **writes wait** until SPV has synced (§3.7).
3. **Once SPV is synced, an unknown quorum never falls back.** Trying another DAPI node does not help, because every
   node's proof cites the same quorum. "Unknown" usually means SPV is behind a quorum that just formed, or has pruned
   an old one. So the engine waits a bounded time (default 2 minutes) for SPV to catch up, or rebuilds the lookup
   from storage, then fails closed. Otherwise an attacker who controls the fallback could cite a quorum SPV does not
   know and force the fallback.
4. **Disagreement is a refusal.** If SPV and the trusted service give different keys, the engine refuses with
   `platform.trust_mismatch` and emits `Notice{PlatformTrustMismatch}`.
5. **Money never moves on unverified data.** These wait until the data re-verifies against SPV, behind a short
   "Verifying with the network…" step:
   - registration;
   - top-up;
   - a payment to a contact whose channel data (contact request, xpub) or DPNS resolution was verified only through
     the fallback.

   **Provenance.** The library stores what it fetched without recording which quorum source verified it, and its
   rows carry no modification time. So the engine:
   - while the fallback is in use, has the changeset tap (§3.5) record every entity a changeset touches (identity
     ids, contact-request ids, DPNS labels) in dw-appdb `dp_trust_unverified(wallet_id, kind, key, since)`;
   - once SPV has synced, re-fetches those entities, and clears each row when its re-fetch verifies against SPV;
   - lets no money move to an entity that still has a row in **any** wallet (DP3-04): the gate queries
     `WHERE kind = ? AND key = ?`, not just the current `wallet_id`. wallet.sqlite holds one row per identity (a write
     from another wallet is a no-op), so wallet A's flag covers an identity wallet B relies on; a verified re-fetch of
     an identity clears its rows in every wallet. Contact-request and DPNS rows are per wallet there and are cleared
     by the owning wallet's re-fetch.
6. **Contracts and the activation height** may still come from the trusted provider; the SDK verifies contract
   fetches through proofs as well.

**The gate (E0-10a, wave 2).** It measures, on testnet and mainnet, read-only:

- whether dash-spv at the pin holds the Platform quorum type at the heights proofs reference: LLMQ_100_67 on mainnet,
  LLMQ_25_67 on testnet, the devnet types on dashmate;
- what verification status those entries carry. The lookup returns anything not `Invalid`, so the provider must
  require the strongest status dash-spv offers, unless the spike shows why the weaker one is still anchored to the
  chain;
- the time from a fresh install to the first SPV-verified proof;
- the miss rate over one day.

It runs in two parts:

1. **At platform's pin.** The run captures every (quorum type, quorum hash, core height) tuple that real proofs cite.
2. **A standalone dash-spv probe at rust-dashcore `dev`**, fed those tuples. This shows whether #1072
   (masternode-list retention window) and #1075 (rebuilding a pruned quorum lookup) close any gap. Moving the whole
   graph to `dev` would not compile: it crosses several breaking rust-dashcore changes that platform has not taken.

**If the spike passes**, R7 stands as written, and H-09 makes it a release gate.

**If the spike fails, 1.0 does not wait.** It ships in a degraded mode:

- the trusted provider supplies quorum keys;
- whenever SPV does have the quorum, its key is compared, and a mismatch refuses (rule 4);
- **rule 5 is suspended**, so money moves on trusted-provider data. That is iOS's trust level today;
- Tools ▸ Information and the release notes say that Platform data is verified against Dash's quorum service.

The dash-spv fix (E0-10c, scheduled right after the spike) is either our own, or #1072/#1075 if the probe shows they
are enough. Full enforcement ships **when the desktop's graph carries the fix**, by one of two routes:

- platform moves its rust-dashcore pin past it;
- or the fix is backported onto platform's pinned rust-dashcore revision on pasta's fork and pinned graph-wide with
  `[patch]`. Publishing that branch needs a manager decision under DEC-09, raised only if the spike fails.

**Why.**

- DashPay turns proofs into payment destinations: DPNS name → identity → contact request → xpub → address.
- The trusted service is a single point of compromise for every desktop and every phone.
- The SPV path exists at the pin and was written for exactly this lookup, so the expected cost is days, not weeks.
- The desktop already trusts SPV for its Core balance; anchoring Platform the same way is consistent.

Holding 1.0 for a failed spike would not help: the fix cannot reach the desktop's build before it is in platform's
pin. The degraded mode is no weaker than iOS today, and it still catches a compromised service whenever SPV knows the
quorum.

**Rejected.**

- Fable's trusted provider at 1.0 as the plan: it accepts a known single point of compromise when the fix is cheap.
- Opus's trusted fallback whenever SPV lacks the quorum: see rule 3.
- Holding 1.0 until SPV enforcement works: the fix's path to the desktop is outside this program's release control.

### 2.3 Contact payments (R6) and #4623

Both sources plan to send contact payments through our own `TxDraft`. That keeps confirm-before-broadcast, spend caps,
coin control, CoinJoin sources and the M1 rules for unknown outcomes. They disagreed on where the DIP-15 address comes
from, and when:

- **Fable:** prepare "through `send_payment`'s builder path with a reservation-only finalize", called a small engine
  refactor. But `send_payment` is upstream code, so this means either an upstream change or a copy of upstream logic.
- **Opus:** after Confirm, reserve the address with #4623's `reserve_payment_address`, which needs a backport. Opus
  kept the one-shot `send_payment` as a fallback if no backport lands.

**Decision: Opus's flow, with three corrections from review and the source of the primitive settled.**

**The flow.**

1. `TxDraft.set_recipients([Contact{identity, contact, amount}])` validates offline. The contact must be established,
   or have a pending incoming request when the action is "Accept and pay". The payment channel must be fine, and
   there must be no lock from an unknown outcome.
2. `estimate()` plans the payment with a placeholder P2PKH output of the same size, so the fee is identical.
3. The confirm sheet shows the contact, the amount, the exact fee and the total.
4. On Confirm, the **host** authorizes `Spend{amount + fee}`. For contact payments the prompt therefore comes *after*
   the sheet, while in M1 it comes before; `SendViewModel` gets a contact branch, and m1-swift is updated.
5. `prepare(grant)`:
   1. plans with the placeholder, checks the cap and redeems the grant. A failure up to here (`grant_exceeded`,
      `outpoint_unavailable`, `vault_locked`, …) consumes no address;
   2. drains contact crypto and reserves the next address (`reserve_payment_address`);
   3. swaps the reserved address into the same-size output, so the fee cannot change;
   4. signs and reserves the inputs.

   The reserved address is stored on the draft. Another `prepare` of the same draft reuses it instead of reserving a
   new one — for example after `send.no_peers`, which in M1 means the transaction was definitely never sent. Grants
   are single-use, so that retry prompts again, as in M1.
6. `broadcast` runs at once.
7. `reconcile_sent_payments_from_tx_history` attributes the payment.
8. An unknown outcome sets the per-contact lock (`dp_payment_lock`).

**Why the sheet shows an estimate for contact recipients.** iOS does the same, and M1's prepared transaction cannot be
used here:

- each reservation consumes an address index for good;
- key-wallet scans only 20 unused addresses past the last used one in a contact pool (DIP-15 recommends 10);
- so with M1's order, about twenty cancelled confirms would push the next payment out of the recipient's view.

Broadcast still happens only on Confirm.

**"Accept and pay".** Paying someone whose request is still pending needs two authorizations: `PlatformOp` for the
accept and `Spend` for the payment.

- The host asks for the credential once, which creates one flow lease (§2.6) carrying both, each with its own cap.
- The engine accepts first.
- Only after the accept succeeds does the payment's `prepare` redeem the spend part of the lease. The 120 s grant
  lifetime therefore cannot run out during the accept's Platform round trip.

**Where the primitive comes from.**

- #4623 applies to v5.0-dev head as text, and #4997 stacks on it (verified, not yet compiled).
- E0-11 puts "v5.0-dev + #4623 + #4997" on a branch of pasta's public fork `PastaPastaPasta/platform`. It holds
  upstream code only. DEC-13 covers only this repo, so the manager records this as its own decision before the first
  push (ROADMAP §5).
- When DP3-01 starts, the whole platform crate set is pinned to that branch, unless the v5.1 move (§3.9) has happened
  by then. One source, never mixed, so the graph holds one copy of each platform type.
- No upstream backport PR is opened unless pasta asks for one; the branch is ready for it.
- If pushing to the fork is ever unwanted, a patched copy of `rs-platform-wallet` under `Vendor/` (with a
  `PATCHES.md` entry, like the vendored SwiftCrossUI) is the fallback.

**Rejected.**

- The one-shot `send_payment` (Opus's fallback): it loses the spend cap, coin control, CoinJoin sources and the
  confirm-before-broadcast rule.
- Reserving at prepare time, before the sheet (Fable): the gap hazard above.
- Copying `reserve_contact_payment_address` into our engine: it duplicates upstream logic and its race fix, when the
  carried branch costs about a day.

### 2.4 Engine API shape (R2)

Fable: UniFFI methods on `NetworkSession`, one file per domain. Opus: a plain-Rust facade in dw-engine that the binding
wraps 1:1.

**Decision: Opus's facade**, starting from Fable's record and error sketches as its vocabulary (§3.6).

- It makes all engine work independent of the UI decision.
- It lets `dwcli` and the T2/T3 suites drive DashPay with no binding at all.
- It makes the binding a thin generated layer: UniFFI under SwiftCrossUI, Tauri commands in `dw-app` under Tauri.

**Rejected:** a binding-first API, which would tie the engine timeline to G-UI.

### 2.5 Runtime: worker stacks and bring-up order (R4)

Both sources propose both changes, and both verified the 2 MiB stack. Fable's `start_subsystems` already lives in
dw-engine; the open question was who sequences it against SPV (Fable §2.1 puts that in the `LifecycleQueue`).

**Decision.**

- **Stacks.** Engine workers and blocking threads get 8 MiB stacks.
- **The engine sequences the bring-up.** `start_spv` (and the session start that calls it) schedules bring-up then SPV
  in an engine task, and returns at once.
  - `LifecycleQueue` awaits `startSPV` serially, so a 20 s wait inside it would block network switch, stop, import and
    remove.
  - Closing the session cancels the task, bring-up included.
  - Meanwhile `is_spv_running` reports a `Starting` state.
- **Every host gets the same order:** Swift, Tauri and `dwcli`.

**Rejected:** host sequencing. It would be duplicated in every host and lost in a UI switch.

### 2.6 Grants, signer scopes and leases (R5)

| | Fable | Opus |
|---|---|---|
| Signer scopes | one, `SignerScope::Platform` | `PlatformIdentity`, `DashPayCrypto`, `PlatformFunding{max_duffs}` |
| Funding | `Spend` | covered by `PlatformOp` |
| Identity-key writes | `PlatformOp`, uncapped | `PlatformOp{max_duffs, max_credits}` |
| Long flows and background work | — | flow leases; a background crypto lease |

**Decision: Opus's model, with the lease rules corrected.**

- **Least privilege.** A DashPay crypto drain can never sign a state transition or a spend.
- **Credits are money**, so writes that spend credits are capped like spends.
- **What a long flow holds.** Today a redeemed grant becomes a Full-scope `VaultSigner`, which lives until it is
  dropped or the vault locks (§1). A flow lease replaces it with a signer set that is scoped, capped, bound to one flow,
  and able to park and resume.
- **How long a lease lives.**
  - **Vault unlocked:** the lease signs with the vault's key and lives until the flow ends or the vault locks.
  - **Vault locked** (the grant carried its own key): the lease keeps that key through the InstantSend window, at
    most 300 s after broadcast, so a normal registration finishes with one prompt.
    - If the flow falls back to the ChainLock wait, which has no time limit, it drops the key and parks in
      `ProofWaiting`.
    - It asks again at identity creation ("Finish registering @alice").
    - Parking needs no aborted future. The flow is two library calls: `create_funded_asset_lock_proof`
      (`PW/wallet/asset_lock/build.rs:806`), which builds, tracks and broadcasts the lock and returns its 300 s
      InstantSend timeout instead of falling back, then `AssetLockFunding::FromExistingAssetLock`, called only once
      the tracked row holds a proof. The build-only `build_asset_lock_transaction` returns an untracked, unsent lock
      and is never used for a hand-off (E0-04 design §4.4).
  - **Lock always wins.** `lock()` revokes every lease. While a lease holds a key on a locked vault, the UI says so,
    with the copy of the E0-04 design §16.10:
    - "Registration in progress — Lock to cancel" before the funds are committed and while no library call runs;
    - "Lock stops new signatures; a transaction already signed may still be sent" while a call runs (DEC-67);
    - "Funds committed — finishing. Lock to stop; you'll finish after you unlock" once funded, between calls;
    - after a lock, manual or automatic, the line that follows the flow's observed outcome (§16.10 C4 and C5). A bare
      "unlock to finish" shows only when nothing of the flow is still pending.
  - The vault releases no signature or crypto result of an epoch once `lock()` has returned (E0-03; m1-engine
    §2.2). A result released just before the lock can still reach the flow; the commit-point rule below is what
    keeps it from going out (E0-04).
- **Commit points: Lock against a flow's hand-off** (E0-04; reviews DW-E0-03 r3 M1, r4 M1 and m1). The E0-04
  design, [`E0-04-grants-leases.md`](E0-04-grants-leases.md), approved at 723d9d3 (DEC-73), is normative. This is
  its summary.
  - **Two modes** (design §2a). Mode A carries one platform PR: an rs-sdk broadcast hook and platform-wallet's fence
    wiring. It needs pasta's go-ahead (DECISIONS-PENDING B5) and a cherry-pick into the pin (DEC-18). Mode B does
    without it. Everything common to both lands first, and E0-04 and DP1-02 can close in either. In Mode B, for the
    flows the library drives, Lock keeps DEC-67's promise: "Lock stops new signatures; a transaction already signed
    may still be sent".
  - **The rule** (design §0, §5). A flow's commit is the hand-off of a signed artifact to a transport. One step under
    the lease table's mutex J decides every hand-off (`admit`):
    - it reads the artifact's entry, checks the origin lease and charges the budget;
    - it compare-and-sets `Unsent → Committing`, granting a permit whose deadline is the grant plus H (10 s), or
      `Unsent → Revoked`.

    The transport starts only after the durable record has returned. So each hand-off is ordered wholly before or
    wholly after a lock, and only the compare-and-set's winner cleans up, in a spawned task that frees the inputs.
  - **The dispatch journal** (design §6). The record is not in platform-wallet's changeset. It is a host-owned file,
    `<network>/dispatch.sqlite` (`synchronous=FULL`, §3.4), that only the engine's fence writes:
    - in Mode A it holds each asset lock, registered with a recovery payload before its `Built` row is tracked, and
      the write-ahead markers of resumable steps;
    - in Mode B it holds the step and funding markers only.

    Rows from before the journal are seeded `PreFence`: possibly sent, so they are only ever resent. A row with no
    entry is neither sent nor cleaned up. Tools ▸ Repair offers "Send it", or "Cancel it", a self-spend that must
    be ChainLocked before the row counts as not sent.
  - **Row-less hand-offs** (state transitions, contact payments; design §5.5). The fence tracks every attempt in its
    process. It settles an artifact definitely unsent only when no attempt still runs and none may have let it out.
  - **At lock time** (design §8).
    - `lock_vault` revokes every lease and snapshots the permits in one synchronous step.
    - A lock barrier keeps new leases out until both the vault gate and the drain are done. Every lock request runs
      its own vault gate, and only the drain is shared.
    - The drain waits for the snapshotted permits, each until its deadline.
    - `lock_vault` returns within `max(H, T_gate)` of its call, for any number of leases. A dropped caller or a
      second lock changes nothing.
    - The FFI's `Vault.lock()` stays synchronous. The drain's end arrives engine-side as
      `LockProgress::Done(LockReport)`.
  - **Outcomes** (design §4.6, §8.4).
    - A flow reports `Sent`, `WillBeSent` (committed, and the engine resends it), `MaybeSent` or `Cancelled`
      (nothing committed), from its artifacts' commit history.
    - No retry, discard or second funding is offered unless `dispatch_status` says `NotSent`, which needs positive
      evidence. `DispatchResolved` fires when a provisional outcome settles.
    - The copy, after a manual or an automatic lock, is the design's §16.10.
  - **A catch-up at load and on reconnect** (design §5.7 H6). dw has none at the pin: no dw code calls
    `resume_asset_lock` (design F6). E0-04 adds one. At load, possibly-sent inputs are fenced (and, in Mode A, a row
    lost to a power loss is restored from its payload) before any build runs. After SPV starts, and on every peers
    0 → >0 transition, every tracked row is resumed.
  - **Commit points at the pin** (`bc41f1bc23`; `PW` = `packages/rs-platform-wallet/src`). In Mode A the fence is
    called at each of them (design §5.1).

    | Flow | Signed artifact | Hand-off |
    |---|---|---|
    | Registration, top-up and invitation funding | asset-lock transaction | `self.broadcaster.broadcast(&tx)` (`PW/wallet/asset_lock/build.rs:1142`) → `SpvBroadcaster` → dash-spv `DashSpvClient::broadcast_transaction` |
    | Identity registration | IdentityCreate | `put_to_platform_and_wait_for_response_with_signer` (`PW/wallet/identity/network/registration.rs:228, 252`) → rs-sdk `BroadcastStateTransition::broadcast` |
    | Identity top-up | IdentityTopUp | `top_up_identity_with_signer_with_metadata` (`registration.rs:470, 493`) |
    | Identity key update | IdentityUpdate | `broadcast_and_wait` (`PW/wallet/identity/network/update.rs:188, 317`) |
    | DPNS name, profile, contactInfo, contact request and accept | document transitions | `put_to_platform_and_wait_for_response` (`PW/wallet/identity/network/document.rs:316`, `sdk_writer.rs:270`) |
    | Contact payment | Core transaction | `self.broadcaster.broadcast(&tx)` (`PW/wallet/identity/network/payments.rs:1446`) |
    | Invitation (X3) | asset lock, then the invitation identity | as funding above, then `put_to_platform_and_wait_for_response_with_private_key` (`PW/wallet/identity/network/invitation.rs:551`) |
    | Re-dispatch of bytes already signed | the same transaction again | asset-lock resume (`PW/wallet/asset_lock/sync/recovery.rs:1176, 1491`), reached from library flows and the library's deferred-resume task, and from dw only through E0-04's catch-up (H6): **dw has no launch catch-up until then** (design F6); dash-spv's rebroadcast timer. The unconfirmed-send replay at load (`PW/manager/load.rs:491-530`) never fires in dw, whose SQLite backend returns no unconfirmed outgoing sends (design F8) |

    The last row is a `Resend` only when the journal says the bytes may already be out (`Dispatching` or
    `PreFence`). A resume that reaches an `Unsent` row makes that row's `First` hand-off.

    Data-contract creation (`PW/wallet/identity/network/contract.rs:297, 545`) is not a DashPay flow. In every row
    but the last, platform-wallet signs and hands off inside one library call: the engine sees only its `Signer`
    adapter, which runs before the release.
  - **Spec check:** `python3 -I docs/design/checks/e0_04_design_model.py` (exit 0 = pass). The draft's design-input
    checks (`e0_04_dispatch_model.py`, `e0_04_split_model.py`, `e0_04_mutations.py`) stay as its history.
  - **Tests and the draft's open issues.** The acceptance lists are the design's §2a.4 (one per mode), with the
    tests of its §12. The draft's open issues (M-A, M-B, M-C, m-1 to m-3 and the nits) are closed in the design's
    §0 table and Appendix A.
- **The background `DashPayCrypto` lease.**
  - It exists while the vault is `Unlocked`, or unencrypted (no passphrase). It does not exist while
    `UnlockedMixingOnly` or `Locked`.
  - It is dropped on lock.
  - A passphrase change ends the vault's epoch while the vault stays unlocked (E0-03 review r3 m3). The lease's
    signer then fails `Locked`, and the engine issues a new one.
  - It lets the sweep build contact accounts without a prompt, and it can neither spend nor sign a state transition.
- **The `DashPayCrypto` scope** may derive exactly the contactInfo children `65536'`/`65537'` under the identity-auth
  root, and nothing else there. *(Amended by E0-03.)* It also needs the identity keys themselves for ECDH and the
  `accountReference` mask, because DIP-15's encryption and decryption keys are identity keys
  (`PW/wallet/identity/network/contact_requests.rs:534-586, 3659-3662`). Those operations return only hashed
  products, never a signature. The scope also needs the BIP44 account-0 xpub for the seed-binding check
  (`seed_binding.rs:200-222`).

**Rejected:**

- a single Platform scope: a drain could sign transitions;
- an uncapped `PlatformOp`;
- holding today's Full-scope signer for a whole registration.

### 2.7 Events and notifications

Fable: derive notifications from contact rows and payment entries. Opus: tap `WalletStore::store(changeset)` (we own
that wrapper) to emit debounced Platform signals and write an idempotent `dp_events` journal.

**Decision: Opus's tap.** It is one place and sees every library change, including those made by background loops.
Events from the first pass after a restore are stored as read, with no OS notification (catch-up silence). Fable's
derivation from rows is the rebuild path if the journal is lost, since the journal is display data.

### 2.8 Milestones and test tiers

**Milestones.** Opus's split, with Fable's test tasks and acceptance criteria folded in:

- foundations → identity and username → contacts → contact payments → profile → invitation claim → identity tools;
- extras X1–X4, the wallet-holder masternode and governance milestone MG (DEC-01, §5a), the remaining mobile parity
  (MP), Windows, packaging and hardening.

**Tiers.** T0–T4, defined in §6.

- Testnet is used from the first engine task, because it works today.
- The dashmate devnet becomes the gating tier for flows that need determinism (the kill -9 matrix, contests,
  invitations) once T-01 is green.

IDs and waves are in `ROADMAP.md`.

### 2.9 Smaller differences

| Topic | Fable | Opus | Decision |
|---|---|---|---|
| Paying a contact whose request is still pending | Android's silent auto-accept | explicit "Accept and pay" | explicit (§2.3) |
| Platform addresses, internal transfer | 1.0 (M4.4) | X1, a cut candidate | a read-only balance in 1.0 (X1a); the rest in 1.1 (X1b) (§2.10) |
| Shielded | 1.0 if its build gate passes | X2, cut first; feature build in CI early | 1.1 (X2), with the feature build measured and kept green from the bump on (E0-12) (§2.10) |
| Masternodes, ProTx, governance | out (product scope at the time) | out | in, as wallet-holder flows after the DashPay core (DEC-01, §5a) |
| Error codes | flat `platform.*` | per domain (`registration.*`, `name.*`, `contact.*`, …) | per domain, matching m1's `send.*` / `vault.*`; Fable's list is the cross-check |
| Avatar fetch | HTTPS only | http(s) | HTTPS only, plus Opus's check of hash and dHash against the profile |
| Username length | 3–23 | 23-character UI cap (DPNS allows 63) | 23 in the UI |
| About text | the engine's limit | 140 (contract) | 140, read from the contract through the engine |
| Shell information architecture | keep the shell, add Contacts | mobile IA, title "Dash Wallet" | Opus (R11) |
| CI | not proposed | GitHub Actions on three OSes | enabled (DEC-13) |
| Windows testing | GitHub runners or a clean VM | KVM VM on agentbox | GitHub runners; a KVM VM on agentbox only if they fall short (T-04) |
| Receiving accounts while locked, from a cached `15'/0'` xpub | — | optional | optional, after 1.0 (DP2-10) |
| Registration progress | engine state machine | draft journal + changesets | one row, `dp_registration`, advanced by the engine; progress signals come from the changeset tap |
| A pending invitation link before any wallet exists | 0600 file in the vault directory | vault record | a vault record once a vault exists; before that, a 0600 file in the vault directory, moved into the vault and deleted when the vault is created |

### 2.10 Release timing (decided here)

| Item | Release | Why |
|---|---|---|
| DashPay core (DP1–DP6) | 1.0 | It is the product goal. |
| X1a: read-only Platform-address balance | 1.0 | A wallet restored from an iOS phrase may hold Platform-address funds, and without this the desktop shows them as missing. The sync loop and the balance exist in the library, so the cost is about 3–4 agent-days. |
| X2: shielded pool | 1.1 | Rejected: Fable's "1.0 if its build gate passes" and Opus's "1.0 if ready, cut first", both of which leave the 1.0 scope open until late. **Not in 1.0:** DashPay does not need it (Core funding works, and Android has no shielded pool at all). Its library code is still moving (#5288 open, PV14 nullifier changes). halo2's build time, binary size and prover warm-up are unknown until E0-12 measures them. 1.0's security review should not also take on a zero-knowledge pool. **Not later than 1.1:** iOS steers new users into shielded funding, so same-seed users will hold shielded funds. Until then, 1.0's restore screen and release notes say that shielded balances are not shown yet. |
| X1b: Platform-address transfers, receive, advanced mode | 1.1 | It shares the advanced-mode and Internal Transfer screens with X2. **Not in 1.0:** it is 8–10 more agent-days on a full 1.0. iOS-restored users see their Platform funds (X1a) but cannot move them from the desktop until 1.1, and the notice points them to the iOS app meanwhile. |
| MG: masternodes and governance for wallet holders | 1.1 | DEC-01 puts it after the DashPay core. It brings new signing paths (owner and voting keys, ProTx payloads, collateral) that deserve their own security review. Its engine is mostly salvage, so 1.1 can follow 1.0 within weeks. |
| X3: invitation creation | 1.1 | DEC-15 |
| X4: username marketplace | 1.2 | iOS-only, and the largest iOS DashPay screen; its name states last one session and need a mirror |
| Integrations (Buy & Sell, DashSpend, ZenLedger, Imgur upload) | each ships disabled until DCG supplies credentials | DEC-15 |

---

## 3. Engine design

### 3.1 Modules (`rust/crates/dw-engine/src/platform/`, one file per domain, each under about 800 lines)

```
mod.rs           PlatformRuntime per NetworkSession: subsystem state, loop handles, cadence, lease table
signers.rs       VaultIdentitySigner  : dpp Signer<IdentityPublicKey>   (DIP-13 identity keys, m/9'/c'/5'/0'/0'/i'/k')
                 VaultContactCrypto   : platform_wallet::ContactCryptoProvider (receiving xpub, ECDH, account
                                        reference and unmask, contactInfo seal/open, auto-accept and invitation key
                                        export; each method path-gated; the invitation export is compiled only with
                                        the `invitation-create` feature)
                 VaultScanKey         : ScanKeyResolver (master xprv, only on the branch that scans; zeroized guard)
startup.rs       ordered bring-up, loop start/stop/quiesce, cadence, unlock drain, status snapshot
keys_policy.rs   port of iOS DWDashPayIdentityKeys: keys 0–3 AUTH MASTER, AUTH CRITICAL, AUTH HIGH, TRANSFER
                 CRITICAL; keys 4–5 ECDSA ENCRYPTION and DECRYPTION at MEDIUM, bound to DashPay `contactRequest`
identity.rs      identities read model, main identity (dw-appdb), discovery, balance refresh
registration.rs  quote → persisted state machine (§3.4) → resume, recover, discard; finish_asset_locks
names.rs         username rules (dash-platform-queries), availability, contest precheck, extra names, temporary
                 name, contest watch
contacts.rs      contacts read model, send/accept/ignore/unignore, eligibility, contactInfo, user links, dapk scan
payments.rs      Recipient::Contact for TxDraft, reserve after Confirm, per-contact lock, counterparty, activity
profile.rs       profile read/update; avatar.rs: fetch guard, verify, re-encode, cache, Imgur, Gravatar
invitations.rs   parse, stash, preview, claim (create behind the X3 feature)
credits.rs       top-up, withdraw, cost table, low-credit thresholds
journal.rs       dp_events from classified changesets
trust.rs         SpvQuorumCache + LayeredContextProvider (§2.2)
errors.rs        per-domain error enums with stable codes; mapping from PlatformWalletError and dash_sdk::Error
```

Every operation that has a `rs-platform-wallet-ffi` counterpart cites it in its doc comment (DESIGN-opus §1.3 rule).
`signers.rs`, `registration.rs`, `payments.rs`, `invitations.rs` and `trust.rs` need a second-agent review against that
counterpart. The counterparts are in `PWF/dashpay.rs` and `rs-sdk-ffi/src/mnemonic_resolver_core_signer.rs`; the test
`SeedCryptoProvider` in `PW/wallet/identity/network/contact_requests.rs:171-345` is the template and the vector source.

Existing modules that change:

| Module | Change |
|---|---|
| `engine.rs` | 8 MiB worker and blocking-thread stacks |
| `session.rs` | `build_sdk` options: CA certificate (`SdkBuilder::with_ca_certificate_file`), initial protocol version, explicit DAPI, quorum URL and peers (these exist); `start_spv` runs the bring-up first |
| `store.rs` | changeset classification (§3.5) |
| `send/` | the `Contact` recipient |
| `history.rs` | `counterparty` |
| `events.rs` | a `Platform` signal |
| `dw-vault` | new scopes and the lease table |
| `dw-uri` | user links (they exist) and the DIP-15 `dash:?du=&dapk=` form |

New dependency: `platform-encryption`, at the same platform revision as the rest of the set.

### 3.2 Bring-up and the sync loops

```
open_network ── load_from_persistor
start_spv (returns at once) ── engine task, cancelled by close:
   ├─ if the wallet has identities, a restore is in progress, or discovery is unsettled:
   │     start_wallet_subsystems(wallet, scan_key?, contact_crypto?, identity_signer?)
   │     budget 20 s (the library's DEFAULT_STARTUP_BUDGET); our own 3 s for a wallet created here and never restored
   │     outcome → status snapshot + Platform{Startup} signal
   └─ start SPV, always, whatever the outcome
      └─ loops: identity_sync, dashpay_sync, dpns_sync (contests) [, platform_address_sync with X1a]
close: cancel a running bring-up → quiesce the loops → stop SPV → manager.shutdown → unload wallets
```

- **Signers at startup.**
  - With an unlocked or unencrypted vault (a restore, for example), the providers are built for this call and dropped
    afterwards (`PW/manager/startup.rs:26-36`).
  - With a locked vault they are `None`, and the snapshot says "identity unsettled". The engine runs the sequence again
    at the first unlock.
- **Cadence.**
  - `dashpay_sync` runs every 15 s while a window is visible and every 60 s while hidden or in the tray.
  - It runs at once when Contacts or the bell opens, after an unlock, and after any DashPay write.
  - The contest watch polls every 10 minutes, and every minute in the last hour.
  - Nothing runs while the session is closed; there is no daemon.
- **Unlock drain.** `Unlocked` runs `drain_pending_contact_crypto_verified` (with the seed-binding gate) under the
  background lease, then `reconcile_dashpay_rescan`.
- **Watch-only wallets** are DashPay read-only ("This wallet can't use DashPay because it has no keys").
- **Startup status values.**
  - The library's 7 `WalletStartupStatus` variants: `Ready`, `NoIdentity`, `PartialNoIdentity`, `DiscoveryFailed`,
    `PartialAccountsPending`, `SeedBindingUnverified`, `IdentityScanIncomplete`.
  - Plus our own `NotRun` and `Starting`.

### 3.3 Keys, signer scopes, grants and leases

| Scope (dw-vault) | May derive or sign | Used by |
|---|---|---|
| `PlatformIdentity` | `m/9'/c'/5'/0'/0'/i'/k'` | state transitions: identity create/update, DPNS, documents, contactInfo |
| `DashPayCrypto` | `m/9'/c'/15'/…` (DIP-15 receiving xpubs, DIP-14 256-bit children), `m/9'/c'/16'/…` (auto-accept xpub and export), the BIP44 account-0 xpub (seed binding), ECDH and the `accountReference` mask with the identity keys `m/9'/c'/5'/0'/0'/i'/k'`, and exactly the contactInfo children `65536'`/`65537'` under the identity-auth root; **never signs** (E0-03; see `m1-engine.md` §2.2) | drains, request send and accept |
| `PlatformFunding{max_duffs}` | BIP44, BIP32 and DashPay-receiving inputs, plus asset-lock credit keys `m/9'/c'/5'/{1',2',3'}/…` and the top-up account xpub `m/9'/c'/5'/2'/i'`; capped | registration, top-up (invitation creation in X3) |
| `Spend{max_duffs}` (exists) | as today | contact payments |

The identity scan's master key (`VaultScanKey`, the `ScanKeyResolver` of §3.2) is not a scope: `Vault::scan_key`
releases it only under a redeemed `IdentityScan` grant, which the unattended bring-up authorizes without a prompt in
the prompt-free states. It refuses a `PlatformOp` token, so a capped flow token cannot release the master key (E0-04
design §3.2). It and the auto-accept key are the only keys that leave dw-vault (E0-03 review; `m1-engine.md` §2.2).

**Grants and leases** (E0-04 design §3).
- `PlatformOp` becomes `PlatformOp{max_duffs, max_credits}`.
- `IdentityScan` is a new purpose: wallet-scoped, uncapped, and prompt-free in the prompt-free states only.
- `authorize_set` issues one grant per purpose from one credential check, all with the same wallet binding and TTL.
  So "Accept and pay" gets both of its grants from one prompt (§2.3).
- `QuickUnlock` (Touch ID) may issue `PlatformOp`, capped at the spend limit. A grant's value is
  `max_duffs + ceil(max_credits / 1000)`, and `authorize_set` checks one sum over the whole set (DEC-67).

A flow redeems its grants **once** and turns them into a **flow lease**: a signer set held by the engine, bound to
one wallet and one flow, with a budget per purpose. Lifetimes, parking and the background lease are defined in §2.6.

**Which actions prompt.** Registration, top-up, withdraw, profile edit, contact request, accept, contactInfo, username
registration and contact payment, under "require authentication for every payment" (default on).

**Secrets.** Identity keys are derived and never stored. New vault records:

- `invitation/<id>`, a pending claim link that carries a WIF;
- `integration/imgur/<delete-hash>`, not secret but private.

Nothing secret goes to `settings.json` or SQLite.

### 3.4 Persistence

| Store | What DashPay adds |
|---|---|
| `wallet.sqlite` (`SqlitePersister`, the library's) | Everything the library persists: identities (with DPNS names and profiles), public identity keys, contacts (alias, note, hidden, accepted accounts, `payment_channel_broken`), ignored senders, asset locks and proofs, invitations, DPNS name states, scan state, the DashPay payments overlay. **Nothing written by us.** |
| `app.sqlite` (dw-appdb, append-only migrations; the wallet column is `wallet_id`, as in every dw-appdb table, so wallet removal and `.dwbackup` export cover these) | `dp_main_identity(wallet_id, identity)`; `dp_registration(id, wallet_id, identity_index, identity, label, temp_label, funding, initial_profile, asset_lock_outpoint, phase, error, retryable, created_at, updated_at)` (`initial_profile`: the profile entered at Draft as JSON, kept until `ProfileCreated`; the outpoint is unique per wallet, so one asset lock funds one flow); `dp_contest_watch(wallet_id, identity, label, ends_at, last_state)`; `dp_events(id, wallet_id, identity, kind, contact, ref, at, read_at)` (contact and ref are `''` when absent; unique on all but id, so journal writes are `INSERT OR IGNORE`); `dp_payment_lock(wallet_id, identity, contact, txid, since)`; `dp_trust_unverified(wallet_id, kind, key, since)` (§2.2; keyed per wallet, but wallet.sqlite stores one row per identity, so another wallet's write is a no-op: the money-move gate blocks when ANY wallet flags the entity, and a verified re-fetch of an identity clears its rows in every wallet; contact-request and DPNS rows are per wallet and cleared by their own wallet's re-fetch); `dp_avatar(url_sha, content_sha, dhash, status, file, fetched_at, bytes)` (network-wide cache index; `status` is the fetch and decode outcome of the URL only, never a verdict on a profile's `avatarHash` or fingerprint, which is checked per profile when reading; thumbnail files are named by content, so rows can share one and rotation counts and unlinks a shared file once, when no row names it); `dp_prefs(wallet_id, identity, key, value)` |
| `<network>/dispatch.sqlite` (dw-appdb `dispatch.rs`, mode 0600; `synchronous=FULL`, `secure_delete=ON`; E0-04 design §6) | the dispatch journal: registered asset locks with their recovery payload (Mode A), and the write-ahead step and funding markers (both modes). Only the engine's dispatch fence writes it. It is not part of `.dwbackup` and dw never restores it. A wallet's rows are erased only by a wiping `remove_wallet`, once that wallet tracks no asset-lock row |
| `<network>/avatars/` (mode 0700) | engine-re-encoded PNG thumbnails (128 and 256 px) named by SHA-256, never the original bytes; rotated at 200 MB, oldest first |
| vault | pending invitation links; the Imgur delete hash |
| UI settings | advanced mode, notifications toggle, contact sort, "load contact pictures" |

**The registration state machine** (one `dp_registration` row per flow):

```
Draft{name, contested, funding, temp_name?, initial_profile?}   user may edit or discard
 → KeysPrepared{identity_index, pubkeys}             public keys pre-persisted through platform-wallet
 → FundingSent{txid, outpoint}                       asset lock broadcast; tracked by platform-wallet
 → ProofWaiting{IS|CL, since}                        InstantSend up to 300 s, then a ChainLock with no time limit;
                                                     parks keyless when the lease ends (§2.6)
 → IdentityRegistered{identity_id}
 → NameRequested → NameRegistered | Contested{ends_at, temp_name_state}
 → ProfileCreated (optional)
 → Done | Failed{phase, code, retryable}
```

- The engine advances the row after each step, when a session opens, at unlock, and on "Finish registration".
- Every step is idempotent against platform-wallet's own tracking (tracked asset locks, persisted identities, DPNS
  states), so a kill at any transition is safe. Tests kill at every transition.
- Funds are committed only at `FundingSent`.
- The profile input (`initial_profile`) lives in the row from Draft, so it survives the ChainLock wait, a restart and a
  restore; `ProfileCreated` consumes it.

**Registration rows: what DP1-02 and DP5-02 must honour** (conditions from the E0-07 review; `dp_registration` is
migrated before the engine that writes it, so these are the contract for that engine).

*The `funding` column* is `TEXT NOT NULL` without a CHECK, so its encoding can grow without a rebuild. Nothing in the
schema defines it; DP1-02 does, before the first row ships:

1. **Versioned and tagged**, e.g. `{"v":1,"kind":"core_balance",…}`. These rows travel in `.dwbackup` files between
   app versions and are imported by column name. A newer build parses an older row; an older build refuses a newer
   one cleanly, as a `Failed` row with a code, never a panic.
2. **`CoreBalance`** carries the locked amount in duffs from the accepted quote, so a resume before `FundingSent`
   rebuilds the same lock under the grant's cap instead of re-quoting silently across the cap the user approved.
3. **`ExistingIdentity{id}`**: the id lives in the `identity` column only (set at Draft for this funding kind), never
   in `funding`, so the two cannot disagree after a partial update.
4. **`FaucetAssetLock`** (developer builds): its key is a bearer credential and goes to the vault; `funding` is never a
   secret. The outpoint goes to `asset_lock_outpoint`.
5. **`Invitation{link_id}`**: the link is a bearer credential held in the vault as `invitation/<id>` (§3.3), and a
   `.dwbackup` carries no such record. See "Restore" below.

*`updated_at`* is never used as the start of the InstantSend window (§2.6): a touch would extend how long a key is
kept. The window runs from an in-memory instant taken at broadcast. After a restart the IS-or-CL wait belongs to
platform-wallet (`resume_asset_lock` takes a relative timeout); a persisted "since" would only move when the UI
switches from "waiting for InstantSend" to "waiting for a ChainLock", and can be added later as a nullable column.

*Restore.* `dp_*` wallet rows are part of the wallet's `.dwbackup` (`docs/contracts/dwbackup-v1.md` §2.3, §3). The
manager's decision: **`dp_registration` rows are restored on another machine**, so a registration whose asset lock is
already funded resumes there and the locked funds are not stranded. The scan rebuilds `wallet.sqlite`, and
platform-wallet's restore reconstructs finalized asset locks that pay this wallet as `RecoveredFromChain`; the signing
key is rederived from the seed, not read from the row. A restored row can be behind reality, because automatic backups
are written when a wallet is added and when a session opens, never on a registration transition (for example the
backup says `KeysPrepared` while the source machine has already funded and registered). Therefore DP1-02:

1. never funds a restored row (never advances it past `KeysPrepared`) until SPV has synced and the same-seed discovery
   of identities and the asset-lock reconstruction pass (DP1-05) have run; this extends the §2.2 rule 5 hold on
   registrations until SPV sync;
2. before funding, looks for an identity at `identity_index` and for `RecoveredFromChain` registration locks, and
   adopts what it finds instead of building a new lock;
3. writes `asset_lock_outpoint` when the lock is **built**, before it is broadcast, so a kill or a snapshot after the
   broadcast never shows an unfunded phase for a funded flow (the unique index then also keeps a second flow off the
   same lock; `finish_asset_locks` reuses the stranded lock's row);
4. runs an automatic backup when the flow reaches `FundingSent`, so the newest backup carries the outpoint;
5. writes `asset_lock_outpoint` in one canonical text only (lower-case hex txid, `:`, decimal vout, `OutPoint`'s
   `Display`), and keys any upsert on the lock with the index's predicate:
   `ON CONFLICT(wallet_id, asset_lock_outpoint) WHERE asset_lock_outpoint IS NOT NULL DO ...` (SQLite rejects the
   statement without the `WHERE`).

An invitation-funded row (`Invitation{link_id}`) **fails with a typed error on another machine**, for example
`invitation.invalid` ("open the invitation link again"): the link is in the vault of the source machine only. That is
safe, because the asset lock is the inviter's and none of the user's funds are stranded. The failed row must neither
retry forever nor count towards `registration.in_progress`, so a new registration can start.

**Gaps in the library's persister** (verified at the pin; also present at head and on v5.1-dev):

| Gap | Effect | Handling |
|---|---|---|
| `WALLET_RESTORE` is not attested | `create_invitation` is refused, and so is `mark_asset_lock_consumption_unknown` | Invitation creation is X3 (1.1), after the storage patch; never fake the capability bit. The rare asset-lock case shows "Finish registration" again. #5207, which relaxes the reconciliation requirement, targets v5.1 only. |
| `pending_contact_crypto` is not reloaded | the queue is lost on restart | drain at unlock and in the bring-up; Contacts shows "Unlock to finish setting up N contacts" |
| the payments overlay and token balances are not reloaded | display metadata only | forced DashPay pass after `load()`; covered by the H-03 kill -9 test |
| `dpns_name_states` live for one session only | marketplace departure classification | X4: mirror them in dw-appdb |

### 3.5 Change detection and events

`WalletStore::store(changeset)` classifies each changeset:

| Changeset field touched | Signal: `EngineEvent::Platform{network, wallet_id, change}` | Journal (`dp_events`) |
|---|---|---|
| `identities` | `Identities` | username registered; contest outcome |
| `contacts`, received | `Contacts{identity}` | "@bob sent you a contact request" |
| `contacts`, established | `Contacts{identity}` | "@bob accepted your request" / "is now your contact" |
| alias, note, hidden, `ignored_senders` | `Contacts{identity}` | — |
| `asset_locks` | `Registration{draft}` | — |
| `dashpay_payments_overlay` | `Payments{identity}` | "Received … from @alice", unless it is catch-up |
| `account_registrations` (contact accounts) | `Contacts{identity}` | — |
| `dpns_name_states` | `Names` | — |

While the trusted fallback is in use (§2.2), the tap also writes every entity a changeset touches to
`dp_trust_unverified`.

- Signals are debounced at 4 Hz per domain, and the last change of a burst is always delivered (the m1 rule).
- Hosts re-query; rows are never pushed.
- Journal writes are idempotent per `(kind, contact, txid or request id)`.

### 3.6 Facade API (sketch; implementers finalize it in `docs/contracts/m4-dashpay-engine.md`)

The conventions are m1's: hex wallet ids, base58 identity ids, duffs and credits as `u64`, `Option` for unknown values,
grants by id, one error enum per domain. Records derive `serde` (plus `uniffi` or `specta` in the binding crate only).
Sync calls read in-memory state; async calls touch the network or persistence.

Note for DP1-02: `dp_registration.asset_lock_outpoint` is unique per wallet byte-wise, and the column has no format
CHECK. Format it in exactly one place, as lower-case hex txid, `:`, decimal vout (`OutPoint`'s `Display`); no helper
exists yet, so DP1-02 adds the single writer and a test that `'T:0'`-style upper-case input is normalized or rejected.

```rust
// NetworkSession::dashpay(wallet_id) -> Arc<DashPay>
impl DashPay {
    // status and identity
    pub fn status(&self) -> DashPayStatus;             // banner state: NoIdentity{reason?} | Registering{draft} |
                                                       //   ContestPending{..} | Ready{main} | StartupIncomplete{StartupStatus}
    pub fn sync_status(&self) -> DashPaySyncStatus;    // startup status, last pass, pending crypto, loops, quorum source
    pub async fn sync_now(&self) -> Result<SyncPassReport, PlatformError>;
    pub fn identities(&self) -> Vec<IdentitySummary>;  // id, index, names, main name, balance: Option, has_dashpay_keys, profile
    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError>;
    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError>;   // + public keys
    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError>;
    pub async fn discover_identities(&self, grant: String) -> Result<u32, PlatformError>;

    // registration
    pub async fn registration_quote(&self, req: RegistrationRequest) -> Result<RegistrationQuote, RegistrationError>;
        // req { label, temporary_label?, funding: CoreBalance | Invitation{link_id} | ExistingIdentity{id}
        //       | FaucetAssetLock{..} (developer builds only), initial_profile? }
        //   persisted as dp_registration.funding / .initial_profile; encoding and restore rules: §3.4
    pub async fn start_registration(&self, req: RegistrationRequest, grant: String) -> Result<String, RegistrationError>;
    pub fn registrations(&self) -> Vec<RegistrationStatus>;          // phase, label, identity, txid, error, retryable
    pub async fn resume_registration(&self, draft: String, grant: Option<String>) -> Result<(), RegistrationError>;
    pub async fn discard_registration(&self, draft: String) -> Result<(), RegistrationError>;  // before FundingSent only
    pub async fn finish_asset_locks(&self, grant: String) -> Result<FinishReport, RegistrationError>;

    // names
    pub async fn name_availability(&self, label: String) -> Result<NameAvailability, NameError>;
        // Invalid{rules} | Available{contested} | Taken{owner} | ContestOpen{ends_at, contenders} | Locked | Unknown
    pub async fn register_name(&self, identity: String, label: String, grant: String) -> Result<NameOutcome, NameError>;
    pub async fn contest_status(&self, identity: String, label: String) -> Result<ContestStatus, NameError>;
    pub async fn search_users(&self, prefix: String, limit: u32) -> Result<Vec<UserHit>, NameError>;  // with relation
    pub async fn resolve_user(&self, username: String) -> Result<Option<UserHit>, NameError>;

    // contacts
    pub fn contacts(&self, identity: String, q: ContactQuery) -> ContactsPage;  // sections, sort, text
    pub fn contact(&self, identity: String, contact: String) -> Option<ContactDetail>;
    pub fn pending_setup_count(&self) -> u32;
    pub async fn eligibility(&self, contact: String) -> Result<Eligibility, ContactError>;
        // Ok | NoDashPayKeys | IsSelf | AlreadyContact | PendingOutgoing | PendingIncoming
    pub async fn send_request(&self, identity: String, to: String, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn accept_request(&self, identity: String, from: String, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn set_private_details(&self, identity: String, contact: String, d: PrivateDetails, grant: Option<String>)
        -> Result<PublishState, ContactError>;            // Local | Published | DeferredUntilTwoContacts
    pub async fn enable_dashpay_keys(&self, identity: String, grant: String) -> Result<(), ContactError>;
    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError>;       // dashpay://user?id=&username=
    pub async fn verify_scanned(&self, text: String) -> Result<ScannedContact, ContactError>;  // plain link or dapk

    // payments and activity (the TxDraft recipient lives in send/)
    pub fn payment_lock(&self, identity: String, contact: String) -> Option<PaymentLock>;
    pub async fn resolve_payment_lock(&self, identity: String, contact: String) -> Result<LockResolution, ContactError>;
    pub async fn contact_activity(&self, identity: String, contact: String, cursor: Option<String>, f: ActivityFilter)
        -> Result<ActivityPage, ContactError>;
    pub fn frequent_contacts(&self, identity: String, limit: u32) -> Vec<ContactSummary>;

    // notifications
    pub fn events(&self, identity: String, cursor: Option<u64>, limit: u32) -> EventPage;   // New / Earlier / Pending
    pub fn unread_count(&self, identity: String) -> u32;
    pub async fn mark_read(&self, identity: String, up_to: u64) -> Result<(), PlatformError>;

    // profile and avatars
    pub fn profile(&self, identity: String) -> Option<Profile>;
    pub fn profile_limits(&self) -> ProfileLimits;                       // from the contract: 25 / 140
    pub async fn prepare_avatar(&self, src: AvatarSource) -> Result<AvatarCandidate, AvatarError>;
        // File{bytes, crop?} | Url{url} | Gravatar{email}
    pub fn avatar_upload_available(&self) -> bool;                       // an Imgur client id is configured
    pub async fn upload_avatar(&self, candidate: String) -> Result<String, AvatarError>;
    pub async fn update_profile(&self, identity: String, edit: ProfileEdit, grant: String) -> Result<(), PlatformError>;
    pub async fn avatar(&self, identity: String, size: AvatarSize) -> Result<Option<AvatarImage>, AvatarError>;

    // credits
    pub fn cost_table(&self) -> CostTable;
    pub async fn top_up(&self, identity: String, duffs: u64, grant: String) -> Result<TopUpOutcome, CreditsError>;
    pub async fn withdraw(&self, identity: String, to: String, amount: WithdrawAmount, grant: String)
        -> Result<WithdrawOutcome, CreditsError>;

    // invitations
    pub async fn stash_invitation(&self, link: String) -> Result<String, InvitationError>;
    pub async fn invitation_status(&self, link_id: String) -> Result<InvitationStatus, InvitationError>;
        // Valid{inviter?, funding_duffs, contested_allowed, expires_at?} | Claimed | Invalid{reason} | Expired
}
pub fn check_username(label: &str) -> UsernameCheck;                    // pure: valid, normalized, contested, rules
```

- History and transaction records gain `counterparty: Option<Counterparty{identity, username, display_name, avatar}>`.
- `TxDraft` gains `Recipient::Contact{identity, contact, amount, subtract_fee, note}`.

**Error domains and codes** (stable strings; the UI picks the copy):

| Domain | Codes |
|---|---|
| `platform.*` | `unavailable`, `timeout`, `proof_invalid`, `trust_mismatch`, `context_unavailable`, `signer_unavailable`, `seed_mismatch`, `insufficient_credits{needed, available}`, `grant_invalid`, `grant_exceeded`, `feature_off{feature}`, `not_implemented{call}` |
| `identity.*` | `not_found`, `keys_missing{purpose}` |
| `registration.*` | `in_progress`, `funding_insufficient{needed, available}`, `islock_timeout`, `recoverable{draft}`, `already_has_username` |
| `name.*` | `invalid{rules}`, `taken`, `contest_open`, `locked`, `unavailable_for_invite` |
| `contact.*` | `ineligible`, `already_contact`, `request_pending`, `self`, `channel_broken`, `payment_locked{txid}` |
| `invitation.*` | `invalid`, `claimed`, `expired`, `already_has_identity` |
| `avatar.*` | `too_large`, `unsupported`, `fetch_failed`, `hash_mismatch`, `upload_unconfigured` |

Notices: `PlatformTrustMismatch`, `DashPayStartupIncomplete`, and the existing `PlatformContextUnavailable`.

### 3.7 Offline and failure behaviour

| Situation | Behaviour |
|---|---|
| DAPI down, SPV fine | The Core wallet works fully. **Paying an existing contact still works**: it is a Core transaction to a locally derived address. Saved data shows "as of …". Writes are disabled with the reason. The banner appears on Contacts only. |
| Offline entirely | The Core rules as today, plus DashPay writes disabled |
| Network lost mid-registration | The draft parks at its phase. It resumes on reconnect while the lease lives; otherwise the UI shows "Unlock to finish". Locked funds are never lost. |
| App killed mid-flow | The next launch resumes the draft (kill -9 at every transition is tested in DP1-02 and H-03) |
| Bring-up over budget | SPV starts anyway, with `Notice{DashPayStartupIncomplete}`; `reconcile_dashpay_rescan` heals contact payments found late |
| Before SPV's masternode state has synced (first launch, long offline) | Reads show saved or fallback-verified data, marked unverified. Platform writes wait, with "Waiting for the network to sync" (§2.2). |
| Locked vault | Reads work. Writes ask for an unlock. New contacts finish setting up after the unlock. |
| Ambiguous broadcast to a contact | The contact is locked for sends until a reconcile settles the payment; paying their address directly still works |
| Panic at the binding boundary | becomes `Internal`, never an abort (m1 rule) |

### 3.8 Privacy and security

- **Avatars** are fetched by the engine only, through the app's proxy setting:
  - HTTPS only; 5 MiB; 10 s; resolved IPs checked against private ranges before connecting, with no redirects to
    them;
  - decoded with `image` size limits, then re-encoded to PNG;
  - checked against the profile's `avatarHash` and dHash; a mismatch shows the initials avatar;
  - "Load contact pictures" is on by default and can be turned off;
  - DP4-02 writes thumbnails through descriptors, never by path: `avatars_dir()` was checked once, at session open, and
    a user-chosen symlink may stand in for the directory. Open the directory `O_NOFOLLOW|O_DIRECTORY` (or through
    dw-fs) and create files with `O_CREAT|O_EXCL|O_NOFOLLOW`, mode 0600. A symlink target keeps its own mode.
- **Gravatar** e-mail addresses are hashed in Rust and never stored. **Imgur** uploads are anonymous, and the delete
  hash is kept.
- **Bearer credentials** (invitation links, `dapk` QR payloads, the faucet asset-lock key):
  - never logged; the tracing layer redacts `dashpay://invite` and `dapk=` (a test checks this);
  - the UI says "anyone with this link can claim it".
- **Seed binding.** Every drain goes through `drain_pending_contact_crypto_verified`.
- **Search** sends only the prefix to DAPI, never the wallet's own id.
- **URL schemes.** `dashpay://` (`invite`, `user`) joins `dash:`, `pay:` and `dashwallet:`, registered per OS by the
  packaging tasks. A `dashpay://user` link opens Add Contact prefilled.
- **Webview hardening** applies under Tauri:
  - no remote content, a strict CSP and the capability allowlist;
  - no raw HTML from engine strings;
  - secrets and signing stay in Rust; avatars reach the UI as engine-decoded PNGs.

### 3.9 Pin policy and upstream work

**E0-01** bumps every platform crate to one v5.0-dev head revision (≥ `bc41f1bc23`, DEC-15), together with rust-dashcore
`40268cc0` and grovedb `9791d277`, exactly as platform's lock has them. The reasons:

- #5294 refuses GroveDB V0 proof envelopes; at the pin a PV13 client still accepts them, and both live networks run PV13;
- #5305 is a grovedb proof-soundness fix;
- #4978 keeps the chosen DPNS name across sync;
- #5206 makes "insufficient credits" a typed error.

**After that, one bump per milestone** (the "pin train"), never a moving pin. The train picks up what lands on
v5.0-dev, which today means #5026.

**The v5.1 move.** The DashPay restore fixes target **v5.1-dev only**: #5256 (one-way contacts) and the restore stack
#5150 → #5207/#5220 → #5210, on #5307's rust-dashcore update. So the train moves to v5.1-dev at the first milestone
bump where both of these hold:

- v5.1-dev contains v5.0-dev's fixes (#4978, #5294, #5305);
- #5256 and the restore stack have merged there.

The pin also moves when iOS moves, if that comes first. The move brings #4623 and #4997, and the carried branch
(E0-11) is retired.

**Our own patches.**

- Patches to platform go on branches of pasta's public fork `PastaPastaPasta/platform`, once the manager has recorded
  that decision: #4623 + #4997 now, the storage loaders and `WALLET_RESTORE` attestation for X3 later. The whole platform crate set is pinned to such a branch,
  never mixed with upstream.
- Upstream PRs are opened only if pasta asks.
- rust-dashcore fixes (E0-10c) stay local until pasta allows publishing them (DEC-09). They reach the desktop through
  platform's pin.

### 3.10 A local full node as a backend (kept open, not depended on)

A user may run a full node (dashd, or a Rust node) next to the wallet. The design leaves room for that in four places:

1. **The node as the SPV peer.** SPV peers are already configurable (`SessionOptions.spv_peers`, `--connect`). A local
   node can be the only peer, which gains privacy and reliability with no code change.
2. **The node as a quorum source.** `trust.rs` takes its quorum keys through a small `QuorumSource` interface (SPV,
   trusted HTTPS). A local full node can be added as a third source.
3. **The node's indexes.** Features that use Insight today (sweep, phrase repair) can use a local index later.
4. **No SPV-only assumptions.** No engine API assumes facts that only SPV provides beyond what m1 already marks
   "requires full-node data source".

No roadmap task depends on a local node.

---

## 4. App layer (stack-neutral)

Each view model is a state holder over the facade. It re-queries when a `Platform` signal arrives, models unknown
values as unknown, and has its flows as exhaustive enums or discriminated unions. View-model tests are named after
parity IDs, so `parity.md` keeps its references.

| View model | Owns | Calls |
|---|---|---|
| `DashPayStatusModel` (app-wide) | identity chip, Home card, sidebar badge, bell count | `status`, `identities`, `unread_count` |
| `JoinDashPayViewModel` | intro, readiness, FAQ, voting info | `status`, `registration_quote` |
| `RegistrationViewModel` | name → contested confirmation → funding → review → progress → done; resume | `check_username`, `name_availability` (debounced 400 ms, latest wins), `registration_quote`, `start/resume_registration`, `registrations` |
| `UsernameRequestStatusViewModel` | tallies, deadline, temporary name | `contest_status`, `register_name` |
| `MyProfileViewModel` | the profile sheet | `identity_detail`, `refresh_balance`, `profile` |
| `EditProfileViewModel`, `AvatarPickerViewModel` | the form, crop, unsaved-changes guard | `profile_limits`, `prepare_avatar`, `upload_avatar`, `update_profile` |
| `CreditsViewModel` | top-up presets 0.05 / 0.1 / custom ≥ 0.01, "≈ N contact requests", withdraw | `cost_table`, `top_up`, `withdraw` |
| `ContactsViewModel` | groups, sort (Android's four orders), local and network search, selection | `contacts`, `search_users`, `sync_now`, `pending_setup_count` |
| `AddContactViewModel` | search, My QR, scan from a file, drop, clipboard or screen region | `search_users`, `my_user_link`, `verify_scanned`, `eligibility`, `send_request` |
| `ContactDetailViewModel` | relation, actions, activity (All / Sent / Received), private details | `contact`, `accept/ignore/unignore`, `contact_activity`, `set_private_details`, `enable_dashpay_keys` |
| `NotificationsViewModel` | New / Earlier / Pending, inline accept | `events`, `mark_read`, `accept_request` |
| `SendViewModel` (extended) | "To a contact" tab, frequent strip, "Accept and pay", lock banner | `TxDraft` + `Recipient::Contact`, `frequent_contacts`, `payment_lock` |
| `InvitationClaimViewModel` | paste or link, validation envelope, continue to registration | `stash_invitation`, `invitation_status` |
| `IdentitiesViewModel` (advanced), `DashPaySyncInfoViewModel` (developer) | advanced and diagnostic pages | `identities`, `identity_detail`, `sync_status`, `sync_now` |

**Under Tauri:**

- server state lives in TanStack Query, with query keys that mirror the facade;
- `PlatformChange` signals become `invalidateQueries`;
- flow state lives in small typed stores;
- amount formatting is ported and checked against the existing `testdata/` golden vectors.

**Under SwiftCrossUI:** the same models live in `WalletFeatures/DashPay/*` (`@MainActor @Observable`) over
`WalletRuntime` protocols, with DashPay fixtures in `WalletDemo`.

**Information architecture (R11).**

- The sidebar holds Home, Pay (To a contact / To an address), Receive, Contacts (with a badge for pending requests;
  Join DashPay while there is no identity), Activity, CoinJoin and Explore (the last two when enabled), and from 1.1
  Masternodes (§5a).
- The toolbar holds the identity chip (avatar and @username, which opens My Profile), the bell, the wallet picker, the
  discreet-mode toggle and lock.
- The window title is "Dash Wallet", with " — Testnet" (or another network name) off mainnet.
- The dash-qt tools (Console, Peers, PSBT, Coin Selection, Sign/Verify, the address book, Repair) stay available
  under Tools, the menus and Settings ▸ Advanced.
- The registration wizard can be closed while it runs; Home and the chip then show its progress.

---

## 5. Feature catalogue and release

Engine support at the pin: **present** means the library has it and we write glue. **head** needs the E0-01 bump.
**carried** needs the carried branch. **upstream** needs a patch that does not exist yet. **ours** means there is no
library support and we build it.

| # | Feature (parity) | Mobile reference | Desktop | Engine support | Release |
|---|---|---|---|---|---|
| F1 | Join DashPay entry and intro (IOS-065, 129) | iOS: banner with about 10 states, intro, FAQ. Android: Home row, More card, minimum-balance note, "mix first" tip | one banner state machine for the Home card, the chip and the Contacts empty state; disabled until sync completes; real costs from the quote; CoinJoin privacy tip | present + ours (quote) | 1.0 |
| F2 | Registration funded from Core, resumable (IOS-067, 033, 139) | iOS coordinator with pre-persisted keys and IS→CL fallback; Android foreground service with retry and reuse | wizard → progress that can be closed; persisted state machine; "Funds locked — finishing" rows; Tools ▸ Repair "Finish transfers" | present; ours: key policy, state machine | 1.0 |
| F3 | Other funding: invitation, Platform addresses, shielded, faucet asset lock (developer builds) | iOS: all of them, shielded steered first. Android: Core and invitation | 1.0: Core and invitation; X1b: addresses; X2: shielded | present (shielded behind its feature) | 1.0 / 1.1 |
| F4 | Choosing a username (IOS-066, 134) | 3–23 characters `[A-Za-z0-9-]`, no edge hyphen, 0.4 s debounce, contested detection, precheck | inline rule checklist, live availability, contested chip and explainer | present (`dash-platform-queries`) | 1.0 |
| F5 | Own contest status, temporary name (IOS-068, 135) | iOS status screen; Android "instant" second name; Android identity-verify link | iOS model; contest watch with OS notification on the outcome; the temporary name is the main name until the contest resolves; **no** verify link (no contract at the pin) | present + head (#4978) + ours (main name) | 1.0 |
| F6 | Same-seed restore of DashPay (IOS-124) | iOS brings DashPay up before SPV; Android restore worker | bring-up during restore; "Looking for your DashPay identity…" (≤ 20 s) | present | 1.0 |
| F7 | Profile sheet, several names (IOS-069, 071, 126) | iOS profile sheet and Identities screen | chip → sheet; Identities page in Settings ▸ Advanced (masternode identities belong to MG, §5a) | present + ours (main identity) | 1.0 |
| F8 | Credits: balance, top-up, withdraw (IOS-069, 131, 070, 132; AND-078) | iOS presets and fee reserve; Android "Buy credits" with action estimate and low-credit warnings | top-up sheet with both; inline low-credit warnings; withdraw in DP6 | present + head (#5206) | 1.0 |
| F9 | Enable DashPay keys (IOS-072) | iOS banner with the fee | banner in Contacts, one confirmation | present | 1.0 |
| F10 | Contacts and requests (IOS-072; AND-090) | iOS sections with inline Accept, hidden; Android sort and no hidden | split view: Requests (n) with inline Accept/Ignore, contacts with a sort menu, Pending and Hidden; keyboard navigation | present + ours (read model) | 1.0 |
| F11 | Find and add a contact (IOS-073, 127, 137; AND-091) | iOS search, My QR, scan with verification, eligibility pre-check | Add Contact: search, My QR (plain link), Scan (file, drop, clipboard, screen region; webcam later); links verified against Platform | present + ours | 1.0 |
| F12 | Contact profile: accept/ignore, activity, alias/note/hide (IOS-074, 128; AND-092, 093) | iOS private details published as an encrypted `contactInfo` once there are ≥ 2 contacts; Android activity filter | right pane: actions, activity with Android's filter, private details "Only visible to you"; unignore from Hidden | present | 1.0 |
| F13 | DIP-15 channels (mechanics) | both apps | channel states shown; locked-vault drain at unlock; optional cached-xpub receiving accounts (DP2-10) | present | 1.0 |
| F14 | Pay a contact (IOS-050, 133; AND-055, 048) | contacts only; Android auto-accepts | Pay ▸ To a contact; frequent strip; explicit "Accept and pay"; CoinJoin source allowed; §2.3 flow | present + **carried** (#4623) | 1.0 |
| F15 | Attribution in history, username in requests (IOS-136, 029, 055; AND-031, 058, 059) | avatar and name on rows; username in request QRs | `counterparty` on rows and details; title rule label → contact → metadata → type; request URIs carry the username | ours (simple: account types carry both ids) | 1.0 |
| F16 | Notifications (IOS-075, 116; AND-094) | bell, New / Earlier / Pending, inline accept; iOS local notifications | bell popover and page; OS notifications through the existing notifier; catch-up silence; Dock and tray badges | ours (journal) | 1.0 |
| F17 | Profile and avatar (IOS-076; AND-095…097) | display name ≤ 25, about ≤ 140; camera, gallery, URL, Gravatar, Imgur | file, drop, clipboard, URL, Gravatar; crop; Imgur only when a key exists (blocker B1); no Google Drive | present + ours (pipeline) | 1.0 |
| F18 | Claim an invitation (IOS-077; AND-007, 025, 089) | iOS paste, scan, link, inviter preview, pre-wallet replay; Android links only, automatic request to the inviter | OS scheme handler, "Paste invitation link" (welcome and Join), vault stash, preview, invite-limited names, preselected request to the inviter | present + carried (#4997 idempotent claim; without it, check the prospective identity id) | 1.0 |
| F19 | Create invitations (IOS-078; AND-084…088) | Android only | X3, Android's flow | **ours on the fork** (storage patch, §3.9) | 1.1 (DEC-15) |
| F20 | Username marketplace (IOS-084) | iOS only | X4, a table page under Explore | present + head; ours (name-state mirror) | 1.2 |
| F21 | Masternode voting on contested names (IOS-079; AND-079…083) | iOS voting with masternode voting keys | MG-07, with the wallet's masternode voting keys (§5a); DashPay itself shows the user's own contest state | present (SDK contested-resource votes) | 1.1, optional |
| F22 | Transaction metadata on Platform (AND-036, 044…047) | Android only | local notes exist; Platform sync post-1.0 | upstream or ours | post-1.0 |
| F23 | DashPay sync info (IOS-114, 139) | iOS Sync Info | DashPay card in Tools ▸ Information: loops, last pass, pending crypto, scan verdict, quorum source, "Sync now" | present | 1.0 |
| F24 | DashConnect and tokens (IOS-085, 086) | test networks only | out of 1.0; revisit with Connect v2 | present | post-1.0 |
| F25 | Platform (DIP-17) addresses, advanced mode, internal transfer (IOS-045, 047, 053, 059…064) | iOS only | X1a: read-only balance and a notice; X1b: transfers, receive, advanced mode | present | X1a 1.0; X1b 1.1 (§2.10) |
| F26 | Shielded pool (IOS-059…062) | iOS only (default on Home) | X2; never the default registration funding; 1.0 says shielded balances are not shown yet | present behind the feature | 1.1 (§2.10) |

Also out: the Android identity-verify link (no contract), Google Drive avatars, iOS's `MOCK_DASHPAY` credits screen,
`dashwallet://request=address` callbacks and `dashid:`.

---

## 5a. Masternodes and governance for wallet holders (DEC-01, milestone MG, release 1.1)

pasta's 2026-10-08 brief asks for voting, masternode creation and shared masternodes with a mobile-inspired UX. DEC-01
brings the **owner-side** flows into the wallet, after the DashPay core. The **operator and server** side stays with
the node and its CLI. `CLAUDE.md` "Product scope" changes accordingly (task S-01).

| In the desktop (owner-side) | Stays with the node and its CLI (operator, server) |
|---|---|
| **Governance**: proposal list and detail (SPV govsync, already measured equal to Core on mainnet, G7); voting yes/no/abstain with the wallet's masternode voting keys, one or many masternodes at once; creating a proposal (collateral transaction, confirmations, submit, resume); the governance clock, superblock dates and budget | running a masternode or evonode; node administration, debug and peer tools beyond the existing console |
| **My masternodes**: masternodes whose collateral, owner, voting or payout key is in this wallet; tracking any masternode and attaching its keys (IOS-082); status from the SPV masternode list; payouts attributed in history; evonode status and claimable credits (IOS-080); evonode Platform credit withdrawal to the payout address (IOS-081, the library's `masternode_withdraw`) | **operator-key-signed transactions**: ProUpServTx (service updates, including "Unban" after PoSe, IOS-081/082) and ProUpRevTx (revocation), QT-125's Update Service and Revoke |
| **Registration wizard**: ProRegTx for regular masternodes and evonodes, with collateral funded new, from an existing UTXO, or held externally and proven with a signed message. The operator's public key, service address and (for evonodes) Platform node details are entered as the operator gives them; a self-operator may fill the operator key from this wallet's masternode keychain (IOS-083, already on `main`). The wallet **never signs with an operator key**, and it generates no operator secret outside the existing keychain reveal flow. | generating or holding an operator BLS secret for a node the wallet does not derive; Platform node operation |
| **ProUpRegTx**: the owner changes the voting, payout or operator key | |
| **v24 shared masternodes**: creation (2–8 shares), reward-address change, key rotation, dissolution, standby (QT-126/127) | |
| Optional (MG-07): voting on contested DPNS names with the wallet's masternode voting keys (IOS-079) | |

**What the parked branches really give us.**

- **Governance (`m3/r2-governance`): finished and tested.** Every M3 governance call works, and the G7 measurement
  passed on mainnet.
- **ProTx (`m3/r3-protx`): partly done.** Implemented and checked against dashd v24.0.0-rc.2:
  - the masternode list;
  - `prepare_registration` with `FundNew` and `ExistingUtxo`, regular and evonode, version-2 payloads;
  - update-registrar;
  - the keychain and tracked keys;
  - update-service and revoke, which we drop.
- **ProTx still `NotImplemented`:**
  - external collateral;
  - every shared-masternode session call, reward update and dissolution (only the payload codecs exist);
  - the version-3 ProRegTx with extended network info;
  - the evonode Platform calls.

  **v24 shared masternodes are therefore mostly new work** (MG-04b).
- **The salvage is larger than the branch diffs.** Main's removal commit `aab64fc` also took out code that sits at the
  branches' merge base, not in their diffs: dw-p2p's governance wire commands, `PeerPicker::full_nodes`, the
  Governance and Masternodes events, the error mapping and base files such as `dw-governance/src/params.rs`. So MG-01
  first reverts `aab64fc` in part, then applies both branches. The ProTx branch conflicts with the keychain port on
  `main` (`374c883`, `dw-vault/src/vault/masternode.rs`).

**The salvage, step by step.**

1. Partly revert `aab64fc`, then apply `dw-governance`, `dw-protx` and the vault's masternode keys onto `main` after
   E0-01.
   - Leave out the operator-signed builders (`prepare_update_service`, `prepare_revoke`) and their `dwcli`/console
     commands.
   - The ProTx suite drives those legs through dashd RPC instead.
   - Keep the crates' unit and oracle tests (object hashes and collateral scripts equal dashd's).
2. Re-home the engine modules behind binding-agnostic facades `Governance` and `Masternodes` in dw-engine, the same
   pattern as `DashPay` (§2.4). Drop the branches' M3 FFI and Swift adapters, unless SwiftCrossUI is chosen.
3. Prefer the library wherever it covers an owner-side flow (R1): tracked masternodes (`TRACKED_MASTERNODES`,
   `PW/masternode/tracked.rs`) and `masternode_withdraw`. Never take its operator-signed paths, such as
   `masternode/update_service.rs`.
4. Bring back `dwcli gov …` and `mn …` (owner commands only) and the console commands. Re-run the suites and the G7
   measurement on the bumped pin.

**Authorization.**

- New signer scopes: `MasternodeVoting` (signs governance and contested-name votes only) and `MasternodeOwner` (owner
  key payloads and collateral proofs).
- The grant purposes `Governance` and `MasternodeOp{max_duffs}` come back; M3 removed them.
- `max_duffs` caps what actually leaves the wallet: the net debit plus fees. A `FundNew` collateral stays in the
  wallet, so it does not count against the cap.

**UX.**

- A sidebar item "Masternodes" appears when the wallet holds masternode keys or a collateral, or in advanced mode. It
  has two tabs, My masternodes and Governance.
- Voting is a sheet listing the user's eligible masternodes.
- The registration wizard runs collateral → operator details → keys → review → submit, with plain-language help for
  each operator field.
- The look follows the DashPay screens (mobile-inspired). UX-SPEC §4.22–4.24, which specify dash-qt-style tables for
  these screens, are replaced by MG-00.

**Parity rows.**

| Rows | Goes to |
|---|---|
| QT-118…123, QT-128…134, IOS-079, IOS-080 | MG |
| QT-124 | MG, as the keychain reveal gate only |
| QT-125 | Update Registrar → MG; Update Service and Revoke → node/CLI, out of scope here |
| QT-126/127 | MG-04b |
| IOS-081 | evonode status and credit withdrawal → MG; Unban → node/CLI |
| IOS-082 | tracking and attaching keys, tracked withdraw → MG; tracked unban → node/CLI |
| IOS-083 | stays as it is (partial, on `main`) |

**Testing.**

- T1 regtest is the main tier: the harness already runs masternodes and quorums with dashd v24, which shared
  masternodes need.
- T2 covers evonode registration and credit withdrawal against Platform.
- Testnet would need 1,000 tDASH of collateral, which the faucet cannot supply, so the public network is used only
  read-only (G7-style).
- The security review is MG-12.

---

## 6. Testing and environments

| Tier | What | Where | Gating |
|---|---|---|---|
| T0 | Rust unit tests; view-model tests; UI tests on the fixture backend with light and dark screenshots | every PR, on agentbox and GitHub Actions (DEC-13) | yes |
| T1 | dashd v24 regtest harness (`regtest/`): the Core half of DashPay (asset-lock transaction type 8, IS/CL waits, funding pools), the existing L1, L2, restore and CoinJoin suites, and the MG suites (governance, ProTx) | agentbox, per PR for engine changes, plus nightly (also in CI) | yes for engine PRs |
| T2 | dashmate `local` devnet on agentbox (T-01): two `dwcli` wallets, the kill -9 matrix, contests (short period or harness votes), invitations created by a test-only patched `dwcli`, evonode registration, PV14 rehearsal | agentbox, per DashPay PR once T-01 is green, plus nightly | yes, once T-01 is green |
| T3 | testnet with `faucet.thepasta.org` (skill `dash-faucet`) | two persistent funded test wallets (seeds in agentbox's secret store); top-ups through the skill's helper within its rate limit, never around CAP; nightly | alerting, not gating |
| T4 | mainnet canary: a non-contested name, a contact round trip with an iOS user, payments both ways, one faucet invitation claim | manual, with a budget from pasta (blocker B3) | release gate |
| GUI | Linux: Playwright and tauri-driver (or AT-SPI under Xvfb for CrossUI). Windows and macOS: GitHub Actions runners (DEC-13). A real Mac only for Touch ID and the notarized first launch (blocker B4). | Linux per UI PR; Windows and macOS per UI PR in CI | yes |

- Testnet contests take about two weeks, so testnet asserts contest *state* only; outcomes are tested on T2.
- The `asset-lock-proof` endpoint funds identities with no L1 balance in developer builds.
- Only the mainnet canary spends real money.
- Nothing runs on pasta's Mac without a planned session; `CLAUDE.md`'s rules still apply there.

---

## 7. The HANDOFF questions — final answers

| # | Question | Answer | Settled by |
|---|---|---|---|
| 1 | Invitations | Claim in 1.0 (F18). Create in 1.1 (X3), after a storage patch on our fork: loaders for invitations, pending contact crypto, token balances, the overlay and DPNS states, then attest `WALLET_RESTORE`. About 3–5 days, done after the v5.1 move brings the restore stack (§3.9). Never fake the bit. | DEC-15 |
| 2 | Pin timing | Bump to v5.0-dev head now (E0-01), then one bump per milestone. Move to v5.1-dev when it carries the restore fixes, or when iOS moves (§3.9). Carry #4623 + #4997 until then (§2.3). | DEC-15; §3.9 |
| 3 | Shielded | 1.1 (X2), with the feature build kept green from E0-12 on (§2.10). | this design |
| 4 | Desktop credentials | DCG owns them; each integration ships disabled until its credentials exist. Imgur is the only DashPay one, and URL and Gravatar avatars work without it. | DEC-15; blocker B1 |
| 5 | Explore database | No Firebase auth: plain HTTPS plus a Rust AES-zip reader (MP-09) | DEC-15 |
| 6 | Local devnet | dashmate on agentbox (T-01); the v5 images exist (verified) | DEC-15 |
| 7 | Trust model | SPV-verified for 1.0; a degraded, disclosed mode if spike E0-10a fails (§2.2) | this design |
| 8 | DashPay QR | Emit the plain `dashpay://user?id=&username=` link. Scan both it and `dash:?du=&dapk=` (sent with `send_contact_request_from_qr`). Generate auto-accept QRs only once a mobile scanner understands them, as an opt-in with a 1 h expiry. | DEC-15 |

---

## 8. Amendments to `DESIGN.md` and `CLAUDE.md`

| Where | Was | Now |
|---|---|---|
| DESIGN.md R1 | "UI: SwiftUI on macOS; SwiftCrossUI on Linux and Windows" | One UI codebase; the stack per G-UI (§2.1); the manager decides (DEC-12) |
| DESIGN.md R2 | "Binding: UniFFI" | The engine facade comes first; UniFFI remains only if SwiftCrossUI is chosen |
| DESIGN.md R2 | "Platform proof trust: trusted provider in M4; SPV in M6" | §2.2: SPV-verified for 1.0 |
| DESIGN.md R1 scope | "invitation creation" | 1.1 (X3) |
| DESIGN.md R1 scope + `CLAUDE.md` "Product scope" | R1 listed governance list/vote/create/resume and "full ProTx incl. v24 shared MN"; the 2026-10-07 `CLAUDE.md` scope then left all of it to Dash Core, and M3 removed it from `main` | Owner-side only (§5a, DEC-01): governance, my masternodes, registration, ProUpRegTx, shared masternodes, evonode credit withdrawal. Operator-signed transactions (ProUpServTx, ProUpRevTx) and server work stay with the node and its CLI. `CLAUDE.md` is updated by S-01. |
| UX-SPEC §4.22–4.24 | dash-qt-style masternode and governance tables | mobile-inspired screens specified by MG-00 |
| DESIGN.md R3 | Mac paths and `swift build` rules | agentbox layout (D1) and GitHub Actions (DEC-13); the Mac rules apply only in pasta's sessions |
| DESIGN-opus WS-07 / M4 "Platform parity" | one M4 milestone | the DashPay milestones in `ROADMAP.md` |
| QT-011 | window title "Dash Core - …" | "Dash Wallet" (R11) |

---

## 9. Risks

| Risk | Likelihood · impact | Mitigation |
|---|---|---|
| The UI port takes longer than planned | medium · high | Engine DashPay work does not depend on it; the gate is measured; DashPay surfaces need only U-01 and U-03 |
| platform-wallet DashPay churn, and fixes that land on v5.1-dev only (#5256, the restore stack) | high · high | pin train; the v5.1-move rule (§3.9); DP2-06 and DP3-05 test exactly those failures; the carried branch has a tracking issue |
| dash-spv lacks Platform quorums | medium · medium | spike in wave 2, run against rust-dashcore `dev` too; the degraded mode keeps 1.0 on schedule (§2.2) |
| Asset-lock funds stuck, or a ChainLock wait without a time limit | medium · high | the state machine parks keyless; library resume and recovery; kill -9 at every transition; "Finish transfers" |
| DashPay restore gaps | medium · high | engine-owned order, unlock drain, `reconcile_dashpay_rescan`, the restore suites |
| DAPI flakiness, testnet resets, PV14 activation | high · medium | healthy-node pinning (H-10), "as of" UI, T3 alarms, pin train |
| Contested names take about 2 weeks on public networks | certain · low | T2 with a short period or harness votes |
| Webview XSS through DashPay profile text | low · high | U6 criteria, CSP, no raw HTML, H-01 review |
| Large wallets (BIP158 loses selectivity past about 100K watched scripts) | medium · medium | about 20 scripts per contact; H-06 measures 500 contacts on a large wallet |
| The MG salvage is bigger than it looks: code removed by `aab64fc`, conflicts with the keychain port, and shared masternodes are only codecs | high · medium | MG-01 reverts `aab64fc` in part before applying the branches; MG-04b is sized as new work; library code is preferred where it overlaps (§5a) |
| Hosted macOS and Windows runners limit GUI automation (screen capture, UI Automation sessions) | medium · medium | the in-app `--selftest` box report; T-04 VM fallback for Windows; blocker B4 for Touch ID |
| Credentials and signing identities (DCG) | high · medium | not on the DashPay path; integrations ship disabled; unsigned test builds until the identities arrive (B1, B2) |
| Faucet limits slow T3 | medium · low | persistent wallets; top up only below a floor |
