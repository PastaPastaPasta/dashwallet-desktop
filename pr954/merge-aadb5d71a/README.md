# PR 954: shared-session captures after merging main

Fixture captures (Chromium in the pinned Playwright image, 1280x1000, the fixture's fixed clock), light and dark.

- Before: main at aadb5d71a7cc80e0a05c11453fd9e071a7008ae3.
- After: the PR head dbf52272b5ec8d25ad25ae4b4cf0b34e54fc1475, which merges that main.

Each checkout ran with capture-only fixture scenarios added. Each scenario gives the session as that revision's engine reports it for the same chain state.

- contested: coordinator, registration sent, with another spend of its coin in a block that isn't final (2 confirmations).
- spendback: participant, out and abandoned, with its spend back in a block that isn't final (1 confirmation).
- participant: participant, funding signed, whose coin another spend holds in a block that isn't final (1 confirmation). The way out to Activity is open.
- pending: coordinator, registration sent, `next` nothing, no `confirming` yet. This is the session as shown before its recovery read.
- refusal: coordinator, Send again confirmed while the engine refuses `shared_stage{conflicting}`.

Main settles each of these chain states as conflicted at once, so nothing is in progress and the sheet opens on its intro. The contested, spendback and participant captures at main were byte-identical, so before/ holds that one pair (not-listed-*.png).

## sha256

```
ab59bd7a3c329b2334eb55d6bafac17729d899b01502af1c0fbcbf2fae4d888e  before/not-listed-dark.png
cff24f77beb2b1209c189343cfa1715b00166b68772f002a60b58a355484e6d7  before/not-listed-light.png
e6edbd3fdb75b5e3dbe5a0af69a2d8a467a93908006f2664e837341484072bbf  head/contested-dark.png
1e6ca720a426918f45e672a6d259232e66dc32e99bec9524db6862c3678052bc  head/contested-light.png
5dd37a1071bbaa1e616b5badd3b73489165f6392f625537e332e2e7e5012dbf3  head/contested-list-dark.png
dee6ba1afba619a9fbde69bb238d73bd45d303455c0e44ac8bd9bd0112f32279  head/contested-list-light.png
1d8c587bf970a9c578f957b30ca7982dfa902d0bde0dae2c4a273671e531d1db  head/participant-dark.png
cb57b1fac29bba483e76893bd485dc993b274c9d890f64de02b9b98582dc34bc  head/participant-light.png
7b95e38eeaca590e5bb73650fad376fd92689a829c5a5a2def0c107d05af195c  head/pending-dark.png
7a9195b65f8a3ffcd248e4e6412bf126c401b6e4165d736b8bab132ad3cd8df2  head/pending-light.png
8b7f119be5516b7d37e8bfa9441d00bf10784574f846a0ab11ebc3b87bf46f32  head/refusal-dark.png
acefb4555c1389d1ff74f4561ddb5933dc54b54927599d9dacba61d4cca9dcc6  head/refusal-light.png
6b84be15e70daf002197f6326d91999e779c7e58a3891975a957c2d44bc47c41  head/spendback-dark.png
cacd470655450d179fe395dbf6760c58b9cea8d5afacbf91339bd1d20a4793fe  head/spendback-light.png
```
