# DP3-06: Pay ▸ To a contact (Tauri)

ROADMAP DP3-06 on the Tauri renderer: [DASHPAY §2.3](DASHPAY.md) (`Recipient::Contact`) and §4's `SendViewModel`
(extended): the "To a contact" tab with the frequent-contacts strip and a searchable picker, paying from the contact
pane, the confirm sheet naming the contact, "Accept and pay" for a request not accepted yet, and the payment-lock
banner. Acceptance (GL on T2): pay `bob` from the UI. There is no T2 devnet script on main (T-01 not merged), so the GL
run is on public testnet (D16).

The engine side is merged and adds nothing here: DP3-01 (`draft_set_contact_recipient`, the contact branch of
`draft_estimate` / `draft_prepare` / `draft_broadcast`, `contact_payment_lock`, `resolve_contact_payment_lock`, the
`AcceptAndPay` and `ContactPayment` flows), DP3-02 (`frequent_contacts`, `contact_activity`, history counterparties),
E0-04 (`authorize_set`, `begin_flow`, `end_flow`) and DP2-02 (`dashpay_accept_request`, `grant_request`). Every call is
already a dw-app command with generated bindings. No dw-app command, contract, §4 table or error code changes, so no
Swift T0 run (D17). Copy follows the mobile wallets (iOS `SendPaymentViewController` with a DashPay user, Android
`PaymentsPayFragment`'s "Frequent contacts" and `strings-dashpay.xml`).

| Part | Where |
|---|---|
| View model (pure TypeScript): payee choice, the banner, refusal wording, the picker's rows, "Accept and pay"'s steps, the promise check for a contact | `src/screens/pay/contact.ts`, the contact branch of `src/screens/pay/model.ts` |
| Their tests, by parity ID | `src/screens/pay/contact.test.ts` (the model's contact branch too) |
| Queries | `src/screens/pay/contactQueries.ts` (frequent strip, picker list, the payee's detail and lock) |
| Screens | `src/screens/pay/PayToContact.tsx` (strip, picker, form, banner); the contact branch of `ConfirmSheet.tsx`, `ResultSheet.tsx`, `useDraft.ts`; `screens/Routes.tsx` (placeholder gone) |
| Shared files (additive) | `app/ui.ts` (`payContact`), `api/backend.ts` + `api/tauri.ts` (bindings), `screens/contacts/ContactPane.tsx` (Pay names the contact, "Accept and pay", the lock note) |
| Fixture | `src/fixture/send.ts` (the contact draft), `src/fixture/handlers.ts` (lock resolve, frequent rule); `src/fixture/payContact.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/pay-contact.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL) | `e2e-tauri/pay-contact.ts`, `scripts/e2e-tauri-pay-contact.sh` |

## Screen inventory

| Part | What it shows | Mobile reference |
|---|---|---|
| Frequent strip | up to 8 contacts the identity pays most, each an initials avatar and its name; picking one selects it | Android `PaymentsPayFragment` "Frequent contacts" (iOS has none) |
| Picker | a search field over the identity's contacts (name, username, alias), My Contacts then Contact Requests, each row an avatar, name, `@username` and why it cannot be paid now | Android contact search in Pay; iOS DashPay user search |
| Payee card | the chosen contact: avatar, name, `@username`, Change | iOS `SendPaymentViewController` header with the DashPay user |
| Form | Amount with Max, Subtract fee, Note (the transaction's message), Advanced (coin control, U-03's), the fee line, Review | U-03's address form without the address, label and Create Unsigned |
| Banner | why the contact cannot be paid now and what to do: the payment lock (Check again, Show transaction), a payment under way, the channel, a mixing-only unlock (Unlock), watch-only, DashPay starting | m1-swift contact branch; iOS "A payment to this contact may have been sent" |
| Confirm | U-03's sheet with "Pay to" naming the contact (avatar, name, `@username`) instead of an address; "Accept and pay" adds the accept line; Send / Accept and pay | iOS `ConfirmPaymentSheet` with a DashPay user |
| Result | U-03's sheet; an accept that went through with a payment that did not says so (C8) | E0-04 C8 |
| Contact pane | Pay opens the tab on that contact; a request offers "Accept and pay" beside Accept and Ignore; a locked contact says so | iOS profile Pay; DP2-09 D9 |

## Bindings

| Call | When | Notes |
|---|---|---|
| `frequent_contacts(wallet, identity, 8)` | the tab opens; `["dashpay"]` invalidations | under `contactsAll(wallet)` (D3) |
| `dashpay_contacts(wallet, identity, {sections: [contacts, requests], text})` | the picker's search | the engine filters (D4) |
| `dashpay_contact(wallet, identity, contact)` | a payee is chosen | its relation, channel and `paymentLock` drive the banner (D6) |
| `resolve_contact_payment_lock(wallet, identity, contact)` | the banner's Check again | D7 |
| `draft_set_contact_recipient(draft, {identity, contact, amount, subtractFee, note})` | every estimate, prepare | `useDraft` writes it in place of `draft_set_recipients` (D8) |
| `draft_estimate`, `draft_prepare(draft, grant or lease)`, `draft_broadcast`, `draft_abandon` | as U-03 | D9 |
| `authorize(spend{amount + fee})` | Send | U-03's sheet, unchanged (D9) |
| `grant_request(acceptRequest)`, `authorize_set([platformOp, spend])`, `begin_flow(acceptAndPay)`, `dashpay_accept_request(lease)`, `end_flow` | Accept and pay | one credential (D10–D12) |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The tab pays the main identity's contacts (DP2-07's owner rule: `ready.main`, or `contestPending.identity`). Without one it shows why (Join DashPay, registering, starting, error) and links to To an address. | One identity pays, as the Contacts page lists one. |
| D2 | Picking a contact (strip, picker, the pane's Pay, a request's "Accept and pay") sets `ui.payContact` (additive in `ui.ts`) with the wallet it was picked in; the tab shows that payee until Change or another pick, and another wallet's tab shows none. The form is keyed by wallet and payee, so a new payee starts a new form (and a new draft: `useDraft` closes the old one). | The pane hands off by id (DP2-09 D9); a payee is the form's recipient, so its amount and note belong to it. |
| D3 | The frequent strip asks for 8 (the engine clamps to 1–20; Android shows a row of about that many on a phone; a desktop column fits 8 chips). It shows nothing when empty or failed (it is a shortcut; the picker is the way in). | DP3-02 D11; Android's rule lives in the engine. |
| D4 | The picker lists My Contacts and Contact Requests from the engine's `contacts` read (sections `contacts`, `requests`, sort by name, the search as the engine's `text`, debounced 250 ms (`SEARCH_DEBOUNCE_MS`, Contacts' search)). Pending (outgoing) and Hidden are left out; a hidden contact is still payable from its pane (D2). A row that cannot be paid stays listed, its reason under the name ("Payments cannot be sent right now"; a request says "Accept and pay"), and selecting it shows the banner. | Only an established contact or an incoming request can be paid (DP3-01 D2). Listing a blocked contact with its reason answers "where is Bob?" better than hiding him. |
| D5 | Avatars are the initials avatar of DP2-07 D10 (`Avatar`, `avatarLetters`) on the strip, picker, payee card and confirm sheet. DashPay pictures are DP4-02/DP4-03 (F1). | The brief: avatars are not done yet. |
| D6 | The banner (`payBanner` in `contact.ts`), first match wins: the payee's detail unread (loading; a failed read says so with Try again); `null` ("This user is no longer in your contacts."), or a relation that is neither a contact nor an incoming request ("You can pay this user once you are contacts."); watch-only ("A watch-only wallet cannot pay contacts.": a contact payment must reserve and sign, Create Unsigned has no address to put in a PSBT); a request while the identity has no DashPay keys (DP2-09's "Enable DashPay first. Then you can accept and pay.", Open in Contacts); `paymentLock` set ("A payment to this contact may have been sent. Check it again before you pay again.", Show transaction, Check again); a `broken` channel ("Payments to this contact cannot be sent right now."); the vault unlocked for mixing only ("The wallet is unlocked for mixing only. Unlock it fully to pay.", Unlock opens U-02's dialog). Then notes that do not hold the payment back: a request ("Accept and pay accepts it and pays in one step."), and the pane's `payNote` for `setupPending` / `notEstablished` (the engine pays both, DP3-01 D19). DashPay not ready is the tab's own state (D1). A `contact.payment_locked` refusal with no lock row (another payment under way, DP3-01 D20) shows as the estimate's problem (D13). | The engine refuses all of these anyway; the banner says why before the user types an amount and what to do (brief). A locked vault never reaches the tab: the lock screen covers the shell. Only `broken` blocks, as the engine and the pane (D15) agree. |
| D7 | Check again calls `resolve_contact_payment_lock`: `sent` → "The payment was sent. You can pay this contact again."; `notSent` → "The payment was not sent. You can pay this contact again."; `unknown` → "Still not known. Check again once the wallet has synced." Then the detail is read again (the engine signals `Platform{Contacts}`, and the action refreshes too). The answer stays under the form once the lock went, but not under a later lock: it spoke of another payment. | m4 §2.5; positive evidence only, so "unknown" must say what helps. |
| D8 | `DraftSpec.recipient` is `Recipient \| ContactRecipient` (told apart by `contact` in it); `useDraft.write` calls `draftSetContactRecipient` for a contact, `draftSetRecipients` otherwise. Nothing else in the draft changes: coin source, fee and change apply to contact payments too (DP3-01 D12). | One draft hook for both tabs (brief: no second copies). |
| D9 | The confirm sheet is U-03's `ConfirmSheet` with a `contact` prop: "Pay to" shows the avatar, name and `@username` instead of an address, Create Unsigned is not offered (the reserved address only exists at prepare; a PSBT would pay the placeholder), and the promise check (`keepsPromise`) takes a contact payment as one non-change output not the wallet's, of the confirmed amount, to whatever address the engine reserved; change, fee and debit are checked as for an address. The prompt and the `spend{amount + fee}` grant come after the sheet, as U-03's already do (DASHPAY §2.3 step 4). | The brief: reuse U-03's sheets. The engine marks the contact's output not the wallet's (`contact_tests.rs` `the_estimate_is_the_payment`), so the check needs no address. |
| D10 | "Accept and pay" (a payee whose relation is `pendingIncoming`) runs DP3-01 D14 from the sheet's Send, under the sheet's one passphrase field: `grant_request(acceptRequest)` sizes the `platformOp`; one `authorize_set([platformOp, spend{total}])` checks the credential once; `begin_flow(acceptAndPay)` makes one lease; `dashpay_accept_request(lease)`; then `prepare(lease)`, the promise check, `broadcast`; `end_flow` once the first broadcast returned (any outcome), or at the step that failed. The steps are `acceptUnderLease` in `contact.ts`, tested against a fake backend: a refused `begin_flow` revokes both grants, a refused accept ends the lease, and a refused credential is the sheet's (it asks again). | One passphrase prompt and one lease (DASHPAY §2.3); dwcli `pay-contact --accept` ends its lease at the same point. A retry after an unknown outcome needs no lease (the dispatch fence is the session's). |
| D11 | No double accept: the sheet's Send is disabled while it runs; once the accept went through, the form remembers it for that contact (`accepted` in the tab's state) and the next Review pays with a plain `spend` grant, even before the relation's re-read lands; `contact.already_contact` from the accept (another surface or device accepted first) counts as accepted and the payment goes on under the same lease. | DP2-09 D3 treats "already contacts" as done; the engine refuses a second accept of a contact before charging anything. |
| D12 | An accept that failed pays nothing: the result is "Payment not sent" with "<the accept's refusal (DP2-09's `actionError`)> Nothing was paid.", also said by a toast (D20). An accept that went through with a payment that did not is E0-04 C8: "You and Bob are now contacts. The payment was not sent: …" (or, for an unknown outcome, the unknown result with "You and Bob are now contacts."), and Back to payment keeps the form for a plain Pay (D11). | m4 §4, E0-04 C8 ("Accepted — payment cancelled. Pay now?"). |
| D13 | Refusal wording, the contact codes in U-03's `sendErrorText`: `contact.payment_locked` ("Another payment to this contact is still being sent or may have been sent. Pay again once it is settled."), `contact.channel_broken` ("Payments to this contact cannot be sent right now."), `contact.ineligible{pendingIncoming}` ("Accept the contact request first, or use Accept and pay."), other `contact.ineligible` ("This contact cannot be paid now."), `contact.self` ("This is you."), `identity.not_found` ("Your DashPay identity is not available. Try again once DashPay has synced."), `platform.unavailable`/`platform.timeout` ("Could not reach Dash Platform. Try again."), `platform.lease_revoked` ("The wallet locked during the payment."), `platform.lease_expired`/`platform.grant_invalid` ("The authorization expired. Send again."), `platform.needs_grant`/`platform.signer_unavailable` ("Unlock the wallet and try again."), `platform.grant_exceeded` ("The payment changed. Review it again."), `storage` ("The payment could not be recorded in this wallet."), `contact.already_contact` (accepted, D11). `send.*` keep U-03's words (`send.cancelled` included); an accept's own refusals use DP2-09's `actionError` (D12). `invalid_argument` gets no words of its own: the engine returns it for a contact draft only on a programming error, and U-03's fallback shows its message. | Every DP3-01 refusal has words (brief). The words say what happened, not whether the payment went: the same text serves a "not sent" result (D18, under "Payment not sent") and an unknown one (under "Payment status unknown"). |
| D14 | After a sent payment the result's Show in Activity opens the transaction as U-03 does; the row and the detail sheet name the contact through DP3-02's counterparty (U-03 D14 / DP3-02 D24). The fixture gives its contact payments that counterparty and an activity item, so the T0 test sees the attribution. | Brief: the payment appears in Activity attributed to the contact. |
| D15 | Contact pane (additive): Pay passes the contact (`payContact`) and is enabled unless the channel is `broken` (`setupPending` and `notEstablished` get a note instead, as the engine pays them; `pane.ts` `payNote`); a request gets "Accept and pay" beside Accept and Ignore (same DashPay-keys hold as Accept), which opens the tab on it; a contact with a payment lock keeps Pay enabled, even when broken, with the note "A payment to this contact may have been sent. Pay shows it and checks it again." | m1-swift says the contact screen disables Send on a lock; on the desktop the Pay tab is where the lock resolves, so Pay leads there instead of a dead button. |
| D16 | GL on public testnet, not T2: `scripts/devnet.sh` (T-01) is not on main. Fixture wallets A (Core funds, ~499 tDASH) and B are contacts both ways; A pays B 100,000 duffs from the real app through "To a contact". The payment is recorded in `<fixture>/DP3-06.json`, written exclusively before Send, so a rerun only re-checks. Checks: the app's result sent, A's Activity row names B, and B's `contact activity --filter received` (dwcli) lists the txid. | ROADMAP §0 T2 needs T-01; the brief allows testnet with a D-row. A pays because B has no Core funds. |
| D17 | No Swift T0: no dw-app command, binding, contract or §4 error code changes (every code worded in D13 is already in m1/m4 §4 and Swift's `engineCodes`). | dw-common: only a §4 or contract change needs Swift T0. |
| D18 | A contact payment's first broadcast refused with `contact.payment_locked` (claimed elsewhere first) or `storage` (the record failed) is "not sent", as U-03's released codes are: the engine held it back before the hand-off and the transaction stays Pending, so the sheet abandons it (`broadcastFailure`'s `CONTACT_HELD`). Any other failure keeps U-03's rule: unknown, nothing abandoned. | DP3-01 D20, D23: both refusals come before dispatch; abandoning frees the reserved coins and address. |
| D19 | Pay's two tabs share their form pieces in `screens/pay/form.tsx` (`FormField`, the amount field and its parse, the live estimate and its refusal, Max, `AmountFields`, the footer, `Phase`); To an address keeps its behaviour and test ids. | Brief: no second copies of U-03. |
| D20 | The accept's outcome is also a toast (`useActionToast`, DP2-09 D23), so it is said even if the user left the sheet's view while it ran. | The accept is an outward action that stays done whatever the payment does (C8). |
| D21 | The promise check also refuses a contact payment to the estimate's placeholder (P2PKH of 20 zero bytes, `send/contact.rs`; mainnet and testnet forms) or to an output without an address. A contact whose DIP-15 account belongs to this wallet (two identities in one wallet paying each other) pays an output the wallet owns, which the check refuses ("The payment changed. Review it again."): such a payment cannot be made from the tab (F6). | Defence in depth against an engine regression that signs the estimate's plan; refusing is the safe failure for the same-wallet case. |
| D22 | Send's re-entry guard is a ref set before the first await, besides the `busy` state. | The accept cannot be undone and costs credits: a second Send in the same frame must not run it twice. |
| D23 | GL load flake: the first testnet run's broadcast ended "Payment status unknown" (load ~130, 3 peers) and its data root went with the run, so its transaction could not be tried again. B, restored and synced to 1573388, had received nothing from it: the ledger moved aside as `DP3-06.failed-1.json` and the GL now presses Try again (the same signed transaction) up to three times on an unknown result. The rerun's broadcast answered at once. | The app did what U-03 D10 says (unknown, locked, never "not sent"); only the harness gave up too early. |
| D24 | The payee card shows the picked row's names until the contact's detail is read (the GL saw the bare id under load); a pane hand-off, with no row, shows the short id meanwhile. | The pick already had the names; the detail is the authority once it lands. |

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Avatar pictures on the strip, picker, payee and confirm sheet | DP4-02, DP4-03 |
| F2 | Home's frequent-contacts strip (DP3-02 D26) | DP3-07 |
| F3 | Settings ▸ "Include my username in payment requests" (DP3-02 D21): the engine calls must take the flag first | DP3-07 |
| F4 | History rows with avatars, detail → contact pane | DP3-07 |
| F5 | The money-move trust gate (DASHPAY §2.2 rule 5) refusing a payment to an unverified entity | DP3-04 |
| F6 | Paying a contact whose DIP-15 account is this wallet's (D21): the engine's `PreparedTxSummary` would need the reserved address, so the check can compare it | backlog (engine) |
| F7 | The fixture does not refuse a second prepared payment to a contact once the first was broadcast (the engine's `ClaimRefused::Spent` → `send.prepared_tx_spent`) | backlog |

## Slices

1. This document.
2. Bindings and fixture: backend calls; the contact draft, lock and resolve, the frequent rule; `fixture/payContact.test.ts`.
3. View model and queries with their tests.
4. Screens: the tab, the confirm and result branches, the pane hand-off; route wiring.
5. Playwright and axe, visual baselines.
6. GL on testnet; evidence below.

## Evidence

- **T0 (fixture):** `tsc` clean; vitest 753/753 (53 files; `contact.test.ts` 20, `payContact.test.ts` 8); Playwright
  458/458 on the affected specs (`pay-contact`, `contacts`, `contact-pane`, `wallet`, `notifications`, `a11y`, `routes`,
  `screens`, `tools`; Chromium and WebKit, light and dark, axe), in the pinned image.
- **Visual:** 140/140 in the pinned image; new baselines `pay-contact`, `pay-contact-form`, `pay-contact-accept`
  (light, dark); `contacts` re-baselined for D15 (Carol's Pay enabled with the longer note); no other baseline moved.
- **GL (testnet, D16):** `scripts/e2e-tauri-pay-contact.sh` PASS 2026-10-16 05:20 UTC on agent-gui :66. Wallet A
  restored at birth 1568806 and synced; B from the frequent strip; the confirm sheet "Pay to D dwrecoverb2, Note DP3-06
  GL, Amount 0.00100000 tDASH, fee 0.00000226"; "Payment sent", txid
  `ed06259d96654f51c934e7bcf9dba13579a03caa9a2b43038682c7b366d05ad2` (InstantSend, height 1573389); Activity's detail
  "Sent to contact @dwrecoverb2"; B's `dwcli contact activity --filter received` lists it (100,000 duffs; 3 before);
  no phrase or passphrase in the output or logs. Ledger `<fixture>/DP3-06.json` "sent"; D23 for the first run.
- **Reviews:** code-simplifier (Sonnet) and code-review-validator (Opus): no critical or high finding; M1–M3, L1, L2,
  L4, L6, L7 (tests), L8 (this document), L9 fixed; L3 → F7; M3's same-wallet case → D21/F6.
