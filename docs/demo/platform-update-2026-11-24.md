# Demo run sheet: Dash platform update, Tue 2026-11-24, 8 AM CT

Build: **Dash Wallet 0.3.0** (Linux AppImage; macOS .dmg as the fallback). Network: **testnet only**. Length:
about 5 minutes with the talk track.

The demo wallet is **@platformdemo24** ("Platform Demo"). Its contact is **@demofriend24** ("Demo Friend"). The
recovery phrases and the demo passphrase are held by whoever prepared the wallets. They are never in this file, on
screen, or in chat.

## Before the demo

The day before:

- [ ] The demo wallet opens on the demo machine. If the demo machine is not the one the wallets were made on, restore
      @platformdemo24 there (birth block 1,596,600) the day before, never during the demo. Then check that Home shows
      the username and Demo Friend.
- [ ] Settings ▸ General: **Show Masternodes** is on, and governance data has loaded (Masternodes ▸ Governance shows
      a budget).
- [ ] Settings ▸ Security: **Lock automatically after: Never**. "Ask for my passphrase for every payment" is on.
- [ ] Settings ▸ CoinJoin: **Target balance 30 DASH**, mixing level **Advanced**. With the default target (1000)
      mixing takes almost every coin, and Pay then shows only a few tDASH as available.
- [ ] Pay ▸ To an address shows **Available: 10 tDASH or more**. If not, fund the wallet from the testnet faucet.
- [ ] Copy Demo Friend's Dash address (Demo Friend's Receive screen, or the prep notes) for step 4.

1 hour before:

- [ ] Start Dash Wallet and unlock it. Open **CoinJoin ▸ Start Mixing** and leave it running. Quitting or locking
      stops mixing.
- [ ] Maximize the window. At 1280×800 the Review button on Pay ▸ To an address sits at the bottom edge.
- [ ] Close the bell's notifications. Go to **Home**.
- [ ] Have Demo Friend's address on the clipboard.

## The 7 steps

| # | Click | Say | Rehearsal (3 runs) |
|---|-------|-----|--------------------|
| 1 | **Home** | "This is the DashPay wallet on the desktop: the balance, my username up top, frequent contacts, and history with names instead of addresses." Point at the Demo Friend rows. | pass ×3, under 2 s |
| 2 | Click **@platformdemo24** (top left) ▸ **Share Username**. Close it with ×. | "Anyone with the iOS or Android wallet scans this to add me. It's a username, not an address." | pass ×3, QR in about 1 s |
| 3 | **Pay** ▸ **To a contact** ▸ type `demofriend24` in the search ▸ click **Demo Friend** ▸ amount `0.1` ▸ Note `Demo coffee` ▸ **Review** ▸ type the passphrase ▸ **Send**. Then **Show in Activity**, or Activity ▸ the top row. | "I pay a person by username. No address to copy. InstantSend locks it in a couple of seconds." In the details, point at **InstantSend** and **Sent to contact @demofriend24**. | pass ×3 plus 9 extra runs; "Payment sent" after 3–7 s; InstantSend shows in under 1.2 s |
| 4 | **Pay** ▸ **To an address** ▸ paste Demo Friend's address in **Pay to** ▸ amount `0.2` ▸ **Review** ▸ passphrase ▸ **Send** ▸ **Close**. | "Plain addresses still work, the same as any Dash wallet." | pass ×3; "Payment sent" after 3–4 s |
| 5 | **CoinJoin** (sidebar) | "CoinJoin mixing has run in the background since before we started. Here is its progress and level. It runs while the app is open and unlocked." Do **not** press Start or Stop. | pass ×3, shows **Mixing** |
| 6 | **Masternodes** (sidebar) ▸ **Governance** | "Show Masternodes in Settings adds this read-only view: the budget, the vote clock and the proposals. This wallet has no voting keys, so it says Browsing only." Click one proposal to show its detail. | pass ×3; loads in 2–4 s |
| 7 | The **lock** icon (top right) ▸ type the passphrase ▸ **Unlock**. | "Lock hides the balance and history. Unlocking takes the passphrase." End here. | pass ×3; unlocks in 3–5 s |

After step 7, mixing has stopped (locking stops it, and the CoinJoin screen says so). Before any second run, open
CoinJoin ▸ **Start Mixing** off camera.

## Avoid on stage

- **Restore**, **Forgot passphrase?**, **Show recovery phrase…**, and any import. They put secrets or long scans on
  screen.
- **Shielded**: Receive ▸ Shielded, "Send from Shielded", and the **Transfer** button on Home.
- **Abandon** or **Resend** on a transaction (Activity ▸ Details, Tools ▸ Repair).
- **Starting or stopping a mix live**, and changing the mixing level.
- **Language switch** (Settings ▸ General ▸ Language).
- **Create proposal**, **Vote**, and **Advanced: unsigned transaction (PSBT)**.
- **Quitting the app** before step 5. Quitting stops mixing.

Known and harmless: under the Pay ▸ To a contact form the hint reads "Enter an address and an amount first." until
an amount is typed (issue #306). Read past it. The governance clock text ("About N minutes left…") changes with every
block.

## If something goes wrong

- **A payment shows "Sending…" for more than 15 s:** keep talking. It always finished in rehearsal. If the sheet shows
  an error, close it and do **not** retry the same payment. Show the history instead.
- **"Passphrase incorrect":** retype it slowly. The wallet counts attempts.
- **CoinJoin shows "Mixing stopped":** say "it stops when the wallet locks", and move on. Don't start it live.
- **The app won't start or the network is down:** play the fallback recording (a clean run of all 7 steps, about 6
  min), attached to the draft release "Demo fallback recording: Dash platform update 2026-11-24".

## Mac fallback (unsigned .dmg)

1. Open `dash-wallet-0.3.0-macos-universal.dmg` and drag **Dash Wallet** to Applications.
2. The build is not signed, so Gatekeeper blocks a double-click. Either right-click **Dash Wallet** ▸ **Open** ▸
   **Open**, or, on macOS versions that no longer offer it, use System Settings ▸ Privacy & Security ▸ **Open
   Anyway**, or run:

   ```sh
   xattr -dr com.apple.quarantine "/Applications/Dash Wallet.app"
   ```

3. Do this, and the wallet setup above, the day before. The first launch on a Mac has not been rehearsed for this
   demo yet.
