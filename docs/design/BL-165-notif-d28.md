# BL-165: WebKit `notifications.spec.ts:150` (DP2-09 D28) fails on main 913893dd

Backlog row BL-165. Main CI on 913893dd (run 38839086903) failed the UI tests job in WebKit only, at the D28 test "on
the page, an ignored request hands focus to the next one, the last to the heading": `authorize` (`e2e/helpers.ts:66`,
called from `notifications.spec.ts:161`) waited 30 s for the passphrase sheet's field. Chromium passed, and so did
d0fef6b4. Fixed on `dw/bl-165-notif-d28` from main 913893dd.

**Result: a focus race in the app, and the test raced the same frame.** `focusPage` (`src/app/shellBridge.ts`), which
every link and menu command calls after it opens a page, moves focus to the page's heading one animation frame later.
When that frame came late, focus had already moved (here, to the request row the D28 refocus had just picked), and the
heading took it back. The app now leaves focus the user moved with a key or a pointer before the frame (D2). The test
also waits for the page's landing focus before it focuses a control with a script, which no app rule can tell from the
page's own focus moves (D4).

| Part | Where |
|---|---|
| `focusPage` leaves focus moved by a key or a pointer before its frame | `apps/desktop/src/app/shellBridge.ts` |
| Unit tests: body, a sheet's return to its opener, a key, a pointer, a key that left focus on the body | `apps/desktop/src/app/shellBridge.test.ts` ("focus after a link or a menu opens a page") |
| e2e: a route command focuses its page when the closing dialog gives focus back to its opener; a key before a late landing frame keeps its focus | `apps/desktop/e2e/routes.spec.ts` |
| `lateFrames`: frames delayed on demand, and a wait until none is pending | `apps/desktop/e2e/helpers.ts` |
| The D28 page test waits for the page's landing focus | `apps/desktop/e2e/notifications.spec.ts:150` |

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
and moved focus off the row. That fails at `:158` (the row is not focused) or, when the steal lands just after `:158`
passed, at `:161`, which is CI's failure: Tab then goes from the `h1` to the row's open button rather than its Accept,
so Enter opens no passphrase sheet. The race has been there since `focusPage` (5bfc9dc8, 10-14) met the D28 test
(d4ce69b7, 10-15). DP1-08 changed no focus code; at most it moved the frame timing. WebKit under load delays frames the
most, which is why only WebKit shows it.

**D2. App: focus the user moved with a key or a pointer before the frame stays.** The race is not test-only: a link or
menu command can arrive while the user is tabbing or clicking, and the app's WebKitGTK window drops frames under load
as Playwright's WebKit does, so the heading would take focus from where the user had just put it. The heading is a
landing place for focus that has none, not an override. `focusPage` now listens for `keydown` and `pointerdown` (capture,
on the document) from the call until its frame; if one came and focus is on an element other than the body, focus
stays. Otherwise the heading takes it, as before: in particular when a closing sheet returns focus to its opener, a
script move that is not the user's (D5). The listeners are removed in the frame. A key that activates the command
itself (a menu shortcut) is dispatched before `focusPage` adds its listener, so it does not count.

**D3. Evidence.** Repeated runs, one command (`--project=webkit -g "D28 on the page" --workers 4`), load 75–170:

| Renderer and test | Runs | Failed |
|---|---|---|
| base 913893dd, test unchanged | 100 | 3: two at `:158`, one at `:161` (CI's) |
| base 913893dd, test unchanged, earlier (BL-164 worktree, same renderer) | 40 + 59 (probe) | 2 + 2, all at `:158` |
| D2, test unchanged | 100 | 1, at `:157`: see D4 |
| D2 and D4 | 100 (load 74 → 82) | 0 |
| A/B back to back, alternating 50-run legs: base, D2+D4, base, D2+D4 | 100 + 100 (load 154 → 103) | base 0, D2+D4 0 |

So the base fails 7 times in 299 runs and D2+D4 0 in 200; at this rate a load-driven A/B can come out even, as the
back-to-back one did. D6 makes the frame late on purpose, which reproduces it every time.

Mutation checks: with the D2 guard removed, the unit tests for a key and a pointer fail; with the first rule (D5) the
opener unit test and the new routes e2e opener test fail (Chromium and WebKit, 2/2). Gate on 800de647 (fixture rebuilt):
`tsc`; vitest 51 files, 730 tests; Playwright for every spec that asserts focus after a page opens or after a contact
action (`notifications`, `routes`, `contacts`, `add-contact`, `contact-pane`, with their axe checks): Chromium 133/133,
WebKit 133/133 at 2 workers. The D6 test was added after that gate and ran 10/10 (both browsers, 5 each). A copied
routes axe test that timed out once in `analyze()` inside a 15-minute probe run at load ~150 passed 3/3 alone (load
rule). Rust: only `read_image`'s comment changed (BL-164 nit); `cargo fmt --check` and `cargo clippy -p dw-desktop
--all-targets -D warnings` pass.

**D4. Test: the D28 page test waits for the page's landing focus before it focuses Ignore.** With D2 the unchanged test
failed once in 100 at `:157`: Enter never ignored, because the late frame moved focus to the `h1` between the test's
`focus()` on Ignore and its Enter. A script's `focus()` is not user input, so D2 rightly lets the landing focus win (a
user cannot put focus on Ignore without a key or a pointer, which D2 counts). The test now waits until the page `h1` is
focused after `navigate`, as `contacts.spec.ts` and `routes.spec.ts` already do after opening a page. No other spec
focuses a control by script right after a route command or link without such a wait (scan of `e2e/*.spec.ts`). No
timeout was raised.

**D5. Review (Opus) of the first rule: replaced.** The first fix compared `activeElement` in the frame with the element
focused at the call. Opus found a regression: a sheet outside the sheet layer (About, Open URI, Recovery phrase, Unlock,
transaction detail) gives focus back to its opener synchronously when it unmounts (`components/sheet.tsx`), before the
frame, so a route command from such a sheet whose opener stays (a sidebar link) left focus on the opener instead of the
page heading, breaking U-01 D19 (a link or a menu command that opens a page moves focus to its title). D2's input rule
keeps that path, and the new routes e2e test pins it.

**D6. A late frame on purpose: `lateFrames` (e2e helper) and a routes test for D2.** Load alone made the race 2–3 %.
`lateFrames(page)` installs, before the page loads, a `requestAnimationFrame` that once switched on waits 500 ms per
callback and counts those pending, and `settled()` polls until none is: no sleep in the test. The new routes test
switches it on, opens Activity by a menu command and expects its `h1` once the frames settle (the frame still lands
focus), then opens Receive, presses Tab, and expects the focus Tab moved to stay after the frames settle. With the same
harness in a throwaway spec (`/work/scratch/dw-bl165/probe-late-*.log`, Chromium and WebKit, 5 each):

| Test, with late frames | base 913893dd | this head |
|---|---|---|
| the D6 routes test | 10/10 fail | 10/10 pass |
| the original D28 page test body | 6/10 fail, at its `:158` (4) and `:161` (2, CI's) | 1/10 fail, at `:157` (D4's case) |
| the D28 page test with D4's wait | 10/10 pass | 10/10 pass |

The last row shows D4 alone makes the D28 test immune; D2 is what keeps a user's key from losing its focus, and the D6
test is what fails without it.
