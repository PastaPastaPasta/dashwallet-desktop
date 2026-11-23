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

Ranked by impact on "people want to use it, and it demos well". The dated plan for everything left before
1.0 is `docs/PLAN-1.0.md`.

**Open**

| # | Improvement | User story | State |
|---|---|---|---|
| 1 | **Instant incoming payments** | As someone being paid, I see the payment and my new balance within seconds, even while the wallet is still syncing, so I can hand over the coffee. | engine lane started 11-18 |
| 5 | **Opens in two seconds** | As anyone launching the app, I see the window and my last-known balance at once, while the engine starts behind it. | the blank-launch fix merged (#33, 11-19); the two-second window is measured on each OS in the QA pass; the last-known balance at once moved to 1.0.x on 11-20 (the project manager's decision) |
| 7 | **Unlock once, then flow** | As a user who just unlocked, I'm not asked for my passphrase again for every send for a few minutes (an option, plus OS biometrics later). | engine commands exist; the send flow still asks every time |
| 11 | **Empty states that lead somewhere** | As a new user, each empty screen has its own illustration and one clear next action. | #25, in review |
| 15 | **Send with a fiat toggle** | As a payer, I can type the amount in my currency and see the Dash amount, and power options (unsigned PSBT) sit under Advanced. | fiat toggle shipped; PSBT still under Tools |

**Shipped**

| # | Improvement | Pull request |
|---|---|---|
| 2 | Activity in plain words | #18 |
| 3 | No developer leftovers | #17 |
| 4 | The DashPay welcome moment | #19 |
| 6 | Pay that never dead-ends | #20 |
| 8 | A cleaner Home | #21 |
| 9 | Dark mode and layout fit | #22 |
| 10 | Local currency by default on mainnet | onboarding rates choice (`4f2f03d2`) |
| 12 | Share my username | #23 |
| 13 | Transaction details for people | #24 |
| 14 | Switch network without a restart | #6 |

## How we ship

Each item is its own pull request, with a regression test for every fix and light and dark before-and-after
screenshots when it changes what a user sees. A pull request merges as soon as its review and required checks
are green; unrelated pull requests never wait for each other. Engine and UI halves of an item run as parallel
lanes.
