# Issue #317: the DashPay welcome after a sheet over the registration closes

Fixture data only (`dashpay-no-identity`, username alice-77, passphrase "demo" of the fixture backend). Chromium, 1280×820.

- before: merge base b942e212dec74fd3977797622dbb8df2a42adc97
- after: fix head 3f05b565dc9eb5118f9cae5327b98b035aa80610

Each state has a plain shot and a `-focus` twin. The twin marks `document.activeElement` and prints the text of the welcome's status region (`registration-announced`).

1. `1-about-over-finished-{light,dark}`: Home focused on the progress screen; About opened from the app menu; the registration finishes behind it.
2. `2-after-escape-{light,dark}`: Escape closes About; its opener, Home, went with the progress screen.
3. `3-status-after-escape-{light,dark}`: the same flow with Hide balance as the opener; Escape returns focus to it.

`capture-317.spec.ts.txt` is the Playwright spec that took them, run with `scripts/e2e-docker.sh --project=chromium e2e/capture-317.spec.ts` at each revision.
