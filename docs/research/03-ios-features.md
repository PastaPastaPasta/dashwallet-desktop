# 03 — dashwallet-iOS feature, architecture and design inventory

Research input for the greenfield desktop Dash wallet (macOS / Windows / Linux) that must ship every user-facing feature of dashwallet-iOS and be "built like the iOS app" (same layering and stack shape, not a code port).

- **Source of truth:** `/Users/pasta/workspace/dashwallet-ios`, HEAD `7c0064d6b` (2026-10-05, branch with the completed DashSync → SwiftDashSDK migration). Read-only survey; the checkout may have local changes.
- **SDK:** SwiftDashSDK at `../platform/packages/swift-sdk` (local SPM package), consumed only through the adapter layer `DashWallet/Sources/Infrastructure/SwiftDashSDK/`.
- **Design system:** `/Users/pasta/workspace/DashUIKit` (origin `PastaPastaPasta/DashUIKit`, upstream `dashpay/DashUIKit`, HEAD `e8d9243`). The iOS app consumes upstream `dashpay/DashUIKit` @ `master` via SPM.
- **Path convention:** `S/` = `DashWallet/Sources/`. Other paths are relative to the iOS repo root.

**Scope warnings.** Several features look present but are removed, mocked or dead. They are listed in §1.22 so the desktop team does not over-scope. In short:
- Paper-wallet / private-key sweep was **removed**.
- CoinJoin **mixing** is discontinued; only a "move mixed coins" sweep remains.
- There is **no** governance budget-proposal UI.
- **Creating** invitations was removed; only claiming remains.
- "Buy credits" is a mock.
- There are no token balances.
- The Today extension is not embedded in the app.

---

## 0. Feature gating — how it actually works

| Gate | Kind | Where defined | Effect |
|---|---|---|---|
| `DASHPAY` | Compile-time. Swift `SWIFT_ACTIVE_COMPILATION_CONDITIONS` + `OTHER_SWIFT_FLAGS -DDASHPAY`; ObjC `GCC_PREPROCESSOR_DEFINITIONS DASHPAY=1` | `DashWallet.xcodeproj/project.pbxproj` (no xcconfigs) | ~147 Swift + ~40 ObjC `#if/#ifdef DASHPAY` sites in ~61 files. Heaviest: `HomeView.swift`, `MainMenuViewController.swift`, `MainTabbarController.swift`, `SendViewModel.swift`. On this branch it is defined in **all four configs (Debug / TestNet / Release / TestFlight) of both app targets**. |
| Target membership | Build-time | pbxproj | ~117 sources build only into the `dashpay` target: `DashPay/Presentation/**` (legacy ObjC), `S/UI/DashPay/**`, `Infrastructure/SwiftDashSDK/{Identity,Contacts,Invitations}`, `DWDashPayModel.m`, `Models/Usernames/*`. The `dashpay` scheme is the working build; `dashwallet` is "not kept green". Voting, the Username Marketplace, DashConnect and Masternodes build in **both** targets. |
| `DASH_TESTNET`, `DEBUG`, `DASH_DEVNET` (`$(DASH_DEVNET_FLAGS)`), `PIGGYCARDS_ENABLED`, `SNAPSHOT` | Compile-time | pbxproj, `DashWallet-Prefix.pch` | Dev-only behavior; Devnet option in the network switcher; PiggyCards (on in all configs); fastlane demo mode. |
| Identity present | Runtime (`MainTabbarController.hasDashPayIdentity` → `DWCurrentUserIdentityInfo`) | `S/UI/Main/MainTabbarController.swift` | Adds the **Contacts** and **Explore** tabs; rebuilt on `DWDashPayRegistrationStatusUpdated`, wallet switch, network change. |
| Advanced mode | Runtime (`DWGlobalOptions.advancedModeEnabled`, UserDefaults `DW_GLOB_advancedModeEnabled`; auto-on the first time a Platform balance is seen unless `advancedModeUserManaged`; `DWAdvancedModeDidChangeNotification`) | `S/Models/DWGlobalOptions.{h,m}`, Settings toggle | Platform balance row, Platform/identity routes in Internal Transfer and Send, Wallets + Identities menu rows, Platform receive. **Shielded is NOT gated by advanced mode.** |
| Voting enabled | Runtime (`VotingPrefs.votingEnabled`, default true) | `S/Models/Voting/VotingPrefs.swift` | Governance → Voting row. |
| Network | Runtime (`WalletEnvironment.networkKind`, UserDefaults `CURRENT_CHAIN_TYPE_KEY` 0/1/2; **missing key = mainnet** in every build config) | `S/Infrastructure/SwiftDashSDK/WalletEnvironment.swift` | Dash DEX is mainnet-only. DashConnect is test networks only. Testnet faucet shortcuts. Many integrations use sandbox/staging hosts on testnet (§1.20). |
| Geo | Runtime | `BuySellPortalViewController.swift`, `GeoRestrictionService.swift` | Coinbase hidden/blocked for GB (App Store storefront + GPS). PiggyCards blocked for RU/CU (GPS or `ip-api.com`). |
| `MOCK_DASHPAY = YES` | Hardcoded constant | `S/UI/DashPay/DWDashPayConstants.{h,m}` | Drives the fake "Buy credits" warning. Do not port. |

Desktop recommendation: the desktop app has one product (no "dashwallet vs dashpay" split). Treat DashPay as always compiled in and keep the **runtime** gates (identity present, advanced mode, network, voting toggle, geo).

---

## 1. User-facing feature inventory

Legend for the **Deps** column:
- **SDK** = SwiftDashSDK call or adapter.
- **HTTP** = external REST host.
- **KC** = keychain.
- **UD** = UserDefaults.
- **SQL** = app SQLite `store.db`.
- **SD** = SDK-owned SwiftData.

Gate `DP` = `#if DASHPAY` and/or dashpay-target-only; `ADV` = advanced mode.

### 1.1 App launch, onboarding, wallet creation and restore

**Launch order** (`DashWallet/AppDelegate.m`):
1. `FIRApp configure`
2. CloudKit in-app messaging (`iCloud.org.dash.dashwallet`)
3. `DWVersionManager migrateUserDefaults`
4. `enableAuthenticationIfNeeded`
5. `SecureTimeService.ratchetToWallClock`
6. `DatabaseConnection.migrateIfNeeded`
7. `SwiftDashSDKKeyMigrator.migrateIfNeeded`
8. `SwiftDashSDKWalletRuntime.startIfReady`
9. Root becomes `DWInitialViewController`

Third-party keyboards are blocked. A device passcode is required: without one the app shows a "Turn device passcode on" alert whose only action is Close App (`DWAppRootViewController`).

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Intro carousel ("Welcome" / "We Upgraded", "Pay with Ease", "More Control") running a live demo mini-wallet on stub models | `S/UI/Onboarding/*`, `Models/DWOnboardingModel.m`, `Controllers/DWDemoAppRootViewController`, `Stubs/*` | UD `shouldDisplayOnboarding` | — |
| Reinstall detection: "Wallets found on this device" → Keep Wallets / Delete All (typed acceptance phrase) | `S/UI/Setup/WalletRecovery/KeychainWalletRecoveryCoordinator.swift`, `DWInitialViewController.m` | SDK `WalletStorage` inventory (KC) | — |
| Setup landing: Create New Wallet / Recover Wallet. Order: Set PIN → biometric enrollment → backup info → main | `S/UI/Setup/DWSetupViewController.m`, `Setup.storyboard` | — | — |
| Set PIN: 4 digits, Set then Confirm. Intents CreateNewWallet / ChangePin / SetPin | `S/UI/Setup/SetPin/*` | KC service `org.dashfoundation.dash`, account `pin` | — |
| Biometric enrollment: enable Face/Touch ID or Skip. Default biometric spend limit 0.5 DASH | `S/UI/Setup/BiometricAuth/*` | LocalAuthentication | — |
| Create wallet + backup info: two warnings; "Advanced" choice of **12 or 24 words** | `S/UI/Setup/SecureWallet/BackupInfo/BackupInfoViewController.swift`, `Seed/RecoveryPhraseLength.swift`, `DWPreviewSeedPhraseModel+Mnemonic.swift` | SDK `Mnemonic.generate(wordCount:)`, `SwiftDashSDKWalletCreator` → `SwiftDashSDKHost.createOrImportWallet` (mnemonic persisted and verified **before** the wallet goes live: `MnemonicFirstWalletCreation`) | — |
| Show phrase + verify by tapping shuffled word chips in order, then "Verified Successfully" | `S/UI/Setup/SecureWallet/Seed/BackupSeedPhraseViewController.swift`, `DWPreviewSeedPhraseViewController.m`, `Verify/DWVerifySeedPhraseModel.m`, `VerifiedSuccessfully/*` | UD `walletNeedsBackup` | — |
| Backup reminder: shown 24 h after the first balance change if the wallet is still unbacked | `S/UI/Home/HomeViewController+BackupReminder.swift`, `DWHomeModel.m` | UD | — |
| Screenshot warning while the phrase is visible (regenerate-on-screenshot exists but is hardcoded off) | `S/UI/Setup/ScreenshotWarning/DWScreenshotWarningViewController.m` | OS screenshot notification | — |
| Restore from phrase: 12/15/18/21/24 words; words checked against all 10 SDK wordlists, language auto-detected; NFKD/lowercase normalisation; sets `resyncingWallet` | `S/UI/Setup/RecoverWallet/*` | SDK `Mnemonic.validate/cleanupPhrase/normalizePhrase`; import birth height 200000 on mainnet, 0 on testnet | — |
| **Phrase repair**: one bad word in 12 → find it; 10–11 words → find the missing words (~1 h). Enumerates checksum-valid candidates, derives m/44'/5'/0'/0/0, asks Insight which address has history; Damerau-Levenshtein suggestions as fallback; cancellable progress, concurrency 4. English only for on-chain confirmation | `S/Infrastructure/SwiftDashSDK/SwiftDashSDKPhraseRepairer.swift`, `DamerauLevenshtein.swift`, `SwiftDashSDKInsightClient.swift`, `S/UI/Setup/RecoverWallet/PhraseRepair/*` | HTTP `POST https://insight.dash.org/insight-api/addrs/txs` | — |
| Forgot PIN: enter a phrase matching any stored wallet → set a new PIN (wallet kept). "Wipe All Wallets" offered at ≥ 6 failures | `S/UI/LockScreen/DWLockScreenViewController.m` | `SwiftDashSDKHost.anyStoredMnemonicMatches` | — |
| DashSync → SwiftDashSDK key migration: background, resumable, multi-wallet. Launch hold "Preparing your wallet…" with failure card (Try Again / Export Logs / Help) | `S/Infrastructure/SwiftDashSDK/SwiftDashSDKKeyMigrator.swift`, `DASHSYNC_KEY_MIGRATION.md` | KC `WALLET_MNEMONIC_KEY_<id>`, `CHAIN_WALLETS_KEY_<genesis>`; UD `swiftSDKKeyMigration.v1.*` | — (iOS-only legacy; desktop has no legacy store) |
| Wallet lifecycle overlay: opening, migrating, failed open, switching network ("Switching to %@…" with Retry / Switch Back), switching / adding / removing wallet, wiping. Help sheet with support email + diagnostics | `S/UI/Main/WalletLifecycleOverlay.swift`, `WalletPreparationSupport.swift`, `Infrastructure/SwiftDashSDK/WalletLifecycleTransitionState.swift`, `WalletPreparationFailure.swift` | `support@dash.org` (Info.plist `SupportEmail`) | — |
| App DB migrations | `S/Infrastructure/Database/DatabaseConnection.swift`, `Migrations.bundle/*.sql`, `Migrations/*.swift` | SQL | — |
| Invitation-based onboarding: link stored before a wallet exists and replayed after setup; with a wallet, deferred until unlock + sync done → Claim screen | `S/UI/RootNavigation/DWInvitationSetupState.{h,m}`, `DWAppRootViewController.m`, `HomeViewController.swift` | Universal links `invitations.dashpay.io`, `dashpay://invite` | DP |

### 1.2 Auth gate (PIN, biometrics, lock screen, spending confirmation)

| Feature | Files | Deps | Gate |
|---|---|---|---|
| PIN lockout: 3 free attempts, then wait `6^(n−3)·60 s` (1 m, 6 m, 36 m, 3.6 h, 21.6 h); wallet **permanently disabled at 8** ("Recover with your recovery phrase"); "N attempts remaining" copy | `S/Infrastructure/Authentication/PinStore.swift` (`LockoutPolicy`), `AuthenticationService.swift` | KC `pin`, `pinfailcount`, `pinfailheight`, `USES_AUTHENTICATION` | — |
| Secure time: a monotonic ratchet so clock rollback can't shorten lockouts; fed by wall clock and HTTPS `Date` headers | `S/Infrastructure/Authentication/SecureTimeService.swift`, `HTTPClient.swift` | UD `SECURE_TIME` | — |
| Biometrics allowed only within **7 days** of the last PIN entry, and for spends only within the remaining biometric allowance; a failure zeroes the allowance and falls back to PIN | `AuthenticationService.swift` | UD `PIN_UNLOCK_TIME`; KC `SPEND_LIMIT_AMOUNT`, `BIOMETRIC_ALLOWED_AMOUNT_LEFT_KEY` | — |
| Lock screen: PIN pad, biometric login, **Quick Receive** (receive QR with Transparent/Platform/Shielded toggle), **Scan to Send**, Forgot PIN, live lockout countdown. Shown in a separate window; incoming URLs deferred until unlock | `S/UI/LockScreen/*`, `LockScreen.storyboard` | — | — |
| Auto-lock after `autoLockAppInterval` (default 60 s) in background | `S/UI/RootNavigation/DWRootModel.m` | UD; KC `LOCKSCREEN_DISABLED_KEY` | — |
| **One shared auth primitive**: `AuthenticationGate` (biometric → PIN modal, 120 s watchdog). Used by send, BIP70, view phrase, change PIN, advanced security, masternode keys, voting, CrowdNode, identity ops, log export | `S/Models/Transactions/WalletSendService.swift` (`AuthenticationGate`), `S/UI/Auth/PinPromptPresenter.swift`, `PinPromptView.swift` | — | — |

### 1.3 Home, balance, sync, shortcuts

**Tabs** (`S/UI/Main/MainTabbarController.swift`):
- **Home | Payments | More** without an identity.
- **Home | Contacts | Payments | Explore | More** when a DashPay identity exists.
- **Payments is not a destination**: `shouldSelect` intercepts the tap and presents `PaymentsLandingHostingController` as a sheet (Send / Receive / Transfer).
- Without an identity, Explore is reached via More → Explore.

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Balance hero: total = Core + Shielded (+ Platform in ADV; credits ÷ 1000 = duffs). Shows "Known balance" when partial, "—" when Core is unknown. Fiat sub-line | `S/UI/Home/Views/Home Balance View/HomeBalanceView.swift`, `BalanceModel.swift`, `Infrastructure/SwiftDashSDK/HomeBalancePresentation.swift` | `SwiftDashSDKWalletState.$balance`, `PlatformAddressSyncCoordinator.platformBalanceState`/`shieldedBalanceState`, `CurrencyExchanger` | Platform part ADV |
| Hide balance (tap hero; persisted; one-time hint). Long-press hero opens the local currency picker | same + `DWGlobalOptions.balanceHidden`, `tapToHideBalanceShown` | UD | — |
| Balance breakdown card: Core ("Transparent" in ADV / "Dash Wallet"), Platform (ADV), Shielded with syncing spinner; tap a row → `BalanceInfoSheet` pros/cons explainer | `BalanceInfoSheet.swift`, `S/UI/Payments/Pay/ChainNetworkToggle.swift` | `ShieldedSyncMonitor` | Platform ADV |
| TESTNET/DEVNET badge; testnet logo; coin sound when balance rises after sync | `HomeView.swift`, `Resources/coinflip.aiff` | — | — |
| DashPay header: username + avatar row, notification bell with unread count → `NotificationsScreen`; avatar → `SDKIdentityProfileSheet` | `S/UI/Home/Views/HomeUsernameRow.swift` | `SwiftDashSDKContactsService.unreadNotificationCount` | DP |
| Sync status: header error states ("Sync Failed" + retry, "Unable to connect"); "Syncing" pill → `SyncingAlertView` with per-phase heights (headers / filter headers / filters / masternode lists), connected peers (Evonode/Masternode/Node), and "Change peers" after a 45 s stall | `S/Application/Syncyng Activity Monitor/SyncingActivityMonitor.swift`, `S/UI/Home/Syncing Views/*`, `S/UI/Home/Views/Cells/SyncingHeaderView.swift` | `SwiftDashSDKSPVCoordinator` (`$connectedPeers`), `SwiftDashSDKWalletRuntime.rotatePeers` | — |
| Exchange-rate toasts: stale (> 30 min), fetch failed, volatile (> 50 % move) | `MainTabbarController.swift` | `BaseRatesProvider.$hasFetchError/$isVolatile` | — |
| **Shortcuts bar**: 4 slots, customisable by long-press (`ShortcutSelectionView`), defaults depend on balance + backup state. Actions: Backup, Receive, Send, Scan QR, Send to Address, Buy&Sell, Explore, Spend, ATM, Coinbase, Uphold, Topper, Dash DEX (mainnet + key), CrowdNode (if account), 1 tDash faucet (testnet, in-app PoW against `faucet.thepasta.org`), Switch Wallet (>1 wallet), Nodes (wallet owns evonodes; live epoch-blocks icon) | `S/UI/Home/Views/Shortcuts/*`, `HomeViewController+Shortcuts.swift`, `ShortcutAction.swift` | UD `DW_GLOB_shortcuts`, `shortcutBannerState`; SDK `TestnetFaucet` | per action |
| Time-skew dialog (device clock vs NTP / HTTP Date) | `S/Utils/TimeUtils.swift` | `pool.ntp.org`, `www.dash.org`, `insight.dash.org` | — |
| Jailbreak warning | `HomeViewController+JailbreakCheck.swift` | — | iOS-only, N/A on desktop |
| Join DashPay banner (Join / Upgrade / Finish registration / voting / failed…) | `S/UI/DashPay/Setup/CreateUsername/JoinDashPayView.swift` | — | DP |
| CoinJoin "move mixed coins" prompt, CrowdNode balance reminder, gift-card sheet, internal-transfer result toast | see §1.8, §1.14, §1.15 | — | — |

### 1.4 Transaction history, details, metadata, taxes, export

| Feature | Files | Deps | Gate |
|---|---|---|---|
| History feed grouped by day ("Date unknown" bucket for restored shielded items); paged in whole-day windows (~100 rows) with "Loading more"; throttled delta reloads | `S/UI/Home/Views/HomeViewModel.swift` (`SwiftDashSDKWalletSource`), `TransactionListDataItem.swift`, `S/Models/Tx/GroupedTransactions.swift` | SD `PersistentTransaction`, `PersistentTxo`, `PersistentShieldedActivity` | — |
| Row kinds: tx, shielded activity, Platform-address activity, CrowdNode group, CoinJoin mixing (one per day), CoinJoin withdrawals (combined) | `S/Models/CoinJoin/CoinJoinMixingTxSet.swift`, `CoinJoinWithdrawalTxSet.swift`, `CoinJoinWithdrawalStore.swift`, `S/UI/Home/Views/ShieldedActivityHistory.swift`, `PlatformAddressHistory.swift`, `S/Models/PlatformAddress/PlatformAddressActivityStore.swift` | — | — |
| Filters: multi-select with All and per-row "Only". Sent, Received, Rewards (only if any coinbase), Masternode (only if any ProTx), Gift card, Shielded sent, Shielded received | `S/UI/Home/Views/TransactionFilterDialog.swift`, `TransactionFilterCategory` in `HomeViewModel.swift` | — | — |
| Row content: title from metadata, time, signed DASH, fiat. Icon priority: merchant logo > metadata icon > gift card > mining > direction. Contact avatar/name for DashPay payments. Route labels for internal transfers ("Transparent → Shielded"). Status Locked / Pending / "Pending — tap to finish" | `HomeView.swift`, DashUIKit `TransactionView` | `DashPayPaymentTxLookup` | contact part DP |
| **Tx details sheet**: amount header; From/To route + lock status for asset-lock fundings; sent-from/to, received-at, moved, registered-from addresses; masternode owner/provider/voting addresses; contact row; fee (from Insight if unknown); date; tax category. Actions: Rebroadcast / Complete Transfer (stuck asset lock), "Remove if Not on Network", raw tx view / copy hex, open in explorer (Insight / Blockchair), Maya / NEAR explorer for swaps, copy txid | `S/UI/Tx/Details/TxDetailViewController.swift`, `Model/TxDetailModel.swift`, `RawTransactionView.swift`, `BlockExplorerSelectionView.swift`, `TxDetailContactViews.swift` | HTTP `insight.dash.org/insight-api`, `insight.testnet.networks.dash.org/insight-api`, `blockchair.com`; `AssetLockRecoveryService`, `UnconfirmedTransactionRemover` | — |
| Shielded activity details (decodes 36-byte Dash text memo), Platform-address activity details | `ShieldedActivityDetailsView`, `PlatformAddressActivityDetailsView` | — | — |
| **Tax categories**: Transfer/unknown, Income, Transfer In, Transfer Out, Expense, Internal Transfer. Tapping cycles Income ↔ Transfer In, Expense ↔ Transfer Out. Integrations pre-tag addresses. One-time "Reclassify your transactions" intro | `S/Models/Taxes/Taxes.swift`, `S/Models/Tx Metadata/TransactionMetadata.swift`, `S/UI/Tx/Reclassify Transactions/*` | SQL `tx_userinfo.taxCategory`, `address_userinfo` | — |
| Tx metadata providers (priority: GiftCard, Coinbase, CustomIcon, SwapOrder); service names crowdnode/uphold/coinbase/ctxspend/piggycards; merchant icons cached in SQL; **historical fiat rate stamped per tx** | `S/UI/Home/Tx Metadata/*`, `S/Models/Tx Metadata/{ServiceName,IconBitmap}.swift`, `S/Models/Tx/Transactions.swift` (`updateRateIfNeeded`) | SQL `tx_userinfo` (`rate`, `rateCurrencyCode`, `rateMaximumFractionDigits`, `timestamp`, `memo`, `service`, `customIconId`), `icon_bitmaps` | — |
| Private memo: column exists, **no editing UI** | — | SQL `tx_userinfo.memo` | — |
| **CSV export** (Tools): requires sync done; excludes mixing and moves. Columns: Date and time, Transaction Type, Sent Quantity, Sent Currency, Sending Source, Received Quantity, Received Currency, Receiving Destination, Fee, Fee Currency, Exchange Transaction ID, Blockchain Transaction Hash. File `report-<date>.csv` → share sheet | `S/Utils/CSVBuilder.swift`, `S/Models/Taxes/Services/TaxReportGenerator.swift`, `S/UI/Menu/Tools/CSVExportSheet.swift` | — | — |
| ZenLedger export: OAuth client-credentials; POSTs every address the wallet received to; opens the returned signup URL | `S/Models/Taxes/ZenLedger.swift`, `S/UI/Menu/Tools/ZenLedger/*` | HTTP `api.zenledger.io` (`/oauth/token`, `/aggregators/api/v1/portfolios/`); `ZenLedger-Info.plist` `CLIENT_ID/CLIENT_SECRET` | — |
| Remove an unconfirmed tx (checks Insight, deletes local rows, frees inputs, rescans ≥ ~30 h of filters); bulk "drop all unconfirmed & rescan" | `S/Infrastructure/SwiftDashSDK/UnconfirmedTransactionRemover.swift` | Insight | — |

### 1.5 Send

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Payments sheet → Send card: Send to username (DP + identity), Send to Dash address, Scan Dash QR, Swap to other crypto (mainnet + SwapKit key) | `S/UI/Payments/Landing/*` (`PaymentsLandingHostingController`, `PaymentsLandingScreen`, `PaymentsLandingViewModel`) | — | partly DP |
| Step 1, address: type / paste / "Send to copied address" clipboard suggestion / QR. Classifier: Base58 → Core; bech32m `dash`/`tdash` 21-byte → Platform; `0x10`+43 bytes → Shielded (Orchard) | `S/UI/Payments/Pay/SendScreen.swift`, `SendViewModel.swift`, `SendScreenViewController.swift`, `S/UI/Payments/PaymentModels/DWParsedPaymentURI.swift` (`DashAddressClassifier`), `DWPasteboardAddressExtractor` | — | — |
| Step 2, amount: source picker filtered by destination; keypad with **DASH ↔ fiat** toggle; fee-aware **Max** per route; show/hide balance | `ExternalSendAmountScreen`, `S/UI/Payments/Amount/*`, `S/UI/Payment Controller/Enter Amount/*` | `SwiftDashSDKWalletState.$balance`, `CurrencyExchanger` | — |
| Routes: core→core, core→shielded, platform→platform, platform→core, platform→shielded, shielded→core, shielded→platform, shielded→shielded | `SendViewModel.Route` | — | Platform routes ADV |
| Core→Core: confirm sheet (address, exact FFI fee, total, "Sending…") → **build+sign without broadcast → broadcast only on Confirm**. Fee is a fixed 1 duff/byte; no user fee choice | `S/UI/Payments/PaymentModels/DWPaymentProcessor.m`, `S/UI/Payment Controller/PaymentController.swift`, `ConfirmPaymentViewController`, `S/Models/Transactions/WalletSendService.swift`, `Infrastructure/SwiftDashSDK/SwiftDashSDKTransactionSender.swift` | SDK `CoreTransactionBuilder`, `broadcastTransaction` | — |
| Other routes: `SendConfirmSheet` with step progress (Authorizing → Locking funds → Generating proof → Broadcasting) and "submitted but unconfirmed" state | `S/UI/Payments/InternalTransfer/Shielded/ShieldedTransferCoordinator.swift`, `Infrastructure/SwiftDashSDK/PlatformSendExecutor.swift`, `PlatformAddressSyncCoordinator.transfer/withdraw` | SDK `shieldedShieldToRecipient`, `shieldedTransfer`, `shieldedUnshield`, `shieldedWithdraw`, `shieldedFundFromAssetLock`, `addressWallet.transfer/withdraw` | — |
| Pay to DashPay contact: contact picker → amount → `sendDashPayPayment` (DIP-15). Core source only. Per-contact lock after an unknown broadcast outcome | `S/UI/Payments/Pay/SendToContactScreen*`, `WalletSendService.sendToContact` | SDK | DP |
| `dash:` / `pay:` / `dashwallet://` URIs; BIP21 params amount, label, message, r (BIP72), sender, user, currency, local, `req-*` | `S/Models/URL Handling/DWURLParser.m`, `DWURLRequestHandler.m`, `S/Models/PaymentProtocol/BIP70URI.swift`, `PaymentURIBuilder.swift` | — | — |
| **BIP70**: fetch → X.509 verify → network/expiry check → confirm → build/sign → broadcast → POST Payment → await ACK. Required by CTX gift cards | `S/Models/PaymentProtocol/*` (`BIP70PaymentService`, `PaymentRequestVerifier`, `PaymentProtocolTransport`, `Codec/*`), `Infrastructure/SwiftDashSDK/BIP70PaymentService+App.swift`, `BIP70SendAuthorizer.swift`, `BIP70_TESTING.md` | merchant `r=` URLs | — |
| Guards: blocked during initial post-restore sync, offline, or sync-blocked; copy for insufficient funds / balance or fee unavailable / broadcast rejected / unknown outcome / shielded too-small, immature or bundle-ceiling | `SendViewModel.swift`, `WalletSendService.swift` | — | — |
| InstantSend: automatic; IS/ChainLock status read from the SDK context byte and shown on receipts. No user toggle | — | SDK | — |
| NFC pay (`DWPayModel payWithNFC`) | `S/UI/Payments/Pay/Models/DWPayModel.m` | CoreNFC | iOS-only, N/A |
| Post-send success tx details | `SuccessTxDetailViewController` | — | — |

### 1.6 Receive

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Receive tab with a balance toggle: Core and Shielded always, Platform in ADV | `S/UI/Payments/Landing/PaymentsReceiveContent.swift`, `PaymentsLandingViewModel.swift` | — | Platform ADV |
| Addresses: Core = next unused BIP44 (rotates); Platform = next DIP-17 receive address; Shielded = deterministic Orchard default address (never rotates). Placeholders while Platform/shielded start | `Infrastructure/SwiftDashSDK/SwiftDashSDKReceiveAddressReader.swift`, `SwiftDashSDKReceiveAddressProvider.swift`, `PlatformAddressSyncCoordinator.derivedAddresses` | SDK `shieldedDefaultAddress` | — |
| Styled QR, tap/copy address ("Copied" toast), Share address | `QRCodeGenerator` | — | — |
| **Live payment watcher** ("Watching for a payment…") → Received card (amount, status e.g. "InstantSend confirmed", time, shielded memo, View transaction, Done / **Receive another** which rotates the address) | `PaymentsReceiveContent.swift` | Core tx stream, Platform activity, shielded projection polled every 5 s | — |
| **Request a specific amount** (Core): QR with amount, address, username (DP), Share, Copy QR, paid detection | `S/UI/Payments/Receive/RequestAmount/RequestAmountHostingController.swift`, `RequestAmountScreen.swift` | `PaymentURIBuilder`, `receivedTotal(excludingAddress:)` | — |

### 1.7 Sweep paper wallet / import private key — REMOVED

- Dropped during the migration ("C8 step 5"): `S/UI/Payments/PaymentModels/DWPaymentProcessor.m:~189`, `S/UI/Payments/ScanQR/DWQRScanModel.m:~197`, `DWPaymentInputBuilder.m`.
- A scanned WIF key is rejected as "Not a valid Dash address". The `importPrivateKey` shortcut is a no-op, and the receive screen's import button is never wired.
- Bringing it back needs an SDK function that spends UTXOs at arbitrary addresses. **Parity decision needed** (the Android/legacy app had it).

### 1.8 CoinJoin

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Mixing UI (modes, progress): **discontinued**. "CoinJoin is no longer supported" | — | — | — |
| One-time recovery scan with a wide gap limit (100) per network to find mixed coins | `S/Models/CoinJoin/CoinJoinRecovery.swift` | SDK; UD flag | — |
| CoinJoin balance | `Infrastructure/SwiftDashSDK/SwiftDashSDKCoinJoinBalanceReader.swift` | `SwiftDashSDKWalletState.coinJoinBalanceDuffs` | — |
| **Move mixed coins**: post-sync prompt (dialog or `CoinJoinMoveFundsSheet` with a choice of Dash Wallet or Shielded). Sweep in chunks of ≤ 500 inputs, may partially succeed. "Later" remembered per balance. Permanent entries in Settings and Tools. Threshold > 1000 duffs | `HomeViewController.showCoinJoinSweepDialog`, `CoinJoinMoveFundsSheet/ViewModel/DestinationPolicy`, `WalletSendService.sweepCoinJoin`, `SwiftDashSDKTransactionSender.sweepCoinJoin` | SDK `shieldedFundFromCoinJoinDrain` | — |
| History grouping of mixing / withdrawal txs | §1.4 | `CoinJoinWithdrawalStore` (UD, per wallet) | — |

### 1.9 Shielded pool (Orchard), Platform addresses, internal transfer, "advanced mode"

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Shielded balance per wallet/network, sync spinner, re-sync on start / reconnect / foreground | `Infrastructure/SwiftDashSDK/ShieldedBalanceController.swift`, `ShieldedSyncMonitor.swift`, `ShieldedRecoveryController.swift` | SDK `syncShieldedNow`, `shieldedSyncIsSyncing` | — |
| Shield from Core (asset lock + type 18), from Platform (type 15, `shieldedShieldPreflight`), from CoinJoin | `ShieldedTransferCoordinator.swift` | SDK | Platform ADV |
| Unshield / withdraw to Core, send to Platform, shielded→shielded, fee-aware **Max** that picks notes to fit the bundle ceiling | same | SDK `estimateShieldedFee` | — |
| Finish stuck shielded transfer (`ShieldedRecoverySheet` → `shieldedResumeFundFromAssetLock`) from a "Pending — tap to finish" row | `S/UI/Home/...ShieldedRecoverySheet` | SDK | — |
| **Internal Transfer** screen: move between Core ↔ Shielded ↔ Platform ↔ DashPay identity credits (top-up via `topUpIdentityWithFunding`; withdraw credits to Core / transfer to own Platform address). Privacy tip; timing sheet ("Shielded → Dash Wallet up to 10 minutes") | `S/UI/Payments/InternalTransfer/*` (`InternalTransferScreen`, `InternalTransferViewModel`, `Confirm/InternalTransferConfirmSheet`, `InternalTransferRunner`, `IdentityWithdrawViewModel`) | SDK | Core↔Shielded always; Platform/identity ADV |
| Platform (DIP-17) address balance + BLAST L2 sync | `PlatformAddressSyncCoordinator.swift` (2287 lines; god object per arch review), `PlatformBalanceController.swift`, `PlatformBalanceReader.swift`, `PlatformCreditsFormatter.swift` (1000 credits = 1 duff) | SD `Documents/SwiftDashSDK/Platform/<network>/` | ADV |
| Advanced mode toggle + info sheet (Shielded / Identity / Platform) | `S/UI/Menu/Settings/Components/AdvancedModeInfoSheet.swift` | UD | — |

### 1.10 DashPay — usernames, identity, contacts, profiles, invitations

| Feature | Files | Deps | Gate |
|---|---|---|---|
| Join DashPay intro, voting info, **shielded-funding readiness checklist**, info dialog | `S/UI/DashPay/Setup/CreateUsername/{JoinDashPayScreen,VotingInfoScreen,JoinDashPayReadinessScreen,JoinDashPayInfoDialog,JoinDashPayView,JoinDashPayViewModel}.swift` | — | DP |
| Username form: 3–23 characters `[A-Za-z0-9-]`, no leading/trailing hyphen; debounced (0.4 s) availability check; contested detection (≤ 19 characters, letters/digits/hyphen, FFI `dash_sdk_dpns_is_contested_username`); contest precheck (locked / active contest / fresh); **buy directly if listed < 10 DASH** | `CreateUsernameViewController.swift`, `CreateUsernameViewModel.swift`, `S/UI/DashPay/Setup/Model/UsernameValidationRuleResult.swift` | SDK `dpnsCheckAvailability`, `UsernameMarketplaceService.ContestPrecheck`, `purchaseDpnsName` | DP |
| Cost: 0.03 DASH non-contested, 0.25 contested. Shielded funding uses fixed 0.1 / 0.25 DASH exits; notes must rest 3 h | `S/UI/DashPay/DWDashPayConstants.m`, `ShieldedIdentityFundingReadiness.swift` | — | DP |
| Registration pipeline: PIN gate → pre-persist keys → fund identity from **Core asset lock / Platform addresses / Shielded pool / invitation** → `registerDpnsName` (preorder + register). Resumable after a process kill; drafts per network/wallet/identity | `Infrastructure/SwiftDashSDK/Identity/DWIdentityRegistrationCoordinator.swift`, `DWIdentityRegistrationBridge.swift`, `DWRegistrationPhaseAdapter.swift`, `DWDashPayIdentityKeys.swift`, `DWUsernameRegistrationRecovery.swift` | SDK `registerIdentityWithFunding`, `registerIdentityFromAddresses`, `shieldedIdentityCreateFromPool`, `claimInvitation`, `createIdentityAndUsername`, `resumeUsernameRegistration` | DP |
| Contested name waiting period: pending name hidden from display until resolved; win → finalize, loss/lock → clear; optional **temporary non-contested username** alongside; request status screen with contenders, tallies, lock votes, deadline | `DWContestedNameStatusService.swift`, `S/UI/DashPay/Usernames/UsernameRequestStatusScreen.swift` | SDK `fetchContestVoteState`, `syncContestedDpnsNames`; UD per network/wallet | DP |
| Asset-lock recovery (Rebroadcast button) and probe store | `AssetLockRecoveryService.swift`, `AssetLockProbeStore.swift` | SDK `resumeTopUpWithAssetLock`, `resumeAssetLock` | — |
| **Identity profile sheet**: avatar, usernames, credit balance, owned DPNS names, pending contest, Edit Profile, marketplace link, **Top Up** (Core / Platform / Shielded) | `S/UI/DashPay/Profile/SDKIdentityProfileSheet.swift` (`IdentityTopUpViewModel`) | SDK `topUpIdentityWithFunding`, `topUpFromAddresses` | DP |
| **Identities screen** (More → Identities): list (User / Masternode / Evonode / Observed), set main, copy ID, keys (purpose, security level, contract bounds, path), refresh keys, find identities, get username | `S/UI/Menu/Security/Wallets/IdentitiesScreen.swift`, `IdentitiesViewModel.swift` | SDK | ADV |
| **Contacts tab**: my contacts, pending requests, search, hidden contacts, my-identity card; **Enable DashPay** (adds missing ENCRYPTION/DECRYPTION keys); FAQ | `S/UI/DashPay/Contacts/SwiftUI/ContactsScreen.swift` | `Infrastructure/SwiftDashSDK/Contacts/SwiftDashSDKContactsService.swift` | DP |
| Add contact: username search (≥ 2 characters), My QR / Scan QR (`dashpay://user?id=…&username=…`), send request | `AddContactScreen.swift`, `DashPayUserLink.swift` | SDK `searchDpnsNames`, `resolveDpnsName`, `sendContactRequest` | DP |
| Contact profile: accept / ignore, Pay, per-contact payment activity, alias / note / hide | `ContactProfileSheet.swift`, `ContactAvatarView.swift` (letter-placeholder colours identical to Android) | SDK `acceptContactRequest`, `ignoreContactSender`, `setDashPayContactInfo` | DP |
| Notifications screen (bell): New / Earlier / Pending, search, accept inline | `NotificationsScreen.swift`, `Infrastructure/Notifications/DashPayNotificationsReadState.swift` | SD contact-request rows; Rust DashPay sync loop every 15 s (`startDashPaySync`) | DP |
| **Edit profile**: display name ≤ 25, about ≤ 250, avatar from **Gravatar / public URL (≤ 256) / camera / photo library** with crop + face detection, upload | `S/UI/DashPay/Profile/Edit Profile/RootEditProfileViewController.swift` → legacy `DashPay/Presentation/Profile/EditProfile/*`, `Infrastructure/Networking/DWAvatarUploadClient.swift`, `Identity/DWProfileUpdateCoordinator.swift` | HTTP `api.imgur.com` (`POST /3/upload`, `DELETE /3/image/<hash>`, `Imgur-Info.plist` `IMGUR_CLIENT_ID`), `www.gravatar.com`; SDK `createDashPayProfile/updateDashPayProfile` (URL + SHA-256 + dHash) | DP |
| **Claim invitation** (invitee only): paste / scan / universal link; inviter preview; already-claimed check; continue to username; auto contact request to inviter | `S/UI/DashPay/Invitations/ClaimInvitationScreen.swift`, `Infrastructure/SwiftDashSDK/Invitations/DWInvitationService.swift`, `DWInvitationLinkNormalizer.swift` | SDK `parseInvitation`, `claimInvitation`; links `dashpay://invite?du&assetlocktx&pk&islock…`, `https://invitations.dashpay.io/applink?…`, AppsFlyer OneLink wrappers (parsed only) | DP |
| Create / share / reclaim invitations | **Removed** (2026-07-29 scope decision; `INVITATIONS_REBUILD_PLAN.md` superseded). The SDK still has `createInvitation` | — | — |
| DashPay sync diagnostics | `S/UI/Menu/SyncInfo/DashPaySyncInfoScreen.swift` | — | DP |

### 1.11 Masternode voting, governance, masternode tooling

| Feature | Files | Deps | Gate |
|---|---|---|---|
| **Governance menu**: Masternodes + Voting. **No budget proposals / proposal voting** | `S/UI/Menu/Governance/GovernanceMenuScreen.swift`, `GovernanceMenuViewModel.swift` | — | — |
| **Contested-username voting**: open contests (search; sort by ending soonest / name / most votes / most lock votes), contest detail, cast Approve contender / Abstain / Lock, **bulk vote**, voting-node selection (default single node for privacy), local vote history | `S/UI/DashPay/Voting/{UsernameVotingScreen,ContestDetailScreen,CastVoteSheet,BulkVoteSheet,VotingNodeSelectionSheet,VotingViewModel}.swift`, `Infrastructure/SwiftDashSDK/Voting/{ContestedNamesService,MasternodeVoteCaster,MasternodeVoterRegistry}.swift`, `S/Models/Voting/VoteHistoryDAO.swift` | SDK `dpnsActiveContests`, `dpnsContestVoteState`, `castContestedResourceVote`; SQL `masternode_vote_history`; evonode weight 4× | DP + `votingEnabled` |
| **Masternodes** list (wallet-owned): Active / PoSe banned / Retired; detail with service address, keys this wallet owns, collateral, claimable Platform balance; evonode blocks proposed this epoch | `S/UI/Menu/Tools/MasternodesScreen.swift`, `Infrastructure/SwiftDashSDK/Masternodes/*` (`EvonodeEpochBlocksService`) | SDK `PlatformWalletManager.masternodes`, `fetchClaimableBalance` | — |
| Evonode status request | `S/UI/Menu/Tools/Evonode Status/*` | SDK `getEvonodeStatus` | — |
| Evonode credit withdrawal (with key preflight) | `S/UI/Menu/Tools/Masternode Withdrawal/*` | SDK `masternodeWithdraw`, `masternodeWithdrawalKeys` | — |
| **Unban** (ProUpServTx) with fee-funding preflight, shielded top-up path, persistent pending state | `S/UI/Menu/Tools/Unban/*` | SDK `masternodePrepareUpdateService`, `masternodeUpdateService` | — |
| **Track any masternode** by IP / proTxHash / key; attach owner / voting / payout / BLS / node keys (keychain vault); tracked withdraw / unban | `S/UI/Menu/Tools/Tracked Masternodes/*`, `Masternodes/TrackedMasternodeKeyVault.swift` | SDK `locateMasternode`, `trackMasternode`, `trackedMasternodeWithdraw`; KC | — |
| **Masternode Keychain** (auth): Owner / Voting / Operator (BLS) / Evonode operator (ed25519) per index. Shows address, private key, WIF, pubkey (and legacy form), Platform node ID, Tenderdash node key, "Used at ip:port" / revoked | `S/UI/Menu/Tools/Masternode Keys/*` | SDK derivation | — |

### 1.12 Username marketplace

- **Tabs:** Find Names / My Names / Browse / Purchases.
- **Features:** search with sale state, buy, list for sale, change price, remove from sale, free transfer to a username or identity ID, trade history, request a contested name (join as a contender). Paid from identity credits. Typed errors: notForSale, priceChanged, insufficientIdentityCredits, contestedNameNotTradable.
- **Files:** `S/UI/Explore Dash/UsernameMarketplaceScreen.swift`, `Infrastructure/SwiftDashSDK/UsernameMarketplaceService.swift`. Reached from Explore, Identities and the profile sheet.
- **SDK calls:** `searchDpnsMarketplace`, `myDpnsMarketplaceNames`, `dpnsMarketplaceNameState`, `dpnsNameHistory`, `purchaseDpnsName`, `setDpnsNamePrice`, `delistDpnsName`, `transferDpnsName`, `syncDpnsMarketplace` / `startDpnsSync`, `dpnsGetCurrentContests`, `documentList`.
- **Gate:** not compile-gated; needs an identity at runtime.

### 1.13 DashConnect (dApp login) and tokens

- **DashConnect** is QR / deep-link login and authorization for third-party Platform dApps.
  - Payloads: `dash-key:` (derive contract-bound login keys, write a `loginKeyResponse` document) and `dash-st:` (a serialized state transition: IdentityUpdate adding login keys, or a **token direct purchase**). 4 KiB URI ceiling.
  - Approve-connection sheet lists permissions ("See your username", "Verify your identity").
  - Connections list per wallet and network.
  - Files: `S/Models/DashConnect/*`, `S/UI/DashConnect/*`.
  - Entry: Tools → Connections, QR scan, or `dash-key:` / `dash-st:` deep link.
  - SDK: `deriveIdentityAuthKeyAtSlot`, `parseStateTransition`, `updateIdentity`, `createDocument`, `replaceDocument`, `tokenPurchase`, `dataContractGet`, `calculateTokenId`.
  - Gate: **test networks only** (pinned testnet contract; devnet contract id set in Devnet Settings; a mock on mainnet). Known app: Yappr.
- **Tokens:** the only token UI is `ApproveTokenPurchaseSheet` (quantity, token ID, maximum price; refuses a mismatched identity or token). There are **no token balances, lists or transfers**. Token SwiftData rows are visible only in the debug Storage Explorer.

### 1.14 Buy / Sell and financial integrations

**Buy & Sell portal** (`S/UI/Buy Sell/BuySellPortalView.swift`, `BuySellPortalViewController.swift`, `Model/*`; opens behind PIN/biometrics). Card order: **Topper** ("Powered by Uphold"), **Uphold**, **Coinbase** (not GB), **Dash DEX** (mainnet + key). Maya is hard-hidden. Connected accounts show their DASH balance plus fiat.

| Integration | Files | Hosts / auth | Notable UX | Network |
|---|---|---|---|---|
| **Uphold** | `S/Models/Uphold/*` (`DWUpholdClient`, `UpholdClient.swift` Moya, `DWUpholdConstants.m`), `S/UI/Uphold/*` (Portal, Transfer, Transfer/OTP, LogoutTutorial) | `api.uphold.com` / `api-sandbox.uphold.com`. OAuth code flow in an auth session, callback scheme `dashwallet`. `Uphold-Info.plist` `CLIENT_ID`, `CLIENT_SECRET`, `AUTHORIZE_URL_FORMAT`, `SANDBOX_CLIENT_SECRET`. Token in KC `DW_UPHOLD_ACCESS_TOKEN` | Buy (opens Topper); Transfer Dash to wallet (amount, max, OTP via `OTP-Token` header, fee-deduct fallback, commit, "See on Uphold"); KYC capability errors; logout tutorial | mainnet + sandbox |
| **Topper** | `S/Models/Uphold/Topper.swift`, `SupportedTopperAssets.swift`, `S/UI/Uphold/TopperViewModel.swift` | Widget `app.topperpay.com/?bt=<JWT>` (sandbox `app.sandbox.topperpay.com`); metadata `api.topperpay.com/assets/crypto-onramp`, `/payment-methods/crypto-onramp`. **ES256 JWT signed in-app** with a private key from `Topper-Info.plist` (`KEY_ID`, `WIDGET_ID`, `PRIVATE_KEY`, `SANDBOX_*`) | Default ~100 USD in the user's fiat; target = wallet receive address; opens in an in-app browser | mainnet + sandbox |
| **Coinbase** | `S/Models/Coinbase/*`, `S/UI/Coinbase/*` | OAuth `login.coinbase.com/oauth2/{auth,token,revoke}`, ephemeral auth session, redirect `dashwallet://brokers/coinbase/connect`; API `api.coinbase.com` with `CB-VERSION: 2021-09-07`; `Coinbase-Info.plist` `CLIENT_ID/CLIENT_SECRET`; tokens + refresh in KC | Buy Dash (market IOC `DASH-USD`, USD cash or bank deposit, 0.6 % fee, min $1.99, auto-transfer to wallet); Transfer to/from Coinbase (min 10,000 duffs, max 1 DASH, 2FA `CB-2FA-TOKEN`); error mapping (KYC, revoked, rate limit…). Convert crypto present but **disabled** (MO-103) | **production on all networks**; GB geoblock |
| **CrowdNode** (staking) | `S/Models/CrowdNode/*`, `S/UI/CrowdNode/*` | `app.crowdnode.io` / `test.crowdnode.io`, `login.crowdnode.io`; **unauthenticated OData** (`apifundings/GetFunds`, `GetBalance`, `GetWithdrawalLimits`, `GetFeeJson`, `apiaddresses/*`, `apimessages/SendMessage`) | **Tx-encoded signup API** (0.009 DASH top-up, signUp `offset+131072`, acceptTerms `offset+65536`, wait for response codes); deposit (min 0.5 DASH); withdraw by a **signed message**, not a tx (DarkCoin message framing, messagetype 4, per-tx/hour/day limits); online-account link (web view + 54,321-duff confirmation tx QR); email registration; APY estimate (`MasternodeAPYCalculator`); balance reminder; grouped tx rows | mainnet + test. **CLAUDE.md says CrowdNode is temporarily suspended/hidden for the migration release**; the Explore "Staking" row shows only for existing accounts |
| **Dash DEX** (SwapKit) + **Maya** routes | `S/Models/{Swap,SwapKit,Maya}/*`, `S/UI/{Swap,SwapKit,Maya}/*` (`SwapFlowCoordinator`), `MAYA.md` | `api.swapkit.dev` (`x-api-key` from `SwapKit-Info.plist` `API_KEY`); Maya `midgard.mayachain.info/v2`, `mayanode.mayachain.info/mayachain`; explorers `mayascan.org`, `explorer.near-intents.org`; coin icons from SwapKit GCS / `assets.coincap.io` / GitHub | **Sell** DASH→X: select coin (~130 curated assets, halted chains greyed), enter address (paste / clipboard / QR / **pull address from linked Coinbase or Uphold**), amount in fiat / DASH / coin, order preview with **10 s countdown** and re-quote, DASH tx with OP_RETURN memo (Maya) or plain deposit (NEAR), status. **Buy** X→DASH: coin, amount, refund address, deposit QR. Orders saved in SQL `swap_orders`, polled every 30 s, expire after 24 h, local notifications; `SwapPendingGate` waits for the previous swap's IS lock | **mainnet only** + key |

### 1.15 Explore Dash, ATMs, DashSpend gift cards

| Feature | Files | Deps | Gate |
|---|---|---|---|
| **Explore menu**: Where to Spend (merchants), ATMs, Staking (CrowdNode), Username Marketplace, Buy & Sell, Get Test Dash (testnet: copy address + open `faucet.testnet.networks.dash.org`) | `S/UI/Explore Dash/ExploreMenuScreen.swift`, `ExploreViewController.swift` | — | tab DP + identity; also via More |
| **Merchant/ATM DB sync**: **Firebase Storage** `gs://dash-wallet-firebase.appspot.com/explore/explore-v4.db` (testnet `explore-v4-testnet.db`). Metadata `Data-Timestamp`/`Data-Checksum` → download zip (password = checksum) → SSZipArchive → `Documents/explore.db`. On launch, every 24 h, and on network switch. Bundled seed `DashWallet/explore.db` (old schema) | `S/Models/Explore Dash/Services/ExploreDatabaseSyncManager.swift`, `Infrastructure/Database Connection/ExploreDatabaseConnection.swift`, `DAO Impl/{MerchantDAO,AtmDAO}.swift` | Firebase (`GoogleService-Info.plist`), UD sync keys | — |
| **Merchants**: Online / Nearby / All; map + paged list; FTS search; location-off cell; all locations of a merchant; one-time merchant-types dialog | `S/UI/Explore Dash/Merchants & ATMs/List/*`, `Views/ExploreMapView.swift` | MapKit, CoreLocation (`DWLocationManager`) | — |
| **ATMs**: All / Buy / Sell / Buy & Sell; search by name/manufacturer | `.../List/*` (`AtmListViewController`) | — | — |
| **Filters**: payment method (Dash / CTX gift card / PiggyCards), sort (distance / name / discount), radius 1/5/20/50 mi, territory, denomination type (fixed / flexible) | `.../Filters/*` (`PointOfUseListFiltersModel`, `MerchantFiltersView`, `TerritoryPickerView`) | — | — |
| **POI details**: address, phone, website, directions, all locations, Pay with Dash, Buy/Sell (ATM), **Buy a Gift Card** with a provider picker sorted by discount; logged-in-as / log out; temporarily unavailable; US-only notice | `.../Details/*` (`POIDetailsView/ViewModel`) | — | — |
| **DashSpend — CTX**: email OTP login (`POST login` → `verify-email`, refresh tokens), live merchant discount/limits, `POST gift-cards` → **BIP70 payment URL** paid via `SendCoinsService.payWithDashUrl` | `S/Models/Explore Dash/Services/DashSpend/CTX/*`, `Model/DashSpend/CTXConstants.swift` | `spend.ctx.com` / `staging.spend.ctx.com`, header `X-Client-Id`; KC `ctx_spend_*` | — |
| **DashSpend — PiggyCards**: email OTP signup (`signup` → `verify-otp` → generated password in KC → `login`, token 1 h), brands, gift cards, `POST orders` → pay `payTo` with a plain send; 1.5 % fee; max 2,500 | `.../DashSpend/PiggyCards/*` | `api.piggy.cards/dash/v1` (production on all networks) | `PIGGYCARDS_ENABLED`; RU/CU geoblock via GPS or `ip-api.com` |
| Purchase UX: provider → login → terms → amount (flexible min/max or **denomination chips** with per-denomination discount and stock) → confirm ("$X card for $Y, Z % off") → auth → pay → save card + tx metadata | `S/UI/Explore Dash/Views/DashSpend/*`, `S/UI/SwiftUI Components/MerchantDenominations.swift` | SQL `gift_cards` | — |
| **Gift card details**: polls the provider every 1.5 s with backoff + Retry; number, PIN, **barcode** (generated, or downloaded and decoded with Vision), how-to-use; opened from tx details / home | `.../GiftCardDetails/*` (`GiftCardDetailsViewModel`), `Services/BarcodeScanner.swift` | SQL `gift_cards` (`txId`, `merchantName`, `merchantUrl`, `price`, `number`, `pin`, `barcodeValue`, `barcodeFormat`, `note`, `provider`, `redeemUrlChallenge`) | — |

### 1.16 More menu: Settings, Security, Wallets, Tools, Sync Info, About, Support

**Main menu** (`S/UI/Menu/Main/MainMenuViewModel.swift`, `MainMenuViewController.swift`):
- Rows: Explore · Sync Info · Wallets (ADV) · Identities (ADV) · Security · Settings · Tools · Support · Governance.
- DashPay header (DP): Join banner / profile, edit profile, claim invitation, username request status.

| Area | Feature | Files | Deps |
|---|---|---|---|
| Settings | **Local currency**: searchable list; default from OS locale, else USD | `S/UI/Menu/Settings/LocalCurrency/*`, `S/Application/App.swift` | UD `LOCAL_CURRENCY_CODE`; rates §2.6 |
| Settings | **Notifications** toggle ("Turned off in iOS Settings" state links to OS settings) | `SettingsMenuViewModel.swift` | UD `localNotificationsEnabled` + OS auth |
| Settings | **Network**: Mainnet / Testnet (/ Devnet in DASH_DEVNET builds) via `SwiftDashSDKWalletRuntime.switchNetwork` under the overlay | `SettingsScreen.swift`, `WalletEnvironment.swift` | UD `CURRENT_CHAIN_TYPE_KEY` |
| Settings | **Devnet settings**: quorum URL (default `quorums.moutai.networks.dash.org`), devnet name, DashConnect contract | `DevnetSettingsScreen.swift`, `Infrastructure/SwiftDashSDK/DevnetConfiguration.swift` | UD |
| Settings | **About**: version + network, Explore DB sync status, last sync, review app, contact support, GitHub; **shake** → tech info (rate, sync heights, MN lists, username), copy / export logs | `About/AboutDashView.swift`, `AboutDashViewModel.swift`, `DWAboutModel.m` | — |
| Settings | Move CoinJoin Funds (conditional), Enable Voting (DP), **Advanced mode** | §1.8, §1.9 | — |
| Settings | **No language picker** (iOS per-app language is used); **no rescan here** (it lives in Sync Info) | — | — |
| Security | **View recovery phrase** (auth → wallet picker if several, labelled by network) | `S/UI/Menu/Security/RecoveryPhraseFlow.swift` | SDK `WalletStorage` via Host |
| Security | **Change PIN**; **biometrics toggle** (enable → 0.5 DASH limit; disable → 0); **Autohide balance** | `SecurityMenuScreen.swift`, `SecurityMenuViewModel.swift` | KC/UD |
| Security | **Advanced Security**: Auto Logout on/off + timer (Immediately / 1 m / 5 m / 1 h / 24 h); Spending Confirmation on/off + biometric limit (0 / 0.1 / 0.5 / 1 / 5 DASH); security-level meter None…Very High; Reset to Default | `Advanced Security/DWAdvancedSecurityViewController.m`, `Model/DWAdvancedSecurityModel.m` | UD/KC |
| Security | **Reset (wipe) wallet**: wallet count check (0 / >1 / read error) → warning → re-enter a phrase matching **all** stored mnemonics → `SwiftDashSDKWalletWiper` (mnemonics, SwiftData, PIN, options, tx metadata, vote history, Uphold / Coinbase / CrowdNode state) | `ResetWalletInfo/*`, `Infrastructure/SwiftDashSDK/SwiftDashSDKWalletWiper.swift`, `App.cleanUp` | KC/SD/SQL/UD |
| Wallets (ADV) | **Multi-wallet**: list, switch (app reloads), rename, view phrase, remove (type that wallet's phrase), add (create 12/24 words or import); per-wallet accounts screen (balances, address pools, credits) | `S/UI/Menu/Security/Wallets/WalletsScreen.swift`, `WalletsViewModel.swift`; `SwiftDashSDKWalletRuntime.switchWallet/performAddWallet` | SDK, UD active wallet per network |
| Tools | **Extended public key** (BIP44 account 0, QR / copy / share) | `S/UI/Menu/Tools/ExtendedKeys/*` | `SwiftDashSDKHost.derivationWallet()` |
| Tools | Masternode Keychain, Masternodes, Tracked Masternodes (§1.11); CSV Export; ZenLedger (§1.4); **Connections** (DashConnect); Move CoinJoin Funds | `ToolsMenuScreen.swift`, `ToolsMenuViewModel.swift` | — |
| Tools | **Export Logs**: zip of ≤ 3 SDK sessions (`Library/Logs/SwiftDashSDK`, ≤ 15 MB) + DWLogger files + `summary.txt`; share sheet, never uploaded | `S/Infrastructure/DiagnosticLogExporter.swift`, `DWLogger.{h,m}` (CocoaLumberjack) | — |
| Tools | **Storage Explorer** (dev tool, ungated, untranslated, has destructive controls; arch review T20 says gate it) | `S/UI/Menu/Tools/StorageExplorer/*` | SD |
| Sync Info | **Core Sync Status**: per-phase progress, peers, last error, Stop; **Rescan** from wallet creation / from height / full; **edit birth height**; drop unconfirmed + rescan | `S/UI/Menu/SyncInfo/SwiftDashSDKSPVStatusScreen.swift`, `SyncInfoMenuScreen.swift` | `SwiftDashSDKSPVCoordinator` |
| Sync Info | Platform Sync Status (derived addresses + balances, Sync Now / Stop / Clear), DashPay Sync Info (DP), Shielded Sync Info (note counts, Sync Now / Clear) | `PlatformSyncStatusScreen.swift`, `DashPaySyncInfoScreen.swift`, `ShieldedSyncInfoScreen.swift` | SDK |
| Support | Email `support@dash.org` with a zipped log attachment (≤ 25 MB) | `S/Categories/UIViewController+DashWallet.swift` | MFMailCompose |

### 1.17 Notifications (all local; no remote push)

- **Architecture** (`S/Infrastructure/Notifications/`):
  - `NotificationsBootstrap`: composition root.
  - `NotificationDispatcher`: the single poster. Handles the permission gate, dedup, badge, and topics: transactions, dashpay, crowdnode, swap, announcements, system.
  - `NotifiedEventStore`: persisted dedup state.
  - `NotificationLifecycle` + `NotificationRouter`: tap → deep-link route (home, transaction detail, staking, swap order, DashPay notifications, url).
  - `NotificationPermissionCoordinator`: in-app toggle × OS authorization.
- **Producers** (`Producers/*`):
  - Incoming tx "Received %@ (%@)": 10 min freshness, 24 h catch-up.
  - DashPay contact request sent / accepted (DP).
  - Swap complete / refunded / failed.
  - CrowdNode deposit / withdrawal events.
  - **Inactivity reminder** 30 days out, only if the wallet had funds; "Remind me later" / "Don't remind me again".
- **Background:**
  - `BackgroundGraceHold` keeps the process alive 25 s after backgrounding.
  - `BackgroundRefreshCoordinator` is **disabled** (`isEnabledInThisBuild = false`): a locked-device launch can't read the keychain and would offer Create/Recover over a funded wallet. The desktop equivalent is a tray process that only syncs while the secret store is unlocked.
- **CloudKit in-app messaging** (`CloudInAppMessaging`, container `iCloud.org.dash.dashwallet`) shows announcements. Desktop needs another announcement channel or drops it.

### 1.18 Widgets, Today extension, Watch → desktop equivalents

- **Today extension** (`TodayExtension/DWTodayViewController.m`): receive QR + address from app-group defaults, Scan QR, open app. **Not embedded in either app target**, so it does not ship. No WidgetKit widgets.
- **Watch app** (`WatchApp/`, `WatchApp Extension/`, phone side `S/AppleWatch/DWPhoneWCSessionManager.m`): balance (DASH + fiat), recent txs, receive QR, keypad for a fixed-amount request QR, received-payment notices. Embedded in the `dashwallet` target only.
- **Desktop equivalents to build:**
  - Menu-bar (macOS) / tray (Windows/Linux) item: balance (respects autohide), receive address + QR, request amount, "Scan/Pay" and "Open" actions, last transaction.
  - OS notifications for incoming payments.
  - A **Quick Receive available while locked**, mirroring the lock screen.

### 1.19 Localization

- **43 locales** in `DashWallet/*.lproj`: ar, bg, ca, cs, da, de, el, en, eo, es, et, fa, fi, fil, fr, hr, hu, id, it, ja, ko, mk, ms, nb, nl, pl, pt, ro, ru, sk, sl, sl_SI, sq, sr, sv, th, tr, uk, vi, zh, zh-Hans, zh-Hant-TW, zh_TW.
- Source `en.lproj/Localizable.strings` has **~2,476 keys** plus `Localizable.stringsdict` (~154 plural keys). The **English text is the key** (`NSLocalizedString("View Recovery Phrase", …)`).
- Files are UTF-8. The Watch `Interface.strings` are UTF-16LE.
- Synced via Transifex project `dash/dash-mobile-wallets` (`.tx/config`, resources `app-localizable-strings`, `app-localizable-strings-dict`, App Store description). The desktop app can reuse the same Transifex project and English-key strings to get the translations for free.
- Several diagnostic / dev screens (Sync Info, Storage Explorer, the Voting toggle) are untranslated English.
- RTL locales (ar, fa) are present, so the desktop layouts must mirror.

### 1.20 External hosts (complete list for the desktop network allow-list / CSP)

| Host | Purpose | Testnet behaviour |
|---|---|---|
| `rates.ctx.com/rates?source=ctx` | Fiat rates, every 60 s | same |
| `insight.dash.org/insight-api` / `insight.testnet.networks.dash.org/insight-api` | fee lookup, phrase repair, unconfirmed-tx check, explorer links | testnet host |
| `blockchair.com/dash` | explorer link | — |
| `pool.ntp.org`, `www.dash.org` | time skew | — |
| `faucet.thepasta.org` (in-app PoW), `faucet.testnet.networks.dash.org` | testnet faucets | testnet only |
| `api.zenledger.io`, `app.zenledger.io` | tax export | no check |
| `api.uphold.com`, `wallet.uphold.com` / `api-sandbox.uphold.com` | Uphold | sandbox |
| `app.topperpay.com`, `api.topperpay.com` / `app.sandbox.topperpay.com` | Topper | sandbox |
| `login.coinbase.com`, `api.coinbase.com` | Coinbase | production |
| `app.crowdnode.io`, `login.crowdnode.io`, `knowledge.crowdnode.io` / `test.crowdnode.io`, `logintest.crowdnode.io` | CrowdNode | test host |
| `api.swapkit.dev`, `midgard.mayachain.info`, `mayanode.mayachain.info`, `mayascan.org`, `explorer.near-intents.org`, `storage.googleapis.com/token-list-swapkit`, `assets.coincap.io`, `raw.githubusercontent.com/jsupa/crypto-icons` | Dash DEX | mainnet only |
| `firebasestorage.googleapis.com` (bucket `dash-wallet-firebase.appspot.com`) | Explore DB | testnet DB file |
| `spend.ctx.com` / `staging.spend.ctx.com` | CTX gift cards | staging |
| `api.piggy.cards` | PiggyCards | production |
| `ip-api.com` | geo-restriction fallback | — |
| `api.imgur.com`, `www.gravatar.com`, arbitrary avatar URLs | DashPay avatars | — |
| `invitations.dashpay.io`, `dashpaytest.onelink.me` | invitation universal links | — |
| BIP70 merchant `r=` URLs, merchant logo URLs | payments / icons | — |
| `quorums.*.networks.dash.org` | devnet quorum service | devnet |
| Platform DAPI / Core P2P | everything else, via the SDK | — |

URL schemes registered: `dash`, `pay`, `dashwallet`, `dashpay`, `dashid`, `dash-key`, `dash-st`. Universal links: `invitations.dashpay.io`, `dashpaytest.onelink.me`. The desktop app must register OS protocol handlers for the same schemes.

### 1.21 Persistence map

| Store | Contents |
|---|---|
| Keychain service `org.dashfoundation.dash` (this-device-only) | PIN + lockout counters, legacy mnemonics, spend limits, lock-screen flag, Uphold token, Coinbase tokens, CTX / PiggyCards credentials |
| SDK `WalletStorage` (keychain, `WhenUnlockedThisDeviceOnly`) | mnemonics keyed by wallet id. **No PIN encryption of the seed**: the OS keychain is the security boundary |
| Tracked-masternode key vault (keychain) | keys keyed by (network, proTxHash, role) |
| SDK SwiftData (per network) | wallets, txs, TXOs, identities, contacts, profiles, Platform addresses, shielded activity, asset locks, tokens, invitations |
| App SQLite `Documents/store.db` (SQLite.swift + SQLiteMigrationManager, timestamped migrations) | `tx_userinfo`, `address_userinfo`, `username_requests`, `masternode_vote_history`, `gift_cards`, `icon_bitmaps`, `swap_orders` |
| `Documents/explore.db` | merchants / ATMs / gift-card providers (FTS4) |
| UserDefaults | `DW_GLOB_*` options (`DWGlobalOptions`: backup flags, biometrics, auto-lock, shortcuts, notifications, balance hidden, advanced mode, onboarding, payments tab, DashPay state…), network, active wallet per network, secure time, rates cache, migration flags, voting / devnet prefs, CrowdNode per-wallet keys |

### 1.22 Removed / mocked / dead — do NOT treat as parity requirements without a decision

1. Paper-wallet / private-key sweep (removed; needs SDK support). → **Decision.**
2. CoinJoin mixing (discontinued; only the sweep remains).
3. Governance budget proposals (never in this app).
4. Invitation creation / history / reclaim (removed from UI; the SDK supports it). → **Decision.**
5. "Buy credits" screen (`S/UI/DashPay/Credits/BuyCreditsViewController.swift`, `MOCK_DASHPAY`).
6. Private-memo editor (column only).
7. Token portfolio / transfers (none).
8. Maya standalone portal (hidden; Maya reachable only as a Dash DEX route); Coinbase "Convert crypto" (disabled).
9. Legacy dead UI: `S/UI/DashPay/Welcome/*`, `GetStarted/*`, `UsernamePending`, `DashPay/Presentation/Setup/*`, `VerifyIdentityScreen.swift`, `DashPayProfileView.swift`, `DWNetworkErrorViewController`, the dead SwiftUI receive stack (`ReceiveScreenHostingController`, arch review T17), the Core Data model `S/Models/DataMigration/DashWallet.xcdatamodeld`.
10. Today extension (not embedded); background refresh (disabled).
11. iOS-only, N/A on desktop: jailbreak check, NFC pay, App Store review prompt, storefront geoblock (replace with IP geo), device-passcode requirement (replace with an OS secret-store availability check).
12. CrowdNode: suspended/hidden for the migration release per CLAUDE.md; keep it behind a flag.

---

## 2. Architecture summary (patterns to replicate)

### 2.1 Codebase shape

- About 790 Swift files, 185 ObjC `.m`, 18 storyboards, 14 XIBs.
- SwiftUI is the mandated direction: 195 `View` types, 92 `ObservableObject` view models, 62 files hosting SwiftUI in `UIHostingController`.
- Deployment target is iOS 18.
- CocoaPods: CocoaLumberjack, SQLite.swift, SQLiteMigrationManager, Moya 15, SwiftJWT, SDWebImage(+SwiftUI), Firebase/CoreOnly + FirebaseStorage 8.15, SSZipArchive, TOCropViewController, MBProgressHUD, DWAlertController, KVO-MVVM, CloudInAppMessaging.
- SPM: SwiftDashSDK (local path) and DashUIKit (`dashpay/DashUIKit` @ master).

### 2.2 Layering

```
UI (SwiftUI Screen + @MainActor ObservableObject ViewModel; legacy UIKit/ObjC controllers)
  │  thin UIHostingController wrappers / UIKit navigation (tab bar + nav controllers + sheets)
  ▼
App services (Models/*, Infrastructure/*): WalletSendService, CurrencyExchanger, Taxes,
  Coinbase/Uphold/CrowdNode/Swap/DashSpend services, NotificationDispatcher, DAOs on SQLite
  ▼
SwiftDashSDK adapter layer (Infrastructure/SwiftDashSDK/*) — the ONLY code that touches the SDK
  SwiftDashSDKHost  ─ owns SDK instance, per-network ModelContainer, PlatformWalletManager, wallets
  SwiftDashSDKWalletRuntime ─ lifecycle orchestration (start/stop/switch network/switch wallet/add)
  SwiftDashSDKSPVCoordinator ─ Core SPV chain sync state (@Published)
  PlatformAddressSyncCoordinator ─ Platform BLAST sync, DashPay/DPNS/shielded loops
  SwiftDashSDKWalletState ─ wallet-side @Published state (balance, credits, CoinJoin balance)
  SwiftDashSDKTransactionSender / PlatformSendExecutor / ShieldedTransferCoordinator ─ money movement
  SwiftDashSDKWalletCreator / KeyMigrator / PhraseRepairer / WalletWiper ─ seed lifecycle
  ▼
SwiftDashSDK (Swift) → DashSDKFFI.xcframework (Rust: key-wallet, dash-spv, platform SDK)
```

Rules worth copying verbatim into the desktop project (from the iOS `CLAUDE.md` + `ARCH_REVIEW_2026-07-03.md`):

1. **One SDK boundary.** UI and view models never call the FFI. Views contain no SDK calls, fee math, auth calls or protocol constants; those live in the VM or a service.
2. **Lifecycle ordering is centralised.** Start is host → SPV → BLAST; stop is BLAST → SPV → host. All lifecycle operations go through a serial async lifecycle queue (`SerialAsyncLifecycleQueue` in `SwiftDashSDKWalletRuntime.swift`). Network switch and wallet switch are runtime operations, shown to the user through `WalletLifecycleTransitionState` and the overlay.
3. **Seed safety ordering.** Persist the mnemonic, read it back and compare, *then* create the live wallet; roll back only the provisional mnemonic on failure (`MnemonicFirstWalletCreation` in `SwiftDashSDKHost.swift`). Resolve the active wallet only through the host (`WalletEnvironment.activeWalletId(for:)`), never "first mnemonic".
4. **Send pipeline.** auth → build + sign (`prepare*`, never broadcasts) → user confirms the exact FFI fee and total → `broadcast()`. Arch review T1 shows what goes wrong otherwise: the confirm sheet was decorative because the tx had already been broadcast.
5. **One auth primitive** (`AuthenticationGate`) with a watchdog. No copies (arch review T6).
6. **Sync gating.** Never gate on SPV `state == .synced`. Gate on `SyncingActivityMonitor` `.syncDone` (dash-spv's steady state is `waitForEvents` at progress ≈ 1).
7. **Model unknowns as unknown** (no fabricated defaults), no stub-and-assert, no re-emitting another system's notification names, and every new singleton needs a stated reason (arch review T10, T12, T18).
8. Debug tools gated out of production (arch review T20).

### 2.3 State publication (SDK → UI)

- **Combine `@Published`** on adapter singletons:
  - `SwiftDashSDKWalletState.shared.$balance` (`WalletBalance{confirmed, unconfirmed, immature, locked}`, four disjoint buckets), `$platformPaymentCredits`, `$coinJoinBalanceDuffs`, `$pooledSpendableDuffs`.
  - `SwiftDashSDKSPVCoordinator.$connectedPeers` and the sync state.
  - `PlatformAddressSyncCoordinator` (about 21 published properties).
  - `BaseRatesProvider.$hasFetchError/$isVolatile`.
- **View models subscribe and derive**, e.g. `HomeViewModel` → `distinctBalanceChanges(from: $balance)`, `BalanceModel`, `SendAmountModel` (`walletSpendable: state.$balance.map{$0?.spendable}`), `CreateUsernameViewModel`.
- **SDK events arrive on FFI callback threads.** The adapters marshal every `@Published` mutation to the main thread (invariant 2 in `SwiftDashSDKWalletState.swift`).
- **Transaction history is not pushed as events.** VMs read SwiftData rows the Rust persister wrote, re-query on change triggers, and coalesce reload passes (`HomeViewModel.reloadPassInFlight` / `reloadPassRequestedAgain`).
- **Sync progress** is wrapped by `SyncingActivityMonitor` into a `SyncStateSnapshot`:
  - `Kind`: offline, headers, filterHeaders, filters, blocks, masternodes, finished.
  - Per-phase rows with current / target / isComplete.
  - Damping constants: max progress delta 10 %, 3.25 s peak delay.
- **Known debt to avoid** (arch review T11, T12, T26):
  - Raw `NSManagedObjectContextDidSave` used as an app-wide event bus.
  - The legacy `DSChainManagerSync*` notification masquerade.
  - Balances refreshed at 1 Hz piggybacked on SPV ticks.
  
  The desktop should have **one typed, debounced "SDK data changed" stream per domain**.

### 2.4 Screen composition and view-model conventions

- **Screen pattern:** `XxxScreen: View` + `@MainActor final class XxxViewModel: ObservableObject` with `@Published` state, `items: [MenuItemModel]` for menus, and a `navigationDestination` enum that the hosting layer observes (e.g. `SecurityMenuViewModel` publishes `SecurityMenuNavigationDestination` / `SecurityMenuResetWalletDestination`).
- **Navigation:**
  - The UIKit tab bar (`MainTabbarController`) hosts `BaseNavigationController`s.
  - SwiftUI screens are pushed in `UIHostingController`; the bridge is `S/Categories/UIHostingController+DashWallet.swift`.
  - Sheets for confirms and info; bottom sheets via DashUIKit `BottomSheet`; a multi-step flow coordinator for swaps (`SwapFlowCoordinator`).
  - Storyboard lookup helpers live in `S/UI/Assembly/UIAssembly.swift` (legacy).
- **Flows as small state machines:** registration phases (`DWRegistrationPhaseAdapter`), send phases, CrowdNode signup states, swap order statuses, wallet lifecycle phases. Each surfaces as a VM enum.
- **Menus are data:** `MenuItemModel(title:subtitle:icon:action:)` lists rendered by a shared `MenuItem` row.
- **Desktop mapping:** Screen + ViewModel + service maps directly onto any reactive desktop stack (SwiftUI-for-macOS, or a Rust/TS core with React/Svelte views and observable stores). Keep the VM as the only place that composes services, and keep the views dumb. That is exactly how DashUIKit components are specified.

### 2.5 Services and DI

- Mostly singletons (`static let shared`; about 86 in Sources), e.g. `WalletSendService.shared`, `CurrencyExchanger.shared`, `AuthenticationService.shared`, `DatabaseConnection.shared`, `SwiftDashSDKHost.shared`.
- Protocol seams exist only in places: `AuthenticationServiceProtocol`, `RatesProvider`, `SwapProvider`, `DashSpendRepository`, `DashConnectDataSource`, transaction sources (`SwiftDashSDKWalletSource` vs `StubTransactionSource` for demo mode).
- Arch review T18 calls the singleton sprawl a debt. **Desktop: inject services through protocols / interfaces from a composition root** (as `NotificationsBootstrap` already does) and keep stubs for demo mode and tests.

### 2.6 Currency exchanger and rates

- `CurrencyExchanger` (`S/Infrastructure/Currency Exchanger/CurrencyExchanger.swift`):
  - `rate(for:)`, `convertDash(amount:to:)`, `convertToDash(amount:currency:)`, `convert(to:amount:amountCurrency:)`.
  - Observers, plus `fiatAmountString(for:)`.
  - Rounds to the fiat formatter's fraction digits.
- `BaseRatesProvider` (`Data Provider/RatesProvider.swift`):
  - Moya `GET https://rates.ctx.com/rates?source=ctx`; body is `[{"symbol":"DASHUSD","price":"12.34"}]` with price as a string.
  - Polls every **60 s**. Cache in UD `DS_PRICEMANAGER_PRICESBYCODE` / `DS_LAST_RATES_RETRIEVAL_TIME`.
  - Volatility window 7 days; publishes `hasFetchError` / `isVolatile`.
- Coinbase screens use their own `CoinbaseRatesProvider` (`/v2/exchange-rates`).
- Fiat currency: UD `LOCAL_CURRENCY_CODE`, default from the OS locale; changes post `.fiatCurrencyDidChange` (`S/Application/App.swift`).
- **Historical rate is stamped onto each tx** in SQL (`tx_userinfo.rate*`) so history shows the fiat value at the time.

### 2.7 Networking

- `HTTPClient<Target: TargetType>` (`S/Infrastructure/Networking/HTTPClient.swift`) wraps a Moya provider with:
  - `AccessTokenPlugin` (bearer only for `AccessTokenAuthorizable` targets);
  - an ETag plugin with an in-memory cache, cleared on memory warning;
  - typed `HTTPClientError` (statusCode / mapping / moya / decoder);
  - a concurrent API queue;
  - feeding HTTPS `Date` headers into `SecureTimeService`.
- Each integration is a `TargetType` enum (`CoinbaseAPIEndpoint`, `UpholdClient`, `CrowdNodeEndpoint`, `SwapKitEndpoint`, `MayaEndpoint`, CTX / PiggyCards endpoints, `DWAvatarUploadClient`).
- Exceptions: Topper (URLSession + SwiftJWT), Explore DB (Firebase SDK), Insight (`InsightExplorerAPI` / `SwiftDashSDKInsightClient`).
- **Desktop:** replicate as one typed HTTP client with per-service endpoint enums, a token-provider hook, ETag support and a `Date`-header hook.

### 2.8 Persistence

See §1.21. Two notes for the desktop:
- **App-owned metadata lives in SQLite** next to (not inside) the SDK's store, with timestamped, append-only migrations. `DatabaseConnection.migrateIfNeeded` refuses duplicate migration versions because a duplicate once silently skipped later tables.
- **The SDK owns wallet/chain data.** The app only reads it.

### 2.9 Auth gate

See §1.2. Model:
- PIN (4 digits) is the root credential, stored in the secure store, with lockout and secure-time ratchet.
- Biometrics are a convenience bounded by a 7-day PIN freshness and a DASH spending allowance.
- The lock screen is shown after auto-lock or launch.
- Every sensitive action calls `AuthenticationGate`.
- The seed is **not** PIN-encrypted on iOS (the keychain is the boundary).
- **Desktop decision required:** desktop OS secret stores (macOS Keychain, Windows DPAPI / Credential Manager, libsecret) are weaker boundaries than the iOS Secure Enclave keychain, especially on Linux and against same-user malware. Consider PIN/password-derived encryption of the seed at rest. Biometric support also differs: Touch ID on macOS, Windows Hello, typically none on Linux.

### 2.10 Logging and diagnostics

- CocoaLumberjack file logs (`DWLogger`) plus SDK session logs. `DiagnosticLogExporter` builds a zip for support.
- `MainThreadStallMonitor.swift` watches for UI stalls.
- Developer status screens (SPV / Platform / Shielded / DashPay) and Storage Explorer exist. The desktop should ship these behind a "Developer" toggle.

---

## 3. Visual design language

### 3.1 Sources of truth

- **App asset catalogs:**
  - `DashWallet/Resources/AppAssets.xcassets`: **632 image sets**, grouped as AppIcon, CoinJoin, Coinbase, CrowdNode, Dash logo, DashConnect, DashPay Users, Explore Dash, Flags, Hourglass Animation, Illustration, Integrations, List, Maya, Menu, Navigation bar, Onboarding, Shortcuts, TabBar, Toast, Transactions, Uphold, Usernames, Voting, plus loose icons (`icon_*`, `dp_*`, `portal.*`, `service.*`, `logo*`, `dash_logo_testnet`).
  - `Shared/Resources/SharedAssets.xcassets`: **125 color sets**, the app palette with light/dark.
  - `DashPay/Assets/DPAssets.xcassets` (a couple of DashPay icons), `TodayExtension/TodayExtensionAssets.xcassets`, `WatchApp/Assets.xcassets`.
- **Swift token layers:**
  - `S/UI/SwiftUI Components/Color+DWStyle.swift` (`Color.primaryText`, `.dashBlue`, `.gray300`…), `Font+DWStyle.swift`.
  - Legacy UIKit: `S/UI/Views/UIColor+DWStyle.{h,m}`, `UIFont+DWFont.{h,m}`.
  - Layout constants: `S/UI/Style/Style.swift` (`stackSpacing = 15`, corner `radius = 12`, `kAnimationDuration = 0.35`).
- **DashUIKit** (§3.5) is the shared design system the iOS app is migrating to; it is imported in about 190 app files. Its own catalog `Sources/DashUIKit/Resources/Media.xcassets` has **159 color sets, 138 image sets**.
- Figma is the upstream source (iOS repo skill `.claude/skills/figma-assets/SKILL.md`).

### 3.2 Color palette (from `SharedAssets.xcassets`; L = light, D = dark)

| Token | Value |
|---|---|
| **Dash blue** `DashBlueColor` / `Blue` | `#008DE4` (+ alpha ramps 5–90 %); `BlueGradientStartColor` `#00BBE4`; `LightBlue` `#78C4F5`; `DarkBlueColor` `#011F5F` |
| Nav bar blue | `#008DE3` |
| `Label` (primary text) | L `#0A0B0D` / D `#FFFFFF` |
| `SecondaryTextColor` | L `#525C66` / D `#A4ABB3` |
| `TertiaryTextColor` | L `#75808A` / D `#FFFFFF@60%` |
| `BackgroundColor` | L `#FFFFFF` / D `#1E1F24` |
| `SecondaryBackgroundColor` (screen bg) | L `#F7F7F7` / D `#141519` |
| `TertiaryBackgroundColor` | L `#FAFAFA` / D `#1D2023` |
| Black ramp | `Black` `#0A0B0D` (Black1000, alpha 5–90 %), `Black800` `#1E1F24`, `Black900` `#141519` |
| Gray ramp | `Gray50` `#F5F6F7`, `Gray100` `#EBEDEE`, `Gray200` `#CED2D5`, `Gray300` `#B0B6BC`, `Gray400` `#75808A`, `Gray500` `#525C66` |
| Semantic | `Green` `#3DB58A`, `Red` `#EB3842`, `SystemRedColor` `#DA2C43`, `ButtonRedColor` `#EA3943`, `Orange` `#FA9169` (testnet badge), `Yellow`/`SystemYellowColor` `#FFC043`, `Purple` `#5856D6` |
| Brand partners | `Uphold` `#49CC68`, `Topper` `#BCF292` |
| Controls | `SeparatorLineColor` L `#D5D5D5` / D `#4A4A4A`; `SearchBackground` `#75808A@10%`; `DisabledButtonColor` L `#DBDBDB` / D `#585858`; `ModalDimmingColor` `#04040F@40%`; `TabbarBorderColor` L `#EAEAEA` / D `#494949`; `TabbarInactiveButtonColor` `#C6C6C6` |

Character:
- Light UI: white cards on a `#F7F7F7` canvas. Dark UI: `#1E1F24` cards on `#141519`.
- A single saturated accent, Dash blue `#008DE4`, used for primary buttons, links, the home header and the payment tab button.
- Near-black text `#0A0B0D` with a cool gray ramp.
- Green / red / orange only for status.
- No purple brand usage. That fits the user's "no AI purple" preference; `Purple` exists only as a system accent token.

### 3.3 Typography

- **System font only: SF Pro, no bundled fonts.** UIKit uses `preferredFontForTextStyle` / `systemFont(weight:)`.
- The SwiftUI scale (`Font+DWStyle.swift` = DashUIKit `DashTextStyle`), size/weight/line-height:
  - largeTitle 34 bold / 41
  - title1 28 bold / 34
  - title2 22 bold / 28
  - title3 20 bold / 25
  - title3Medium 20 medium / 25
  - headline 17 bold / 22
  - body 17 regular / 22
  - callout 16 regular / 21
  - calloutMedium 16 semibold / 21
  - subhead 15 regular / 20
  - subheadMedium 15 medium / 20
  - footnote 13 regular / 18
  - footnoteMedium 13 medium / 18
  - caption1 12 regular / 16
  - caption1Medium 12 medium / 16
  - caption2 11 regular / 13
- **Desktop:** use SF Pro on macOS. On Windows/Linux use Inter (closest metrics) or Segoe UI Variable / system UI, with the same scale. Line heights matter: DashUIKit sets them explicitly via `.dashFont`.

### 3.4 Key components and patterns (what makes it look like the iOS family)

- **Balance hero:** large DASH amount with the Dash "D" currency glyph (`icon_dash_currency`, `dashCurrency`) and a fiat sub-line. Tap to hide, which shows an eye icon. Testnet badge.
- **Shortcut bar:** 4 rounded tiles with an icon over a caption, on a white card.
- **Transaction rows** (`TransactionView`): 36–40 pt circular icon (direction / merchant / avatar), title + time, right-aligned signed amount + fiat. Grouped by day headers.
- **Menu rows** (`MenuItem`): icon, title / subtitle, trailing accessory (chevron, toggle, badge, value); grouped in rounded white cards (`MenuViewModifier`: 12 pt radius, subtle shadow in light mode only).
- **Buttons** (`DashButton`):
  - 4 sizes. Paddings 10/8/6/6 pt vertical and 20/16/12/8 horizontal; font 16/14/13; corner radius per size.
  - 11 styles: filledBlue, filledRed, strokeGray, tintedBlue, tintedGray, plainBlue, plainBlack, plainRed, filledWhiteBlue, tintedWhite, plainWhite.
  - Loading state; full-width option.
- **Amount entry** (`EnterAmountView` / `SwapAmountView` / `NumericKeyboardView`): large centred amount with a DASH ↔ fiat swap animation, currency picker, Max button, custom numeric keypad. Desktop also takes hardware-keyboard input; the iOS app already has a `HardwareNumericKeyboardView`.
- **Sheets:** bottom sheets with a grabber and three-slot nav bar (`BottomSheet`, `NavigationBar`) for confirm, info and pickers. Desktop: modal sheets / dialogs with the same header.
- **Feedback:** blurred `Toast` (warning / info / error / success / copied / loading / noInternet), `SystemMessageView`, 90×90 success / error illustrations, loading spinner, a "Sending…" animation.
- **Top intros** (`TopIntroView`): title + subtitle + optional illustration at the top of flows.
- **Converter card** (`ConverterCard`) for swap from → to; `CoinSelector` rows with a "halted" badge; `RadioButtonRow`; `SearchBar`; `AddressFieldView` (QR + clear + error).
- **Tab bar:** five icons; the centre Payments item is a filled blue circle (`MainTabbarController.makePaymentTabImage`).

### 3.5 DashUIKit inventory and platform support

**Package** (`/Users/pasta/workspace/DashUIKit/Package.swift`):
- swift-tools 6.3, one library target `DashUIKit`, resources `Media.xcassets` via `Bundle.module`.
- `platforms: [.iOS(.v14)]` only. The iOS-14 floor is a hard project rule; public API is annotated `@available(iOS 14, macOS 11, *)`.
- Docs in `docs/` (foundation, buttons-and-inputs, amount-and-currency, lists-and-rows, navigation-and-containers, feedback, utilities).
- **The README install URL is stale** (`romchornyi/DashUIKit`); the real one is `dashpay/DashUIKit`.

**Public components:**

| Group | Types |
|---|---|
| Buttons & inputs | `DashButton` (+ `DashButtonSize`, `DashButtonStyle`), `DashSwitch`, `SwitchView`, `SearchBar`, `AddressFieldView`, `NumericKeyboardView` |
| Amount & currency | `EnterAmountView` (`EnterAmountStyle`), `SwapAmountView`, `ReceiveEstimateView`, `DashAmount` (`DashAmountSign`), `DashBalanceView`, `DashPickerView`, `CurrencyOption` |
| Lists & rows | `CoinSelector` (`CoinSelectorTrailing`), `MenuItem` (`MenuItemAccessory`), `ConverterCard`, `ConverterCardItem`, `TransactionView`, `RadioButtonRow` (`Checkbox`), `List1View` |
| Navigation & containers | `NavigationBar`, `NavigationBarElement`, `TopIntroView`, `BottomSheet` (+ `BottomSheetHeightPreferenceKey`), `MenuViewModifier` |
| Feedback | `Toast` (`ToastStyle`), `SystemMessageView`, `LoadingIllustration`, `LoadingSpinner`, `SuccessIllustration`, `ErrorIllustration`, `XmarkIcon` |
| Utilities | `readingFrame`, `readingLocation`, `ScrollViewWithOnScrollChanged`, `ScaleToFitWidth` |
| Foundation | `Color.dash.*` (`DashColors`: text / background / button / component tokens + raw ramps; asset-backed light/dark; code-defined adaptive `shadow`), `Font.dash.*` (`DashFonts`), `DashTextStyle` + `.dashFont()`, `DashIconSource` (`.system` / `.custom` / `.uiImage`) + `Image(dash:)`, icon / illustration namespaces (`DashIcon`, `Icons`, `Illustrations`, `Menu`, `Transaction`, `Toast`, `SystemMessage`, `Other`, `Common`, `AdditionalInfo`) |

**macOS build (verified 2026-10-05).** `swift build` for the macOS host (arm64, Swift 6.3.3, macOS 26.5 SDK) **succeeds**, but several components are compiled out:
- Wrapped wholly in `#if canImport(UIKit)`, so **absent on macOS**: `Toast` (+ `BackgroundBlurView`), `SearchBar`, `AddressFieldView`.
- Partially UIKit: `BottomSheet` (window-height lookup via `UIApplication`/`UIScreen`, `#if os(iOS)` paths), `SwapAmountView`, `Image+DashUI` (the `.uiImage` case is typed `Never` off UIKit), `Color+DashUI` (adaptive shadow), `LineHeight+DashUI` (already has an AppKit branch).
- Everything else is plain SwiftUI and works on macOS.

**Implications for the desktop:**
- (a) If the desktop app is **SwiftUI on macOS**, DashUIKit is directly reusable after:
  - adding `.macOS(.v13)` (or similar) to `platforms`;
  - porting `Toast` (NSVisualEffectView), `SearchBar` and `AddressFieldView` (AppKit / FocusState paths);
  - replacing `BottomSheet` with a macOS sheet.
- (b) Windows and Linux cannot run SwiftUI. There, DashUIKit is the **spec**:
  - Export the tokens: 159 color sets + DashTextStyle scale, e.g. to JSON / CSS variables generated from the `.colorset` files.
  - Export the icons: 138 image sets, mostly PDF/SVG vectors.
  - Re-implement the ~35 components in the chosen cross-platform UI toolkit, keeping the same names, props and states so designers and developers share vocabulary across iOS, Android and desktop.
- Either way, **generate tokens from the asset catalogs** (both `SharedAssets.xcassets` and DashUIKit `Media.xcassets`) rather than hand-copying hex values. The DashUIKit rule "never hardcode a Color" is enforced by convention there.

---

## 4. Parity checklist

Status key for the desktop tracker: one line per user-visible capability. "(decide)" marks removed / iOS-only items that need an explicit product decision; they are not parity requirements by default.

- IOS-001 Intro / onboarding carousel (Welcome, Pay with Ease, More Control) with demo mode
- IOS-002 Create new wallet with 12- or 24-word phrase (mnemonic persisted and verified before the wallet goes live)
- IOS-003 Recovery-phrase backup warnings + show phrase
- IOS-004 Phrase verification by ordered word-chip selection + "Verified Successfully"
- IOS-005 Backup reminder 24 h after first funds if unbacked; Backup shortcut
- IOS-006 Screenshot / screen-capture warning while the phrase is visible
- IOS-007 Restore from 12/15/18/21/24-word phrase, all 10 BIP39 languages, per-word errors
- IOS-008 Phrase repair: find one wrong word / 1–2 missing words via checksum + Insight history + edit-distance suggestions, cancellable progress
- IOS-009 Existing-wallet detection on reinstall (Keep / Delete All with typed acceptance)
- IOS-010 Set / confirm 4-digit PIN
- IOS-011 Biometric enrollment (Touch ID / Windows Hello where available) with default 0.5 DASH limit
- IOS-012 PIN lockout policy (3 free, exponential waits, disabled at 8) with tamper-resistant secure time
- IOS-013 Lock screen with PIN pad, biometric unlock, Quick Receive, Scan to Send, Forgot PIN
- IOS-014 Forgot PIN → reset PIN by entering a matching recovery phrase
- IOS-015 Auto-lock timer (Immediately / 1 m / 5 m / 1 h / 24 h) and Auto Logout toggle
- IOS-016 Spending confirmation toggle + biometric spending limit (0 / 0.1 / 0.5 / 1 / 5 DASH) + security-level meter + reset to default
- IOS-017 Shared auth gate on every sensitive action (send, view phrase, keys, voting, wipe…)
- IOS-018 Wallet lifecycle overlay (opening / switching network / switching / adding / removing / wiping) with Retry, Switch Back, Help + diagnostics email
- IOS-019 Home balance hero: Core + Shielded (+ Platform in advanced mode), fiat line, partial / unknown states
- IOS-020 Hide / show balance (persisted, autohide option, first-use hint)
- IOS-021 Balance breakdown card (Transparent / Platform / Shielded) with per-balance info sheets
- IOS-022 Testnet / devnet badge and testnet logo
- IOS-023 Sync status: failure / no-connection banners, per-phase progress dialog, connected peers, Change peers after a stall
- IOS-024 Exchange-rate stale / failed / volatile warnings
- IOS-025 Customisable 4-slot shortcut bar with state-dependent defaults and all shortcut actions
- IOS-026 Time-skew detection dialog
- IOS-027 Transaction history grouped by day with paging
- IOS-028 History filters (Sent, Received, Rewards, Masternode, Gift card, Shielded sent / received; All / Only)
- IOS-029 Tx rows with merchant / service / contact icons, route labels for internal transfers, status labels
- IOS-030 Grouped rows: CoinJoin mixing per day, CoinJoin withdrawals, CrowdNode
- IOS-031 Transaction details (addresses, fee incl. Insight lookup, date, status, contact, masternode addresses)
- IOS-032 Tx details actions: copy txid, open in Insight / Blockchair, raw tx view / copy hex, Maya / NEAR explorer for swaps
- IOS-033 Rebroadcast / complete stuck asset-lock transfers
- IOS-034 Remove unconfirmed tx if not on network + bulk drop-and-rescan
- IOS-035 Shielded and Platform activity rows + details (incl. shielded memo)
- IOS-036 Tax category per tx (cycle Income / Transfer In, Expense / Transfer Out) + address pre-tagging by integrations
- IOS-037 One-time "Reclassify your transactions" intro
- IOS-038 Historical fiat rate stamped per transaction
- IOS-039 CSV tax export (exact column set, mixing excluded, requires synced)
- IOS-040 ZenLedger portfolio export
- IOS-041 Payments sheet with Send / Receive / Transfer entry
- IOS-042 Send to Dash address (type / paste / clipboard suggestion / QR) with Core / Platform / Shielded address classification
- IOS-043 QR scanning (desktop: webcam, image file, screen region, clipboard image)
- IOS-044 Amount entry with DASH ↔ fiat toggle, fee-aware Max per route, show / hide balance
- IOS-045 Source-balance picker and all 8 send routes (Core / Platform / Shielded combinations)
- IOS-046 Core send confirm with exact fee and total; broadcast only on Confirm
- IOS-047 Non-Core send confirm with step progress and submitted-unconfirmed state
- IOS-048 dash: / pay: / dashwallet: URI handling incl. BIP21 params; OS protocol-handler registration
- IOS-049 BIP70 / BIP72 payment requests (verify, expiry, network, pay, ACK)
- IOS-050 Pay to DashPay contact (DIP-15) with unknown-outcome per-contact lock
- IOS-051 Send guards (initial-sync block, offline block) and full error-copy set
- IOS-052 Post-send success details screen
- IOS-053 Receive: Core / Shielded (+ Platform in ADV) address toggle, QR, copy, share
- IOS-054 Core receive address rotation ("Receive another") and live incoming-payment watcher with Received card
- IOS-055 Request a specific amount (QR with amount + username, share, paid detection)
- IOS-056 (decide) Sweep paper wallet / import private key — removed on iOS
- IOS-057 CoinJoin recovery scan + "Move mixed coins" to Dash Wallet or Shielded (chunked sweep), entries in Home, Settings and Tools
- IOS-058 (decide) CoinJoin mixing — discontinued on iOS
- IOS-059 Shielded balance, sync spinner, Shielded Sync Info
- IOS-060 Shield from Core / Platform / CoinJoin; unshield; shielded → shielded / Platform; shielded Max
- IOS-061 Finish stuck shielded transfer ("Pending — tap to finish")
- IOS-062 Internal Transfer between Core / Shielded / Platform / identity credits, privacy tip, timing sheet
- IOS-063 Advanced mode toggle (auto-enable on first Platform balance) + info sheet
- IOS-064 Platform (DIP-17) address balance and BLAST sync
- IOS-065 Join DashPay banner states + intro + voting info + shielded-funding readiness checklist
- IOS-066 Username form: rules, debounced availability, contested detection, contest precheck, direct-buy-if-listed
- IOS-067 Identity + username registration funded from Core, Platform addresses, Shielded pool or invitation; resumable
- IOS-068 Contested-name voting-period tracking, temporary username, request status screen (contenders, tallies, deadline)
- IOS-069 Identity profile sheet (credits, names, top-up from Core / Platform / Shielded)
- IOS-070 Withdraw identity credits to Core / transfer to own Platform address
- IOS-071 Identities screen (list, set main, keys, refresh, find, get username)
- IOS-072 Contacts tab (contacts, pending requests, search, hidden, my identity card, Enable DashPay keys, FAQ)
- IOS-073 Add contact by username search or QR; My QR
- IOS-074 Contact profile: accept / ignore, pay, payment activity, alias / note / hide
- IOS-075 DashPay notifications screen (bell, unread count, New / Earlier / Pending, inline accept)
- IOS-076 Edit profile: display name, about, avatar via Gravatar / URL / camera / file with crop + upload (Imgur)
- IOS-077 Claim invitation (paste / scan / link, inviter preview, claimed check, auto contact request); invitation links pre-wallet
- IOS-078 (decide) Create / share / reclaim invitations — removed on iOS, SDK supports it
- IOS-079 Contested-username voting: contests list, search / sort, detail, approve / abstain / lock, bulk vote, node selection, vote history, Voting toggle
- IOS-080 Masternodes list + detail (status, keys, collateral, claimable balance, epoch blocks)
- IOS-081 Evonode status request, evonode credit withdrawal, Unban (ProUpServTx) with pending state
- IOS-082 Track any masternode + attach keys (secure vault), tracked withdraw / unban
- IOS-083 Masternode keychain viewer (owner / voting / operator BLS / ed25519; WIF, pubkeys, node IDs, usage)
- IOS-084 Username marketplace (find / my names / browse / purchases; buy, list, reprice, delist, transfer, history, request contested)
- IOS-085 DashConnect: dash-key login, dash-st state transitions, approve-connection sheet, connections list (test networks)
- IOS-086 DashConnect token purchase approval sheet
- IOS-087 Buy & Sell portal (Topper, Uphold, Coinbase, Dash DEX ordering + balances, auth-gated, geo rules)
- IOS-088 Topper buy widget (signed JWT, sandbox on testnet)
- IOS-089 Uphold: OAuth link, DASH card balance, transfer to wallet with OTP, logout tutorial
- IOS-090 Coinbase: OAuth link, balance, buy Dash (min / fee / payment method), transfer both directions with 2FA, error mapping, GB geoblock
- IOS-091 CrowdNode: tx-based signup, deposit, signed-message withdraw with limits, online-account link, APY, balance reminder (behind flag)
- IOS-092 Dash DEX sell flow (coin select, address incl. linked-exchange address, quote / convert, 10 s preview, memo tx, status)
- IOS-093 Dash DEX buy flow (coin, amount, refund address, deposit QR)
- IOS-094 Swap order persistence, 30 s tracking, 24 h expiry, notifications, pending-IS gate
- IOS-095 Explore menu (Where to Spend, ATMs, Staking, Username Marketplace, Buy & Sell, Get Test Dash)
- IOS-096 Explore DB sync from Firebase Storage (checksum-zipped, per network, 24 h)
- IOS-097 Merchants: Online / Nearby / All, map + list, FTS search, all locations
- IOS-098 ATMs: All / Buy / Sell / Buy & Sell, search
- IOS-099 Explore filters (payment method, sort, radius, territory, denomination type)
- IOS-100 POI details (contact, directions, pay, buy gift card with provider picker)
- IOS-101 DashSpend CTX: email OTP login, live discount, BIP70-paid gift card purchase
- IOS-102 DashSpend PiggyCards: email OTP signup / login, denominations, order + pay, RU / CU geoblock
- IOS-103 Gift card details (polling, number, PIN, generated / decoded barcode, how-to) + stored gift cards
- IOS-104 Settings: local currency (searchable), default from OS locale
- IOS-105 Settings: notifications toggle reflecting OS permission
- IOS-106 Settings: network switch mainnet / testnet (/ devnet in dev builds) + Devnet settings
- IOS-107 Settings: About (version, network, Explore sync status, support, GitHub, tech-info / log export)
- IOS-108 Security: view recovery phrase (multi-wallet picker), change PIN, biometrics toggle, autohide balance
- IOS-109 Security: reset / wipe wallet with phrase confirmation and full cleanup (incl. integrations)
- IOS-110 Wallets (multi-wallet): list, switch, rename, view phrase, remove, add (create / import), accounts view
- IOS-111 Tools: extended public key with QR / copy / share
- IOS-112 Tools: export logs (zip) + Support email with logs
- IOS-113 Sync Info: Core sync status, rescan (creation / height / full), edit birth height, drop unconfirmed
- IOS-114 Sync Info: Platform, DashPay and Shielded sync status screens (developer toggle)
- IOS-115 Storage explorer / debug tools behind a developer toggle
- IOS-116 Local notifications: incoming payments (catch-up), contact requests / accepts, swap results, CrowdNode events, inactivity reminder; tap deep-link routing
- IOS-117 Menu-bar / tray companion (balance, receive QR, request amount, scan / pay, last tx) replacing Watch / Today widget
- IOS-118 Localization: 43 locales via Transifex dash-mobile-wallets, English-key strings, plurals, RTL
- IOS-119 Visual parity: SharedAssets + DashUIKit color tokens (light / dark), SF-style type scale, DashUIKit component set, app icon set
- IOS-120 Accessibility labels on icon-only controls (iOS ACCESSIBILITY.md rule)
- IOS-121 Testnet faucet shortcuts (in-app PoW faucet + web faucet fallback)
- IOS-122 Inactivity / announcement channel (replacement for CloudKit in-app messaging) (decide)
- IOS-123 Secure storage of seed and PIN in the OS secret store, with a decision on PIN-derived seed encryption for desktop (decide)
