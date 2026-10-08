# dashwallet-iOS — complete user-facing feature inventory (verified 2026-10-07)

Research input for "DashPay on desktop" (dashwallet-desktop). It re-verifies and extends
`docs/research/03-ios-features.md` (written 2026-10-05) and the IOS-001…123 checklist in `docs/parity.md`.

- **Source:** `/Users/pasta/workspace/dashwallet-ios`, branch `develop`, HEAD `3df12fc89` (2026-10-06,
  "fix(release): read the Platform branch from its default branch (#1186)"). Clean tree.
- **Delta since research 03:** research 03 read `7c0064d6b` (2026-10-02 merge). Only 9 commits since:
  #1172 (Identity **Max** withdrawal now holds back a fee reserve; masternode withdrawal reserve derived from it),
  #1179 (launch no longer holds on DashSync mnemonics orphaned by an 8.x reset), #1187 (accessibility identifiers
  on the Send address field), #1186 (CI reads Platform default branch). No screens were added or removed.
- **Method:** walked every menu view model, every SwiftUI `*Screen/*Sheet/*Dialog` and every `*ViewController`
  (list in §12), extracted `NSLocalizedString` keys per screen, read the URL routing (`AppDelegate.m`,
  `DWAppRootViewController.m`, `DWURLParser.m`), `Info.plist` / entitlements, the notification producers and the
  SwiftDashSDK adapter layer. Each SDK call named below was checked to exist in the desktop's Platform pin
  (`dashwallet-desktop-deps/platform` @ `bc321362b9`, `packages/swift-sdk`) — all of them do (§10).
- **Path prefixes:** `S/` = `DashWallet/Sources/`; `I/` = `DashWallet/Sources/Infrastructure/SwiftDashSDK/`;
  `DP/` = legacy `DashPay/Presentation/` (ObjC, dashpay target).
- **Verdict on research 03:** accurate. Every claim spot-checked (menus, shortcuts, gating, costs, limits, hosts,
  removed features, notification producers, URL schemes) still holds. Corrections and additions are in §1 and
  in the new rows IOS-124…IOS-140.

**Scope tags** (last column of every table), matching the desktop `CLAUDE.md` "Product scope":

| Tag | Meaning |
|---|---|
| **DP** | DashPay core: identity, username, contacts, profile, contact payments. Must ship for "DashPay on desktop". |
| **M** | Mobile wallet parity: build it. |
| **M-int** | Mobile parity but a third-party integration (partner API keys, OAuth client, ToS). Needs a per-partner decision. |
| **CORE** | Dash Core scope per desktop `CLAUDE.md` (masternode/ProTx/governance). Do not build UI; PARKED. |
| **DEC** | Product decision needed (removed on iOS, or iOS-specific behaviour with no obvious desktop answer). |
| **N/A** | iOS-only or dead code. Do not port. |
| **DEV** | Developer/diagnostic surface. Ship behind a developer toggle. |

---

## 1. What changed versus research 03 (corrections and additions)

Corrections (small; research 03 is otherwise correct):

1. **Profile sheet does not link to the Username Marketplace.** The marketplace is reached from Explore →
   Username Marketplace and from More → Identities (`UsernameMarketplaceScreen(` constructed only in
   `S/UI/Explore Dash/ExploreMenuScreen.swift` and `S/UI/Menu/Security/Wallets/IdentitiesScreen.swift`). The
   create-username flow uses `UsernameMarketplaceService` for "buy directly if listed" but does not open the screen.
2. **ShieldedRecoverySheet path** is `S/UI/Payments/InternalTransfer/Shielded/ShieldedRecoverySheet.swift` (not under Home).
3. **Shortcut list:** `sendToContact` ("Send to Contact") and `createUsername` ("Join Evolution") exist in
   `ShortcutActionType` but are **not** in `customizableActions`, so users can't place them. `payWithNFC`,
   `localCurrency`, `importPrivateKey`, `switchToTestnet/Mainnet`, `reportAnIssue` are legacy and also not offered
   (`importPrivateKey` and `reportAnIssue` are `break`).
4. **The `MOCK_DASHPAY = YES` constant is still live and has a user-visible effect** beyond "Buy credits": every
   profile save from the More menu subtracts 0.25 from a fake `BuyCreditsModel.currentCredits` and, after the 3rd/4th
   save, shows a fake "Your credit balance is low / fully depleted" warning that opens the mock `BuyCreditsViewController`
   (`S/UI/Menu/Main/MainMenuViewController.swift:666-680`). `DWDashPayModel.username` also short-circuits to
   `DWGlobalOptions.dashpayUsername` under the flag. **Bug; do not port.**
5. **Sync Info → Core Sync Status** has more actions than listed: **Finish Transfers** (bulk-complete unfinished
   asset-lock transfers), **Clear & resync** (clear chain data), **Drop Unconfirmed & Rescan**, Rescan Filters
   (from wallet creation / from height / full), **Edit birth height** (`S/UI/Menu/SyncInfo/SwiftDashSDKSPVStatusScreen.swift`).
6. **`dashid` URL scheme is registered but unhandled**: `DWURLParser.canHandleURL` accepts only `dash`, `dashwallet`,
   `pay` and DashConnect, so `dashid:` shows "Not a Dash URL". `dashpay://user?…` contact links are likewise only
   understood **inside** the Add-Contact QR scanner, not as an OS deep link. Only `dashpay://invite…` and the
   `invitations.dashpay.io` universal link are routed from the OS.
7. **Devnet** is internal-build only (`WalletEnvironment.isDevnetAvailable`); the "Devnet Settings" row is absent in shipping builds.
8. **Unused constants:** `DWDP_THUMBNAIL_SERVER = http://54.74.4.114`, `DWDP_MIN_BALANCE_TO_CREATE_INVITE` (invites removed).

Additions (new IDs, details in the tables):

- **IOS-124** Same-seed identity + DashPay contact recovery on restore/import (automatic, no UI). Critical for DashPay restore.
- **IOS-125** Inter-app URL actions: `dashwallet://request=address&sender=…` (authorised address hand-off with
  callback), `dashwallet://pay=…&sender=…`, `dashwallet://scanqr`; deferral until unlock; "Unsupported URL" / "Not a Dash URL" alerts.
- **IOS-126** Multiple usernames per identity: register an additional available name on an existing identity, list of
  DPNS names, "Get another username"; finish registration from an identity that already has credits.
- **IOS-127** Contact-request recipient eligibility pre-check ("Can't receive contact requests").
- **IOS-128** Contact alias / note / hide published as an encrypted Platform `contactInfo` document (DIP-15), deferred until ≥ 2 contacts.
- **IOS-129** DashPay intro / FAQ sheet ("What is DashPay?", privacy comparisons, "Coming soon").
- **IOS-130** QR decoding from an image on the clipboard (send address paste).
- **IOS-131** Identity credit top-up presets and the two-step Shielded → identity route.
- **IOS-132** Identity withdrawal **Max** with fee reserve (#1172) and the "insufficient credits" refusal mapping.
- **IOS-133** Send-to-username is **contacts-only** (picker + "Search for a User" to add first); paying a non-contact by username is "coming soon".
- **IOS-134** Contested-username rules explainer (Voting Info) and contested-name confirmation sheet.
- **IOS-135** Username request status (More-menu header entry) incl. temporary username registration.
- **IOS-136** DashPay incoming-payment attribution in history and tx details (contact avatar/name; open contact profile from tx).
- **IOS-137** My DashPay QR (`dashpay://user?id=…&username=…`) + scan a contact QR with on-chain verification.
- **IOS-138** Wallet switch shortcut dialog (1 → confirm, 2–5 → picker, >5 → Wallets screen).
- **IOS-139** Sync Info developer actions: Finish Transfers, Clear & resync (see correction 5).
- **IOS-140** Mock "Buy credits" / fake credit-depletion warning (do not port; listed so it is consciously excluded).

---

## 2. Feature gating (verified; unchanged)

| Gate | Where | Effect |
|---|---|---|
| `#if DASHPAY` + dashpay-target membership | `DashWallet.xcodeproj/project.pbxproj`; `DashPay/Presentation/**`, `S/UI/DashPay/**`, `I/{Identity,Contacts,Invitations}` | DashPay UI. Desktop: always on (one product). |
| Identity present (`DWCurrentUserIdentityInfo.hasIdentity`) | `S/UI/Main/MainTabbarController.swift` | Tabs become Home \| Contacts \| Payments \| Explore \| More (else Home \| Payments \| More). |
| Advanced mode (`DWGlobalOptions.advancedModeEnabled`, auto-on at first Platform balance unless user-managed) | `S/Models/DWGlobalOptions.m`, Settings toggle, `.advancedModeDidChange` | Platform balance/receive/send routes, identity routes in Internal Transfer, More → Wallets + Identities. Shielded is **not** gated. |
| `VotingPrefs.votingEnabled` (default true; Settings "Enable Voting", untranslated) | `S/Models/Voting/VotingPrefs.swift` | Governance → Voting row. |
| Network (`CURRENT_CHAIN_TYPE_KEY`, missing = mainnet) | `I/WalletEnvironment.swift` | Dash DEX mainnet only; DashConnect test networks only (mock on mainnet); faucet testnet only; partner sandboxes on testnet. |
| Geo | `S/UI/Buy Sell/BuySellPortalViewController.swift`, `GeoRestrictionService.swift` | Coinbase hidden for GB; PiggyCards blocked RU/CU (GPS or `ip-api.com`). |
| Partner key present | `SwapKitConstants.isConfigured`, plist-backed constants | Dash DEX card/shortcut only with a SwapKit key. |
| CrowdNode posture | iOS `CLAUDE.md:74` | "temporarily suspend/hide for the migration release"; Explore → Staking and the shortcut show only for existing signed-up accounts. |
| `MOCK_DASHPAY = YES` | `S/UI/DashPay/DWDashPayConstants.m` | Fake credits UX (IOS-140). Do not port. |

---

## 3. Onboarding, wallet lifecycle, security

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-001 | Intro carousel + demo mode | "Welcome / We Upgraded", "Pay with Ease", "More Control"; live demo mini-wallet on stubs | First launch | `S/UI/Onboarding/*` (`Stubs/*`, `Controllers/DWDemoAppRootViewController`) | — | — (stubs) | M |
| IOS-002 | Create wallet 12/24 words | Mnemonic persisted + read back **before** the wallet goes live | Setup → Create New Wallet → Backup info "Advanced" | `S/UI/Setup/SecureWallet/BackupInfo/BackupInfoViewController.swift`, `Seed/RecoveryPhraseLength.swift`, `I/SwiftDashSDKWalletCreator.swift`, `I/SwiftDashSDKHost.swift` (`MnemonicFirstWalletCreation`) | — | `Mnemonic.generate(wordCount:)`, `createOrImportWallet` | M |
| IOS-003 | Backup warnings + show phrase | Two warnings then phrase | Setup / Backup shortcut / reminder | `S/UI/Setup/SecureWallet/Seed/BackupSeedPhraseViewController.swift`, `DWPreviewSeedPhraseViewController.m` | — | — | M |
| IOS-004 | Verify phrase by word chips | Ordered shuffled chips → "Verified Successfully" | After IOS-003 | `.../Verify/DWVerifySeedPhraseModel.m`, `VerifiedSuccessfully/*` | — | — | M |
| IOS-005 | Backup reminder | 24 h after first balance change if unbacked; Backup shortcut slot | Home | `S/UI/Home/HomeViewController+BackupReminder.swift`, `DWHomeModel.m` | — | — | M |
| IOS-006 | Screenshot warning | Alert on screenshot while phrase shown | Phrase screens | `S/UI/Setup/ScreenshotWarning/*` | OS screenshot notification | — | DEC (desktop: no reliable signal; consider screen-capture exclusion) |
| IOS-007 | Restore from phrase | 12/15/18/21/24 words, 10 wordlists, NFKD normalise; per-word errors | Setup → Recover Wallet | `S/UI/Setup/RecoverWallet/*` | — | `Mnemonic.validate/cleanupPhrase/normalizePhrase`; birth height 200000 mainnet / 0 testnet | M |
| IOS-008 | Phrase repair | Find 1 wrong word in 12, or 1–2 missing words; Insight history probe; edit-distance fallback; cancellable | Recover screen "repair" | `I/SwiftDashSDKPhraseRepairer.swift`, `I/DamerauLevenshtein.swift`, `I/SwiftDashSDKInsightClient.swift`, `S/UI/Setup/RecoverWallet/PhraseRepair/*` | `POST insight.dash.org/insight-api/addrs/txs` | derivation m/44'/5'/0'/0/0 | M |
| IOS-009 | Reinstall wallet detection | "Wallets found on this device" → Keep / Delete All (typed phrase) | Launch | `S/UI/Setup/WalletRecovery/KeychainWalletRecoveryCoordinator.swift` | — | `WalletStorage` inventory | M (desktop: existing vault detection) |
| IOS-010 | Set / confirm 4-digit PIN | Intents create / change / set | Setup, Security | `S/UI/Setup/SetPin/*` | KC `pin` | — | M |
| IOS-011 | Biometric enrollment | Enable or skip; default 0.5 DASH spend allowance | Setup | `S/UI/Setup/BiometricAuth/*` | LocalAuthentication | — | M (Touch ID / Windows Hello; none on Linux) |
| IOS-012 | PIN lockout + secure time | 3 free, then 6^(n−3) min waits, disabled at 8; monotonic time ratchet fed by HTTPS Date | Lock screen | `S/Infrastructure/Authentication/PinStore.swift`, `SecureTimeService.swift`, `HTTPClient.swift` | HTTPS `Date` headers | — | M |
| IOS-013 | Lock screen | PIN pad, biometric login, **Quick Receive** (Transparent/Platform/Shielded), **Scan to Send**, Forgot PIN, countdown; URLs deferred until unlock | Launch / auto-lock | `S/UI/LockScreen/*` | — | receive-address readers | M |
| IOS-014 | Forgot PIN | Enter any matching stored phrase → new PIN; "Wipe All Wallets" at ≥ 6 failures | Lock screen | `S/UI/LockScreen/DWLockScreenViewController.m` | — | `anyStoredMnemonicMatches` | M |
| IOS-015 | Auto-lock / Auto Logout | Off or Immediately / 1 m / 5 m / 1 h / 24 h (default 60 s) | Security → Advanced Security | `S/UI/Menu/Security/Advanced Security/Model/DWBaseAdvancedSecurityModel.m:49` | — | — | M |
| IOS-016 | Spending confirmation + biometric limit | 0 / 0.1 / 0.5 / 1 / 5 DASH; security-level meter; Reset to Default | Security → Advanced Security | `.../Advanced Security/*` (`DWAdvancedSecurityModel.m:27,104`) | — | — | M |
| IOS-017 | One shared auth gate | Biometric → PIN modal with 120 s watchdog for every sensitive action | — | `S/Models/Transactions/WalletSendService.swift` (`AuthenticationGate`), `S/UI/Auth/PinPrompt*.swift`, `I/Identity/DWIdentityAuthorizer.swift` | — | — | M |
| IOS-018 | Wallet lifecycle overlay | Opening / migrating / switching network ("Switching to %@…", Retry, Switch Back) / switching / adding / removing / wiping; Help + Export Logs | Lifecycle ops | `S/UI/Main/WalletLifecycleOverlay.swift`, `WalletPreparationSupport.swift`, `I/WalletLifecycleTransitionState.swift`, `I/WalletPreparationFailure.swift` | `support@dash.org` | runtime lifecycle queue | M |
| — | DashSync → SwiftDashSDK key migration | Launch hold "Preparing your wallet…"; #1179 no longer holds on orphaned 8.x mnemonics | Launch | `I/SwiftDashSDKKeyMigrator.swift`, `DASHSYNC_KEY_MIGRATION.md` | — | — | N/A (desktop has no DashSync store; "open a dash-qt wallet" is the desktop analogue) |
| IOS-123 | Seed + PIN storage | iOS: mnemonic in keychain (`WhenUnlockedThisDeviceOnly`), not PIN-encrypted | — | SDK `WalletStorage`, KC service `org.dashfoundation.dash` | — | — | DEC (desktop dw-vault already decided passphrase vault) |
| IOS-124 | **Same-seed identity + DashPay recovery** (new) | On restore/import (and every runtime start until settled) the wallet discovers identities created by the same seed elsewhere, adopts the main identity, brings identity → contacts → DIP-15 contact accounts up **before** Core SPV so the first filter scan already watches contact payment addresses; `reconcile_dashpay_rescan` covers contacts that arrive later. Generated-on-device wallets get a short probe budget | Automatic after restore / at start | `I/Identity/DWCurrentUserIdentityInfo.swift` (`DWSameSeedIdentityRecoveryCoordinator`, `StartupIdentityRecoveryPolicy`), `I/Contacts/DashPayContactAddressReadiness.swift`, `I/PlatformAddressSyncCoordinator.swift`, `I/SwiftDashSDKWalletRuntime.swift` | — | `PlatformWalletManager.startWalletSubsystems`, identity discovery, `startDashPaySync`, `syncDpnsNames` | DP |

## 4. Home, balances, history, tax

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-019 | Balance hero | Core + Shielded (+ Platform in ADV, credits ÷ 1000); "Known balance" partial; "—" unknown; fiat line | Home | `S/UI/Home/Views/Home Balance View/HomeBalanceView.swift`, `BalanceModel.swift`, `I/HomeBalancePresentation.swift` | rates (§9) | `SwiftDashSDKWalletState.$balance`, Platform/shielded balance states | M |
| IOS-020 | Hide balance | Tap hero; persisted; one-time hint; Autohide option; long-press → currency picker | Home, Security | same + `DWGlobalOptions.balanceHidden` | — | — | M |
| IOS-021 | Balance breakdown + info sheets | Transparent / Platform / Shielded rows with pros/cons sheet | Home | `.../BalanceInfoSheet.swift`, `S/UI/Payments/Pay/ChainNetworkToggle.swift` | — | `ShieldedSyncMonitor` | M |
| IOS-022 | TESTNET / DEVNET badge | Badge + testnet logo; coin sound on balance rise | Home | `HomeView.swift`, `Resources/coinflip.aiff` | — | — | M |
| IOS-023 | Sync status | "Sync Failed"/retry, "Unable to connect", Syncing pill → per-phase dialog, peers, "Change peers" after 45 s stall | Home | `S/Application/Syncyng Activity Monitor/SyncingActivityMonitor.swift`, `S/UI/Home/Syncing Views/*` | — | `SwiftDashSDKSPVCoordinator`, `rotatePeers` | M |
| IOS-024 | Rate warnings | Stale > 30 min, failed, > 50 % move toasts | Home | `S/UI/Main/MainTabbarController.swift` | `rates.ctx.com` | — | M |
| IOS-025 | Shortcut bar | 4 slots; long-press to customise; customisable set: Buy & Sell, Explore, Spend, ATM, Receive, Send, Scan QR, Send to Address, Coinbase, Uphold, Topper, + Dash DEX (mainnet+key), CrowdNode (signed up), 1 tDash (testnet), Switch Wallet (>1), Nodes (evonode owner). Backup appears automatically when unbacked | Home | `S/UI/Home/Views/Shortcuts/*`, `Models/ShortcutAction.swift` (`customizableActions`), `S/UI/Home/HomeViewController+Shortcuts.swift` | per action | `TestnetFaucet` | M (Nodes = CORE) |
| IOS-026 | Time-skew dialog | Device clock vs NTP / HTTP Date | Home | `S/Utils/TimeUtils.swift` | `pool.ntp.org`, `www.dash.org`, `insight.dash.org` | — | M |
| — | Jailbreak warning | — | Home | `HomeViewController+JailbreakCheck.swift` | — | — | N/A |
| IOS-065 | Join DashPay banner | States: Join / Upgrade / Request your username / Finish registration / voting in progress / failed / blocked / given to someone else / shielded resting ("register privately around %@") / identity ready "use existing credits" | Home, More header | `S/UI/DashPay/Setup/CreateUsername/JoinDashPayView.swift`, `JoinDashPayViewModel.swift`, `S/UI/Home/Models/DWDPRegistrationStatus.m` | — | registration state, `fetchContestVoteState` | DP |
| IOS-027 | History by day + paging | Whole-day windows (~100 rows), "Loading more", "Date unknown" bucket | Home | `S/UI/Home/Views/HomeViewModel.swift`, `TransactionListDataItem.swift`, `S/Models/Tx/GroupedTransactions.swift` | — | SwiftData `PersistentTransaction/Txo/ShieldedActivity` | M |
| IOS-028 | History filters | All/Only; Sent, Received, Rewards*, Masternode*, Gift card, Shielded sent/received (*only if present) | Home | `S/UI/Home/Views/TransactionFilterDialog.swift` | — | — | M |
| IOS-029 | Tx rows | Metadata title, icon priority (merchant > metadata > gift card > mining > direction), route labels ("Transparent → Shielded" etc.), Locked / Pending / "Pending — tap to finish" | Home | `HomeView.swift`, DashUIKit `TransactionView` | merchant icons | — | M |
| IOS-136 | **DashPay attribution in history** (new) | Contact avatar + name on DIP-15 payments; tx details shows contact row → opens `ContactProfileSheet` | Home → tx | `HomeView.swift`, `S/UI/Tx/Details/Views/TxDetailContactViews.swift`, `I/Contacts/SwiftDashSDKContactsService.swift` (`DashPayPaymentTxLookup`, `info(forTxidHex:)`, `refreshPaymentsProjection`) | — | contact-payment projection over SwiftData | DP |
| IOS-030 | Grouped rows | CoinJoin mixing per day, CoinJoin withdrawals, CrowdNode group (→ `GroupedTransactionsScreen`) | Home | `S/Models/CoinJoin/*TxSet.swift`, `S/UI/CrowdNode/Tx Details/GroupedTransactionsScreen.swift` | — | — | M |
| IOS-031 | Tx details | Amount, route + lock status, sent-from/to, received-at, moved, registered-from, MN owner/provider/voting addresses, contact, fee (Insight fallback), date, tax category | History row | `S/UI/Tx/Details/TxDetailViewController.swift`, `Model/TxDetailModel.swift`, `HomeView.swift` (`TransactionDetailsSheet`) | Insight | — | M |
| IOS-032 | Tx details actions | Copy txid, Select block explorer (Insight / Blockchair), raw tx (inputs/outputs/script/special payload, copy hex), Maya / NEAR explorer | Tx details | `RawTransactionView.swift`, `BlockExplorerSelectionView.swift` | `insight.dash.org`, `blockchair.com`, `mayascan.org`, `explorer.near-intents.org` | — | M |
| IOS-033 | Rebroadcast / Complete Transfer | Stuck asset lock → "Funds locked — finishing transfer", Rebroadcast | Tx details | `I/AssetLockRecoveryService.swift`, `I/AssetLockProbeStore.swift` | — | `resumeTopUpWithAssetLock`, `resumeAssetLock`, `resumeIdentityWithAssetLock` | M |
| IOS-034 | Remove if Not on Network | Insight check → delete local rows → free inputs → rescan ≥ ~30 h; bulk drop & rescan | Tx details, Sync Info | `I/UnconfirmedTransactionRemover.swift` | Insight | SPV rescan | M |
| IOS-035 | Shielded / Platform activity details | Shielded memo (36-byte text), Platform-address activity | History row | `S/UI/Home/Views/ShieldedActivityHistory.swift`, `PlatformAddressHistory.swift` | — | — | M |
| IOS-036 | Tax categories | Income / Transfer In / Transfer Out / Expense / Internal; tap cycles; integrations pre-tag | Tx details | `S/Models/Taxes/Taxes.swift`, `S/Models/Tx Metadata/TransactionMetadata.swift` | — | — | M |
| IOS-037 | Reclassify intro | One-time "Reclassify your transactions" | First tx details | `S/UI/Tx/Reclassify Transactions/*` | — | — | M |
| IOS-038 | Historical fiat rate per tx | `tx_userinfo.rate*` stamped at first sight | — | `S/Models/Tx/Transactions.swift` | rates | — | M |
| IOS-039 | CSV export | 12 columns, requires sync, excludes mixing/moves; `report-<date>.csv` | Tools → CSV Export | `S/Utils/CSVBuilder.swift`, `S/Models/Taxes/Services/TaxReportGenerator.swift`, `S/UI/Menu/Tools/CSVExportSheet.swift` | — | — | M |
| IOS-040 | ZenLedger | OAuth client-credentials; posts all receive addresses; opens signup URL | Tools → ZenLedger | `S/Models/Taxes/ZenLedger.swift`, `S/UI/Menu/Tools/ZenLedger/*` | `api.zenledger.io` (`ZenLedger-Info.plist` CLIENT_ID/SECRET) | — | M-int |

## 5. Payments: send, receive, internal transfer, shielded, CoinJoin

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-041 | Payments sheet | Tabs Send / Receive / Internal transfer; Send card: Send to username (DP+identity), Send to Dash address, Scan Dash QR, Swap to other crypto (mainnet+key) | Center tab button (sheet) | `S/UI/Payments/Landing/*` (`PaymentsTabSelector`, `PaymentsSendCard`, `PaymentsReceiveContent`) | — | — | M |
| IOS-042 | Send to address | Type / paste / "Send to copied address" suggestion / QR; classifier Base58 → Core, bech32m `dash`/`tdash` → Platform, `0x10`+43 bytes → Shielded | Payments → Send | `S/UI/Payments/Pay/SendScreen.swift`, `SendViewModel.swift`, `S/UI/Payments/PaymentModels/DWParsedPaymentURI.swift` | — | `parsePlatformRecipient` | M |
| IOS-130 | **QR from clipboard image** (new) | Pasteboard extractor decodes a QR from a copied image | Send address paste | `S/UI/Payments/Pay/Models/DWPasteboardAddressExtractor.m` (CIDetector QR) | — | — | M (desktop: paste/drag image, screen region) |
| IOS-043 | QR scanning | Camera scanner; BIP70 `r=` fetch on scan; "Not a Dash QR code"/"Invalid Payment Request"; no photo-library import | Scan QR shortcut/card | `S/UI/Payments/ScanQR/*` (`DWQRScanModel.m`) | camera | — | M (webcam + image file) |
| IOS-044 | Amount entry | DASH ↔ fiat, fee-aware Max per route, hide balance | Send step 2 | `SendScreen.swift` (`ExternalSendAmountScreen`), `S/UI/Payments/Amount/*` | rates | — | M |
| IOS-045 | Source picker + 8 routes | core↔core/shielded, platform→platform/core/shielded, shielded→core/platform/shielded | Send step 2 | `SendScreen.swift` (`SendSourceScreen`), `SendViewModel.Route` | — | — | M (Platform routes ADV) |
| IOS-046 | Core confirm | Exact FFI fee + total; build+sign, broadcast only on Confirm; 1 duff/byte fixed | Send | `ConfirmPaymentViewController.swift` (`ConfirmPaymentSheet`), `WalletSendService.swift`, `I/SwiftDashSDKTransactionSender.swift` | — | `CoreTransactionBuilder`, `broadcastTransaction` | M |
| IOS-047 | Non-Core confirm | Steps Authorizing → Locking funds → Generating proof → Broadcasting; "Submitted — waiting" | Send | `SendScreen.swift` (`SendConfirmSheet`), `I/PlatformSendExecutor.swift`, `S/UI/Payments/InternalTransfer/Shielded/ShieldedTransferCoordinator.swift` | — | `shieldedShieldToRecipient`, `shieldedTransfer`, `shieldedUnshield`, `shieldedWithdraw`, `shieldedFundFromAssetLock`, address-wallet `transfer`/`withdraw` | M |
| IOS-048 | URI handling | `dash:`/`pay:` BIP21 (amount, label, message, r, sender, user, currency, local, req-*) | OS link, QR, paste | `S/Models/URL Handling/*`, `S/Models/PaymentProtocol/BIP70URI.swift`, `PaymentURIBuilder.swift` | — | — | M |
| IOS-125 | **Inter-app URL actions** (new) | `dashwallet://request=address&sender=<scheme>` → PIN prompt "Application %@ is requesting an address…" → opens `<sender>://callback=address&address=…&source=dashwallet`; `dashwallet://pay=…&sender=…` → pay; `dashwallet://scanqr` → scanner; URLs deferred until unlock; "Unsupported URL" / "Not a Dash URL" alerts; Uphold OAuth callback routed by substring | OS URL open | `DashWallet/AppDelegate.m:268-315`, `S/UI/RootNavigation/DWAppRootViewController.m:108-160`, `S/Models/URL Handling/DWURLParser.m`, `DWURLRequestHandler.m` | `LSApplicationQueriesSchemes` `dashcontrol`, `dashdirect` | receive-address reader | M (desktop protocol handler) / DEC for request=address callback |
| IOS-049 | BIP70 / BIP72 | Fetch → X.509 verify → expiry/network → pay → Payment → ACK | `r=` URI | `S/Models/PaymentProtocol/*`, `I/BIP70PaymentService+App.swift`, `I/BIP70SendAuthorizer.swift` | merchant `r=` | build/sign/broadcast | M |
| IOS-050 | Pay DashPay contact | Contact picker → amount → confirm (`ConfirmContactSendSheet`) → DIP-15 send from Core; per-contact lock after unknown broadcast outcome; also from contact profile (`PayContactSheet`) | Payments → Send to username; contact profile → Pay | `S/UI/Payments/Pay/SendToContact*.swift`, `SendScreen.swift`, `S/UI/DashPay/Contacts/SwiftUI/ContactProfileSheet.swift`, `WalletSendService.sendToContact` | — | `sendDashPayPayment` | DP |
| IOS-133 | **Send to username = contacts only** (new) | Picker lists established contacts; "Search for a User" opens Add Contact; no direct pay-to-username for non-contacts (FAQ "Coming soon") | Payments → Send to username | `S/UI/Payments/Pay/SendToContactScreen.swift` (`SendToContactPickerScreen`) | — | `searchDpnsNames` | DP |
| IOS-051 | Send guards + error copy | Blocked during initial sync / offline; insufficient, fee unavailable, rejected, unknown outcome, shielded too-small/immature/bundle ceiling | Send | `SendViewModel.swift`, `WalletSendService.swift` | — | — | M |
| IOS-052 | Post-send success | Success tx details | After send | `TxDetailViewController.swift` (`SuccessTxDetailViewController`) | — | — | M |
| — | NFC pay | `DWPayModel payWithNFC` | not offered | `S/UI/Payments/Pay/Models/DWPayModel.m` | CoreNFC | — | N/A |
| IOS-053 | Receive | Transparent/Shielded (+ Platform ADV) toggle, QR, copy, Share; placeholders while Platform/shielded start | Payments → Receive; lock screen Quick Receive | `PaymentsReceiveContent.swift`, `I/SwiftDashSDKReceiveAddress*.swift` | — | `shieldedDefaultAddress`, Platform derived addresses | M |
| IOS-054 | Live payment watcher | "Watching for a payment…" → Received card (amount, InstantSend/ChainLocked/mempool, time, memo, View transaction, **Receive another**) | Receive | same + `PaymentsLandingViewModel.swift` | — | tx stream; shielded projection polled 5 s | M |
| IOS-055 | Request amount | Core QR with amount + username; Share; Copy QR; paid detection | Receive → Specify amount | `S/UI/Payments/Receive/RequestAmount/RequestAmountScreen.swift` | — | `PaymentURIBuilder` | M |
| IOS-056 | Sweep / import private key | Removed (WIF rejected; menu row hidden; shortcut `break`) | — | `PaymentsReceiveContent.swift:39-42`, `DWQRScanModel.m` | — | needs arbitrary-UTXO spend | DEC |
| IOS-057 | Move mixed CoinJoin coins | Recovery scan (gap 100); prompt/sheet → Dash Wallet or Shielded; ≤ 500-input chunks; Settings + Tools rows while > 1000 duffs | Home popup, Settings, Tools | `S/Models/CoinJoin/CoinJoinRecovery.swift`, `S/UI/Home/Views/CoinJoinMoveFunds/*`, `WalletSendService.sweepCoinJoin` | — | `shieldedFundFromCoinJoinDrain`, CoinJoin balance | M |
| IOS-058 | CoinJoin mixing | Discontinued on iOS ("CoinJoin is no longer supported") | — | — | — | — | DEC (desktop M3 shipped mixing) |
| IOS-059 | Shielded balance + sync | Per wallet/network; re-sync on start/foreground | Home, Sync Info | `I/ShieldedBalanceController.swift`, `ShieldedSyncMonitor.swift`, `ShieldedRecoveryController.swift` | — | `syncShieldedNow`, `startShieldedSync` | M |
| IOS-060 | Shield / unshield / shielded sends | From Core (asset lock + type 18), Platform (type 15), CoinJoin; to Core/Platform/Shielded; fee-aware Max. **No memo entry on send** (memo only displayed on receive) | Send, Internal transfer | `ShieldedTransferCoordinator.swift` | — | `shieldedShieldPreflight`, `estimateShieldedFee`, shielded calls above | M |
| IOS-061 | Finish stuck shielded transfer | "Pending — tap to finish" → sheet | History row | `.../Shielded/ShieldedRecoverySheet.swift` | — | `shieldedResumeFundFromAssetLock` | M |
| IOS-062 | Internal transfer | Core ↔ Shielded ↔ Platform ↔ identity credits; privacy tip; timing sheet; "Withdraws the entire balance" | Payments → Internal transfer | `S/UI/Payments/InternalTransfer/*` | — | `topUpIdentityWithFunding`, `identityWithdraw`, address transfer | M (identity/Platform ADV) |
| IOS-063 | Advanced mode | Toggle + info sheet; auto-on at first Platform balance | Settings | `S/UI/Menu/Settings/SettingsMenuViewModel.swift`, `Components/AdvancedModeInfoSheet.swift` | — | `enableAdvancedModeIfFunded` | M |
| IOS-064 | Platform (DIP-17) addresses | Balance + BLAST sync | Home (ADV), Sync Info | `I/PlatformAddressSyncCoordinator.swift`, `PlatformBalance*.swift`, `PlatformCreditsFormatter.swift` | — | `startPlatformAddressSync`, `syncPlatformAddressNow`, `addressesWithBalances` | M |

## 6. DashPay (identity, usernames, contacts, profile, invitations)

All DashPay writes are PIN/biometric-gated through `I/Identity/DWIdentityAuthorizer.swift` and signed with
`KeychainSigner` into `ManagedPlatformWallet`. Reads come from SwiftData rows the Rust persister writes; the DashPay
background sync loop (`startDashPaySync`, every ~15 s) is started by `PlatformAddressSyncCoordinator`.

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-065 | Join DashPay intro + readiness | Intro (Create a username, Add friends, Personalise profile, Private by design); Voting info; **shielded-funding readiness checklist** (≥ 0.1 / 0.25 DASH shielded, 3 h rest, pool ≥ 250 notes, "Use transparent balance instead", "Have an invitation?") | Home/More Join banner | `S/UI/DashPay/Setup/CreateUsername/{JoinDashPayScreen,JoinDashPayReadinessScreen,VotingInfoScreen,JoinDashPayInfoDialog}.swift`, `I/Identity/ShieldedIdentityFundingReadiness.swift` | — | shielded balance/pool count | DP |
| IOS-134 | **Contested-name explainer + confirmation** (new) | Voting Info: names with digits 2–9, hyphen or > 20 chars are auto-approved; others voted ~2 weeks; can be locked; keep passphrase safe. `ContestedNameConfirmationSheet` before submitting a contested request | Create username | `VotingInfoScreen.swift`, `CreateUsernameViewController.swift` (`ContestedNameConfirmationSheet`) | — | `dpnsNormalizeLabel`, `dash_sdk_dpns_is_contested_username` | DP |
| IOS-066 | Username form | 3–23 chars `[A-Za-z0-9-]`, no edge hyphen; 0.4 s debounced availability; contested detection; contest precheck (locked / active / fresh); "For sale" → **Buy for %@ Dash** | Join flow | `CreateUsernameViewController.swift`, `CreateUsernameViewModel.swift`, `S/UI/DashPay/Setup/Model/UsernameValidationRuleResult.swift` | — | `dpnsCheckAvailability`, `contestPrecheck`, `dpnsMarketplaceNameState`, `purchaseDpnsName` | DP |
| IOS-067 | Identity + username registration | Costs 0.03 / 0.25 DASH (`DWDashPayConstants.m`). Pay with: **Core asset lock**, **Platform addresses** (0.002 DASH fee headroom on input 0), **Shielded pool** (Type 20, fixed 0.1/0.25 exits), **Invitation** (DIP-13). Pre-persist keys incl. ENCRYPTION/DECRYPTION; preorder + register; resumable after kill ("Your previous payment was found…"); drafts per network/wallet | Join flow | `I/Identity/DWIdentityRegistrationCoordinator.swift` (2115 lines), `DWIdentityRegistrationBridge.swift`, `DWRegistrationPhaseAdapter.swift`, `DWDashPayIdentityKeys.swift`, `DWUsernameRegistrationRecovery.swift` | — | `prePersistIdentityKeysForRegistration`, `registerIdentityWithFunding`, `registerIdentityFromAddresses`, `shieldedIdentityCreateFromPool`, `claimInvitation`, `registerDpnsName`, `resumeIdentityWithAssetLock` | DP |
| IOS-068 | Contested-name tracking | Pending name hidden until resolved; win → finalize, loss/lock → clear; optional **temporary non-contested username** ("Submit both usernames") | Join flow, status screen | `I/Identity/DWContestedNameStatusService.swift` | — | `fetchContestVoteState`, `syncContestedDpnsNames`, `getContestedDpnsNames` | DP |
| IOS-135 | **Username request status** (new id; was folded into 068) | Contenders, tallies, lock/abstain votes, "Leading", deadline, outcomes ("You won", "went to someone else", "Locked", "no winner"), Add a temporary username, Register username | More → DashPay header | `S/UI/DashPay/Usernames/UsernameRequestStatusScreen.swift` (built in `MainMenuViewController.swift`) | — | `dpnsContestVoteState`, `registerDpnsName` via `UsernameMarketplaceService` | DP |
| IOS-069 | My Profile sheet | Avatar, display name, Identity ID (copy), **Identity Account Balance** + info, DPNS Names, pending contest "Voting in progress / Ends around", Edit Profile, Top Up, Get (another) username, Finish registration, retry loading | Home avatar; Contacts → My identity | `S/UI/DashPay/Profile/SDKIdentityProfileSheet.swift` | — | `identityGet`, `identityBalanceCredits`, `getDpnsNames` | DP |
| IOS-131 | **Identity top-up** (new detail) | Presets 0.05 / 0.1 DASH + Custom ≥ 0.01; Pay from Transparent / Platform / Shielded; Shielded is two-step ("Step 1 of 2 — moving Dash out of your Shielded balance…") | My Profile → Top Up | same (`IdentityTopUpViewModel`, `IdentityTopUpSheet`) | — | `topUpIdentityWithFunding`, `topUpFromAddresses`, `shieldedUnshield` | DP |
| IOS-070 | Withdraw identity credits | To Core / to own Platform address | Internal transfer (ADV) | `S/UI/Payments/InternalTransfer/IdentityWithdrawViewModel.swift` | — | `identityWithdraw`, credit transfer | DP (ADV) |
| IOS-132 | **Identity Max withdrawal reserve** (new, #1172) | Max holds back enough credits for the network to accept; refusal mapped to "insufficient credits" (needs platform #5206, on v5.0-dev **after** the desktop pin) | Internal transfer | `IdentityWithdrawViewModel.swift`, masternode withdrawal reserve derived from it | — | `identityWithdraw` | DP (ADV) |
| IOS-071 | Identities screen | List User / Masternode / Evonode / Observed; Set as Main; copy ID; public keys (purpose, security level, contract bounds, derivation path); Refresh keys from Platform; Find identities; Get a username; voting status | More → Identities (ADV) | `S/UI/Menu/Security/Wallets/IdentitiesScreen.swift` (`IdentityDetailScreen`, `IdentityPublicKeysScreen`), `IdentitiesViewModel.swift`, `I/Identity/DWIdentityKeyUpgrader.swift` (`loadIdentity(atIndex:)`) | — | `setMainIdentityId`, `identityGetKeys`, identity discovery | DP (ADV); MN/evonode identities CORE |
| IOS-072 | Contacts tab | Gradient header with My identity ("@user · View profile"); Contact Requests (n) with inline Accept; My Contacts; Pending Requests; Hidden; search contacts + inline network username search ("On the Dash network", "Show more results"); empty state; **Enable DashPay** (adds missing keys, shows ~fee "Paid from your identity's credit balance", success sheet) | Contacts tab (identity present) | `S/UI/DashPay/Contacts/SwiftUI/ContactsScreen.swift` (`EnableDashPayConfirmSheet`, `EnableDashPaySuccessSheet`), `I/Contacts/SwiftDashSDKContactsService.swift` (`enableDashPay`, `missingDashPayKeyCount*`, fee = 100,000 + 6,500,000 credits/key) | — | IdentityUpdate (add ENCRYPTION/DECRYPTION keys), `searchDpnsNames` | DP |
| IOS-129 | **DashPay intro / FAQ** (new) | "About DashPay": Pay people not addresses; private payments; history organised; **Coming soon: private contact requests, Shielded DashPay, paying non-contacts**. FAQ: What is DashPay, cost, privacy vs Bitcoin/Ethereum, Unstoppable Domains, why enable, when requests become private | Contacts tab | `ContactsScreen.swift` (`DashPayFAQSheet`) | — | — | DP |
| IOS-073 | Add contact | Username search (≥ 2 chars); My QR; Scan QR; preview sheet; Send Contact Request; states (already a contact, sent you a request → Accept, pending, this is you) | Contacts → Add; Send-to-username → Search | `S/UI/DashPay/Contacts/SwiftUI/AddContactScreen.swift` (`AddContactPreviewSheet`, `MyDashPayUserQRSheet`) | — | `searchDpnsNames`, `resolveDpnsName`, `sendContactRequest` | DP |
| IOS-127 | **Recipient eligibility pre-check** (new) | Before PIN: recipient needs an enabled ECDSA DECRYPTION (or ENCRYPTION) key; otherwise "Can't receive contact requests — This user hasn't set up the keys…" | Add contact | `SwiftDashSDKContactsService.contactRequestEligibility(for:)` | — | `identityGetKeys` | DP |
| IOS-137 | **DashPay user QR** (new) | `dashpay://user?id=<base58>&username=…`; scanned QR is verified on-chain ("Verifying user…", "couldn't be verified… QR may be outdated", "isn't a DashPay user QR code") | Add contact → My QR / Scan QR | `I/Contacts/DashPayUserLink.swift`, `AddContactScreen.swift` | camera | `resolveDpnsName`, identity fetch | DP |
| IOS-074 | Contact profile | Accept / Ignore; Pay; **Activity** (payments with this contact; "Payments made directly to an address aren't retained here"); Contact settings: Alias, Note ("Only visible to you"), Hide / Unhide | Contacts rows, Notifications, tx details | `S/UI/DashPay/Contacts/SwiftUI/ContactProfileSheet.swift`, `ContactAvatarView.swift` | avatar URLs | `acceptContactRequest`, `ignoreContactSender` (local mute), `payments(with:)` | DP |
| IOS-128 | **Contact info document** (new) | Alias/note/hidden saved locally and published as an encrypted DashPay `contactInfo` document; publish deferred until ≥ 2 established contacts (DIP-15); skipped for watch-only identity | Contact settings → Save | `SwiftDashSDKContactsService.setContactMeta` | — | `setDashPayContactInfo` → `ContactInfoPublishOutcome` | DP |
| IOS-075 | Notifications feed | Bell with unread count on Home; New / Earlier / Pending sections; search; inline Accept; events: request received, request accepted, "is now your contact", you sent / you added | Home bell | `S/UI/DashPay/Contacts/SwiftUI/NotificationsScreen.swift`, `S/Infrastructure/Notifications/DashPayNotificationsReadState.swift` | — | SwiftData contact-request rows | DP |
| IOS-076 | Edit profile | Display name ≤ 25, About me ≤ 250 (counter), avatar: **Take a Photo**, **Select from Gallery** (→ crop + face detection → Imgur upload), **Public URL** (≤ 256, fetched), **Gravatar** (email hashed, not stored); "Save changes?" on dismiss | Home, More header | `S/UI/DashPay/Profile/Edit Profile/RootEditProfileViewController.swift` → `DP/Profile/EditProfile/*` (`DWCropAvatarViewController`, `Utils/DWFaceDetector`, `External Sources/*`, `Upload/*`, `Imgur/*`), `S/Infrastructure/Networking/DWAvatarUploadClient.swift`, `I/Identity/DWProfileUpdateCoordinator.swift` | `api.imgur.com` (`Imgur-Info.plist` `IMGUR_CLIENT_ID`), `www.gravatar.com`, any public image URL | `createDashPayProfile` / `updateDashPayProfile` (SDK hashes avatar: SHA-256 + dHash) | DP |
| IOS-077 | Claim invitation | Paste / scan; "Valid invitation", "Invitation from %@"; already-has-username refusal; continue to username; after register offer contact request to inviter; links stored pre-wallet and replayed | Join readiness "Have an invitation?", Home, OS link | `S/UI/DashPay/Invitations/ClaimInvitationScreen.swift` (`ClaimInvitationFlow`), `I/Invitations/DWInvitationService.swift`, `DWInvitationLinkNormalizer.swift`, `S/UI/RootNavigation/DWInvitationSetupState.m` | `invitations.dashpay.io`, `dashpaytest.onelink.me` (associated domains) | `parseInvitation`, `claimInvitation` | DP |
| IOS-078 | Create / share / reclaim invitations | Removed from iOS UI (SDK has `createInvitation`; 0.01 DASH constant unused) | — | `INVITATIONS_REBUILD_PLAN.md` (superseded) | — | `createInvitation` (exists at pin) | DEC |
| IOS-126 | **Multiple usernames per identity** (new) | Register an additional available name directly on the identity ("Register “%@”", paid from identity credits); "Get another username" in profile/identities; DPNS Names list; finish registration with existing credits | Marketplace → Find Names; profile; identities | `S/UI/Explore Dash/UsernameMarketplaceScreen.swift` (`RegisterNameSheet`), `I/UsernameMarketplaceService.swift`, `SDKIdentityProfileSheet.swift` | — | `registerDpnsName`, `getDpnsNames`, `setMainIdentityId` | DP |
| IOS-084 | Username marketplace | Tabs Find Names / My Names / Browse / Purchases; buy (independent-seller disclaimer), List For Sale, Change Price, Remove From Sale, Transfer (username or identity ID, free), trade history, Request (join contest as contender, cost not returned) | Explore, Identities | `UsernameMarketplaceScreen.swift` (2117 lines; `MarketplaceNameDetailSheet`, `SetNamePriceSheet`, `TransferNameSheet`), `I/UsernameMarketplaceService.swift` | — | `searchDpnsMarketplace`, `myDpnsMarketplaceNames`, `dpnsMarketplaceNameState`, `dpnsNameHistory`, `purchaseDpnsName`, `setDpnsNamePrice`, `delistDpnsName`, `transferDpnsName`, `syncDpnsMarketplace`, `startDpnsSync`, `dpnsGetCurrentContests` | DP |
| IOS-079 | Contested-username voting | Contests list (search; sort ending soonest / name / most votes / most lock), detail, cast Approve / Abstain / Lock, bulk vote, voting-node selection, local history; voter set = wallet-owned MN voting keys + keys attached to tracked MNs | More → Governance → Voting (if enabled) | `S/UI/DashPay/Voting/*`, `I/Voting/{ContestedNamesService,MasternodeVoteCaster,MasternodeVoterRegistry}.swift`, `S/Models/Voting/VoteHistoryDAO.swift` | — | `dpnsActiveContests`, `dpnsContestVoteState`, `castContestedResourceVote` | **DEC** (masternode-owner voting; desktop scope says "governance… voting" → Dash Core) |
| IOS-140 | **Mock Buy credits** (new) | Fake credit balance decremented per profile save, fake low/depleted warning, mock buy screen | More → edit profile | `S/UI/DashPay/Credits/BuyCreditsViewController.swift`, `S/UI/Menu/Main/MainMenuViewController.swift:300,666` | — | — | N/A |
| — | DashPay sync diagnostics | Identity/contacts sync status | Sync Info → DashPay Sync Info | `S/UI/Menu/SyncInfo/DashPaySyncInfoScreen.swift` | — | `dashPaySyncNow` | DEV |
| — | Legacy dead DashPay UI | Welcome/GetStarted pages, UsernamePending, `VerifyIdentityScreen`, `DP/Setup/*`, `DWNetworkErrorViewController` | — | as named | — | — | N/A |

## 7. Explore, spend and integrations

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-095 | Explore menu | Where to Spend, ATMs, Staking (CrowdNode account only; sync-wait dialog), Username Marketplace, Buy & Sell, Get Test Dash (testnet) | Explore tab / More → Explore | `S/UI/Explore Dash/ExploreMenuScreen.swift` | `faucet.testnet.networks.dash.org` | — | M |
| IOS-096 | Explore DB sync | Firebase Storage `gs://dash-wallet-firebase.appspot.com/explore/explore-v4[-testnet].db`; checksum-zipped; launch, 24 h, network switch; bundled seed DB | — | `S/Models/Explore Dash/Services/ExploreDatabaseSyncManager.swift` | Firebase (`GoogleService-Info.plist`) | — | M-int (needs a non-Firebase mirror or Firebase REST) |
| IOS-097 | Merchants | Online / Nearby / All; map + list; FTS; all locations; merchant-types dialog | Explore → Where to Spend | `S/UI/Explore Dash/Merchants & ATMs/*`, `Info/MerchantTypesDialog.swift` | MapKit, CoreLocation | — | M (desktop: map provider + IP/OS location) |
| IOS-098 | ATMs | All / Buy / Sell / Buy & Sell; search | Explore → ATMs | `.../List/AtmListViewController` | — | — | M |
| IOS-099 | Explore filters | Payment method, sort, radius 1/5/20/50 mi, territory, denomination type | Explore lists | `.../Filters/*` | — | — | M |
| IOS-100 | POI details | Address, phone, website, directions, Pay with Dash, Buy a Gift Card (provider picker by discount) | Merchant row | `.../Details/*` (`POIDetailsViewController`) | — | — | M |
| IOS-101 | DashSpend CTX | Email OTP login, live discount, BIP70-paid gift card | POI → Buy a Gift Card | `S/Models/Explore Dash/Services/DashSpend/CTX/*`, `S/UI/Explore Dash/Views/DashSpend/*` | `spend.ctx.com` / `staging.spend.ctx.com` (`X-Client-Id`) | BIP70 pay | M-int |
| IOS-102 | DashSpend PiggyCards | Email OTP signup/login, denominations, order → plain send; RU/CU block | POI → Buy a Gift Card | `.../DashSpend/PiggyCards/*` | `api.piggy.cards` (`apidev.piggy.cards`) | Core send | M-int |
| IOS-103 | Gift card details | Polling, number, PIN, barcode (generated / downloaded + Vision decode), how-to; stored cards | Tx details, Home | `.../GiftCardDetails/*`, `HomeView.swift` (`GiftCardDetailsSheet`) | provider APIs | — | M-int |
| IOS-087 | Buy & Sell portal | Topper, Uphold, Coinbase (not GB), Dash DEX (mainnet+key); balances; auth-gated | Shortcut, Explore | `S/UI/Buy Sell/*` | — | — | M-int |
| IOS-088 | Topper | ES256 JWT signed in-app → widget | Portal | `S/Models/Uphold/Topper.swift` | `app.topperpay.com`, `api.topperpay.com` (`Topper-Info.plist` private key) | receive address | M-int |
| IOS-089 | Uphold | OAuth; DASH card balance; transfer to wallet with OTP; logout tutorial | Portal | `S/Models/Uphold/*`, `S/UI/Uphold/*` | `api.uphold.com` / sandbox (`Uphold-Info.plist`) | receive address | M-int |
| IOS-090 | Coinbase | OAuth; buy (min $1.99, 0.6 %); transfer both ways with 2FA; Convert disabled (MO-103) | Portal | `S/Models/Coinbase/*`, `S/UI/Coinbase/*` | `login.coinbase.com`, `api.coinbase.com` (`Coinbase-Info.plist`) | send/receive | M-int |
| IOS-091 | CrowdNode staking | Tx-encoded signup, deposit ≥ 0.5, signed-message withdraw, online account link, APY, reminder | Explore → Staking (existing accounts), shortcut | `S/Models/CrowdNode/*`, `S/UI/CrowdNode/*` | `app.crowdnode.io`, `login.crowdnode.io` / test hosts | Core send, message signing | DEC (suspended on iOS) |
| IOS-092 | Dash DEX sell | Coin select (~130), address (paste/QR/linked Coinbase/Uphold), amount, 10 s preview, memo tx, status | Payments → Swap; portal | `S/Models/{Swap,SwapKit,Maya}/*`, `S/UI/{Swap,SwapKit,Maya}/*` | `api.swapkit.dev` (`SwapKit-Info.plist` key), Maya midgard/mayanode | Core send w/ OP_RETURN | M-int |
| IOS-093 | Dash DEX buy | Coin, amount, refund address, deposit QR | same | same | same | — | M-int |
| IOS-094 | Swap order tracking | SQL `swap_orders`, 30 s polling, 24 h expiry, notifications, pending-IS gate | — | same | same | — | M-int |
| IOS-085 | DashConnect | `dash-key:` login keys + `dash-st:` state transitions; approve sheet; connections list; test networks only (mainnet mock) | Tools → Connections, QR, deep link | `S/Models/DashConnect/*`, `S/UI/DashConnect/*` | dApps (Yappr) | `deriveIdentityAuthKeyAtSlot`, `parseStateTransition`, `updateIdentity`, `createDocument`, `replaceDocument` | DP (DEC: Connect v2 supersedes; dips#191) |
| IOS-086 | Token purchase approval | `ApproveTokenPurchaseSheet` (quantity, token ID, max price). No token balances / lists / transfers anywhere | DashConnect | `S/UI/DashConnect/ApproveTokenPurchaseSheet.swift` | — | `tokenPurchase`, `calculateTokenId` | DEC |

## 8. More menu, settings, tools, governance, notifications, platform surfaces

More menu rows (`S/UI/Menu/Main/MainMenuViewModel.swift`, verified): Explore · Sync Info · Wallets (ADV) · Identities (ADV)
· Security · Settings · Tools · Support · Governance; DashPay header (Join banner / profile, Edit Profile, request status).

| ID | Feature | Description | Entry point | Key files | External deps | SDK / Platform calls | Scope |
|---|---|---|---|---|---|---|---|
| IOS-104 | Local currency | Searchable; default from OS locale else USD | Settings | `S/UI/Menu/Settings/LocalCurrency/*` | rates | — | M |
| IOS-105 | Notifications toggle | "Turned off in iOS Settings → Open Settings" when OS-denied | Settings | `SettingsMenuViewModel.swift:280-310` | OS | — | M |
| IOS-106 | Network switch | Mainnet / Testnet (Devnet internal builds + Devnet Settings) | Settings | `SettingsScreen.swift`, `DevnetSettingsScreen.swift`, `I/DevnetConfiguration.swift` | `quorums.*.networks.dash.org` | `switchNetwork` | M |
| IOS-107 | About | Version, network, Explore DB sync status, Review app, Contact support, GitHub, support site; shake → tech info + Copy/Export Logs | Settings → About | `S/UI/Menu/Settings/About/*` | `support.dash.org/en/support/solutions` | — | M |
| — | Enable Voting toggle | Untranslated toggle | Settings | `SettingsMenuViewModel.swift` | — | — | DEC (with IOS-079) |
| IOS-108 | Security menu | View Recovery Phrase (wallet picker), Change PIN, Enable Touch/Face ID, Autohide Balance, Advanced Security, Reset Wallet | More → Security | `S/UI/Menu/Security/*` (`RecoveryPhraseFlow.swift`) | — | `WalletStorage` | M |
| IOS-109 | Reset / wipe | Phrase matching **all** stored mnemonics → wipe everything incl. integrations, vote history, tx metadata | Security → Reset Wallet | `ResetWalletInfo/*`, `I/SwiftDashSDKWalletWiper.swift` | — | — | M |
| IOS-110 | Multi-wallet | List, switch, rename, view phrase, remove (type phrase), add (create/import); accounts view + account detail | More → Wallets (ADV) | `S/UI/Menu/Security/Wallets/{WalletsScreen,WalletAccountsScreen,WalletAccountDetailScreen}.swift` | — | `switchWallet`, `performAddWallet` | M |
| IOS-138 | **Switch Wallet shortcut** (new) | 1 other → confirm; 2–5 → `WalletSwitchDialog`; > 5 → Wallets screen | Home shortcut | `HomeViewController+Shortcuts.swift` (`showSwitchWallet`), `WalletSwitchDialog.swift` | — | `switchWallet` | M |
| IOS-111 | Extended public key | BIP44 xpub QR / copy / share | Tools | `S/UI/Menu/Tools/ExtendedKeys/*` | — | `derivationWallet()` | M |
| IOS-112 | Export logs + support email | Zip ≤ 3 SDK sessions + app logs; Support → email with zip | Tools, Support, overlay Help | `S/Infrastructure/DiagnosticLogExporter.swift`, `S/Categories/UIViewController+DashWallet.swift` | `support@dash.org` | — | M |
| IOS-113 | Core Sync Status | Aggregate + per-phase progress, peers, last error, Stop, Rescan Filters (creation / height / full), Edit birth height, Drop Unconfirmed & Rescan | Sync Info | `S/UI/Menu/SyncInfo/SwiftDashSDKSPVStatusScreen.swift` | — | SPV coordinator | M (DEV parts) |
| IOS-139 | **Finish Transfers / Clear & resync** (new) | Bulk-complete unfinished asset-lock transfers ("Finishing %1$d of %2$d…"); clear chain data and resync | Sync Info → Core Sync Status | same | — | `resumeAssetLock`, SPV clear | M |
| IOS-114 | Platform / DashPay / Shielded sync screens | Sync Now / Stop / Clear; note counts | Sync Info | `PlatformSyncStatusScreen.swift`, `DashPaySyncInfoScreen.swift`, `ShieldedSyncInfoScreen.swift` | — | sync controls | DEV |
| IOS-115 | Storage Explorer | SwiftData browser incl. tokens; ungated, untranslated, destructive | Tools | `S/UI/Menu/Tools/StorageExplorer/*` | — | — | DEV |
| IOS-080 | Masternodes list/detail | Active / PoSe banned / Retired; keys owned, collateral, claimable balance, epoch blocks | Governance → Masternodes; Nodes shortcut | `S/UI/Menu/Tools/MasternodesScreen.swift`, `I/Masternodes/*` | — | `masternodes`, `fetchClaimableBalance` | CORE |
| IOS-081 | Evonode status / withdrawal / Unban | — | Masternode detail | `S/UI/Menu/Tools/{Evonode Status,Masternode Withdrawal,Unban}/*` | — | `getEvonodeStatus`, `masternodeWithdraw`, `masternodeUpdateService` | CORE |
| IOS-082 | Tracked masternodes | Track any MN; attach keys (vault) | Masternodes → Add | `S/UI/Menu/Tools/Tracked Masternodes/*` | — | `locateMasternode`, `trackMasternode` | CORE |
| IOS-083 | Masternode Keychain | Owner / Voting / Operator BLS / ed25519 keys | Tools | `S/UI/Menu/Tools/Masternode Keys/*` | — | derivation | CORE |
| IOS-116 | Local notifications | Received %@ (%@) (10 min fresh, 24 h catch-up); contact request received/accepted; swap complete/refunded/failed/unknown; CrowdNode deposit; inactivity reminder (30 d, Remind later / Don't remind); tap routing | Automatic | `S/Infrastructure/Notifications/*` (`Producers/*`, `NotificationRouter.swift`) | OS notifications | — | M (contact parts DP) |
| IOS-117 | Watch / Today → tray | Watch: balance, recent txs, receive QR, fixed-amount request keypad; Today ext not embedded | — | `WatchApp*/*` (`BRAWBalanceInterfaceController`, `BRAWReceiveMoneyInterfaceController`, `BRAWKeypad`), `TodayExtension/*` | WatchConnectivity | — | M (desktop menu-bar/tray analogue) |
| IOS-118 | Localization | 43 locales; 2,477 `Localizable.strings` keys + 154 plurals; English-as-key; Transifex `dash/dash-mobile-wallets` | — | `DashWallet/*.lproj`, `.tx/config` | Transifex | — | M |
| IOS-119 | Visual parity | SharedAssets (125 colors) + DashUIKit tokens/components | — | `Shared/Resources/SharedAssets.xcassets`, DashUIKit | — | — | M |
| IOS-120 | Accessibility labels / ids | Icon-only labels; #1187 added identifiers on Send address field | — | `ACCESSIBILITY.md` | — | — | M |
| IOS-121 | Testnet faucet | "1 tDash" in-app PoW faucet (SDK `TestnetFaucet().requestCoreDash`) + web fallback | Shortcut (testnet), Explore | `HomeViewController+Shortcuts.swift:377-421`; SDK `SwiftDashSDK/Utils/TestnetFaucet.swift` (exists at pin) | `faucet.thepasta.org` (inside SDK), `faucet.testnet.networks.dash.org` | `TestnetFaucet` | M |
| IOS-122 | Announcements | CloudKit in-app messaging `iCloud.org.dash.dashwallet` | Launch | `AppDelegate.m:150` | CloudKit | — | DEC |

---

## 9. URL schemes, links and external services

**Registered schemes** (`DashWallet/Info.plist` `CFBundleURLTypes`): `pay`, `dash`, `dashwallet`, `dashid`, `dashpay`,
`dash-key`, `dash-st`. **Associated domains** (`dashwallet.entitlements`): `invitations.dashpay.io`, `dashpaytest.onelink.me`.
**Queried schemes:** `dashcontrol`, `dashdirect`.

| Link | Handled where | Action |
|---|---|---|
| `dash:<addr>?…`, `pay:<addr>?…` | `DWURLParser` → `DWURLPayAction` | Send flow (BIP21/BIP72) |
| `dashwallet://pay=<…>&sender=<scheme>` | `DWURLParser` | Send flow |
| `dashwallet://request=address&sender=<scheme>[&account=0]` | `DWURLRequestHandler` | PIN prompt → open `<sender>://callback=address&address=<addr>&source=dashwallet` |
| `dashwallet://scanqr` | `DWURLParser` | Open QR scanner |
| any URL containing `uphold` | `DWURLIntegrationAction` | OAuth callback (`authURLReceived`) |
| `dashwallet://brokers/coinbase/connect` | Coinbase auth session | OAuth callback |
| `dash-key:…`, `dash-st:…` (≤ 4 KiB) | `DWDashConnectDeepLink` | DashConnect approve sheets |
| `dashpay://invite?du=…&assetlocktx=…&pk=…&islock=…`, `https://invitations.dashpay.io/applink?…`, OneLink wrappers | `AppDelegate.handleOpenURL` / `handleUserActivity` → `DWInvitationLinkNormalizer` | Claim invitation (stored pre-wallet) |
| `dashpay://user?id=…&username=…` | Add-Contact scanner only | Add contact (not an OS deep link) |
| `dashid:` | none | "Not a Dash URL" alert |

External services: unchanged from research 03 §1.20 (verified by host grep): `rates.ctx.com`, Insight
(`insight.dash.org`, `insight.testnet.networks.dash.org`), `blockchair.com`, `pool.ntp.org`, `www.dash.org`, faucets,
ZenLedger, Uphold, Topper, Coinbase, CrowdNode, SwapKit + Maya + NEAR explorers + coin-icon hosts, Firebase Storage,
CTX, PiggyCards, `ip-api.com`, **Imgur + Gravatar + arbitrary avatar URLs (DashPay)**, invitation links,
devnet quorum service, `support.dash.org`. Partner secrets live in git-ignored plists: `GoogleService-Info.plist`,
`Topper-Info.plist`, `Coinbase-Info.plist`, `ZenLedger-Info.plist`, `SwapKit-Info.plist`, `Imgur-Info.plist`,
`Uphold-Info.plist`.

## 10. DashPay → SDK call map and Platform pin check

Every SDK entry point the iOS DashPay surfaces call was searched for in the desktop pin's
`packages/swift-sdk/Sources` (`bc321362b9`). **All exist.** (Swift names; the desktop calls the same Rust
`rs-platform-wallet` functions through dw-ffi.)

| Area | Calls |
|---|---|
| Startup / sync | `startWalletSubsystems`, `startDashPaySync`, `stopDashPaySync`, `dashPaySyncNow`, `syncDpnsNames`, `startDpnsSync`, `syncContestedDpnsNames`, `startPlatformAddressSync`, `startShieldedSync` |
| Identity create | `prePersistIdentityKeysForRegistration`, `registerIdentityWithFunding`, `registerIdentityFromAddresses`, `shieldedIdentityCreateFromPool`, `claimInvitation`, `parseInvitation`, `resumeIdentityWithAssetLock` |
| DPNS | `dpnsCheckAvailability`, `dpnsNormalizeLabel`, `registerDpnsName`, `searchDpnsNames`, `resolveDpnsName`, `getDpnsNames`, `getContestedDpnsNames`, `fetchContestVoteState`, `dpnsContestIsOpen` |
| Marketplace | `searchDpnsMarketplace`, `myDpnsMarketplaceNames`, `dpnsMarketplaceNameState`, `dpnsNameHistory`, `purchaseDpnsName`, `setDpnsNamePrice`, `delistDpnsName`, `transferDpnsName`, `syncDpnsMarketplace`, `dpnsGetCurrentContests` |
| Voting | `dpnsActiveContests`, `dpnsContestVoteState`, `castContestedResourceVote` |
| Identity balance | `identityBalanceCredits`, `topUpIdentityWithFunding`, `topUpFromAddresses`, `identityWithdraw`, `identityGetKeys`, `setMainIdentityId`, IdentityUpdate (key upgrade) |
| Contacts | `sendContactRequest`, `acceptContactRequest`, `ignoreContactSender`, `setDashPayContactInfo`, `getIncomingContactRequest` |
| Profile | `getDashPayProfile`, `createDashPayProfile`, `updateDashPayProfile` |
| Payments | `sendDashPayPayment` (DIP-15), contact-payment projection over SwiftData |
| Not used by UI | `createInvitation` (exists; invites removed from iOS UI) |

Pin vs the branch the iOS app ships against (`release-dashpay-testflight.yml` → dashpay/platform default branch
`v5.0-dev`, fetched over HTTPS to FETCH_HEAD, checkout untouched): `v5.0-dev` = `bc41f1bc23`, pin is an ancestor,
**35 commits behind**. Wallet-relevant ones: #4978 "keep the chosen DPNS name across wallet sync", #5206 "report an
identity balance refusal on withdrawal as insufficient credits" (iOS #1172 depends on it), plus PV14 consensus
changes (#5014 shielding nullifiers, #5239, #5250, #5228 Core v24 ports) and a swift-sdk schema-freeze CI change.

## 11. Desktop analogues for iOS-specific surfaces

| iOS surface | Desktop analogue |
|---|---|
| Camera QR scan | Webcam scan + open image file + paste/drag image (IOS-130) + screen-region capture |
| Photo library / camera avatar (IOS-076) | File picker + webcam; keep crop + face-centering; Imgur upload needs a client ID |
| Lock-screen Quick Receive | Locked-window Quick Receive + tray item |
| Watch / Today widget | Menu-bar (macOS) / tray (Windows, Linux) with balance, receive QR, request amount |
| Universal links (invitations) | Protocol handler for `dashpay://invite` + "Paste invitation link"; `https://invitations.dashpay.io` needs a browser hand-off |
| `dashwallet://request=address&sender=` callback | Only meaningful with a local inter-app protocol; candidate to drop |
| Local notifications, permission row | OS notifications; tray badge for DashPay unread count |
| Face ID / Touch ID | Touch ID (macOS), Windows Hello; none on Linux |
| Share sheet | Save-as / copy / system share where available |
| CloudKit announcements | Drop or a signed JSON feed (DEC) |
| Jailbreak, NFC, App Store review, storefront geo | Drop (IP geo for Coinbase/PiggyCards) |

## 12. Screen census (for completeness checks)

SwiftUI screens/sheets/dialogs found (100): AddContactPreviewSheet AddContactScreen AddMasternodeScreen AddWalletSheet
AdvancedModeInfoSheet ApproveConnectionSheet ApproveTokenPurchaseSheet BalanceInfoSheet BulkVoteSheet BuySellPortalScreen
CSVExportSheet CastVoteSheet ClaimInvitationScreen CoinJoinMoveFundsSheet ConfirmContactSendSheet ConfirmPaymentSheet
ConfirmSpendDialog ConnectionsScreen ContactProfileSheet ContactsScreen ContestDetailScreen ContestedNameConfirmationSheet
CrowdNodeBalanceReminderSheet DashPayFAQSheet DashPaySyncInfoScreen DashSpendConfirmationDialog DashSpendPayConfirmationSheet
DashSpendPayScreen DashSpendTermsScreen DashSpendUserAuthScreen DevnetSettingsScreen EnableDashPayConfirmSheet
EnableDashPaySuccessSheet EvonodeStatusScreen EvonodeWithdrawalConfirmSheet EvonodeWithdrawalScreen ExploreMenuScreen
ExtendedPublicKeySheet ExternalSendAmountScreen GiftCardDetailsSheet GiftCardPurchaseSelectionSheet GovernanceMenuScreen
GroupedTransactionsScreen IdentitiesScreen IdentityDetailScreen IdentityPublicKeysScreen IdentityTopUpSheet
InternalTransferConfirmSheet InternalTransferScreen JoinDashPayInfoDialog JoinDashPayReadinessScreen JoinDashPayScreen
MainMenuScreen MarketplaceNameDetailSheet MasternodeDetailScreen MasternodesScreen MerchantTypesDialog ModalDialog
MyDashPayUserQRSheet NotificationsScreen PayContactSheet PaymentsLandingScreen PlatformSyncStatusScreen
RecoveryPhrasePickerScreen RegisterNameSheet RemoveWalletSheet RequestAmountScreen SDKIdentityProfileSheet
SecurityMenuScreen SendConfirmSheet SendScreen SendSourceScreen SendToContactPickerScreen SetNamePriceSheet SettingsScreen
ShareSheet ShieldedRecoverySheet ShieldedSyncInfoScreen SwapFeeInfoSheet SwiftDashSDKSPVStatusScreen SyncInfoMenuScreen
ToolsMenuScreen TrackedKeyManagementSheet TrackedMasternodeDetailScreen TrackedWithdrawalSheet TransactionDetailsSheet
TransactionFilterDialog TransferNameSheet TransferTimingSheet UnbanMasternodeSheet UsernameMarketplaceScreen
UsernameRequestStatusScreen UsernameVotingScreen VerifyIdentityScreen(dead) VotingInfoScreen VotingNodeSelectionSheet
WalletAccountDetailScreen WalletAccountsScreen WalletSwitchDialog WalletsScreen ZenLedgerInfoSheet.

UIKit controllers of note: legacy Edit Profile (`DP/Profile/EditProfile/*`), Coinbase (`S/UI/Coinbase/*`), Uphold,
CrowdNode, Maya/SwapKit portals, Explore lists/POI, Setup/backup, Masternode keys, Tx details, Syncing alert,
`BuyCreditsViewController` (mock). Every one maps to a row above.

## 13. Open questions

1. **Username voting (IOS-079) and the Enable Voting toggle:** desktop scope puts "governance… voting" in Dash Core, but
   contested-DPNS voting is a DashPay-adjacent feature on iOS. Build it, or leave to Dash Core?
2. **Identities screen for MN/evonode identities and the Nodes shortcut:** keep the User identity view (DashPay), drop masternode views?
3. **Invitations (IOS-078):** desktop could add creation since the SDK supports it, or match iOS (claim only).
4. **Shielded-first registration:** iOS steers new users to fund usernames from the Shielded pool (3 h rest, pool ≥ 250
   notes, fixed 0.1/0.25 exits). `shieldedIdentityCreateFromPool` exists in the pin's swift-sdk; does the desktop
   want shielded-first registration as the default path (as iOS does), and does dw-ffi expose the pool note count?
5. **Avatar upload:** Imgur needs a client ID and is anonymous-public; use the same key, another host, or URL/Gravatar only?
6. **Platform pin:** iOS ships on `v5.0-dev` tip (35 commits ahead of the pin). Bump to pick up #4978 (DPNS name kept across
   sync) and #5206 (identity withdrawal refusal) before DashPay work?
7. **Mainnet DashConnect / tokens:** iOS limits DashConnect v1 to test networks; Connect v2 is planned. Skip v1 on desktop?
8. **CoinJoin:** iOS dropped mixing, desktop M3 ships it. Keep (Dash Core parity) or match iOS (sweep only)?
9. **`dashwallet://request=address` inter-app callback and `dashid:`:** drop on desktop?
10. **Partner integrations (Topper/Uphold/Coinbase/DEX/CTX/PiggyCards/ZenLedger/Firebase Explore DB):** which partners
    will issue desktop credentials? All need secrets that iOS keeps in git-ignored plists.
