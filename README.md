# UI artifacts for PastaPastaPasta/dashwallet-desktop#6

**Revisions:**
- Before: the exact base 54d9894652abc7356687fb6776f468f9d37f9746 (the merge base of `dw/0.1.x-row74-network-switch-live` and `main`).
- After: the full PR head 44a6e0a54732d15ec08527e73ccf315631d29ed1.

**How they were shot:**
- Each revision was built on its own (`pnpm build:fixture`, in its own worktree) and served with `vite preview --mode fixture`.
- The browser was Playwright Chromium at 1280×820, scale 1, locale en-US, light and dark themes.
- The fixture backend uses its own data only: no engine, no real wallet, no network.

**Scenarios:**
- `no-wallet` (Welcome, window on Testnet)
- `funded` (encrypted wallet "Main" on Testnet)
- `recovery-one` (a wallet with an unfinished transfer)
- `network-pinned` (a window started with `--network regtest`)

**How states were held:**
- Held states use the fixture's `hold(command)`.
- Refusals use its `networkSwitch` knobs.
- The capture spec was a scratch file, not committed.

**Files:**
- `before/` holds only the states the base has: Welcome with Mainnet chosen, Settings with Mainnet chosen, and Add a wallet.
- `after/` holds every state.
- Names are `<theme>-<state>.png`.
