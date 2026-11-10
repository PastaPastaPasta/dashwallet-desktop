# UI artifacts for PastaPastaPasta/dashwallet-desktop#6

## Current set: `r2/`, PR head 3bb3da859d6706d059f5f86ad2a6f6f810770997

`r2/` supersedes `after/` (head 44a6e0a54732d15ec08527e73ccf315631d29ed1) for every state. `after/` is kept only so the earlier comment's links still resolve.

**What changed since 44a6e0a5:**
- The Settings Network picker no longer moves when a refusal or note appears under it. It now stays at the card's right edge, in line with the Join DashPay button. At 44a6e0a5 it was centred over the longest line under it, so it shifted with each message. This is a CSS-only change (`.settings-card > .network-picker { align-self: stretch; }`).
- New state `switch-link-not-opened`: a link that arrives while a switch runs is held. If the window ends up on the other network, a toast says it was not opened.

**How the set was shot:**
- The same runner, image (`mcr.microsoft.com/playwright:v1.64.0-noble`), fixture build, viewport (1280×820) and themes as `after/`.
- REV=r2, from a worktree at 3bb3da85.
- `capture/zz-row74-capture.spec.ts` and `capture/run-docker.sh` are the r2 versions; the only runner change is that SHOTS_DIR is configurable.

**Compared with `after/`** (`capture/diff-r2-vs-44a6e0a5.txt`, RGB difference bounding boxes):
- 11 states differ in both themes: the picker at rest, Mainnet chosen, the confirmation (pending and checking), switching, the five refusals, and the pinned note. Every difference box lies in the picker's row (x ≥ 684, y 349–451).
- 6 states are pixel-identical in both themes: add-wallet, switch-none-open, switch-rolled-back, switch-save-failed, welcome-switching and welcome-testnet-chosen.
- 1 state is new: switch-link-not-opened.

**Compared with `before/`:** `settings-picker` now differs from the base only in the picker's row, because the picker sits at the right edge.

## Earlier set: `before/` and `after/`, PR head 44a6e0a5

This set replaces the earlier one at e36875509c628ec6d5ecd1658b965a7c61923b7b. That set was shot with host Playwright, showed Welcome with Mainnet chosen instead of the Testnet defect, and was missing the shielded-spend refusal.

**Revisions:**
- Before: the exact base 54d9894652abc7356687fb6776f468f9d37f9746 (the merge base of `dw/0.1.x-row74-network-switch-live` and `main`).
- After: the full PR head 44a6e0a54732d15ec08527e73ccf315631d29ed1.

**How they were shot:**
- Each revision was built on its own, in its own worktree, inside the Docker image `mcr.microsoft.com/playwright:v1.64.0-noble`. This is the image and pnpm setup that `apps/desktop/scripts/e2e-docker.sh` uses.
- Each build ran `pnpm build:fixture` and was served with `vite preview --mode fixture`. The fixture build only: no engine, no real wallet, no network.
- The browser was Playwright Chromium at 1280×820, scale 1, locale en-US, light and dark themes.
- `capture/run-docker.sh` is the runner and `capture/zz-row74-capture.spec.ts` is the spec. Both are scratch files, not committed to the product branch.

**Disclosed fixture addition at the base:**
- The base has no fixture scenario for "no choice saved, window on Mainnet".
- To show the base's Welcome with Testnet chosen, the head's `no-wallet-mainnet` scenario was copied into the base's `src/fixture/scenarios.ts`, uncommitted: `capture/base-fixture-scenario.patch`.
- It changes fixture data only. No app code changed. The base's restart text and disabled Create button come from the base's own UI code.

**Scenarios:**
- `no-wallet-mainnet`: Welcome on a new install; the window is on Mainnet and no choice is saved.
- `no-wallet`: Welcome; the window is on Testnet.
- `funded`: the encrypted wallet "Main" on Testnet.
- `recovery-one`: a wallet with an unfinished transfer.
- `network-pinned`: a window started with `--network regtest`.

**How states were held:**
- Held states use the fixture's `hold(command)`: `switch_network` for the switching card, `list_wallets` for "Checking for unfinished transfers…".
- Refusals and failures use the fixture's `world.networkSwitch` knobs (`busy`, `closeRefused`, `fail`, `saveFails`).

**Files:**
- `before/` holds only the states the base has: Welcome with Testnet chosen, the Settings picker, Settings with Mainnet chosen, and Add a wallet (8 images).
- `after/` holds every state at 44a6e0a5 (34 images), superseded by `r2/` (36 images at 3bb3da85).
- Names are `<theme>-<state>.png`. `SHA256SUMS` lists every image.
- `settings-picker` is pixel-identical in `before/` and `after/` (44a6e0a5). In `r2/` it differs from the base, because the picker now sits at the right edge.
