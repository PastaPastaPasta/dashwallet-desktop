# BL-165: WebKit `notifications.spec.ts:150` (DP2-09 D28) fails on main 913893dd

Backlog row BL-165. Main CI on 913893dd (run 38839086903) failed the UI tests job in WebKit only, at the D28 test "on
the page, an ignored request hands focus to the next one, the last to the heading": `authorize` (`e2e/helpers.ts:66`,
called from `notifications.spec.ts:161`) waited 30 s for the passphrase sheet's field. Chromium passed, and so did
d0fef6b4. Fixed on `dw/bl-165-notif-d28` from main 913893dd.

**Result: a real focus race in the app, not in the test.** `focusPage` (`src/app/shellBridge.ts`), which every link and
menu command calls after it opens a page, moves focus to the page's heading one animation frame later. When that frame
came late, focus had already moved somewhere else (here, to the request row the D28 refocus had just picked), and the
heading took it back. The test was right and is unchanged.

| Part | Where |
|---|---|
| `focusPage` leaves focus that moved before its frame | `apps/desktop/src/app/shellBridge.ts` |
| Tests: heading from the body, heading when focus stayed, moved focus stays | `apps/desktop/src/app/shellBridge.test.ts` ("focus after a link or a menu opens a page") |

## Decisions

**D1. Cause: `focusPage`'s late frame steals focus moved since the call.** The test opens Notifications with the
`navigate` menu command, which calls `focusPage`. Then it focuses the first request's Ignore and presses Enter; the
request leaves, and D28's `useRefocus` gives focus to the next request's `.bell-row-open`. A WebKit copy of the test
recorded `focusin`/`focusout` and `activeElement` (`/work/scratch/dw-bl165/focus-trace.log`, 59 runs, 2 bad):

| Run | Before Ignore | After the request left |
|---|---|---|
| good | the page `h1` held focus (`focusout H1 … rel=bell-ignore`): the frame had already run | `focusin .bell-row-open`, and it stays |
| bad (both) | no `h1` focus at all: the frame was still pending | `focusin .bell-row-open`, then 16–17 ms later `focusout .bell-row-open rel=H1.top-intro-title`, `focusin H1` |

So the frame `focusPage` asked for ran about 150–200 ms after the navigation, after two key presses and an action,
and moved focus off the row. That fails either at `:158` (the row is not focused) or, when the steal lands just after
`:158` passed, at `:161`, which is CI's failure: Tab then goes from the `h1` to the row's open button rather than its
Accept, so Enter opens no passphrase sheet. The race has been there since `focusPage` (5bfc9dc8, 10-14) met the D28
test (d4ce69b7, 10-15). DP1-08 changed no focus code; it only moves the frame timing, by adding renders and queries.
WebKit under load delays frames the most, which is why only WebKit shows it.

**D2. Fix it in the app: `focusPage` moves focus only if it did not move meanwhile.** The race is not test-only. A link
or menu command can arrive while the user is tabbing or clicking, and the app's WebKitGTK window drops frames under
load as Playwright's WebKit does. Its frame would then take focus from wherever the user had just put it. The page
heading is a landing place for focus that has none, not an override. `focusPage` now notes `document.activeElement` when
it is called, and in its frame moves focus only if focus is still there, on the body, or nowhere. Every case that worked
before still lands on the heading: a dialog that closes drops focus to the body, a menu or a link from outside the
renderer leaves focus where it was, and an old page's control that unmounts leaves the body. The test is unchanged, and
no timeout was raised.

**D3. Evidence.** Same worktree, same command (`--project=webkit -g "D28 on the page" --workers 4`), load 100–150:

| Renderer | Runs | Failed |
|---|---|---|
| base 913893dd | 100 (load 141 → 106) | 3: two at `:158`, one at `:161` (CI's) |
| base 913893dd, earlier (BL-164 worktree, same renderer) | 40 + 59 (probe) | 2 + 2, all at `:158` |
| with the fix | 100 (load 99 → 102) | 0 |
| with the fix, earlier (same patch) | 80 (load ~106) | 0 |

That is 7/199 on the base against 0/180 with the fix. The vitest test "leaves focus the user moved before the frame
came" fails with the guard removed (mutation check). Gate: `tsc`; vitest 51 files, 728 tests; Playwright for every spec
that asserts focus after a page opens or after a contact action (`notifications`, `routes`, `contacts`, `add-contact`,
`contact-pane`, with their axe checks): Chromium 132/132, WebKit 132/132 at 2 workers, load ~100. No Rust changed.
