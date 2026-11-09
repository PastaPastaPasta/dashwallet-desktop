# UI artifacts for PastaPastaPasta/dashwallet-desktop#6

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
- `after/` holds every state (34 images).
- Names are `<theme>-<state>.png`. `SHA256SUMS` lists every image.
- `settings-picker` is pixel-identical in `before/` and `after/`, because the idle picker did not change.
