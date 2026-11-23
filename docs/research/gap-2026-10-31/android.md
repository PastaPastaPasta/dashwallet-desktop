# Dash Android wallet: user-story inventory mapped onto the desktop plan (2026-10-31)

Purpose: find what `dashwallet-desktop`'s plan is missing, starting from every user story in the Android wallet.
Read-only research; nothing was checked out, committed or changed in either repo.

## 0. Sources, method, legends

| Item | Value |
|---|---|
| Android repo | `~/workspace/dash-wallet`, `origin/master` = `472e86090` (2026-10-31, v11.9.1 line, `minSdk 24`: `wallet/build.gradle:270-274`) |
| Android big branch | `origin/feat/kotlin-sdk-phase-1` (440 commits ahead of master, tip `2e6259194`; `versionName "12.0.0-sync"`, `minSdk 29`, 64-bit only: `wallet/build.gradle:307-322` on that ref). Called **S1** below. Exported read-only with `git archive` to a scratch dir to get line numbers. |
| Desktop repo | `~/workspace/dashwallet-desktop` @ `9e3add03` |
| Desktop plan docs | `docs/design/ROADMAP.md` (R), `docs/parity.md` (P), `docs/design/DASHPAY.md` (D), `docs/design/DECISIONS-PENDING.md`, `docs/research/dashpay/android.md` (the desktop team's Android research, "AND-xxx") |
| Method | (1) walked manifest, nav graphs, Tools/Settings/Security screens, string-prefix histogram on master; (2) re-verified every AND-xxx claim that I rely on against master with my own file:line; (3) read S1's migration plan + new UI + string resources; (4) diffed all 138 remote branches by "ahead of master" count and read commit subjects of the feature-like ones; (5) grepped the desktop docs per topic. |
| Not done | No GUI run of either app. `gh` (GitHub API) was blocked by the sandbox TLS proxy, so PR numbers come only from commit subjects. Did not read the desktop Rust/TS code, only its docs. |

**Path prefixes.** `W/` = `wallet/src/de/schildbach/wallet/`; `C/` = `common/src/main/java/org/dash/wallet/common/`;
`X/` = `features/exploredash/src/main/java/org/dash/wallet/features/exploredash/`;
`IU/IC/IN/IM/` = `integrations/{uphold,coinbase,crowdnode,maya}/src/main/java/org/dash/wallet/integrations/{uphold,coinbase,crowdnode,maya}/`.
Evidence prefixed `S1:` is `origin/feat/kotlin-sdk-phase-1`; `connect:` is `origin/feat/connect-ui-update`; same path layout as master. Other branches are named in the row.
`R:263` = ROADMAP.md line 263; `P:IOS-025` = parity.md row; `D:610` = DASHPAY.md line 610; `android.md:N` = line N of the desktop file `docs/research/dashpay/android.md`.

**Flag column.** `master` = on `origin/master`. `branch-only (name)` = not on master. `flag-gated (X)` = on the ref but behind X.

**Gap column starts with one of:**
`COVERED` = a roadmap task or parity row delivers the story (build status is in the mapping text; "not started" is still COVERED if a task owns it).
`PARTIAL` = some of the story is in the plan; the text says which part is missing.
`MISSING` = NOT IN PLAN (no task, no parity row, no exclusion).
`EXCLUDED` = the plan says no (quoted with doc:line).
`N/A` = mobile-only or legacy-store behaviour with no desktop analogue.

## 1. The branch picture (what "Android" means today)

There are two Androids. The desktop research (`android.md`, written against master `a8de1f079`, 2026-10-07) only describes the first.

| | Android master (ships today, 11.9.1) | S1 `feat/kotlin-sdk-phase-1` (12.0.0, "ground-up QA release") |
|---|---|---|
| L1 engine | dashj SPV | Rust SDK SPV (compact filters), dashj still runs as shadow until a persisted cutover (`CutoverCoordinator`, `S1:W/service/platform/sdk/CutoverCoordinator.kt:36`) |
| CoinJoin | full mixing client | **removed** (`83ce13ca1 feat!: remove CoinJoin mixing`); replaced by a "Move your mixed coins" sheet (Shielded or Dash Wallet) |
| Shielded (Orchard) pool | **none** (`git grep -il shielded origin/master` hits only the Maya Zcash address parsers) | **user-facing**: hub, internal transfer, send, username/invite funding, home total, mixed-funds migration |
| Platform (DIP-17) addresses | none | none user-facing (`git grep -i "platform address\|PlatformAddress" origin/feat/kotlin-sdk-phase-1 -- wallet/src/de/schildbach/wallet/ui` is empty); only SDK-internal credits plumbing |
| Flags | n/a | `USE_KOTLIN_SDK_SHIELDED`, `_DASHPAY_WRITES`, `_DPNS_READS`, `_L1_INVITE`, `_L1_SHADOW` are **seeded ON in every build, including the prodRelease store build** (`S1:W/ui/dashpay/utils/DashPayConfig.kt:441-469` and `:852-861`; policy dated 2026-07-30). `_L1_SEND` is never seeded (`:430-438`). |

Other unmerged lines with user-visible stories: `feat/connect-ui-update` (96 ahead of master; Connect v2 UI), `codex/mo-1072-connect` (73), `feat/app-connect-dashpay-carrier` (64), `feature/dashpay-user-qr` (3; My QR / Scan QR), `t3code/audit-invitation-funding` (6), `feat/analytics-opt-out` (13 over S1), `feat/home-balance-include-shielded` (23 over S1, mostly reconciliation hardening). Most other branches (`qa/integration-run*`, `fix/*`, `chore/*`, `diag/*`) are QA or hardening and add no stories.

Master moved only 4 commits since the desktop snapshot: `7b2e23188` (#1583 contested fee follows protocol version), `cdaf542b6` and `233a26c9b` (the `/pre-review` skill), `472e86090` (#1660 dark-mode Cancel button).

## 2. Verified negatives (the owner's suspicion, tested)

1. **Shielded balances on Android master: none.** `git grep -il shielded origin/master` lists only `integrations/maya/.../ZcashAddressParser.kt` and `ZcashPaymentIntentParser.kt`. (`git grep orchard` hits the BIP39 English wordlist entry "orchard", not the pool.)
2. **Shielded on Android S1: yes, and it is the headline privacy feature.** See section 9. So the desktop rationale "Android has no shielded pool at all" (`D:610`, X2 row) and "Android: Core and invitation" (`D:1106`) and `android.md:39` are true of master only.
3. **Platform addresses on Android: none anywhere.** Not on master, not on S1 UI. The owner's suspicion that desktop underplans Platform addresses is therefore an **iOS** parity question (IOS-045, 047, 053, 059-064), not an Android one. Android's evidence is only indirect: S1's username-via-L2 design hops funds through a Platform address internally (`S1:docs/kotlin-sdk-migration-plan.md:513-542`), and in the shipped code only the shielded impl calls `fundFromAssetLock` (`S1:W/service/platform/sdk/ShieldedBalanceServiceImpl.kt`). Whether leftover credits can sit on a DIP-17 address after an interrupted shield is **unverified**.
4. **Android funds usernames and invites from exactly two sources**: Dash Wallet (L1) and Shielded (`S1:W/ui/username/request/UsernamePaymentSheets.kt:100-107`, enum `UsernamePaymentSource`). Never from Platform addresses or identity credits.

## 3. Onboarding, create, restore, backup

| ID | Story (as a user) | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| ON-1 | I see a short intro (3 slides) and choose Create or Restore | `W/ui/onboarding/WelcomePagerAdapter.kt:10`, `W/ui/OnboardingActivity.kt:78` | master | U-02 Welcome; `P:IOS-001` partial (M5, demo mode is iOS-only) | PARTIAL: Android has no demo; carousel copy is M5 |
| ON-2 | I create a wallet with 12 or 24 words | `W/ui/onboarding/SelectSecurityLevelActivity.kt:10-38` | master | U-02, `P:IOS-002` done (12, 24 under Advanced) | COVERED |
| ON-3 | I set a 4-digit PIN (and am offered biometrics) | `W/ui/SetPinActivity.kt:56,440`, `W/ui/widget/PinPreviewView.kt:33` | master | Desktop uses a vault passphrase by design (`P:IOS-010` partial, DESIGN-opus 1.8); `P:IOS-011` Touch ID on macOS only | COVERED (by design: passphrase, not PIN) |
| ON-4 | I may skip backing up the phrase now and am nagged until I do ("Secure now" shortcut, reminder after first funds) | `W/ui/main/shortcuts/ShortcutOption.kt:29`, `W/ui/LockScreenActivity.kt:217` | master | `P:IOS-005` partial (macOS Overview only; none in CrossUI/Tauri status unknown); desktop verifies the phrase before the wallet goes live (IOS-002/004) | PARTIAL: reminder only on one shell |
| ON-5 | I view my recovery phrase (screenshot-blocked) and verify it by tapping words in order | `W/ui/verify/ShowSeedFragment.kt:75` (FLAG_SECURE), `W/ui/verify/VerifySeedConfirmFragment.kt:40` | master | `P:IOS-003/004` done/partial, `P:IOS-006` capture guard macOS only | COVERED (capture guard missing on Windows/Linux) |
| ON-6 | I restore from a 12 or 24 word phrase in any of 9 BIP39 languages; the app detects the language | `W/util/MnemonicCodeExt.kt:63` (12/24 only), `:70-90` (`checkNonEnglish` tries every asset list), `wallet/assets/bip39-wordlists/` (9 files) | master | `P:IOS-007` partial (live per-word check, "not every BIP39 language checked"); `QT-104` 12/15/18/21/24 English | COVERED. `android.md:56` is wrong here (see section 19) |
| ON-7 | On restore I say roughly when the wallet was created so the rescan starts there | `W/ui/RestoreWalletFromSeedActivity.kt:152-184`, `W/ui/compose_views/WalletCreationDateInfoDialog.kt:42` | master | U-02 D25 (`birthHeight.ts`), tests `QT105_birthDateBecomesAConservativeHeight` | COVERED |
| ON-8 | I restore from an encrypted wallet backup file (debug builds only) | `W/ui/OnboardingActivity.kt:237`, `W/ui/backup/RestoreFromFileActivity.kt` | master, DEBUG only | dash-qt wallet import `P:QT-106..108` partial | N/A (debug-only on Android) |
| ON-9 | An invitation link opened before I have a wallet is remembered and used after setup | `W/ui/OnboardingActivity.kt:212`, `W/ui/InviteHandlerActivity.kt:43` | master | DP5-01 (vault stash, replay after onboarding) | COVERED |
| ON-10 | I see clear screens when the wallet file is invalid/unencrypted or the PIN is permanently locked (8 failures) and can wipe | `wallet/res/layout/activity_onboarding_invalid_wallet.xml`, `activity_onboarding_perm_lock.xml`, `W/ui/ResetWalletDialog.kt:36`, `W/ui/OnboardingActivity.kt:270` | master | `P:IOS-012` done (disabled at 8, restore offered), `P:IOS-109` done | COVERED |
| ON-11 | After an old-version upgrade I am forced to set a PIN / upgrade key chains | `W/ui/AppUpgradeActivity.kt:37`, `W/ui/security/UpgradePinDialog.kt:40` | master | no legacy store on desktop | N/A |
| ON-12 | I reset (wipe) my wallet after a warning that offers a backup shortcut, and (with an identity) a chance to save my tx metadata first | `W/ui/more/SecurityFragment.kt:136-155`, `W/ui/more/SecurityViewModel.kt:105`, `W/ui/more/SaveMetadataAndResetDialogFragment.kt:43` | master | `P:IOS-109` done (typed sentence); metadata save is F22 (see META-4) | PARTIAL: "save metadata before wipe" has no desktop counterpart |

## 4. Security, lock, auth

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| SEC-1 | I unlock with PIN pad or fingerprint; I see a lockout countdown | `W/ui/LockScreenActivity.kt:355,495,528` | master | `P:IOS-013` partial (passphrase; "no Quick Receive, Scan to Send or biometrics"), `P:IOS-011` | PARTIAL: biometrics macOS only; Windows Hello is W-02 |
| SEC-2 | From the lock screen I can open Quick Receive or Scan to Pay without unlocking | `W/ui/LockScreenActivity.kt:317,320`, `W/ui/payments/QuickReceiveActivity.kt:31` | master | `P:IOS-013` lists both as missing; `P:IOS-117` menu-bar companion shows a receive QR (macOS) | PARTIAL: no lock-screen receive on Windows/Linux; Scan to Pay not planned |
| SEC-3 | Wrong PINs: 3 free tries, then 6^(n-3) minute waits, permanent lock at 8 | `W/security/PinRetryController.java:46-48,74,85` | master | `P:IOS-012` done (same policy) | COVERED |
| SEC-4 | I recover from a forgotten PIN by entering my phrase and setting a new PIN | `W/ui/ForgotPinActivity.kt:27`, `W/ui/security/ForgotPinDialog.kt` | master | `P:IOS-014` done | COVERED |
| SEC-5 | Every sensitive action (send, show phrase, keys, voting, identity ops) asks for PIN/biometric | `W/ui/CheckPinDialog.kt`, `W/ui/DecryptSeedWithPinDialog.kt` | master | `P:IOS-017` partial (voting/keys later); E0-04 grants | COVERED |
| SEC-6 | Security menu: view phrase, change PIN, biometric toggle, autohide balance, reset | `W/ui/more/SecurityFragment.kt:80-95` | master | `P:IOS-108` partial, U-04 | COVERED |
| SEC-7 | Advanced security: auto-logout (immediately/1m/5m/1h/24h), spending confirmation, biometric spending limit (0/0.1/0.5/1/5 DASH) | `W/ui/more/AdvancedSecurityActivity.kt:41,64,112` | master | `P:IOS-015` done, `P:IOS-016` partial (limit macOS only; no security-level meter) | PARTIAL |
| SEC-8 | Wallet secrets are wrapped by the OS keystore with a mnemonic-derived recovery key | `W/security/MnemonicBasedKeyProvider.kt:39` | master | `P:IOS-123` partial (dw-vault; Windows store unverified) | COVERED |

## 5. Home

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| HOME-1 | I see my balance with a fiat line, tap to hide it (hint shown once), "syncing balance" state | `W/ui/main/HeaderBalanceFragment.kt:36,69` | master | `P:IOS-019/020` partial; MP-01 fiat (1.0); IOS-020 "no autohide option or first-use hint" | PARTIAL: fiat is MP-01 (cuttable, `R:386`); first-use hint missing |
| HOME-2 | I see sync progress ("Syncing NN%") and a "no connection" pane | `W/ui/main/WalletTransactionsFragment.kt:322` | master | `P:QT-025` done, `P:IOS-023` partial | COVERED |
| HOME-3 | I have a 4-slot shortcut bar (long-press to customise; defaults depend on balance/backup state) | `W/ui/main/shortcuts/ShortcutOption.kt:24`, `ShortcutProvider.kt:34`, `W/ui/main/WalletFragment.kt:291` | master | `P:IOS-025` partial (macOS Overview only) | PARTIAL: Tauri Home status not recorded in parity |
| HOME-4 | Toolbar shows my avatar (opens Edit Profile) and a bell with an unread dot | `W/ui/main/WalletFragment.kt:239` | master | DP1-08 (chip), DP2-09 (bell) | COVERED |
| HOME-5 | A "Join DashPay" row shows identity-creation progress, retry, done | `W/ui/dashpay/HistoryHeaderAdapter.kt:35` | master | DP1-08/09, `P:IOS-065` | COVERED |
| HOME-6 | After an invite link, an "Accept invite" row offers Create username / Hide, validating the invite first | `W/ui/main/WalletTransactionsFragment.kt:319,446` | master | DP5-03 | COVERED |
| HOME-7 | A mixing status card shows percent mixed, paused/finishing | `W/ui/main/MixingStatusCard.kt:37` | master (removed on S1) | U-06 CoinJoin card; `P:QT-041` not started | COVERED (see CJ section for scope question) |
| HOME-8 | I get a toast when exchange rates are stale, failed or volatile | `W/ui/main/MainActivityExt.kt:448` | master | MP-01 (IOS-024, 1.0) | COVERED (MP-01 cuttable) |
| HOME-9 | A dialog warns me my clock is skewed (CoinJoin-aware copy) | `W/ui/main/MainActivity.kt:316`, `MainActivityExt.kt:76` | master | MP-03 (IOS-026, 1.0, cuttable `R:381`) | COVERED |
| HOME-10 | A dialog warns me storage is low | `W/ui/main/MainActivity.kt:317` | master | `P:QT-004` data-dir chooser shows free space; no runtime low-disk warning | N/A (mobile); desktop could add a low-disk notice |
| HOME-11 | If my SIM/network country suggests another currency I'm asked to switch | `W/ui/main/MainActivityExt.kt:259` | master | `P:IOS-104` default from OS locale | N/A |
| HOME-12 | I'm told why notifications matter and asked for permission once | `W/ui/main/MainActivityExt.kt:336` | master | `P:IOS-105` partial | COVERED |
| HOME-13 | I'm prompted to exempt the app from battery optimisation | `W/ui/main/MainActivityExt.kt:353`, `W/ui/SettingsFragment.kt:142` | master | none | N/A (mobile) |
| HOME-14 | After a crash I'm offered to send the stack trace to support | `W/util/CrashReporter.java:70` | master | `P:IOS-112` partial (log export; no support email); no crash reporter in plan | PARTIAL |
| HOME-15 | Links and intents route me: `dash:`, `pay:`, invite, `dash-key:`, NFC, `payments/{tab}`, `transactions/{id}`, exchange OAuth returns | `W/ui/main/MainActivity.kt:172`, `wallet/res/navigation/nav_home.xml:138,244,560,584` | master | U-01 deep links (`dash:`, `pay:`, `dashwallet:`, `dashpay:`); `P:IOS-048` partial (`pay:`/`dashwallet:`/Linux+Windows handlers not built) | COVERED (OAuth returns belong to MP-11) |

## 6. History, details, tax, export

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| HIST-1 | I scroll a paged history grouped by day with a loading state | `W/ui/main/WalletTransactionsFragment.kt`, `W/ui/main/PrebuiltRowsPagingSource.kt` | master | `P:IOS-027` done | COVERED |
| HIST-2 | I filter All / Sent / Received / Gift cards | `W/ui/main/TransactionsFilterDialog.kt:34`, `W/transactions/TxDirectionFilter.kt:24` | master | `P:IOS-028` partial (gift-card chip M5) | COVERED |
| HIST-3 | Rows show contact avatar/name, merchant/service icons, status | `W/ui/main/TransactionAdapter.kt` | master | DP3-07; `P:IOS-029` partial (icons need M4/M5 data) | COVERED |
| HIST-4 | Mixing txs group per day; CrowdNode txs group; a group sheet explains | `W/ui/transactions/TransactionGroupDetailsFragment.kt:40` | master | `P:IOS-030` partial (CoinJoin per day done; CrowdNode M5) | COVERED |
| HIST-5 | Tx details: amount+fiat, addresses, OP_RETURN, fee, date, status, contact, BIP70 payee | `W/ui/transactions/TransactionDetailsDialogFragment.kt:62`, `TransactionResultViewBinder.kt:101` | master | `P:IOS-031` partial (no Insight fee lookup) | COVERED |
| HIST-6 | A failed tx explains why and offers Rescan blockchain | `W/ui/transactions/TransactionDetailsDialogFragment.kt:142` | master | `P:IOS-034` partial (Remove unconfirmed in Tools > Repair, not driven) | PARTIAL |
| HIST-7 | A credit top-up tx shows the Platform credits leg (completed / error / not transferred) | `W/ui/transactions/TransactionResultViewBinder.kt:482` | master | `P:IOS-033` not started (M4, rebroadcast/complete stuck asset locks); DP1-06 | PARTIAL |
| HIST-8 | I open a tx in Insight or Blockchair (and a swap in the provider explorer) | `W/ui/transactions/BlockExplorerSelectionView.kt:29` | master | `P:IOS-032` partial | COVERED |
| HIST-9 | I report an issue about a tx | `W/ui/transactions/TransactionDetailsDialogFragment.kt:205` | master | `P:IOS-112` partial | PARTIAL |
| HIST-10 | I add/edit a private note on a tx | `W/ui/dashpay/transactions/PrivateMemoDialog.kt:36` | master | D:1125 F22 "local notes exist" | COVERED (local only; Platform sync is META) |
| HIST-11 | I rebuild the history display cache from a menu | `W/ui/main/WalletTransactionsFragment.kt:197` | master | no display cache | N/A |
| HIST-12 | After sending, a full-screen result with explorer link and sent sound | `W/ui/transactions/TransactionResultActivity.kt:61` | master | `P:IOS-052` partial (opens tx in Transactions) | COVERED |
| HIST-13 | A gift-card purchase row opens the card (or order) details | `W/ui/main/WalletTransactionsFragment.kt:73` | master | MP-10 (IOS-103), disabled until B1 | COVERED (deferred) |
| HIST-14 | Each tx stores the fiat rate at the time and shows it | `C/transactions/*`, `W/database/dao/TransactionMetadataDao.kt` | master | MP-01 (IOS-038) | COVERED |
| TAX-1 | I tag each tx Income/Transfer In or Expense/Transfer Out; a one-time "Reclassify" explainer; linked-exchange addresses are pre-tagged | `W/ui/transactions/TransactionDetailsDialogFragment.kt:206`, `TaxCategoryExplainerDialogFragment.kt:30` | master | MP-04 (IOS-036/037, 1.2) | COVERED (1.2) |
| TAX-2 | I export a 12-column tax CSV (blocked until synced) | `W/ui/compose_views/ExportCSVDialog.kt:65`, `W/util/WalletUtils.java:204`, `W/ui/more/ToolsViewModel.kt:89` | master | MP-04 (IOS-039, 1.2); plain history CSV `P:QT-093` done | COVERED (1.2) |
| TAX-3 | I send my addresses to ZenLedger | `W/ui/compose_views/ZenLedgerDialog.kt:63`, `W/ui/more/tools/ZenLedgerClient.kt:28` | master | MP-12 (IOS-040), needs B1 | COVERED (deferred) |
| META-1 | I choose to save tx metadata (categories, notes, fiat, gift cards) to Platform: past/future toggles, frequency (10 txs / weekly / every tx), progress | `W/ui/more/TransactionMetadataSettingsScreen.kt:59`, `TransactionMetadataSettingsViewModel.kt:69`, `W/ui/more/SettingsScreen.kt:228`, `Constants.java:68` (`SUPPORTS_TXMETADATA`) | master | D:1125 F22 "Android only; local notes exist; Platform sync post-1.0; upstream or ours". No task ID, no parity row | MISSING (explicitly deferred post-1.0, owned by no task) |
| META-2 | First-time prompt and a cost explainer (credits per upload) | `W/ui/more/TransactionMetadataDialog.kt:37`, `TransactionMetadataCostDialog.kt:30` | master | same | MISSING |
| META-3 | After restore, my metadata is downloaded and merged | `W/service/platform/PlatformSyncService.kt:253,1204`, `W/service/platform/work/PublishTransactionMetadataWorker.kt:36` | master | same | MISSING |
| META-4 | Before a wipe I can save unsaved metadata | `W/ui/more/SaveMetadataAndResetDialogFragment.kt:43` | master | same | MISSING |

## 7. Send, receive, QR, URIs, BIP70

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| SEND-1 | A Payments screen with Receive / Pay, scan, send to address, send to contact, and a strip of frequent contacts | `W/ui/payments/PaymentsFragment.kt:50`, `PaymentsPayFragment.kt:60`, `W/ui/dashpay/FrequentContactsAdapter.kt:27` | master | U-03 Pay/Receive; DP3-02 frequent contacts; D:1117 | COVERED |
| SEND-2 | I paste/type an address, the app suggests what is on my clipboard, bad addresses error | `W/ui/send/DashAddressInputFragment.kt:32`, `C/ui/address_input/AddressInputViewModel.kt:21` | master | `P:IOS-042` partial (clipboard suggestion listed in the iOS row) | COVERED |
| SEND-3 | I scan a QR (torch, autofocus) that may hold a URI, BIP70, private key, DashPay user id or raw tx | `C/ui/scan/ScanActivity.kt:74`, `W/ui/util/InputParser.java:77` | master | `P:IOS-043` partial (image/clipboard; no webcam or screen region yet); DP2-08 | COVERED |
| SEND-4 | Amount keypad with DASH/fiat toggle and Max; revealing a hidden balance needs PIN | `C/ui/enter_amount/EnterAmountFragment.kt:61` | master | `P:IOS-044` partial; MP-01 | PARTIAL: fiat toggle is MP-01 |
| SEND-5 | Confirm sheet (address or username+avatar, amount, fee, total) then auth, sign, broadcast | `W/ui/send/ConfirmTransactionDialog.kt:39` | master | `P:IOS-046`, `QT-059` done | COVERED |
| SEND-6 | Insufficient funds offers "Empty wallet"; dust and adjust-down errors | `W/ui/send/SendCoinsFragment.kt:188,276` | master | `P:QT-055/062` done, `QT-053` use-available | COVERED |
| SEND-7 | When mixing is on I can only spend mixed funds ("insufficient mixed money") | `W/ui/send/SendCoinsFragment.kt:167,189` | master (removed on S1) | `P:QT-051` partial (view model only; no CoinJoin page) | COVERED |
| SEND-8 | `dash:` `darkcoin:` `pay:` URIs (BIP21 amount/label/message, `r=`) open Send; NFC tag tap does the same | `wallet/AndroidManifest.xml:169-171,151` | master | `P:QT-054` done, `P:IOS-048` partial (`pay:` handler not built) | PARTIAL: `pay:` scheme and Linux/Windows handlers (W-02/PK-01) |
| SEND-9 | I pay a BIP70/72 request (PKI "secured by", expiry, network check, ACK) | `W/ui/send/PaymentProtocolFragment.kt:51` | master | MP-05 (IOS-049, 1.2); prerequisite of DashSpend | COVERED (1.2) |
| SEND-10 | I pay a DashPay contact; if they had requested me and I hadn't accepted, paying auto-accepts | `W/ui/send/SendCoinsFragment.kt:240` | master | DP3-01 explicit "Accept and pay" (D:589, D:1117) | COVERED (deliberately different) |
| SEND-11 | Send is blocked while syncing / offline with clear copy | `W/ui/send/SendCoinsFragment.kt` (`updateView`) | master | `P:IOS-051` done | COVERED |
| SEND-12 | Other apps ask my wallet to pay (with `payack` callback), give a receive address or my master public key via `dashwallet:` | `W/ui/WalletUriHandlerActivity.kt:48`, `W/ui/util/WalletUri.java:42`, `integration-android/.../BitcoinIntegration.java:29` | master | D:1131 "Also out: `dashwallet://request=address` callbacks"; `dashwallet:` pay routes via U-01 | PARTIAL/EXCLUDED: address and xpub callbacks excluded; pay-with-callback not specified |
| SEND-13 | NFC tap to read a payment request | `wallet/AndroidManifest.xml:34,151` | master | none | N/A (mobile) |
| RECV-1 | Receive screen: QR, address+copy, my username+copy, share | `W/ui/payments/PaymentsReceiveFragment.kt:44` | master | `P:IOS-053` partial, DP3-03 (username in Receive) | COVERED |
| RECV-2 | Specify an amount; the QR carries amount, address, username (no paid detection) | `W/ui/payments/ReceiveDetailsDialog.kt:38`, `ReceiveFragment.kt:40` | master | `P:IOS-055` partial; `P:QT-081/082/085` done | COVERED |
| RECV-3 | I import/sweep a paper wallet: WIF or BIP38 (password), see found balance, sweep with fee | `W/ui/payments/SweepWalletFragment.java:102`, `SweepWalletPasswordDialog.kt:31`, `W/ui/compose_views/ImportPrivateKeyDialog.kt:23` | master | MP-07 (IOS-056, 1.2); `P:IOS-056` "decide / not started" | COVERED (1.2) |
| RECV-4 | I get a "received X DASH" notification and a coin sound | `W/service/BlockchainServiceImpl.kt:489,564` | master | `P:QT-031/032` partial (never shown by a running bundle); `P:IOS-116` | COVERED |
| RECV-5 | A home-screen widget shows my balance with Quick Receive / Scan to pay; launcher shortcuts do the same | `W/WalletBalanceWidgetProvider.java:63`, `wallet/res/xml/shortcuts.xml` | master | `P:IOS-117` menu-bar/tray companion (macOS only) | N/A (mobile) with desktop equivalent partial |

## 8. CoinJoin

Android master mixes; S1 removes mixing. Desktop plans a full dash-qt-style CoinJoin client (QT-041..050, all "not started", U-06) and says CoinJoin is "build first" (desktop `CLAUDE.md`).

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| CJ-1 | First-time CoinJoin info (spend only mixed, auto-mix while app open, battery) | `W/ui/coinjoin/CoinJoinInfoFragment.kt:30` | master | `P:QT-018` (CoinJoin info dialog, M3) | COVERED |
| CJ-2 | I pick Intermediate or Advanced level and start/stop mixing (confirm changes) | `W/ui/coinjoin/CoinJoinLevelFragment.kt:39`, `W/data/CoinJoinConfig.kt:26` | master | `P:QT-044/046/047` not started (rounds 2-16, advanced toggle) | COVERED (different knobs: rounds vs levels) |
| CJ-3 | Mixing warns on mobile data and refuses when the clock is off | `W/ui/coinjoin/CoinJoinLevelViewModel.kt:61,28` | master | MP-03 time skew; metered-link check n/a | COVERED / N/A |
| CJ-4 | Progress: Home card, Settings row ("Mixing 45%", "x of y"), foreground notification | `W/ui/main/MixingStatusCard.kt:37`, `W/ui/more/SettingsScreen.kt:124-157`, `W/service/BlockchainServiceImpl.kt:1754` | master | `P:QT-041/042/043` (panel, progress formula, fully-mixed rule), U-06 | COVERED |
| CJ-5 | Before creating a username I'm advised to mix first (Mix balance / Skip) | `W/ui/more/MixDashFirstDialogFragment.kt:36` | master (removed on S1) | D:1104 F1 "CoinJoin privacy tip" | COVERED |
| CJ-6 | Mixing txs show as one grouped row per day | `W/transactions/coinjoin/CoinJoinMixingTxSet.kt:10` | master | `P:IOS-030`, `QT-038` | COVERED |

## 9. Platform addresses, credits, shielded (the owner's question)

Desktop plan recap: X1a read-only Platform-address balance + restore notice for shielded (1.0, cuttable: `R:215,382`); X1b Platform transfers/receive/advanced mode (1.1, `R:216`); X2 shielded balance, shield/unshield, shielded registration funding, "move mixed coins to Shielded" (1.1, 3 x L, `R:217`); 1.0 "restore screen and release notes say that shielded balances are not shown yet" (`D:610`).

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| PLT-1 | I hold, receive or move funds on a Platform (DIP-17) address | **No such Android story.** Master: none. S1: none in `ui/` (git grep empty). | none | X1a/X1b, `P:IOS-053/064` are iOS-derived | N/A for Android parity. See section 2 item 3 for the indirect evidence |
| PLT-2 | I see my identity credit balance (Tools > Credits) and buy credits from my Dash balance, with an estimate of contacts/profile updates; I can read "What are credits?" | `W/ui/more/ToolsScreen.kt:209`, `W/ui/send/BuyCreditsFragment.kt:26`, `W/ui/more/tools/WhatAreCreditsDialogFragment.kt:19`, `ConfirmTopUpDialogFragment.kt:39`, `W/service/platform/TopUpRepository.kt:163` | master | DP1-06 (top-up, cost table), DP1-10 (top-up sheet); `P:IOS-069` partial | COVERED. Android funds top-ups from Core only; desktop's iOS-style Platform/Shielded top-up sources are X1b/X2 |
| PLT-3 | I get low/empty-credit warnings before sending a contact request or editing my profile, with "Buy Credits" | `W/ui/dashpay/DashPayViewModel.kt:26`, strings `credit_balance_*_warning_*` (`strings-dashpay.xml`) | master | DP1-10 inline low-credit warnings (D:1111 F8) | COVERED |
| SH-1 | My More screen shows two balance cards, Dash Wallet and Shielded ("Syncing..." while the pool loads) | `S1:W/ui/more/MoreFragment.kt:445` | flag-gated (`USE_KOTLIN_SDK_SHIELDED`, seeded ON) | `P:IOS-021` balance breakdown card, not started (M1/M4); X1a "breakdown line + info sheet (MP-02)" is Platform-only | PARTIAL: X1a breakdown has no Shielded line until X2 |
| SH-2 | The Home header total is my Core balance **plus** my shielded balance (unknown shielded balance holds the header in "syncing") | `S1:W/ui/main/MainViewModel.kt:517`, `S1:W/ui/main/HeaderBalanceFragment.kt:57-76` | flag-gated | `P:IOS-019` "Core + Shielded (+ Platform in advanced mode)", Core only today | PARTIAL: until X2 (1.1) a desktop restore of an Android 12 wallet shows a lower Home total than the phone |
| SH-3 | A Shielded hub with tabs Receive / Internal / Send; Payments gains an "Internal" segment | `S1:W/ui/shielded/ShieldedBalanceActivity.kt:68`, `S1:W/ui/payments/PaymentsFragment.kt:66`, `PaymentsInternalFragment.kt:65` | flag-gated | X2; `P:IOS-053/062` | COVERED (1.1) |
| SH-4 | I receive to a shielded address (`dash1...`/`tdash1...`) with QR, copy, share | `S1:W/ui/shielded/ShieldedReceiveScreen.kt:77`, `ShieldedBalanceService.kt:333` | flag-gated | X2; `P:IOS-053` (Core/Shielded toggle) | COVERED (1.1) |
| SH-5 | Internal transfer Dash Wallet -> Shielded (asset-lock shield, about 30 s) | `S1:W/ui/shielded/ShieldedUi.kt:45`, `ShieldedBalanceService.kt:383` | flag-gated | X2 shield; `P:IOS-060/062` | COVERED (1.1) |
| SH-6 | Internal transfer Shielded -> Dash Wallet (withdraw to L1, up to 10 min) with a timing explainer | `S1:W/service/platform/sdk/ShieldedBalanceService.kt:497,507`, strings `shielded_timing_*` (`strings-dashpay.xml:679-684`) | flag-gated | X2 unshield; `P:IOS-060/062` | COVERED (1.1) |
| SH-7 | I send to a shielded address (paste or scan); no memo field in the UI though the service takes one | `S1:W/ui/shielded/ShieldedSendScreen.kt:75`, `ShieldedSendViewModel.kt:81`, service memo arg `ShieldedBalanceService.kt:488` | flag-gated | `P:IOS-060` (shielded -> shielded); `P:IOS-035` mentions the shielded memo | COVERED (1.1) |
| SH-8 | I get a privacy tip ("wait at least 2 hours") and a confirm sheet; Max is handled specially (fee reserve) | `S1:strings-dashpay.xml:675-678`, `S1:W/ui/shielded/ShieldedTransferScreen.kt:96`, `ShieldedTransferViewModel.kt` | flag-gated | `P:IOS-062` privacy tip, timing sheet; `P:IOS-060` shielded Max | COVERED (1.1) |
| SH-9 | A ~30 s proof runs in the background: dismissible dialog, notification when done, 3-min stall watchdog, "Transfer status unknown, do not send again" on ambiguous outcomes | `S1:W/ui/shielded/ShieldedTransferExecutor.kt:292`, `ShieldedUi.kt:63`, strings `shielded_transfer_*` (`:711-721`) | flag-gated | X2; E0-04 unknown-outcome rules; `P:IOS-061` finish stuck transfer | COVERED (1.1) |
| SH-10 | An interrupted Dash Wallet -> Shielded transfer finishes by itself next sync ("Transfer in progress") | `S1:strings-dashpay.xml:704-710,741-742` (`resumePendingWalletShields`) | flag-gated | `P:IOS-061` "Pending - tap to finish"; E0-04 funding resume | COVERED (1.1) |
| SH-11 | Transfers are blocked until the chain is synced, the pool is READY and my funding is verified, with specific copy for each | `S1:W/ui/shielded/ShieldedTransferViewModel.kt:145` (`CHAIN_SYNCING, POOL_SYNCING, FUNDING_PENDING`) | flag-gated | "readiness checklist" in X2 (`R:217`) | COVERED (1.1) |
| SH-12 | I pay for my username from Shielded or Dash balance (shielded recommended; denominations 0.1 / 0.3; wait 2 h for privacy) | `S1:W/ui/username/request/UsernamePaymentSheets.kt:100-107`, `SdkShieldedUsernameCreation.kt:451`, strings `username_payment_*` (`strings-dashpay.xml:472-511,752-759`), `S1:docs/kotlin-sdk-migration-plan.md:544-555` | flag-gated | X2 "shielded registration funding" (F3, D:1106); DP1-02 funding | COVERED (1.1) |
| SH-13 | I fund an invitation from Shielded (decision sheet; overage top-up) | `S1:W/ui/invite/InviteShieldedFundingViewModel.kt:65`, `S1:W/service/platform/sdk/SdkShieldedInviteCreation.kt:46`, `ShieldedInviteOverageTopUp.kt` | flag-gated | X3 "Invitation creation, share and history (Android flow)" says nothing about shielded funding; X2 lists registration only | MISSING: shielded-funded invitations are in neither X2 nor X3 |
| SH-14 | After upgrade, a sheet moves my old mixed coins to Shielded (privacy kept) or to Dash Wallet (un-mix); also under Tools > "Move CoinJoin funds" | `S1:W/ui/migration/MixedFundsMigrationDialogFragment.kt:78`, `S1:strings-dashpay.xml:786-826` | branch-only | X2 "move mixed coins to Shielded" (1.1); MP-08 Core part (1.2); `P:IOS-057` not started (M3) | PARTIAL: Core target is 1.2 (MP-08), Shielded target 1.1; neither is in 1.0 though Android-restored wallets may hold CoinJoin-account funds |
| SH-15 | My history shows the L1 leg of shield/unshield as "Shielded" / "Unshielded" / "Unshielded Withdrawal"; the in-pool activity feed exists in the service but no screen consumes it | `S1:W/service/platform/sdk/CutoverUiDataService.kt:482-490`, `ShieldedBalanceService.kt:249` (`observeShieldedActivity`, no UI caller) | flag-gated | `P:IOS-035` shielded activity rows + memo, not started (M4) | COVERED (desktop plan exceeds Android here) |
| SH-16 | Wiping and restoring brings the shielded balance back from the seed | `S1:docs/kotlin-sdk-migration-plan.md:600-601` ("Shielded pool (already SDK-native)") | flag-gated | D:610 restore notice only until 1.1 | PARTIAL: cross-wallet Orchard derivation compatibility (Android vs iOS vs desktop) is **unverified** by me |

## 10. DashPay identity, usernames, contested-name voting

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| ID-1 | I find "Join DashPay" in More, on Home, and on the Contacts-tab upgrade screen; it waits for sync and a minimum balance | `W/ui/more/MoreFragment.kt:70`, `W/ui/dashpay/UpgradeToEvolutionFragment.kt:39` | master | DP1-08, `P:IOS-065` | COVERED |
| ID-2 | A Welcome screen explains cost (more than 0.03 DASH; some names up to 0.25) and a "What is username voting?" page | `W/ui/username/request/WelcomeToDashPayFragment.kt:18`, `UsernameVotingInfoFragment.kt:20` | master | DP1-08 "FAQ, voting info" | COVERED (amounts changed, see ID-4) |
| ID-3 | I choose a username: rules 3-23 chars, live availability, contested detection with voting dates, invite-limited variants | `W/ui/username/request/RequestUsernameFragment.kt:40`, `UsernameTypesDialog.kt:27` | master | DP1-03, DP1-09 (F4) | COVERED |
| ID-4 | I confirm the name and its cost: 0.03 DASH non-contested; contested 0.15 DASH from protocol version 14, 0.25 before | `W/ui/username/request/ConfirmUsernameRequestDialogFragment.kt:40`, `W/service/platform/ContestedUsernameFees.kt:39-47`, `Constants.java:321` | master (#1583) | DP1-02 quote "real costs from the quote" (F1) | COVERED. `android.md:168,191` say 0.25: stale |
| ID-5 | Registration runs as a resumable 4-step background job with a progress notification and Retry | `W/ui/dashpay/CreateIdentityService.kt:64` | master | DP1-02 (state machine, kill-9 matrix) | COVERED |
| ID-6 | After restoring a seed my existing identity and name come back | `W/service/platform/work/RestoreIdentityWorker.kt:56` | master | DP1-05, "never cut" list (`R:392`) | COVERED |
| ID-7 | While a contested name is voted on I can register an "instant" second name; if I win it replaces the instant one and keeps contacts | `W/ui/compose_views/CreateInstantUsernameDialog.kt:23`, `W/ui/username/request/RequestUsernameSecondaryFragment.kt:9` | master | D:1108 F5 "temporary name is the main name until the contest resolves" (iOS model); DP1-03 temporary-name policy | COVERED (different model; confirm replace-on-win keeps contacts) |
| ID-8 | I can post a "please vote for me" link that is published as an `identityVerify` document | `W/ui/username/request/VerifyIdentityFragment.kt:26` | master | **D:1108** "no verify link (no contract at the pin)"; **D:1131** "Also out: the Android identity-verify link" | EXCLUDED |
| ID-9 | I follow my request status (requesting / voting / blocked / lost / approved) in More and a details screen | `W/ui/username/request/VotingRequestDetailsFragment.kt:41`, `W/ui/dashpay/work/GetUsernameVotingResultOperation.kt:32` | master | DP1-04 contest watch, DP1-10 request status | COVERED |
| ID-10 | If my identity can't pay, flows say so and offer Buy Credits | see PLT-3 | master | DP1-10 | COVERED |
| VOTE-1 | As a masternode owner I list contested requests grouped by name, with filters (type, duplicates only, has link) and sorting | `W/ui/username/voting/UsernameRequestsFragment.kt:61`, `UsernameRequestFilterDialog.kt:33` | master | MG-07/MG-11 (1.1, optional, F21); `P:IOS-079` partial | PARTIAL: Android's filter/sort set not specified; MG-07 is "optional" |
| VOTE-2 | I vote Approve or Block (the "lock" vote), cancel an approval, with a "one vote left" guard | `W/ui/username/voting/UsernameRequestDetailsFragment.kt:55,112`, `W/database/entity/UsernameVote.kt:41-42` (`APPROVE`, `LOCK`), `OneVoteLeftDialogFragment.kt:29`, strings `vote_to_block` (`strings-dashpay.xml:344`) | master | MG-07 (approve/abstain/lock per `P:IOS-079`) | COVERED (no abstain on Android) |
| VOTE-3 | "Quick vote" approves the first-submitted request for every filtered name | `W/ui/username/voting/QuickVoteDialogFragment.kt:29` | master | `P:IOS-079` bulk vote | COVERED |
| VOTE-4 | I import a masternode voting private key (WIF) with detailed validation; several keys cast several votes | `W/ui/username/voting/AddVotingKeysFragment.kt:41`, `VotingKeyInputFragment.kt:41`, `UsernameRequestsViewModel.kt:413,432` | master | MG-03 "tracking any masternode and attaching keys" (IOS-082) | COVERED (1.1) |

## 11. Invitations

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| INV-1 | I open Invitations from More; history filters All / Pending / Claimed; empty states | `W/ui/invite/InvitesHistoryFragment.kt:37`, `InviteFilterSelectionDialog.kt:32`, `W/ui/more/MoreFragment.kt:50` | master | X3 "creation, share and history (Android flow)" (1.1) | COVERED (1.1) |
| INV-2 | I create an invitation choosing Contested or Non-contested funding, then confirm and pay | `W/ui/invite/InvitationFeeDialogFragment.kt:40-96`, `W/ui/more/ConfirmInviteDialogFragment.kt:43`, `W/ui/invite/CreateInviteViewModel.kt:43,86` | master | X3 (storage patch on fork + engine + UI) | COVERED (1.1) |
| INV-3 | I get an "Invitation created" envelope, can tag it ("Dad"), share an iOS-openable link with a generated preview image, copy the link | `W/ui/invite/InviteCreatedFragment.kt:39,67`, `InvitePreviewDialog.kt:31`, `W/data/InvitationLinkData.kt:137` | master | X3 | COVERED (1.1) |
| INV-4 | I open an invitation's details: send again, see who claimed it, or "claimed, username pending" | `W/ui/invite/InviteDetailsFragment.kt:52` | master | X3 "history" | COVERED (1.1) |
| INV-5 | I claim an invitation from a link; it validates (invalid / already claimed / already have a username / contested already requested) | `W/ui/invite/InviteHandler.kt:44`, `W/ui/InviteHandlerActivity.kt:43`, `InviteAlreadyClaimedDialog.kt:33` | master | DP5-02/03 (1.0); D:1121 F18 adds paste and scan | COVERED |
| INV-6 | After claiming, a contact request goes to the inviter automatically and a dialog says so | `W/ui/invite/InviteSendContactRequestDialog.kt:34` | master | DP5-02 "request to the inviter" | COVERED |
| INV-7 | A dismissible "Invite someone" hint appears in user search and notifications | `W/ui/dashpay/UserAlert.kt:27`, `wallet/res/layout/invite_friend_hint_view.xml:5` | master | none in X3 text | PARTIAL: trivial discoverability hint, not specified |
| INV-8 | (branch) I can claim invitations funded with as little as 0.003 DASH, and the inviter contact request is skipped when credits are empty | `t3code/audit-invitation-funding`: `dcdd3a96a`, `7faf7d391` | branch-only (`t3code/audit-invitation-funding`) | DP5-02 "invite-limited names" | PARTIAL: minimum invite value and the empty-credits path are not stated in DP5-02 |
| INV-9 | (branch) L1 invitations create/claim via the SDK's DIP-13 wrappers | `S1:W/service/platform/sdk/SdkL1InviteCreation.kt:52`, `DashPayConfig.kt:389` | flag-gated (`USE_KOTLIN_SDK_L1_INVITE`, seeded ON) | X3 (engine builds on platform-wallet) | COVERED (engine detail) |

## 12. Contacts, requests, profiles, avatars, notifications, "About me"

"About me" is two things on Android: the 140-character profile field (`W/ui/EditProfileActivity.kt:103-142`, `Constants.java:292-293`) and the contact-details sheet redesigned in #1486/#1500/#1505 (`W/ui/dashpay/user/DashPayUserBottomSheet.kt:109`, `W/ui/DashPayUserActivity.kt:58`).

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| CON-1 | Contacts tab: search, sort (name / username / date added / last activity), pending requests with inline Accept/Ignore | `W/ui/dashpay/ContactsFragment.kt:63`, `W/data/UsernameSortOrderBy.kt:18` | master | DP2-07 (sort menu), D:1113 F10 | COVERED |
| CON-2 | I find users by name and see their relation (none / sent / received / contact) | `W/ui/SearchUserFragment.kt:62`, `W/ui/UsernameSearchResultsAdapter.kt:28` | master | DP2-03/08 | COVERED |
| CON-3 | My QR / Scan QR to add a contact (`dashpay://user?id=...&username=...`, verified against Platform, shared wire contract with iOS) | `feature/dashpay-user-qr`: `86661e3dc`, `062489550` (`W/data/DashPayUserLink.kt`, `W/ui/dashpay/user/MyQrCodeBottomSheet.kt`) | branch-only (`feature/dashpay-user-qr`, 3 commits over master) | DP2-03 (user links, `dapk`), DP2-08 (My QR), DEC-15 | COVERED. `android.md:202` ("no QR My code") is stale on this branch |
| CON-4 | Contact sheet: avatar, name, relation actions (Send request / Accept / Ignore / Pay) | `W/ui/dashpay/user/DashPayUserBottomSheet.kt:109`, `W/ui/dashpay/widget/ContactRequestPaneCompose.kt:43` | master | DP2-09 | COVERED |
| CON-5 | Contact payment activity grouped Today/Yesterday/date with All / Received / Sent filter | `W/ui/dashpay/user/DashPayUserBottomSheetViewModel.kt:61` | master | D:1115 F12 "Android's filter" | COVERED |
| CON-6 | Contact requests are sent/accepted by a retrying background job | `W/ui/dashpay/work/SendContactRequestWorker.kt:34` | master | DP2-02 | COVERED |
| CON-7 | Notifications: New / Earlier, inline accept, unread dot | `W/ui/dashpay/NotificationsFragment.kt:55`, `NotificationCountLiveData.kt:26` | master | DP2-05, DP2-09 | COVERED |
| CON-8 | Edit profile: display name (25), about me (140) with counters, unsaved-changes prompt, retry on failure | `W/ui/EditProfileActivity.kt:80`, `W/ui/dashpay/work/UpdateProfileWorker.kt:50` | master | DP4-01/03 | COVERED |
| CON-9 | Avatar from camera, gallery, Gravatar, public URL; circular crop; delete | `W/ui/dashpay/SelectProfilePictureDialog.kt:28`, `GravatarProfilePictureDialog.kt:26`, `ExternalUrlProfilePictureDialog.kt:56`, `CropImageActivity.kt:44` | master | DP4-02/03 (file, drop, clipboard, URL, Gravatar; no camera) | COVERED |
| CON-10 | I upload the avatar to Imgur (no account) or Google Drive (sign-in), with a policy dialog | `W/ui/dashpay/ChooseStorageServiceDialog.kt:30`, `utils/ImgurService.kt:20`, `utils/GoogleDriveService.kt:22` | master | Imgur: B1 (DP4-02 "only with B1"); Google Drive: **D:1131** "Also out: ... Google Drive avatars" | PARTIAL (Imgur deferred on B1) / EXCLUDED (Drive) |
| CON-11 | I set my avatar from the OS share sheet (image) | `W/ui/ImportSharedImageActivity.kt:46` | master | DP4 drag-and-drop | N/A |
| CON-12 | Contacts and profiles sync in the background before block download | `W/service/platform/PlatformSyncService.kt:132` | master | E0-05/E0-06 | COVERED |
| CON-13 | Frequent-contacts strip in Pay | SEND-1 | master | DP3-02/06 | COVERED |
| CON-14 | (no OS notification for contact requests on Android) | `android.md:297` AND-146 | master | DP2-05 plans OS notifications | COVERED (desktop exceeds Android) |

## 13. DashConnect (dApp login via QR key exchange, #1510)

Master has the v1 flow, **testnet/devnet only** (`Constants.java:104` sets `SUPPORTS_CONNECT = false` for prod; `:124,147` true). Branches rebuild it as "App Connect v2" (PV14). Desktop: D:1127 F24 "test networks only | out of 1.0; revisit with Connect v2 | post-1.0"; `P:IOS-085` partial ("`dw-uri::deeplink` routes DashConnect links; nothing else"); `P:IOS-086` not started. No roadmap task ID.

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| CONN-1 | Tools > Connections lists connected apps (Active / Approved / Disconnected) and scans a QR; on mainnet it says "available on testnet only" | `W/ui/more/connections/ConnectionsScreen.kt:71`, `ConnectionsFragment.kt:46`, `W/ui/more/ToolsScreen.kt:195`, `Constants.java:104` | master, flag-gated (`SUPPORTS_CONNECT`, testnet/devnet) | F24 post-1.0, `P:IOS-085` | MISSING (no task; deferred) |
| CONN-2 | I approve "Connect to <app>?" (what it can/can't do), deriving login keys and doing a key exchange | `W/ui/compose_views/ApproveConnectionDialog.kt:59`, `connections/protocol/KeyExchangeCrypto.kt:55`, `LoginKeyDerivation.kt:34`, `DashConnectUri.kt:26` | master, flag-gated | same | MISSING (deferred) |
| CONN-3 | A second `dash-st:` QR registers first-login keys by signing an IdentityUpdate | `W/ui/more/connections/PlatformDashConnectRepository.kt:87` | master, flag-gated | same | MISSING; removed on branches (`dd98e014f` "remove second QR registration path") |
| CONN-4 | (v2) A tabbed Connections page (Active / Disconnected / Expired); each connection has a details sheet: funding limit split between keys, expiry period, date made, Reconnect, Revoke access, Disconnect, "return to app" with a warning | `connect:wallet/res/values/strings_app_connect.xml`, `connect:W/ui/compose_views/ConnectionDetailsDialog.kt`, `AppConnectSession.kt` (820 lines); commits `303056ab8`, `b7fe355d5`, `92374ccfa` | branch-only (`feat/connect-ui-update`, `codex/mo-1072-connect`) | none | MISSING (Connect v2 has no desktop task) |
| CONN-5 | (v2) The approve sheet shows identity funding limit, expiry, group-scope consent, first-connection warning; offers Buy credits or Create username when the identity can't pay | same strings (`dash_connect_insufficient_credits_*`, `dash_connect_first_connection_warning`, `dash_connect_no_username_*`); `d991fc1d5`, `b81fb9af2` | branch-only | none | MISSING |
| CONN-6 | (v2) Apps can start a connection by link `dashpay://connect` (not only QR) | `ab56974b7` (#1656), `AppConnectRequest.kt:108` (https return URL only) | branch-only | `dw-uri` already routes `dashpay:` links (U-01) | PARTIAL: routing only |
| CONN-7 | (v2) I approve an app's token purchase (quantity, max cost in credits, contract, token id) after setting up a purchase signing key | `connect:W/ui/compose_views/ApproveTokenPurchaseDialog.kt`, strings `dash_st_purchase_*`, `14f0e045e`; "legacy DashConnect login requests rejected" `004e748b7` | branch-only | `P:IOS-086` not started (M4); F24 post-1.0 | MISSING (no task) |

## 14. Explore and DashSpend

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| EXP-1 | An Explore hub: Where to Spend, ATMs, Staking (only with a CrowdNode account), Get Test Dash (testnet); first-time info | `W/ui/explore/ExploreScreen.kt:61`, `X/ui/explore/dialogs/ExploreDashInfoDialog.kt:32` | master | MP-09 (1.2); `P:IOS-095` | COVERED (1.2) |
| EXP-2 | The merchant/ATM database updates itself (Firebase Storage zip, checksum password, periodic) and About shows sync status | `X/ExploreSyncWorker.kt:46`, `X/repository/ExploreRepository.kt:25`, `W/ui/more/AboutScreen.kt:157` | master | MP-09 "DB sync (HTTPS + AES zip)"; D:52 (R12) "Explore without the Firebase SDK" | COVERED |
| EXP-3 | Merchants: Online / Nearby / All with map + list, FTS search, location permission states, km/miles | `X/ui/explore/SearchFragment.kt:89`, `ExploreMapFragment.kt:56`, `ExploreViewModel.kt:54` | master | MP-09; `P:IOS-097` | COVERED (1.2) |
| EXP-4 | ATMs: All / Buy / Sell / Buy & Sell with detail | `X/ui/explore/ExploreViewModel.kt:51` | master | MP-09; `P:IOS-098` | COVERED (1.2) |
| EXP-5 | Filters: spending options (Dash, CTX, PiggyCards), card type, sort (name/distance/discount), radius, state | `X/ui/explore/dialogs/FiltersDialog.kt:49` | master | MP-09; `P:IOS-099` | COVERED (1.2) |
| EXP-6 | Merchant/ATM details: contact, directions, all locations, Pay with Dash, Buy a gift card with a provider picker | `X/ui/explore/ItemDetails.kt:67` | master | MP-09; `P:IOS-100` | COVERED (1.2) |
| EXP-7 | On testnet, "Get Test Dash" opens the faucet | `X/ui/ExploreTestNetFragment.kt:38`, `Constants.java:109` | master (testnet) | `P:IOS-121` partial (web faucet shortcut) | COVERED |
| DS-1 | CTX: email OTP login, terms, session-expired, logout | `X/ui/dashspend/DashSpendUserAuthFragment.kt:50`, `X/repository/CTXSpendRepository.kt:41`, `dialogs/DashSpendTermsDialog.kt:33` | master | MP-10 (IOS-101), B1 CTX client id; ships disabled | COVERED (deferred on B1) |
| DS-2 | PiggyCards: email signup/login; blocked in RU and CU | `X/repository/PiggyCardsRepository.kt:53`, `X/ui/explore/ExploreViewModel.kt:256` | master | MP-10 (IOS-102) | COVERED (deferred) |
| DS-3 | Buy a CTX gift card: flexible/fixed amount, live discount, pay the BIP70 request | `X/ui/dashspend/PurchaseGiftCardFragment.kt:71` | master | MP-10 + MP-05 | COVERED (deferred) |
| DS-4 | Buy PiggyCards in single/multiple quantities with per-merchant inventory | `X/ui/dashspend/PurchaseGiftCardFragmentV2.kt:73` | master | MP-10 | COVERED (deferred) |
| DS-5 | Gift card details: polling, number, PIN, barcode, how-to-use, balance check, support | `X/ui/dashspend/dialogs/GiftCardDetailsDialog.kt:107`, `GiftCardDetailsViewModel.kt:29`, `X/data/explore/GiftCardDao.kt:31` | master | `P:IOS-103` | COVERED (deferred) |
| DS-6 | A multi-card order lists each card | `X/ui/dashspend/dialogs/GiftCardOrderDetailsDialog.kt:72` | master | `P:IOS-103` ("stored gift cards") | PARTIAL: order grouping not mentioned |
| DS-7 | A payment whose merchant reply was lost is recovered | commit `3b5079b97` (#1560) | master | E0-04 unknown-outcome model (generic) | PARTIAL: provider-specific recovery not planned |

## 15. Buy / Sell / Dash DEX

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| BS-1 | A Buy & Sell portal lists Topper, Uphold, Coinbase (link state + balance), Dash DEX | `W/ui/buy_sell/BuyAndSellScreen.kt:68`, `BuyAndSellViewModel.kt:269`, `BuyAndSellScreen.kt:201` (`SUPPORTS_SWAPKIT`) | master | MP-11 (IOS-087); R:259 | COVERED (per partner, B1) |
| BS-2 | Topper: buy with a signed URL, no account | `IU/api/TopperClient.kt:51` | master | MP-11, B1(4) | COVERED (deferred) |
| BS-3 | Uphold: OAuth, card balance, transfer to wallet with OTP, buy via Topper, disconnect | `IU/ui/UpholdPortalFragment.kt:50`, `UpholdOtpDialog.java:36`, `UpholdTransferActivity.kt:55` | master | MP-11, B1(3) | COVERED (deferred) |
| BS-4 | Coinbase: OAuth, balance, buy, convert, transfer both ways with 2FA | `W/ui/buy_sell/IntegrationOverviewFragment.kt:45`, `IC/ui/CoinbaseBuyDashFragment.kt:50`, `CoinbaseConvertCryptoFragment.kt:60`, `TransferDashFragment.kt:61`, `EnterTwoFaCodeFragment.kt:50` | master | MP-11, B1(3); `P:IOS-090` | COVERED (deferred) |
| BS-5 | Dash DEX sell: pick coin, enter address (parsers for BTC, ETH, SOL, TON, TRX, XRP, ADA, NEAR, SUI, Starknet, Zcash, RUNE, XRD...), or "paste from linked exchange"; preview with fees; send with memo | `IM/ui/MayaPortalFragment.kt:34`, `MayaCryptoCurrencyPickerFragment.kt:38`, `MayaAddressInputFragment.kt:61`, `MayaConvertCryptoFragment.kt:73`, `MayaConversionPreviewFragment.kt:65`, `MayaConvertResultFragment.kt:55`, `IM/payments/parsers/` (26 files) | master (mainnet) | MP-11 "SwapKit"; `P:IOS-092` not started (M5); B1(5) SwapKit key | COVERED (deferred); the multi-chain address validation workload is not sized separately |
| BS-6 | Dash DEX buy: coin, amount, refund address, deposit QR with 10-minute expiry | `IM/ui/DEXEnterAmountFragment.kt:44`, `DEXRefundAddressFragment.kt:51`, `DEXReceiveFragment.kt:47` | master | `P:IOS-093` | COVERED (deferred) |
| BS-7 | Swap orders persist, are tracked, notify on result and link the history tx to the provider explorer | `IM/api/SwapTrackingService.kt:54`, `IM/data/SwapOrderDao.kt:32` | master | `P:IOS-094` not started | COVERED (deferred) |

## 16. CrowdNode (wind-down)

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| CN-1 | I can still create a new CrowdNode account; linking an existing one and deposits are hidden ("functionality is limited") | `IN/ui/entry_point/EntryPointFragment.kt:36,57-58`, `NewAccountFragment.kt:49`, `IN/ui/portal/PortalFragment.kt:137` | master | MP-13 "withdraw-only, 1.2, or drop" (R:261) | COVERED/PARTIAL: new-account creation is **not** withdraw-only on Android |
| CN-2 | I withdraw my CrowdNode balance with limits explained (signed message) | `IN/ui/portal/PortalFragment.kt:55`, `TransferFragment.kt:65`, `IN/ui/dialogs/WithdrawalLimitsInfoDialog.kt:35` | master | MP-13, `P:IOS-091` | COVERED (1.2 or drop) |
| CN-3 | A withdrawal reminder banner/dialog nags while a CrowdNode balance remains | `W/ui/explore/ExploreEntryViewModel.kt:55`, `W/ui/staking/CrowdNodeWithdrawalReminderDialog.kt` | master | MP-13 | COVERED |
| CN-4 | CrowdNode txs show grouped, with the CrowdNode icon | `W/ui/transactions/CrowdNodeTxResourceMapper.kt:28` | master | `P:IOS-030` (M5) | COVERED |

## 17. Tools, settings, about, support, system

| ID | Story | Android evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| TOOL-1 | Address book: my receive addresses with labels, saved payees (add from clipboard/scan, edit, QR, copy, send to) | `W/ui/AddressBookActivity.java:58`, `SendingAddressesFragment.java:79`, `WalletAddressesFragment.java:72` | master | `P:QT-095..097` partial/done; U-05 address book | COVERED |
| TOOL-2 | Network monitor: Peers tab (address, version, height, ping) and Blocks tab (block info lists this wallet's txs) | `W/ui/NetworkMonitorActivity.java:37`, `PeerListFragment.java:68`, `BlockListFragment.java:76`, `BlockInfoActivity.kt:33` | master (rewritten in Kotlin on S1: `S1:W/ui/NetworkMonitorActivity.kt:56`) | U-05 Peers; `P:QT-024` partial; desktop `CLAUDE.md` leaves "debug and peer tools beyond the existing console" to the node | PARTIAL: peers yes, block list no (acceptable) |
| TOOL-3 | Extended public key with QR/copy/share | `W/ui/compose_views/ExtendedPublicKeyDialog.kt:32` | master | `P:IOS-111` done | COVERED |
| TOOL-4 | Masternode keys: owner / voting / operator BLS / platform ed25519, each by index with WIF/hex/base64, usage, "+" add key | `W/ui/more/masternode_keys/MasternodeKeysFragment.kt:36`, `MasternodeKeyChainFragment.kt:43`, `MasternodeKeysViewModel.kt:190` | master | `P:IOS-083` partial (M3 engine/view model done); MG-10 UI | COVERED (1.1) |
| TOOL-5 | Rescan blockchain, optionally choosing a wallet creation date | `W/ui/SettingsFragment.kt:103`, `W/ui/more/RescanBlockchainDialogFragment.kt:66`, `WalletCreationDatePickerDialog.kt:51` | master | `P:IOS-113` partial (Repair: rescan, birth height) | COVERED |
| TOOL-6 | Private key import, CSV export, ZenLedger, Connections, Credits live in Tools | `W/ui/more/ToolsScreen.kt:155-225` | master | see RECV-3, TAX-2, TAX-3, CONN-1, PLT-2 | (rows above) |
| SET-1 | Local currency: searchable list; rates from several providers | `C/ui/exchange_rates/ExchangeRatesDialog.kt:46`, `W/rates/ExchangeRatesRepository.kt:31` | master | MP-01 (IOS-104 partial), U-04 "not yet (MP-01)" | COVERED (MP-01 is cuttable, `R:386`) |
| SET-2 | Settings > Notifications opens OS settings | `W/ui/SettingsFragment.kt:108` | master | `P:IOS-105` partial | COVERED |
| SET-3 | Dark mode follows the system theme (no in-app toggle) | `common/src/main/res/values-night/styles.xml`, `values-night/colors.xml`, commit `b6d29b233` (#1480, 2026-07-27); no `setDefaultNightMode` anywhere | master | UX-SPEC light and dark; `P:IOS-119` partial (tokens) | COVERED. `android.md:43,301` ("light only") wrong |
| SET-4 | There is no in-app language picker; 50 locales via system language | `wallet/res/values-*` (no language row in `W/ui/more/SettingsScreen.kt`) | master | MP-14 (Transifex, 1.0 infrastructure); U-04 "language (MP-14)" | COVERED |
| SET-5 | About: version, DashJ/Platform versions, network, Explore sync status + Update, rate the app, FCM token, GitHub, license | `W/ui/more/AboutScreen.kt:47,75,86,157` | master | `P:IOS-107` partial | COVERED |
| SET-6 | Contact support: describe the problem, attach app info, device info, logs, stack traces, wallet dump, via a mail chooser | `W/ui/more/ContactSupportDialogFragment.kt:45`, `ContactSupportViewModel.kt:81` | master | `P:IOS-112` partial (log export zip; no support email); U-04 "Support, GitHub, log export: not yet" | PARTIAL |
| SET-7 | Inactivity nudge: "you have funds, back up / open the app" with Dismiss forever | `W/service/BootstrapReceiver.java:116` | master | `P:IOS-122` (post-1.0 decide), `P:IOS-005` | PARTIAL (post-1.0) |
| SET-8 | Push messages (FCM) with open-URL action | `W/ui/notifications/PushMessagingService.kt:31` | master | `P:IOS-122` "announcement channel (decide)" post-1.0 | PARTIAL (post-1.0) |
| SYS-1 | Background sync service with an ongoing notification (peers connected, paused) | `W/service/BlockchainServiceImpl.kt:190` | master | tray/menu-bar `P:QT-028/029`, `IOS-117` | N/A (mobile process model) |

## 18. Other branch-only stories (not already listed)

| ID | Story | Evidence | Flag | Desktop mapping | Gap |
|---|---|---|---|---|---|
| BR-1 | CoinJoin is gone from the app; old mixed funds stay spendable | `83ce13ca1 feat!: remove CoinJoin mixing`; `S1:docs/kotlin-sdk-migration-plan.md:8-12` | branch-only (S1) | Desktop keeps CoinJoin (U-06, M3) | PARTIAL: scope question, see Top gaps #3 |
| BR-2 | After the upgrade a one-time notice says a full sync is needed (balance stays at last known) | `S1:W/ui/cutover/CutoverSyncNoticeDialogFragment.kt:49`, strings `cutover_sync_notice_*` | branch-only | no legacy engine on desktop | N/A |
| BR-3 | A "degraded wallet" entry and "bind pending" dialogs when the SDK wallet isn't ready | `S1:W/ui/DegradedWalletEntry.kt:8`, `ui/cutover/SdkBindPendingDialogFragment.kt:55` | branch-only | E0-05 bring-up states | N/A |
| BR-4 | An ambiguous send/swap/Coinbase transfer ends in a terminal "outcome unknown, do not retry" state | `S1:W/ui/send/SendFailureMessageMapping.kt:83`; branch `fix/sdk-ambiguous-send-no-retry` (`31bff724f`, `2450bf8f6`) | branch-only | M1 unknown-outcome rules; E0-04 dispatch journal | COVERED (desktop is stricter) |
| BR-5 | I can turn off usage analytics; amounts are no longer logged | `feat/analytics-opt-out`: `70a07f6ef` (MO-1065) | branch-only | desktop has no analytics SDK in plan (grep: no telemetry task) | N/A |
| BR-6 | If registration produced an identity but a name failed or was lost, I can retry registering a replacement name | `fix/sdk-username-replacement-retry`: `0daa13dd8`, `a42967d99`, `6efe067fb` | branch-only | DP1-02 "recover, discard, Finish registration"; DP1-03 | PARTIAL: replacement-name retry for an identity whose name failed is not explicit in DP1-02 |
| BR-7 | A devnet flavor ("Sakura") exists for Platform devnet testing | `f550a56c5`; `docs/sakura-devnet.md` | branch-only | `P:IOS-106` partial (devnet by flag), T-01 dashmate devnet | COVERED |
| BR-8 | Shielded home-total reconciliation hardening (cache per wallet, fail closed on unresolved spends) | `feat/home-balance-include-shielded`, `fix/shielded-balance-reconcile` | branch-only | X2 should inherit the lessons (spend settles only when the note store reflects it) | NOTE (design input, not a story) |
| BR-9 | Restore of a CoinJoin-heavy wallet must scan past 30 CoinJoin addresses or funds vanish (found live) | `S1:docs/kotlin-sdk-migration-plan.md:444-451` (key-wallet `DEFAULT_COINJOIN_GAP_LIMIT` 30 vs dashj 100; fixed to 100 in platform#4074 vendored patch) | branch-only (finding) | `DESIGN-opus.md:450` sets gap 1000 only for **imported-from-Core** wallets; no statement for mnemonic restores or the pin's CoinJoin gap | PARTIAL: verify the desktop pin (rust-dashcore `40268cc0`) restores Android CoinJoin accounts beyond 30 addresses |

## 19. Where `docs/research/dashpay/android.md` is stale or wrong

Checked against `origin/master` unless noted. "android.md:N" in this document always means the desktop file `dashwallet-desktop/docs/research/dashpay/android.md` (not this inventory).

| # | android.md says | Reality | Evidence |
|---|---|---|---|
| 1 | `:43`, `:301`: "Theme: light only; night colours commented out" | Dark mode shipped 2026-07-27, 72 days before the snapshot date. It follows the system theme; there is no in-app toggle. | `b6d29b233 feat: support dark mode (#1480)`; `common/src/main/res/values-night/{styles,colors}.xml`; follow-ups `#1660` etc. |
| 2 | `:56`: restore "validates against **one** BIP39 wordlist (English by default)" | It tries all 9 bundled lists and switches to the one that matches. 12/24-word limit is right. | `W/util/MnemonicCodeExt.kt:63,70-90`; `wallet/assets/bip39-wordlists/` (chinese x2, czech, english, french, italian, japanese, korean, spanish) |
| 3 | `:168`, `:191`, `:164`: contested name / contested invite cost 0.25 DASH | 0.15 DASH once Platform protocol version is 14 or higher (0.25 before); invite fee dialog uses the same object. | `W/service/platform/ContestedUsernameFees.kt:39-47`; `W/ui/invite/InvitationFeeDialogFragment.kt:42,67`; commit `7b2e23188` (#1583) |
| 4 | `:39`: "no Shielded/Orchard pool, no Platform addresses, no advanced mode, no internal transfer" | True for master only. `feat/kotlin-sdk-phase-1` (12.0.0) has a shielded pool, Internal transfer, shielded sends, shielded-funded usernames and invites, and no CoinJoin. Platform addresses: still none. The doc never looked at branches. | section 9; `S1:DashPayConfig.kt:441-469,852-861` |
| 5 | `:309`: CoinJoin "keep it ... Android is the living reference" | S1 removes mixing and migrates mixed coins to Shielded or Dash Wallet. | `83ce13ca1`; `S1:W/ui/migration/MixedFundsMigrationDialogFragment.kt:78` |
| 6 | `:202`: "No QR 'My code' or scan-a-user flow" | Exists on an unmerged branch, wire-compatible with iOS `dashpay://user`. | `feature/dashpay-user-qr` `86661e3dc`, `062489550` |
| 7 | `:215-217`: DashConnect has a second `dash-st:` QR; "no token-purchase sheet on Android" | v2 removed the second QR and added token-purchase approval, funding limits, expiry, `dashpay://connect`. Still testnet/devnet only. | `dd98e014f`, `14f0e045e`, `ab56974b7`, `connect:wallet/res/values/strings_app_connect.xml` |
| 8 | `:5`: snapshot `a8de1f0794` (2026-10-07) | master is `472e86090`; 4 commits later; none change a story except #1583 and dark-mode fixes. | `git log a8de1f079..origin/master` |
| 9 | `:36`: "no runtime network switch; flavors prod / testnet / staging / devnet (Ouzo)" | A "Sakura" devnet flavor is on branches. Minor. | `f550a56c5`, `docs/sakura-devnet.md` |
| 10 | `:309` says CoinJoin "M3 already built it" | Desktop's own `parity.md:108-117` lists QT-041..050 as **not started**. This is an inconsistency inside the desktop docs, not about Android. | `docs/parity.md:108-117` |

Two items in `android.md` I could not reproduce quickly and did not rely on: the exact "Contacts and Explore tabs are hidden until a username exists" gating (`:40`), and the Maya-native row being off (`Constants.java:69` `SUPPORTS_MAYA_NATIVE = false` is consistent with it).

## 20. Android-only stories (not on iOS) the desktop should weigh

"Android-only" here means: not in the iOS checklist in `docs/parity.md`. Plan status in the last column.

| Story | Android | iOS | Plan today | Recommendation |
|---|---|---|---|---|
| Invitation creation, share, history, tag, preview image | master | removed (`P:IOS-078`) | X3, 1.1 (DEC-15) | Covered. Add shielded-funded invites if X2 lands (SH-13) |
| Tx metadata saved to Platform (categories, notes, fiat, gift-card data; restore on another device) | master | none | F22 "post-1.0", no task | Needs an owner. Highest-value Android-only story for a desktop that restores the same seed |
| DashConnect incl. v2 and token approvals | master (testnet) + branches | `P:IOS-085/086` (test networks) | F24 post-1.0, no task | iOS has it too, so parity needs it eventually; add a placeholder task |
| Import/sweep private key, BIP38 | master | removed (`P:IOS-056`) | MP-07, 1.2 | Covered |
| Address book | master | none | QT-095..097, U-05 | Covered |
| Network monitor Blocks tab | master | none | Peers only | Skip (peer/node tools belong to dash-qt) |
| Masternode keys viewer with add-key | master | `P:IOS-083` | MG-10 | Covered |
| Username voting: Block ("lock") vote, quick vote, import WIF key | master | `P:IOS-079` (approve/abstain/lock) | MG-07/MG-11, optional 1.1 | Covered; optional |
| Instant (secondary) username during a contested vote | master | optional temporary name | DP1-03 temporary-name policy | Covered by the iOS model; confirm replace-on-win |
| Verify-identity link (`identityVerify`) | master | none | excluded D:1108,1131 | Keep excluded |
| Frequent contacts strip, contact sort, contact activity filter | master | none | DP3-02, DP2-07, F12 | Covered |
| Google Drive avatar upload | master | none | excluded D:1131 | Keep excluded |
| `dashwallet:` inter-app pay/address/xpub protocol with `payack` | master | `dashwallet://` parse | partial; address callbacks excluded D:1131 | Decide whether pay-with-callback is wanted |
| Shielded-funded invitations and usernames (denomination-bound 0.1 / 0.3) | S1 | usernames yes (`P:IOS-067`), invites removed | X2 covers usernames only | Add invites to X2/X3 (SH-13) |
| Dash DEX | master | **also on iOS** (`P:IOS-092/093`) | MP-11 | Not Android-only. The brief's example is wrong on this point |
| Shielded pool | S1 | **also on iOS** (`P:IOS-059..062`) | X2, 1.1 | Not Android-only |

## 21. Top gaps, ranked by user impact

Impact = how many same-seed users see a wrong or missing result on desktop 1.0/1.1, then how hard it is to fix later.

| # | Gap | Evidence (Android) | Desktop plan today | Suggested move |
|---|---|---|---|---|
| 1 | **Shielded balances are invisible on desktop 1.0 although Android 12.0.0 turns shielded ON for every user.** The Android flag set is seeded ON in all variants including the prodRelease store build, so after the update every Android user has a Shielded card, a shielded address and a shielded send. | `S1:DashPayConfig.kt:441-469,852-861`; `S1:MoreFragment.kt:445`; `S1:ShieldedBalanceActivity.kt:68` | X2 is 1.1; 1.0 only says "not shown yet" (`D:610`); rationale "Android has no shielded pool at all" is wrong for S1 | Re-decide X2 timing. E0-12 measured the feature at +6.6 % CPU work and +0.23 MB stripped, 3.5 s prover warm-up (`docs/research/shielded-cost.md`), so the "unknown cost" argument is gone. At minimum add a read-only shielded balance to X1a |
| 2 | **Home total differs from the phone.** Android's Home header is Core + Shielded and holds in a "syncing" state until the shielded part is known. Desktop 1.0 shows Core (+ Platform line from X1a), and X1a is on the 1.0 cut list. | `S1:MainViewModel.kt:517`, `S1:HeaderBalanceFragment.kt:57-76` | `P:IOS-019` Core only; `R:382` cut order #3 "X1a (keep the shielded notice)" | Make the Home total and breakdown card (IOS-021/MP-02) shape-ready for a shielded term; do not cut X1a without a replacement notice that names the amount |
| 3 | **CoinJoin direction.** Both mobile wallets have dropped mixing (iOS `P:IOS-058`; Android S1 removed it and offers "Move your mixed coins" to Shielded or Dash Wallet). Desktop plans a full mixing client (QT-041..050 all "not started", U-06) as "build first", while the un-mix paths are 1.1 (Shielded, X2) and 1.2 (Core, MP-08). | `83ce13ca1`; `S1:MixedFundsMigrationDialogFragment.kt:78`; `S1:strings-dashpay.xml:786-826` | `R:130,256`; `P:108-117`; android.md:309 even claims "M3 already built it" | Product call: keep mixing, or demote to "move mixed coins" only. Either way a restored Android wallet with CoinJoin-account funds needs a 1.0 path to spend/un-mix them |
| 4 | **Tx metadata on Platform (categories, notes, fiat, gift-card data, restore on another device) has no owner.** It ships on Android master today. | `W/ui/more/TransactionMetadataSettingsScreen.kt:59`, `PlatformSyncService.kt:253,1204`, `Constants.java:68` | `D:1125` F22 "post-1.0", no task, no parity row | Create a task ID so it is tracked. Note Android's own SDK migration is blocked on encrypted-document create/decrypt in the SDK (`S1:docs/kotlin-sdk-migration-plan.md:465`) |
| 5 | **Restore of CoinJoin-heavy Android wallets can silently miss funds.** key-wallet's default CoinJoin gap is 30; Android found live that this skips mixing history and mis-debits spends; fixed to 100 in a vendored patch. | `S1:docs/kotlin-sdk-migration-plan.md:444-451` | `DESIGN-opus.md:450` gap 1000 only for imported-from-Core wallets | Add a restore test with a >30-address CoinJoin chain at desktop's pin (rust-dashcore `40268cc0`); set the gap for mnemonic restores too |
| 6 | **Shielded-funded invitations are in neither X2 nor X3.** Android lets a user fund an invitation from Shielded (with an overage top-up) and pay usernames from Shielded at denomination-bound 0.1 / 0.3. | `S1:InviteShieldedFundingViewModel.kt:65`, `S1:SdkShieldedInviteCreation.kt:46`, `S1:UsernamePaymentSheets.kt:100` | X2 "shielded registration funding"; X3 silent on funding source | Add "shielded funding" to X3's acceptance when X2 lands |
| 7 | **Connect v2 (96-commit UI rebuild, token-purchase approvals, `dashpay://connect`) has no task.** Android keeps it testnet/devnet only, but both mobile wallets have it. | `connect:strings_app_connect.xml`; `ab56974b7`; `14f0e045e`; `Constants.java:104` | `D:1127` F24 "post-1.0"; `P:IOS-085` partial, `IOS-086` not started; no ROADMAP row | Add a placeholder task and link the Connect v2 spec so it is not lost |
| 8 | **Platform addresses (iOS-only) are cuttable in 1.0 and thin in 1.1.** Android adds no evidence either way, so the underplanning claim rests on iOS parity: X1a is M+S and on the cut list; X1b is 2 x L; stuck asset-lock rebroadcast (IOS-033) is "not started". | Android: none (section 2). iOS: `P:IOS-033,045,047,053,059-064` | `R:215-216,382` | Keep X1a out of the cut list if iOS-restored users are a target; add IOS-033 to DP1-02's recovery tasks |
| 9 | **Tax categories, tax CSV and ZenLedger are 1.2/B1.** Android ships all three. Plain history CSV is the only 1.0 export. | `TransactionDetailsDialogFragment.kt:206`, `ExportCSVDialog.kt:65`, `ZenLedgerDialog.kt:63` | MP-04 1.2, MP-12 B1 | Consider pulling MP-04 into 1.1; desktop users are the ones who file taxes |
| 10 | **41 of the 128 COVERED rows are covered only after 1.0** (1.1, 1.2, or blocked on B1). The whole commerce side (Explore, DashSpend, Buy & Sell, Dash DEX, CrowdNode) is 1.2/B1. | rows tagged (1.1)/(1.2)/(deferred) | `R:257-261,336` | State the Android-parity promise per release in `parity.md`'s header so "covered" is not read as "shipping" |
| 11 | **Private-key import / paper-wallet sweep incl. BIP38** is Android-only and 1.2. | `SweepWalletFragment.java:102`, `SweepWalletPasswordDialog.kt:31` | MP-07 1.2; `P:IOS-056` "decide" | Fine as is; note dash-qt covers it meanwhile |
| 12 | **Dash DEX address validation for 20+ chains** (26 parser files) is not sized in MP-11's "4 x L". | `IM/payments/parsers/` | `R:259` | Size separately or reuse the SwapKit token/address rules |
| 13 | **Lock-screen Quick Receive / Scan to Pay** exist on Android; desktop lists both as missing and has no plan outside the macOS menu-bar companion. | `LockScreenActivity.kt:317,320`; `QuickReceiveActivity.kt:31` | `P:IOS-013`, `P:IOS-117` | Add a tray/lock-screen receive on Windows/Linux (W-02/U-01) |
| 14 | **Replacement-name retry after a failed/lost registration name** (branch) is not explicit in DP1-02/03. | `0daa13dd8`, `a42967d99`, `6efe067fb` | DP1-02 "recover, discard" | Add the case to DP1-02's kill matrix |
| 15 | **Smallest claimable invitation (0.003 DASH) and the empty-credits path** (branch) are not in DP5-02. | `dcdd3a96a`, `7faf7d391` | DP5-02 | Copy the thresholds into DP5-02 acceptance |
| 16 | **CrowdNode on Android still lets users create new accounts**; MP-13 assumes withdraw-only. | `IN/ui/entry_point/NewAccountFragment.kt:49`, `PortalFragment.kt:137` | `R:261` "withdraw-only ... or drop" | Likely "drop"; decide and record |
| 17 | **Backup nag and capture protection are macOS-only.** Android has both everywhere. | `LockScreenActivity.kt:217`, `ShowSeedFragment.kt:75` | `P:IOS-005/006` partial | Close for Tauri on Windows/Linux |
| 18 | **Support: crash report on next start, attach logs + wallet dump, mail chooser.** Desktop has log export only. | `CrashReporter.java:70`, `ContactSupportViewModel.kt:81` | `P:IOS-112` partial; U-04 "not yet" | Add a "report an issue" bundle; careful with the wallet dump (secrets) |
| 19 | **Contested-name voting UX specifics** (filters by type/duplicates/link, "one vote left" guard, Block vote, quick vote) are only partly specified; MG-07 is optional. | `UsernameRequestFilterDialog.kt:33`, `OneVoteLeftDialogFragment.kt:29`, `UsernameVote.kt:41-42` | MG-07/MG-11, `P:IOS-079` | Fine for an optional 1.1 task; use Android's filters as the spec |
| 20 | **Docs hygiene**: `android.md` is stale in 10 places (section 19) and `D:610,1106` repeat its "Android has no shielded pool" claim. | section 19 | `D:610,1106`; `android.md:39,309` | Update the three docs together; add an "Android branches" subsection |

## 22. Counts

Counted by script over the story tables (rows with an ID like `ON-1`). Excluded from counts as duplicates/notes: TOOL-6, CON-13, ID-10, CON-14, PLT-1 (no Android story), BR-8 (design note).

| Measure | Count |
|---|---|
| Stories inventoried | 186 (192 table rows) |
| COVERED (a task or parity row delivers it) | 128, of which about 41 only after 1.0 (1.1, 1.2, or blocked on B1) |
| PARTIAL | 33 |
| MISSING (not in plan) | 11 (META-1..4, SH-13, CONN-1..5, CONN-7) |
| EXCLUDED (plan says no) | 1 (ID-8 verify link); two more partial exclusions inside PARTIAL rows (SEND-12 callbacks, CON-10 Google Drive) |
| N/A (mobile/legacy) | 13 |
| On master / branch-only or branch flag-gated | 155 / 31 (master rows include the 3 testnet-only Connect v1 rows) |

By area: Onboarding 12, Security 8, Home 15, History+Tax+Metadata 21, Send+Receive 18, CoinJoin 6, Platform+Shielded 18, Identity+Voting 13, Invitations 9, Contacts 12, Connect 7, Explore+DashSpend 14, Buy/Sell 7, CrowdNode 4, Tools+Settings+System 14, Branch-only extras 8 (sum 186).
