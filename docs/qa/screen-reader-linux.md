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
- every sidebar screen, Pay's tabs, Add contact (the sheet, and from a `dashpay:` link), Notifications, Join
  DashPay or My Profile;
- every Settings tab and the sheets it opens without funds, every Tools tab, the Masternodes tabs and Create
  proposal, Open URI.

At each state, `apps/desktop/scripts/atspi-unnamed.py` walks the app's AT-SPI tree (nodes that are SHOWING and
VISIBLE) until two walks in a row agree. A node is interactive when its role is one a user operates (button,
toggle button, check box, radio button, link, entry, password text, combo box, menu items, slider, spin button,
page tab, switch), when it is editable, or when it is focusable and is not a plain container or text.

**It fails** when any interactive node has an empty accessible name. It prints a table of screen, role, the path
of ancestor roles and names, and nearby text (the element's tag and class, its own text or description, its
parent's name and its siblings), and writes every screen's result to `atspi-unnamed.json` in the output
directory. A state it cannot reach also fails the run; states that do not exist without funds (a transaction's
detail) are listed as skipped. There is no allowlist: a decorative element exposed as interactive is a defect
to fix in the renderer. Screens that show a recovery phrase are walked with every name and text dropped from
the output, and the run fails if any output file holds a phrase or a passphrase.

## Run it

In Docker (Ubuntu 24.04 with CI's Tauri apt set, under Xvfb; nothing opens on the host display):

```sh
apps/desktop/scripts/atspi-walk-docker.sh /path/to/out    # builds, then runs the walk
apps/desktop/scripts/atspi-walk-docker.sh --clean         # removes this checkout's target and pnpm volumes
```

On a Linux host with the packages of `ci/github/linux-host-deps.sh tauri`, tauri-driver and a private display:

```sh
cd apps/desktop
xvfb-run -a -s "-screen 0 1280x800x24" pnpm e2e:tauri:atspi          # DW_E2E_SKIP_BUILD=1 reuses a build
```

## What stays by hand

Narrator on Windows and VoiceOver on macOS have no equivalent harness here; they stay in the by-hand QA pass
([`H-08-macos-windows.md`](H-08-macos-windows.md) and H-05, accessibility, in [`PLAN-1.0.md`](../PLAN-1.0.md)). This
check proves only that every control has a name; whether the name reads well, the reading order and the live
announcements are still checked by listening to Orca.
