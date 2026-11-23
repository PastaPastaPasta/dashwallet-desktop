# Issue #287: keyboard focus leaving the toolbar bell's popover

Fixture data only (`dashpay-registered`). Chromium, 1280×820.

- before: merge base b942e212dec74fd3977797622dbb8df2a42adc97
- after: fix head 3eff3083499f71cc688cfa7078de9f033ab2307f

Each state has a plain shot and a `-focus` twin. The twin marks `document.activeElement` and prints the bell's `aria-expanded` and whether the popover is open.

1. `1-see-all-focused-{light,dark}`: the bell opened with Enter, Tab to its last control, See all notifications.
2. `2-after-tab-{light,dark}`: Tab once more; focus moves to Hide balance in the toolbar.
3. `3-shift-tab-to-bell-{light,dark}`: Shift+Tab from the popover's heading to the bell; the popover stays open (unchanged).
4. `4-ignore-ended-after-tab-out-{light,dark}`: Ignore heidi with the call held, Tab out past See all while it runs, then let it finish.

`capture-287.spec.ts.txt` is the Playwright spec that took them, run with `scripts/e2e-docker.sh --project=chromium e2e/capture-287.spec.ts` at each revision.
