# Dash Wallet for desktop: product brief

This page says who the app is for, what it must do well, and the order in which we improve it. Scope rules
live in `CLAUDE.md` and `docs/design/DASHPAY.md`; the version plan lives in
`docs/research/gap-2026-10-31/RELEASE-PLAN.md`. This brief decides which **user-facing** improvement ships
next. It is updated as items ship.

## Who it is for

1. **Dash holders who live on their computer.** They use DashPay on their phone, or want to, and want the
   same wallet with the same recovery phrase on a big screen: pay people by username, see what came in, keep
   their savings private.
2. **The Dash community, at a demo.** A merchant, a meetup or a conference booth: someone sends you Dash, it
   shows up at once, you pay `@someone` back, and the app looks like it belongs next to the best wallets.
3. **Masternode owners** (later, release 1.1): vote and keep an eye on their nodes without running dash-qt.

Power users keep dash-qt. We add a Core feature only when an ordinary desktop user would miss it.

## What it must do great

- **Money in, money out, instantly and visibly.** A payment someone sends shows up within seconds, with its
  amount, who sent it and a clear status, even while the wallet is catching up.
- **People, not addresses.** Usernames, faces and names everywhere a transaction or a payment appears.
- **Plain words.** No protocol jargon (asset lock, 0/unconfirmed, ProTx) and no developer leftovers (task IDs,
  command-line flags) on any screen an ordinary user reaches.
- **Calm privacy.** Mixing and the shielded balance work in the background and explain themselves in one
  sentence.
- **Feels like a native, finished app** in light and dark: it opens fast, nothing overflows or misrenders, and
  empty screens point to the next step.

## Walkthrough findings (testnet, build `70c80f7`, 2026-11-14)

The app runs the real engine on testnet. A new wallet, a 25 tDASH faucet payment, a username
(`@dwdemo2611`), a send, mixing, a contact search, settings, the lock screen and the masternode screens (on
fixture data) were all walked in light and dark. Every core flow worked. What hurts:

- The window took 31 s to appear, with nothing on screen, before it showed anything.
- A new wallet shows "Balance unavailable" until sync ends. The faucet payment (InstantSend-locked within
  seconds) appeared only about 6 minutes later, when sync finished.
- Activity calls the username purchase "Asset lock". A send to yourself shows its fee as `+0.00000226`.
  Details say "0/unconfirmed, verified via InstantSend".
- After the username is registered, the success screen says "Your account is ready" with a single Done
  button: no photo, no "find friends", no way to share your username.
- Settings ▸ General shows a "Coming later" panel that names internal task IDs (MP-14, DP2-09, W-02), and
  "This window was started with --network testnet". The network picker highlights Mainnet while the window
  runs on Testnet, on onboarding and in Settings.
- Pay opens on "To a contact" even when you have no contacts, which is a dead end. Home shows four "Join DashPay"
  buttons at once.
- Dark mode draws unchecked checkboxes as solid white squares that read as checked. The Contacts header wraps
  "Add contact" onto two lines, shows an "already open" note and scrolls sideways when the add pane is open.
- Every send and every username action asks for the passphrase again although the status bar says
  "Unlocked".
- Empty states reuse one filter-funnel icon for "no transactions", "no contacts" and "join DashPay".

## Ranked backlog

Ranked by impact on "people want to use it, and it demos well". **UI** means renderer-only work; **engine**
means it needs the Rust engine owner.

| # | Improvement | User story | Needs |
|---|---|---|---|
| 1 | **Instant incoming payments** | As someone being paid, I see the payment and my new balance within seconds, even while the wallet is still syncing, so I can hand over the coffee. | engine + UI |
| 2 | **Activity in plain words** | As a user scanning my history, every row says what happened ("Username @me", "Sent to @bob", "Mixing ×12" grouped) with the right sign and a plain status, so I never meet "Asset lock" or "0/unconfirmed". | UI (small engine field for asset-lock purpose) |
| 3 | **No developer leftovers** | As a first-time user, I never see task IDs, command-line flags, a network picker that disagrees with the badge, or a "passphrases do not match" error on empty fields. | UI |
| 4 | **The DashPay welcome moment** | As someone who just got a username, I'm congratulated and offered the next steps (add a photo and name, find friends, share my username QR), so the app feels alive at once. | UI |
| 5 | **Opens in two seconds** | As anyone launching the app, I see the window and my last-known balance at once, while the engine starts behind it. | engine + UI |
| 6 | **Pay that never dead-ends** | As a payer with no contacts, Pay opens on "To an address", and typing a username finds the person and offers "Add and pay". | UI |
| 7 | **Unlock once, then flow** | As a user who just unlocked, I'm not asked for my passphrase again for every send for a few minutes (an option, plus OS biometrics later). | design + engine |
| 8 | **A cleaner Home** | As a user, Home shows one balance with readable decimals, hides empty Shielded and Platform rows, and asks me to join DashPay once, not four times. | UI |
| 9 | **Dark mode and layout fit** | As a dark-mode user, checkboxes, the Contacts header and every page render correctly at 1024 px and wider, with no sideways scrolling. | UI |
| 10 | **Local currency by default on mainnet** | As a mainnet user, I see my balance and each payment in my currency, after a one-line privacy choice at onboarding. | UI |
| 11 | **Empty states that lead somewhere** | As a new user, each empty screen has its own illustration and one clear next action. | UI |
| 12 | **Share my username** | As a DashPay user, Profile has a big "share my username" QR and link, so a friend at the table can add me. | UI |
| 13 | **Transaction details for people** | As a user, a transaction shows who, when and how much in plain words, with "View in explorer", and inputs and outputs folded away. | UI |
| 14 | **Switch network without a restart** | As a tester or demoer, switching between Mainnet and Testnet takes effect in place. | engine + UI |
| 15 | **Send with a fiat toggle** | As a payer, I can type the amount in my currency and see the Dash amount, and power options (unsigned PSBT) sit under Advanced. | UI |

## How we ship

One or two items a day, each its own pull request with light and dark before-and-after screenshots on the PR
and a regression test for every fix. Engine-dependent items (1, 5, 7, 14) are raised with the engine owner
while the UI half ships behind them.
