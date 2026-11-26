# Shielded spends on testnet: UI evidence

Screenshots of the real Linux app (release build of `dash-wallet`, the binary the AppImage bundles) on **public testnet**, 1280x800 window, driven through WebDriver. Each directory is named by the exact commit it was built from; the binary's embedded build version (`0.3.0-dev.1.g<sha7>`) was checked against that commit before shooting.

Fixture: a new throwaway testnet wallet made for this evidence (receive address `yXZKr65w2QKDEqZGPZXB9YGFjABrGD8duP`, Shielded address `tdash1zr6gqn6ztu6jmrplzxpstesje8jju0fkwkmc7mrr5n94wflptgv0kus3g2p0y0m4uc7w9tsgmn6rf`), funded from the testnet faucet and shielded through the app's own Dash wallet → Shielded transfer. The same recovery phrase was restored into a fresh data directory for each build, so both builds start from the same balances. Sends go to the wallet's own Shielded address; unshields go to its own Dash wallet.

## Dry run (2026-11-26): before `3947258449` / after `4e976524b0`

| State | Before (base `39472584496822b04da8f56adb03c07a266529b4`) | After (head `4e976524b0bc23d059fd4b5dfe1b122a5dfbb4fe`) |
|---|---|---|
| Send from Shielded, confirm (light) | `before/3947…/light-send-confirm.png` | `after/4e97…/light-send-confirm.png` |
| Send from Shielded, after Confirm (light) | `light-send-result.png`: refused, "Dash Platform did not answer. Try again." | `light-send-running.png` (proving), `light-send-result.png` ("Sent") |
| Send from Shielded (dark) | `dark-send-confirm.png`, `dark-send-result.png` | `dark-send-confirm.png`, `dark-send-running.png`, `dark-send-result.png` |
| Shielded → Dash wallet, confirm (light) | `light-out-confirm.png` | `light-out-confirm.png` |
| Shielded → Dash wallet, after Confirm (light) | `light-out-result.png`: refused, same text | `light-out-running.png`, `light-out-result.png` ("Not confirmed yet": broadcast, waiting for Platform), `light-out-settled.png` (Home after it settled: Unshield row, Core balance up) |
| Shielded → Dash wallet (dark) | `dark-out-confirm.png`, `dark-out-result.png` | `dark-out-confirm.png`, `dark-out-running.png`, `dark-out-result.png`, `dark-out-settled.png` |

The confirm screens are the same at both revisions (the quote is not gated); the difference starts after Confirm and the passphrase. Background balances in the "after" shots move between captures because each executed spend changes them, and an unshield's change shows as pending until Platform confirms it.
