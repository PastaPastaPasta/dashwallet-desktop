# DP4-03: Edit Profile and avatars in every list (Tauri)

ROADMAP DP4-03 (M, UI, T0): "UI: Edit Profile (crop, unsaved-changes guard); avatars in every list". DASHPAY F17
("display name ≤ 25, about ≤ 140; file, drop, clipboard, URL, Gravatar; crop; Imgur only when a key exists (B1)") and
§4's `EditProfileViewModel` / `AvatarPickerViewModel` ("the form, crop, unsaved-changes guard": `profile_limits`,
`prepare_avatar`, `upload_avatar`, `update_profile`). It also takes DP4-02's F5 (the Settings row "Load contact
pictures" and pictures in every list) and F9 (avatar reads reset when the toggle changes; an e2e case for a thumbnail
that does not load), and DP1-10's F1.

The engine is merged: DP4-01's `profile`, `profile_limits`, `update_profile`; DP4-02's `prepare_avatar`,
`avatar_upload_available`, `upload_avatar`, `avatar` and the toggle. dw-app exposes them all (E0-13, DP4-02). This task
adds no engine call and no contract change; dw-app gains one file-dialog purpose (D3). Copy follows the mobile wallets
(Android `strings-dashpay.xml` at v11.9.1: `edit_profile`, `char_count`, `public_url_*`, `gravatar_*`,
`image_cropper_title`, `profile_picture_upload*`, `save_profile_reminder_text`), adapted (D-rows).

| Part | Where |
|---|---|
| The edit model (form, picture choice, save, conflict, guard state), its copy and error texts; one per wallet | `apps/desktop/src/dashpay/editProfile.ts` |
| The square crop's geometry (pure) | `apps/desktop/src/dashpay/crop.ts` |
| Their tests, by parity ID (IOS-076, AND-095…097) | `src/dashpay/{editProfile,crop}.test.ts` |
| Edit Profile sheet: the form, Public URL, Gravatar, the crop step, the discard question | `src/screens/dashpay/EditProfileSheet.tsx` |
| One avatar for every list: the picture scope (window's wallet, the toggle), `Avatar` with `identity` | `src/screens/contacts/Avatar.tsx`, `src/dashpay/profileQueries.ts` |
| The toggle's Settings row | `src/screens/settings/Privacy.tsx` |
| The leave guard (D11) | `src/app/ui.ts` |
| Shared files (additive): `Backend` and its binding, the scope in App, the avatar at each list's call site, My Profile's Edit button, the fixture's handlers and scenarios, styles | `api/{backend,tauri}.ts`, `app/App.tsx`, `components/{history,shell}.tsx`, `screens/contacts/*`, `screens/pay/*`, `screens/notifications/Notifications.tsx`, `screens/TxDetailSheet.tsx`, `screens/dashpay/MyProfile.tsx`, `fixture/{handlers,dashpay,scenarios,profile}.ts`, `styles/app.css` |
| dw-app: `FilePurpose::AvatarImage` (D3) | `rust/crates/dw-app/src/shell/files.rs`, generated `FilePurpose.ts` |
| Playwright (flows, axe; light and dark; Chromium and WebKit) and baselines | `e2e/edit-profile.spec.ts`, `e2e/avatars.spec.ts`; `e2e/visual.spec.ts` |

## Bindings

| Call | Used for |
|---|---|
| `identities` (the shell's query) | the baseline: the main identity's `profile` when the sheet opens (D6) |
| `dashpay_profile_limits` | the counters and the length checks (D7) |
| `grant_request({updateProfile})`, then U-05's `GrantModel` | the save's `platformOp` grant (as DP2-09's actions) |
| `dashpay_update_profile(identity, edit, grant)` | Save: the whole profile with its `baseline` |
| `dashpay_prepare_avatar(source)` | a Public URL, a Gravatar, a cropped file (D4) |
| `dashpay_avatar_upload_available`, `dashpay_upload_avatar(candidate)` | a file's picture, only when an upload host is configured (D5) |
| `dashpay_avatar(identity, size)` | every avatar (D12) |
| `load_contact_pictures`, `set_load_contact_pictures(on)` | the Settings row; whether lists read other users' pictures (D13) |
| `open_file("avatarImage")` | Choose File… (D3) |

Re-reads: a save's `Platform{Identities}` signal re-reads `["dashpay", …]`, which holds the identities, the own avatar
and the contacts. A toggle change resets every `["dashpay","avatar", …]` read (D14).

## State inventory

**Edit Profile** (`edit-profile-sheet`; `data-step` on `edit-profile-body`), a sheet over My Profile, title "Edit
Profile". axe runs on every row in both themes on Chromium and WebKit; the visual baselines (light and dark, Chromium)
cover E2, E3 with E4, E4 (new), E7, E9, U2, G1, K1, Q1 (D21).

| # | Input | Shows | Scenario |
|---|---|---|---|
| E1 | `form`, profile without a picture | the initials avatar with Change picture; Display Name (placeholder the username) and About me, each with "N/25 characters", "N/140 characters"; Save Changes (disabled until something changed), Cancel | `dashpay-registered` |
| E2 | `form`, profile with a picture | E1 with the picture and Remove picture | `dashpay-avatar` |
| E3 | a field over its limit | the counter in red, the field `aria-invalid`, "At most 25 characters." (or 140); Save held | any |
| E4 | `form`, a new or removed picture | the new preview (or initials) with "New picture" (or "Picture removed") and Undo | any |
| E5 | `saving` | "Saving…" with a spinner under the fields, every field and button disabled, the sheet not closable; a file's picture first "Uploading your picture to the network" | any, with `__dwFixture.hold("dashpay_update_profile")` or `&delay=` (D22) |
| E6 | saved | the sheet closes; toast "Your profile was saved."; the header shows the new profile | any |
| E7 | save refused | the edits as they were + the refusal's text (D9) under the buttons; Save again | `dashpay-profile-save-error`, `&fail=dashpay_update_profile:…` |
| E8 | `profile.conflict` | E7 with "Your profile was changed on another device since you opened it. Save again to replace it with your changes, or cancel to keep the new one." (D10) | `&fail=dashpay_update_profile:profileConflict` |
| E9 | "Load contact pictures" off | the picture sources replaced by "Profile pictures are off. Turn on “Load contact pictures” in Settings to choose a new picture." + Open Settings; Remove picture and Undo stay (D13) | `dashpay-pictures-off` |
| E10 | no upload host (B1) | Choose File… disabled with "Pictures from this computer need an upload service this version does not have yet. Use a public URL or Gravatar." (D5) | `dashpay-registered` |

**Picture steps** (same sheet, `data-step`):

| # | Step | Shows | Actions |
|---|---|---|---|
| U1 | `url` | "Public URL" / "You can specify any URL which is publicly available on the internet so users can see it on the Dash network." + the field "Paste your image URL" | Back, Fetch Image |
| U2 | `url`, refused | U1 + the refusal (D9): "Unable to fetch image. Please enter a valid image URL.", "Use a PNG, JPEG or GIF picture.", "This picture is too large. Choose one under 5 MB.", "Enter the picture's https:// address." | as U1 |
| U3 | `url`, fetching | "Fetching Image" with a spinner; fields held | — |
| G1 | `gravatar` | "Gravatar" / "Enter your Gravatar Email ID" (field, placeholder "sample@email.com") / Android's `gravatar_disclaimer` | Back, Fetch Gravatar |
| G2 | `gravatar`, refused | G1 + "Unable to fetch your Gravatar. Please enter a valid Gravatar email ID." | as G1 |
| K1 | `crop` | "Move and zoom your photo to find the perfect fit" (Android `image_cropper_title`), the square frame over the picture, Zoom slider; drag or arrow keys move it, + / − zoom | Back, Use Photo |
| K2 | `crop`, preparing | "Preparing your picture…"; held | — |
| K3 | a file refused before or by the engine | back at the form with the refusal under the picture: "Use a PNG, JPEG or GIF picture.", "This picture is too large. Choose one under 5 MB.", "This picture could not be read." | — |

**Discard question** (Q1, `edit-profile-discard`): "Discard your changes?" / "You made changes to your profile that are not
saved." [Keep Editing] [Discard] (D11).

**Avatars** (`data-picture` on the avatar when a picture shows; D12): every list row and pane below shows the
identity's picture, else its initials; with the toggle off, initials only, nothing read (D13).

| # | Where | Size | Scenario |
|---|---|---|---|
| A1 | Contacts: the list (contacts, requests, hidden), the Usernames search rows, the selected contact's pane | row 128 px / pane 256 px | `dashpay-avatars` |
| A2 | Add Contact: search rows and the candidate | row / pane | `dashpay-avatars` |
| A3 | Activity and Home's recent rows; the transaction detail's contact | row | `dashpay-avatars` |
| A4 | The frequent strip (Pay ▸ To a contact, Home), the pay-to-contact picker and its confirm sheet | pane / row | `dashpay-avatars` |
| A5 | Notifications page and the bell (requests, events) | row | `dashpay-avatars` |
| A6 | The identity chip, My Profile's header, Edit Profile's picture | small / large / large (own: shown from the cache with the toggle off, DP4-02 D17) | `dashpay-avatars` |
| A7 | A thumbnail that does not load | the initials (`onError`, DP4-02 F9) | `dashpay-avatars` (Erin's thumbnail is not a PNG) |
| A8 | Toggle off | initials everywhere but the own identity's | `dashpay-pictures-off` |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Edit Profile is a sheet over My Profile (`edit-profile-sheet`), opened by its header's Edit Profile (DP1-10 D4's disabled button, now live). Its state is a model per wallet (`editProfileFor`, as `topUpFor`), kept while the app runs: a save keeps running if the page is left, and the sheet is there with its edits and outcome when My Profile shows again. | iOS and Android open Edit Profile from the profile screen as a full-screen form; a desktop sheet keeps My Profile behind it (U-03 D22's sheet layer). A page-bound state would lose a running save's outcome (DP1-10 D14's reasoning for top-ups). |
| D2 | The picture's sources are Choose File… (and a dropped or pasted image), Public URL and Gravatar, in that order, plus Remove picture when the profile has one. No camera. | F17: "file, drop, clipboard, URL, Gravatar; crop"; a desktop has no camera flow in scope (DASHPAY §5). |
| D3 | dw-app gains `FilePurpose::AvatarImage` ("Choose a Profile Picture", PNG, JPEG, GIF, refused unread above 5 MiB, DP4-02's `File` limit), so Choose File… uses the native dialog as Tools' files do (U-05 D2). No contract change: `FilePurpose` is dw-app's, not the engine's; the generated `FilePurpose.ts` follows. | The renderer never opens a dialog or names a path itself (U-05 D2); `qrImage`'s title and filter are the QR scan's. The brief's "smallest binding in dw-app". |
| D4 | The crop is square, made in the renderer, for files only: the picture fills a 240 px frame (zoom 1× = its shorter side), the user pans (drag, arrow keys by 10 % of the frame) and zooms (1–4×, slider, + / −); the result is a pixel rectangle of the oriented picture (the browser applies JPEG's EXIF orientation as the engine does), sent as `prepare_avatar(File{bytes, crop})`. The engine re-encodes the cropped region (≤ 512 px PNG, no metadata, DP4-02 D18); only that is uploaded and its hash published, never the original. A URL or Gravatar picture is not cropped: the engine centres a square, as it does for every thumbnail. | The brief: "square, client-side, the result goes through DP4-02's re-encoding (never upload the uncropped original)". The renderer has no full-size remote picture (only the engine's 256 px thumbnail), so it cannot crop one; Android's `dashpay-profile-pic-zoom` is read (DP4-02 D15) but not written here (F1). |
| D5 | With `avatar_upload_available` false (every build until B1), Choose File… is disabled with a reason (E10), and a dropped or pasted picture is ignored, the E10 line already saying why; URL and Gravatar work. With it true (the fixture's `dashpay-avatar-upload`), Save uploads the candidate first ("Uploading your picture to the network"), then publishes it; an uploaded candidate keeps its URL, so a retry after a refused publish does not upload again. | DP4-02 D2: a `File` candidate cannot be published without an upload. Offering a crop whose result can never be saved would waste the user's work. The flow is built and tested now so F1 (Imgur) only flips the flag. |
| D6 | The baseline is the main identity's profile when the sheet opens (`IdentitySummary.profile`, which is `dashpay_profile`'s answer, DP4-01), sent as the edit's `baseline`. A Platform signal while the sheet is open does not move it. | DP4-01 D23: an edit is made against what the user saw; a change elsewhere meanwhile is `profile.conflict`, not silently overwritten. |
| D7 | The limits are `profile_limits` (the contract's 25 / 140); until it answers, or if it fails, the built-in 25 / 140 (DP4-01 D3's own fallback). Lengths are counted as the engine counts them: Unicode scalar values of the trimmed text (`Array.from`). The counter is Android's "N/25 characters"; over the limit Save is held (E3). An empty field clears it. | DP4-01: the engine trims and counts scalar values; counting UTF-16 units would let "😀" × 13 pass the UI and fail the engine. |
| D8 | Save is enabled only when the trimmed fields or the picture differ from the baseline, no field is over its limit, the result would not be empty (no name, no message, no picture: "A profile needs a display name, an about text or a picture.", the engine's `profile.empty`) and nothing is preparing. One save at a time: Save is disabled and the sheet held while it runs, and a second call is ignored (E5). The grant is sized by `grant_request(updateProfile)` and asked through `GrantModel`; a grant the call did not use (nothing to do, a refusal) is revoked. | The brief: "a save in flight cannot be double-submitted". DP2-09 D3's grant pattern. |
| D9 | A refused save keeps every edit and shows the refusal under the buttons (E7): `profile.too_long` "The display name can be at most N characters." / "About me can be at most N characters."; `profile.empty` (D8); `profile.conflict` (D10); `profile.unsupported_contract` "This version cannot edit profiles on the network's DashPay contract. Update Dash Wallet."; `platform.insufficient_credits` "Not enough credits. Top up your credits and try again."; `platform.unavailable` / `platform.timeout` "Could not reach Dash Platform. Try again."; `vault.locked` / `platform.signer_unavailable` "Unlock the wallet and try again."; `platform.context_unavailable` "Waiting for the network to sync and verify this identity. Try again shortly."; `platform.trust_mismatch` "This identity's data could not be verified against the network. Nothing was changed."; `platform.grant_exceeded` "The cost went above what you approved."; the engine's two refusals of a new picture's candidate, from the upload or the publish (`invalid_argument` whose text ends "unknown avatar candidate" or "the avatar candidate has no URL; upload it first", `CANDIDATE_GONE`), "The picture you chose is no longer ready. Choose it again." with the choice back to the published picture, so Save cannot repeat it (candidates live 30 minutes, DP4-02); any other `invalid_argument` (a watch-only identity, say) keeps the choice and shows the engine's text; `avatar.upload_unconfigured` "Uploading pictures is not available in this version."; any other upload refusal Android's "Unable to upload your picture. Please try again."; any other code, the engine's message. A cancelled passphrase changes nothing and says nothing. | The brief: "a failed save keeps the edits and shows the engine's error text". Codes with a known cause get the mobile-style copy; the rest show the engine's own message, which for these calls never quotes a secret. |
| D10 | `profile.conflict{current}` makes `current` the new baseline and keeps the user's fields; Save again replaces the profile with them, Cancel (then Discard) keeps the other device's. When `current` already equals the edit, nothing is left to save: Save is disabled and the line says "…, and it now matches your changes." instead. | DP4-01 D23 hands the current profile back for this. Replacing the form with it would throw the user's edits away (the brief: a failed save keeps them). |
| D11 | Unsaved changes: closing the sheet (×, Esc, the scrim, Cancel) with changes asks "Discard your changes?" (Android `save_profile_reminder_text`, as a question with Keep Editing and Discard; the question has no Save, since a save needs the grant and can be refused). Leaving My Profile while the sheet has changes (a menu command, a shortcut, a link) asks the same through a leave guard on `navigate` (`setLeaveGuard` in ui.ts): Discard drops the edits and goes, Keep Editing stays, with focus back on the step's first field (WebKit leaves no focused opener after a clicked button). A lock keeps the sheet and its edits under the lock screen. While a save runs nothing is asked: it goes on in the model (D1); a leave then cancels the passphrase prompt asked for now or later (the grant's epoch is taken as the save starts, before its upload), so no prompt waits unseen and the save ends with its edits kept, unless its publish was already sent. | The brief: "leaving with edits asks to discard". The shell is inert under a sheet, so menus, shortcuts and links are the only ways out; one guard covers all three. |
| D12 | One avatar: `Avatar` takes the identity, reads `dashpay_avatar(identity, small)` for rows and the chip, `large` for panes and My Profile, and shows the initials while it reads, without a picture, on any error (`hash_mismatch` included, DP4-02 D7) and when the PNG does not load. The window's wallet and the toggle come from a `PictureScope` context App provides; outside it (or before the wallet is known) only initials. Every list DP2/DP3 built uses it (A1–A6). | The brief: "Reuse one avatar component with the placeholder (initials/colour) rule". The engine single-flights a URL and caches thumbnails (DP4-02 D12, D35), so one read per row is cheap; a context keeps the wallet out of every list's props. |
| D13 | "Load contact pictures" off: no other user's avatar is read at all (the query is not enabled), so nothing is fetched and no Gravatar address is hashed; lists show initials. The own identity's picture is still read (the engine serves it from its cache and never fetches it while off, DP4-02 D17). Edit Profile offers no URL, Gravatar or file source while off (E9): each would fetch or hash a picture. While the setting is unread or unreadable it counts as off. The fixture counts `dashpay_avatar` per identity, and the D13 spec walks Home, Activity, Contacts and a contact's pane, Pay and the bell with the switch off and finds no read of another identity (the engine answering `null` alone would pass a spec that only looks at the screen). | The brief: avatars off means "no avatar fetch, no Gravatar hash, placeholders only" everywhere. The engine already returns `None` for others while off; not asking at all also keeps a stale cached picture off the screen. Unreadable counts as off as the engine's D22 does. |
| D14 | The Settings row is in Settings ▸ General ▸ Privacy, below "Include my username in payment requests": "Load contact pictures", on by default, for the window's network. A change writes it, then resets every avatar read (`resetQueries(["dashpay","avatar"])`): cached pictures go at once on Off, and are read again on On. | DP4-02 F5 and F9. The engine's setting is per network (D17), so the row is not per wallet. |
| D15 | A Gravatar address is held only in the step's field: it goes to `prepare_avatar` and is cleared from the model when the call starts, never kept in a query key, the model or a log. | Android's disclaimer, shown on the step: "Your Email is not stored … then discarded." |
| D16 | A file is checked before it is shown: at most 5 MiB (else "This picture is too large. Choose one under 5 MB.") and PNG, JPEG or GIF by its first bytes (else "Use a PNG, JPEG or GIF picture."). The crop step shows the user's own file as a `data:` URL (the CSP allows `data:` images; no `blob:`); remote pictures still reach the renderer only as the engine's re-encoded PNGs. A file the webview cannot draw is "This picture could not be read." One the webview draws wider or higher than DP4-02's 4096 pixels is refused before the crop with "…at most 4096 pixels wide and high." (the engine would refuse it at Use Photo as `avatar.too_large`, worded for bytes). | DP4-02 D8's formats and limit, refused before reading as the QR scan does (DP2-08 D35). The CSP is not widened. |
| D17 | Saved: the sheet closes and a toast says "Your profile was saved."; My Profile's header follows the `Platform{Identities}` signal. The own avatar reads again under that signal (its key is under `["dashpay"]`). | Android returns to the profile after "Updating Profile on Dash Network". |
| D18 | The header's "Edit Profile" is enabled whenever My Profile shows a main identity (P4–P6); the engine refuses what it must (keys, credits, D9). | DP1-10 D4 kept the place for it. |
| D19 | The fixture models the engine's checks in its order (trim, `too_long`, `empty`, baseline `conflict`, credits, the candidate's URL) and its picture sources by rule: an `https://` URL containing `missing` fails to fetch, `.webp` is unsupported, `huge` is too large; a Gravatar address `nobody@…` has none; only `https://` URLs are fetched; a file is sniffed and its size checked, and a PNG's or GIF's crop must lie inside it (a JPEG's is unchecked: its size as the engine crops it needs the EXIF orientation, which a WebKit and Chromium spec checks in the browser instead, `ROTATED_JPEG`); every prepare is recorded (the crop included) for the specs. It renders no picture: a candidate's preview is a fixed PNG per source. | The renderer's behaviour is what T0 tests; the decoder and the guard are DP4-02's (T0 + T2). |
| D20 | Test titles cite IOS-076 (Edit Profile) and Android's AND-095 (profile, counters, the unsaved-changes prompt), AND-096 (picture sources, crop) and AND-097 (a shared image, here a drop or paste), as DASHPAY F17 maps them (`docs/parity.md` holds IOS-076; the AND rows are `docs/research/dashpay/android.md`'s); the avatar specs cite F15 (IOS-136, AND-031). | The brief named no IDs. |
| D21 | Visual baselines (`edit-profile-…`): E2 (`form`), E3 with E4 (`changed`), E4 with a new picture (`new-picture`), E7 (`error`), E9 (`pictures-off`), U2 (`url`), G1 (`gravatar`), K1 (`crop`), Q1 (`discard`); and the lists with pictures (`avatars-…`): Home (chip and frequent strip), Activity, Contacts with the pane, Pay ▸ To a contact, the bell, Notifications, in light and dark. DP1-10's profile baselines and Settings ▸ General's are updated: the header's button is live (D18) and Privacy gains a row (D14); `bell` and `notifications` too, as their event rows now carry the contact's avatar (initials in `dashpay-registered`). | New screens and every list with avatars, per the brief; the rest is copy, asserted with axe. |
| D22 | A slow save and a refused save are not scenarios of their own: a slow save is the fixture's `hold("dashpay_update_profile")` (or `&delay=`), as the other sheets' in-flight states are; a refused save is `dashpay-profile-save-error` (the first save fails with `platform.unavailable`) or `&fail=dashpay_update_profile:<class>`. | The brief's "slow save" needs the save held while the spec looks; a hold does that without a timer. |
| D23 | Edit Profile's textarea is styled under `.edit-profile-field` only: a `.form-field textarea` rule restyled Tools ▸ Sign/Verify's and the restore phrase's (their baselines moved). | Shared UI files minimal (brief). |
| D24 | Accepted, from the review: wallet A's open sheet is not asked about when the window turns to another wallet through Add Wallet's onboarding (not a `navigate`); the edits stay in A's model (D1) and show when A's My Profile opens again. | Nothing is lost; the guard covers the ways out of the page. |
| D25 | Accepted, from the review: reopening Edit Profile in the moment between a save and the `Platform{Identities}` refetch takes the old profile as its baseline; a save then says the profile changed elsewhere (D10) and the next one goes through. | The window is one event's round trip; the conflict path is the engine's own and safe. |
| D26 | The candidate's two refusals (D9) are told from other `invalid_argument`s by the end of the engine's text, kept in one constant the fixture also throws (`CANDIDATE_GONE`), not by a new typed `kind`. | Review r1 Minor 4. A typed kind means new `ErrorData` in dw-app and a contract version for one UI line; the texts are DP4-02's and a test pins each. F8 turns them into a kind when the error contract next moves. |
| D27 | BL-190 (manager r1): every `avatarFingerprint` this wallet publishes (a URL or Gravatar picture now, an uploaded file at B1) is the mobile wallets' dHash, ported from dash-wallet v11.9.1's `CocoaImageDHash` (`avatar_dhash.rs`): the bytes as decoded, EXIF orientation not applied, scaled to 9 × 9 as Android's Skia `SkBitmapProcState` filters (16.16 taps, 4-bit bilinear weights on premultiplied colour, truncated), grey `(r + g + b) / 3` over black, bit `x` of byte `y` set where a cell is brighter than the next. The read keeps the library's hash and does not compare the fingerprint (DP4-02 D32). Vectors: the 53 byte-exact testnet avatars (`tests/fixtures/bl190_avatar_fingerprints.json`, the grid's pixels of each). **38/53 equal bit for bit** from the engine's decoder. The 15 others: 4 Android JPEGs equal from libjpeg-turbo's pixels (zune-jpeg rounds some taps up to 2 levels apart; 1–3 bits from ours); 1 Android JPEG equal from libjpeg's fast IDCT, which older Android used (3 bits); 9 published with the library's dHash (2025-09 on, platform-wallet clients such as iOS 9.x; 12–35 bits); 1 a Gravatar robohash PNG from another client whose dHash is neither (13 bits). So all 43 Android-set avatars are Android's algorithm, 38 from our pixels. | The pinned contract requires a fingerprint with a picture, so one is published either way; the mobile one is what Android, the most common publisher, wrote. Found while porting: no current reader compares it (Android only logs a mismatch, platform-wallet and SwiftDashSDK and DashSync do not compare), and iOS 9.1.3 publishes the library's dHash of its re-encoded JPEG, so testnet holds both; DIP-15's "should compare Hamming distance" is read by no client. The 5 decoder exceptions would need libjpeg-turbo's IDCT in the engine for a few bits on pictures Android decoded, and our own pictures are hashed from the bytes we decode, so they get no D-row of their own beyond this one. |

## Slice plan

1. This document.
2. dw-app's `AvatarImage` file purpose and the generated type.
3. `Backend` bindings; the picture scope and `Avatar`; every list's call site; the Settings row.
4. The edit model and crop geometry with their tests.
5. The sheet, the leave guard, My Profile's button; styles.
6. The fixture's handlers and scenarios with tests.
7. Playwright flows + axe; visual baselines; `docs/parity.md`.

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Writing Android's `dashpay-profile-pic-zoom` for a cropped URL picture (needs the full picture in the renderer, or an engine crop preview) | later, with F2 |
| F2 | Imgur uploads (B1): `avatar_upload_available` turns Choose File… on with no UI change (D5) | DP4-02 F1 |
| F3 | Done in this task (D27): the published `avatarFingerprint` is the mobile wallets' dHash | BL-190, done |
| F4 | An avatar in the registration's initial profile | DP4-02 F6 |
| F5 | Avatar reads sit under `["dashpay", …]` (DP4-02), so every DashPay event re-reads each avatar on screen (from the engine's cache): key them by the profile's `avatarHash`, or invalidate them on `identities` and `contacts` changes only | DP4-02 follow-up |
| F6 | Smaller review notes: Esc in a picture step could go Back rather than close; paste works only with focus in the picture block; a removed wallet's model stays in memory until restart | later |
| F7 | Before Choose File… is turned on (F2, B1), from review r1: (a) WebKit draws a PNG with an `eXIf` orientation unrotated while the engine applies it, so the framed square is not the one cut (and for orientations 5–8 is often refused): crop on an engine-oriented preview, or strip or refuse PNG orientation on both sides; the fixture's PNG crop check models WebKit, not the engine; (b) the webview still decodes whatever size a 5 MB file declares before D16's 4096-pixel check can refuse it: read the size from the header (PNG IHDR, GIF screen, JPEG SOF) first | must land before B1 |
| F8 | A typed `ErrorData` kind for the candidate's refusals (D26) | with the next error-contract change |
