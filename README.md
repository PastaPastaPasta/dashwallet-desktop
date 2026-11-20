UI artifacts for fix/lock-screen-holds-nothing (issue #133). Fixture data only (`scenario=locked`, passphrase `demo`), Chromium 1280x820.

- `before/`: base revision 277e1571
- `after/`: PR head a87c1b94 (branch fix/lock-screen-holds-nothing)

Round 3 (`r3/`): fixture data only (`scenario=locked` and `scenario=governance`), Chromium 1280x820.

- `r3/before/`: 5e322ec5, the merge-base of the PR head with main
- `r3/after/`: PR head a3841ca8

Round 4 (`r4/`): fixture data only (`scenario=locked` and `scenario=governance`), Chromium 1280x820, PR head 981fe085. Only the states whose pixels changed since round 3: Home after the unlock and the proposal fee notices.

Round 5 (`r5/`): every image linked in the description was shot again with the same scripts, Chromium 1280x820: the "before" set at main 15cbc1d4, the "after" set at PR head 73830769, and compared with `compare -metric AE`. Only the images that differed are here, all shot at 73830769.

- `r5/after/`: the round 3 script's "after" states
- `r5/r4/`: the round 4 script's states

Round 6 (`r6/`): the new state for two payments of one proposal draft, a proposal in Mine either could have made, shot at PR head 346d06b6, Chromium 1280x820. Every image linked before was compared again at main 6e61b32b and head 346d06b6 (two runs each); none changed beyond the "Sending…" spinner's angle and anti-aliased pixels at the selected sidebar item's corner, so none was shot again.
