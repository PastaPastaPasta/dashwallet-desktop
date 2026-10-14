# U-02: onboarding, lock screen, unlock sheet, recovery phrase, Forgot passphrase (Tauri)

ROADMAP U-02 on the Tauri stack (ADR 0003): the first-run and Add Wallet flows (create, verify,
restore with live checks, the vault passphrase), the lock screen with the iOS lockout policy, the
unlock sheet, Show Recovery Phrase, and Forgot passphrase. The view models are plain TypeScript
classes with no React in them (the Swift `WalletFeatures` models, ported); their tests are the
Swift tests ported by parity ID. The screens follow the mobile wallets (UX-SPEC §4.3, §4.4), not
dash-qt. Every call goes through dw-app's generated commands (E0-13); U-02 adds no command and no
permission: two menu rows (D14), one field of the existing `shell_sync` (D15), and one error code
of it, `app.screen_protection` (D27).

| Part | Where |
|---|---|
| View models | `apps/desktop/src/onboarding/{onboarding,lock,forgot,recovery,passphrase,words,birthHeight}.ts` |
| Their tests (by parity ID) | `apps/desktop/src/onboarding/*.test.ts` |
| Screens | `src/screens/{Onboarding,LockScreen,ForgotPassphrase,RecoveryPhrase,Settings}.tsx`, `src/components/{passphrase,phrase}.tsx` |
| Content protection | `ShellSync.secretShown`, `rust/crates/dw-app/src/shell/mod.rs` (`protect_secrets`, `Shell::protect`); the renderer's gate is `useSecretScreen` (`src/app/ui.ts`) and `ProtectionPending` (`src/components/phrase.tsx`) |
| Fixture handlers and scenarios | `src/fixture/{vault,handlers,scenarios}.ts` |
| Playwright | `e2e/onboarding.spec.ts` (flows, axe, light and dark, Chromium and WebKit), `e2e/visual.spec.ts` (baselines) |
| Real app on dashd regtest (T1, GL) | `e2e-tauri/onboarding.ts`, `scripts/e2e-tauri-onboarding.sh` (`pnpm e2e:tauri:onboarding`); the WebDriver client is `e2e-tauri/driver.ts`, shared with the U-07 smoke and U-03's regtest send |
| Menu rows | `rust/crates/dw-app/src/shell/menu.rs` (`wallet.add`, `wallet.recovery-phrase`) |

## Flow map

| Row | Flow | Steps (screens) | Engine calls |
|---|---|---|---|
| IOS-002 | Create | Welcome ▸ (no vault: Choose passphrase) ▸ Recovery phrase (12 words; 24 under Advanced options) ▸ Verify ▸ Done | `lockStatus`, `generateMnemonic(12\|24, english)`, then at the last correct chip `createVault(passphrase)` when there is no vault and `importWallet(phrase, "", {birthHeight: null})` |
| IOS-003 | Recovery phrase warnings + show phrase | the create phrase step; Wallet ▸ Show Recovery Phrase… | `authorize(revealSecret)`, `revealMnemonic` |
| IOS-004 | Verify | four positions, six chips (the four words and two other words of the phrase), tapped in order; a wrong chip is marked and does not advance | none until the last chip |
| IOS-007 | Restore with live checks | Welcome ▸ Restore ▸ phrase (live per-word check, suggestions) ▸ Options (BIP39 passphrase, Dash Core compatibility, scan start) ▸ (no vault: Choose passphrase) ▸ Done | `checkMnemonic` per edit, `createVault`, `importWallet(phrase, bip39, {birthHeight: 0 or the estimate})` |
| IOS-010 | Set / confirm the passphrase | Choose passphrase (twice, strength meter, dash-qt's warning); "Continue without a passphrase" under Advanced | `createVault(passphrase \| null)` |
| IOS-012 | Lockout | every passphrase prompt (lock screen, unlock sheet, onboarding unlock step, Show Recovery Phrase): attempts left, the wait, disabled at 8 failures | `lockStatus` at start, `unlock` / `authorize` |
| IOS-013 | Lock screen | passphrase, For mixing only, Forgot passphrase? | `unlock`, `unlockMixingOnly` |
| IOS-014 | Forgot passphrase | lock screen ▸ Forgot passphrase? ▸ Wallet (several) ▸ What happens (other wallets lose their keys) ▸ Phrase ▸ New passphrase ▸ Done | `checkMnemonic`, `recoverWithMnemonic` |
| IOS-006 | Screen capture | the window is content-protected while onboarding, Show Recovery Phrase or Forgot passphrase is open, and none of them shows a step before it is (D15, D27) | `shellSync({secretShown})` |

## Where each secret lives

| Secret | Renderer | Crosses IPC | Rust side |
|---|---|---|---|
| New recovery phrase | private field of `OnboardingModel` from `generateMnemonic` until `importWallet` returns; the word list exists only while the phrase step shows; the verify step renders only its six chip words | twice: `generateMnemonic`'s reply, `importWallet`'s argument | `Zeroizing` on receipt (E0-13 D10), then the vault |
| Restored phrase | the textarea's DOM value and the model's normalized copy until `importWallet` returns or the flow ends | once per edit (`checkMnemonic`), once for `importWallet` | `Zeroizing` |
| BIP39 passphrase | model field (the restore options field shows the model's copy, so going back and forth keeps it); Forgot's field is cleared when its phrase step ends | `importWallet`, `recoverWithMnemonic` | `Zeroizing` |
| Vault passphrase | model field from Choose passphrase to `createVault`, cleared whatever its outcome, and when no phrase could be generated; every prompt keeps only its field's value, cleared after each attempt | `createVault`, `unlock`, `authorize`, `recoverWithMnemonic` | `Zeroizing` |
| Revealed phrase | the recovery sheet's word list while it shows, dropped on close; a reveal that returns after the close is dropped; the BIP39 passphrase is never kept, only whether there is one | `revealMnemonic`'s reply | grant single use |

Never: the TanStack query cache, `ui` (the shell store), `localStorage`, `console`, the window title, the
tray, an error text built from the engine's `message` (D16), the clipboard (D18), a screenshot of
real data (the T1 run does not capture phrase screens, and checks that no file it leaves holds a
phrase or a passphrase; the visual baselines show fixture phrases). JavaScript strings cannot be
zeroed: dropping every reference is the most the renderer can do (G-01 H-01, E0-13 D10).

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Onboarding is a full-window surface, not a sheet. It replaces the shell while the network has no wallet (first run: no Cancel), and covers the shell for Wallet ▸ Add Wallet… (with Cancel). A locked vault shows the lock screen instead, and unmounts onboarding with what it held: Add Wallet… closes, the first run starts again at Welcome once unlocked (so a locked empty vault is unlocked on the lock screen, with its policy). The shell store holds only whether it is open (`ui.onboarding`, latched on while there is no wallet, so Done stays up after the wallet is added), never its state. Done closes it only once the new wallet is listed. | iOS and Android onboard full screen. The lock screen must cover whatever is open (U-01 D5/D20). A list still empty when Done closed would open onboarding again (review r1 #5). |
| D2 | Order. Create: the passphrase (when the network has no vault) comes **before** the phrase, and the wallet is added at the verify step's last correct chip: `createVault` then `importWallet`, then the model drops the phrase. Restore: phrase ▸ options ▸ passphrase ▸ add, as on iOS. Nothing is stored before then; an abandoned flow leaves no vault. | The renderer must not keep the phrase after the verify step, and IOS-002 stores the phrase only once it is verified. iOS sets the PIN before the phrase too. Creating the vault first would leave an empty encrypted vault behind an abandoned flow, which a forgotten passphrase would then make unusable (`destroyVault` needs it). |
| D3 | A vault unlocked for mixing only is unlocked fully right after Welcome (the unlock step, with the lock screen's policy: D12), before any secret is entered. If the import still fails as locked (the vault locked meanwhile), the model keeps the phrase for that one retry, asks for the passphrase and adds the wallet again; Cancel drops it. While an engine call runs (`busy`), Welcome's buttons, the prompts and the chips wait and a second submit is ignored: a double click must not generate a second phrase or add the wallet twice. A reply that arrives after the screen closed is dropped. | Swift `lockedVaultIsUnlockedBeforeAddingAWallet`; keeps the phrase out of the renderer while the user hunts for a passphrase. Review r1 #3, #7. |
| D4 | Verify (IOS-004): four positions drawn at random and asked in ascending order; six chips, the four words and two other distinct words of the phrase, shuffled; `crypto.getRandomValues` (a seeded generator in tests). A wrong chip marks the step ("Wrong word. Try again.", the chips shake) and does not advance. The phrase step needs "I have written down my recovery phrase" checked. | The Swift port (and its tests); UX-SPEC §4.3. |
| D5 | Phrase length: 12 words, 24 under Advanced options on Welcome (IOS-002). English only for new phrases. | iOS offers 12; dash-qt and Swift offered 24 too. |
| D6 | The network comes from the launch (`--network`); Welcome shows its capsule and has no network picker. Switching networks before a wallet exists (IOS-106) waits for the network-switch work in U-04. | E0-13 D7: one active session, opened by the launch; a picker here would reopen the session under the shell. |
| D7 | Restore (IOS-007): every edit is normalized (lower case, single spaces) and sent to `checkMnemonic`; a generation counter drops stale replies. It names the first unknown word ("Word 2 is not in the word list."), an unsupported length, or a bad checksum. A phrase that passes only Dash Core's checksum turns on and locks Dash Core compatibility (QT-104) with a note. Up to five English suggestions complete the last word (click or Tab). Options: BIP39 passphrase, Dash Core compatibility, and the scan start, from genesis by default, or from a date or a block height. A date is read as a local date (today is never "in the future") and becomes the height of the last Dash Core checkpoint mined at least a week before it (`birthHeight.ts`: the mainnet and testnet checkpoints with their block times, as rust-dashcore's dash-spv ships them; no extrapolation past the last one; 0 on regtest and devnets). A typed height that is not a whole number from 0 up scans from genesis (D30), never `NaN` (which crosses IPC as `null`, the tip). The scan choice and the typed date or height live in the model, so going back and forth keeps them. `lookahead` stays the engine's default. | Swift `OnboardingViewModel`; the engine refuses an unknown lookahead (DESIGN-fable U12). A late restore height loses transactions, an early one only costs time: the first estimate (200 s blocks from genesis) overshot testnet's real height by up to 450,000 blocks (review r1 #1). |
| D8 | Lock screen (IOS-012, IOS-013): it reads `lockStatus` when it appears, so a restart keeps the wait and the count. After a wrong passphrase it says how many attempts remain; from the third failure it shows the wait (the longer of the policy's and the vault's `retryAfterSecs`, minutes below two hours, hours above) and refuses input until then; at 8 failures it stops taking a passphrase, moves focus to Forgot passphrase and offers only that. An encrypted vault with no wallet's keys has nothing Forgot passphrase could recover: past 8 it keeps the vault's growing waits instead of stopping for good. | The vault already waits `6^(n−3)` minutes from the third failure (dw-vault `throttle_wait_secs`); the UI adds the count and the cut-off, as iOS. The count lives in the vault file, so it survives a restart. A cut-off with no way out would lock the user out of the vault for good (review r1 #4). |
| D9 | "For mixing only" on the lock screen unlocks with `unlockMixingOnly` (QT-112); the shell then shows (only `locked` is covered) and anything that signs asks for a full unlock. | UX-SPEC §4.4; dash-qt. |
| D10 | Not on the lock screen yet: biometric unlock (Touch ID stays off until B4, ROADMAP U-08; Windows Hello later), Quick Receive (with U-03's Receive and QR) and Scan to Send (no camera intake on the desktop yet). | Their parts are other tasks'. Follow-ups. |
| D11 | Forgot passphrase (IOS-014): a sheet opened from the lock screen's link and rendered by the shell (`ui.forgotPassphrase`), not by the lock screen: `recoverWithMnemonic` unlocks the vault, which unmounts the lock screen, and the sheet must stay to say what happened. The lock screen under it is inert. U-04 Security reuses `ForgotPassphraseSheet`. The wallet is picked when the vault holds secrets for more than one; a "What happens" step names every other wallet with secrets, which becomes watch-only; then the phrase (same live check), the BIP39 passphrase, a new passphrase twice, and `recoverWithMnemonic`. The phrase and new-passphrase steps have Back (the phrase is typed again). A mismatch says so and returns to the phrase. Done lists the wallets left without keys. The sheet cannot be closed while the reset runs. An encrypted vault with no wallet secrets has nothing to recover, and the link is hidden (D8). | Swift `SecurityViewModel`. Losing other wallets' keys without warning would be the worst surprise of the flow, so closing mid-reset must not hide Done (review r1 #7). |
| D12 | One prompt and one policy for every vault passphrase: `PassphrasePrompt` (one field, cleared after each attempt) driven by `LockModel.attempt`, so the lock screen, the unlock sheet, the onboarding unlock step and Show Recovery Phrase all show the attempts left and the wait, and stop at 8. The unlock sheet (UX-SPEC §4.4 `UnlockSheet`) opens from the status bar's "Unlocked for mixing only" (`dialog: "unlock"`); U-03/U-04 use it for operation-triggered unlocks. | dw-vault counts a failure on every passphrase check (`check_passphrase`, `authorize` too), so a prompt without the policy would let guessing go on past the cut-off (review r1 #4). |
| D13 | Show Recovery Phrase (IOS-003, IOS-108): Wallet ▸ Show Recovery Phrase… opens a sheet: the wallet (when several have a phrase), iOS's warning, the passphrase (encrypted vault; an unencrypted one uses the `unencrypted` credential), `authorize(revealSecret)` then `revealMnemonic`, the words in the phrase grid, and a note when the wallet also has a BIP39 passphrase (which is not shown: it never leaves the reveal's reply). Closing it drops the phrase, and a reveal that returns after the close is dropped (its grant revoked). Recording "backed up" for the IOS-005 reminder comes with the reminder (U-03 Home). | The grant is single use and the engine checks it; the renderer holds the phrase only while it is on screen. |
| D14 | Two menu rows: Wallet ▸ Add Wallet… (`wallet.add`, `ShellCommand::AddWallet`) and Wallet ▸ Show Recovery Phrase… (`wallet.recovery-phrase`, `ShellCommand::RecoveryPhrase`), no accelerators. Until U-04 builds Settings, the Settings route shows a Wallet card with the same two actions (Forgot passphrase is the lock screen's). Asked for while locked, they are dropped (U-01 D20); the rows stay enabled. | dash-qt's File ▸ Create Wallet…; menus are where desktop users look. The Settings card gives the flows a visible way in, which the T1 run (WebDriver cannot open native menus) also uses. |
| D15 | Screen capture (IOS-006): while onboarding, Show Recovery Phrase or Forgot passphrase is open, before any of its steps shows and until it closes, the window's content protection is requested (macOS `NSWindowSharingNone`, Windows `WDA_EXCLUDEFROMCAPTURE`). The renderer reports it in `ShellSync.secretShown` through the existing `shell_sync`, one call at a time and in order; dw-app's `protect_secrets` applies it with `set_content_protected` on the Rust side. No window permission is granted, so the renderer cannot drop protection by itself. No step of the surface shows before the native protection request has returned successfully, and none shows if it fails (D27). Residual: the request returning is not proof the platform applied it; a request the platform silently ignores is not detected (BL-149 tracks reading the state back). Linux has no such API: the phrase step's warning says to make sure no one can see the screen. | iOS warns on screenshots; desktops cannot detect them, but two of three can exclude the window. Protecting only while the grid showed painted it a frame or two before protection landed, and two async calls could land out of order (review r1 #2). |
| D16 | Onboarding, recovery and Forgot passphrase show copy chosen by error code, never the engine's `message` (a mnemonic error may quote a word). An unknown code shows "Something went wrong (`code`)." | Keeps phrase words off the screen outside the phrase grid. |
| D17 | Passphrase rules (QT-111): not empty, the two fields equal, at most 1024 characters; a strength meter (length and character classes, Swift `PassphraseStrength`) only guides. "Continue without a passphrase" (under Advanced, with dash-qt's warning) creates an unencrypted vault whose key the OS store keeps; where there is none, the engine's `vault.os_store_unavailable` says so. | dash-qt lets a wallet stay unencrypted; the Swift tests (`skippingEncryptionCreatesAnUnencryptedVault`). |
| D18 | Inputs that hold a secret are `type="password"` or carry `autocomplete="off"`, `autocapitalize="off"`, `spellcheck="false"`, `autocorrect="off"`, and are cleared when their step ends. The phrase grid has no Copy button; its words and the verify step's slots and chips cannot be selected (`user-select: none`) and refuse copy events. | Spell checkers, autofill and clipboard managers keep what they see; iOS has no copy for the phrase. |
| D19 | Fixture (U-07): `generate_mnemonic`, `check_mnemonic`, `create_vault`, `import_wallet`, `unlock_mixing_only`, `recover_with_mnemonic` and `reveal_mnemonic` handlers; the unlock throttle follows dw-vault's schedule; known phrases (valid BIP39 phrases from fixed entropy) map to wallet ids. Scenarios `no-wallet-vault` (an unlocked vault, no wallet), `locked-throttled` (3 failures), `locked-disabled` (8 failures), `locked-two-wallets`. | The flows run end to end without an engine; visual baselines show only these phrases. |
| D20 | T1 and GL: `e2e-tauri/onboarding.ts` drives the real app (production renderer, real engine, WebKitGTK through tauri-driver) on a dashd regtest node with SPV: create with verify on a fresh data root, Add Wallet ▸ restore of a phrase dashd funded (its balance arrives through SPV), lock and unlock (wrong passphrase, attempts left; for mixing only the engine refuses a spend grant without a credential), Show Recovery Phrase (equals the created phrase), Forgot passphrase with the restored phrase (the engine's `list_wallets` then reports the created wallet watch-only and the restored one not), the new passphrase unlocks and the old one does not, and three failures keep their wait across a relaunch. Settings opens with its menu shortcut through xdotool (in the app the menu bar owns the shortcuts). `pnpm e2e:tauri:onboarding` runs it; it needs Docker, the regtest image and xdotool, so CI does not run it yet. | The ROADMAP's T1 acceptance for IOS-002/004/007/010/013/014. Evidence below. |

| D21 | `filledBlue` buttons (`components/basics.tsx`, `.dash-button-filledBlue`) fill with `--dw-role-accent` (#006FC3), not the brand blue #008DE4. `plainWhite` is the lock screen's link style. U-03 added the same style; since the merge of main 6b989904 there is one rule, U-03's (its text token `--dw-role-text-on-hero` is #FFFFFF in every theme, as `--dw-c-button-filled-blue-content` was), so the pixels are unchanged. | White text on #008DE4 is 3.5:1; axe flags it, and §2.1a names #006FC3 the AA accent for fills under white text. |
| D22 | A create flow whose vault passphrase the engine refuses at the last chip (`vault.passphrase_rejected`, `vault.os_store_unavailable` after "Continue without a passphrase") returns to Choose passphrase with the error, keeping the verified phrase; the passphrase it tried is dropped either way. The next one adds the wallet without a second phrase. | The copy says "Choose another one", which Back could not reach without Cancel throwing the verified phrase away (review r1 #6). |
| D23 | The Settings route shows `SettingsPage` (D14's Wallet card and a placeholder) instead of U-01's "not yet" page; `NOT_YET.settings` stays for U-04 to remove. | The flows need a way in that WebDriver and a pointer can reach. |
| D24 | The visual baselines mask the verify step's slots and chips. The other onboarding, lock-wait, Forgot and recovery baselines show fixture data only. | The challenge is drawn at random (D4). |
| D25 | The restore options read a creation date against the checkpoints in `birthHeight.ts` (D7), leaving out mainnet 750,000: in rust-dashcore's table at 67b307c6d its block time (1491953700, 2017-04-12) is earlier than 700,000's (2017-07-08), which would map April–June 2017 dates past their blocks. | Never start a scan past the wallet's first transaction. Follow-up: correct that checkpoint in rust-dashcore. |
| D26 | Onboarding's Back is disabled, and `back()`/`cancel()` do nothing, while an engine call runs (an unlock is one Argon2id run). A reply that arrives after the screen went away is dropped by an epoch that `dispose()` bumps, not a flag, so a model that React StrictMode disposes and mounts again keeps working; the recovery sheet does the same. A phrase is not valid while a newer edit is still being checked. Every engine reply is checked against the epoch (D29). | Review r2: Back during a call let its reply continue a flow the user had left; Continue could act on an answer that no longer held. |
| D27 | A secret surface (onboarding from Welcome, the Forgot sheet, the recovery sheet) shows only "Preparing…" until the native protection request has returned successfully. `shell_sync` answers once the change has run: dw-app runs `set_content_protected` on the main thread (from another thread Tauri only queues it; on it, tauri-runtime-wry 2.12 applies it at once) and awaits it. The renderer (`useSecretScreen`) shows the steps, their inputs and Back only once the latest call with `secretShown: true` has answered. If the change could not run (`app.screen_protection`: no main window, the event loop gone, no protection set up), the surface stays blocked with an error and Try again; it never falls back to showing the secret. Add Wallet offers Cancel while preparing and after a failure (the first run has none). On Linux, which has no content protection (D15), the answer is immediate. Residual (D15): the platform call's own result is not reported back, so a request the platform ignores still opens the surface; BL-149 tracks a read-back of the window's state. | Review r1-Sol (Major): D15 asked for protection at mount but did not wait for it, so a quick first step could paint before it landed. A phrase shown without the protection the user was promised is worse than none. |
| D28 | The cut-off at 8 failures stays renderer policy (D8, D12). dw-vault keeps only its throttle, `6^(n−3)` minutes from the third failure, counted on every passphrase check and kept in the vault file; there is no hard lockout in the engine. | The throttle makes guessing infeasible: the ninth try waits 6⁵ minutes (5.4 days), each next one six times longer. A hard stop in the engine could lock a user out of their funds for good, where Forgot passphrase has nothing to recover (D8). Manager ruling on review r1-Sol. |
| D29 | Late replies. Onboarding checks the epoch (D26) after every engine call (`lockStatus`, `generateMnemonic`, `unlock`, `createVault`, `importWallet`), on success and on error, before it changes the model. The lock model numbers its reads and attempts: a `lockStatus` read that answers after an attempt started or ended (or after a newer read started) keeps only what the vault holds (`canRecover`, and the lock state when none is known yet), as the attempt's count and wait are newer; with `canRecover` it judges that count again, so eight failures of a recoverable vault reach the cut-off rather than a wait. | Review r1-Sol minors 3 and 4: a late `importWallet` or `lockStatus` reply moved a closed screen on, and the lock screen's first read could put back the count and wait from before the first attempt. Review r2-Sol minor 1: an eighth failure judged before that read was a wait, not the cut-off. |
| D30 | A typed scan height that is not a safe whole number from 0 up (a fraction, a negative, an infinite or no number) scans from genesis (0), not from a truncated or rounded height. | D7: a late start loses transactions, and "1234.7" is a typo, not a height (review r1-Sol minor 1). |
| D31 | Merge of main 6b989904 (U-03 D22). U-03's sheet layer sits between the shell and the App-level overlays, which stack by DOM order (no z-index): shell, sheet layer, transaction detail and dialogs, onboarding, lock screen, Forgot passphrase. The layer is inert while locked (U-03) and also during onboarding, which covers it: Add Wallet from the menu can open over Pay's confirm sheet, and Tab must not reach it. A dialog asked for during onboarding, while locked, or behind either kind of sheet is dropped (U-01 D20). App-level sheets (recovery, unlock, Forgot) are outside the layer's provider, so they do not portal. | Both sides' overlay rules, kept together: the lock screen and every secret surface stay above anything the shell opened. |
| D32 | Merge of main 6b989904. One WebDriver client: `e2e-tauri/driver.ts` from main (it writes tauri-driver's output to a file, as a pipe deadlocks `spawnSync`) with the calls T1 needs added (`findAll`, `clear`, `scriptAsync`, `script` with arguments). T1's own `webdriver.ts` is gone; `driverGone` (the driver's process group has exited) replaces its wait for the pipe to close before the data root is checked. | Two copies of the client would drift, and the pipe they shared was the deadlock U-03 fixed. |
| D33 | Merge of main 6b989904. `Backend.authorize` and `revokeGrant` were added on both sides; one copy is kept, with `walletId: string \| null` as the generated command has it (`null` only for `changeCredential`). U-03's Pay passes a wallet id, which the wider type accepts. | The narrower type would hide a valid engine call. |
| D34 | Merge of main 6b989904. U-02 and U-03 both defined `.sheet-actions`; one definition is kept, U-03's, which gives a sheet's footer buttons equal widths. The Forgot sheet's Back and Continue (and a lone button, such as Continue with one wallet or Cancel while protection is pending) now fill the footer instead of sitting compact at its end; `light-forgot` and `dark-forgot` are the only baselines regenerated in the merge. | UX-SPEC C23: every sheet's footer is "Cancel tintedGray L + primary filledBlue L, equal widths"; U-02's compact row did not follow it. |

## T1 and GL evidence (D20)

2026-10-14, branch `dw/u-02-onboarding` (app, renderer and harness built at `bd926ab1`, the
review r1-Sol fix round), agentbox (Xvfb,
WebKitGTK 2.52.6 through tauri-driver), `dwd-regtest-dashd:24.0.0-rc.2` (compose project
`dw-e2e-onboarding`). `scripts/e2e-tauri-onboarding.sh` → PASS, 13 steps:

1. fresh data root: Welcome, no Cancel
2. Choose passphrase: mismatch refused, passphrase set
3. phrase shown, verify: wrong chip refused, four words in order, wallet created
4. Open wallet: Overview
5. Show Recovery Phrase: passphrase, the created phrase
6. Add Wallet ▸ Restore: unknown word flagged, suggestion completes, scan from genesis
7. restored wallet: 1.50000000 tDASH through SPV from dashd (funding txid
   `97055e577dce6b92887f96d4371c1db77fa444c85a38098ac5673c08f5084dd2`, 2 blocks mined on top)
8. lock: wrong passphrase (attempts remaining), mixing only (a spend refused: credential_required, then mixing_only), unlock sheet to full
9. Forgot passphrase: Wallet 2 keeps its keys, Wallet 1 watch-only (list_wallets agrees)
10. the old passphrase fails, the new one unlocks
11. three failures: the wait, kept across a relaunch
12. after the wait: the new passphrase unlocks
13. no phrase or passphrase in the data root, the logs or the other files of OUT

Steps 8 and 9 call the real engine through the app's own IPC (`window.__TAURI_INTERNALS__.invoke`
over WebDriver's `execute/async`), past the UI. For mixing only, a `spend` grant with the
`unencrypted` credential is refused first by "require authentication for every payment" (on by
default; `vault.credential_required`); with that setting off (a `changeCredential` grant against
the passphrase), by the vault itself (`vault.mixing_only`); the setting is then turned on again.
After Forgot passphrase, `list_wallets` reports Wallet 1 `watchOnly` and Wallet 2 not.

Step 13 searches the files' bytes, on a failed run too, so it cannot see text drawn into a PNG:
the screenshots are kept off every screen with phrase words instead (the phrase, verify and
restore-entry steps, the revealed recovery phrase, Forgot's phrase step), and a failure
screenshot is dropped if a phrase screen came up while it was taken. U-07's `scripts/e2e-tauri.sh` smoke
passed on the `e2e1576d` binary (11 steps).

## Ported view-model tests

The Swift `WalletFeatures` tests, ported by parity ID (`apps/desktop/src/onboarding/*.test.ts`,
vitest). Rows without a Swift counterpart cover U-02's own decisions and review r1.

| Swift test | TypeScript test |
|---|---|
| `OnboardingViewModelTests.IOS002_createShowsAFreshPhraseAndStoresNothing` | `onboarding.test.ts` same name |
| `OnboardingViewModelTests.IOS002_offers12And24Words` | same name |
| `OnboardingViewModelTests.IOS006_phraseScreenIsProtectedFromCapture` | `IOS006_theWordsAreOnScreenOnlyOnThePhraseStep`; the protection itself in `e2e/onboarding.spec.ts` (`secretShown` from Welcome to Open wallet, and for the recovery sheet) |
| `OnboardingViewModelTests.IOS004_verifyChallengeNeedsTheRightWordsInOrder` | same name |
| `OnboardingViewModelTests.verifyWordChecksPositions` | `IOS004_verifyWordChecksPositions` |
| `OnboardingViewModelTests.QT111_passphraseRules` | same name |
| `OnboardingViewModelTests.IOS010_createEncryptsTheNewVaultAndImportsAsANewWallet` | same name |
| `OnboardingViewModelTests.skippingEncryptionCreatesAnUnencryptedVault` | `IOS010_skippingEncryptionCreatesAnUnencryptedVault` |
| `OnboardingViewModelTests.secondWalletSkipsVaultCreation` | same name |
| `OnboardingViewModelTests.lockedVaultIsUnlockedBeforeAddingAWallet` | `IOS013_lockedVaultIsUnlockedBeforeAddingAWallet` |
| `OnboardingViewModelTests.importRefusedAsLockedAsksForTheVaultPassphrase` | same name |
| `OnboardingViewModelTests.failureAfterUnlockReturnsToTheStepBeforeIt` | same name |
| `OnboardingViewModelTests.withoutAnAuthGateALockedVaultFails` | not ported: the renderer has no auth gate to leave out; the unlock step is always there (D3) |
| `OnboardingViewModelTests.QT104_restoreDefaultsScanFromGenesisWithoutLookahead` | same name |
| `OnboardingViewModelTests.IOS007_liveValidationAndSuggestions` | same name |
| `OnboardingViewModelTests.IOS007_unknownWordsAndBadChecksumAreReported` | same name |
| `OnboardingViewModelTests.QT104_coreOnlyChecksumForcesCoreCompatibility` | same name |
| `OnboardingViewModelTests.QT104_restoreWithBIP39PassphraseCoreCompatAndBirthHeight` | same name |
| `OnboardingViewModelTests.QT105_birthDateBecomesAConservativeHeight` | same name |
| `OnboardingViewModelTests.birthHeightEstimateIsBelowTheRealHeight` | same name, with testnet checkpoints (D7) |
| `OnboardingViewModelTests.importFailureCanBeRetriedFromTheSameStep` | same name |
| `OnboardingViewModelTests.createAfterAbandonedRestoreUsesNewWalletOptions` | same name |
| `OnboardingViewModelTests.IOS106_networkChoiceOnTheWelcomeScreen` | not ported: no network picker on Welcome (D6) |
| `OnboardingViewModelTests.passphraseStrengthScale` | same name |
| `LockViewModelTests.IOS012_policyIsThreeFreeThenSixfoldWaitsThenDisabledAtEight` | `lock.test.ts` same name |
| `LockViewModelTests.QT111_unlockSucceeds` | same name |
| `LockViewModelTests.QT112_unlockForMixingOnly` | same name |
| `LockViewModelTests.emptyPassphraseIsNotSent` | same name |
| `LockViewModelTests.IOS012_wrongPassphrasesCountDownThenThrottle` | same name |
| `LockViewModelTests.IOS012_fourthFailureWaitsSixMinutes` | same name |
| `LockViewModelTests.IOS012_eightFailuresDisableThePassphrase` | same name |
| `LockViewModelTests.IOS012_failureCountSurvivesARestart` | same name |
| `LockViewModelTests.engineThrottleWinsWhenLonger` | same name |
| `LockViewModelTests.IOS013_followsLockStateChanges` | not ported as a model test: the shell follows `lockStatus` (U-01); `e2e/onboarding.spec.ts` locks and unlocks through it |
| `LockViewModelTests.QT111_lock` | not ported: locking is U-01's toolbar and menu (`e2e/routes.spec.ts`) |
| `LockViewModelTests.durationText` | same name |
| `WalletsSecurityAndHomeTests.IOS014_forgotPassphraseChecksThePhraseThenReplacesTheVault` | `forgot.test.ts` same name |
| `WalletsSecurityAndHomeTests.IOS014_aPhraseOfAnotherWalletIsAMismatch` | same name |
| `WalletsSecurityAndHomeTests.IOS108_revealPhraseOfAChosenWalletRecordsTheBackup` | `recovery.test.ts` `IOS108_revealPhraseOfAChosenWalletNeedsThePassphrase`; recording the backup comes with the IOS-005 reminder (D13) |
| (U-02) | `onboarding.test.ts`: `IOS002_firstWalletAsksForThePassphraseBeforeThePhrase`, `IOS004_thePhraseIsDroppedOnceTheWalletIsAdded`, `abandoningCreateStoresNothing`, `D16_failuresShowCopyByCodeNeverTheEngineMessage`, `aMixingOnlyVaultIsUnlockedFullyBeforeCreating`, `emptyUnlockPassphraseIsNotSent`, `IOS007_aLateCheckDoesNotOverwriteANewerOne`, `IOS010_restoreAsksForThePassphraseLastWhenThereIsNoVault` |
| (U-02) | `lock.test.ts`: `IOS012_aWaitThatRanOutWhileClosedIsNotShown`, `IOS013_offersForgotPassphraseOnlyWhenTheVaultHoldsSecrets`; `forgot.test.ts`: `IOS014_thePhraseIsDroppedWhateverTheOutcome`, `IOS014_theBip39PassphraseIsPassedOn`, `IOS014_nothingToRecoverWithoutSecrets`, `aWalletWithoutASummaryIsNamedByItsId`; `recovery.test.ts`: `IOS108_aWrongPassphraseRevealsNothing`, `IOS003_closingDropsThePhrase`, `IOS108_anUnencryptedVaultUsesTheUnencryptedCredential`, `aGrantThatDidNotReachTheRevealIsRevoked`, `noWalletWithAPhrase` |
| (review r1) | `onboarding.test.ts`: `aSecondClickWhileACallRunsIsIgnored`, `aPhraseThatArrivesAfterTheScreenClosedIsDropped`, `aRefusedVaultPassphraseCanBeReplacedWithoutANewPhrase`, `everyWrongChipIsAnnounced`, `theScanStartSurvivesGoingBackAndForth`, `todayIsNotInTheFuture`; `recovery.test.ts`: `revealFollowsTheLockScreenPolicy`, `revealWaitsOutTheThrottle`, `aRevealThatEndsAfterTheSheetClosedIsDropped`; `forgot.test.ts`: `backFromTheNewPassphraseDropsThePhrase`, `backAfterALoadFailureLoadsAgain` |
| (review r2) | `onboarding.test.ts`: `backWhileAPhraseIsGeneratedIsIgnored`, `aModelMountedAgainStillWorks`, `backWhileUnlockingIsIgnored`; `recovery.test.ts`: `aSheetMountedAgainStillReveals`, `aRevealFailureAfterTheRightPassphraseIsExplained`, `anUnencryptedRevealTakesOneClick`; `forgot.test.ts`: `continueWaitsForTheNewestCheck` |
| (review r1-Sol) | `onboarding.test.ts`: `aFailedPhraseDropsTheChosenVaultPassphrase`, `aVaultStateThatArrivesAfterTheScreenClosedIsDropped`, `anImportThatSettlesAfterTheScreenClosedIsDropped`, `theScanStartSurvivesGoingBackAndForth` (fractions, negatives, infinities); `lock.test.ts`: `aReadThatAnswersAfterAnAttemptIsDropped`, `anEighthFailureBeforeTheFirstReadIsTheCutOff` (review r2-Sol); `e2e/onboarding.spec.ts`: "secret surfaces wait until the window is protected, and stay closed when it cannot be (D27)", "only the latest protection request counts: an older answer opens nothing (D27)", "Forgot passphrase waits until the window is protected (D27)"; dw-app `ipc_tests.rs`: `shell_sync_answers_once_the_protection_is_in_effect` |
