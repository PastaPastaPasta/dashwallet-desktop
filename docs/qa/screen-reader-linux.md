# Screen-reader check on Linux (AT-SPI)

The axe checks (`apps/desktop/e2e/a11y.spec.ts` and the route and scenario suites) read the renderer's DOM in
Chromium and WebKit on the fixture. This check reads what a screen reader gets instead: the accessibility tree
that WebKitGTK hands to Orca over AT-SPI, from the real app (debug build, production renderer) on the real
engine.

## What it does

`apps/desktop/e2e-tauri/atspi-walk.ts` drives the app through tauri-driver, as the other `e2e-tauri` scripts do,
and stops at every state a user reaches without funds or a network:

- onboarding on a fresh data root: Welcome, Choose passphrase and its mismatch, the recovery phrase, its check,
  Done; then Settings ▸ Wallets ▸ Add Wallet: Welcome with Cancel, Restore (empty and valid), the restore options;
- a regtest wallet made by `dwcli`: the lock screen and a wrong passphrase, Forgot passphrase (wallet choice,
  what happens, the phrase), unlock for mixing only and the unlock sheet;
- every sidebar screen, Pay's tabs, a payment request (its sheet and the list that holds it), Join DashPay and its
  sheets, Contacts, Add contact from a `dashpay:` link, Notifications;
- every Settings tab and the sheets it opens without funds (CoinJoin's Restore defaults among them), every Tools
  tab, the Masternodes tabs and Create proposal, Open URI.

At each state, `apps/desktop/scripts/atspi-unnamed.py` walks the app's AT-SPI tree: every node that is VISIBLE and
either SHOWING or inside the web document, so a control scrolled out of view counts too. A state counts only once
the tree has caught up with it: the walk first waits in the DOM for a control only that state has (a sheet by its
own test id, a tab by a control of its panel), reads from it a node to expect (the sheet's dialog name, the step's
heading, the control's name), and the walker waits until that node is in the AT-SPI tree, there is at least one
interactive node, every read succeeded (each is retried once) and two walks in a row agree. A state that does not
get there in 30 seconds fails the run, as does a parent with more children than the walker reads (2,000) or a node
it could not read.

A node is interactive when its role is one a user operates (button, toggle button, check box, radio button, link,
entry, password text, combo box, menu items, slider, spin button, page tab, switch), when it offers an action a user
operates (click, press, activate, jump), when it is editable, or when it is focusable and is not a plain container,
text or live region (a form's error that takes focus to be read is not a control, unless it also offers an action).

**It fails** when any interactive node has an empty accessible name. It prints a table of screen, role, the path
of ancestor roles and names, and nearby text (the element's tag and class, its own text or description, its
parent's name and its siblings), and writes every screen's result to `atspi-unnamed.json` in the output
directory. A state it cannot reach also fails the run, except the three this wallet cannot show, listed with
their reason in `EXPECTED_SKIPS`: a transaction's detail (no funds), Contacts' header **Add contact** and My
Profile (both need a DashPay identity, so Platform; [H-08](H-08-macos-windows.md) §8.6 and §8.8 check them by
hand). There is no allowlist: a decorative element exposed as interactive is a defect to fix in the renderer.
Screens that show a recovery phrase are walked with every name and text dropped from the output.

Each run writes to a directory it creates, `atspi-<time>` under the output directory: the driver's log,
`atspi-unnamed.json`, and on failure the test ids shown and a screenshot. The run fails if any file there holds a
phrase or a passphrase, and deletes that file; it reads and deletes nothing outside its own directory.

## Run it

In Docker (Ubuntu 24.04 with CI's Tauri apt set, under Xvfb; nothing opens on the host display):

```sh
apps/desktop/scripts/atspi-walk-docker.sh /path/to/out    # builds, then runs the walk; results in out/docker-<time>
apps/desktop/scripts/atspi-walk-docker.sh --clean         # removes this checkout's target and pnpm volumes
```

On a Linux host with the packages of `ci/github/linux-host-deps.sh tauri`, tauri-driver and a private display:

```sh
cd apps/desktop
xvfb-run -a -s "-screen 0 1280x800x24" pnpm e2e:tauri:atspi          # DW_E2E_SKIP_BUILD=1 reuses a build
```

The walker's rules (what is interactive, the action check, unread and truncated subtrees, the expected nodes) have
tests on synthetic trees, which need only python3-gi and gir1.2-atspi-2.0:

```sh
cd apps/desktop
python3 -I scripts/test_atspi_unnamed.py
```

## What stays by hand

Narrator on Windows and VoiceOver on macOS have no equivalent harness here; they stay in the by-hand QA pass
([`H-08-macos-windows.md`](H-08-macos-windows.md) and H-05, accessibility, in [`PLAN-1.0.md`](../PLAN-1.0.md)). This
check proves only that every control has a name; whether the name reads well, the reading order and the live
announcements are still checked by listening to Orca.
