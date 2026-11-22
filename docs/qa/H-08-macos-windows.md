# H-08 by hand: install, upgrade and uninstall on macOS and Windows

The steps a person runs on a real Mac and a real PC before the release candidate is called good. CI covers the
upgrade path on Linux (`upgrade_path_tests`, see the H-08 section of [`PLAN-1.0.md`](../PLAN-1.0.md)) and launches
the bundles on hosted macOS and Windows runners. It does not cover what this script does: each OS's installer, an
upgrade in place over an older release, the default data root, the OS secret store, uninstalling, and the first
launch of an unsigned app on a machine that has never seen it.

Testnet only. No real recovery phrase, no mainnet address with funds. Fund wallets from the testnet faucet.

## What you need

| Machine | OS | Build |
|---|---|---|
| Mac, Apple silicon | macOS 15 or 26 | `dash-wallet-<v>-macos-universal.dmg` |
| Mac, Intel | macOS 13, 14 or 15 | the same universal dmg |
| PC, x64 | Windows 11 | `dash-wallet-<v>-windows-x64-setup.exe` |
| PC, x64 | Windows 10 22H2 | the same installer |
| PC, Arm (optional) | Windows 11 on Arm | the same x64 installer, under emulation |

Releases to install, from <https://github.com/PastaPastaPasta/dashwallet-desktop/releases>: `v0.1.0`, `v0.2.0`,
`v0.3.0`, and the candidate under test, written `RC` below (`vX.Y.Z-rc.N`). `<v>` is a release's version without
the `v`.

On each machine, a test user account that has never run Dash Wallet. Its data root is
`~/Library/Application Support/org.dash.wallet.desktop` on macOS and `%APPDATA%\org.dash.wallet.desktop` on Windows.
Check that the folder does not exist before section 2. Each upgrade run in section 3 starts from an empty data
root again (section 5 shows how to empty it).

Shorthand used below: **Home**, **Activity**, **Settings** are the sidebar items. "Note" means write the value into
the results table at the end.

## 1. Download and verify

For each release you install:

- [ ] Download the bundle and `SHA256SUMS` from the release page into a new empty folder.
- [ ] macOS: `shasum -a 256 -c SHA256SUMS --ignore-missing`. Expected: `dash-wallet-<v>-macos-universal.dmg: OK`.
- [ ] Windows (PowerShell): `(Get-FileHash dash-wallet-<v>-windows-x64-setup.exe).Hash` equals that file's line
      in `SHA256SUMS` (case does not matter).
- [ ] If the GitHub CLI is installed: `gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop`
      passes ([`verifying.md`](../release/verifying.md)).

## 2. Fresh install of the candidate and the unsigned first launch

Do this first, on every machine, in the clean account.

### macOS

- [ ] Open the dmg. Expected: a window with Dash Wallet and an Applications link. Drag Dash Wallet to Applications.
- [ ] Double-click Dash Wallet in Applications. Expected: macOS refuses to open it ("cannot be opened because Apple
      cannot check it" or "not opened"). Screenshot the prompt. Choose Done or Cancel, not Move to Trash.
- [ ] Open it the documented way. macOS 14 and earlier: right-click the app ▸ **Open** ▸ **Open**. macOS 15 and
      later: System Settings ▸ Privacy & Security ▸ **Open Anyway** next to "Dash Wallet was blocked", then confirm
      with your password. Expected: the app opens and the prompt does not come back on the next launch. If neither
      works, `xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"` must; note which path worked.
- [ ] Time from the click to the first window (splash or Welcome), with a stopwatch, on the second launch. Note it.
      The 1.0 target is two seconds (PLAN-1.0 lane B, P5).
- [ ] About this Mac and Activity Monitor ▸ Dash Wallet ▸ Kind: "Apple" on Apple silicon, "Intel" on an Intel Mac.
      Expected: the app runs natively on both (it is universal), never under Rosetta on Apple silicon.

### Windows

- [ ] Run the installer. Expected: SmartScreen "Windows protected your PC". Screenshot it. Choose **More info ▸
      Run anyway**. Expected: no administrator (UAC) prompt; the app installs for the current user only.
- [ ] Expected: a Start menu entry "Dash Wallet". Start it from there.
- [ ] Windows 10: if the app does not open and says WebView2 is missing, note it; install the Evergreen WebView2
      runtime from Microsoft and try again. That is a known limitation, not a pass.
- [ ] Time from the click to the first window, on the second launch. Note it.
- [ ] Windows on Arm (optional): Task Manager ▸ Details ▸ Architecture column shows `x64`; the app works the same.

### Both

- [ ] Welcome shows. Choose **Testnet** in the network picker on Welcome. If the app says the choice applies after
      a restart, quit (⌘Q on macOS, File ▸ Exit on Windows) and open it again. Expected: Testnet is active.
- [ ] Create a wallet with a passphrase. Write down the recovery phrase on paper (testnet only). Expected: Home
      with a zero balance, syncing.
- [ ] Fund it from the faucet. Expected: the payment shows in Activity, then confirms.
- [ ] Settings ▸ About shows the candidate's version. Tools ▸ Information shows the pre-release warning on a
      candidate.
- [ ] Screenshots in light and dark: Home, Activity, Settings, Settings ▸ About.

## 3. Upgrade in place from 0.1.0, 0.2.0 and 0.3.0

Run this three times on every machine, once for each old release (`OLD` below), each time from an empty data root.
Windows 10 and the Intel Mac may do only `OLD = v0.3.0` if time is short; Apple silicon and Windows 11 do all three.

Touch ID and Windows Hello are not in these releases or in the candidate, so there is no biometric step.

1. [ ] Install `OLD` as in section 2 (the unsigned launch prompts are the same). On macOS, replace any Dash Wallet
       already in Applications.
2. [ ] Choose **Testnet** on Welcome, quit and open again (0.1.0 saves the choice for the next launch and keeps
       wallet creation disabled until then), and check that Testnet is active.
3. [ ] Create wallet A **with** a passphrase. Fund it from the faucet and wait for one confirmation.
4. [ ] In wallet A:
       - Send 0.1 tDASH back to the faucet's return address (or a second wallet) with a recipient label;
       - add a transaction label to the faucet payment in Activity;
       - add an address-book entry;
       - create a receive request with an amount, a label and a message;
       - Settings: unit mDASH, auto-lock 15 minutes, dark theme, CoinJoin rounds 6;
       - Tools ▸ Sign/Verify: sign `h08 upgrade` with the first receive address. Copy the signature.
5. [ ] Add wallet B **without** a passphrase (**Continue without a passphrase**). Expected: the warning that the key
       is kept in this computer's secret store. Fund it or not; note its first receive address.
6. [ ] Note: OLD's version, the wallet names, each balance, every Activity row of A (txid, amount, status, label),
       the address book, the receive request, the Settings values above, the signature. Screenshot Home, Activity and
       Settings.
7. [ ] Quit OLD (⌘Q or File ▸ Exit). Keep a copy of the data root:
       - macOS: `ditto ~/Library/Application\ Support/org.dash.wallet.desktop ~/h08-backup-OLD`
       - Windows: `robocopy "%APPDATA%\org.dash.wallet.desktop" "%USERPROFILE%\h08-backup-OLD" /E`
8. [ ] Install the candidate over it: drag it to Applications and choose **Replace** (macOS); run the candidate's
       `setup.exe` (Windows). Expected on Windows: the installer upgrades without asking to uninstall first, or, if
       it asks, uninstalling keeps the data (do not tick any "delete the application data" box). Open it. The
       unsigned launch prompt appears again on macOS; open it the same way.
9. [ ] Expected:
       - the lock screen, not Welcome; the network is still Testnet;
       - wallet A's passphrase unlocks;
       - wallet B opens without a passphrase (its key comes from the secret store). On macOS a Keychain prompt
         asking to allow Dash Wallet access to its own item is acceptable if **Always Allow** makes it go away; note
         it. Any error that wallet B's key is missing is a blocker;
       - the same wallet names, balances, every Activity row with its label, the address book, the receive request
         and the Settings values from step 6;
       - Tools ▸ Sign/Verify verifies the old signature, and signing `h08 upgrade` again with the same address gives
         the same signature;
       - wallet A can send 0.01 tDASH, and the payment confirms.
10. [ ] Quit and open again. Expected: the same as step 9, with no new prompts.
11. [ ] Downgrade: install `OLD` again over the candidate and open it. Expected for 0.1.0: "Dash Wallet could not
        open the wallet: storage error: app database: migration: migration V2026110501__tx_fiat is missing from the
        filesystem" with Try again (seen on Linux). For 0.2.0 and 0.3.0: note what happens; a crash, a blank
        window, or data that looks gone is a finding, a clear refusal like 0.1.0's is not. Quit it.
12. [ ] Install the candidate again. Expected: it still opens the data root exactly as in step 9. Then empty the
        data root (section 5) before the next `OLD`.

## 4. Uninstall

On each machine, after section 3, with the candidate installed and a wallet in it.

### macOS

- [ ] Quit the app. Drag Dash Wallet from Applications to the Trash and empty it. Expected: nothing in
      Applications; the data root is still there (`ls ~/Library/Application\ Support/org.dash.wallet.desktop`).
- [ ] Install the candidate again and open it. Expected: the lock screen with the same wallets (an app removed by
      dragging to the Trash keeps its data; that is the macOS norm, and the notes say so).
- [ ] Full removal: quit, delete the app, then delete
      `~/Library/Application Support/org.dash.wallet.desktop`, `~/Library/Caches/org.dash.wallet.desktop` and
      `~/Library/WebKit/org.dash.wallet.desktop` if present. Keychain Access: search for `dash` and note the items
      Dash Wallet left (wallet B's key). Delete them. Install again. Expected: Welcome, no wallet.

### Windows

- [ ] Quit the app. Settings ▸ Apps ▸ Installed apps (Windows 11) or Apps & features (Windows 10) ▸ Dash Wallet ▸
      Uninstall. Expected: no UAC prompt; the Start menu entry and the install folder go. Note whether the
      uninstaller offers to delete the application data, and leave it unticked.
- [ ] Expected: `%APPDATA%\org.dash.wallet.desktop` is still there. Install again: the lock screen with the same
      wallets.
- [ ] Uninstall again, this time ticking the delete-data box if there is one. Expected: `%APPDATA%\org.dash.wallet.desktop`
      and `%LOCALAPPDATA%\org.dash.wallet.desktop` are gone. If there is no box, delete both by hand. Credential
      Manager ▸ Windows Credentials: note any entry Dash Wallet left (wallet B's key) and remove it. Install again.
      Expected: Welcome, no wallet.

## 5. Emptying the data root between runs

Quit the app first.

- macOS: `rm -rf ~/Library/Application\ Support/org.dash.wallet.desktop`, and remove the Dash Wallet items in
  Keychain Access.
- Windows (PowerShell): `Remove-Item -Recurse -Force "$env:APPDATA\org.dash.wallet.desktop"`, and remove the
  Dash Wallet entries in Credential Manager.

## 6. Optional: the fixture check on a Mac

On a Mac with a checkout of the candidate's commit and the Rust toolchain:
`cd rust && cargo test -p dw-app --lib upgrade_path_tests`. Expected:
`test upgrade_path_tests::every_release_data_root_opens_with_the_same_answers ... ok`. CI runs it on macOS on
`main`. It cannot run on Windows, where `dw-app`'s test binaries do not load ([`docs/ci.md`](../ci.md)); there,
section 3 is the check.

## 7. Results

One row per machine and run. Pass, fail (with the issue number) or skipped (with the reason).

| Machine, OS | Run | Verify | First launch (path, seconds) | Fresh install | Upgrade checks (step 9) | Reopen | Downgrade | Uninstall keep / remove |
|---|---|---|---|---|---|---|---|---|
| Mac Apple silicon, macOS … | fresh | | | | — | — | — | |
| Mac Apple silicon, macOS … | from 0.1.0 | | | — | | | | — |
| … | from 0.2.0 | | | — | | | | — |
| … | from 0.3.0 | | | — | | | | — |
| Mac Intel, macOS … | … | | | | | | | |
| PC x64, Windows 11 | … | | | | | | | |
| PC x64, Windows 10 22H2 | … | | | | | | | |
| PC Arm, Windows 11 (optional) | … | | | | | | | |

File each failure as an issue with the OS, the machine, the release pair and screenshots (testnet data only), and
link it here. A blocker or critical failure holds the release candidate. Post the screenshots on the release
pull request or tracking issue, committed to a `pr-artifacts/…` branch and linked by SHA-pinned raw URLs, never to
`main`.
