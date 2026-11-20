UI artifacts for fix/lock-screen-holds-nothing (issue #133). Fixture data only (`scenario=locked`, passphrase `demo`), Chromium 1280x820.

- `before/`: base revision 277e1571
- `after/`: PR head a87c1b94 (branch fix/lock-screen-holds-nothing)

Round 3 (`r3/`): fixture data only (`scenario=locked` and `scenario=governance`), Chromium 1280x820.

- `r3/before/`: 5e322ec5, the merge-base of the PR head with main
- `r3/after/`: PR head a3841ca8

Round 4 (`r4/`): fixture data only (`scenario=locked` and `scenario=governance`), Chromium 1280x820, PR head 981fe085. Only the states whose pixels changed since round 3: Home after the unlock and the proposal fee notices.
