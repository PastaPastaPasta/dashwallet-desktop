# DP2-07: Contacts split view (Tauri)

ROADMAP DP2-07 on the Tauri renderer: the Contacts page of [DASHPAY §4](DASHPAY.md) (`ContactsViewModel`) and F10.
It shows the main identity's contacts in groups (Contact Requests, My Contacts, Pending Requests, Hidden), sorts them
in Android's four orders, filters them locally, searches the Dash network for usernames as the user types, and says
when contacts still wait for an unlock to finish their setup. The page is a split view: the list on the left, a
detail slot on the right. The contact pane's actions (DP2-09) and Add Contact (DP2-08) are later tasks; this one
leaves their entry points (D11, D12).

The engine side is merged: DP2-01's read model (`dashpay_contacts`, `dashpay_pending_setup_count`), DP2-03's user
search (`search_users`) and DP2-04's hidden flag. The page adds no dw-app command and no permission. It follows the
mobile wallets (iOS `ContactsScreen.swift` at `develop`, Android `ContactsFragment`, `strings-dashpay.xml`), not
dash-qt, which has no contacts.

| Part | Where |
|---|---|
| View model (pure TypeScript, no React) | `apps/desktop/src/contacts/model.ts` |
| Its tests, by parity ID | `apps/desktop/src/contacts/model.test.ts` |
| Queries (TanStack, keys under `["dashpay", …]`) | `apps/desktop/src/contacts/queries.ts` |
| Screens | `src/screens/contacts/{ContactsPage,ContactList,ContactPane,Avatar}.tsx` |
| Fixture: engine-faithful sort and filter, scenarios | `src/fixture/{handlers,dashpay,scenarios}.ts`, `src/fixture/contacts.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/contacts.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL) | `e2e-tauri/contacts.ts`, `scripts/e2e-tauri-contacts.sh` |

## Screen inventory

| Part | What it shows | Mobile reference |
|---|---|---|
| Header | "Contacts", Add contact, the sort menu, the search field | iOS banner title + add; Android toolbar and "Sort by:" |
| Pending-setup hint | "Unlock to finish setting up N contacts" with Unlock (vault unlocked for mixing only); "Finishing the setup of N contacts…" (unlocked) | DASHPAY §3.4 |
| Updating line | "Updating contacts…" while a sync pass holds the wallet (`ContactList.updating`) | m4 §2.4 item 37 |
| Contact Requests (n) | incoming requests, newest first, "Request received" | iOS, Android "Contact Requests (%d)" |
| My Contacts | established contacts in the chosen order; channel chips "Setting up", "Payments unavailable" | iOS, Android |
| Pending Requests | outgoing requests, newest first, "Request sent" | iOS "Pending Requests" |
| Hidden (n) | hidden contacts and ignored senders ("Ignored"), collapsed until opened or a search matches | iOS "Hidden" |
| On the Dash network | while the search has ≥ 2 characters: up to 3 usernames not already listed, "Show more results", "No usernames found", a failure with Try again | iOS `networkResultsSection` |
| No match | "There are no contacts that match" | Android `contacts_no_results` |
| Empty | "Add a New Contact", "Find a user and add them to your contacts.", Search for a User | iOS / Android empty state |
| No identity | Join DashPay prompt (or the registration in progress, a watch-only wallet, DashPay starting) | F1 |
| Detail slot | the selected row's name, username, identity id, relation and channel, and a "not available yet" note for DP2-09; Add contact (DP2-08) placeholders | DASHPAY F10/F12 |

## Bindings

| Call | When | Notes |
|---|---|---|
| `dashpay_status`, `identities` | the page opens; `platform` events | which identity's contacts (D2) |
| `dashpay_contacts(wallet, identity, {sections: [], sort, text})` | the page opens, the sort changes, the search settles (250 ms); `platform` and `dashPayNews` events and the wallet's `historyChanged` (m4 §2.4: a mined payment moves `last_activity_at`) | all four sections in one read |
| `search_users(wallet, prefix, 12)` | the search has ≥ 2 characters, 350 ms after the last key, latest wins | D6 |
| `dashpay_pending_setup_count(wallet)` | with the list; `platform` events | D8; `lock_status` picks the copy |
| `dashpay_sync_soon()` | once when the page opens | DASHPAY §3.2 "runs at once when Contacts opens" |

Not called: `dashpay_sync_now` and `dashpay_avatar` are engine stubs at this base (`platform.not_implemented`), so
the page neither offers "Sync now" nor shows pictures (D9, D10).

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | One read lists every section (`sections: []`); the page groups the reply in the engine's order (Requests, Contacts, Pending, Hidden) and drops empty groups. The section names are iOS's: "Contact Requests (n)", "My Contacts", "Pending Requests", "Hidden (n)". | One snapshot keeps the groups consistent with each other; the engine already sorts and filters per section. |
| D2 | The identity is the one DashPay status names: `ready.main`, or `contestPending.identity` (a contested name still has its identity, U-01's sidebar rule). `startupIncomplete` uses the identity `identities` marks main, if any. `noIdentity` and `registering` show the Join DashPay prompt in the page (the sidebar already shows Join DashPay then; a menu shortcut or a link can still open Contacts). A status read that fails with nothing cached (or `startupIncomplete` whose `identities` read fails) shows "Could not read DashPay's state" with Try again, not an endless "Starting DashPay…". | DashPay is per main identity on mobile; the status is the app's one source for it. |
| D3 | Sort (AND-090): a "Sort by" menu with Android's four orders (Name, Username, Date added, Last activity), Name by default. It orders My Contacts and Hidden; requests are always newest first (the engine's rule). The choice lives in the shell store for the session (`ui.contactSort`), not in settings. | DASHPAY §3.4 lists "contact sort" among UI settings; U-04's settings file is not merged at this base. Follow-up F4. |
| D4 | Local search: the field's text, trimmed, goes to the engine as `text` 250 ms after the last key (Activity's debounce); the engine matches username, display name or alias, and from four characters the start of the identity id. Escape clears the field. A blank field filters nothing. | The engine owns matching (one rule for the list and any later caller). |
| D5 | Hidden: a disclosure "Hidden (n)", closed by default. While a search is on, the group is a plain heading with its matches listed (no disclosure to close them); clearing the search brings back the disclosure as it was. Hidden rows are not dimmed: ignored senders say "Ignored". | iOS lists hidden contacts at the bottom at 55 % opacity, which fails AA contrast on a desktop screen; closing the group keeps them out of the way as iOS's position does. |
| D6 | Network search (IOS-072 "inline network username search"): from 2 trimmed characters, 350 ms after the last key, `search_users(prefix, 12)`, latest wins (a query keyed by the prefix). Shown under the local groups as "On the Dash network": only strangers (`relation: stranger`): the identity itself and everyone it relates to are in the groups already, whether or not the local filter matched them, at most 3 rows, "Show more results" when more remain. A row or Show more opens Add contact (D12) with the hit or the query. A failure says "Could not search the Dash network." with Try again; "No usernames found" when nothing is left. A hit flagged `unverified` shows "Unverified". | iOS's teaser: same minimum, debounce, limit and cut. Add Contact owns sending (DP2-08). |
| D7 | Rows: initials avatar (D10), the name shown (alias, display name, username, else the shortened id: the engine's `DisplayName` key), `@username` under it when the name is not the username. Chips: "Setting up" (channel `setupPending`), "Payments unavailable" (`broken`), "Unverified" (`unverified`). Trailing text: "Request received" (Requests), "Request sent" (Pending), "Ignored" (an ignored sender in Hidden). "Unverified" is a new `warning` chip tone drawn on the card's own colour, so a selected row's tint under it keeps the text at AA (axe flagged the translucent version). | F13 says the channel states are shown; Android's "Request sent" is shorter than iOS's "Contact Request Pending" in a 360 px column. |
| D8 | Pending-setup hint (`pending_setup_count` > 0): with the vault unlocked for mixing only, "Unlock to finish setting up N contacts" and an Unlock button (U-02's unlock sheet); unlocked or unencrypted, "Finishing the setup of N contacts…" (the unlock drain or the DIP-15 rescan runs). A locked vault shows the lock screen, so no hint is needed. A failed read (`platform.unavailable` with no snapshot) shows none. | DASHPAY §3.4: "Contacts shows 'Unlock to finish setting up N contacts'". An unlocked wallet that still counts some is mid-drain, so "unlock" would be wrong. |
| D9 | No "Sync now" or pull-to-refresh: `dashpay_sync_now` is a stub at this base. Opening the page asks for a pass with `dashpay_sync_soon` (real), and the page re-reads on the engine's signals. | The task must not stub a facade call silently. Follow-up F3. |
| D10 | Avatars are initials on the accent tint, from the name shown (two letters at most, `?` without letters). No pictures: `dashpay_avatar` is a stub, and the contact rows carry no avatar URL. | Follow-up F2 (DP4's avatar body). |
| D11 | Detail slot (DP2-09's): selecting a row shows its name, `@username`, identity id (monospace, selectable), relation and channel from the row, with "Contact actions, activity and private details are not available yet." (`placeholder-contact-pane`). Nothing else is read. Requests rows have no inline Accept / Ignore yet (F10 lists them): accepting needs a grant and a flow lease, which DP2-09 builds once for the pane and the bell. The selection lives in the shell store (`ui.contact`), so DP3-07 (transaction detail → contact) can open Contacts on a contact. A selection the list does not hold (another wallet, a dropped relation) shows the empty slot; one that only the search filtered out stays in the slot while the search lasts (the last row seen). | DP2-09 owns the actions; one accept flow, not two. |
| D12 | Add contact (DP2-08's): the header's Add contact, the empty state's Search for a User, a network row and Show more open the Add contact placeholder in the detail slot (`placeholder-add-contact`), carrying the query or the hit. A `dashpay://user` link keeps U-01's card (D12 there: the link's username and identity, a disabled Send) in the same slot. Opening Add contact replaces a link's card and a selection; a link arriving later shows its card over the intent; selecting a row clears both. The intent names its wallet, so switching wallets does not carry it over. | Clean entry points that DP2-08 fills. |
| D13 | Keyboard (F10): ArrowUp / ArrowDown move between rows across groups, Home / End to the first and last, Enter or Space selects; each row is one button (`aria-current` when selected). | The Activity table's rule: one control per row. |
| D14 | `updating` (a sync pass holds the wallet): "Updating contacts…" above the list; with no rows yet it replaces the empty state ("Show 'updating', not 'no contacts'"). With rows (a refresh of a known snapshot) the rows stay and the line sits above them. Switching identity drops the previous identity's rows rather than showing them as a placeholder. | m4 §2.4 item 37. |
| D15 | The contacts query key is `["dashpay", "contacts", wallet, identity, sort, text]`, so `platform` and `dashPayNews` events (which make `["dashpay"]` stale) re-read it; `historyChanged` re-reads the wallet's contacts too (`keys.contactsAll`). | m4 §2.4's re-query list. |
| D16 | Fixture: `dashpay_contacts` sorts and filters as the engine does (shown-name order, known usernames first, newest first with unknown times last, ties by id; requests newest first; id prefix from four characters), pending rows carry their request time as `since`, and `last_activity_at` is the engine's `paid.max(since)` (contacts_model.rs). New scenarios: `dashpay-contacts-empty`, `dashpay-contacts-updating` (no snapshot yet: no rows), `dashpay-contacts-refreshing` (rows and `updating`), `dashpay-contacts-mixing-only`, `dashpay-search-offline`. The directory gains four strangers starting with "ma" for "Show more". | The UI tests mean something only if the fixture orders like the engine. |
| D17 | GL launches the app with `--sync`: dw-app starts SPV and DashPay's bring-up only with that flag (`launch.rs`; without it the engine stays idle and Contacts shows "Starting DashPay…" for ever, which the first GL attempt hit). The script restores fixture wallet A with `dwcli import` (phrase on stdin), never sends, accepts or registers, and scans OUT for the phrase (as read and single-spaced) and the passphrase on every exit; dwcli's stderr is printed with the phrase's words masked. | Reads only, as the task asks; the scan proves no screenshot, log or result holds a secret. |
| D18 | Styles reuse Activity's list classes (`tx-groups`, `tx-group`, `tx-group-rows`, `tx-row`, `tx-row-text`, `tx-row-title`, `tx-filter-bar`) and U-01's `add-contact` card; `app.css` gains only a contacts block (split, pane, avatar, disclosure, selected row, opaque chip bases) and a global `chip-warning`. | Minimal, additive shared-file edits (U-04 and U-05 touch `app.css`). |

## Deferred and follow-ups

| # | What | Owner |
|---|---|---|
| F1 | Inline Accept / Ignore on request rows, Unignore from Hidden, the contact pane, the Enable DashPay banner | DP2-09 |
| F2 | Avatars (pictures) in rows, search hits and the pane | DP4 avatar body (`dashpay_avatar`) |
| F3 | "Sync now" / refresh | the `sync_now` body (E0-05 follow-up) |
| F4 | Keep the sort choice in the settings file | after U-04 merges |
| F5 | Add Contact (search, My QR, scan) in the slot's placeholder | DP2-08 |
| F6 | iOS's own-identity card at the top of Contacts and the DashPay FAQ | DP1-08 / the toolbar chip covers it |
| F7 | Fixture vs engine edge cases the fixture does not model: Unicode fold and trim of `text`, homograph-normalised `search_users`, a blank-after-normalising prefix | DP2-08 (search) / fixture upkeep |
| F8 | Arrow keys reach the local rows only; the network rows are Tab stops | DP2-08, when its slot owns the search |
| F9 | Announce the detail slot's change to screen readers (a live region or focus move) once it holds DP2-09's actions | DP2-09 |
| F10 | A `--sync`-less launch (the default) leaves Contacts on "Starting DashPay…"; say that sync is off instead | E0-05 / the launch-flag owner |

## Parity

| Row | Covered here | Tests |
|---|---|---|
| IOS-072 | contacts, pending requests (both directions), local search, inline network search, hidden; not: my identity card, Enable DashPay, FAQ, inline Accept | `model.test.ts` `IOS-072 …`; `contacts.spec.ts` `IOS-072 …` |
| AND-090 | sort by display name, username, date added, last activity; requests section; search; "There are no contacts that match"; not: inline Accept / Ignore | `model.test.ts` `AND-090 …`; `contacts.test.ts` `AND-090 …`; `contacts.spec.ts` `AND-090 …` |

## Slices

1. This document.
2. Fixture: engine-faithful sort and filter, `dashpay_sync_soon`, the scenarios; `contacts.test.ts`.
3. View model and queries: `model.ts`, `queries.ts`, the backend calls, the `historyChanged` invalidation; `model.test.ts`.
4. Screens: the split view, rows, groups, search, hint, detail slot and Add placeholder; App and Routes wiring.
5. Playwright and axe, visual baselines.
6. GL on testnet through tauri-driver; evidence below.

## Evidence

(filled in as the slices land)
