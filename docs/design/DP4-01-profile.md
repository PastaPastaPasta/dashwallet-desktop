# DP4-01: profile read and update

ROADMAP row DP4-01 (S, engine, T2): "Profile read and update (limits from the contract: 25 / 140)". Acceptance: a
contact sees the change within 2 passes. The UI is DP1-10 and DP4-03; the avatar pipeline (`prepare_avatar`,
`upload_avatar`, `avatar`, `avatar_upload_available`) is DP4-02. This task gives `profile`, `profile_limits` and
`update_profile` their bodies and makes a contact's profile change reach the contacts within one DashPay pass.

The contract is m4-dashpay-engine.md §2.7 (the three calls), §4 (two new codes), §5 (`ProfileEdit`), DASHPAY §3.3
("profile edit" prompts), §3.5 (signals) and §3.6. No call's signature changes. `PlatformError` gains the nested
`Profile(ProfileError)` (as `Identity(IdentityError)`), so the surface moves to version 24 (§7 item 50).

| Part | Where |
|---|---|
| The facade calls | `dw-engine/src/platform/profile.rs` |
| The limits, the edit's validation, the write (document replace, refusals, read-back), the reads and the contact-profile freshness | `dw-engine/src/platform/profiles.rs` |
| Platform seam: read the profile document, put it | `RequestNet` (`requests_net.rs`): `profile_document`, `put_profile` |
| A document write's grant, permit and key without the contact crypto | `requests.rs` (`prepare_signer`; `FlowKind::ProfileEdit` in `flow_levels`/`flow_keys`) |
| The codes | `errors.rs` (`ProfileError`), `dw-app/src/error/platform.rs`, generated `ProfileCode.ts`/`ErrorData.ts` |
| The tap's hook that expires contact profiles | `journal.rs` (`Classified.contact_profiles_checked`, `ChangesetTap.on_contact_profiles`), `session.rs` |
| dwcli | `rust/bin/dwcli/src/dashpay.rs`: `profile get` (alias of `show`), `profile set`, `profile limits` |
| Tests | `dw-engine/src/platform/profile_tests.rs` (on requests_tests.rs's fixture); T2 on testnet with dwcli (the task's evidence) |
| Contract | m4 §2.7, §4, §5 `ProfileEdit`, §7 item 50 (version 24) |

## Reads

`profile(identity)` is sync and reads memory only, like `identities()` and `contact()`:

1. one of the wallet's identities: its profile from the identity read (`identity_read`: the library's memory, or the
   snapshot the last read kept while a sync pass holds the lock, DEC-138), which is what `IdentitySummary.profile`
   shows;
2. else a user one of the wallet's identities holds a profile for (a contact, a pending sender): the contacts read
   model's entry (DP2-01, `read_contacts`), looked up under the main identity first, then the others by index;
3. else `None`.

A stale read without any snapshot to answer from (no read has ever completed) is `platform.unavailable`, as
`contact()` answers; re-query on the `Platform{…}` signal that follows. An id that is not Base58 is
`invalid_argument`; the session and wallet checks are the other reads'.

**Signals.** No new change kind. An own profile written here or fetched by the library's pass, and a contact's profile
the pass fetched, are stored as an `identities` changeset (the library's `IdentityEntry` carries `dashpay_profile`
and `contact_profiles`), which the tap already signals as `Platform{Identities}` (§3.5). The contacts model rebuilds
its snapshot after the writer and signals `Platform{Contacts}`. Hosts re-query `profile` on both.

## Limits

`profile_limits()` reads `displayName.maxLength` and `publicMessage.maxLength` from the `profile` document type of
the DashPay system contract the library bundles (`load_system_data_contract(Dashpay, PlatformVersion::latest())`,
the copy `dashpay_contract()` already parses for registration). At the pin both contract versions (v1, v2) say
25 / 140. If either property lacked a `maxLength` the call would answer the built-in 25 / 140 and log it (D3). It
reads no wallet and never the network, like `cost_table`.

## Update

`update_profile(identity, edit, grant)` publishes the whole profile the edit describes: `display_name` and
`public_message` as given (`None` clears), the avatar kept, removed or (DP4-02) set. Steps, each refusing before the
grant is redeemed where it can:

1. **Validate** (no I/O). Each text is trimmed; an empty one is `None`. A text longer than its limit, counted in
   Unicode scalar values as the contract's `maxLength` is, is `profile.too_long{field, length, max}`.
   `AvatarChange::Set` is `invalid_argument` until DP4-02 (D6).
2. **The identity.** One of the wallet's (else `identity.not_found`), with an index (a watched identity cannot sign:
   `invalid_argument`). The identity's write lock is taken here and held to the end (`identity_writes.rs`), so no
   other write of the identity runs meanwhile; the spent balance is refreshed after it (`after_spend`).
3. **Read Platform's document** (`$ownerId = identity`, limit 1). A failed read fails the call.
4. **The new properties**: Platform's, then `displayName` and `publicMessage` set or removed, and on `Remove` the
   three avatar fields removed together (the contract makes each require the other two). Every field the edit does
   not own stays as it is: the avatar on `Keep`, and v2's `corePaymentAddress`, `platformPaymentAddress` and
   `shieldedAddress` (D2).
5. **Nothing to do**: the same as Platform's (or no document and nothing to set) is `Ok`, nothing sent, the grant
   unused, whatever the edit's baseline. The library's cached profile is brought in line with Platform's on the way.
6. **Changed since it was read** (D23): an edit with a `baseline` whose profile is not what `profile` shows of
   Platform's document now (`updated_at` aside) is `profile.conflict{current}` with that profile, and the cache is
   brought in line with Platform's. Without a baseline the edit replaces whatever Platform holds.
7. **Empty**: a document that would hold no field is `profile.empty` (the contract's `minProperties: 1`, and profiles
   cannot be deleted from here).
8. **The contract** (D21): Platform's DashPay contract is fetched (proved), and a `profile` type with a property the
   bundled contract lacks or types otherwise (or none) is `profile.unsupported_contract`.
9. **Credits and grant**, as every DashPay document write (`requests::prepare_signer`): the identity's proved balance
   below the quote (`DOCUMENT_10K_CREDITS`, `grant_request(UpdateProfile)`'s bound) is
   `platform.insufficient_credits`; a `PlatformOp` grant whose `max_credits` is below it `platform.grant_exceeded`;
   a lease of another flow than `ProfileEdit` `platform.grant_invalid`; no HIGH or CRITICAL authentication key the
   vault holds that may sign a profile now (D22) `identity.keys_missing`, the grant unused. The write runs under a
   call permit charged the quote, signed by that one key.
10. **Put**: a replacement at the next revision when Platform has the document, else a creation (the SDK draws the
   entropy and the id), with `put_to_platform_and_wait_for_response`, the call the library's own profile write
   makes (D1).
11. **Outcome.** Success: the library's cached profile is set to the new one (`set_dashpay_profile`, which persists
   and so signals `Platform{Identities}`). A failure before anything was signed is that error (the permit ends
   `Cancelled`, the quote goes back). A failure after signing is read back: if Platform now holds the new
   properties, the call succeeds as above; otherwise the error, the quote kept (fail closed), as DP2-04.

## A contact sees the change (the acceptance)

The library's DashPay pass fetches the wallet's own profiles every pass, but a contact's profile at most once an
hour (`CONTACT_PROFILE_REFRESH_MS` in `rs-platform-wallet`), keyed by the entry's `checked_at_ms`. A contact would
therefore see an edit up to an hour late, and a fresh process not before the hour since its last session.

The engine expires the cache instead (D12): it sets every contact-profile entry's `checked_at_ms` to 0 in memory, so
the library's next pass fetches them all again (one `$ownerId In [...]` query per own identity, up to 100 contacts
each), applies them with its own full-replace rules (an absent profile is cached as absent, an unsafe avatar URL
dropped), stores them and signals.

- **When.** At session open, for every loaded wallet; when a wallet is opened later (`load_wallet`, whose cache
  comes back from the database with its check times); and after every pass that fetched: the pass stores each owner's
  identity snapshot with the fetched entries' new `checked_at_ms`; the tap sees a stored contact-profile entry with
  a non-zero `checked_at_ms` and hands the wallet to the session's hook, which expires the wallet's entries in a
  task (it cannot take the library's lock inside `store`, which runs under it). The task queues on the library's
  write lock behind the pass's own write, so it lands before the next pass reads its plan (tokio's `RwLock` is
  FIFO). An expired entry stored again by another write carries 0, which triggers nothing, so there is one expiry
  per fetching pass.
- **Bound.** An edit that has landed on Platform before a pass begins its contact-profile step is fetched by that
  pass; one that lands during it, by the next. So the contact sees it within two passes of the write (15 s each
  while a window is visible, 60 s hidden), which T2 measures.
- **Cost** (D12, D17): per pass, one proved profile query per 100 contacts of each own identity, and the library's
  store of each such owner's whole identity entry (one SQLite commit of a blob that carries its payments and contact
  profiles; the library persists after every fetch), so a `Platform{Identities}` signal per pass: about four of
  each a minute while a window is visible, one while hidden. The one-hour window existed to bound exactly this.

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The engine replaces the profile document itself, with the SDK's `put_to_platform_and_wait_for_response` (the call platform-wallet's `sdk_writer.put_document` makes), and not with the library's `update_profile_with_external_signer`. | The library's update merges the edit onto Platform's document and can only set fields, never clear one, while m4 §2.7 and §5 say `ProfileEdit` is the whole profile and `None` clears. The creation path is the same call, so one path serves both. |
| D2 | Fields the edit does not own are carried over from Platform's document: the avatar trio on `Keep`, and the v2 contract's payment-address fields. `Remove` drops `avatarUrl`, `avatarHash` and `avatarFingerprint` together. | A replace that rebuilt the map would erase what another client published (the merge the library added for the same reason). The contract's `dependentRequired` makes the three avatar fields all-or-none. |
| D3 | The limits come from the DashPay contract the library bundles (`load_system_data_contract`, latest), the `maxLength` of `displayName` and `publicMessage`; the built-in 25 / 140 only if a property lacks one (not at this pin; a test asserts the contract gives 25 / 140). No network fetch. | "Limits from the contract" (the row). Both contract versions at the pin agree, and the bundled contract is the one every DashPay write here is built against; m4 §1's "until the contract is fetched" fallback then never applies. |
| D4 | Texts are trimmed (Unicode whitespace); an empty text clears the field; length is counted in Unicode scalar values (`chars().count()`). | DPP's `max_length` counts characters (`StringPropertySizes`), and the contract's `minLength: 1` refuses an empty string, so "" can only mean "clear". |
| D5 | Two new codes, `profile.too_long{field, length, max}` and `profile.empty`, as `PlatformError::Profile(ProfileError)` (the `identity.*` pattern), because `update_profile` returns `PlatformError`. | The row asks for clear codes; `invalid_argument` would leave the UI parsing `detail`. Swift: `ServiceErrorCode.engineCodes` mirrors m1-engine.md §4 only (`SettingsAndCodesTests`), so m4 codes need no Swift change; the Swift T0 suite runs anyway. |
| D6 | `AvatarChange::Set` is `invalid_argument` until DP4-02. | No candidate can exist while `prepare_avatar` is a stub; DP4-02 adds the candidate's URL, hash and fingerprint to step 4. |
| D7 | An edit equal to Platform's document sends nothing and returns `Ok`, the grant unused. | Saves credits; makes a retry after `broadcast_unknown`-like failures harmless. |
| D8 | The refusal order is §"Update" steps 1–7: everything local, then one Platform read, then credits, grant and key, so a refusal before step 8 never redeems the grant. | DP2-02's rule for every write (`requests.rs` header). |
| D9 | A failed signed write is read back from Platform and counts as done when the document holds the new properties. | As DP2-02/DP2-04: platform-wallet flattens the SDK's errors, and a timeout often hides a landed write. |
| D10 | On success the library's cached own profile is set (`ManagedIdentity::set_dashpay_profile`), which persists and signals `Platform{Identities}`; the next pass's `sync_profiles` replaces it with Platform's anyway. The cached value is what the library's own fetch makes of the document (`fetch_contact_profiles_chunk`): an avatar URL that is not `https://` or longer than 2048 dropped, and no profile when no field is left, so the next pass changes nothing. Known flicker: a pass whose own-profile fetch ran before the write landed applies after it and shows the old profile until the next pass (15–60 s). | `profile()` and `identities()` show the edit at once instead of after the next pass. |
| D11 | `profile(identity)` serves own identities from the identity read and other users from the contacts read model (main identity's view first); unknown is `None`; stale with no snapshot is `platform.unavailable`. | One source per kind, the ones `identities()` and `contact()` already show, so the three calls never disagree. |
| D12 | The engine expires the library's contact-profile cache at session open and after each pass that fetched (the tap's hook), so every pass fetches the contacts' profiles. | The acceptance (2 passes) against the library's one-hour window. The library offers no setting; a patch to the pin is outside DEC-18 (cherry-picks of public upstream PRs only). Follow-up F1: an upstream option for the window, or the `$updatedAt` query Android uses, would let this go. |
| D13 | `dwcli profile get` is an alias of the existing `profile show`. | The brief names `get`; E0-09 named it `show`, and tests and scripts use that. |
| D14 | `profile_limits` reads no wallet and needs no open session, like `cost_table`. | The limits are the network's contract, not a wallet's. |
| D15 | `Profile.updated_at` stays `None`. | The library keeps no `$updatedAt` for a profile; reading it would mean a second query per read. |
| D16 | The write's quote is `DOCUMENT_10K_CREDITS` (`grant_request(UpdateProfile)`), not the cost table's `profile_update`. | The grant a host asks for is `grant_request`'s, and the engine charges no more than the grant's quote (m4 §1). DP2-04 does the same. |
| D17 | The per-pass cost of D12 (a proved query per 100 contacts per own identity, a store of each such identity's entry, an `Identities` signal; about four a minute while visible) is accepted, not throttled. | The acceptance is two passes; a throttle (say, every few minutes while hidden) would break it for a hidden window. F1 removes the cost at its source. |
| D18 | Expiry also runs when a wallet is opened mid-session (`load_wallet`), after its state is refreshed. A restore or a bring-up starts with an empty cache, which the library fetches anyway. | Review finding: a reopened wallet's cache comes back with its check times, and nothing else stores an entry that would fire the hook. |
| D19 | Identities the wallet only watches keep their check times: the expiry covers `wallet_managed_identities`. | The contacts read model shows none of their contacts; their snapshot stores fire a no-op expiry. |
| D20 | The document key is DP2's choice (`requests::flow_keys`), with the library's eligibility rule (D22), but `ECDSA_SECP256K1` only: the library's `profile_signing_key` also takes `ECDSA_HASH160`. | One key policy for every DashPay document write (E0-04 leases check the vault holds it). An identity whose only such key is HASH160 gets `identity.keys_missing`, as for a contact request. |
| D21 | Amended in fix round f1 (Sol r1 finding 2); `every_shipped_dashpay_contract_passes_the_guard` checks both DashPay versions the pin ships (v1 at protocols 1–14, v2 at 15). Before any write the engine fetches Platform's DashPay contract (`DataContract::fetch`, proved) and compares its `profile` type with the bundled one: a property the bundled type lacks, one it types otherwise, or no `profile` type at all is `profile.unsupported_contract`, before the grant is redeemed. A property only the bundled type has is no loss and passes. | Platform's document is parsed with the bundled type and the replace carries only what that parse kept, so a newer contract's field would be dropped, or reshaped. Refusing is safe; the fix is an app update (a pin bump with the new contract). The live contract's other document types are not compared. One proved contract query per write (not per no-op or refused edit) is accepted: profile edits are rare. |
| D22 | Fix round f1 (Sol r1 finding 1). Every DashPay document flow (contact request, accept, accept-and-pay, `contactInfo`, profile) chooses its key with the library's `usable_authentication_key` rule, in both the pre-redemption check (`check_held_keys`, `candidate_keys`) and the signer's narrowing (`flow_signer`). The key must be unbound, bound to DashPay, or bound to the flow's document type (`contactRequest`, `contactInfo`, `profile`); not group-bound; not expired at the wall clock; not disabled. Keys with no budget and no expiry come first, then the others, each in key-id order. The key upgrade's master signs an IdentityUpdate, which no document bound limits; the expiry rule applies to it as well. | The DP2 flows had the same gap: `flow_keys` took the first held key, and a lower-id key bound elsewhere or expired was narrowed to. The library's contact-request picker then signs with it, so Platform refuses after the grant is redeemed; its `contactInfo` picker finds no key it may use. "Unbounded first" in the finding reads as the library's order, unlimited before limited, so the narrowed signer and the library always agree. Regressions: `requests::tests::a_document_key_may_sign_its_document` (per document type), plus `profile_tests::a_profile_signs_with_a_key_that_may_sign_it` and `no_key_that_may_sign_keeps_the_grant`. |
| D23 | Fix round f1 (manager's decision on Sol r1's full-edit note). `ProfileEdit.baseline: Option<ProfileBaseline{profile: Option<Profile>}>` is what the user saw. It is compared with what `profile` shows of Platform's document now (the library's parse of it, `updated_at` aside, the avatar hashes in lower case), and a mismatch is `profile.conflict{current}`, carrying Platform's profile (or `None`) for the UI to show and re-apply against. No baseline replaces whatever Platform holds (dwcli `--force`). An edit that already matches Platform is `Ok` whatever its baseline. A conflict brings the cache in line with Platform's. | Deviation from the brief, which asked for a baseline *revision*: the library caches no revision with the profile (`DashPayProfile`), so `profile()` has none to give. A revision the engine tracked itself would be stale after another device's edit until the engine fetched the document again, which means an extra query per pass; until then every edit made after a re-read would conflict. The profile itself works from any read. A change that is made and then undone ("ABA") passes, which is harmless: the user saw that content. Hidden fields (v2 payment addresses, an untrusted avatar URL) are not compared; the edit does not own them and carries Platform's. |
| D24 | dwcli `profile set` makes its edit against the profile `profile show` shows just before it. `--baseline JSON` passes one `show` printed earlier (`null`: none), and `--force` sends none; the two conflict in clap. `profile.conflict`'s params are `{"current": profile}`. | "Passes the revision it just read" (the brief), in D23's form. `--baseline` lets a script (and T2) reproduce the UI's read-edit-submit gap. |
| D25 | Swift T0 is not rerun for f1. | The brief's rule is "only if a file Swift parses changed". Swift's tests parse `docs/contracts/m1`–`m3` (`SettingsAndCodesTests`, the contract-surface tests) and its own sources; f1 changes m4, Rust and the renderer only. `dw-ffi` is unchanged. |
| D26 | f1's surface is folded into Contract-Version 24 (item 50), not 25: the §3 marker was set back to main's recorded 23 and re-blessed at 24. | The manager kept 24 for DP4-01 (DP1-08 takes 25), and 24 has not merged, so no host has seen the earlier v24 listing. The bless refuses an unchanged version, hence the marker step. |

## Follow-ups

- F1 (D12): upstream, a configurable contact-profile refresh window (or an `$updatedAt`-incremental fetch) in
  `rs-platform-wallet`, then drop the expiry.
- F2: DP4-02 fills `AvatarChange::Set` (step 4) and `avatar`.
- F3 (D21): carry unknown fields across a contract upgrade instead of refusing, if Platform's DashPay contract gains
  profile fields (parse with the live contract).

## T2 evidence (testnet, 2026-10-15)

dwcli sessions (`dashpay session --spv`) for fixture wallets A (identity
`CgPsN9TRQK4yLcU5jhtK475YfXLqpQb2mkqU5ExUUA6a`) and B (`HjfQJmEbqM8bbwtoGMVbouiWVC9Wmk6187VLx3XjN4J6`), established
contacts. Before: A had no profile document (`profile get` on A, and on B `profile get --identity <A>`: `null`).
`profile limits`: 25 / 140. B polled `dashpay sync-status` (`dash_pay_sync.last_run_at`, a pass every 15 s) and
`profile get --identity <A>` every 2 s.

| Edit on A (`profile set`) | Write returned (UNIX s) | B shows it at | Pass on B that showed it |
|---|---|---|---|
| create: "DP4-01 Alice" / "DP4-01 T2 check: profile update reaches contact B" | 1792075045 | 1792075046 (1 s) | the first pass begun after the write (046) |
| replace: "DP4-01 Alice v2" / "second edit: replace at the next revision" | 1792075081 | 1792075092 (11 s) | the first pass begun after the write (091); the one running at the write (076) did not |
| replace, message cleared: "DP4-01 Alice v3" / `null` | 1792075130 | 1792075137 (7 s) | the first pass begun after the write (137) |

Between the edits B fetched A's profile on every pass (the cached entry was minutes old, inside the library's hour),
which is D12 at work. The same edit again sent nothing; a 26-character name was `profile.too_long{field:
"display_name", length: 26, max: 25}` and a message-only edit of blanks `profile.empty` (it would have left no field).
A's credits went from 46 487 960 420 to 46 435 126 689 over the three writes.

Those three edits ran on the binary before the review fixes (cache parse, `load_wallet` expiry). A rerun on the final
head (22bbbeb9), with both sessions reopened about two hours later: at open, A and B both read "DP4-01 Alice v3" from the
cache. Then:

| Edit on A (`profile set`) | Write returned (UNIX s) | B shows it at | Pass on B that showed it |
|---|---|---|---|
| replace: "DP4-01 Alice v4" / "final-binary rerun" | 1792082890 | 1792082903 (13 s) | the first pass begun after the write (901); the one begun at the send (886) did not |

A's credits went from 46 435 126 689 to 46 430 461 521 (4 665 168 for the write). In every run the session logs and
outputs held no phrase or passphrase (0 matches for the words and for the passphrase files' contents).
