# DP3-07: history rows with the contact; transaction detail → contact pane (Tauri)

ROADMAP DP3-07 on the Tauri renderer: [DASHPAY §5](DASHPAY.md) F15 ("avatar and name on rows"; the title rule label →
contact → type) on Home and Activity, the transaction detail naming the contact and opening its pane (DP2-09's), and
the follow-ups DP3-06 left: F2 (Home's frequent-contacts strip), F3 (the "Include my username in payment requests"
setting) and F4 (rows with avatars). Tier T0 (fixture, Playwright + axe, visual baselines). Copy and layout follow the
mobile wallets: iOS `TxListCell` / `TxDetailViewController` with a DashPay user (the user's avatar in place of the
type icon, the name as the title, the profile one tap away) and Android `TransactionRowView`.

The engine side is merged and adds nothing here: DP3-02 (history and detail counterparties, `frequent_contacts`,
`contact_activity`), DP2-07 (`dashpay_contacts`), DP2-09 (the pane). No dw-app command, binding, contract, §4 table or
error code changes, so no Rust change and no Swift T0 run (D11).

## What DP3-02 exposes

| What | Where | Notes |
|---|---|---|
| `TxRow.counterparty?: TxCounterparty{identity, username, displayName, avatarUrl}` | every `history_page` row, and `TxDetails.rows` | absent when the record is not a contact payment (DP3-02 D1–D3, D22–D23); `username` may land later (D4) |
| Title rule label → contact → type | `format/tx.ts` `txTitle`, `rowParty`, `contactName` (DP3-02 D24) | already wired by DP3-02's follow-merge of U-03 |
| Detail "Sent to contact" / "Received from contact" row | `TxDetailSheet.tsx` (DP3-02 D24) | the name only; no avatar, no way to the contact (D26) |
| `RowDecor.leading` | `components/history.tsx` | the slot U-03 left for the avatar (DP3-02 D26), unused |
| Refresh on `Platform{Contacts, Payments, Identities}` | `app/shellBridge.ts` (DP3-02 D25) | history and the detail re-read when a name lands |
| `frequent_contacts(wallet, identity, limit)` | `screens/pay/contactQueries.ts` `useFrequentContacts` (DP3-06 D3) | Android's rule (DP3-02 D11); under `contactsAll(wallet)` |
| The contact pane | `screens/contacts/ContactsPage.tsx`, selected by `ui.contact` | lists the main identity's contacts in every section (`findContact`) |

## Screen inventory

| Part | What it shows | Mobile reference |
|---|---|---|
| History row (Home, Activity list) | a contact payment: the contact's initials avatar in place of the type icon, the title by the rule (label, else the contact's name); any other row as before | iOS `TxListCell` with a DashPay user; Android `TransactionRowView` |
| Activity table | unchanged: the Address / Label column is the label, else the contact's name, else the address (DP3-02 D24) | dash-qt |
| Transaction detail | the contact row ("Sent to contact" / "Received from contact") gains the avatar, the name and `@username`, and "View contact", which opens Contacts on the contact's pane; the address row stays | iOS `TxDetailViewController` header's user, tap → profile |
| Home ▸ Frequent contacts | up to 8 contacts the main identity pays most (DP3-06's strip), above History; picking one opens Pay ▸ To a contact on them | DP3-06 F2; Android Pay's "Frequent contacts" |

## Bindings

| Call | When | Notes |
|---|---|---|
| `history_page` (`useHistory`) | as U-03 | rows carry `counterparty`; no new call |
| `tx_detail` (`useTxDetail`) | as U-03 | its `rows[0].counterparty` |
| `dashpay_status` (`useContactsOwner`) | detail sheet with a contact; Home | whose contacts the pane lists (DP2-07 D2) |
| `dashpay_contacts(wallet, identity, {sections: [], sort, text: null})` (`contactListQuery`) | detail sheet with a contact | the same query key as Contacts' unfiltered list, so the pane opens warm (D4) |
| `frequent_contacts(wallet, identity, 8)` (`useFrequentContacts`) | Home with an owner | DP3-06's query, shared with the Pay tab |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | A row whose record has a counterparty with a name (`contactName` not `null`) shows the contact's initials avatar (`Avatar`, DP2-07 D10) in place of the type icon; the letters are `initials` of `contactName` (the display name, else `@username`; the `@` is skipped). A counterparty with neither (the username lands later, DP3-02 D4), or whose name has no letter (`initials` would give `?`), keeps the type icon, as the first keeps the type title. A label still wins the title (DP3-02 D24) while the avatar stays the contact's. The avatar is decoration (`aria-hidden`); the title names the contact for a screen reader, or, when a label takes the title, the row's hidden text does (D13). | iOS and Android draw the user's picture where the type icon was. A `?` avatar for a name not yet known would say less than the icon it replaces. The label is the user's own word; the avatar still says who. |
| D2 | Pictures are the initials avatar until DP4-02 / DP4-03 (F1). `avatarUrl` is not fetched by the UI: the engine owns avatar fetching (DASHPAY §4, `avatar()`), and its body is a stub at this base. | Brief; DASHPAY's avatar policy. |
| D3 | The address shows when there is no contact where U-03 shows one: the table's Address / Label column (`rowParty`: label, else the contact, else the address) and the detail's To / Received with row, which stays under the contact row too. List rows carry no address (U-03 D14, iOS). | The brief's "address shown when there is no contact" is DP3-02 D24's fallback; attribution is display data (DP3-02 D4), not a replacement for what was paid. |
| D4 | The detail's contact row offers "View contact" only when the main identity (DP2-07's owner) relates to the counterparty: it is in the identity's unfiltered contact list (`findContact`, any section, hidden included), the same check the Contacts page selects with. Otherwise (another identity of this wallet paid it, the contact went, DashPay not ready, the list not read yet) the row names the contact without the button. "View contact" calls `navigate("contacts", { contact, addContact: null, contactPrefill: null })`, which closes the sheet and opens the contact's pane. The row that opened the sheet went with its page, so focus goes to the contact's selected row (else the Contacts heading) through the sheet's `returnFocus`; opened from the pane's own activity, focus goes to that same contact's list row too, one place for every View contact. | Contacts lists one identity; a button that opened an empty pane would be a dead end. The list query is Contacts' own key, so the page opens on cached rows. |
| D5 | Rows themselves open the detail, not the pane: a row is one button (U-03), and a nested control would break it for the keyboard and for screen readers. The pane is one step on, from the detail (the ROADMAP row's "transaction detail → contact pane"). The pane's activity already opens the transaction (DP2-09), so the two link both ways. | One control per row (U-03 D13). iOS opens the transaction too, and the profile from its header. |
| D6 | The detail's contact row shows the avatar, the name (`contactName`), and `@username` under it when the name is a display name other than the username (`subtitle`, as Contacts' rows). Its label stays DP3-02 D24's ("Sent to contact" / "Received from contact"), and it names the contact of the record whose address the sheet shows (each record's contact is DP3-02 D26's follow-up, F5). The name is the contact's own (display name, else `@username`), not the user's private alias, which `TxCounterparty` does not carry: View contact can open a pane titled by the alias (F7). | DP3-06's payee card and confirm sheet show a contact this way. |
| D7 | Home's frequent strip is DP3-06's strip, moved to `screens/pay/FrequentStrip.tsx` and used by both: Home and the Pay tab render the same component, with their own test ids (`home-frequent`, `pay-frequent`). Home's sits under the DashPay card and above History, in a card; it asks for 8 (DP3-06 D3), shows nothing without an owner, when empty, while loading or when the read failed (a shortcut, as on Pay). Picking a contact opens Pay ▸ To a contact on them (`payContact`, as the pane's Pay does, DP3-06 D2/D15). | Brief (F2); no second copy. |
| D8 | Discreet mode hides Home's strip with the history (UX-SPEC §4.5 QT-039 hides the recent rows): who the wallet pays most is payment history. Pay ▸ To a contact keeps its strip in discreet mode, as it keeps the picker: the user opened Pay to choose a payee, and discreet mode there hides amounts, not contacts. | The Home strip is a summary of the same rows, shown to whoever looks at Home. |
| D9 | F3 is deferred: DASHPAY.md does not define the setting (§5 F15 says request URIs carry the username, with no opt-out), and the engine does not take the flag (`receive_username(wallet)`, `create_receive_request`, `receive_requests` have no parameter; DP3-02 D21's ruling left it unthreaded because dw-ffi is frozen until E0-13). Building a toggle that changes nothing would mislead. Follow-up F3 (engine first, then the Settings ▸ General toggle). | Brief: only if DASHPAY defines it and the engine supports it. |
| D10 | Fixture: the new scenario `dashpay-history-contacts` is `dashpay-registered` (Bob's and Dave's attributed payments) plus the newest payment, from Marta (a stranger to the main identity: no "View contact"), the next one, "Coffee refund", from Bob (a label over the contact; in Bob's activity too), an older payment whose counterparty has no name yet, and the oldest payment, to Judy, a contact the user hid (listed, in the collapsed Hidden section: D4's fallback). Marta's and Bob's are Home's first two recent rows. `dashpay-registered` is unchanged, so its specs keep their rows. | Each D1–D4 branch needs a row; changing `dashpay-registered` would move 37 specs' data. |
| D11 | No Rust, contract or Swift change: `TxRow.counterparty`, `frequent_contacts` and `dashpay_contacts` are bound already (DP3-02 D14, DP3-06). | dw-common: only a §4 or contract change needs Swift T0. |
| D12 | The table mode keeps text only (no avatar column). | dash-qt's columns (U-03 D13); the name is already in Address / Label. |
| D13 | A list row whose title is not the contact's name (a label took it, DP3-02 D24) says the contact in visually hidden text after the title: "from Bob Builder" for a receipt, "to …" for a payment (`contactAside`), so its accessible name is "Coffee refund from Bob Builder 1:10 AM …" on Home and Activity. A row titled by the contact adds nothing. The table (D12) keeps dash-qt's columns: its Address / Label cell shows the label to everyone, as dash-qt does. | Sol r1 P2: the avatar is `aria-hidden`, so a labelled row's contact was visible but not spoken. |
| D14 | Locking does not unmount Home's strip: the lock screen covers the whole window and the shell under it is `inert` (U-02), as for every other part of Home. The spec pins it: under the lock screen no chip takes focus and the lock screen is what is drawn where a chip sits. Discreet mode (D8) is the state that removes the strip. | The shell's lock model is U-02's; a strip-only rule would be a second one. Sol r1 P3. |
| D15 | A change of main identity is not exercised end to end: the fixture holds one identity's contacts per wallet (`DashPayWorld.contacts`, "the main identity's contacts"), so a second identity would list the same contacts and prove nothing. What follows a change is keyed by the owner instead: once `dashpay_status` (`useContactsOwner`) reports another main identity, Home's strip remounts for it (`HomeFrequentOf` is keyed by the identity) and View contact's check reads `contactListQuery(…, identity, …)`, whose key holds the identity, so neither can show the previous identity's answer. When `dashpay_status` re-reads is the shell's (BL-153), not this task's. Per-identity fixture contacts come with the identity switcher (F6). | Sol r1 P3: "if the fixture can model it cheaply"; it cannot without reshaping `DashPayWorld`, which 37 specs share. |

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Avatar pictures on rows, the detail and the strip | DP4-02, DP4-03 |
| F2 | A direction badge on a contact's avatar (UX-SPEC C9's 14 pt badge): the amount's sign says it today | backlog (UI) |
| F3 | Settings ▸ "Include my username in payment requests" (D9): DASHPAY to define it, the engine to take the flag, then the toggle | engine (DP3-03 follow-up), then UI |
| F4 | Activity's search matching contact names (DP3-02 D7) | backlog (engine) |
| F5 | The detail naming each record's contact (DP3-02 D26) | backlog (UI) |
| F6 | "View contact" for a payment of the wallet's other identity (D4): Contacts lists the main identity only | after an identity switcher (DP1) |
| F7 | Rows and the detail naming a contact by the user's alias (D6): `TxCounterparty` would need `alias` | backlog (engine) |
| F8 | Opening Contacts' Hidden section when the selected contact is hidden (View contact here, the bell's rows too): the pane shows, the row stays collapsed | backlog (UI, Contacts) |
| F9 | One name per contact across screens: Contacts and the strip say "dave" (`shownName`), history "@dave" (`contactName`, DP3-02 D24) | backlog (UI) |

## Slices

1. This document.
2. Fixture: the `dashpay-history-contacts` scenario and its test.
3. View model (`format/tx.ts` `contactLetters`, `components/history.tsx` the default leading avatar) and tests.
4. Screens: the detail's contact row and "View contact"; `FrequentStrip` shared by Pay and Home.
5. Playwright and axe (light and dark, Chromium and WebKit); visual baselines.

## Evidence

Base desktop main 75b0d9b6 (unmoved at the report). Gate on f1e5fea1 (the code head; the commit after it is this
document only):

| Check | Result |
|---|---|
| `npx tsc --noEmit -p .` | clean |
| `npx vitest run` | 55 files, 771 tests passed (new: `fixture/historyContacts.test.ts` 3, `format/tx.test.ts` +1) |
| Affected Playwright (history-contacts, wallet, pay-contact, contact-pane; Chromium + WebKit) | 176 passed |
| UI gate (install, tokens, typecheck, vitest, build, build-fixture, guard negative control, selftest judge, Playwright Chromium + WebKit) | GATE EXIT 0: 986 passed, 328 skipped (DW_VISUAL-only), 0 failed, no reruns |
| Visual (`DW_VISUAL=1`, Chromium) | 146 passed; new baselines home-contacts, activity-contacts, tx-detail-contact (light, dark); bell re-baselined for the Home strip |
| axe (serious) | 0 on Home with the strip, the detail with a contact and Activity, light and dark, both engines |
| Rust | not run: no Rust, api or contract diff (D11) |
| Reviews | code-simplifier (Sonnet): 5 of 7 applied (a shared "open contact" helper would touch Notifications, skipped); code-review-validator (Opus) r1: 9 findings, fixed or followed up (F7–F9); delta r2: approved, D1/D6 doc nits fixed, View contact from the pane's own activity focusing the list row recorded in D4 |
