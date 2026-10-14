# U-02: onboarding, lock screen, unlock sheet, recovery phrase, Forgot passphrase (Tauri)

ROADMAP U-02 on the Tauri stack (ADR 0003): the first-run and Add Wallet flows (create, verify,
restore with live checks, the vault passphrase), the lock screen with the iOS lockout policy, the
unlock sheet, Show Recovery Phrase, and Forgot passphrase. The view models are plain TypeScript
classes with no React in them (the Swift `WalletFeatures` models, ported); their tests are the
Swift tests ported by parity ID. The screens follow the mobile wallets (UX-SPEC §4.3, §4.4), not
dash-qt. Every call goes through dw-app's generated commands (E0-13); U-02 adds no command, only two
menu rows and one Tauri core window permission (D14, D15).

| Part | Where |
|---|---|
| View models | `apps/desktop/src/onboarding/{onboarding,lock,forgot,recovery,passphrase,words,birthHeight}.ts` |
| Their tests (by parity ID) | `apps/desktop/src/onboarding/*.test.ts` |
| Screens | `src/screens/{Onboarding,LockScreen,ForgotPassphrase,RecoveryPhrase,Settings}.tsx`, `src/components/{passphrase,phrase}.tsx` |
| Fixture handlers and scenarios | `src/fixture/{vault,handlers,scenarios}.ts` |
| Playwright | `e2e/onboarding.spec.ts` (flows, axe, light and dark, Chromium and WebKit), `e2e/visual.spec.ts` (baselines) |
| Real app on dashd regtest (T1, GL) | `e2e-tauri/onboarding.ts`, `scripts/e2e-tauri-onboarding.sh` |
| Menu rows | `rust/crates/dw-app/src/shell/menu.rs` (`wallet.add`, `wallet.recovery-phrase`) |

## Flow map

| Row | Flow | Steps (screens) | Engine calls |
|---|---|---|---|
| IOS-002 | Create | Welcome ▸ (no vault: Choose passphrase) ▸ Recovery phrase (12 words; 24 under Advanced options) ▸ Verify ▸ Done | `lockStatus`, `generateMnemonic(12\|24, english)`, then at the last correct chip `createVault(passphrase)` when there is no vault and `importWallet(phrase, "", {birthHeight: null})` |
| IOS-003 | Recovery phrase warnings + show phrase | the create phrase step; Wallet ▸ Show Recovery Phrase… | `authorize(revealSecret)`, `revealMnemonic` |
| IOS-004 | Verify | four positions, six chips (the four words and two other words of the phrase), tapped in order; a wrong chip is marked and does not advance | none until the last chip |
| IOS-007 | Restore with live checks | Welcome ▸ Restore ▸ phrase (live per-word check, suggestions) ▸ Options (BIP39 passphrase, Dash Core compatibility, scan start) ▸ (no vault: Choose passphrase) ▸ Done | `checkMnemonic` per edit, `createVault`, `importWallet(phrase, bip39, {birthHeight: 0 or the estimate})` |
| IOS-010 | Set / confirm the passphrase | Choose passphrase (twice, strength meter, dash-qt's warning); "Continue without a passphrase" under Advanced | `createVault(passphrase \| null)` |
| IOS-012 | Lockout | lock screen: attempts left, the wait, disabled at 8 failures | `lockStatus` at start, `unlock` |
| IOS-013 | Lock screen | passphrase, For mixing only, Forgot passphrase? | `unlock`, `unlockMixingOnly` |
| IOS-014 | Forgot passphrase | lock screen ▸ Forgot passphrase? ▸ Wallet (several) ▸ What happens (other wallets lose their keys) ▸ Phrase ▸ New passphrase ▸ Done | `checkMnemonic`, `recoverWithMnemonic` |

## Where each secret lives

| Secret | Renderer | Crosses IPC | Rust side |
|---|---|---|---|
| New recovery phrase | private field of `OnboardingModel` from `generateMnemonic` until `importWallet` returns; the word list exists only while the phrase step shows; the verify step renders only its six chip words | twice: `generateMnemonic`'s reply, `importWallet`'s argument | `Zeroizing` on receipt (E0-13 D10), then the vault |
| Restored phrase | the textarea's DOM value and the model's normalized copy until `importWallet` returns or the flow ends | once per edit (`checkMnemonic`), once for `importWallet` | `Zeroizing` |
| BIP39 passphrase | model field, input cleared on leaving the step | `importWallet`, `recoverWithMnemonic` | `Zeroizing` |
| Vault passphrase | model field from Choose passphrase to `createVault`; the lock screen and the unlock sheet keep only the field's value, cleared after each attempt | `createVault`, `unlock`, `authorize`, `recoverWithMnemonic` | `Zeroizing` |
| Revealed phrase | the recovery sheet's state while it shows, dropped on close | `revealMnemonic`'s reply | grant single use |

Never: the TanStack query cache, `ui` (the shell store), `localStorage`, `console`, the window title, the
tray, an error text built from the engine's `message` (D16), a screenshot of real data (the T1 run
does not capture phrase screens; the visual baselines show fixture phrases). JavaScript strings
cannot be zeroed: dropping every reference is the most the renderer can do (G-01 H-01, E0-13 D10).

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Onboarding is a full-window surface, not a sheet. It replaces the shell while the network has no wallet (first run: no Cancel), and covers the shell for Wallet ▸ Add Wallet… (with Cancel). The lock screen stays above it. The shell store holds only whether it is open (`ui.onboarding`), never its state. | iOS and Android onboard full screen. The lock screen must cover whatever is open (U-01 D5/D20). |
| D2 | Order. Create: the passphrase (when the network has no vault) comes **before** the phrase, and the wallet is added at the verify step's last correct chip: `createVault` then `importWallet`, then the model drops the phrase. Restore: phrase ▸ options ▸ passphrase ▸ add, as on iOS. Nothing is stored before then; an abandoned flow leaves no vault. | The renderer must not keep the phrase after the verify step, and IOS-002 stores the phrase only once it is verified. iOS sets the PIN before the phrase too. Creating the vault first would leave an empty encrypted vault behind an abandoned flow, which a forgotten passphrase would then make unusable (`destroyVault` needs it). |
| D3 | A vault that is locked or unlocked for mixing only is unlocked right after Welcome (the unlock step), before any secret is entered. If the import still fails as locked (the vault locked meanwhile), the model keeps the phrase for that one retry, asks for the passphrase and adds the wallet again; Cancel drops it. | Swift `lockedVaultIsUnlockedBeforeAddingAWallet`; keeps the phrase out of the renderer while the user hunts for a passphrase. |
| D4 | Verify (IOS-004): four positions drawn at random and asked in ascending order; six chips, the four words and two other distinct words of the phrase, shuffled; `crypto.getRandomValues` (a seeded generator in tests). A wrong chip marks the step ("Wrong word. Try again.", the chips shake) and does not advance. The phrase step needs "I have written down my recovery phrase" checked. | The Swift port (and its tests); UX-SPEC §4.3. |
| D5 | Phrase length: 12 words, 24 under Advanced options on Welcome (IOS-002). English only for new phrases. | iOS offers 12; dash-qt and Swift offered 24 too. |
| D6 | The network comes from the launch (`--network`); Welcome shows its capsule and has no network picker. Switching networks before a wallet exists (IOS-106) waits for the network-switch work in U-04. | E0-13 D7: one active session, opened by the launch; a picker here would reopen the session under the shell. |
| D7 | Restore (IOS-007): every edit is normalized (lower case, single spaces) and sent to `checkMnemonic`; a generation counter drops stale replies. It names the first unknown word ("Word 2 is not in the word list."), an unsupported length, or a bad checksum. A phrase that passes only Dash Core's checksum turns on and locks Dash Core compatibility (QT-104) with a note. Up to five English suggestions complete the last word (click or Tab). Options: BIP39 passphrase, Dash Core compatibility, and the scan start, from genesis by default, or from a date (a conservative height: 200 s blocks and a week's margin, 0 on regtest and devnets) or a block height. `lookahead` stays the engine's default. | Swift `OnboardingViewModel`; the engine refuses an unknown lookahead (DESIGN-fable U12). A late restore height loses transactions, an early one only costs time. |
| D8 | Lock screen (IOS-012, IOS-013): it reads `lockStatus` when it appears, so a restart keeps the wait and the count. After a wrong passphrase it says how many attempts remain; from the third failure it shows the wait (the longer of the policy's and the vault's `retryAfterSecs`, minutes below two hours, hours above) and refuses input until then; at 8 failures it stops taking a passphrase and offers only Forgot passphrase. | The vault already waits `6^(n−3)` minutes from the third failure (dw-vault `throttle_wait_secs`); the UI adds the count and the cut-off, as iOS. The count lives in the vault file, so it survives a restart. |
| D9 | "For mixing only" on the lock screen unlocks with `unlockMixingOnly` (QT-112); the shell then shows (only `locked` is covered) and anything that signs asks for a full unlock. | UX-SPEC §4.4; dash-qt. |
| D10 | Not on the lock screen yet: biometric unlock (Touch ID stays off until B4, ROADMAP U-08; Windows Hello later), Quick Receive (with U-03's Receive and QR) and Scan to Send (no camera intake on the desktop yet). | Their parts are other tasks'. Follow-ups. |
| D11 | Forgot passphrase (IOS-014): from the lock screen, and as `ForgotPassphrase` for U-04 Security. The wallet is picked when the vault holds secrets for more than one; a "What happens" step names every other wallet with secrets, which becomes watch-only; then the phrase (same live check), the BIP39 passphrase, a new passphrase twice, and `recoverWithMnemonic`. A mismatch says so and returns to the phrase. Done lists the wallets left without keys. An encrypted vault with no wallet secrets has nothing to recover, and the link is hidden. | Swift `SecurityViewModel`. Losing other wallets' keys without warning would be the worst surprise of the flow. |
| D12 | The unlock sheet (UX-SPEC §4.4 `UnlockSheet`) is `PassphraseSheet`: a small sheet on `card` with one field, the lock screen's wrong-passphrase and wait copy (shared `lockoutText`), cleared after each attempt. Show Recovery Phrase and the onboarding unlock step use it; U-03/U-04 use it for operation-triggered unlocks. | One prompt and one copy for every passphrase. |
| D13 | Show Recovery Phrase (IOS-003, IOS-108): Wallet ▸ Show Recovery Phrase… opens a sheet: the wallet (when several have a phrase), iOS's warning, the passphrase (encrypted vault; an unencrypted one uses the `unencrypted` credential), `authorize(revealSecret)` then `revealMnemonic`, the words in the phrase grid, the BIP39 passphrase if there is one. Closing it drops the phrase. Recording "backed up" for the IOS-005 reminder comes with the reminder (U-03 Home). | The grant is single use and the engine checks it; the renderer holds the phrase only while it is on screen. |
| D14 | Two menu rows: Wallet ▸ Add Wallet… (`wallet.add`, `ShellCommand::AddWallet`) and Wallet ▸ Show Recovery Phrase… (`wallet.recovery-phrase`, `ShellCommand::RecoveryPhrase`), no accelerators. Until U-04 builds Settings, the Settings route shows a Wallet card with the same two actions and Forgot passphrase. Asked for while locked, they are dropped (U-01 D20). | dash-qt's File ▸ Create Wallet…; menus are where desktop users look. The Settings card gives the flows a visible way in, which the T1 run (WebDriver cannot open native menus) also uses. |
| D15 | Screen capture (IOS-006): while a phrase is on screen (the create phrase and verify steps, the recovery sheet) the window is content-protected (`Window.setContentProtected`: macOS `NSWindowSharingNone`, Windows `WDA_EXCLUDEFROMCAPTURE`); the permission `core:window:allow-set-content-protected` is granted to the main window in `capabilities/vault.json`. Linux has no such API: the phrase step says screenshots and screen sharing can capture it. Best effort: a failed call is ignored. | iOS warns on screenshots; desktops cannot detect them, but two of three can exclude the window. |
| D16 | Onboarding, recovery and Forgot passphrase show copy chosen by error code, never the engine's `message` (a mnemonic error may quote a word). An unknown code shows "Something went wrong (`code`)." | Keeps phrase words off the screen outside the phrase grid. |
| D17 | Passphrase rules (QT-111): not empty, the two fields equal, at most 1024 characters; a strength meter (length and character classes, Swift `PassphraseStrength`) only guides. "Continue without a passphrase" (under Advanced, with dash-qt's warning) creates an unencrypted vault whose key the OS store keeps; where there is none, the engine's `vault.os_store_unavailable` says so. | dash-qt lets a wallet stay unencrypted; the Swift tests (`skippingEncryptionCreatesAnUnencryptedVault`). |
| D18 | Inputs that hold a secret are `type="password"` or carry `autocomplete="off"`, `autocapitalize="off"`, `spellcheck="false"`, `autocorrect="off"`, and are cleared when their step ends. The phrase grid has no Copy button. | Spell checkers and autofill keep what they see; iOS has no copy for the phrase. |
| D19 | Fixture (U-07): `generate_mnemonic`, `check_mnemonic`, `create_vault`, `import_wallet`, `unlock_mixing_only`, `recover_with_mnemonic` and `reveal_mnemonic` handlers; the unlock throttle follows dw-vault's schedule; known phrases (valid BIP39 phrases from fixed entropy) map to wallet ids. Scenarios `no-vault-wallet` (an unlocked vault, no wallet), `locked-throttled` (3 failures), `locked-disabled` (8 failures), `locked-two-wallets`. | The flows run end to end without an engine; visual baselines show only these phrases. |
| D20 | T1 and GL: `e2e-tauri/onboarding.ts` drives the real app (production renderer, real engine, WebKitGTK through tauri-driver) on a dashd regtest node with SPV: create with verify on a fresh data root, Add Wallet ▸ restore of a phrase dashd funded (its balance arrives through SPV), lock and unlock (wrong passphrase, attempts left), Show Recovery Phrase (equals the created phrase), Forgot passphrase with the restored phrase (the created wallet becomes watch-only), the new passphrase unlocks and the old one does not, and three failures keep their wait across a relaunch. | The ROADMAP's T1 acceptance for IOS-002/004/007/010/013/014. |

## Ported view-model tests

| Swift test | TypeScript test |
|---|---|
| (filled in as the tests land) | |
