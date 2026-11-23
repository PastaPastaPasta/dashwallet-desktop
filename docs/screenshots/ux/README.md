# UX restyle screenshots (macOS)

The macOS app restyled to `docs/design/UX-SPEC.md` (M3 UX pass, branch `m3/ux-mac`). "Before" links
point at the M1/M2 screenshots of the same screen; "after" at `mac/`. Every screen is rendered
offscreen by the MacUI tests over the demo services:

```sh
DWD_HEADLESS=1 DWD_WRITE_SCREENSHOTS=1 swift test --filter MacUITests
```

The tests draw into a borderless window placed off screen (repo `CLAUDE.md`). Two limits of that
setup show in the images: the window is never key, so switches that are on draw grey instead of Dash
blue, and the sidebar in the main-window shots is drawn by the test (the real one is a system
sidebar list tinted Dash blue). The PNGs are written in sRGB.

Reference mock: `docs/design/ux/home-light.png`, `home-dark.png`.

| Screen | UX-SPEC | Before | After |
|---|---|---|---|
| about | §4.18 | [light](../m2/about-light.png) · [dark](../m2/about-dark.png) | [light](mac/about-light.png) · [dark](mac/about-dark.png) |
| address-book | §4.10 | [light](../m1/address-book-light.png) · [dark](../m1/address-book-dark.png) | [light](mac/address-book-light.png) · [dark](mac/address-book-dark.png) |
| coin-control-list | §4.14 | [light](../m2/coin-control-list-light.png) · [dark](../m2/coin-control-list-dark.png) | [light](mac/coin-control-list-light.png) · [dark](mac/coin-control-list-dark.png) |
| coin-control-tree | §4.14 | [light](../m2/coin-control-tree-light.png) · [dark](../m2/coin-control-tree-dark.png) | [light](mac/coin-control-tree-light.png) · [dark](mac/coin-control-tree-dark.png) |
| command-line-options | §4.18 | [light](../m2/command-line-options-light.png) · [dark](../m2/command-line-options-dark.png) | [light](mac/command-line-options-light.png) · [dark](mac/command-line-options-dark.png) |
| data-directory | §4.2 | [light](../m2/data-directory-light.png) · [dark](../m2/data-directory-dark.png) | [light](mac/data-directory-light.png) · [dark](mac/data-directory-dark.png) |
| lock | §4.4 | [light](../m1/lock-light.png) · [dark](../m1/lock-dark.png) | [light](mac/lock-light.png) · [dark](mac/lock-dark.png) |
| menu-bar | §4.19 | [light](../m2/menu-bar-light.png) · [dark](../m2/menu-bar-dark.png) | [light](mac/menu-bar-light.png) · [dark](mac/menu-bar-dark.png) |
| menu-bar-compact | §4.19 | [light](../m2/menu-bar-light.png) · [dark](../m2/menu-bar-dark.png) | [light](mac/menu-bar-compact-light.png) · [dark](mac/menu-bar-compact-dark.png) |
| onboarding-phrase | §4.3 | [light](../m1/onboarding-phrase-light.png) · [dark](../m1/onboarding-phrase-dark.png) | [light](mac/onboarding-phrase-light.png) · [dark](mac/onboarding-phrase-dark.png) |
| onboarding-welcome | §4.3 | [light](../m1/onboarding-welcome-light.png) · [dark](../m1/onboarding-welcome-dark.png) | [light](mac/onboarding-welcome-light.png) · [dark](mac/onboarding-welcome-dark.png) |
| options-display | §4.12 | [light](../m2/options-display-light.png) · [dark](../m2/options-display-dark.png) | [light](mac/options-display-light.png) · [dark](mac/options-display-dark.png) |
| options-network | §4.12 | [light](../m2/options-network-light.png) · [dark](../m2/options-network-dark.png) | [light](mac/options-network-light.png) · [dark](mac/options-network-dark.png) |
| options-security | §4.13 | [light](../m2/options-security-light.png) · [dark](../m2/options-security-dark.png) | [light](mac/options-security-light.png) · [dark](mac/options-security-dark.png) |
| options-wallet | §4.12 | [light](../m2/options-wallet-light.png) · [dark](../m2/options-wallet-dark.png) | [light](mac/options-wallet-light.png) · [dark](mac/options-wallet-dark.png) |
| overview | §4.5 | [light](../m1/overview-light.png) · [dark](../m1/overview-dark.png) | [light](mac/overview-light.png) · [dark](mac/overview-dark.png) |
| overview-920x600 | §4.5, minimum window | new state | [light](mac/overview-920x600-light.png) · — |
| overview-discreet | §4.5 (QT-039) | new state | [light](mac/overview-discreet-light.png) · [dark](mac/overview-discreet-dark.png) |
| overview-out-of-sync | §4.5 (QT-037), stalled banner | new state | [light](mac/overview-out-of-sync-light.png) · [dark](mac/overview-out-of-sync-dark.png) |
| overview-shortcuts | §4.5 | [light](../m2/overview-shortcuts-light.png) · [dark](../m2/overview-shortcuts-dark.png) | [light](mac/overview-shortcuts-light.png) · [dark](mac/overview-shortcuts-dark.png) |
| peers | §4.6 | [light](../m1/peers-light.png) · [dark](../m1/peers-dark.png) | [light](mac/peers-light.png) · [dark](mac/peers-dark.png) |
| psbt-empty | §4.15 | [light](../m2/psbt-empty-light.png) · [dark](../m2/psbt-empty-dark.png) | [light](mac/psbt-empty-light.png) · [dark](mac/psbt-empty-dark.png) |
| psbt-load-error | §4.15 | [light](../m2/psbt-load-error-light.png) · [dark](../m2/psbt-load-error-dark.png) | [light](mac/psbt-load-error-light.png) · [dark](mac/psbt-load-error-dark.png) |
| receive | §4.8 | [light](../m1/receive-light.png) · [dark](../m1/receive-dark.png) | [light](mac/receive-light.png) · [dark](mac/receive-dark.png) |
| security | §4.13 | [light](../m2/security-light.png) · [dark](../m2/security-dark.png) | [light](mac/security-light.png) · [dark](mac/security-dark.png) |
| send | §4.7 | [light](../m1/send-light.png) · [dark](../m1/send-dark.png) | [light](mac/send-light.png) · [dark](mac/send-dark.png) |
| send-coin-control | §4.7, §4.14 | [light](../m2/send-coin-control-light.png) · [dark](../m2/send-coin-control-dark.png) | [light](mac/send-coin-control-light.png) · [dark](mac/send-coin-control-dark.png) |
| send-confirm | §4.7 | [light](../m1/send-confirm-light.png) · [dark](../m1/send-confirm-dark.png) | [light](mac/send-confirm-light.png) · [dark](mac/send-confirm-dark.png) |
| settings | §4.12 General | [light](../m1/settings-light.png) · [dark](../m1/settings-dark.png) | [light](mac/settings-light.png) · [dark](mac/settings-dark.png) |
| settings-unreadable | §4.2 | [light](../m2/settings-unreadable-light.png) · [dark](../m2/settings-unreadable-dark.png) | [light](mac/settings-unreadable-light.png) · [dark](mac/settings-unreadable-dark.png) |
| shutdown | §4.2 | [light](../m2/shutdown-light.png) · [dark](../m2/shutdown-dark.png) | [light](mac/shutdown-light.png) · [dark](mac/shutdown-dark.png) |
| sign-verify | §4.11 | [light](../m1/sign-verify-light.png) · [dark](../m1/sign-verify-dark.png) | [light](mac/sign-verify-light.png) · [dark](mac/sign-verify-dark.png) |
| sign-verify-verify | §4.11 | new state | [light](mac/sign-verify-verify-light.png) · [dark](mac/sign-verify-verify-dark.png) |
| splash | §4.2 | [light](../m2/splash-light.png) · [dark](../m2/splash-dark.png) | [light](mac/splash-light.png) · [dark](mac/splash-dark.png) |
| sync-overlay | §4.6 | [light](../m1/sync-overlay-light.png) · [dark](../m1/sync-overlay-dark.png) | [light](mac/sync-overlay-light.png) · [dark](mac/sync-overlay-dark.png) |
| tools-console | §4.16 | [light](../m2/tools-console-light.png) · [dark](../m2/tools-console-dark.png) | [light](mac/tools-console-light.png) · [dark](mac/tools-console-dark.png) |
| tools-information | §4.16 | [light](../m2/tools-information-light.png) · [dark](../m2/tools-information-dark.png) | [light](mac/tools-information-light.png) · [dark](mac/tools-information-dark.png) |
| tools-peers | §4.16 | [light](../m2/tools-peers-light.png) · [dark](../m2/tools-peers-dark.png) | [light](mac/tools-peers-light.png) · [dark](mac/tools-peers-dark.png) |
| tools-repair | §4.16 | [light](../m2/tools-repair-light.png) · [dark](../m2/tools-repair-dark.png) | [light](mac/tools-repair-light.png) · [dark](mac/tools-repair-dark.png) |
| tools-window | §4.16 | [light](../m2/tools-window-light.png) · [dark](../m2/tools-window-dark.png) | [light](mac/tools-window-light.png) · [dark](mac/tools-window-dark.png) |
| transaction-detail | §4.9 detail | [light](../m2/transaction-detail-light.png) · [dark](../m2/transaction-detail-dark.png) | [light](mac/transaction-detail-light.png) · [dark](mac/transaction-detail-dark.png) |
| transactions | §4.9 list (default) | [light](../m1/transactions-light.png) · [dark](../m1/transactions-dark.png) | [light](mac/transactions-light.png) · [dark](mac/transactions-dark.png) |
| transactions-history | §4.9 list | [light](../m2/transactions-history-light.png) · [dark](../m2/transactions-history-dark.png) | [light](mac/transactions-history-light.png) · [dark](mac/transactions-history-dark.png) |
| transactions-table | §4.9 table | [light](../m1/transactions-light.png) · [dark](../m1/transactions-dark.png) | [light](mac/transactions-table-light.png) · [dark](mac/transactions-table-dark.png) |
| wallets | §4.17 | [light](../m2/wallets-light.png) · [dark](../m2/wallets-dark.png) | [light](mac/wallets-light.png) · [dark](mac/wallets-dark.png) |

Component gallery (light and dark, compared by `DashUIMacSnapshotTests`):
`Tests/DashUIMacSnapshotTests/__Snapshots__/` — new: `amount-text`, `balance-hero`, `history`,
`menu-card`, `states`, `phrase-grid`, `button-styles`.

Not in this set: the M3 screens (CoinJoin page and card, Masternodes, Governance, governance clock);
their pages are not in MacUI yet. Linux/Windows (CrossUI) are restyled separately.
