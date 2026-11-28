# Transfer progress on PR #1153

| Revision | Commit | Images |
| --- | --- | --- |
| Before | `2f230f1abd9736ca0c19a5844b9e098b2de65c16` | 38 |
| After | `b70b5edbd4de47f8f5758a8bc8fae9d247070493` | 60 |

Both fixture builds ran in the pinned Playwright 1.64.0 Linux image. Each image captures the transfer sheet in Chromium at a 1280 × 820 viewport, device scale 1, with the same fixture wallet, locale, amount and route. The add uses 0.001 tDASH and each move out uses 0.5 tDASH. All addresses and amounts come from the desktop fixtures.

The base shows one waiting view for each route. The head fixture pauses at each engine stage. Both revisions include the settled and unresolved outcomes, refusals, and the result after closing and reopening the sheet. For the balance-unavailable case the fixture marks the Shielded scan as unsuccessful. For the interrupted-event and reopen cases the head fixture delays the settled event past the command reply. The base has no separate settled event and takes its result from the reply.
