# Release plan: 0.1 (Tue 2026-11-03) to 1.0

> **Superseded in part by DEC-1058 (2026-11-01): no PGP until 1.0.** Every GPG, `SHA256SUMS.asc`, fingerprint and
> temporary-key instruction below is replaced by GitHub build attestations (`actions/attest-build-provenance`,
> checked with `gh attestation verify`) and continuous delivery from `main`: see
> [`docs/release/verifying.md`](../../release/verifying.md) and `.github/workflows/release.yml`. "Signed checksums" in
> the never-cut list reads "attested artifacts". The struck-through text is kept for history.

Owner's direction, 2026-10-31 (pasta). Decisions below are made on his behalf; the manager may adjust details with
evidence but not the intent. Gap evidence: [REPORT.md](REPORT.md), [ios.md](ios.md), [android.md](android.md),
[core.md](core.md).

**Intent.** Ship small, usable, attested releases on GitHub early and often, starting with 0.1 this
Tuesday. 0.1 is what people think the app *is* (a DashPay wallet with usernames, contacts and shielded funds), not
the long tail. 1.0 means full user-story parity with the mobile wallets, interactive QA done on all three OSes,
and the important Dash Core stories (masternode registration, shared masternodes, governance, proposal creation,
mixing) complete.

## 0. What changes in the current plan

1. **Stop the 1.1-first ordering.** Pause MG-13 / remaining MG-04b UI work after its current step until 0.1 ships.
   Masternode owner UI comes back in 0.3.
2. **Shielded and Platform addresses are core, not extras.** X1a is no longer cuttable. X1b and X2 are split per
   route (below). The DASHPAY:610 rationale ("Android has no shielded pool") is stale: Android 12.0.0
   (`feat/kotlin-sdk-phase-1`) seeds shielded ON in every build; iOS shows Shielded on Home unconditionally.
3. **The pinned library already has the operations.** `rs-platform-wallet` at `dc0c946307`:
   `wallet/shielded/{fund_from_asset_lock.rs (Core→Shielded), operations.rs: shield (Platform addr→Shielded),
   unshield (→Platform addr), transfer (Shielded→Shielded), withdraw (Shielded→Core), identity_top_up_from_pool,
   identity_create_from_shielded_pool}`, `coordinator.rs` sync/balance/restore, `keys.rs` address_at / viewing keys;
   `wallet/platform_addresses/{sync_balances, next_unused_receive_address, transfer, withdraw,
   fund_from_asset_lock}`. 0.1 is engine wiring + UI, not protocol work.
4. **Privacy story.** Keep CoinJoin mixing on desktop (a Dash Core story pasta wants). Both mobile apps dropped
   mixing (iOS `b89826c80`, Android 12.0.0 `83ce13ca1`) for the shielded pool, so the desktop must also own the
   bridge: "move mixed coins to Shielded" ships with shielded spending (0.2).
5. **Versions replace W/H waves as the delivery unit.** ROADMAP.md gets a "Releases" section mapping task IDs to
   versions; parity.md is re-baselined on the Tauri app in 0.2.

## 1. Version 0.1 — "usable DashPay desktop wallet" — GitHub release Tue 2026-11-03

Minimally tested, functional. Pre-release label, no security review, says so on first run.

### In scope

| Area | 0.1 contents | State today |
|---|---|---|
| Wallet | create, restore (12/24 words), verify phrase, passphrase lock, auto-lock, multiple wallets | built (U-02, U-04) |
| Network | **in-app network choice** (mainnet / testnet) at onboarding and in Settings (restart OK); **SPV sync on by default** (today it needs `--sync`); mainnet default | small new work |
| Home | balance hero = everything the user can spend (Core incl. mixed + Shielded + Platform addresses); breakdown rows Core / Shielded / Platform (iOS model; Platform row always shown on desktop); sync status | new: breakdown (MP-02) |
| Core send/receive | address + QR + `dash:` URI, Max, InstantSend, history, tx detail, coin control, PSBT, sign/verify (Tools) | built (U-03, U-05) |
| History export | "Export CSV" button on Activity (engine `exportHistoryCsv` exists, no caller) | tiny |
| **Shielded** | shielded sync + balance; shielded receive address + QR; **Core → Shielded** (fund from asset lock); **send Shielded → external shielded address**; **Shielded → Core** (withdraw); shielded rows in Activity; prover warm-up at app start or at confirm. Gate every shielded action on the network's detected Platform protocol version — if the network (e.g. mainnet) does not support the pool yet, show "Shielded is not active on this network yet" instead of failing | new (X2 core slice) |
| **Platform addresses** | sync loop + balance (X1a); Activity rows for incoming/outgoing; **"Move to Dash wallet" (withdraw to Core)** so no restored funds are stranded. No Platform receive address in 0.1 (avoid users parking funds where 0.1 can't spend them) | new (X1a + withdraw) |
| Usernames | register (non-contested and contested), status, main name, profile + avatar, top-up, withdraw credits, identities page | built (DP1, DP4, DP6). **Fix: contested cost follows protocol version** (0.15 DASH from PV14; `registration_flow.rs:79`, `invitation_check.rs:90`) |
| Contacts / DashPay | search, send/accept requests, contacts list, pay a contact, contact history, in-app notifications | built (DP2, DP3) |
| Invitations | claim (link) | built (DP5) |
| CoinJoin | mixing start/stop, settings, progress (as built, labelled experimental) | built (U-06) |
| Masternodes | Governance + Username voting tabs as built, **hidden behind Settings ▸ "Show Masternodes" (off by default)** | built (MG-09, MG-11) |

### Out of 0.1 (decided)

Username marketplace / extra names; Dash DEX, Buy & Sell, Explore, DashSpend, CrowdNode; DashConnect; invitation
creation; tx metadata; tax categories; private-key sweep; phrase repair; localization (English only); fiat (stretch
only: Home hero fiat if the rates service lands without risk); MN registration / shared-MN UI; shielded-funded
username registration, top-up from Shielded/Platform, Platform↔Shielded, external Platform sends, Max drains,
mixed→Shielded (all 0.2); auto-update; notarization; Windows code signing; hardware wallets.

### Release mechanics (decided)

- **Version** 0.1.0 everywhere (Tauri conf, Cargo, About). Tag `v0.1.0` on `main`.
- **Repo hygiene:** set the GitHub default branch to `main` (it is `m3/r2-governance`); rewrite README for the Tauri
  app (install, unsigned-binary instructions, verification, scope, "pre-release, small amounts only").
- **Artifacts, built by GitHub Actions on the tag** (PK-01/PK-02 minimal):
  - macOS: universal `.dmg`, ad-hoc signed, **not notarized**. Release notes: right-click ▸ Open, or
    `xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"`.
  - Windows: unsigned NSIS `.exe` (and/or `.msi`). Notes: SmartScreen ▸ More info ▸ Run anyway.
  - Linux: AppImage + `.deb` (x86_64; arm64 if free).
- ~~**Signing with a new local GPG key (temporary):** generate on agentbox, commit the public key and its fingerprint,
  sign `SHA256SUMS` (`SHA256SUMS.asc`) and the tag.~~ **Superseded (DEC-1058):** no PGP. The workflow attests every
  artifact and `SHA256SUMS` with GitHub build provenance; nothing is signed by hand, no key is in the repo or the
  README, and `docs/release/signing-key-temporary.asc` is replaced by `docs/release/verifying.md`. The temporary
  key stays unused in the secret store.
- **Release** as a normal GitHub release marked *pre-release*, with notes: scope, known limitations
  (no security review, shielded availability depends on the network's protocol version, mixing experimental,
  English only), how to verify (`gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop`, DEC-1058; was
  `gpg --verify SHA256SUMS.asc`).

### Minimal test bar for 0.1

On testnet (fund with the `dash-faucet` skill), on the Linux build on agentbox, plus launch-and-screenshot on the
macOS and Windows CI runners:
1. Create wallet, write down phrase, restore it in a second datadir — same balance.
2. Receive tDASH, send tDASH, see both in Activity; export CSV.
3. Register a username; add a contact (second wallet), accept, pay the contact.
4. Core → Shielded, Shielded → external shielded address (second wallet), Shielded → Core.
5. Restore an iOS-created testnet seed holding Platform + shielded funds: all three balances appear; move Platform
   funds to Core.
6. Start and stop mixing.
7. App launches on macOS and Windows runners (screenshot as artifact).

Bugs found that block 1–7 are fixed; everything else becomes a 0.1.x/0.2 issue.

### Schedule (UTC, four lanes)

| Day | E1 engine | E2 engine | UI | L4 infra |
|---|---|---|---|---|
| Sun 11-01 | shielded facade: sync, balance, receive address, Core→Shielded, send to shielded, withdraw to Core; protocol-version gate | Platform-address sync loop + balance + withdraw to Core; contested-cost fix; sync-on default + network setting | Home breakdown + network picker + CSV button | ~~GPG key~~ (DEC-1058: attestations); release workflow (dmg/exe/AppImage/deb) on a test tag; README; default branch |
| Mon 11-02 | finish + dwcli smoke | finish + dwcli smoke | Shielded screens (receive, send, withdraw), Activity rows, Masternodes toggle | first full CI artifact set; version bump |
| Tue 11-03 | fixes | fixes | fixes | test bar run → release `v0.1.0` (attested by the workflow on the tag) |

Cut order if Tuesday is at risk (last first): fiat stretch → Shielded→Core withdraw (keep send-to-shielded) →
Platform withdraw (keep balance + notice) → Windows artifact. Never cut: shielded balance + Core→Shielded +
shielded send, usernames, contacts, ~~signed checksums~~ attested artifacts (DEC-1058).

## 2. Versions after 0.1

Dates are targets for four agent lanes; the manager re-plans with evidence. Each version: an attested GitHub
pre-release (DEC-1058; no PGP until 1.0), a short test bar, parity.md updated.

### 0.2 — Platform & shielded complete, privacy bridge (target ~2026-11-14)

- Per-route tasks with a design doc each (X1b/X2 split): Platform receive address; external Platform send;
  Core → own Platform; Platform ↔ Shielded; Shielded → Platform; Max drains (Core→Shielded restart-safe, iOS #1205;
  Max to external shielded, #1251).
- Username funding-source picker (Shielded / Dash / Platform) with the shielded readiness checklist; identity
  top-up from Shielded or Platform; credit withdrawal to own Platform address; shielded-funded identity creation.
- Recovery: stuck funding asset locks (types 4/5), "Pending — finish", bulk Finish after restore.
- Vault scopes for DIP-17 and Orchard keys; cached viewing key so shielded sync runs while locked; shielded sync-info.
- **Move mixed coins** to Shielded and to Core, with consolidation under the 100-input cap (iOS #1221).
- Pay a contact from any balance (iOS PR #1117).
- Fiat everywhere (MP-01) — no longer cuttable.
- Protocol-compatibility advisory banner (iOS #1232); send guard while a restored wallet is still syncing
  (iOS #998/#1238); wallet-open failure cards + reset-and-rescan (iOS #1195/#1200).
- Verify same-seed shielded derivation matches iOS and Android 12.0.0 (restore test with both).
- Re-baseline parity.md on Tauri; add IOS-124+ and Android 12.0.0 rows; refresh research docs.

### 0.3 — Masternodes & governance for owners, Core power stories (target ~2026-11-25)

- My masternodes UI (MG-10): list, detail, keys, payouts, tracking, evonode credits + withdrawal (MG-13).
- Registration wizard: regular + evonode, FundNew / ExistingUtxo / External collateral; ProUpRegTx.
- Shared masternodes UI (MG-10b): create, reward address, key rotation, dissolve now/together, standby.
- Governance: proposal creation/resume polished, vote with many MNs (built; QA).
- Voting-key import (iOS #1162); contested-name voting polish (filters, vote change, cast ceiling).
- Masternodes section visible by default once a wallet holds MN keys.
- MG-12 security review of the MG signing paths.
- CoinJoin to Core parity: V24 denomination promotion/demotion; mixing persists and resumes after restart; keeps
  mixing while locked "for mixing only" and survives auto-lock; App Nap / sleep prevention; start-on-login + tray;
  mixed-send warnings (≥10 inputs, no change); explainer; recognise coins mixed by dash-qt (regtest test);
  restore past the CoinJoin gap limit 30 (Android finding).
- Core stories: open Berkeley DB `wallet.dat` (or a guided import), multi-recipient send, spend-unconfirmed-change
  option, explorer link, per-tx notifications on macOS/Linux/Windows.

### 0.4 — Mobile parity, remaining DashPay (target ~2026-12-09)

- Username marketplace (X4): browse, buy a listed name, extra/second names, instant-name offer at contest confirm.
- Identity-verify link (iOS #1141 — exclusion at DASHPAY:1131 is stale).
- Invitation creation, share, history (X3), shielded-funded invitations; claim Home card + invitation QR scan;
  small invites (0.003).
- DashConnect / App Connect v1 + v2 (dApp login, grants, budgets, expiry, revoke, `dashpay://connect`).
- Transaction metadata on Platform (Android), tax categories + tax CSV (MP-04).
- Private-key sweep incl. BIP38 (MP-07), phrase repair (MP-06), lock-screen quick receive / scan.
- Localization: infrastructure + Transifex pull of the mobile languages (MP-14).
- Settings/support: log export, support path, backup reminder.

### 0.5 — Commerce (target ~2026-12-19)

- Explore: merchants, ATMs, map links (MP-09); DashSpend (CTX, PiggyCards) (MP-10).
- Buy & Sell: Dash DEX / Maya (incl. multi-chain address validation), Topper, Uphold, Coinbase (MP-11); swap rows
  in history. ZenLedger (MP-12).
- CrowdNode: **decision — withdraw-only exit for existing accounts, no new accounts** (iOS is removing it).
- **External blocker B1 (partner credentials): raise with pasta now.** Without keys these ship disabled.

### 0.6 — Hardening (target ~2026-12-31)

- H-01 security review (vault, scopes, leases, IPC/CSP, avatar fetcher, deep links, shielded and Platform paths),
  H-02 fuzzing, H-03 kill -9 matrix incl. shielded/Platform flows, H-09 trust gate, H-06 performance, H-10 DAPI
  pinning; Tor/SOCKS5 proxy.
- Auto-update feed (PK-03), signed with the release key once it exists.

### 0.9 — Release candidate (target ~2027-01-12)

- **Interactive QA** (`agentic-qa`) of every user story on macOS, Windows and Linux (real GUI, evidence published);
  accessibility pass (H-05); mainnet canary with an iOS user (H-07: name, contact round trip, payments both ways,
  invitation claim, shielded round trip).
- Real signing: a real release key (PGP or Sigstore identity, decided then) replaces attestation-only releases;
  notarization and Windows signing if B2 certificates exist (otherwise 1.0 ships unsigned with signed checksums and
  says so).

### 1.0 (target ~late January 2027)

Exit criteria:
- Every iOS and Android user story mapped in parity.md is done or has a written N/A reason (desktop-specific) —
  no "not started".
- Dash Core stories done: masternode registration (regular/evonode), shared masternodes, governance voting and
  proposal creation, CoinJoin at Core parity, coin control, PSBT, sign/verify, CSV export, wallet.dat import.
- Interactive QA report with no open blocker/critical on all three OSes; H-01 findings fixed or accepted in
  writing; kill -9 matrix green.
- Signed releases with the real key.

## 3. Open verifications to run early (not blockers for 0.1 unless they fail)

1. Shielded key derivation is identical across iOS, Android 12.0.0 and desktop (same seed → same balance).
2. Is the shielded pool active on mainnet at its current protocol version? Decides the 0.1 mainnet behaviour.
3. Coins mixed by dash-qt (BIP44 chains) show as mixed on desktop.
4. Desktop restores CoinJoin accounts beyond 30 addresses.
