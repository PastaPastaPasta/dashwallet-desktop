# UI evidence for PR #9

Before: `a51d04c9ab2e4e3fa08289e80832679249eae2ca`. After: `7a36f7038ac426ee896491cfc8ace848c5788b40`.

Each revision was built independently from its own clean checkout and node_modules. All comparison images are Chromium fixture-renderer captures with disposable browser contexts, en-US, UTC, device scale 1. Each state was captured in both light and dark. Contacts is 1000×620; narrow Contacts is 800×620; compact Home is 900×900; other comparisons are 1280×820. Every published PNG was opened at original resolution before upload. SHA-256 and sizes are in [manifest.json](manifest.json).

No live engine, seed, private key, funded wallet, host address or network endpoint appears in these captures. Wallet addresses and contact identities are deterministic demo data. The fixture clock is fixed. The request label is Rent; the contact query is mal and the selected result Mallory.

Synthetic conditions are identical at both revisions: mixing marks the first ordinary fixture coin reserved/unspendable and emits the CoinJoin event; Max pending holds platform_fund_quote; the running transfer holds shield_from_core; fund-reserved makes only the fixture IPC platform_fund_quote reject with credits.coins_locked. No renderer DOM/text is fabricated.

Before Receive has stacked panels: Platform/Shielded comparisons scroll to their full QR and address. After shows that pool at the top through the selector. Receive-syncing compares the old unavailable action with the selected syncing explanation. Request-confirm intentionally preserves a username after explicit opt-in; default omission and persistence are covered by tests. The preexisting real-world red error on an empty amount was not reproduced in the old fixture; mixing-empty demonstrates the spendable/mixing presentation, and browser assertions cover validation timing. Max-pending shows the base's collapsed field and the head's preserved field.

The visual-baselines directory contains 16 refreshed existing snapshots, generated and re-compared successfully in the pinned Playwright container. They are supplemental snapshots of the head, not before/after pairs. They are kept off the feature branch as instructed. The existing Settings general snapshot includes a public service hostname and is not published; the paired Settings privacy/footer captures cover the changed controls without it. CI's existing non-blocking visual job still compares its checked-in base snapshots; these refreshed files are provided here for review/use, with no runtime download mechanism added to the product.

## Comparison matrix

| State | Fixture | Claim |
|---|---|---|
| fund-reserved | funded; synthetic IPC refusal | Reserved-funds message gives a concrete recovery action |
| transfer-running | funded; held Shielded command | Another transfer keeps other routes accessible |
| home-wide | funded | Remove Platform move prompt, preserve wide action layout |
| home-dust | platform-partly-dust | Remove dust-only cannot-move line |
| home-compact | funded | Actions below the vertical balance breakdown |
| withdrawal-form | funded | Planned amount excludes the fee reserve; revised copy |
| withdrawal-confirm | funded | Planned withdrawal in confirmation rows |
| mixing-empty | coinjoin-mixing; synthetic held coin | Spendable source, separate mixing amount and Pause action |
| mixing-insufficient | coinjoin-mixing; synthetic held coin | Insufficient-spendable validation after entering an amount |
| max-pending | funded; held quote | Amount steady and button spinner while pending |
| max-complete | funded | Max fills value and To card follows the route quote |
| receive-dash | dashpay-registered | Dash QR, selector and request form |
| receive-platform | dashpay-registered | Platform QR and selector; no request form |
| receive-shielded | dashpay-registered | Shielded QR and selector; no request form |
| receive-syncing | shielded-syncing | Selected syncing explanation retains pool selector |
| request-opt-in | dashpay-registered | Per-request username opt-in |
| request-confirm | dashpay-registered | Explicit opt-in retains a named request |
| contacts-open | dashpay-registered | Explicit Add opener disabled while open |
| contacts-search | dashpay-registered | Header and columns remain in place while searching |
| contacts-selected | dashpay-registered | Selecting Mallory preserves column widths and header |
| contacts-compact | dashpay-registered | Focus reveals the selected pane when columns stack |
| settings-privacy | dashpay-registered | Username default off and revised explanation |
| settings-footer | dashpay-registered | Unused Core options card removed |

## Images

### fund-reserved

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![fund-reserved, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/fund-reserved-light.png) | ![fund-reserved, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/fund-reserved-light.png) |
| dark | ![fund-reserved, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/fund-reserved-dark.png) | ![fund-reserved, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/fund-reserved-dark.png) |

### transfer-running

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![transfer-running, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/transfer-running-light.png) | ![transfer-running, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/transfer-running-light.png) |
| dark | ![transfer-running, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/transfer-running-dark.png) | ![transfer-running, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/transfer-running-dark.png) |

### home-wide

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![home-wide, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-wide-light.png) | ![home-wide, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-wide-light.png) |
| dark | ![home-wide, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-wide-dark.png) | ![home-wide, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-wide-dark.png) |

### home-dust

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![home-dust, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-dust-light.png) | ![home-dust, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-dust-light.png) |
| dark | ![home-dust, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-dust-dark.png) | ![home-dust, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-dust-dark.png) |

### home-compact

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![home-compact, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-compact-light.png) | ![home-compact, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-compact-light.png) |
| dark | ![home-compact, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/home-compact-dark.png) | ![home-compact, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/home-compact-dark.png) |

### withdrawal-form

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![withdrawal-form, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/withdrawal-form-light.png) | ![withdrawal-form, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/withdrawal-form-light.png) |
| dark | ![withdrawal-form, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/withdrawal-form-dark.png) | ![withdrawal-form, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/withdrawal-form-dark.png) |

### withdrawal-confirm

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![withdrawal-confirm, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/withdrawal-confirm-light.png) | ![withdrawal-confirm, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/withdrawal-confirm-light.png) |
| dark | ![withdrawal-confirm, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/withdrawal-confirm-dark.png) | ![withdrawal-confirm, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/withdrawal-confirm-dark.png) |

### mixing-empty

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![mixing-empty, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/mixing-empty-light.png) | ![mixing-empty, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/mixing-empty-light.png) |
| dark | ![mixing-empty, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/mixing-empty-dark.png) | ![mixing-empty, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/mixing-empty-dark.png) |

### mixing-insufficient

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![mixing-insufficient, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/mixing-insufficient-light.png) | ![mixing-insufficient, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/mixing-insufficient-light.png) |
| dark | ![mixing-insufficient, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/mixing-insufficient-dark.png) | ![mixing-insufficient, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/mixing-insufficient-dark.png) |

### max-pending

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![max-pending, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/max-pending-light.png) | ![max-pending, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/max-pending-light.png) |
| dark | ![max-pending, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/max-pending-dark.png) | ![max-pending, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/max-pending-dark.png) |

### max-complete

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![max-complete, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/max-complete-light.png) | ![max-complete, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/max-complete-light.png) |
| dark | ![max-complete, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/max-complete-dark.png) | ![max-complete, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/max-complete-dark.png) |

### receive-dash

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![receive-dash, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-dash-light.png) | ![receive-dash, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-dash-light.png) |
| dark | ![receive-dash, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-dash-dark.png) | ![receive-dash, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-dash-dark.png) |

### receive-platform

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![receive-platform, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-platform-light.png) | ![receive-platform, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-platform-light.png) |
| dark | ![receive-platform, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-platform-dark.png) | ![receive-platform, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-platform-dark.png) |

### receive-shielded

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![receive-shielded, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-shielded-light.png) | ![receive-shielded, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-shielded-light.png) |
| dark | ![receive-shielded, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-shielded-dark.png) | ![receive-shielded, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-shielded-dark.png) |

### receive-syncing

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![receive-syncing, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-syncing-light.png) | ![receive-syncing, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-syncing-light.png) |
| dark | ![receive-syncing, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/receive-syncing-dark.png) | ![receive-syncing, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/receive-syncing-dark.png) |

### request-opt-in

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![request-opt-in, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/request-opt-in-light.png) | ![request-opt-in, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/request-opt-in-light.png) |
| dark | ![request-opt-in, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/request-opt-in-dark.png) | ![request-opt-in, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/request-opt-in-dark.png) |

### request-confirm

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![request-confirm, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/request-confirm-light.png) | ![request-confirm, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/request-confirm-light.png) |
| dark | ![request-confirm, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/request-confirm-dark.png) | ![request-confirm, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/request-confirm-dark.png) |

### contacts-open

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![contacts-open, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-open-light.png) | ![contacts-open, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-open-light.png) |
| dark | ![contacts-open, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-open-dark.png) | ![contacts-open, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-open-dark.png) |

### contacts-search

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![contacts-search, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-search-light.png) | ![contacts-search, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-search-light.png) |
| dark | ![contacts-search, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-search-dark.png) | ![contacts-search, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-search-dark.png) |

### contacts-selected

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![contacts-selected, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-selected-light.png) | ![contacts-selected, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-selected-light.png) |
| dark | ![contacts-selected, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-selected-dark.png) | ![contacts-selected, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-selected-dark.png) |

### contacts-compact

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![contacts-compact, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-compact-light.png) | ![contacts-compact, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-compact-light.png) |
| dark | ![contacts-compact, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/contacts-compact-dark.png) | ![contacts-compact, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/contacts-compact-dark.png) |

### settings-privacy

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![settings-privacy, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/settings-privacy-light.png) | ![settings-privacy, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/settings-privacy-light.png) |
| dark | ![settings-privacy, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/settings-privacy-dark.png) | ![settings-privacy, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/settings-privacy-dark.png) |

### settings-footer

| Theme | Before — a51d04c9ab2e4e3fa08289e80832679249eae2ca | After — 7a36f7038ac426ee896491cfc8ace848c5788b40 |
|---|---|---|
| light | ![settings-footer, light, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/settings-footer-light.png) | ![settings-footer, light, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/settings-footer-light.png) |
| dark | ![settings-footer, dark, before at a51d04c9ab2e4e3fa08289e80832679249eae2ca](before/settings-footer-dark.png) | ![settings-footer, dark, after at 7a36f7038ac426ee896491cfc8ace848c5788b40](after/settings-footer-dark.png) |

## Refreshed visual baselines

dark-add-contact-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-add-contact-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-add-contact-chromium-linux.png)

dark-contacts-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-contacts-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-contacts-chromium-linux.png)

dark-home-contacts-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-home-contacts-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-home-contacts-chromium-linux.png)

dark-overview-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-overview-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-overview-chromium-linux.png)

dark-platform-receive-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-platform-receive-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-platform-receive-chromium-linux.png)

dark-receive-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-receive-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-receive-chromium-linux.png)

dark-receive-fiat-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-receive-fiat-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-receive-fiat-chromium-linux.png)

dark-receive-requests-fiat-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![dark-receive-requests-fiat-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/dark-receive-requests-fiat-chromium-linux.png)

light-add-contact-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-add-contact-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-add-contact-chromium-linux.png)

light-contacts-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-contacts-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-contacts-chromium-linux.png)

light-home-contacts-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-home-contacts-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-home-contacts-chromium-linux.png)

light-overview-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-overview-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-overview-chromium-linux.png)

light-platform-receive-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-platform-receive-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-platform-receive-chromium-linux.png)

light-receive-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-receive-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-receive-chromium-linux.png)

light-receive-fiat-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-receive-fiat-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-receive-fiat-chromium-linux.png)

light-receive-requests-fiat-chromium-linux, shot at `7a36f7038ac426ee896491cfc8ace848c5788b40`.

![light-receive-requests-fiat-chromium-linux, 7a36f7038ac426ee896491cfc8ace848c5788b40](visual-baselines/light-receive-requests-fiat-chromium-linux.png)
