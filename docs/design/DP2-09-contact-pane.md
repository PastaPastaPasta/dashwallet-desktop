# DP2-09: contact pane, Enable DashPay banner, notifications (Tauri)

ROADMAP DP2-09 on the Tauri renderer: [DASHPAY §4](DASHPAY.md) `ContactDetailViewModel` and `NotificationsViewModel`,
features F9 (Enable DashPay keys), F10's inline Accept / Ignore, F12 (contact profile) and F16's bell popover and page.
It fills DP2-07's detail slot (its D11, follow-ups F1, F9, F12) with the selected contact's actions, payment activity
and private details, adds the Enable DashPay banner to Contacts, and replaces the Notifications placeholder with the
bell's popover and a page. Acceptance (GL on testnet): accept a contact request from the bell in the real app.

The engine side is merged and adds nothing here: DP2-02 (`accept_request`, `ignore`, `unignore`,
`enable_dashpay_keys`, `grant_request`), DP2-04 (`set_private_details`), DP2-05 (`events`, `unread_count`,
`mark_read`), DP3-02 (`contact_activity`), BL-153 (`status`) and DP1-06 (`credits_cost_table`). No dw-app command,
contract or §4 table changes, so no Swift T0 run. Copy follows the mobile wallets (iOS `ContactProfile*`,
`NotificationsViewController`; Android `ContactProfileFragment`, `NotificationsFragment`, `strings-dashpay.xml`).

| Part | Where |
|---|---|
| View models (pure TypeScript): the pane's actions and copy, event rows, the action model over grants | `src/contacts/pane.ts`, `src/contacts/actions.ts`, `src/notifications/model.ts` |
| Their tests, by parity ID | `src/contacts/pane.test.ts`, `src/contacts/actions.test.ts`, `src/notifications/model.test.ts` |
| Queries | `src/contacts/queries.ts` (detail, activity, cost, identity), `src/notifications/queries.ts` |
| Screens | `src/screens/contacts/{ContactPane,ContactList,ContactsPage,EnableBanner}.tsx`, `src/screens/notifications/{NotificationsPage,NotificationsPopover,EventRows}.tsx` |
| Shell wiring (additive) | `components/shell.tsx` (bell popover slot), `app/App.tsx` (route, popover, Contacts' network), `screens/Routes.tsx` (placeholder gone), `api/backend.ts` + `api/tauri.ts` (bindings) |
| Fixture | `src/fixture/{handlers,dashpay,scenarios}.ts`; `src/fixture/pane.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/contact-pane.spec.ts`, `e2e/notifications.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL) | `e2e-tauri/bell-accept.ts`, `scripts/e2e-tauri-bell-accept.sh` |

## Screen inventory

| Part | What it shows | Mobile reference |
|---|---|---|
| Pane header | initials avatar, the name shown, `@username`, chips (DP2-07 D7), identity id (monospace, copyable), relation | iOS `ContactProfile` header |
| Pane actions | by relation (D2): Accept / Ignore, Request sent, Pay, Unignore | iOS profile buttons; Android `ContactProfileFragment` |
| Payment activity | All / Sent / Received, rows newest first, "Show more", a row opens the transaction | Android activity filter (AND-093), iOS profile activity |
| Private details | "Only visible to you": alias, note, Hide this contact; Save; publish state | iOS `ContactInfo` (IOS-128) |
| Enable DashPay banner | "Enable DashPay" with the fee, Enable | iOS Contacts banner (IOS-072, F9) |
| Inline row actions | Accept / Ignore beside each Contact Requests row | iOS inline Accept (F10) |
| Bell popover | Contact Requests (Accept / Ignore), New, "See all notifications" | iOS bell / Android notifications |
| Notifications page | Contact Requests, New, Earlier, "Show more" | iOS `NotificationsViewController` (IOS-075) |

## Bindings

| Call | When | Notes |
|---|---|---|
| `dashpay_contact(wallet, identity, contact)` | a contact is selected; `["dashpay"]` invalidations | private details, publish state, request times (D1) |
| `contact_activity(wallet, identity, contact, cursor, filter)` | an established contact is selected, the filter changes, Show more | keyed under `contactsAll(wallet)`, so `historyChanged` re-reads it too (D8) |
| `grant_request(wallet, identity, action)` | before Accept, Enable and Save (publishing) | sizes the `platformOp` grant (D3) |
| `authorize` / `revoke_grant` | through U-05's `GrantModel` | the passphrase sheet when needed (D3) |
| `dashpay_accept_request`, `dashpay_ignore_contact`, `dashpay_unignore_contact` | the actions | D2–D6 |
| `dashpay_set_private_details(…, grant)` | Save, Hide | D7 |
| `dashpay_enable_keys(wallet, identity, grant)` | the banner's Enable | D10 |
| `identities(wallet)`, `credits_cost_table(wallet)` | Contacts opens | `hasDashpayKeys` and the banner's fee (D10) |
| `dashpay_events(wallet, identity, cursor, limit)` | the popover or page opens; `["dashpay"]` invalidations | D11, D12 |
| `dashpay_mark_read(wallet, identity, upTo)` | once per opening, after the first page shows | D11 |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The pane reads `dashpay_contact` for the selected contact under `[…contactsAll(wallet), "detail", identity, contact]`; the header draws from the list's row at once and the detail (private details, publish state) fills in. A detail answering `null` (the relation went) keeps the row's header and drops the detail parts. | One read per selection; the row already holds what the header needs, so selecting never flashes. |
| D2 | Actions by relation. **Request received**: Accept, Ignore. **Request sent**: "Contact request sent", no action (DASHPAY has no cancel). **Contact**: Pay, private details, activity. **Ignored**: Unignore. A hidden contact is a contact with Hide on. | iOS's profile buttons; Android lists the same per state. |
| D3 | One accept flow for the pane, the request rows, the popover and the page (`ContactActions.accept`): `grant_request({acceptRequest})` sizes a `platformOp` grant, U-05's `GrantModel.withGrant` gets it (the passphrase sheet when "Require authentication for every payment" is on or the vault is not fully unlocked, U-05 D7), then `accept_request(…, grant)`. A vault grant runs as a one-shot lease in the engine (m4 §2.11), so no `begin_flow`. "Accept and pay" is DP3-06's. Enable (D10) and Save (D7) use the same path with their actions. | DP2-07 D11: one accept flow, not two. A single call needs no lease. |
| D4 | Errors are said where the action was: in the pane under its buttons (`role=alert`), as a toast from a request row or the popover, inline on the page. Copy: `platform.insufficient_credits` "Not enough credits. Top up your credits and try again."; `contact.ineligible` (no DashPay keys) "Enable DashPay first."; `contact.already_contact` "You are already contacts."; `platform.unavailable` / `timeout` "Could not reach Dash Platform. Try again."; others the engine's message. A cancelled passphrase says nothing. | One rule per surface; the codes are m4 §5's. |
| D5 | Ignore runs at once (no confirmation) and says "Request ignored. You can find it under Hidden." Unignore says "The request is back in Contact Requests" (the engine returns it on its next sweep, DP2-05 vectors). | Both are free and reversible from Hidden. |
| D6 | After Unignore the network search (`["userSearch", wallet, identity]`) is invalidated, so a cached `ignored` hit shows as a stranger again until the request returns (DP2-07 D21, F12). | DP2-07's hand-off. |
| D7 | Private details ("Only visible to you"): Alias (one line), Note (multi-line), Hide this contact. Save sends all three with a `publishPrivateDetails` grant, so they publish as iOS's `contactInfo` does; the engine leaves the grant unused when nothing is sent (fewer than two contacts, or Platform already holds them) and the UI revokes it after every call (a spent id is ignored). Outcome line: `published` "Saved"; `deferredUntilTwoContacts` "Saved. It is published once you have two contacts."; `local` "Saved on this device." The Hide switch saves at once with the alias and note as last saved. `contact.details_too_long` says "The alias and note are too long." | iOS publishes; the engine owns when. Revoking leaves no live grant behind. |
| D8 | Activity (established contacts, hidden ones too): a segmented All / Sent / Received (Android), 50 per page with "Show more" (`nextCursor`), rows with direction, time and amount in the user's unit (unmasked in discreet mode, as Activity's rows are at this base: discreet mode hides the balance only), a row opens the transaction's detail sheet. Empty: "No payments with this contact yet." The query key sits under `contactsAll(wallet)`, so `historyChanged` and contact and payment signals re-read it (m4 item 47). | The engine pages and filters; the sheet is U-03's. |
| D9 | Pay (established contacts): opens Pay ▸ To a contact, DP3-06's placeholder at this base. Disabled with the reason when the channel is not ready ("Payments with this contact are still being set up." / "…cannot be sent right now."). The payment-lock banner is DP3-06's. | DP3-06 owns paying; the pane only hands off. |
| D10 | Enable DashPay banner (F9): on Contacts, above the list, when the main identity (the one Contacts lists for) has `IdentitySummary.hasDashpayKeys` `false` (not while unknown). "Enable DashPay" / "Add the keys DashPay needs to accept contact requests and pay contacts (a fee of about N duffs in credits)." with N from `credits_cost_table.enableDashpayKeys` over `creditsPerDuff`, rounded up (omitted while unknown; the table is read only while the banner shows). Enable is the one confirmation: it asks for the grant (the passphrase sheet when needed) and runs `enable_dashpay_keys`; afterwards the identities re-read and the banner goes, so success is a toast ("DashPay is enabled."); a failure stays in the banner. While the keys are missing, Accept is disabled with "Enable DashPay first." | DASHPAY F9: "banner in Contacts, one confirmation". The status says whose identity; `identities` says whether its keys are there. |
| D11 | Bell popover (an identity known): a non-modal dialog under the bell (`aria-expanded`, `aria-controls`; focus moves to its heading; Escape or a click outside closes it and focus goes back to the bell, except while an action or its passphrase sheet is open). It lists Contact Requests (Accept / Ignore, at most 5 and "N more"), New (at most 5) and "See all notifications" (the page). Opening reads the first page and, once it shows, marks read up to the newest event id it holds; the rows that were unread stay under New for that opening. Each view (a popover opening, the page) keeps its own snapshot, so the page opened from "See all" right after the popover lists those rows under Earlier. Once the mark is stored the badge re-reads. Without an identity the bell opens the page, which says how to join. | iOS marks the feed read when shown; keeping New stable for the opening avoids rows jumping under the pointer. |
| D12 | Notifications page: Contact Requests, New, Earlier, 50 per page with "Show more" (the engine's cursor). Marks read as D11. A request row's Accept / Ignore act inline; a contact row opens Contacts on that contact (`ui.contact`); a payment opens its transaction. Empty: "No notifications yet". | DASHPAY F16: New / Earlier / Pending, inline accept. |
| D13 | Event copy (names from the identity's unfiltered contact list, else the shortened id): `requestReceived` "X sent you a contact request"; `requestAccepted` "X accepted your contact request"; `contactEstablished` "You and X are now contacts"; `paymentReceived` "X sent you a payment"; `usernameRegistered` "Your username @label is registered"; `contestWon` "You won the vote for @label"; `contestLost` "You lost the vote for @label"; `contestLocked` "The vote for @label ended locked". Time as Activity's day and time. | iOS / Android strings, shortened to one line. |
| D14 | The detail slot announces its change (DP2-07 F9): a polite, visually hidden status line says "<name>, <relation>" when the selection changes; focus stays on the list so the arrow keys keep working. | A live region over the whole pane would read every field. |
| D15 | Request rows get Accept / Ignore as sibling buttons beside the row button (DP2-07 F1), not inside it (no nested buttons); one `RequestButtons` serves the Contacts rows, the popover and the page, each with its own test-id prefix and an accessible name per contact ("Accept heidi"). Arrow keys still move between rows (DP2-07 D13); Tab reaches the two buttons. | Valid markup and DP2-07's keyboard model. |
| D16 | Each surface (Contacts, the popover, the page) owns one `ContactActions` model and its `GrantModel` with U-05's passphrase sheet. One action runs at a time per surface; its buttons are disabled meanwhile and the busy row says "Accepting…". Unmounting cancels a pending grant request. | A request belongs to the screen that made it (U-05 D1). |
| D17 | `ContactsPage` takes the window's `network` (for amounts); App passes it. The bell keeps navigating to the page when no main identity is known. | Minimal shared edits. |
| D18 | Fixture: handlers for `grant_request` (the engine's per-action credit bounds), `contact_activity` (per-contact payments, filtered, paged by 50 with an opaque cursor), `dashpay_enable_keys`, `credits_cost_table`; accept and ignore journal nothing (DP2-05 vectors: a write journals through the sweep). New scenarios: `dashpay-no-keys` (Alice without DashPay keys), `dashpay-low-credits` (balance under one request), `dashpay-bell-empty`. | The UI tests mean something only if the fixture refuses as the engine does. |
| D19 | GL: two fixture wallets that are not contacts (D20). The sender's request goes out through `dwcli contact --spv request` on a scratch data root (phrase on stdin), after its `contact list` showed no relation to the receiver; the recipient's wallet is restored in the real app (`--sync`), the bell must list the request, Accept in the popover (grant through the passphrase sheet), then Contacts must list the sender under My Contacts and the sender's dwcli must list the receiver as a contact. Secrets scanned on every exit as DP2-07 D17, over OUT and the run's log directory. The send is recorded in `<fixture>/DP2-09.json`, written exclusively before it is made, so a rerun never sends again (as DP3-02's ledger); a rerun refuses a ledger of another pair, and a pair that is contacts already (the request was accepted). Script: `scripts/e2e-tauri-bell-accept.sh` → `e2e-tauri/bell-accept.ts`. | The row's acceptance, with the write path the user takes. |
| D20 | The GL pair: the fixture's contacts are A:{B,C}, B:{A}, C:{A,D}, D:{C}, so B–C, B–D and A–D are not contacts. C sends, B accepts: B holds the most credits (~44.8e9) and pays the accept, C (~8.7e9) pays the request; both have DashPay keys. Since this task's run (Evidence) B and C are contacts; B–D and A–D are left, and a new pair needs the ledger moved aside. | The brief: two fixture wallets that are not contacts, with enough credits. |
| D21 | The contact avatar's fill is the accent tint mixed opaque into the card colour (was a translucent tint): on a selected row or the raised popover the translucent tint stacked under the link-coloured initials and fell below AA (axe, serious). Same look on a plain card. | Found by this task's axe states; a shared rule, one line. |
| D22 | Accept that meets `contact.already_contact` (the other surface, or another device, accepted first) reports success: it is what was asked. Every outcome, a refusal too, re-reads what the action touches; Enable revokes its grant in a `finally`, as the engine leaves it unused when the keys were already there. | Review r1 M1, L1, L2. |
| D23 | An outcome that lands after its view closed (the popover closed by the bell, the page left) is said as a toast; while the view is open it is said inline. The action runs on: closing never cancels a Platform write once its passphrase was given. | Review r1 M2; D4's toast for the popover. |
| D24 | Private details: Hide shows the saved value, or the value being saved, and a refused or cancelled Hide shows the saved value again. A re-read of the saved details updates only the fields the user has not edited. Each view reads the event feed afresh on opening (`refetchOnMount: "always"`) and marks read from that read only. | Review r1 M3, H1. |
| D25 | The fixture's activity cursor names its contact and filter (the engine's is bound to its query), `set_private_details` checks the size before the relation and counts the identity's own contacts. | Review r1 L7. |
| D26 | Load flake: the gate's first Playwright run failed 15 WebKit tests, every one on the 30 s test timeout with its element resolved (a11y, onboarding axe, contacts axe, and the two longest contact-pane tests), at load 100–211; the UI half rerun alone passed all 714. | Box rule: rerun alone before blaming the code. |
| D27 | Wallet scope. The bell's open state belongs to the wallet and main identity that opened it (`Toolbar`'s `bellOwner`): a switch of either while it is open, from the keyboard or a link, closes it, so the next wallet's first page is never marked read unopened. An outcome that ends after the window moved to another wallet is always a toast, named for the wallet it ran in ("Savings: You and heidi are now contacts."); the inline outcome belongs to the view that is gone. One hook says outcomes for every surface (`useActionToast`: the popover, the page, the pane, the request rows, the banner, private details), which also closes D23's gap on Contacts: a pane or row outcome that ends after the page was left is a toast. | Sol r1 findings 1 and 3. |
| D28 | Focus after an action whose control leaves with its row (`useRefocus`): in the popover and the page, the request now in its place, else the popover's heading or the page's `h1`; in the Contacts list, the next request row, else the page's `h1`; in the pane (Accept, Ignore, Unignore), its actions area with what the pane offers now and the outcome; Enable, the page's `h1`. Only focus that fell to the body moves, and it retries for a few frames while the closing passphrase sheet leaves the shell inert. The pane's buttons are keyed per action, so React does not reuse the Accept element as Pay and keep focus on a now-disabled button. | Sol r1 finding 2 (was review r1 L3). |
| D29 | Fixture: `World.dashpayOf` gives a wallet its own DashPay state (others share `dashpay`), and `dashpay-two-wallets` has "alice" in both wallets with unread events in each, for D27's tests. The bell tests open the popover only once the bell offers it (`aria-haspopup="dialog"`): before the main identity is known it opens the page, which under load made a keyboard test open the page. | Test support. |

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Pay a contact, "Accept and pay", the payment-lock banner and its resolve | DP3-06 |
| F2 | Add contact (search, My QR, scan) in the slot's placeholder; sending a request from the pane | DP2-08 |
| F3 | Avatars (pictures), the contact's public message and profile | DP4 |
| F4 | OS notifications for contact requests and accepts (F16's notifier part), Dock and tray badges | F16 follow-up (E0-11 notifier) |
| F5 | History rows → contact pane | DP3-07 (`ui.contact` is ready) |

## Slices

1. This document.
2. Bindings and fixture: backend calls, handlers, scenarios; `fixture/pane.test.ts`.
3. View models and queries with their tests.
4. Screens: pane, request rows, banner, popover, page; shell and route wiring.
5. Playwright and axe, visual baselines.
6. GL on testnet; evidence below.

## Evidence

**Gate (copy of the desktop CI gate on a `git archive` of 7f17b123; later commits are this section, the GL script
and parity only):** `GATE EXIT 0`: fmt; clippy (workspace and `dw-app --features selftest`); Rust tests 1894 passed, 0 failed;
tokens, typecheck, vitest 653 passed (46 files), production and fixture builds, the fixture guard and its negative
control, the selftest judge; Playwright + axe 714 passed, 170 skipped (the visual project without `DW_VISUAL`). The
UI half is the rerun (`dw-gate-ui.sh`, same copy): the first run's Playwright lost 15 WebKit tests to the 30 s test
timeout at load 100–211, ten of them in specs this task does not touch (D26).

**T0 (fixture backend, Playwright in the pinned image, `scripts/e2e-docker.sh`):** vitest 653 passed;
`e2e/contact-pane.spec.ts`, `e2e/notifications.spec.ts`, `contacts.spec.ts`, `routes.spec.ts` and `screens.spec.ts`,
Chromium and WebKit, 278 passed, then the specs touched by the review fixes 40 passed; visual baselines (Chromium) 68
passed, with `contacts` and the new `notifications-popover` / `notifications-page` light and dark baselines. axe
(serious and critical) in light and dark, both engines: the popover, the page, the pane on a request (`pane-request`)
and on a contact (`pane-contact`), and the Enable DashPay banner (`enable-banner`).

**GL (real app, real engine, public testnet; 2026-10-15 14:12–14:13 UTC):** `scripts/e2e-tauri-bell-accept.sh` with
`DW_TESTNET_FIXTURE` = the testnet DashPay fixture. Pair: C and B, which were not contacts (C's contacts were A and D).
The sender's `dwcli contact --spv request` ran first and was recorded in the fixture's ledger `DP2-09.json` (written
with `wx` before the send, so a rerun cannot send twice); then wallet B was restored by `dwcli import --birth-height
1568806` (phrase on stdin) and the app launched with `--network testnet --sync` under tauri-driver on a private X
display. Evidence: `result.json` and four screenshots (`1-bell-request`, `2-grant-sheet`, `3-accepted`,
`4-my-contacts`).

| Step | Result |
|---|---|
| C before | contacts `CgPsN9…` (A) and `CQdTQx…` (D) |
| C → B request | 14:12:41, grant `max_credits` 150000000, outcome `pending`, request `1792073559863`; sender C `44ZjKSRCzscQaNymnykM4BnY7YzBsNJqiF1LpmTHWyPc`, receiver B `HjfQJmEbqM8bbwtoGMVbouiWVC9Wmk6187VLx3XjN4J6` |
| Unlock B, open Contacts | the real renderer; bell badge empty |
| Open the bell | 14:13:05: Contact requests lists 1 row, `44ZjKS…WyPc`, Accept and Ignore; New lists "Your username @dwrecoverb2 is registered" |
| Accept from the bell | the passphrase sheet (a `platformOp` grant); passphrase given |
| Outcome | 14:13:10 `bell-outcome` "You and 44ZjKS…WyPc are now contacts."; the bell's requests empty, New gains "You and 44ZjKS…WyPc are now contacts" |
| B's Contacts | My Contacts lists `44ZjKS…WyPc`, relation Contact; Contact Requests gone |
| C after (`dwcli contact list`) | A, D and `HjfQJm…` (B), each relation `contact`, channel ready |
| Secret scan | no phrase or passphrase in the output directory or the run's logs |

Seen on the way, not changed here: the badge read 4 with the popover open, because three "dwrecovera2 sent you a
payment" events and the accept arrived after it opened (D11 marks read on opening only); and those payment events
carry the time the engine saw them while syncing, not the payments' own times (DP2-05's journal).
