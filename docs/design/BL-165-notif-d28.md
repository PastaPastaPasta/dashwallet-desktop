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

**D2. App: focus that moved after a key or a pointer before the frame stays.** The race is not test-only: a link or menu
command can arrive while the user is tabbing or clicking. That the app's WebKitGTK window delays frames under load as
Playwright's WebKit does is an inference from the Playwright runs, not a native measurement; if it does, the heading
would take focus from where the user had just put it. The heading is a landing place for focus that has none, not an
override. `focusPage` now listens for `keydown` and `pointerdown` (capture, on the document) from the call until its
frame and notes where focus was at the first one. In the frame it leaves focus alone only if an input came and focus has
since moved from that element to another one (not the body). Otherwise the heading takes it, as before: when a closing
sheet returns focus to its opener (a script move, D5), and after a key that moved nothing, such as a modifier or Escape
(D7). The listeners are removed in the frame. A newer call replaces a pending one (its frame cancelled, its listeners
removed), so while frames are paused (a hidden window) one wait is held, not one per link or command (D8); a key before
the newer call does not count for it, as it would not have had the frames come on time. The key that triggers the
command is not counted where the order is known: a browser shortcut runs in the window's bubble phase, after the
document's capture phase, and Open URI calls `applyLink` only after awaiting the route, past its keydown or pointerdown.
For native menu accelerators the order (the page sees the key before the menu event, if at all) is assumed, not tested.

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
WebKit 133/133 at 2 workers. A copied routes axe test that timed out once in `analyze()` inside a 15-minute probe run at
load ~150 passed 3/3 alone (load rule). On the final rule (D7, 8eebb00d): `tsc`; vitest 51 files, 731 tests; the same
Playwright set at load 240–340 ran Chromium 134/134, WebKit 132/134. The two WebKit failures were frames over 5 s late:
`contact-pane.spec.ts:24` (untouched) waiting for its `h1` focus, and the D6 test's `settled()` before it switched the
delay off. Rerun at load 105–134 (`/work/scratch/dw-bl165/verify.sh`, which waits for load < 150): both BL-165 routes
tests 5× per browser 20/20, the D28 page test in WebKit 30/30, `contact-pane.spec.ts:24` 3/3, and with the D2 guard
removed the D6 test fails 2/2 (both browsers) at its `not.toBeFocused()` on the Receive `h1`. Rust: only `read_image`'s
comment changed (BL-164 nit); `cargo fmt --check` and `cargo clippy -p dw-desktop --all-targets -D warnings` pass.

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
`lateFrames(page, ms)` installs, before the page loads, a `requestAnimationFrame` that once switched on waits `ms` per
callback, counting those pending and those run; `settled()` switches it off and polls until none is pending (a late
frame that asks for another must not keep it pending forever): no sleep in the test. The new routes test
switches it on (1.5 s), opens Activity by a menu command and expects its `h1` once the frames settle (the frame still
lands focus), then switches it on again, opens Receive and reads the count of late frames run in the same `evaluate`,
presses Tab, and expects that count unchanged when it reads where Tab put focus (so the page's frame was still
pending; otherwise the test would prove nothing, Opus D7) and that focus to stay after the frames settle. With the same
harness in a throwaway spec (`/work/scratch/dw-bl165/probe-late-*.log`, Chromium and WebKit, 5 each):

| Test, with late frames | base 913893dd | this head |
|---|---|---|
| the D6 routes test | 10/10 fail | 10/10 pass |
| the original D28 page test body | 6/10 fail, at its `:158` (4) and `:161` (2, CI's) | 1/10 fail, at `:157` (D4's case) |
| the D28 page test with D4's wait | 10/10 pass | 10/10 pass |

The last row shows D4 alone makes the D28 test immune; D2 is what keeps a user's key from losing its focus, and the D6
test is what fails without it.

**D7. Delta review (Opus) of D2's first input rule: refined.** Approved with suggestions. (1) Any key or pointer
counted, so a modifier, Escape or a scrollbar click over a sheet's restored opener kept the opener: D2 now notes focus
at the input and yields only if it moved from there (new unit test "a key that moved no focus"; the any-input rule fails
it). (2) The D6 test's Tab half could pass without the frame pending under load: it now checks the count of late frames
run. (3) D2's claim on the triggering key is now limited to the paths shown (browser shortcuts, Open URI). (4) Listeners
waiting on paused frames hold no element: accepted as is, then bounded in D8.

**D8. Review (Sol r1): approve with nits, all applied.** (1) Each pending call kept its listener pair and `atInput`
until its frame, and frames are paused in a hidden window: a newer call now cancels the pending one's frame and removes
its listeners, so one wait is held at most. A frame that cannot be cancelled (`lateFrames` delays it with a timer) is a
no-op once replaced. Unit tests: a newer page before the frame keeps one pair and one frame, and the newer page takes
the focus; a replaced frame that still runs leaves the newer wait and its input in place. Mutation checks: without the
replace, both "keeps one wait" tests fail; without the replaced-frame guard, "a frame replaced but not cancelled" fails.
(2) The D6 routes test holds the node Tab focused by a handle and checks that very node keeps focus, not its test id or
tag. (3) D2 calls the native WebKitGTK frame delay an inference from the Playwright runs. Checks on e3fd8b21
(follow-merged main 6a33c381; `/work/scratch/dw-bl165/verify-r1.sh`, load 111–148): `tsc`; vitest 52 files, 739 tests
(at load ~200 `files.test.ts` and `fixture.test.ts`, untouched, timed out at 5 s twice, and passed at < 150); Playwright
`routes`, `notifications` and `contact-pane`, Chromium 67/67 and WebKit 67/67 at 2 workers; both BL-165 routes tests 5×
per browser 20/20; the D28 page test in WebKit 30/30. No Rust changed since 800de647.
