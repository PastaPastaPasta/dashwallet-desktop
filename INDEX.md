# dashwallet-desktop PR UI artifacts

Screenshots for PastaPastaPasta/dashwallet-desktop PRs #4 and #3 (DEC-1484 backfill). Evidence only: this branch is not product code and never merges.

## Capture setup (both PRs)

- Backend: the fixture backend only (`pnpm build:fixture`, `vite preview --mode fixture`), no engine, no network. All balances, labels and history rows are fixture data.
- Runner: `apps/desktop/scripts/e2e-docker.sh --project=chromium` (the pinned `mcr.microsoft.com/playwright` image, as CI runs it), Chromium.
- Viewport 1280×820, deviceScaleFactor 1, locale en-US; theme from the fixture URL (`&theme=light|dark`).
- Each revision was checked out on its own detached worktree and built on its own; the capture specs were uncommitted scaffolding, removed after.
- Each shot waits for its state's assertions, `document.fonts.ready`, and every `<img>` loaded and decoded.

## PR #4: one Transfer sheet

- Before (exact base): `128e86c6dd6ad95c42048cdcf553f63722986445`
- After (full PR head): `fbd232cc36464a14c79984211b71b67e4394570c` (merged as `fb8f25458cf33d20e5e0c4563e720ff948fda88d`)
- States come from `apps/desktop/e2e/transfer.ts` `TRANSFER_STATES` at the head, run through a copy of the earlier capture scaffold. The key states only, not every state in the table.
- Capture-only state added (not in the table): `coreToShielded-max-discreet`. `home` is a plain Home shot of the `funded` scenario at both revisions.
- No shot from earlier runs (c3fc1b5d) is reused.

| State | What it shows |
|---|---|
| home | Home. Before: the old per-card buttons (Shielded: Receive/Send/Add to Shielded/Move to Dash wallet; Platform: Move to Dash wallet/Add from Dash wallet). After: the Transfer shortcut in the quick actions and the pool cards' headings (Shielded ›, Platform addresses ›) |
| entry-shielded-card | Home with the Shielded card heading's Transfer button focused (heading named "Shielded, transfer") |
| loading | Transfer opened from the shortcut before Shielded and Platform are read: "Reading your balances…" |
| coreToShielded-idle | Dash wallet → Shielded, the default route on opening the shortcut |
| coreToShielded-max | Dash wallet → Shielded, Max filled from a successful quote and re-quote (fee line for that amount) |
| coreToShielded-max-discreet | CAPTURE-ONLY STATE: discreet mode on, then Max on Dash wallet → Shielded: field, fee line and balances masked |
| coreToShielded-max-refused | Dash wallet → Shielded, Max refused (an earlier add to Shielded is not finished); field left empty |
| coreToShielded-max-coins | Dash wallet → Shielded, Max's re-quote refused: too many separate amounts, consolidate first |
| shieldedToCore-idle | Shielded → Dash wallet, opened from the Shielded card |
| shieldedToCore-max | Shielded → Dash wallet, Max filled from a successful quote (1.24980000) |
| coreToPlatform-idle | Dash wallet → Platform ("Fee shown on the next step") |
| coreToPlatform-max | Dash wallet → Platform, Max filled from a successful quote and re-quote |
| coreToPlatform-max-refused | Dash wallet → Platform, Max refused (an earlier add to Platform is not finished); field left empty |
| coreToPlatform-max-coins | Dash wallet → Platform, Max's re-quote refused: too many separate coins, consolidate first |
| platformToCore-idle | Platform → Dash wallet with the whole-balance note ("Platform moves its whole balance."), Max fixed on |
| platformToCore-discreet | Platform → Dash wallet in discreet mode: amounts masked |
| shieldedPlatform-unsupported | Shielded → Platform picked: the unavailable reason, with the From picker reopened (not a dead end) |
| none-absent | No usable route: a build with neither Shielded nor Platform addresses |
| none-inactive | No usable route: Shielded not active on the network and no Platform addresses |

| File | State | Revision | Theme | Scenario | sha256 | Checked |
|---|---|---|---|---|---|---|
| `pr4/before/home-light.png` | home | 128e86c6 | light | `funded` | `0e9735595e3dbbc7` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/before/home-dark.png` | home | 128e86c6 | dark | `funded` | `b5e4066463bc204b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/home-light.png` | home | fbd232cc | light | `funded` | `56925bee68d64fc4` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/home-dark.png` | home | fbd232cc | dark | `funded` | `fdde58cc68b55ae7` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/entry-shielded-card-light.png` | entry-shielded-card | fbd232cc | light | `funded` | `ce3e0d7f7338cd8b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/entry-shielded-card-dark.png` | entry-shielded-card | fbd232cc | dark | `funded` | `4b43ceb4fea9d69e` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/loading-light.png` | loading | fbd232cc | light | `funded &hold=shielded_status&hold=platform_addresses` | `feaa8fcb9b0189dd` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/loading-dark.png` | loading | fbd232cc | dark | `funded &hold=shielded_status&hold=platform_addresses` | `7614826e1389ae50` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-idle-light.png` | coreToShielded-idle | fbd232cc | light | `funded` | `13ecbb6b82295198` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-idle-dark.png` | coreToShielded-idle | fbd232cc | dark | `funded` | `1cb293e8de21d0e1` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-light.png` | coreToShielded-max | fbd232cc | light | `funded` | `9aec076abbd8ce5c` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-dark.png` | coreToShielded-max | fbd232cc | dark | `funded` | `952a7b00251c921b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-discreet-light.png` | coreToShielded-max-discreet | fbd232cc | light | `funded` | `0f20d2fd63f57c2b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-discreet-dark.png` | coreToShielded-max-discreet | fbd232cc | dark | `funded` | `10db705154dc1ff3` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-refused-light.png` | coreToShielded-max-refused | fbd232cc | light | `transfer-shield-pending` | `df1ce02732c215ac` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-refused-dark.png` | coreToShielded-max-refused | fbd232cc | dark | `transfer-shield-pending` | `3a9db72169ee2175` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-coins-light.png` | coreToShielded-max-coins | fbd232cc | light | `transfer-many-coins` | `ec2fb89641992b4a` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToShielded-max-coins-dark.png` | coreToShielded-max-coins | fbd232cc | dark | `transfer-many-coins` | `5376341737ed1894` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedToCore-idle-light.png` | shieldedToCore-idle | fbd232cc | light | `funded` | `4a7c0ba1bf1e6500` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedToCore-idle-dark.png` | shieldedToCore-idle | fbd232cc | dark | `funded` | `ebdaa6ae0ead80f1` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedToCore-max-light.png` | shieldedToCore-max | fbd232cc | light | `funded` | `c7f06c68e6561526` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedToCore-max-dark.png` | shieldedToCore-max | fbd232cc | dark | `funded` | `a12f88e5c374b17a` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-idle-light.png` | coreToPlatform-idle | fbd232cc | light | `funded` | `d19c6c881cf115c2` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-idle-dark.png` | coreToPlatform-idle | fbd232cc | dark | `funded` | `caf0792c4ed71401` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-light.png` | coreToPlatform-max | fbd232cc | light | `funded` | `71c8bc29433c0166` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-dark.png` | coreToPlatform-max | fbd232cc | dark | `funded` | `82e1930abae0e90b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-refused-light.png` | coreToPlatform-max-refused | fbd232cc | light | `paddr-fund-proof-waiting` | `1a57c109154f06ad` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-refused-dark.png` | coreToPlatform-max-refused | fbd232cc | dark | `paddr-fund-proof-waiting` | `a2c4b31445174cb0` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-coins-light.png` | coreToPlatform-max-coins | fbd232cc | light | `transfer-many-coins` | `2473a15b1a939629` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/coreToPlatform-max-coins-dark.png` | coreToPlatform-max-coins | fbd232cc | dark | `transfer-many-coins` | `d4e0e7f11a84cd8a` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/platformToCore-idle-light.png` | platformToCore-idle | fbd232cc | light | `funded` | `76f8ec730ad81010` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/platformToCore-idle-dark.png` | platformToCore-idle | fbd232cc | dark | `funded` | `292f54282624ba26` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/platformToCore-discreet-light.png` | platformToCore-discreet | fbd232cc | light | `funded` | `d14bcde38386daaf` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/platformToCore-discreet-dark.png` | platformToCore-discreet | fbd232cc | dark | `funded` | `c30f130c8731693f` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedPlatform-unsupported-light.png` | shieldedPlatform-unsupported | fbd232cc | light | `funded` | `6e68d29edbcb3a0f` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/shieldedPlatform-unsupported-dark.png` | shieldedPlatform-unsupported | fbd232cc | dark | `funded` | `5bb7dcbc6b9f8ee5` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/none-absent-light.png` | none-absent | fbd232cc | light | `pools-absent` | `5c6b2bff3e8b00a8` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/none-absent-dark.png` | none-absent | fbd232cc | dark | `pools-absent` | `812ef024e500f719` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/none-inactive-light.png` | none-inactive | fbd232cc | light | `transfer-none` | `b3d5348396e86a6f` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr4/after/none-inactive-dark.png` | none-inactive | fbd232cc | dark | `transfer-none` | `7783232d99b4bcf0` | opened at 1280×820, state right, nothing clipped, fixture data only |

## PR #3: "Add from Dash wallet" when the approval ended

- Before (exact base): `54d9894652abc7356687fb6776f468f9d37f9746`
- After (full PR head): `714f6c456b8999475ef9da959a822393ece73a7f` (merged as `aaf4a9429d8534fc381895b8d2f1dc2a0a8fecdc`)
- Fixture: the scenarios `paddr-fund-approval-ended` and `paddr-fund-resume-ended` are added by the PR. For the before shots, the PR's fixture-only diff (`apps/desktop/src/fixture/pending.ts`, `apps/desktop/src/fixture/scenarios.ts`) was applied, uncommitted, on the base worktree, so both revisions get the same fixture answers. The UI code (`PlatformFundSheet.tsx`, `src/platform/fund.ts`) is the base's own.
- The engine part of the PR (the apply keeps its signer across the funding call) has no fixture-visible state; it is covered by `rust/crates/dw-engine/src/platform/platform_fund_tests.rs`.

| State | What it shows |
|---|---|
| approval-ended | "Add from Dash wallet" of 0.5 tDASH whose approval ended after the lock (platform.lease_revoked): "Locked, not applied" outcome. Before: raw code in the text, Close only. After: plain text naming the amount, Finish + Close |
| resume-ended | Finish on a waiting 0.50 tDASH lock refused platform.needs_grant before its apply. Before: "It stopped with platform.needs_grant.", Close only. After: plain text, Finish (same lock) + Close |

| File | State | Revision | Theme | Scenario | sha256 | Checked |
|---|---|---|---|---|---|---|
| `pr3/before/approval-ended-light.png` | approval-ended | 54d98946 | light | `paddr-fund-approval-ended` | `ab2d510191335e87` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/before/approval-ended-dark.png` | approval-ended | 54d98946 | dark | `paddr-fund-approval-ended` | `4a4616f38b47ff4c` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/before/resume-ended-light.png` | resume-ended | 54d98946 | light | `paddr-fund-resume-ended` | `a216e70a8b22898b` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/before/resume-ended-dark.png` | resume-ended | 54d98946 | dark | `paddr-fund-resume-ended` | `fd9feb10ef073892` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/after/approval-ended-light.png` | approval-ended | 714f6c45 | light | `paddr-fund-approval-ended` | `0e11314a4b8a98bb` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/after/approval-ended-dark.png` | approval-ended | 714f6c45 | dark | `paddr-fund-approval-ended` | `e6594b59295309ec` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/after/resume-ended-light.png` | resume-ended | 714f6c45 | light | `paddr-fund-resume-ended` | `1ae16a54c241a3b3` | opened at 1280×820, state right, nothing clipped, fixture data only |
| `pr3/after/resume-ended-dark.png` | resume-ended | 714f6c45 | dark | `paddr-fund-resume-ended` | `9300b506ffd8a576` | opened at 1280×820, state right, nothing clipped, fixture data only |
