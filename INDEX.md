# PR #7 artifacts: POLISH 23, Experimental badge on the selected CoinJoin item

- **Before** = main `fb8f25458cf33d20e5e0c4563e720ff948fda88d` (the PR base).
- **After** = PR head `9ea59111183fae3a3ae46bb175e48d6d71d65e33`.
- Each revision was captured in its own disposable worktree, with Playwright Chromium in the pinned Docker image (`scripts/e2e-docker.sh`) on the fixture build. The viewport is 1280×820.
- **Fixture:** scenario `funded` (fixture data only), light and dark themes.
- **States:**
  - `home-unselected`: Home is selected, so CoinJoin's badge is unselected. This is the control.
  - `coinjoin-selected`: the CoinJoin page, reached by clicking the sidebar item. The page-title badge is in the same frame.
- `full/` holds the 1280×820 originals. `crop/` holds the region (0,40)-(640,300), enlarged 2× nearest-neighbour.
- **Pixel diff, before vs after:**
  - `coinjoin-selected` (both themes): 1056 px, box (57,250)-(141,263), which is the sidebar badge only.
  - `home-unselected` (both themes): 0 px.
- **Sidebar badge contrast:** solid glyph colour vs. pill colour, RGB.

| State | Before light | After light | Before dark | After dark |
|---|---|---|---|---|
| Selected | 1.02:1 | 18.60:1 | 2.12:1 | 18.60:1 |
| Unselected | 4.10:1 | 4.10:1 | 6.60:1 | 6.60:1 |

## sha256
```
9a0adf3e1a6618a4e0c2c7f68aa4bdd224ee96c847e02da4c266133518d5a73e  polish23/after/crop/dark-coinjoin-selected.png
34995be5ee5e3c3630320fbe57e63e82d10bf3879f1558371cb3a3c00b6f7a83  polish23/after/crop/dark-home-unselected.png
b0a2c6066bc2ecfe132d1016be5939393083846f1570a390a972431531849b68  polish23/after/crop/light-coinjoin-selected.png
f16194e1e95fb520f230c794c05d7315f01a5dd82f5b6a4c0f263cb1e43b6c04  polish23/after/crop/light-home-unselected.png
359ad954b48a6e3bd4f86760339722c17e30f8dc330ef9db33ac9378c5092a22  polish23/after/full/dark-coinjoin-selected.png
fdde58cc68b55ae7665cf5b571d0139c381d9e6f6127d8dd7d3f41b525eb9607  polish23/after/full/dark-home-unselected.png
4109256f9f03ab4ed17b151111d02f97310dd97933b479e583f34b92d107f7cf  polish23/after/full/light-coinjoin-selected.png
56925bee68d64fc482db1e806d98815c4def0ab5ce5ccd814dcd6e9baa77e957  polish23/after/full/light-home-unselected.png
ea8ea437cdee06a1076ed4df81a31351db6c76d1c74f591c79d28607575949a3  polish23/before/crop/dark-coinjoin-selected.png
34995be5ee5e3c3630320fbe57e63e82d10bf3879f1558371cb3a3c00b6f7a83  polish23/before/crop/dark-home-unselected.png
5d91a74a9e00ebf60a5dcf4a17cbc34350f2459130fde77ce2313248f727ae9d  polish23/before/crop/light-coinjoin-selected.png
f16194e1e95fb520f230c794c05d7315f01a5dd82f5b6a4c0f263cb1e43b6c04  polish23/before/crop/light-home-unselected.png
9c24fb45cf2ef8494f5ac79a2a46a9e9db912a5687fad3380aa6f7ada792c77a  polish23/before/full/dark-coinjoin-selected.png
fdde58cc68b55ae7665cf5b571d0139c381d9e6f6127d8dd7d3f41b525eb9607  polish23/before/full/dark-home-unselected.png
53aed22385a21dc3e4d9b18212006cd1ca86178ba70bba8cd38c5a88b4c66d12  polish23/before/full/light-coinjoin-selected.png
56925bee68d64fc482db1e806d98815c4def0ab5ce5ccd814dcd6e9baa77e957  polish23/before/full/light-home-unselected.png
```
