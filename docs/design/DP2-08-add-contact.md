# DP2-08: Add Contact (Tauri)

ROADMAP DP2-08 on the Tauri renderer: [DASHPAY §4](DASHPAY.md) `AddContactViewModel` and feature F11 (IOS-073; the
scanner parts of IOS-043). It fills the Contacts detail slot that DP2-07 left for it (its D12, follow-up F5) and
DP2-09's F2: find a user by username, check before any prompt whether a request can go to them, send it, show the
wallet's own link as a QR code ("My QR"), and scan a contact's QR code from an image file, a drop, the clipboard or a
region of the screen. Every scanned or pasted code is verified against Platform before a request can be sent. A
`dashpay://user` link (DASHPAY §3.8) opens Add Contact prefilled and verified.

The engine side is merged: DP2-03 (`search_users`, `my_user_link`, `verify_scanned`), DP2-02 (`eligibility`,
`send_request`, `grant_request`) and BL-153 (`status`). This task adds dw-app commands that turn images into text
(QR decoding with dw-desktop's `rqrr` decoder, which the Swift hosts already use) and capture a screen region; it
changes no engine call, contract or §4 table, so no Swift T0 run. Copy follows the mobile wallets (Android
`strings-dashpay.xml` "Add a New Contact", "Search for a User on the Dash Network", "Send Contact Request"; iOS
`DashPayUserLink`, its scanner's "couldn't be verified on the Dash network").

| Part | Where |
|---|---|
| View model (pure TypeScript): search, candidate, eligibility, scan, My QR | `src/contacts/add.ts`, tests `src/contacts/add.test.ts` |
| The send, beside DP2-09's actions | `src/contacts/actions.ts` (`ContactActions.send`) |
| Queries | `src/contacts/queries.ts` (user link, eligibility; the search shares DP2-07's key) |
| Screens | `src/screens/contacts/AddContact.tsx`; `ContactPane.tsx` (slot), `ContactsPage.tsx` (no-identity slot) |
| Scan commands | `rust/crates/dw-app/src/api/scan.rs`; screen capture `rust/crates/dw-desktop/src/screen.rs` |
| Fixture | `src/fixture/{handlers,dashpay,scan,scenarios}.ts`; `src/fixture/addContact.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/add-contact.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL) | `e2e-tauri/add-contact.ts`, `scripts/e2e-tauri-add-contact.sh` |

## Screen inventory

| Part | What it shows | Mobile reference |
|---|---|---|
| Header | "Add a New Contact", Close | Android `add_new_contact` |
| Tabs | Search, Scan, My QR | iOS search screen with its QR buttons |
| Search | "Search username" field (prefilled from Contacts' search), the hits with name, `@username` and relation, "No usernames found", a failure with Try again | Android `search_user_*`, iOS `SearchUser` |
| Scan | a drop area "Drop a QR code image here", Open Image…, Scan a Screen Region (or the OS shortcut, D10), "Paste a link or a QR code image" field, the scan's error | iOS scanner, IOS-043 desktop sources |
| My QR | the wallet's `dashpay://user` link as a QR code, the link (selectable), Copy Link; no username yet; names updating | iOS "My QR" (plain link) |
| Candidate card | initials avatar, name, `@username`, identity id, "Verified on the Dash network" or "Unverified", where it came from, the eligibility line, Send Contact Request (or what to do instead), the outcome | iOS profile preview before a request |
| Link opened | the candidate card for a `dashpay://user` link, verified, or the link's error | DASHPAY §3.8 |

## Bindings

| Call | When | Notes |
|---|---|---|
| `search_users(wallet, prefix, 12)` | Search has ≥ 2 trimmed characters, 350 ms after the last key, latest wins | same query key as DP2-07's network rows (D3) |
| `dashpay_verify_scanned(wallet, text)` | a scanned, dropped or pasted text, or a link's rebuilt text | D6; the text never enters a query key or the store (D16) |
| `dashpay_eligibility(wallet, identity, contact)` | a candidate shows; after a send; `["dashpay"]` invalidations | D4 |
| `grant_request(wallet, identity, {sendRequest})`, `authorize`, `dashpay_send_request(wallet, identity, to, scan, grant)` | Send Contact Request | DP2-09's grant path (D5) |
| `dashpay_my_user_link(wallet, identity)`, `qr_matrix(link)` | My QR opens | D12 |
| `scan_qr_file()` | Open Image… | new (D9): the OS dialog, PNG / JPEG / BMP, decoded in Rust |
| `scan_qr_image(data)` | a dropped or pasted image | new (D9) |
| `scan_qr_screen()`, `screen_scan_mode()` | Scan a Screen Region | new (D10) |
| `dashpay_enable_keys` | the card's Enable (no DashPay keys) | DP2-09's `ContactActions.enable` |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Add Contact is the Contacts detail slot (DP2-07 D12): the header's Add contact, the empty state's Search for a User, a network row and Show more open it, and a `dashpay://user` link opens it too (it replaces U-01's unverified link card). Close empties the slot. The intent stays wallet-scoped (DP2-07 D12): another wallet never shows it. | One place for adding, beside the list it adds to. |
| D2 | Three tabs, Search (the default), Scan and My QR, as an ARIA tablist (arrow keys move, Home / End). A network hit, a scan or a link shows its candidate card above the tabs; Search keeps its text and hits while a candidate shows, so going back is one click. | iOS's search screen with its QR entries; one column fits the slot. |
| D3 | Search: the field starts with the query Contacts carried; from 2 trimmed characters, 350 ms after the last key, `search_users(prefix, 12)`, under the key DP2-07's network rows use (`["userSearch", wallet, identity, prefix]`), so the two share answers and Unignore's invalidation (DP2-09 D6). Every hit is listed, with its relation as a note ("Contact", "Request sent", "Request received", "Ignored", "You"), so a user the list already holds is found too. "No usernames found", or "Could not search the Dash network." with Try again. | iOS lists every match with its state; Contacts' teaser shows strangers only because the groups above it list the rest. |
| D4 | Eligibility pre-check before any prompt: an `ignored` relation answers at once ("You ignored their contact request." and Show in Hidden), since the engine's eligibility treats an ignored sender as a stranger; otherwise `dashpay_eligibility`, which answers from the wallet's state first (DP2-02). `ok` offers Send Contact Request; `isSelf` "This is you."; `alreadyContact` "Already in your contacts." and Show Contact; `pendingOutgoing` "Contact request sent." and Show Request; `pendingIncoming` "They sent you a contact request." and Show Request (the pane's Accept, DP2-09); `noDashPayKeys` "Enable DashPay to send contact requests." and Enable (DP2-09's `ContactActions.enable`, the banner's flow); `contactCannotReceive` "This user cannot receive contact requests yet."; a failed read "Could not check this user." and Try again. Show selects the row in the list (`ui.contact`), where DP2-09's pane acts. | The brief's states, each with the one next step. No second accept flow. |
| D5 | Send is `ContactActions.send`, DP2-09's model (D3 there): `grant_request({sendRequest})` sizes the `platformOp` grant, U-05's `GrantModel` gets it (the passphrase sheet when needed), then `dashpay_send_request(…, scan, grant)`. `pending` says "Contact request sent to X."; `established` (their request was already in) "You and X are now contacts.". Refusals: DP2-09's copy, plus `contact.request_pending` "A contact request to X is already being sent.", `contact.scan_expired` "This QR code has expired. Ask for a new one.", `contact.self` "This is you.", `contact.ineligible` the reason's line from D4. Afterwards the contacts, the search and the eligibility are read again, so the card says "Contact request sent." and the list shows the row under Pending Requests. An outcome that lands after the slot closed or the wallet changed is a toast (`useActionToast`, DP2-09 D27); focus goes to the card's status line when the Send button leaves (`useRefocus`, D28). | One send and outcome path for every surface. |
| D6 | Verification: every scanned, dropped, pasted or linked text goes through `dashpay_verify_scanned`, and only its answer becomes a candidate: Platform's spelling of the username, the identity, the relation and `unverified`. `contact.scan_unverified` "This QR code couldn't be verified on the Dash network."; `contact.scan_expired` "This QR code has expired. Ask for a new one."; `invalid_argument` "This is not a DashPay contact code."; `platform.unavailable` / `timeout` "Could not reach Dash Platform." with Try again; others the engine's message. An error state has no Send. | The brief: an unverifiable link is an error, not a send. iOS's scanner refuses the same way. |
| D7 | An auto-accept code (`dash:?du=&dapk=`, `AutoAcceptClaim`) sends with its `scan` id. The card says "This code asks them to accept you automatically. They confirm it when your request arrives." and when the code expires; an expired claim at send time is D5's `contact.scan_expired`. Search hits and user links send without one. | DP2-03: the claim is unconfirmed until the recipient accepts. |
| D8 | An image with several QR codes: their texts are tried in reading order (dw-desktop's order), and the first that verifies wins; texts that are not contact codes (`invalid_argument`) are skipped; any other refusal stops there and is said. None left: "There is no DashPay contact code in this image." | A screenshot often holds other codes (a payment request beside the contact's). |
| D9 | Scan sources. **Open Image…**: `scan_qr_file`, the OS dialog with a PNG / JPEG / BMP filter, read and decoded in Rust (`dw-desktop::qr`, 32 MiB / 40 MP), no path crossing IPC (U-05 D2). **Drop**: an image dropped on the drop area (or anywhere on the Scan tab) goes to `scan_qr_image` as bytes; a dropped text or link is verified as pasted. **Paste**: the "Paste a link or a QR code image" field takes Ctrl/Cmd+V; a pasted image goes to `scan_qr_image`, text is verified when pasted or on Enter. There is no "read the clipboard" button: the webview's clipboard read prompts or fails on WebKitGTK (U-05 D16). Image errors: `app.image_unreadable` "This image could not be read. Use a PNG, JPEG or BMP file."; `app.no_qr_code` "No QR code was found in this image."; the file codes as U-05's. | The desktop's ways in for a picture; one decoder in Rust for all of them. |
| D10 | Screen region, per OS (`screen_scan_mode`). **Linux**: the XDG desktop portal's `Screenshot` with `interactive`, so the desktop's own picker selects the region and the app gets the image's file (no new permission for an unsandboxed app); a session without the portal is `app.unsupported`: "This desktop has no screen capture. Take a screenshot, then open or paste it here." **Windows**: the Snipping Tool's region capture (`ms-screenclip:`), then the image it puts on the clipboard (no permission prompt); the wait ends after 60 s, and Cancel in the app stops listening. Not run on Windows yet (GW follow-up F4). **macOS**: not shipped. An app that captures the screen (`screencapture -i`, CoreGraphics or ScreenCaptureKit) needs the Screen Recording permission, a new native permission flow; without it the capture holds only the wallpaper. The button is replaced by "Press Control-Shift-Command-4 to copy a region of the screen, then paste it here.", the system's own capture, which needs no permission (follow-up F1). The portal decides where its file lives (GNOME keeps it in Pictures); the app reads it and does not delete a file it did not make. | The brief: ship the OSes that need no new permission flow, record the gap. |
| D11 | Drag and drop: Tauri's own drop handler is turned off on the main window (`dragDropEnabled: false`), so the webview receives HTML5 drops with the file's bytes and no path. A drop outside a drop area does nothing: the document cancels `dragover` / `drop` unless a drop area took it, so a dropped file never navigates the webview. Nothing used Tauri's drop events. | The renderer never names a path (U-05 D2); without the guard a webview opens a dropped file. |
| D12 | My QR: `dashpay_my_user_link` drawn with `qr_matrix` (U-03's QR component), the link in monospace and Copy Link. iOS's plain link only: no auto-accept code is made (DASHPAY §7.8). `contact.no_username` "Your username is not registered yet, so there is no link to share."; `platform.unavailable` "Your username is being updated…" with Try again. Re-read on `["dashpay"]` invalidations (the main name may change, m4 §2.4). | iOS's My QR; the engine owns the link's form. |
| D13 | A `dashpay://user` link opens Contacts with Add Contact showing the link's candidate: its text is rebuilt in `DashPayUserLink::to_uri`'s form (`dashpay://user?id=…&username=…`, the label percent-encoded) and verified as D6. While verifying, the card shows the link's username as "Checking @x on the Dash network…". The wallet the window shows verifies it; a wallet switch verifies again. With no identity (Join states, BL-153), the card still verifies and says "Join DashPay to send contact requests." with Join DashPay in place of Send. | DASHPAY §3.8; BL-153's status gates sending at all. |
| D14 | The verified candidate is checked against the identity's unfiltered contact list as well as its relation (DP2-07 D21): a request that arrives or goes after the check moves the card to the matching D4 line, because the eligibility re-reads on `["dashpay"]` invalidations. | A card must not offer Send to a contact. |
| D15 | Keyboard and focus: the tabs as D2; a new candidate or a scan error moves focus to the card's heading or the error (the result of a click elsewhere is otherwise silent); Close returns focus to the header's Add contact. | Where a keyboard user expects the answer. |
| D16 | Secrets: a scanned or pasted text may carry a `dapk` key (a bearer secret, DASHPAY §3.8). The model keeps it only while `verify_scanned` runs and for Try again on a network failure; it never enters a query key, the shell store, a log or the URL, and the paste field is cleared once verified. A dropped or pasted image is passed on and not kept. | Bearer credentials stay out of caches and logs. |
| D17 | Fixture: handlers for `dashpay_send_request` (the engine's order: the ids, itself, the scan, eligibility, credits, the grant), `dashpay_my_user_link`, `dashpay_verify_scanned` (the user link and `dash:?du=&dapk=` forms, expiry, ownership) and the scan commands, whose "images" are text files starting `fixture-qr:` with one code per line (anything else is `app.image_unreadable`, `fixture-qr:` alone `app.no_qr_code`). New scenarios: `dashpay-add-contact-offline` (verification fails as with DAPI down), `dashpay-no-username` (My QR's no-name state), `dashpay-screen-shortcut` (macOS's shortcut text). | The UI tests mean something only if the fixture refuses as the engine does. |
| D18 | GL: wallet A of the testnet DashPay fixture restored in the real app (`--sync`), Contacts ▸ Add contact, search the receiver's username, pick it, the eligibility says Send, send (passphrase sheet), then the list shows the receiver under Pending Requests. Then Open URI… with the receiver's `dashpay://user` link: Add Contact shows the receiver verified and "Contact request sent.", no Send. The pair is A → D (not contacts after DP2-09). The send is recorded in `<fixture>/DP2-08.json`, written exclusively before it is made, so a rerun never sends twice; the request is left unaccepted. Secrets scanned on every exit as DP2-07 D17. | The brief's acceptance with the write path the user takes. |

## Deferred and follow-ups

| # | What | Owner |
|---|---|---|
| F1 | macOS screen-region capture inside the app (Screen Recording permission flow, or ScreenCaptureKit's system picker on macOS 14+) | macOS packaging / GM |
| F2 | Webcam scanning | DASHPAY F11 "webcam later" |
| F3 | Avatars in hits and the card | DP4 avatar body |
| F4 | Run the Windows snip path on Windows (GW) | W / GW |
| F5 | Making auto-accept (`dapk`) codes for My QR, opt-in with a 1 h expiry, once a mobile scanner reads them | DASHPAY §7.8 |

## Parity

| Row | Covered here | Tests |
|---|---|---|
| IOS-073 | search by username with eligibility, send, My QR (plain link), scan with verification, the `dashpay://user` link | `add.test.ts` `IOS-073 …`; `add-contact.spec.ts` `IOS-073 …` |
| IOS-043 | the Tauri app's sources: image file, drop, pasted image or text, screen region (Linux, Windows); not macOS's region nor a webcam | `scan.rs` tests; `add-contact.spec.ts` `IOS-043 …` |

## Slices

1. This document.
2. Scan commands: dw-desktop's screen capture, dw-app's `scan` domain and its capability, `dragDropEnabled: false`.
3. Fixture: the DashPay and scan handlers, scenarios; `addContact.test.ts`.
4. View model, the send action and queries, with their tests.
5. Screens: Add Contact in the slot, the link, the document drop guard.
6. Playwright and axe, visual baselines.
7. GL on testnet; evidence below.

## Evidence

To be filled in by the last slice.
