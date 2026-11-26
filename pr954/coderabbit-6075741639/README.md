# PR 954: states added or changed in the CodeRabbit 6075741639 round

Fixture captures (Chromium, 1280x1000, the fixture's fixed clock), light and dark, at head 23d55523c69d3726e6b9954f0a7fd0e513c141a9.
The head ran in its own checkout with four fixture scenarios added for the capture only.

- pending: coordinator, registration sent, `next` nothing, no `confirming` yet (the session shown before its recovery read): the neutral waiting row.
- participant: participant, funding signed, `next` nothing, `confirming` other_spend 1: the contested row with its way out to Activity open.
- refusal: coordinator, Send again confirmed while the engine refuses `shared_stage{conflicting}`: the refusal's copy.
- spendback: participant, out and abandoned, its spend back in a block that isn't final (1 confirmation): the confirming line, which no longer names a txid.

Base bfe8160719b5d83a220750550abbb2b111c04707 settles each of these chain states as conflicted at once, so the session is not listed there: see ../blocker-fix/base/. The spend back's line at the previous head ef17f6f953fc483b10a96511bf56fa661f0eda78 is ../blocker-fix/head/spendback-*.png.

## sha256

```
8d58750892d4f8360f838964380b94cb12da06a409fc65e53ea7f2e6393c9f84  head/participant-dark.png
ac19f2efff453f2577a11814aaba2cdbeb0bdff5026b2444128942e02e783ff0  head/participant-light.png
3b3b3ca92076618d19e004a8ff2b69b770651b7f773e24d02063ebb9bce5baa3  head/pending-dark.png
5653315abd39174d56c835244b7587ae5903700168ff10d1145f147c56e0252d  head/pending-light.png
8b7f119be5516b7d37e8bfa9441d00bf10784574f846a0ab11ebc3b87bf46f32  head/refusal-dark.png
acefb4555c1389d1ff74f4561ddb5933dc54b54927599d9dacba61d4cca9dcc6  head/refusal-light.png
3894df82dff0c9f879ec19029e2ee466002126f9b63692c79dc431e769d4fa2b  head/spendback-dark.png
968f37ee78c8356694673516ba3095cb30c6571c6753bfac1a621d9957f60a68  head/spendback-light.png
```
