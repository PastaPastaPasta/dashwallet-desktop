# H-08 by hand: install, upgrade and uninstall on macOS and Windows

The steps a person runs on real Macs and PCs (or the VMs marked below) before the release candidate is called good.
CI checks the upgrade path on Linux (`upgrade_path_tests`, see the H-08 section of [`PLAN-1.0.md`](../PLAN-1.0.md))
and installs the bundles silently on hosted macOS and Windows runners. It does not check what this script does: the
interactive installer, an upgrade in place over an older release, each OS's default data root and secret store,
uninstalling, and the first launch of an unsigned app downloaded the ordinary way.

## Rules for the whole run

- Testnet only, with wallets made for this run. Fund them from the testnet faucet.
- Never write a recovery phrase, passphrase, private key or backup file into this document, the results table, an
  issue or a screenshot. Before you upload a screenshot, open it and crop or redact anything else private: a system
  prompt naming a Keychain or Credential Manager item, a path with your user name, another app's window. Caption each
  image with the state it shows and the release it was taken on.
- Keep a **worksheet** per machine (a local file, for example `h08-<machine>.md`, never uploaded as is). It holds the
  baselines each run compares against: wallet names, balances, txids, labels, addresses, the signature, settings.
  The results table at the end gets only pass, fail or skipped, with issue numbers.
- Each section ends with **Fail if**. Any of those is a failure to file (section 7). Anything else that differs
  from an "Expected" is a finding to note in the worksheet and the results.
- Shorthand: **Home**, **Activity**, **Settings** are the sidebar items. "Quit" is ⌘Q on macOS and File ▸ Exit on
  Windows.

## What you need

| Machine | OS | Build | Real hardware or VM |
|---|---|---|---|
| Mac, Apple silicon | macOS 15 or 26 | `dash-wallet-<v>-macos-universal.dmg` | **real** for section 2 (launch time, native-architecture check); a macOS VM on it is enough for sections 3 and 4 |
| Mac, Intel | macOS 13, 14 or 15 | the same universal dmg | **real** (an Intel macOS VM cannot run on Apple silicon) |
| PC, x64 | Windows 11 | `dash-wallet-<v>-windows-x64-setup.exe` | **real** for the launch time in section 2; a VM is enough for everything else |
| PC, x64 | Windows 10 22H2 | the same installer | VM is enough |
| PC, Arm (optional) | Windows 11 on Arm | the same x64 installer, under emulation | VM is enough (for example on an Apple silicon Mac) |

Releases, from <https://github.com/PastaPastaPasta/dashwallet-desktop/releases>: `v0.1.0`, `v0.2.0`, `v0.3.0`, and
the candidate under test, written `RC` below (`vX.Y.Z-rc.N`). `<v>` is a release's version without the `v`. Until
`v0.3.0` and the candidate are published, their rows in the results are **pending**, not skipped.

On each machine, a test user account that has never run Dash Wallet, with the OS's standard security settings
(Gatekeeper on its default, SmartScreen on). Its data root is `~/Library/Application Support/org.dash.wallet.desktop`
on macOS and `%APPDATA%\org.dash.wallet.desktop` on Windows (`$env:APPDATA\org.dash.wallet.desktop` in PowerShell).
Check that the folder does not exist before section 2.

Download every bundle **on the machine under test, with its ordinary browser**. A file copied in through a shared
folder, drag and drop, `curl` or `gh` carries no quarantine flag (macOS) or Mark of the Web (Windows), so Gatekeeper
and SmartScreen never prompt and section 2 passes without testing anything.

## 1. Download and verify

For each release you install:

- [ ] Download the bundle and `SHA256SUMS` from the release page into a new empty folder, with the browser.
- [ ] macOS: `shasum -a 256 -c SHA256SUMS --ignore-missing`. Expected: `dash-wallet-<v>-macos-universal.dmg: OK`.
- [ ] Windows (PowerShell): `(Get-FileHash dash-wallet-<v>-windows-x64-setup.exe).Hash` equals that file's line
      in `SHA256SUMS` (case does not matter).
- [ ] If the GitHub CLI is installed: `gh attestation verify <file> --repo PastaPastaPasta/dashwallet-desktop`
      passes ([`verifying.md`](../release/verifying.md)).

**Fail if:** a checksum does not match, or an attestation does not verify. Stop: do not install that file.

## 2. Fresh install of the candidate and the unsigned first launch

On every machine, in the clean account, before anything else has been installed.

### macOS

- [ ] Open the dmg. Expected: a window with Dash Wallet and an Applications link. Drag Dash Wallet to Applications.
- [ ] Double-click Dash Wallet in Applications. Expected: macOS refuses to open it (for example "cannot be opened
      because Apple cannot check it", or "Dash Wallet Not Opened"). Screenshot the prompt. Choose Done or Cancel,
      not Move to Trash. If no prompt appears, note that and why it may not (was the file downloaded some other
      way?).
- [ ] Open it the documented way. macOS 14 and earlier: right-click the app ▸ **Open** ▸ **Open**. macOS 15 and
      later: System Settings ▸ Privacy & Security ▸ **Open Anyway** next to the Dash Wallet message, then confirm.
      Expected: the app opens, and the prompt does not come back on the next launch. Note the path that worked.
      `xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"` is the fallback in the notes; if you need it,
      the documented path failed, and it does not count as a pass for that path.
- [ ] Time from the click to the first window (splash or notice), with a stopwatch, on the second launch. Note it.
      The 1.0 target is two seconds (PLAN-1.0 lane B, P5).
- [ ] Activity Monitor ▸ Dash Wallet ▸ the Kind column. Expected: "Apple" on Apple silicon, "Intel" on an Intel Mac
      (the app is universal, so never under Rosetta on Apple silicon).

### Windows

- [ ] Run the installer. Expected: SmartScreen "Windows protected your PC". Screenshot it. Choose **More info ▸
      Run anyway**. Expected: no administrator (UAC) prompt; the app installs for the current user only. If
      SmartScreen does not prompt, note it.
- [ ] If WebView2 is missing (some Windows 10 installs), the installer itself tries to download and install it.
      Note whether that worked. If the installer or the app fails because of WebView2, note the stage and the
      message before you install WebView2 by hand.
- [ ] Expected: a Start menu entry "Dash Wallet". Start it from there.
- [ ] Time from the click to the first window, on the second launch. Note it.
- [ ] Windows on Arm (optional): Task Manager ▸ Details ▸ the Architecture column shows `x64`; the app works the same.

### Both

- [ ] Expected on the first launch: the notice "Dash Wallet is an early release". Choose **I understand**.
      Expected: Welcome.
- [ ] Choose **Testnet** in the network picker on Welcome. If the app says the choice applies after a restart,
      quit and open it again. Expected: Testnet is active.
- [ ] Create a wallet with a passphrase. Write the recovery phrase on paper only. Expected: Home with a zero
      balance, syncing.
- [ ] Fund it from the faucet. Expected: the payment shows in Activity, then confirms.
- [ ] Settings ▸ About shows version `X.Y.Z` (the candidate's version without `-rc.N`; the exact candidate is the
      file you verified in section 1). Tools ▸ Information shows a warning banner (a candidate is a pre-release
      build; a higher-priority warning, such as the network one, may be the one shown). Note its text.
- [ ] Screenshots in light and dark: Home, Activity, Settings, Settings ▸ About.

**Fail if:** the documented way to open the app does not work; Windows asks for administrator rights; the app runs
under Rosetta on Apple silicon; the window takes more than 5 seconds on the second launch; the notice or Welcome
does not appear on the first launch; Testnet is not active after the restart; the wallet cannot be created or
funded; About shows another version. A missing Gatekeeper or SmartScreen prompt on a browser download is a
finding: write down the OS version and security settings.

## 3. Upgrade in place from 0.1.0, 0.2.0 and 0.3.0

There is one vault per network, and the passphrase choice is offered only for the first wallet. A second wallet
inherits the vault's protection. So each release pair needs **two runs**, each from an empty data root (section 5):

- run **E**: the vault behind a passphrase;
- run **S**: the vault's key in the OS secret store (**Continue without a passphrase**).

That is six runs per machine (0.1.0, 0.2.0 and 0.3.0, each E and S). If a machine cannot do all six, record each
pair it skips as **outstanding** in the results, not as covered by another machine. Do the Apple silicon Mac and
Windows 11 in full first.

Touch ID and Windows Hello are not in these releases or in the candidate, so there is no biometric step.

### Each run (`OLD` is the old release, `R` is E or S)

1. [ ] Empty the data root (section 5). Install `OLD` (section 2's steps; on macOS replace the app in
       Applications). Record whether the unsigned-launch prompt appears; it may not, since the machine has
       approved Dash Wallet before.
2. [ ] Dismiss the early-release notice. Choose **Testnet** on Welcome. 0.1.0 saves the choice for the next launch
       and keeps wallet creation disabled until then: quit and open it again. 0.2.0 and 0.3.0 switch at once.
       Check that Testnet is active.
3. [ ] Create wallet A. Run E: with a passphrase. Run S: **Continue without a passphrase**. Expected for S: the
       warning that the key is kept in this computer's secret store. Fund it from the faucet.
4. [ ] In wallet A:
       - send 0.1 tDASH back to the faucet's return address with a recipient label;
       - add a transaction label to the faucet payment in Activity;
       - add an address-book entry;
       - create a receive request with an amount, a label and a message;
       - Settings: unit mDASH, theme dark, CoinJoin rounds 6, and for run E auto-lock 5 minutes;
       - Tools ▸ Sign/Verify: sign `h08 upgrade` with the first receive address.
5. [ ] Wait until both payments in Activity are confirmed. Write **baseline 1** into the worksheet: `OLD`'s
       version, the wallet name, the balance, every Activity row (txid, amount, status, label), the address book,
       the receive request, the Settings values above, the signing address and the signature. Screenshot Home,
       Activity and Settings.
6. [ ] Quit `OLD`. Copy the data root to a new folder for this run and check that the copy has the same files:
       - macOS: `ditto ~/Library/Application\ Support/org.dash.wallet.desktop ~/h08-backup-<OLD>-<R>`, then
         `diff -rq ~/Library/Application\ Support/org.dash.wallet.desktop ~/h08-backup-<OLD>-<R>` prints nothing;
       - Windows (PowerShell): `robocopy "$env:APPDATA\org.dash.wallet.desktop" "$env:USERPROFILE\h08-backup-<OLD>-<R>" /E`,
         and the robocopy summary shows no failed files.
7. [ ] Install the candidate over it: drag it to Applications and choose **Replace** (macOS); run the candidate's
       `setup.exe` (Windows). Expected on Windows: the installer upgrades in place. If it asks to uninstall the old
       version first, note it, and leave **Delete the application data** unticked. Open the candidate.
8. [ ] Expected, without changing anything:
       - run E: the lock screen, not Welcome, and the passphrase unlocks. Run S: the wallet opens with no
         passphrase. A macOS Keychain prompt to let Dash Wallet use its own item is a finding if **Always Allow**
         makes it go away, and a failure if it does not or the wallet reports a missing key;
       - the network is Testnet;
       - everything in baseline 1 is the same;
       - Tools ▸ Sign/Verify verifies the baseline signature, and signing `h08 upgrade` with the same address gives
         the same signature.
9. [ ] Quit and open again (a cold restart). Expected: the same as step 8, with no new prompt.
10. [ ] Send 10 mDASH (0.01 tDASH) from wallet A to the faucet's return address, once. Wait until it confirms.
        Write **baseline 2**: the new balance and Activity row; everything else as baseline 1.
11. [ ] Quit and open again. Expected: baseline 2, read only (do not send again).
12. [ ] Downgrade: quit the candidate, install `OLD` over it and open it. Expected for 0.1.0: "Dash Wallet could not
        open the wallet: storage error: app database: migration: migration V2026110501__tx_fiat is missing from the
        filesystem" with Try again (seen on Linux). For 0.2.0 and 0.3.0: note what happens; a clear refusal like
        0.1.0's is expected. Quit it.
13. [ ] Install the candidate again and open it. Expected: baseline 2, read only.
14. [ ] Restore the step 6 copy: quit, replace the data root with the copy (macOS:
        `rm -rf ~/Library/Application\ Support/org.dash.wallet.desktop && ditto ~/h08-backup-<OLD>-<R> ~/Library/Application\ Support/org.dash.wallet.desktop`;
        Windows: delete `$env:APPDATA\org.dash.wallet.desktop`, then robocopy the backup back). Open the
        candidate. Expected: as step 8, then the send from step 10 appears once the wallet syncs (it is on the
        chain).
15. [ ] If this is the machine's last run, keep the data root and go to section 4. Otherwise go to the next run.

**Fail if** (blocker): Welcome shows instead of the lock screen (E) or the wallet (S); the passphrase does not
unlock (E); the wallet asks for a passphrase or reports a missing key (S); anything in baseline 1 is missing or
different in step 8 or 9; the network is not Testnet; the baseline signature does not verify or signing again
gives another signature; the send in step 10 fails; baseline 2 differs in step 11 or 13; step 14 differs from
step 8.
**Fail if** (finding): the installer will not upgrade over `OLD`; the downgraded `OLD` crashes, shows a blank
window or looks empty instead of refusing clearly.

## 4. Uninstall

On each machine, after its last run of section 3, with the candidate installed and a wallet in it.

### macOS

- [ ] Quit the app. Drag Dash Wallet from Applications to the Trash and empty it. Expected: nothing in
      Applications; the data root is still there (`ls ~/Library/Application\ Support/org.dash.wallet.desktop`).
- [ ] Install the candidate again and open it. Expected: the same wallet as baseline 2 (an app removed by
      dragging to the Trash keeps its data, as is usual on macOS).
- [ ] Full removal: quit, delete the app, then delete
      `~/Library/Application Support/org.dash.wallet.desktop`, `~/Library/Caches/org.dash.wallet.desktop` and
      `~/Library/WebKit/org.dash.wallet.desktop` if present. In Keychain Access, find this run's item: account
      `dek`, service `dash.platform-wallet-storage/<hex>` (run S only). Delete that item only. Install again.
      Expected: the early-release notice, then Welcome with no wallet.

### Windows

- [ ] Quit the app. Settings ▸ Apps ▸ Installed apps (Windows 11) or Apps & features (Windows 10) ▸ Dash Wallet ▸
      Uninstall. Expected: no UAC prompt; the uninstaller shows a **Delete the application data** checkbox. Leave it
      unticked. Expected: the Start menu entry and the install folder go; `$env:APPDATA\org.dash.wallet.desktop`
      stays.
- [ ] Install again and open it. Expected: the same wallet as baseline 2.
- [ ] Uninstall again, this time ticking **Delete the application data**. Expected:
      `$env:APPDATA\org.dash.wallet.desktop` and `$env:LOCALAPPDATA\org.dash.wallet.desktop` are both gone.
- [ ] Credential Manager ▸ Windows Credentials: find this run's entry (its name contains
      `dash.platform-wallet-storage`; run S only). Note whether the uninstaller removed it, then remove it. Install
      again. Expected: the early-release notice, then Welcome with no wallet.

**Fail if:** removing the app without the checkbox removes the wallet; the checkbox is missing; with it ticked,
either folder stays; the uninstaller asks for administrator rights; after full removal the app does not start at
the notice and Welcome. Deleting a folder by hand after the uninstaller left it does not turn a failure into a pass.

## 5. Emptying the data root between runs

Quit the app first.

- macOS: `rm -rf ~/Library/Application\ Support/org.dash.wallet.desktop`; after a run S, also delete that run's
  Keychain item (account `dek`, service `dash.platform-wallet-storage/<hex>`).
- Windows (PowerShell): `Remove-Item -Recurse -Force "$env:APPDATA\org.dash.wallet.desktop"`; after a run S, also
  remove that run's Credential Manager entry.

## 6. Optional: the fixture check on a Mac

On a Mac with a checkout of the candidate's commit and the Rust toolchain:
`cd rust && cargo test -p dw-app --lib upgrade_path_tests`. Expected:
`test upgrade_path_tests::every_release_data_root_opens_with_the_same_answers ... ok`. CI runs it on macOS on
`main`. It cannot run on Windows, where `dw-app`'s test binaries do not load ([`docs/ci.md`](../ci.md)); there,
section 3 is the check.

## 7. Results

One row per machine and run: pass, fail (with the issue number), skipped (with the reason), outstanding (a pair
not yet run) or pending (the release is not published yet). The worksheet holds the values behind each cell.

| Machine, OS | Run | Verify | First launch (path, seconds) | Fresh install | Upgrade (8, 9) | Send and reopen (10, 11) | Downgrade (12, 13) | Restore (14) | Uninstall keep / remove |
|---|---|---|---|---|---|---|---|---|---|
| Mac Apple silicon, macOS … | fresh | | | | — | — | — | — | |
| Mac Apple silicon, macOS … | 0.1.0 E | | | — | | | | | — |
| … | 0.1.0 S | | | — | | | | | — |
| … | 0.2.0 E | | | — | | | | | — |
| … | 0.2.0 S | | | — | | | | | — |
| … | 0.3.0 E | | | — | pending | | | | — |
| … | 0.3.0 S | | | — | pending | | | | — |
| Mac Intel, macOS … | … | | | | | | | | |
| PC x64, Windows 11 | … | | | | | | | | |
| PC x64, Windows 10 22H2 | … | | | | | | | | |
| PC Arm, Windows 11 (optional) | … | | | | | | | | |

File each failure as an issue with the OS, the machine, the release pair, the run (E or S) and redacted
screenshots, and link it here. A blocker or critical failure holds the release candidate. Post the screenshots on
the release pull request or tracking issue, committed to a `pr-artifacts/…` branch and linked by SHA-pinned raw
URLs, never to `main`.
