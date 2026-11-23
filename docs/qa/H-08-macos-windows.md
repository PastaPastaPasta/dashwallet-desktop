# H-08 by hand: install, upgrade, uninstall, DashPay and screen readers on macOS and Windows

The steps a person runs on real Macs and PCs (or the VMs marked below) before the release candidate is called good.
CI checks the upgrade path on Linux (`upgrade_path_tests`, see the H-08 section of [`PLAN-1.0.md`](../PLAN-1.0.md)).
On hosted runners it installs the Windows NSIS installer silently, and on macOS it mounts the dmg, copies the app to
a temporary folder and launches it there. It does not check what this script does: the interactive installer, an
upgrade in place over an older release, each OS's default data root and secret store, uninstalling, and the first
launch of an unsigned app downloaded the ordinary way.

The automated DashPay journeys (`apps/desktop/e2e-tauri/`) run on Linux only: they drive the app through
tauri-driver, and macOS has no WebDriver for its web view. The screen-reader walk is automated for Orca on Linux
only. Sections 7 to 9 cover the rest by hand: the DashPay journeys on a Mac and on Windows, VoiceOver and Narrator,
and the DashPay flows no automated test covers.

## Rules for the whole run

- Testnet only, with wallets made for this run. Fund them from the testnet faucet.
- Never write a recovery phrase, passphrase, private key or backup file into this document, the results table, an
  issue or a screenshot. Before you upload a screenshot, open it and crop or redact anything else private: a system
  prompt naming a Keychain or Credential Manager item, a path with your user name, another app's window. Caption each
  image with the state it shows and the release it was taken on.
- Keep a **worksheet** per machine (a local file, for example `h08-<machine>.md`, never uploaded as is). It holds the
  baselines each run compares against: wallet names, balances, txids, labels, addresses, the signature, settings.
  The results table at the end gets only the statuses section 10 lists, with issue numbers.
- A screen reader speaks whatever it lands on. With a reader on (section 8), use headphones on the recovery phrase
  screens, in a room where no one else can hear, and record no audio, video or screenshot of them; on macOS turn
  VoiceOver's caption panel off there, because it prints the spoken words on screen.
- Each section ends with its verdicts. **Fail if**: a failure to file (section 10). **Stop** or **incomplete**: not a
  product failure; redo the step or finish the report before going on. Anything else that differs from an
  "Expected" is a finding to note in the worksheet and the results.
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

Sections 7 to 9 need more than one machine or account:

| Section | What it needs | Real hardware or VM |
|---|---|---|
| 7 and 9, DashPay | two wallets, A and B, each in its own data root: A on the Apple silicon Mac and B on the Windows 11 PC (preferred, so each OS talks to the other), or two OS user accounts on one machine | VM is enough |
| 7.6, the self-test | the GitHub CLI and permission to run workflows on the repository; no machine of yours | CI (hosted runners) |
| 8, VoiceOver | a Mac with headphones | **real** Mac preferred: a macOS VM is enough for most checks, but its audio and the VoiceOver keys often misbehave |
| 8, Narrator | a Windows 11 PC with headphones | VM with working audio is enough |
| 9.1, opening an invitation | a third wallet, C, with no DashPay identity, and a testnet invitation link made outside this app | VM is enough |

Run sections 7 to 9 in **other test user accounts** than the one sections 2 to 5 use: section 3 empties that
account's data root and section 4 removes the app, and the DashPay wallets must outlive both. On Windows the
installer installs for the current user only, so install the candidate in each of these accounts too (section 2's
steps). On macOS the app in Applications serves every account.

Paths used below. Set them once per run in the shell you use, with the run's own name (for example `0.1.0-E`):

```sh
# macOS (Terminal, zsh)
ROOT="$HOME/Library/Application Support/org.dash.wallet.desktop"
RUN="0.1.0-E"; BAK="$HOME/h08-backup-$RUN"
```

```powershell
# Windows (PowerShell)
$Root = "$env:APPDATA\org.dash.wallet.desktop"
$Run = "0.1.0-E"; $Bak = "$env:USERPROFILE\h08-backup-$Run"
# Every folder and file under a root, with each file's SHA-256: two equal trees print the same list.
function Get-Tree($p) {
  $p = (Get-Item -LiteralPath $p -Force -ErrorAction Stop).FullName.TrimEnd('\')
  Get-ChildItem -LiteralPath $p -Recurse -Force -ErrorAction Stop | ForEach-Object {
    $rel = $_.FullName.Substring($p.Length)
    if ($_.PSIsContainer) { "D $rel" } else { "F $rel $((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256 -ErrorAction Stop).Hash)" }
  } | Sort-Object
}
```

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
- [ ] Double-click Dash Wallet in Applications. Expected: macOS blocks it (for example "cannot be opened because
      Apple cannot check it", or "Dash Wallet Not Opened"). Then branch:
      - **Blocked** (expected): screenshot the prompt, choose Done or Cancel (not Move to Trash), and open it the
        documented way. macOS 14 and earlier: right-click the app ▸ **Open** ▸ **Open**. macOS 15 and later: System
        Settings ▸ Privacy & Security ▸ **Open Anyway** next to the Dash Wallet message, then confirm. Expected: the
        app opens, and the prompt does not come back on the next launch. Note the path that worked.
        `xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"` is the fallback in the notes: if you need
        it, the documented path failed.
      - **Opened directly**: note it with the macOS version and the Privacy & Security setting for apps, and mark
        the override path **unexercised**. That is a finding, not a pass or a failure of the override.
- [ ] Activity Monitor ▸ Dash Wallet ▸ the Kind column. Expected: "Apple" on Apple silicon, "Intel" on an Intel Mac
      (the app is universal, so never under Rosetta on Apple silicon).

### Windows

- [ ] Run the installer. Expected: SmartScreen "Windows protected your PC". Then branch:
      - **Blocked** (expected): screenshot it and choose **More info ▸ Run anyway**;
      - **Ran directly**: note it with the Windows version and the SmartScreen setting, and mark the override path
        **unexercised** (a finding).

      Expected either way: no administrator (UAC) prompt; the app installs for the current user only. On the
      installer's last page, untick the option to run Dash Wallet now if there is one.
- [ ] If WebView2 is missing (some Windows 10 installs), the installer itself tries to download and install it.
      Note whether that worked. If the installer or the app fails because of WebView2, note the stage and the
      message before you install WebView2 by hand.
- [ ] Expected: a Start menu entry "Dash Wallet". Start it from there.
- [ ] Windows on Arm (optional): Task Manager ▸ Details ▸ the Architecture column shows `x64`; the app works the same.

### Both

- [ ] Expected on the first launch: the notice "Dash Wallet is an early release". Choose **I understand**.
      Expected: Welcome. Quit the app fully.
- [ ] Second launch, timed: start the app from Applications (macOS) or the Start menu (Windows) and time, with a
      stopwatch, from the click to the first window. Note it. The 1.0 target is two seconds (PLAN-1.0 lane B, P5).
- [ ] Choose **Testnet** in the network picker on Welcome. If the app says the choice applies after a restart,
      quit and open it again. Expected: Testnet is active.
- [ ] Create a wallet with a passphrase. Write the recovery phrase on paper only. Expected: Home with a zero
      balance, syncing.
- [ ] Fund it from the faucet. Expected: the payment shows in Activity, then confirms.
- [ ] Settings ▸ About shows version `X.Y.Z` (the candidate's version without `-rc.N`; the exact candidate is the
      file you verified in section 1). Tools ▸ Information shows a warning banner (a candidate is a pre-release
      build; a higher-priority warning, such as the network one, may be the one shown). Note its text.
- [ ] Screenshots in light and dark: Home, Activity, Settings, Settings ▸ About.

**Fail if:** the OS blocked the app and the documented way to open it does not work; Windows asks for administrator
rights; the app runs under Rosetta on Apple silicon; the timed second launch takes more than 5 seconds; the notice
or Welcome does not appear on the first launch; Testnet is not active after the restart; the wallet cannot be
created or funded; About shows another version.
**Finding** (note, not a failure): a second launch between 2 and 5 seconds (a P5 miss); an app that opened without
a Gatekeeper or SmartScreen block.

## 3. Upgrade in place from 0.1.0, 0.2.0 and 0.3.0

There is one vault per network, and the passphrase choice is offered only for the first wallet. A second wallet
inherits the vault's protection. So each release pair needs **two runs**, each from an empty data root (section 5):

- run **E**: the vault behind a passphrase;
- run **S**: the vault's key in the OS secret store (**Continue without a passphrase**).

That is six runs per machine (0.1.0, 0.2.0 and 0.3.0, each E and S). If a machine cannot do all six, record each
pair it skips as **outstanding** in the results, not as covered by another machine. Do the Apple silicon Mac and
Windows 11 in full first. End each machine with a run S, so section 4 has a secret-store entry to check.

Touch ID and Windows Hello are not in these releases or in the candidate, so there is no biometric step.

### Each run (`OLD` is the old release, `R` is E or S)

1. [ ] Empty the data root (section 5). Install `OLD` (section 2's steps; on macOS replace the app in
       Applications). Record whether the unsigned-launch prompt appears; it may not, since the machine has
       approved Dash Wallet before.
2. [ ] Dismiss the early-release notice. Choose **Testnet** on Welcome. 0.1.0 saves the choice for the next launch
       and keeps wallet creation disabled until then: quit and open it again. 0.2.0 and 0.3.0 switch at once.
       Check that Testnet is active.
3. [ ] Create wallet A. Run E: with a passphrase. Run S: **Continue without a passphrase**. Expected for S: the
       warning that the key is kept in this computer's secret store. Run S: write the new secret-store entry into the
       worksheet (keep it out of screenshots). macOS: Keychain Access, the item with account `dek` and service
       `dash.platform-wallet-storage/<64 hex digits>`. Windows: Credential Manager ▸ Windows Credentials ▸ Generic
       Credentials, the entry `dek.dash.platform-wallet-storage/<64 hex digits>`. Fund wallet A from the faucet.
4. [ ] In wallet A:
       - send 0.1 tDASH back to the faucet's return address with a recipient label;
       - add a transaction label to the faucet payment in Activity;
       - add an address-book entry;
       - create a receive request with an amount, a label and a message;
       - Settings: unit mDASH, theme dark, CoinJoin rounds 6, and for run E auto-lock 5 minutes;
       - Tools ▸ Sign/Verify: sign `h08 upgrade` with the first receive address.
5. [ ] Wait until both payments in Activity are confirmed. Write `OLD`'s version into the worksheet as the run's
       metadata. Write **baseline 1**: the wallet name, the balance, every Activity row (txid, amount, direction,
       label), the address book, the receive request, the Settings values above, the signing address and the
       signature. Statuses are not part of the baseline: a row may move on from a confirmation count to ChainLocked
       at any time. Screenshot Home, Activity and Settings.
6. [ ] Quit `OLD`. Set the run's paths (above), then copy the data root to a backup folder that does not exist yet,
       and compare the two:
       - macOS: `test ! -e "$BAK" && ditto "$ROOT" "$BAK" && diff -rq "$ROOT" "$BAK" && echo SAME`. Expected: `SAME`
         and nothing else;
       - Windows: `Test-Path $Bak` prints `False`; then
         `robocopy $Root $Bak /E; if ($LASTEXITCODE -ge 8) { throw "robocopy failed: $LASTEXITCODE" }`; then
         `Compare-Object (Get-Tree $Root) (Get-Tree $Bak)` prints nothing. Any line it prints is a difference.
7. [ ] Install the candidate over it: drag it to Applications and choose **Replace** (macOS); run the candidate's
       `setup.exe` (Windows). Expected on Windows: the installer upgrades in place. If it asks to uninstall the old
       version first, note it, and leave **Delete the application data** unticked. Open the candidate.
8. [ ] Expected, without changing anything:
       - run E: the lock screen, not Welcome, and the passphrase unlocks. Run S: the wallet opens with no
         passphrase. A macOS Keychain prompt to let Dash Wallet use its own item is a finding if **Always Allow**
         makes it go away, and a failure if it does not or the wallet reports a missing key;
       - the network is Testnet; Settings ▸ About shows the candidate's `X.Y.Z`;
       - everything in baseline 1 is the same (statuses may have moved on to more confirmations or ChainLocked);
       - Tools ▸ Sign/Verify verifies the baseline signature, and signing `h08 upgrade` with the same address gives
         the same signature.
9. [ ] Quit and open again (a cold restart). Expected: the same as step 8, with no new prompt.
10. [ ] Send 0.01 tDASH from wallet A to the faucet's return address, once: enter `10` in the amount field, which
        shows the unit `mtDASH` on testnet. Wait until it confirms. Write **baseline 2**: the new balance and the
        new Activity row with its txid; everything else as baseline 1.
11. [ ] Quit and open again. Expected: baseline 2, read only (do not send again).
12. [ ] Downgrade: quit the candidate, install `OLD` over it and open it. Expected for 0.1.0: "Dash Wallet could not
        open the wallet: storage error: app database: migration: migration V2026110501__tx_fiat is missing from the
        filesystem" with Try again (seen on Linux). For 0.2.0 and 0.3.0: note what happens; a clear refusal like
        0.1.0's is expected. Quit it.
13. [ ] Install the candidate again and open it. Expected: baseline 2, read only.
14. [ ] Restore the step 6 copy. Quit the candidate. Check the backup is there, move the live root aside (do not
        delete it), copy the backup in and compare:
        - macOS: `test -d "$BAK" && test ! -e "$ROOT.after-$RUN" && mv "$ROOT" "$ROOT.after-$RUN" && ditto "$BAK" "$ROOT" && diff -rq "$BAK" "$ROOT" && echo SAME`.
          Expected: `SAME`;
        - Windows: `Test-Path $Bak` prints `True` and `Test-Path "$Root.after-$Run"` prints `False`; then
          `Move-Item -LiteralPath $Root -Destination "$Root.after-$Run" -ErrorAction Stop`;
          `robocopy $Bak $Root /E; if ($LASTEXITCODE -ge 8) { throw "robocopy failed: $LASTEXITCODE" }`;
          `Compare-Object (Get-Tree $Bak) (Get-Tree $Root)` prints nothing.

        Run these one at a time, and if any of them prints an error or a `False`/`True` other than the one expected,
        stop there: do not run the next command, and do not open the app. A failed move leaves the live root where it
        was, and the copy that follows would write into it.

        Open the candidate. Expected: the unlock (E) or open (S) as in step 8; the wallet name, the address book, the
        receive request, the labels, the Settings values and the signature as in baseline 1. Once the wallet has
        synced: the balance of baseline 2, and exactly one Activity row with step 10's txid.
15. [ ] Delete the moved-aside root (`$ROOT.after-$RUN` or `$Root.after-$Run`). If this is the machine's last run,
        keep the data root (now in baseline 2's state) and go to section 4. Otherwise go to the next run.

**Fail if** (blocker): Welcome shows instead of the lock screen (E) or the wallet (S); the passphrase does not
unlock (E); the wallet asks for a passphrase or reports a missing key (S); anything in baseline 1 other than a
status that moved on is missing or different in step 8 or 9; the network is not Testnet; About does not show the
candidate's version; the baseline signature does not verify or signing again gives another signature; the send in
step 10 fails; baseline 2 differs in step 11 or 13; after step 14's sync, one of the wallet details step 14 lists, the baseline 2
balance or the single step 10 row is missing or different.
**Stop and redo the run** (not a product failure): a backup or restore comparison in step 6 or 14 shows a
difference. Before the redo, delete that attempt's backup folder (`rm -rf "$BAK"` or
`Remove-Item -Recurse -Force $Bak`), and restore the moved-aside root if step 14 moved it.
**Fail if** (finding): the installer will not upgrade over `OLD`; the downgraded `OLD` crashes, shows a blank
window or looks empty instead of refusing clearly.

## 4. Uninstall

On each machine, after its last run of section 3, with the candidate installed and a wallet in it.

### macOS

- [ ] Quit the app. Drag Dash Wallet from Applications to the Trash and empty it. Expected: nothing in
      Applications; the data root is still there (`ls ~/Library/Application\ Support/org.dash.wallet.desktop`); the
      Keychain item written in the worksheet at step 3 is still listed (data kept, so the next install can open the
      wallet).
- [ ] Install the candidate again and open it. Expected: the same wallet as baseline 2 (an app removed by
      dragging to the Trash keeps its data, as is usual on macOS).
- [ ] Full removal: quit, delete the app, then delete
      `~/Library/Application Support/org.dash.wallet.desktop`, `~/Library/Caches/org.dash.wallet.desktop` and
      `~/Library/WebKit/org.dash.wallet.desktop` if present. In Keychain Access, delete the exact item written in
      the worksheet at step 3, and nothing else (macOS has no uninstaller, so removing the item by hand is part of
      this step). Install again.
      Expected: the early-release notice, then Welcome with no wallet.

### Windows

- [ ] Quit the app. Settings ▸ Apps ▸ Installed apps (Windows 11) or Apps & features (Windows 10) ▸ Dash Wallet ▸
      Uninstall. Expected: no UAC prompt; the uninstaller shows a **Delete the application data** checkbox. Leave it
      unticked. Expected: the Start menu entry and the install folder go; `$env:APPDATA\org.dash.wallet.desktop`
      stays; the Credential Manager entry written in the worksheet at step 3 is still listed (data kept, so the next
      install can open the wallet).
- [ ] Install again and open it. Expected: the same wallet as baseline 2.
- [ ] Uninstall again, this time ticking **Delete the application data**. Expected:
      `$env:APPDATA\org.dash.wallet.desktop` and `$env:LOCALAPPDATA\org.dash.wallet.desktop` are both gone.
- [ ] Credential Manager ▸ Windows Credentials ▸ Generic Credentials: look for the exact entry written in the
      worksheet at step 3. Expected: gone. If it is still listed, record the failure (section 10) **before** you
      touch it; then remove that entry and nothing else. Install again. Expected: the early-release notice, then
      Welcome with no wallet.

**Fail if:** removing the app without the checkbox removes the wallet or its secret-store entry; the checkbox is
missing; with it ticked, either folder or the secret-store entry stays; the uninstaller asks for administrator rights; after full removal the app does not start at
the notice and Welcome. Deleting a folder or an entry by hand after the uninstaller left it does not turn a failure into a pass.

## 5. Emptying the data root between runs

Quit the app first.

- macOS: `rm -rf "$ROOT"`; after a run S, also delete that run's Keychain item, the exact one in the worksheet.
- Windows: `Remove-Item -Recurse -Force $Root`; after a run S, also remove that run's Credential Manager entry,
  the exact one in the worksheet.

**Stop before the next run if:** the data root still exists (`test -e "$ROOT"` succeeds, or `Test-Path $Root`
prints `True`), or the previous run S's secret-store entry is still listed.

## 6. Optional: the fixture check on a Mac

On a Mac with a checkout of the candidate's commit and the Rust toolchain:
`cd rust && cargo test -p dw-app --lib upgrade_path_tests`. Expected:
`test upgrade_path_tests::every_release_data_root_opens_with_the_same_answers ... ok`. CI runs it on macOS on
`main`. It cannot run on Windows, where `dw-app`'s test binaries do not load ([`docs/ci.md`](../ci.md)); there,
section 3 is the check.

**Fail if:** the test fails, or `running 0 tests` (the name matched nothing).

## 7. DashPay on a real Mac and on Windows (testnet)

By hand, the journeys `apps/desktop/e2e-tauri/` drives on Linux: `registration.ts`, `add-contact.ts`,
`bell-accept.ts`, `pay-contact.ts` and the profile edit. Wallet A is on the Mac and wallet B on the Windows PC, or
each in its own OS account (see "What you need"). VM is enough for this whole section.

Usernames, identity IDs and display names are public on Dash Platform, so they may stay in screenshots; redact the
rest as the rules say. Write into the worksheet each wallet's username, its identity ID (My Profile ▸ **Copy
identity ID**), each txid and the times noted below.

Choose usernames the network approves at once: 3 to 23 letters, digits and hyphens, **with a digit from 2 to 9**,
for example `h08mac2` or `h08win3` followed by a few letters of your own. A name without such a digit and shorter
than 20 characters goes to a vote of about two weeks: do not use one.

### Before you start

- [ ] In each account: open the candidate, choose **I understand**, choose **Testnet** on Welcome, then **Create a
      new wallet** with a passphrase (recovery phrase on paper only). Fund it from the faucet with enough for a
      username (the Join page shows the cost) and about 0.2 tDASH more for sections 7 and 9; fund it again if step 4
      of 7.1 says "You need at least …".
- [ ] Wait until Home's balance line no longer says "Still syncing, balance may change" and the faucet payment is
      confirmed in **Activity**.

### 7.1 Register a username (A, then B)

1. [ ] Home: the DashPay card says "Get a DashPay username" with **Join DashPay** ("Available after sync
       finishes" until the wallet has synced). The sidebar shows **Join DashPay** where **Contacts** will be. Choose
       **Join DashPay**.
2. [ ] Expected: the Join page, "Create username", with "A username costs about <amount>". Write the amount into
       the worksheet. Choose **Create username**.
3. [ ] Expected: "Step 1 of 2 · Choose a username". Type the name into **Username**. Expected: the checklist met
       and "Username available", with no note that the network must vote. Choose **Continue**.
4. [ ] Expected: "Step 2 of 2 · Confirm", "Confirm your username", an "Upgrade fee", and no line "You need at least
       …". Choose **Confirm & pay**. If the **Authorize** sheet asks for the passphrase, enter it and choose
       **Authorize**.
5. [ ] Expected: "Creating your username", which moves through "(1/3) Processing payment", "(2/3) Creating
       identity" and "(3/3) Registering username", with what it waits for (for example "Waiting for the network to
       confirm the payment.").
6. [ ] On A only, the resume. As soon as **Activity** lists the registration's payment, while the screen still
       shows "(1/3) Processing payment" or "(2/3) Creating identity", quit the app. Open it again and unlock.
       Expected: Home's DashPay card says "Registration paused" with **Continue**, and nothing runs until you choose
       it. Choose **Continue** (and **Authorize** if asked). Expected: the same registration goes on. If it had
       finished before you could quit, mark the resume **unexercised**.
7. [ ] Expected at the end: "Welcome to DashPay, @<name>" with **Go to wallet**. The identity chip in the toolbar
       shows @<name>, and the sidebar shows **Contacts** instead of Join DashPay. **Activity** lists one
       registration payment (on A also after the resume), and the balance fell by about step 4's fee. Write down the
       time from **Confirm & pay** to the end.

**Fail if:** a name with a digit from 2 to 9 is shown as one the network votes on; the registration ends on
"Registration failed" or "Registration stopped"; a paused registration runs before **Continue**, or **Continue**
pays a second time (two registration payments in Activity); the end screen or the chip does not appear; Contacts does
not replace Join DashPay.
**Finding:** a registration that takes more than 15 minutes; a resume marked unexercised.

### 7.2 Send a contact request from A to B

1. [ ] On A: **Contacts** ▸ **Add contact**. Expected: the sheet "Add a New Contact" with the tabs **Search**,
       **Scan** and **My QR**, and the field **Search username**.
2. [ ] Type B's username. Expected: B among the results ("Checking…", then "Verified on the Dash network"). If it
       says "Enable DashPay to send contact requests.", close the sheet, choose **Enable** on the Contacts page
       ("Enable DashPay"), wait for "DashPay is enabled." and start again; note it.
3. [ ] Choose **Send Contact Request** (and **Authorize** if asked). Expected: "Contact request sent to <B>.".
       Close the sheet. Expected: Contacts lists B under "Pending Requests"; selecting it shows the "Relation"
       "Request sent" and "Contact request sent. You become contacts once they accept it."

**Fail if:** B is not found by its exact username; **Send Contact Request** fails with DashPay enabled; B is not
under Pending Requests after the confirmation.

### 7.3 Accept it on B from the bell

1. [ ] On B, with the app open, unlocked and synced: within a few minutes the bell in the toolbar shows a badge
       (opening **Contacts** checks for new requests at once).
2. [ ] Click the bell. Expected: the popover "Notifications", with A's request under "Contact requests" and the
       buttons **Accept** and **Ignore**.
3. [ ] Choose **Accept** (and **Authorize** if asked; the popover stays open behind it). Expected: "You and <A>
       are now contacts." in the popover. Press Escape. Expected: the popover closes.
4. [ ] Expected on B: Contacts ▸ "My Contacts" lists A, with the "Relation" "Contact". On A, within a few minutes
       or when Contacts is opened again: "<B> accepted your contact request" in the bell, and B under "My Contacts".

**Fail if:** the request does not reach B's bell or "Contact Requests" within 15 minutes and a restart of B;
**Accept** fails; either side does not list the other under "My Contacts" afterwards.

### 7.4 Pay the contact (A→B, then B→A)

1. [ ] On A: **Pay** ▸ **To a contact**. Choose B under "My Contacts" (or under "Frequent contacts"). Expected: the
       form "Pay <B>", whose card shows B's display name with @<username> under it, or the username alone when B has
       no display name; **Change** goes back to the list.
2. [ ] Enter 0.01 tDASH in the amount field (its label names the unit: `0.01` in tDASH, `10` in mtDASH) and the
       **Note** `h08 A to B`. Choose **Review**.
3. [ ] Expected: the sheet "Confirm payment", "Do you want to send this payment?", with "Pay to" B, the "Amount",
       the "Network fee" and the "Total". Choose **Send** (after the **Passphrase** if the sheet asks for it).
4. [ ] Expected: "Payment sent" with its "Transaction ID". Write the txid into the worksheet. Choose **Show in
       Activity**. Expected: "Transaction details" names B.
5. [ ] On B, within a few minutes: the bell shows "<A> sent you a payment" (or "Received <amount>" under
       "Payments"); **Activity** lists the payment, naming A; Contacts ▸ A ▸ "Activity" lists it under
       **Received**.
6. [ ] Repeat steps 1 to 5 from B to A, with the note `h08 B to A`.

**Fail if:** a contact both sides list as "Contact" cannot be paid; the amount received is not the amount sent;
either side's Activity does not name the other; a payment ends on "Payment status unknown" or "Payment not sent"
while the wallet is synced and funded.

### 7.5 Edit the profile and see it on the other side

1. [ ] On A: the identity chip in the toolbar (its name is "My profile, @<name>") opens **My Profile**. Choose
       **Edit Profile**.
2. [ ] In the sheet "Edit Profile": **Display Name** `H08 Mac <date>`, **About me** `h08 run <date>`. Expected: each
       field's counter ("<n>/<max> characters"). Choose **Save Changes**. Expected: "Saving…", then "Your profile
       was saved."; My Profile shows the display name and the about text.
3. [ ] Quit and open A again. Expected: the same.
4. [ ] On B: A's row in Contacts, and A's card in **Pay** ▸ **To a contact**, show `H08 Mac <date>` with @<A's
       username> under it (Contacts may say "Updating contacts…" first). This build shows another user's About me
       nowhere, so the about text is checked on A only.
5. [ ] Repeat steps 1 to 4 from B (`H08 Win <date>`), seen on A.

**Fail if:** **Save Changes** fails while My Profile's "Credit balance" is not low or empty; the profile is back to
the old one after the restart; the other side still shows the old name after 15 minutes and a restart.

### 7.6 The self-test on macOS and Windows (CI)

`--selftest` exists only in a build with dw-app's `selftest` feature
([`apps/desktop/README.md`](../../apps/desktop/README.md)). Releases leave it out, and the workflow that builds it,
`tauri-selftest.yml`, uploads the report and not the binary, so there is no self-test build to run by hand. Run it
on GitHub's macOS and Windows runners against the candidate's tag:

- [ ] `gh workflow run tauri-selftest.yml --repo PastaPastaPasta/dashwallet-desktop --ref vX.Y.Z-rc.N -f os=both`
- [ ] Find the run with `gh run list --repo PastaPastaPasta/dashwallet-desktop --workflow tauri-selftest.yml --limit 1`
      and open it. Expected: both jobs, "Tauri on macos" and "Tauri on windows", succeed. The job's `--selftest`
      step fails when the report fails `ci/github/selftest-judge.jq` (all four screens recorded in light and dark,
      every check passed) and prints the problems.
- [ ] Download the artifacts `tauri-macos` and `tauri-windows`: `selftest-<os>.json`, `selftest.log` and a
      screenshot of a second launch. Open each screenshot. Write the run's URL into the results.

**Fail if:** a job fails in its `--selftest` step (the app or the judge). A job that fails before it (toolchain,
dependencies, the runner) is not a product failure: rerun it once, and if it fails again the cell is
**incomplete** with the run's URL.

## 8. Screen readers: VoiceOver (macOS) and Narrator (Windows)

CI walks the keyboard-only DashPay journeys in a browser and is automating Orca on Linux; this section checks what
VoiceOver and Narrator say in the real app. VoiceOver runs on the Mac with wallet A, Narrator on the Windows PC with
wallet B (section 7). VoiceOver: **real** Mac preferred; a macOS VM is enough for most checks, but its audio and the
VoiceOver keys often misbehave, so redo on a real Mac anything that fails only in the VM. Narrator: a VM with
working audio is enough. Write the OS, reader and app versions into the worksheet.

### Setting up

- VoiceOver: System Settings ▸ Keyboard ▸ **Keyboard navigation** on, so Tab reaches every control. ⌘F5 starts and
  stops VoiceOver. VO is Control-Option. VO-Right and VO-Left arrow move to the next and previous item; VO-Space
  activates; VO-Shift-Down and VO-Shift-Up go into and out of a group; VO-U opens the rotor (Left and Right arrow
  switch between Headings, Landmarks and Form Controls, Up and Down choose, Return goes there); VO-Command-H moves
  to the next heading; VO-F3 describes the item. Control stops speech.
- Narrator: Control-Windows-Enter starts and stops it. The Narrator key ("Narrator" below) is Caps Lock or Insert.
  Narrator-Right and Narrator-Left arrow move to the next and previous item; Narrator-Space turns scan mode on and
  off; in scan mode Up and Down arrow read item by item, H and Shift-H move by heading and D by landmark; Space or
  Enter activates. Control stops speech; Narrator-F1 lists every command.
- Both: Tab and Shift-Tab move the focus; Escape closes a sheet or popover.

### What every screen must do

The steps below name only what is particular to each screen; this applies to all of them.

- Every control is announced with a name and a role (button, text field, checkbox, tab, link). A control announced
  by its role alone ("button", "group") is a failure, as is an icon button without a name: the bell, **Show
  balance** / **Hide balance**, **Lock wallet**.
- Each page has a heading with its name that the rotor (VoiceOver) or H (Narrator) finds.
- Tab reaches every control in the order they are shown, and the focus can always be seen.
- A sheet (a dialog) is announced with its title when it opens, takes the focus, keeps Tab inside it (Tab never
  reaches the page behind it), closes with Escape (unless it is sending) or its **Close** button, and then gives
  the focus back to the control that opened it.
- What changes without moving the focus is announced: an error at once; a toast, a payment's result or the sync
  line when the reader is idle.

**Fail if:** any point above fails on a control or state a user needs to finish that screen's task. **Finding:** a
doubled or unclear name, an odd reading order, an announcement that repeats itself.

### 8.1 The lock screen

- [ ] Lock the wallet (**Lock wallet** in the toolbar). Expected: "Unlock wallet" as a heading, "Enter your
      passphrase to unlock your wallet.", the field **Passphrase** (a secure field: the reader does not speak the
      typed characters), the checkbox **For mixing only**, **Unlock** and **Forgot passphrase?**.
- [ ] Type a wrong passphrase and press Return (macOS) or Enter (Windows). Expected: "The passphrase entered for
      the wallet decryption was incorrect." is announced without a move, and the focus is in the empty field.
- [ ] Type the right one. Expected: Home, and the reader announces where it is (the page or its heading), not
      silence.

**Fail if:** the reader speaks a typed character of the passphrase; the wrong-passphrase error is not announced; the
unlock leaves the focus nowhere (the next Tab starts at the top of the window with nothing announced).

### 8.2 Welcome and onboarding

Headphones on, no one else in the room, no recording; on macOS the VoiceOver caption panel off (VoiceOver Utility ▸
Visuals). Use an account whose data root is empty, for example the one of sections 2 to 5 right after section 5
emptied it; do not fund this wallet, and empty the data root again when you finish (section 5).

- [ ] The notice "Dash Wallet is an early release": announced as a dialog with its title; **I understand** closes
      it and the focus moves to Welcome.
- [ ] Welcome: "Welcome to Dash Wallet" as a heading; the "Network" group with **Mainnet** and **Testnet**, the
      chosen one announced as pressed or selected; **Create a new wallet** and **Restore from recovery phrase**.
      Choose **Testnet**, then **Create a new wallet**.
- [ ] "Choose a passphrase": **New passphrase** and **Repeat the passphrase** (secure fields), "Strength: <Weak, Fair,
      Good or Strong>" announced as you type, **Continue without a passphrase** (do not choose it here) and
      **Continue**. Type two different passphrases and choose **Continue**. Expected: "The supplied passphrases do
      not match." announced. Then type matching ones and choose **Continue**.
- [ ] "Your recovery phrase": the warning "Anyone with your recovery phrase can take your Dash. Never share it, and
      make sure no one can see your screen." is read. The words are not read until you move onto them; then they
      read as a list, in order. The checkbox **I have written down my recovery phrase**, and **Continue**, which
      stays unavailable until it is ticked.
- [ ] "Verify your recovery phrase": "Choose the words in the order of their numbers.", each slot announced as
      "Word <n>", each chip as a button with its word. Choose a wrong chip. Expected: "Wrong word. Try again."
      announced. Finish with the right ones.
- [ ] "Your wallet is ready": the title and **Open wallet**; choosing it lands on Home with the page announced.

**Fail if:** the phrase is read out without you moving onto it (as the screen opens, or in a dialog's or status's
announcement); a passphrase character is spoken; a mismatch or wrong-word error is not announced; a step cannot be
finished by keyboard and reader alone.

### 8.3 Home, Activity and a transaction detail

- [ ] Home: the "Balance" with the amount (or "Balance unavailable"), the sync line (for example "Still syncing,
      balance may change"), the shortcuts **Receive**, **Send**, **Scan QR** and **Back up**, the "History" heading
      and **See all transactions**. Choose **Hide balance** in the toolbar. Expected: the button's state is
      announced, and the balance is no longer read as an amount. Choose **Show balance** again.
- [ ] **Activity**: the heading, the filters, then each row read with its direction, amount, date and status.
      Choose a row (Return or Enter, or VO-Space). Expected: the sheet "Transaction details" announced, its facts
      read in order; Escape closes it and the focus is back on that row.

**Fail if:** a row cannot be opened by keyboard; the sheet does not take or give back the focus; with the balance
hidden the reader still speaks the amount.

### 8.4 Pay: recipient, amount, review, confirm

First turn on Settings ▸ **Security** ▸ "Ask for my passphrase for every payment", so the confirm sheet asks for the
passphrase. Turn it off again at the end if you prefer.

- [ ] **Pay**: the tabs **To a contact** and **To an address** announced as tabs (Left and Right arrow move
      between them). Choose **To an address**.
- [ ] **Pay to** (the field says "Dash address"), **Address Book…**, **Label**, **Message** and **Amount (<unit>)**
      each read with its label; a hint is read after its field. Type an address of another network, or a broken
      one. Expected: the error is announced and tied to the field. Correct it: the faucet's return address.
- [ ] Type an amount larger than the balance. Expected: "The amount exceeds your balance." (or the fee variant)
      announced, and **Review** announced as unavailable with the reason. Correct it to 0.001 tDASH.
- [ ] Choose **Review**. Expected: "Confirm payment" announced as a dialog; "Do you want to send this payment?",
      then "Pay to", "Amount", "Network fee" and "Total", each with its value; the **Passphrase** field; **Send**.
      Press Escape. Expected: the sheet closes and the focus is back on **Review**. Choose **Review** again, enter
      the passphrase and choose **Send**.
- [ ] Expected: "Payment sent" announced (the result sheet's title), with "Transaction ID", **Copy transaction
      ID**, **Show in Activity** and **Close**.
- [ ] **To a contact** (A or B from section 7): "Search contacts or a username", the groups "My Contacts" and
      "Frequent contacts", and each contact read with its name. Choose one and leave the amount empty. Expected:
      the form "Pay <name>", and **Review** announced as unavailable with "Enter an amount first.".

**Fail if:** a Pay field has no label; an error is shown but not announced; "Confirm payment" lets the focus behind
it; the result is not announced.

### 8.5 Receive

- [ ] **Receive**: "Receive in" with **Dash**, **Platform** and **Shielded**; "Your Dash address" read with the
      address; **Copy address**, **Copy link** and **New address**. The QR code is read as "QR code of <address>",
      not as an unnamed image.
- [ ] Choose **Copy address**. Expected: "Copied" is announced.
- [ ] "Request payment": **Label**, **Amount (<unit>)**, **Message**, "Include my username in this request",
      **Clear** and **Create request**. Create one with a label. Expected: the sheet "Request payment to <address>"
      announced, its QR code named, **Copy payment link** reachable; Escape closes it and the focus is back on
      **Create request**. "Requested payments" lists it, with **Show**, and **Remove** named "Remove the request
      <label>".

**Fail if:** the address or the QR code has no name; the request sheet does not take or give back the focus.

### 8.6 Contacts and the bell

- [ ] **Contacts**: the heading, **Add contact**, "Search contacts or a username", "Sort by", and the groups ("Contact
      Requests (<n>)", "My Contacts", "Pending Requests", "Hidden (<n>)") as headings. Each row is read with its name
      and its state ("Unverified", "Setting up" when shown). Choosing a row announces "<name>, <relation>" and the
      detail ("Contact") follows with "Identity", "Relation", **Pay** and "Only visible to you".
- [ ] **Add contact**: "Add a New Contact" announced as a dialog; the tabs **Search**, **Scan** and **My QR**;
      **Search username** labelled; the results read; **Close** or Escape closes it and the focus is back on
      **Add contact**.
- [ ] The bell: announced as "Notifications" (or "Notifications, <n> unread") and as a button that opens a popup.
      Open it with Space or Return. Expected: the focus moves to the popover's "Notifications" heading; the requests
      are read with **Accept <name>** and **Ignore <name>**; **See all notifications** at the end. Press Escape.
      Expected: the popover closes and the focus is back on the bell. The popover is not modal: Tab past its last
      control closes it, which is expected.
- [ ] With a waiting request (send one from the other wallet), choose **Accept <name>** in the popover. Expected:
      the result ("You and <name> are now contacts.") announced without the focus being lost.

**Fail if:** the bell has no name or does not say it is unread; the popover's buttons do not name the person;
Escape does not close the popover or the focus is lost after it.

### 8.7 Settings

- [ ] **Settings**: the tabs **General**, **Security**, **Wallets**, **CoinJoin (experimental)**, **Backups**,
      **Advanced** and **About**, announced as tabs, Left and Right arrow moving between them.
- [ ] **General**: "Appearance" (System, Light, Dark), "Unit to show amounts in", "Language" and "Load contact
      pictures", each control read with its label and its state; a hint read after its control.
- [ ] **Security**: "Lock automatically after" with its value, and "Ask for my passphrase for every payment" with
      its state. Change one. Expected: the new state is announced. Open **Change passphrase…** and close it with
      Escape: the focus is back on it.
- [ ] **About**: the version read.

**Fail if:** a setting is read without its label or its state; a tab cannot be reached or chosen by keyboard.

## 9. Flows no automated test covers (testnet, by hand)

With wallets A and B from section 7. VM is enough. A flow this build does not have is marked so: record it
**skipped (not in this build)**, never as a pass.

### 9.1 Invitations

- [ ] Creating an invitation: not in this build ("Cuts applied 2026-11-19" in [`PLAN-1.0.md`](../PLAN-1.0.md)
      moves it to 1.0.x; claiming one stays). Skip and record **skipped (not in this build)**.
- [ ] Opening an invitation needs a testnet invitation link made outside this app, for example by a mobile Dash
      wallet that creates invitations on testnet. If you have none, record **skipped (no invitation link)**.
      Otherwise, in a third account with a new, unfunded wallet C (section 7's "Before you start", without the
      faucet): **Join DashPay** ▸ **Have an invitation?**. Expected: the sheet "Open an invitation" with the field
      **Invitation link**. Paste the link and choose **Open invitation**.
- [ ] Expected: "DashPay invitation" (or "You have been invited to DashPay"), "Checking the invitation…", then
      "Invited by" with the inviter's username, and **Accept invitation** and **Not now**. Choose **Accept
      invitation**, choose a username as in 7.1 step 3, and confirm. Expected: "Paid by the invitation: nothing is
      paid from this wallet.", the checkbox "Send a contact request to <inviter>", and **Claim invitation**. Choose
      it.
- [ ] Expected: "Welcome to DashPay, @<name>" and "A contact request was sent to your inviter."; C's balance is
      still zero. Open the same link again in C. Expected: "This wallet already has a DashPay identity. …" (or "This
      invitation has already been used").

**Fail if:** a valid link is refused ("This invitation link is not valid"); the claim pays anything from C; the
claim ends without an identity and a name; a used invitation is accepted a second time.

### 9.2 Ignore a contact request (the app has no separate Decline)

- [ ] From B, send a new contact request to a third identity that is not yet B's contact (C from 9.1, or another
      test wallet with a username from 7.1; with none, record **skipped (no third identity)**). On the receiver,
      open the bell or Contacts ▸ "Contact Requests" and choose **Ignore**. Expected: "Request ignored. You can find it under Hidden."; the
      request leaves "Contact Requests" and appears under "Hidden (1)"; its detail says "You ignored this contact
      request." with **Unignore**.
- [ ] Quit and open the receiver again. Expected: still under "Hidden"; no new bell entry for it.
- [ ] Choose **Unignore**. Expected: "The request is back in Contact Requests."; **Accept** works from there.
- [ ] On the sender, all along: the receiver stays under "Pending Requests" ("Request sent"). Ignoring is private to
      the receiver, as in the mobile apps.

**Fail if:** **Ignore** fails or the request comes back by itself after a restart; **Unignore** does not bring it
back; the sender is told that it was ignored.

### 9.3 Alias and note on a contact

- [ ] On A: Contacts ▸ B ▸ "Only visible to you": **Alias** `B on Windows`, **Note** `h08 note <date>`. Choose
      **Save**. Expected: "Saved", or "Saved. It is published once you have two contacts." when A has one contact,
      or "Saved on this device.".
- [ ] Expected: B's row now shows `B on Windows`, with @<B's username> under it; Pay ▸ **To a contact** shows the
      alias too; sorting by "Name" uses it.
- [ ] Quit and open A again. Expected: the alias and the note are kept.
- [ ] On B: neither the alias nor the note shows anywhere (B's My Profile, and A's row in B's Contacts, are
      unchanged).
- [ ] Clear both fields and **Save**. Expected: B's row shows B's own name again.

**Fail if:** **Save** fails; the alias or note is lost after the restart; either shows anywhere on B.

### 9.4 Avatars

- [ ] Settings ▸ **General** ▸ "Load contact pictures" is on (the default) on both A and B.
- [ ] On A: My Profile ▸ **Edit Profile**, the "Change picture" area. Expected: **Choose File…** unavailable, and
      "Pictures from this computer need an upload service this version does not have yet. Use a public URL or
      Gravatar.": choosing a file is not in this build; record that part **skipped (not in this build)**.
- [ ] Choose **Public URL**, paste the https:// address of a small public PNG or JPEG you are free to use (no
      photo of a person), and choose **Fetch Image**. Expected: "Move and zoom your photo to find the perfect fit";
      choose **Use Photo**, then **Save Changes**. Expected: "Your profile was saved." and the picture on My
      Profile and the identity chip.
- [ ] On B, within 15 minutes and a restart at most: A's row in Contacts and A's card in Pay show the picture.
- [ ] On B, turn "Load contact pictures" off. Expected: A shows as initials. Turn it back on.
- [ ] Optional: **Gravatar** with a test email address that has a Gravatar; the same expectations.
- [ ] On A: **Edit Profile** ▸ **Remove picture** ▸ **Save Changes**. Expected: initials on A, then on B.

**Fail if:** a valid public image URL is refused; the saved picture never reaches B with pictures on; B still loads
it with "Load contact pictures" off; removing it does not reach B.

### 9.5 Platform credits: top up and withdraw

- [ ] On A: **My Profile**. Expected: "Credit balance" with an amount and "≈ <n> contact requests · <n> profile
      updates". Write it down.
- [ ] Choose **Top up**. Expected: the sheet "Top up credits", "Credits pay for contact requests and profile changes
      on Dash Platform." Choose **Other amount**, enter 0.05 tDASH (or the smallest top-up the sheet names, if that
      is more), and choose **Continue**. Expected: "Amount", "Top-up fee", "Total" and "Adds about <n> credits: …".
      Choose **Confirm & pay** (and **Authorize** if asked).
- [ ] Expected: "Adding credits…", then "Credits added" with "+<n> credits." and the new balance. Choose **Done**.
      My Profile's balance went up by about that; Activity lists the top-up payment.
- [ ] Choose **Withdraw**. Expected: the sheet "Withdraw credits", **Amount (tDASH)** or **Amount (mtDASH)**,
      **Max**, and **To** with the wallet's receive address. Enter 0.01 tDASH (if the sheet says "Withdraw between
      <min> and <max>.", an amount in that range) and choose **Continue**. Expected:
      "Withdraw", "Network fee", "Taken from credits", "You receive about" and "To". Choose **Confirm & withdraw**.
- [ ] Expected: "Withdrawing…", then "Withdrawal queued". Choose **Done**. My Profile ▸ "Withdrawals" lists
      "Withdrawal of <amount>" as "Pending", then "Sent"; the payment reaches the address in **Activity** (minutes,
      or hours when Platform's daily withdrawal limit is reached; note how long).

**Fail if:** the top-up takes the payment without adding credits and My Profile offers no **Finish**; the credits
added are far from "Adds about"; the withdrawal ends "Failed: the credits were not returned"; the Dash never
arrives within 24 hours while the row says "Sent".

## 10. Results

One row per machine and run: pass, fail (with the issue number), skipped (with the reason), outstanding (a pair
not yet run), pending (the release is not published yet) or incomplete (with the reason, for example 7.6's runner
failure). In the first-launch cell, an override path the OS
never asked for is **unexercised** (with the OS version and security setting). The worksheet holds the values behind each cell.

| Machine, OS | Run | Verify | First launch (path, seconds) | Fresh install | Upgrade (8, 9) | Send and reopen (10, 11) | Downgrade (12, 13) | Restore (14) |
|---|---|---|---|---|---|---|---|---|
| Mac Apple silicon, macOS … | fresh | | | | — | — | — | — |
| Mac Apple silicon, macOS … | 0.1.0 E | | | — | | | | |
| … | 0.1.0 S | | | — | | | | |
| … | 0.2.0 E | | | — | | | | |
| … | 0.2.0 S | | | — | | | | |
| … | 0.3.0 E | | | — | pending | | | |
| … | 0.3.0 S | | | — | pending | | | |
| Mac Intel, macOS … | … | | | | | | | |
| PC x64, Windows 11 | … | | | | | | | |
| PC x64, Windows 10 22H2 | … | | | | | | | |
| PC Arm, Windows 11 (optional) | … | | | | | | | |

Uninstall, one row per machine (section 4, after its last run):

| Machine, OS | Last run | Remove, data kept | Reinstall finds the wallet | Remove with data | Secret-store entry removed by the uninstaller | Clean reinstall |
|---|---|---|---|---|---|---|
| … | | | | | | |

DashPay, one row per wallet (section 7). A cell for a step the wallet does not do (the resume on B) is —; a resume
that finished before the app could be quit is **unexercised**. The pay cell is the payment this wallet sent, and the
profile cell is this wallet's edit as the other wallet saw it:

| Wallet, machine, OS | Register (7.1) | Resume (7.1, 6) | Request sent or accepted (7.2, 7.3) | Pay the other (7.4) | Profile seen on the other side (7.5) |
|---|---|---|---|---|---|
| A, Mac …, macOS … | | | | | |
| B, PC …, Windows … | | — | | | |

Self-test (7.6), one row per OS, with the run's URL:

| OS | Run URL | Result |
|---|---|---|
| macOS (runner) | | |
| Windows (runner) | | |

Screen readers, one row per reader (section 8), with the OS and reader versions and real or VM:

| Machine, OS, reader | Real or VM | Lock (8.1) | Onboarding (8.2) | Home, Activity, detail (8.3) | Pay (8.4) | Receive (8.5) | Contacts and bell (8.6) | Settings (8.7) |
|---|---|---|---|---|---|---|---|---|
| Mac …, macOS …, VoiceOver | | | | | | | | |
| PC …, Windows 11 …, Narrator | | | | | | | | |

Untested flows, one row per run of section 9 (the wallets it used):

| Wallets, machines | Create invitation (9.1) | Open invitation (9.1) | Ignore and unignore (9.2) | Alias and note (9.3) | Avatar from a file (9.4) | Avatar by URL (9.4) | Top up (9.5) | Withdraw (9.5) |
|---|---|---|---|---|---|---|---|---|
| A (Mac), B (Windows), C … | skipped (not in this build) | | | | skipped (not in this build) | | | |

A step this build does not have is **skipped (not in this build)**; any other skipped cell names its reason (for
example "no invitation link"). The report is **incomplete** while any row is outstanding or pending, a cell is empty
or incomplete, a skipped cell has no reason, or a failure has no issue number.

File each failure as an issue with the OS, the machine, the release pair, the run (E or S) and redacted
screenshots, and link it here. For sections 7 to 9 the issue names the section and step instead of the release pair
and run, and for section 8 the reader and its version. A blocker or critical failure holds the release candidate.
Post the screenshots on the release pull request or tracking issue, committed to a `pr-artifacts/…` branch and
linked by SHA-pinned raw URLs, never to `main`.
