# DP3-03b: "Include my username in payment requests"

The follow-up of DP3-02 D21, DP3-06 F3 and DP3-07 F3/D9. Since DP3-03, every receive request and the
Receive QR carry `user=<main name>` when the wallet has a main name (iOS IOS-055). The manager's
ruling (DP3-02 D21) keeps that as the default and adds a Settings privacy toggle, **"Include my
username in payment requests"**, on by default. While it is off, `receive_username` returns `None`,
and `create_receive_request` / `receive_requests` leave `username` out and build the plain dash-qt
URI.

| Part | Where |
|---|---|
| The setting and its calls | dw-engine `receive.rs`: `username_in_requests`, `set_username_in_requests`, read by `username_of` |
| Contract | m1 §2.6 (`docs/contracts/m1-engine.md`); no §4 change |
| Bindings | dw-app `api/wallets.rs` (`usernameInRequests`, `setUsernameInRequests`), `capabilities/wallets.json`, generated `commands.ts`, `docs/design/E0-13-facade-coverage.md` |
| UI | `src/screens/settings/Privacy.tsx` (the card, in Settings ▸ General); `src/screens/Receive.tsx` (D8) |
| Fixture | `ReceiveWorld.usernameInRequests` (`src/fixture/send.ts`), scenario `dashpay-username-off` |
| Tests | dw-engine `platform/users_tests.rs` (`the_username_opt_out_gives_plain_core_uris`), `tests/e1_offline.rs` (`username_in_requests_survives_a_restart`); vitest `src/fixture/send.test.ts`; Playwright `e2e/username-optout.spec.ts`; visual `settings-general` |

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The setting persists in the network's app.sqlite, in `settings_kv` under the **wallet's** scope (the wallet id), key `dashpay.username_in_requests`: absent (the default) is on, `"0"` is off (the engine writes nothing else; D6). Being in the network's file, the same seed on testnet and mainnet has two settings, as every other wallet setting. dw-appdb's wallet removal deletes the wallet's settings scope, and `.dwbackup` export and restore carry its scoped rows, so the choice goes with the wallet both ways. | The ruling makes it per wallet: the username is the main name of *this wallet's* main identity. The other per-wallet engine settings (`bip44.lookahead`, `coinjoin.salt`, `watch_only_xpub`) live there too. |
| D2 | Not in `dp_prefs(wallet_id, identity, key, value)`, and not in the UI's `settings.json`. | `dp_prefs` is per identity: choosing another main identity (DP1-05) would quietly bring back a choice the user made against another one. `settings.json` is the renderer's file: the engine builds every request URI, for dw-app, dwcli and dw-ffi alike, so only an engine setting holds for every host. |
| D3 | The flag reaches the engine as the engine's own state, not as a parameter: `username_of`, the one helper behind `receive_username`, `create_receive_request` and `receive_requests`, reads the setting first and answers `None` while it is off. Two new m1 calls read and write it: `username_in_requests(id) -> bool` and `set_username_in_requests(id, on)`, both `wallet_not_found` for an unknown wallet. No existing m1 signature changes. | A parameter on the three calls would let one caller forget it and print the name anyway; DP3-02 D21 left the flag unthreaded because those calls are shared with dw-ffi. A stored setting needs no signature change at all. |
| D4 | **dw-ffi is still frozen.** E0-13 merged with "UniFFI `dw-ffi` stays frozen and gains nothing" (E0-13-bindings.md), and nothing has unfrozen it since. It gets no binding for the two calls. With D3 it needs none: its `create_receive_request` / `receive_requests` call the same engine methods, so the Swift shells follow the setting (on, unless a Tauri window turned it off for that wallet). | The ROADMAP keeps the Swift shells frozen for the Tauri stack; D3 keeps them correct without touching them. |
| D5 | Stored requests follow the toggle. `ReceiveRequest.username` (and so `uri`) is computed when a request is read (DP3-02 D18), so turning the setting off removes `user=` from every stored request's next read, and turning it back on restores it. Nothing is rewritten in storage. | D18 already stores no username; the toggle is one more input to the same read. The request sheet lives on the Receive page, which Settings replaces, so no sheet stays open across a change. |
| D6 | A setting that cannot be read, or holds a value the engine never writes (a hand edit, a foreign `.dwbackup` row), counts as **off** for the URIs (`username_of` answers `None`, never an error, as DP3-03 D19), while `username_in_requests` itself returns `storage` (dw-appdb `Corrupt` for a bad value, as `parse_threshold` does), which the card shows above the box. The box then shows off (what the URIs do), even if an earlier read is still cached, and Receive reads it as off too (D8) and stays clickable: a click writes a value the engine reads, and the refetch clears the problem. | Fail closed for privacy: a broken store must not print a name the user may have turned off. A plain Core URI is always a valid request. Unlike "require authentication for every payment", where an unknown value reads as the strict "on", here the strict reading is "off" (Opus r1 #5). |
| D7 | No engine event for the change. The setting changes only through `set_username_in_requests`. Turning it off drops a read of the setting already under way and writes `false` into the renderer's cached setting at once, before the call; once the call answers (or is refused), the renderer refetches `keys.receiveAll(wallet)` (the setting, the Receive username and the requests) with `refetchType: "all"`, and the box settles only after that. A refusal's refetch puts the engine's value back. Turning it on waits for the engine. | One window writes it; an `EngineEvent` variant would reach dw-ffi's frozen `forward()` and the bridge's byte-bound match for no reader. Receive is unmounted while Settings shows, so a plain invalidation would only mark its reads stale, and going back would draw the cached `user=alice` while they refetched; a request opened in that moment would keep it (Opus r1 #1). The early `false` covers leaving Settings before the engine answers (Opus r2 L1). |
| D8 | Receive reads `usernameInRequests` itself. While it reads `false` the QR has no username, whatever a cached `receive_username` holds; and the held name (DP3-02 D25: while names update the engine reads `None`, so the card keeps the last name it showed) applies only while it reads `true`. Off, unknown or failed, the engine's `None` is the answer. | Without it, a Receive page open while the setting went off (or a read that later fails) would keep showing `user=`, and a stale cached username would too (D7 is the first guard, this the second). `e2e/username-optout.spec.ts` turns the fixture's flag off under an open Receive page, and holds `receive_username` on the way back from Settings; both check the link has no name. |
| D9 | The toggle is a **Privacy** card in Settings ▸ General, after Dust attack protection: one checkbox, "Include my username in payment requests", with a hint naming the wallet and what it shares. It is for the window's wallet, picked as App.tsx picks it (`ui.walletId`, else the first). It shows for every wallet, with or without a username yet (the setting applies once there is one), watch-only too. The component is its own file; General gains one import and one line. | The ruling names Settings; General already holds the engine's per-network options. Siblings in flight (U-06 CoinJoin settings, DP1-10 My Profile, MG-02) touch Settings too, so the edit stays additive. |
| D10 | The box shows the clicked choice until the engine answers and takes one save at a time (U-04 D36). A refused save shows the failure under the card and the box returns to the engine's value. | U-04's pattern for engine-backed checkboxes; it is what made `settings-dust-on` stable under load. |
| D11 | "Off is byte-identical to dash-qt's `formatBitcoinURI`": the engine test compares every request with dw-uri's `format_bitcoin_uri` (the port of `GUIUtil::formatBitcoinURI`, checked against `testdata/uri_cases.json`, which dash-qt's code produced) and with a literal whose label and message need percent-encoding (`label=Rent%20%26%20co&message=May%2FJune`). | Comparing with the port alone could hide a shared mistake; the literal is dash-qt's `QUrl::toPercentEncoding` output. |
| D12 | The m1 contract changes in §2.6 only (two rows); no §4 error code or table changes, so the Swift T0 suite is not required by the brief (it is a §4-only rule there). | dw-common: only a §4 change needs Swift T0; dw-ffi gains nothing (D4). |
| D13 | The restart test is the engine's (`e1_offline`: off, restart, still off; the other wallet unaffected; a removed and re-imported wallet starts on). Its wallets have no DashPay identity, so it reads the setting and makes no URI claim; the URIs are `users_tests`'. The fixture has no restart, and no T1 (e2e-tauri) script was extended. | The persistence is the engine's; a renderer restart would only re-read it. |
| D14 | dwcli gains no command. | Not in the row; dwcli has no request or URI command (it prints addresses only), so nothing there carries `user=`. |
| D15 | Receive's QR query key is `["receiveUri", address, user]`, no longer `["paymentUri", …]`, which Tools ▸ Address book also uses with a label in the user's place. | A receiving address-book entry labelled with the username shared the cache entry of Receive's `?user=` URI (`staleTime: Infinity`), so either screen could draw the other's URI (Opus r1 #7). |
| D16 | Visual baseline: `settings-general`, light and dark, regenerated; the card sits inside its viewport, under Dust attack protection. No other baseline changes. | Brief: a baseline for the Settings row. |
| D17 | A `walletCreated` or `walletRemoved` event resets the wallet's receive reads (`resetQueries` on `keys.receiveAll`): their data is dropped and mounted ones refetch. | A restore can bring back a wallet whose backup holds "off", and a re-import starts on (D1): a cached value from before would draw the name, or show the box in the wrong state, until a refetch (Opus r2 L3). |
| D18 | Receive's QR keeps the previous link as a placeholder while a new one is built only when it is for the same address and the new link has a name (not while the name is still being read) or the old one had none: a link without a name never shows one with it, even for a moment. | Copy link is enabled on the placeholder (Opus r2 I1). |

## DASHPAY.md text

Added with this task:

- §5 F15, "Ours": "… request URIs carry the username, unless the wallet's "Include my username in
  payment requests" (Settings ▸ General ▸ Privacy, default on) is off; then they are dash-qt's plain
  URIs (DP3-03b)."
- §3.4 `app.sqlite`: "the wallet-scoped setting `dashpay.username_in_requests` in `settings_kv`
  ("Include my username in payment requests", F15; absent = on)".

## Tests by claim

| Claim | Test |
|---|---|
| On by default, requests and the Receive QR carry `user=` | `users_tests::the_username_opt_out_gives_plain_core_uris`, `e1_offline::username_in_requests_survives_a_restart` (the setting reads on), `send.test.ts`, `username-optout.spec.ts` (light and dark, Chromium and WebKit) |
| Off gives the plain URI byte-identical to dash-qt's `formatBitcoinURI` | `users_tests::the_username_opt_out_gives_plain_core_uris` (D11), `send.test.ts` |
| Stored requests follow the toggle, both ways | `users_tests::the_username_opt_out_gives_plain_core_uris`, `send.test.ts`, `username-optout.spec.ts` |
| The toggle survives a restart, per wallet; removal resets it | `e1_offline::username_in_requests_survives_a_restart` |
| Receive drops a held or cached name when the setting reads off (D7, D8), also while the save is still out | `username-optout.spec.ts` |
| An unreadable setting shows off and can be written again (D6) | `username-optout.spec.ts` |
| A link without a name never shows the named one while it is built (D18) | `username-optout.spec.ts` |
| A wallet that comes or goes drops its cached receive reads (D17) | `src/api/events.test.ts` |
| A stored value the engine never writes fails closed (D6) | `users_tests::the_username_opt_out_gives_plain_core_uris` |
| Pending choice, one save at a time, a refusal keeps the setting (D10) | `username-optout.spec.ts` |
| The Settings row's look | `visual.spec.ts` `settings-general` light and dark (baselines regenerated, D16) |

## Follow-ups

| # | Follow-up | Owner |
|---|---|---|
| F1 | The Swift shells have no toggle (dw-ffi frozen, D4); they follow the engine's value. | if dw-ffi unfreezes |
| F2 | The fixture's `paymentUri` trims an amount's zeros (`amount=1.5`), where dash-qt and dw-uri write eight decimals (`amount=1.50000000`). Existing specs assert the trimmed form, so it was left; the engine tests hold the dash-qt form. | backlog (fixture) |
