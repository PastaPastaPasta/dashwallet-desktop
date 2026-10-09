# M4 DashPay engine contract (`dw-engine` facade)

Contract-Version: 7

Status: **contract**, 2026-10-08 (ROADMAP E0-08). Milestone M4, DashPay (DASHPAY.md). Code: the facade files of
`rust/crates/dw-engine/src/platform/` (§0). Design background: DASHPAY §2.4 (a plain-Rust facade that the binding wraps
one to one), §3.1–§3.6. Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) still applies, with the changes
in §1 below.

The facade is plain Rust in dw-engine. `dwcli` (E0-09) and the T2/T3 suites drive it directly; E0-13 binds it for the
chosen UI stack (UniFFI `dw-ffi` or Tauri `dw-app`), with `uniffi` or `specta` derives in the binding crate only.

Every call exists now and returns `platform.not_implemented{call}` (§4), where `call` is `"DashPay.<method>"`,
`"NetworkSession.<method>"` or the free function's name. No call fakes success. Unlike the m2/m3 stubs, these do not
check the session or their arguments first: E0-08 adds no behaviour, so a stub called after `close_network` still
returns `platform.not_implemented`.

The DP tasks fill in the bodies. **They do not change a signature, a record, a trait impl or a code without updating
this file and raising `Contract-Version` in the same change.** `rust/crates/dw-engine/tests/m4_dashpay_contract.rs`
enforces that:

- §3 is a listing generated from the source, with its SHA-256 and the version it was approved under. Any difference
  fails and prints a line diff. To approve a change: raise `Contract-Version` above, run
  `DW_BLESS=1 cargo test -p dw-engine --test m4_dashpay_contract`, review the diff, and run again without `DW_BLESS`.
  The bless run always fails ("re-run"), it refuses when `CI` is set, and it refuses a changed surface unless the
  version was raised.
- the facade is an explicit list of files (§0): every file of `src/platform/` is classified as facade or not, and no
  `impl DashPay` lives outside the facade files;
- every `pub` item of the facade files is re-exported from `dw_engine::platform` by name (no glob);
- every call has a §2 row whose Kind (`sync`, `async` or `free, pure`) and Errors (the first type named) match its
  signature, and every §2 row names a call. Rows are keyed by owner: `NetworkSession.x(…)` names its owner, a free
  function has Kind `free, pure`, any other row is a `DashPay` call;
- the §4 rows match each error enum's `code()`, one sample per variant in order; each code names its variant
  (`contact.self` is `IsSelf`), and every error enum in `errors.rs` has a row;
- every call not in the test's `IMPLEMENTED` list returns `platform.not_implemented` with its own name. A DP task that
  fills in a body removes the call's stub line from that test and adds the name to `IMPLEMENTED`;
- `BearerSecret` implements none of `Serialize`, `Display`, `Clone`, `PartialEq` or `Into<String>`, and
  `AvatarSource` neither `Serialize` nor `Clone` (compile-time assertions).

## 0. Object model, files and owners

```
Engine ──open_network──▶ NetworkSession ──dashpay(wallet_id)──▶ Arc<DashPay>
                                         ──stash / status / pending / forget invitation   (per network, §2.9)
                                         ──begin_flow / end_flow                          (leases, §2.11)
check_username(label)      (free function, no session)
```

`NetworkSession::dashpay(wallet_id: WalletId)` is sync and infallible: it does not check the wallet. Each call does,
once implemented (`wallet_not_found`). A `DashPay` holds its session; once implemented, its calls return
`network_not_open` after `close_network`. `DashPay` is a stateless handle, and any number may exist for one wallet:
state that outlives a call (avatar candidates, `dapk` scan proofs, read caches) lives in the session's Platform runtime
(§3.1 `mod.rs`), never in the handle.

The facade is the files of `src/platform/` listed in the table below, one per domain (DASHPAY §3.1), and the
contract test's `FACADE` list names exactly these. The other files there (`mod.rs`, `signers.rs`, `status.rs`,
DP1-01's `keys_policy.rs`, and E0-05's `bringup.rs`, `runtime.rs`, `runtime_tests.rs` and `startup_status.rs`, which
run the bring-up and the loops that `startup.rs` reads, and DP1-03's `names_net.rs` and `names_tests.rs`) are in its `NOT_FACADE` list; a new file must join one of the two lists. Each domain file
holds its records and its own `impl DashPay` block, so parallel DP tasks edit different files, and no `impl DashPay`
lives anywhere else in the crate. A record the facade returns is `pub` and re-exported by name from `mod.rs`; a helper
type is `pub(crate)`.

| File | Owner (ROADMAP) | Calls |
|---|---|---|
| `dashpay.rs` | E0-08 | `NetworkSession.dashpay`, `wallet_id`; `BearerSecret` |
| `startup.rs` | E0-05 bring-up | `status`, `sync_status`, `sync_now` |
| `identity.rs` | DP1-01, DP1-05, DP6-01 | `identities`, `set_main_identity`, `identity_detail`, `refresh_balance`, `discover_identities` |
| `registration.rs` | DP1-02 | `registration_quote`, `start_registration`, `registrations`, `resume_registration`, `discard_registration`, `finish_asset_locks`, `prepare_faucet_lock` |
| `names.rs` | DP1-03, DP1-04, DP2-03 | `check_username`, `name_availability`, `register_name`, `set_main_name`, `main_name`, `contest_status`, `search_users`, `resolve_user` |
| `contacts.rs` | DP2-01…DP2-04 | `contacts`, `contact`, `pending_setup_count`, `eligibility`, `send_request`, `accept_request`, `ignore`, `unignore`, `set_private_details`, `enable_dashpay_keys`, `my_user_link`, `verify_scanned` |
| `payments.rs` | DP3-01, DP3-02 | `payment_lock`, `resolve_payment_lock`, `contact_activity`, `frequent_contacts` |
| `notifications.rs` | DP2-05 | `events`, `unread_count`, `mark_read` |
| `profile.rs` | DP4-01, DP4-02 | `profile`, `profile_limits`, `prepare_avatar`, `avatar_upload_available`, `upload_avatar`, `update_profile`, `avatar` |
| `credits.rs` | DP1-06, DP6-02 | `cost_table`, `top_up_quote`, `top_up`, `withdraw_quote`, `withdraw` |
| `invitations.rs` | DP5-01, DP5-02 | `NetworkSession.stash_invitation`, `invitation_status`, `pending_invitations`, `forget_invitation` |
| `flows.rs` | E0-04 (design rev2 §16, §7) | `NetworkSession.begin_flow`, `end_flow`, `leases`, `grant_request`, `dispatch_status` |
| `errors.rs` | E0-05 (the mapping, §6) and each domain's owner | the error enums of §4 |

## 1. Conventions (changes to m1-engine.md §1)

| Topic | Rule |
|---|---|
| Ids | Wallet: `WalletId` (the facade is bound to one wallet). Identities and contacts: Base58 `String`. Txids: lower-case hex. Drafts, candidates, scans, faucet keys, invitation links, activity cursors: opaque `String` ids the engine issued, each 1–64 characters of `[0-9a-z_-]` (`dwcli dashpay session` checks them by that form), as is the `<id>` of a funding step id (`registration/<id>/funding`, `topup/<id>/funding`). |
| Amounts | Duffs and credits are `u64`; the field name or doc says which. |
| Every call returns `Result` | Including the in-memory reads DASHPAY §3.6 sketched without one (`status`, `identities`, `contacts`, …) and `check_username`. A stub has to return `platform.not_implemented`, and the finished reads need `network_not_open` and `wallet_not_found`. |
| Sync and async | A sync call reads in-memory state only (m1 rule 3): no SQLite on the caller's thread. An async call touches the network or persistence; once implemented it runs on the engine runtime (`NetworkSession::on_runtime`), so any executor may poll it. The sync reads are served from caches their owners keep in the session's Platform runtime, filled at bring-up and refreshed on each `Platform` signal: the status snapshot (`status`, `sync_status`; E0-05), the identity list with the main identity (`identities`, `profile`; DP1-05), the contacts read model (`contacts`, `contact`, `pending_setup_count`, `frequent_contacts`, `my_user_link`; DP2-01), the payment locks (`payment_lock`; DP3-01), the unread counts (`unread_count`; DP2-05) and the static tables (`profile_limits`, `cost_table`, `avatar_upload_available`). Paged or unbounded reads (`events`, `registrations`) are async. |
| Static tables before Platform answers | `profile_limits` returns the built-in 25 / 140 until the DashPay contract is fetched; `cost_table` returns the fee table of the SDK's current protocol version. Neither waits for the network. |
| Grants and leases | `grant: String` is either a grant id from `Vault.authorize` or a lease id from `NetworkSession.begin_flow` (§2.11). The purposes, caps and budgets are E0-04's (DASHPAY §2.6). A flow that spans calls ("Accept and pay": `accept_request`, then the payment's `TxDraft.prepare`, §2.3) redeems its grants once with `begin_flow`, from one credential prompt, and passes the lease id to each call, so the 120 s grant lifetime cannot run out between them; `end_flow` releases it. **A lease id is accepted only by a call of the lease's own wallet, and only for a purpose the lease carries**: another wallet's lease is `platform.grant_invalid`, and a lease without the purpose the call needs (a `ProfileEdit` lease passed to `top_up`) is `platform.needs_grant{purpose}`. The idle reaper ends a vault-key lease after **10 minutes** with no call, no permit and no running flow task; an own-key lease's key is dropped at its `key_until` (the flow then needs a grant again), so a host that abandons a sheet leaks nothing. Every write's quote says what to authorize (`grant: GrantRequest`, or `grant_request(identity, action)`), and the engine charges no more than it quoted. |
| Records | Derive `serde` `Serialize` and `Deserialize`, plus `Debug`, `Clone` and `PartialEq`. The exceptions are inputs that carry private data, which derive `Deserialize` only: `AvatarSource` (the Gravatar e-mail). Enums with data are internally tagged with `kind` and snake-case names; unit enums serialize as snake-case strings. Byte payloads (`AvatarSource::File.bytes`, `AvatarImage.png`) serialize as standard base64 strings, never as number arrays. |
| Errors | One enum per domain, with `code()` returning a stable string (§4), and `platform()` returning the wrapped `PlatformError`, if any. `Display` is diagnostic detail for logs. Each domain wraps `PlatformError` in a `Platform` variant, so `platform.*`, `identity.*` and the common codes reach the host from any call. |
| Secrets | See below. |

**Secrets.** Bearer credentials come in as `BearerSecret` (DASHPAY §3.8): invitation links (`stash_invitation`) and
scanned payloads that may carry a `dapk` (`verify_scanned`). The faucet path carries no secret at all (§5).

- `BearerSecret` deserializes from a JSON string and never serializes. It has no `Display`, `Clone`, `PartialEq` or
  conversion back to `String`; its `Debug` prints `BearerSecret(..)`; it is zeroed on drop; `expose()` reads it in the
  engine.
- **Bindings take secrets as bytes**, as m1's "Secrets in" rule says, because a Swift or JS `String` cannot be zeroed.
  E0-13 binds `BearerSecret` as a custom type over `Vec<u8>` built with `BearerSecret::from_utf8(Zeroizing<Vec<u8>>)`,
  never as a host string, and no binding returns one. Residual risk, as in m1: the IPC or JSON payload that carried
  the bytes is not zeroed.
- **No error `detail` and no `Display` may quote a `BearerSecret` input or any part of it.** A link parser reports
  "malformed invitation link", never the link. `from_utf8` already refuses invalid UTF-8 without quoting it.
- **`dwcli` reads bearer inputs from stdin or a file, never from argv** (argv is visible in `/proc/*/cmdline` and in
  shell history). E0-09 follows this for `invite claim` and for scans.
- `AvatarSource`'s `Debug` hides the Gravatar e-mail and prints only the byte count of a file.

**`dwcli` output (E0-09).** The DashPay commands (`dashpay`, `identity`, `name`, `contact`, `pay-contact`, `profile`,
`invite`; `rust/bin/dwcli/src/dashpay.rs`) call this facade only and print one JSON line: `{"ok":true,"result":…}`
with the record in its serde form (§1 "Records"), or `{"ok":false,"error":{"code","message","params"}}` with exit
status 1.

- `code` is the §4 code, or m1's for dwcli's own vault unlock, grants and wallet lookup. `message` is its `Display`
  text, and `params` holds the code's parameters by name, such as `call` for `platform.not_implemented`. A failure
  before the command runs (the passphrase file, the data root, opening the network) is code `setup`.
- A write asks for its grant as a host does: a `PlatformOp` capped by its quote's or `grant_request`'s `GrantRequest`,
  from `Vault.authorize` (`--max-duffs`/`--max-credits` for `identity resume`, `finish-asset-locks` and `faucet-key`,
  which have no quote). It prints the request with the outcome: `{"quote"|"grant", "outcome"}`. Registration prints `draft` instead of `outcome`.
- `pay-contact` reports `not_implemented{call: "Recipient::Contact"}` until DP3-01 adds that `TxDraft` recipient (§6).
- State kept per session (scan ids, avatar candidates, leases, a running registration, dispatch tombstones) dies with
  a one-shot process. `dwcli dashpay session` keeps one engine and runs one request per stdin line,
  `{"args":[…],"input":…,"id":…}`, where `input` is the bearer input and `id` a string or an integer. It answers each
  line in order with that command's line plus `id` (none for a line that is not a JSON object), refuses nesting and a
  request's own `--spv`, and ends with `{"ok":true,"result":{"requests":N}}`, exit status 0, whatever the requests
  returned. A line holds at most 64 KiB before its LF or CR LF.
- Session lines are read and parsed into zeroizing buffers only, `id` included: an accepted request's `id` is written
  from that buffer into its answer. Integers follow JSON's grammar (no leading zero). Before clap sees `args`, each
  token is checked against the command grammar in place: a subcommand of the command reached so far, one of its
  options (`--name`, `--name value`, `--name=value`; a separate value never starts with `-`), or a value of the
  type its option or the next positional takes. The types are a wallet id (64 lower-case hex), an identity or
  contact id (Base58 of 32 bytes), an engine-issued id (drafts, invitation links, faucet keys, activity cursors:
  1–64 of `[0-9a-z_-]`), a `dispatch-status` artifact (64 lower-case hex, or `registration/<id>/funding` or
  `topup/<id>/funding`), a Dash address (26–35 Base58 characters), a decimal integer, an enum's serde name, a DPNS
  label (3–63 of `[A-Za-z0-9-]`, no edge hyphen; `name search` takes 1–63 of those characters, and
  `name resolve` a label with `.dash` or without), an `http(s)://` URL (up to 2048 printable ASCII characters), a
  path (1–4096 of `[A-Za-z0-9._/ +@,~-]`), and free text. `name check` takes free text, so that it reports the
  rules a bad label breaks. Anything else (an unknown option or command, a surplus
  value, a value of the wrong form) refuses the request by its position. No refusal quotes the line or its `args`.
- **Every `args` value is public by contract; a credential travels only in `input`.** clap copies the values it
  parses without wiping them, and no form check can tell a key from an id of the same shape. Free text is
  `--display-name`, `--alias`, `--text` and `name check`'s label (up to 256 bytes each) and `--public-message` and
  `--note` (up to 1024 bytes each), with no control characters but LF and tab. Free text, a URL or a path that looks
  like a bearer input (`dashpay://invite`, `dapk=`, `dash:?`, an invitation link's host, `pk=` or `assetlocktx=`)
  is refused anyway, as a diagnostic.
- A panic answers with `internal` ("the outcome is unknown": a write may or may not have gone through; check before
  retrying), and the panic hook prints only its location. A panic in an engine task counts the same: the engine's
  error for it is `internal` with the detail `engine task panicked (message withheld)` (`dw_engine::TASK_PANICKED`),
  never the panic's message. After a panic a health probe runs: the session must be open, the vault must answer
  without panicking, the wallet list without an error, and the engine runtime must run a task, all within 10 s. In
  one-shot mode the panic is the command's one line, with exit status 1; if the probe fails, dwcli exits without
  stopping SPV or shutting the engine down (either may never finish). In a session the panic answers the request,
  and the session goes on if the probe passes. Otherwise it prints `session_poisoned` as its last line and exits with
  status 1 at once, without stopping SPV or the engine's shutdown. Both lines are written best effort: a failed write
  or flush (a closed pipe, a full disk) changes neither the exit status nor the skipped shutdown. Before such an exit
  dwcli wipes the passphrase it read; key material the engine holds is left to the OS. A facade error for a
  panicking engine task is `internal` with the detail `TASK_PANICKED`, and counts the same.
- `--no-platform` (a chain with no Platform) refuses every DashPay command, a session included, with
  `platform.feature_off{feature: "platform"}` before anything is opened.
- Arguments clap refuses exit with status 2 and no JSON line. stderr names the error kind, the arguments involved where
  clap names them, and the usage, never a refused value. `setup` replaces the code of what failed before the command
  ran (a storage error opening the network, for example); `message` says which.

## 2. Calls

Every call's status today: **stub**, except §2.10, DP1-05's `identities`, `set_main_identity` and
`discover_identities` (§2.1), and DP1-03's names calls in §2.3 (`check_username`, `name_availability`,
`register_name`, `set_main_name`, `main_name`). Kind is `sync`, `async` or `free, pure` (§1).

### 2.1 Status and identity

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `status()` | sync | The banner state (F1) shared by the Home card, the chip and the Contacts empty state: `NoIdentity{reason}`, `Registering{draft}`, `ContestPending{identity, label, ends_at}`, `Ready{main}`, `StartupIncomplete{startup}`. | `PlatformError` |
| `sync_status()` | sync | Tools ▸ Information's DashPay card (F23): startup status, last pass, pending contact crypto, loops, quorum source. | `PlatformError` |
| `sync_now()` | async | Runs one DashPay pass now and reports it. | `PlatformError` |
| `identities()` | sync | The wallet's identities by index: names (the library's order), main name, credit balance (`None` = unknown), whether the DashPay keys 4–5 exist, profile. `is_main` marks the `dp_main_identity` choice while the wallet still has that identity, else the lowest index. `names` and `main_name` are as `main_name(identity)` (§2.3) has them: names the identity owns by Platform's evidence (not a label in a contest or one whose write may be in flight), and its pick while owned, else the temporary or won contested name, else the name acquired first; `None` while it owns none. Read from the library's memory, or the last snapshot while a sync pass writes it; which names show comes from the app database's main-name rows, read in one statement on each call (a failed read fails the call), never from a cache. From the live list, a label whose write may be in flight or that Platform refused is hidden. From the older snapshot, `names` is empty, `main_name` is `None` and `names_updating` is `true` (show "updating", not "no name"); the other fields still come from it. `names_updating` is `false` on a live read. **Implemented (DP1-05).** | `PlatformError` |
| `set_main_identity(identity)` | async | Writes `dp_main_identity`; one of the wallet's identities, else `identity.not_found`. **Implemented (DP1-05).** | `PlatformError` (`identity.not_found`) |
| `identity_detail(identity)` | async | The summary plus revision and public keys. | `PlatformError` (`identity.not_found`) |
| `refresh_balance(identity)` | async | Fetches the credit balance; `None` = not found on Platform yet. | `PlatformError` (`identity.not_found`) |
| `discover_identities(grant)` | async | Same-seed discovery (DP1-05) past the highest identity index on file; returns the number of identities it stored. What it finds then gets the rest of a recovery in the background: a bring-up of the wallet (DashPay pass, contact accounts) and a names pass, at once while SPV runs, otherwise at the next start. Also forgets Platform's earlier proof that the seed owns none, so the next start's bring-up looks again (DP6-01's "find"). `grant` is an `IdentityScan` grant id only (E0-04 §3.2); a lease id there is `platform.grant_invalid`. A grant confirmed with the passphrase on a locked or mixing-only vault scans through its key hold (E0-04 §3.5). A vault lock during the scan is `platform.cancelled`, as is the wallet's removal, unload or a failed restore's rollback, which waits for the call to end and admits no new one until it is done; identities the scan stored before a lock or an error still get their bring-up and names pass; running out of time with nothing stored is `platform.timeout`. **Implemented (DP1-05).** | `PlatformError` |

### 2.2 Registration (DASHPAY §3.4)

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `registration_quote(req)` | async | The real costs for `req`: contested, lock, fee, total, credits left, and the `grant` to ask for (`max_duffs` and `max_credits`). Refuses a bad label (`name.*`) and bad invitation funding (`invitation.*`, `name.unavailable_for_invite`) before any prompt. | `RegistrationError` |
| `start_registration(req, grant)` | async | Persists a `Draft` row and runs the state machine; returns the draft id. `req.funding` is stored as DP1-02's versioned `funding` encoding (§3.4 "Registration rows"), not as this record's serde form. An `initial_profile` avatar must already have a URL (§5); registration never uploads. `FaucetAssetLock` is refused with `platform.feature_off{feature: "faucet"}` outside developer builds. | `RegistrationError` |
| `registrations()` | async | Every `dp_registration` row of the wallet, with what each waits for (§5). | `RegistrationError` |
| `resume_registration(draft, grant)` | async | Advances a parked flow; it acts on any state (§4.1). `grant` is required exactly when the row's `waiting` is `Unlock` or `Authorize`, and ignored otherwise. | `RegistrationError` |
| `discard_registration(draft)` | async | Allowed only when the registration's funding reads `NotSent` or `None`; otherwise `invalid_argument`. That implies `!funds_committed`, and it also refuses a live `Unsent`, whose flow is still building. This is one of the engine's funding gates, which read an asset lock's `None` with the tracked-row check (§4.1). The journal decides, whatever the phase. | `RegistrationError` |
| `finish_asset_locks(grant)` | async | Tools ▸ Repair "Finish transfers": resumes tracked asset locks that no flow finished. Not gated by the journal: it resumes committed locks and builds none. | `RegistrationError` |
| `prepare_faucet_lock(grant)` | async | Developer builds only. Derives a fresh registration asset-lock key (`m/9'/c'/5'/1'/…`) and returns its id and compressed public key for the faucet's `POST /api/asset-lock-proof` (§5). Outside developer builds: `platform.feature_off{feature: "faucet"}`. | `RegistrationError` |

### 2.3 Names

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `check_username(label)` | free, pure | The rule checklist (F4: 3–23 characters, `[A-Za-z0-9-]`, no edge hyphen, no `--`), the normalized label and whether it is contested. A bad label is `valid: false`, not an error. | `NameError` |
| `name_availability(label)` | async | `Invalid{rules}`, `Available{contested}`, `Taken{owner}`, `ContestOpen{ends_at, contenders}`, `Locked` or `Unknown`. | `NameError` |
| `register_name(identity, label, grant)` | async | An extra name, or the name of a registration that parked before it. Its cost is budgeted, not capped: an estimated fee bound plus, for a contested label, the contest fund to join, which the grant's `max_credits` and the identity's balance must cover before the grant is redeemed or anything is sent (`platform.grant_exceeded`, `platform.insufficient_credits`; a refused grant stays usable). The fee bound is an estimate until DP1-06's cost table and E0-04's per-transition accounting; Platform may charge more. A contested label returns `ContestStarted{ends_at}`; joining another identity's contest whose join deadline is unknown is `platform.unavailable`, with nothing spent. A plain name registered while the identity's own contest is open becomes its temporary name. What became of a write comes from Platform only: a label whose write may be in flight is no owned name, and a retry (or the identity's next registration) asks Platform and records the answer. | `NameError` |
| `set_main_name(identity, label)` | async | Picks which owned name the identity shows; `None` clears the pick. A name the identity does not own by Platform's evidence (a label whose write may be in flight included) is `invalid_argument`. The pick is the user's and sync never rewrites it (#4978). | `NameError` |
| `main_name(identity)` | async | The name the identity shows: the pick while owned, else the temporary name during an open contest, else the label it contended for once won, else the name it got first by Platform's acquisition time (the marketplace row's `$transferredAt`, else `$createdAt`, else the library's stamp; untimed names last). `None` if it owns none. | `NameError` |
| `contest_status(identity, label)` | async | The own contest: state, deadline, contenders and votes, the temporary name. | `NameError` |
| `search_users(prefix, limit)` | async | DPNS prefix search. Only the prefix goes to DAPI. `relation` is relative to the main identity. | `NameError` |
| `resolve_user(username)` | async | Exact lookup; `None` if no such name. `relation` as in `search_users`. | `NameError` |

### 2.4 Contacts

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `contacts(identity, q)` | sync | The sections asked for (empty = all), sorted, filtered by `text`. | `ContactError` |
| `contact(identity, contact)` | sync | One contact with profile, private details, publish state and payment lock; `None` if unknown. | `ContactError` |
| `pending_setup_count()` | sync | Contacts waiting for an unlock to finish their crypto ("Unlock to finish setting up N contacts"). | `ContactError` |
| `eligibility(identity, contact)` | async | Before any prompt: `Ok`, `NoDashPayKeys`, `IsSelf`, `AlreadyContact`, `PendingOutgoing`, `PendingIncoming`. | `ContactError` |
| `send_request(identity, to, scan, grant)` | async | Sends a contact request. `scan` is `ScannedContact.scan` when the request comes from a `dapk` QR: the engine then sends it with the auto-accept proof (`send_contact_request_from_qr`). A scan id is single-use and lives for the session; a stale id or an expired proof is `contact.scan_expired`. An ineligible target is `contact.ineligible{reason}` with the `eligibility` answer. | `ContactError` |
| `accept_request(identity, from, grant)` | async | Sends the reverse request. | `ContactError` |
| `ignore(identity, contact)` | async | Adds to `ignored_senders`. | `ContactError` |
| `unignore(identity, contact)` | async | Removes it again. | `ContactError` |
| `set_private_details(identity, contact, d, grant)` | async | Alias, note, hidden. Stored locally; published as an encrypted `contactInfo` once there are ≥ 2 contacts (`DeferredUntilTwoContacts` before that; `grant` is needed to publish). | `ContactError` |
| `enable_dashpay_keys(identity, grant)` | async | Adds keys 4–5 to an identity that lacks them (F9). | `ContactError` |
| `my_user_link(identity)` | sync | `dashpay://user?id=&username=`. | `ContactError` |
| `verify_scanned(text)` | async | A plain user link or a DIP-15 `dash:?du=&dapk=` payload, verified against Platform. A `dapk` proof stays in the engine; the result carries its `scan` id. An expired proof is `contact.scan_expired`. | `ContactError` |

### 2.5 Payments and activity (the `TxDraft` recipient is in `send/`, §6)

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `payment_lock(identity, contact)` | sync | The lock an ambiguous broadcast set (`dp_payment_lock`), if any. | `ContactError` |
| `resolve_payment_lock(identity, contact)` | async | Reconciles the lock with E0-04 §16.6's evidence (§4.1) for the locked payment's txid. `Sent` (an attempt or a Resend finished `Sent`, or the wallet has seen the transaction) clears the lock. `NotSent` clears it only on **positive evidence**: in this process, `dispatch_status(txid)` is `Some(NotSent)` (the send's tombstone), or a ChainLocked conflicting spend of one of its inputs exists. Never "not found on chain or in the wallet": a peer may be withholding it. Anything else is `Unknown`, which reads like `MaybeSent` and `None`: the lock stays. | `ContactError` |
| `contact_activity(identity, contact, cursor, f)` | async | Payments to and from the contact, newest first, filtered All / Sent / Received. | `ContactError` |
| `frequent_contacts(identity, limit)` | sync | The Pay screen's frequent strip. | `ContactError` |

### 2.6 Notifications

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `events(identity, cursor, limit)` | async | Pending (incoming requests), New (unread) and Earlier (read) from `dp_events`. `cursor` is an event id. | `PlatformError` |
| `unread_count(identity)` | sync | The bell count. | `PlatformError` |
| `mark_read(identity, up_to)` | async | Marks events with id ≤ `up_to` read. | `PlatformError` |

### 2.7 Profile and avatars

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `profile(identity)` | sync | The stored profile; `None` if none. | `PlatformError` |
| `profile_limits()` | sync | From the DashPay contract: 25 / 140 (§1, before the first fetch). | `PlatformError` |
| `prepare_avatar(src)` | async | Fetches or decodes `File{bytes, crop}`, `Url{url}` or `Gravatar{email}`, re-encodes to PNG and returns a candidate with a preview (DASHPAY §3.8). | `AvatarError` |
| `avatar_upload_available()` | sync | An Imgur client id is configured. | `AvatarError` |
| `upload_avatar(candidate)` | async | Uploads a `File` candidate to Imgur; returns its URL and sets the candidate's `url`. | `AvatarError` |
| `update_profile(identity, edit, grant)` | async | Publishes the whole new profile (`None` clears a field). | `PlatformError` |
| `avatar(identity, size)` | async | The cached or fetched thumbnail of any identity (a contact, a search hit, the inviter, an own identity); `None` if there is no avatar or "Load contact pictures" is off. The contact and user records carry no image, so this call serves them. | `AvatarError` |

### 2.8 Credits

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `cost_table()` | sync | Credit costs for "≈ N contact requests" and the low-credit warnings (§1, before the first fetch). | `CreditsError` |
| `top_up_quote(identity, duffs)` | async | The fee and total in duffs, the credits a top-up of `duffs` buys, and the `grant` to ask for. Refuses below `top_up_min_duffs` (`credits.below_minimum{min}`) and above the spendable balance (`credits.funding_insufficient{needed, available}`). | `CreditsError` |
| `top_up(identity, duffs, grant)` | async | Funds the identity from the Core balance; the same refusals as the quote. | `CreditsError` |
| `withdraw_quote(identity, amount)` | async | The credits taken, the fee in credits, the duffs expected at `to`, and the `grant` to ask for; `All` leaves the fee reserve (DP6-02). | `CreditsError` |
| `withdraw(identity, to, amount, grant)` | async | Credits to the Core address `to`. | `CreditsError` |

### 2.9 Invitations (on `NetworkSession`)

Per network, not per wallet: a link may arrive before any wallet exists (F18, "Paste invitation link" on the welcome
screen), and the vault is per network. Before a vault exists (the session's vault is `NoVault`), a stashed link is a
0600 file in the vault directory; it moves into the vault, and the file is deleted, when the vault is created (DASHPAY
§2.9, last row). The ids stay the same across the move.

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.stash_invitation(link)` | async | Stores the link as the vault record `invitation/<id>`, or as the 0600 file before a vault exists; returns the id. A malformed link is `invitation.invalid`, and the error never quotes it. | `InvitationError` |
| `NetworkSession.invitation_status(link_id)` | async | `Valid{inviter, funding_duffs, contested_allowed, expires_at}`, `Claimed`, `Invalid{reason}` or `Expired`. | `InvitationError` |
| `NetworkSession.pending_invitations()` | async | The ids of the stashed links, oldest first: the replay after a restart or after onboarding (DP5-01). | `InvitationError` |
| `NetworkSession.forget_invitation(link_id)` | async | Deletes a stashed link: the user dismissed it, or the claim finished. | `InvitationError` |

### 2.10 Facade

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.dashpay(wallet_id)` | sync | A new handle for the wallet (§0). **works** | — |
| `wallet_id()` | sync | The wallet the facade is bound to. **works** | — |

### 2.11 Flows, grants and the dispatch journal (`flows.rs`; E0-04 design rev2 §16)

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.begin_flow(wallet_id, flow, grants)` | async | Redeems every grant id into one lease for `flow` (E0-04 `begin_lease`) and returns the lease id, which the calls of that wallet accept as their `grant` for the purposes the lease carries (§1). Waits while a lock drain runs. A lock that lands meanwhile is `platform.cancelled` (`lease.locked`); ask again. | `PlatformError` |
| `NetworkSession.end_flow(lease)` | sync | Releases the lease; idempotent. In-flight hand-offs finish first (E0-04 §4.1). | `PlatformError` |
| `NetworkSession.leases()` | sync | The live leases as `LeaseView`s (E0-04 §4.6): flow, state, own key and its seconds left, `funds_committed`, budgets, permits in flight, and whether a library call of the flow runs (which picks the copy, E0-04 §16.10). Re-queried on E0-04's `LeaseChanged` (§6). | `PlatformError` |
| `grant_request(identity, action)` | async | The `GrantRequest` for one write that has no quote of its own: `send_request`, `accept_request`, `register_name`, `update_profile`, `set_private_details` (publishing) and `enable_dashpay_keys`. `GrantAction` carries no payload except the label, so the request is a worst-case bound for the action, which the engine budgets against (E0-04 owns enforcing it). `RegisterName` is implemented (DP1-03): no duffs, credits for the estimated fee bound plus the label's fund to join at the contest's current size; until DP1-06 and E0-04's accounting the fee part is an estimate, not a ceiling the engine enforces. | `PlatformError` |
| `dispatch_status(artifact)` | async | What the engine knows about a handed-off artifact: a txid, a state-transition hash, or a funding step id (`registration/<draft>/funding`, `topup/<id>/funding`), as `broadcast_unknown{artifact}` and `will_be_sent{artifact}` carry. `WillBeSent`, `MaybeSent`, `Sent` or `NotSent`; `None` means the engine has no entry. Answered by E0-04 §16.6's table (§4.1). The host reads every `None` as unknown, and only `Some(NotSent)` allows a retry it offers; it calls this before it offers any retry. | `PlatformError` |

## 3. Surface (generated)

The exact public surface: records, enums, error enums, signatures and the headers of every trait impl, with the
`derive`, `serde` and `cfg` attributes that fix what a binding and the JSON see. Do not edit by hand (see the top of
this file).

<!-- BEGIN GENERATED: dashpay-surface -->
<!-- surface-sha256: 77aae1157f5a188e9f9142ef84a1cbd30351b1c834dc43902a044b38f9d42664 version: 7 -->

```rust
// src/platform/contacts.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Stranger,
    Contact,
    PendingOutgoing,
    PendingIncoming,
    Ignored,
    IsSelf,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactQuery {
    pub sections: Vec<ContactSection>,
    pub sort: ContactSort,
    pub text: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSection {
    Requests,
    Contacts,
    Pending,
    Hidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSort {
    DisplayName,
    Username,
    DateAdded,
    LastActivity,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsPage {
    pub sections: Vec<ContactSectionPage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSectionPage {
    pub section: ContactSection,
    pub contacts: Vec<ContactSummary>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSummary {
    pub contact: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub alias: Option<String>,
    pub relation: Relation,
    pub hidden: bool,
    pub channel: ChannelState,
    pub since: Option<u64>,
    pub last_activity_at: Option<u64>,
    pub unverified: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelState {
    NotEstablished,
    SetupPending,
    Ready,
    Broken,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDetail {
    pub summary: ContactSummary,
    pub profile: Option<Profile>,
    pub private_details: PrivateDetails,
    pub publish_state: PublishState,
    pub request_sent_at: Option<u64>,
    pub request_received_at: Option<u64>,
    pub payment_lock: Option<PaymentLock>,
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrivateDetails {
    pub alias: Option<String>,
    pub note: Option<String>,
    pub hidden: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    Local,
    Published,
    DeferredUntilTwoContacts,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Ok,
    NoDashPayKeys,
    IsSelf,
    AlreadyContact,
    PendingOutgoing,
    PendingIncoming,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestOutcome {
    Pending { request: String },
    Established { request: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedContact {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub scan: Option<String>,
    pub relation: Relation,
    pub unverified: bool,
}
impl DashPay {
    pub fn contacts(&self, identity: String, q: ContactQuery) -> Result<ContactsPage, ContactError>;
    pub fn contact(&self, identity: String, contact: String) -> Result<Option<ContactDetail>, ContactError>;
    pub fn pending_setup_count(&self) -> Result<u32, ContactError>;
    pub async fn eligibility(&self, identity: String, contact: String) -> Result<Eligibility, ContactError>;
    pub async fn send_request(&self, identity: String, to: String, scan: Option<String>, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn accept_request(&self, identity: String, from: String, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn set_private_details(&self, identity: String, contact: String, d: PrivateDetails, grant: Option<String>) -> Result<PublishState, ContactError>;
    pub async fn enable_dashpay_keys(&self, identity: String, grant: String) -> Result<(), ContactError>;
    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError>;
    pub async fn verify_scanned(&self, text: BearerSecret) -> Result<ScannedContact, ContactError>;
}
// src/platform/credits.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTable {
    pub contact_request: u64,
    pub profile_update: u64,
    pub contact_info: u64,
    pub enable_dashpay_keys: u64,
    pub credits_per_duff: u64,
    pub top_up_min_duffs: u64,
    pub low_credits: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpQuote {
    pub fee_duffs: u64,
    pub total_duffs: u64,
    pub credits: u64,
    pub grant: GrantRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpOutcome {
    pub txid: String,
    pub credits_added: Option<u64>,
    pub balance: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawAmount {
    All,
    Credits { credits: u64 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawQuote {
    pub credits: u64,
    pub fee_credits: u64,
    pub expected_duffs: u64,
    pub grant: GrantRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawOutcome {
    pub credits: u64,
    pub expected_duffs: Option<u64>,
    pub remaining_credits: Option<u64>,
}
impl DashPay {
    pub fn cost_table(&self) -> Result<CostTable, CreditsError>;
    pub async fn top_up_quote(&self, identity: String, duffs: u64) -> Result<TopUpQuote, CreditsError>;
    pub async fn top_up(&self, identity: String, duffs: u64, grant: String) -> Result<TopUpOutcome, CreditsError>;
    pub async fn withdraw_quote(&self, identity: String, amount: WithdrawAmount) -> Result<WithdrawQuote, CreditsError>;
    pub async fn withdraw(&self, identity: String, to: String, amount: WithdrawAmount, grant: String) -> Result<WithdrawOutcome, CreditsError>;
}
// src/platform/dashpay.rs
pub struct DashPay {
    ..
}
impl NetworkSession {
    pub fn dashpay(self: &Arc<Self>, wallet_id: WalletId) -> Arc<DashPay>;
}
impl DashPay {
    pub fn wallet_id(&self) -> WalletId;
}
#[derive(serde::Deserialize)]
#[serde(from = "String")]
pub struct BearerSecret(..);
impl BearerSecret {
    pub fn new(secret: String) -> Self;
    pub fn from_utf8(mut bytes: Zeroizing<Vec<u8>>) -> Result<Self, PlatformError>;
    pub fn expose(&self) -> &str;
}
impl From<String> for BearerSecret;
impl std::fmt::Debug for BearerSecret;
// src/platform/errors.rs
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    Unavailable,
    Timeout,
    ProofInvalid,
    TrustMismatch,
    ContextUnavailable,
    SignerUnavailable,
    SeedMismatch,
    InsufficientCredits { needed: u64, available: u64 },
    GrantInvalid,
    GrantExceeded { purpose: BudgetPurpose, needed: u64, remaining: u64 },
    BroadcastUnknown { artifact: String },
    WillBeSent { artifact: String },
    Cancelled,
    NeedsGrant { purpose: BudgetPurpose },
    LeaseRevoked { cause: RevokeCause },
    LeaseExpired,
    FeatureOff { feature: String },
    NotImplemented { call: String },
    Identity(IdentityError),
    InvalidArgument { detail: String },
    NetworkNotOpen,
    WalletNotFound,
    Storage { detail: String },
    Internal { detail: String },
}
impl PlatformError {
    pub fn code(&self) -> &'static str;
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    NotFound,
    KeysMissing { purpose: KeyPurpose },
}
impl IdentityError {
    pub fn code(&self) -> &'static str;
}
impl From<IdentityError> for PlatformError;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    InProgress,
    FundingInsufficient { needed: u64, available: u64 },
    IslockTimeout,
    Recoverable { draft: String },
    AlreadyHasUsername,
    Name(NameError),
    Invitation(InvitationError),
    Platform(PlatformError),
}
impl RegistrationError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
impl From<NameError> for RegistrationError;
impl From<InvitationError> for RegistrationError;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    Invalid { rules: Vec<UsernameRule> },
    Taken,
    ContestOpen,
    Locked,
    UnavailableForInvite,
    Platform(PlatformError),
}
impl NameError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContactError {
    Ineligible { reason: Eligibility },
    AlreadyContact,
    RequestPending,
    IsSelf,
    ChannelBroken,
    PaymentLocked { txid: String },
    ScanExpired,
    Platform(PlatformError),
}
impl ContactError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvitationError {
    Invalid,
    Claimed,
    Expired,
    AlreadyHasIdentity,
    Platform(PlatformError),
}
impl InvitationError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AvatarError {
    TooLarge,
    Unsupported,
    FetchFailed,
    HashMismatch,
    UploadUnconfigured,
    Platform(PlatformError),
}
impl AvatarError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreditsError {
    FundingInsufficient { needed: u64, available: u64 },
    BelowMinimum { min: u64 },
    Platform(PlatformError),
}
impl CreditsError {
    pub fn code(&self) -> &'static str;
    pub fn platform(&self) -> Option<&PlatformError>;
}
impl From<dash_sdk::Error> for PlatformError;
impl From<platform_wallet::PlatformWalletError> for PlatformError;
impl From<dw_vault::VaultError> for PlatformError;
impl From<crate::EngineError> for PlatformError;
// src/platform/flows.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    Registration,
    TopUp,
    Withdraw,
    NameRegistration,
    ProfileEdit,
    ContactRequest,
    Accept,
    AcceptAndPay,
    PrivateDetails,
    EnableDashPayKeys,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPurpose {
    Funding,
    Credits,
    Spend,
    Crypto,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeCause {
    Lock,
    Close,
    PassphraseChange,
    WalletRemoved,
    WalletClosed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRequest {
    pub max_duffs: u64,
    pub max_credits: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantAction {
    SendRequest,
    AcceptRequest,
    RegisterName { label: String },
    UpdateProfile,
    PublishPrivateDetails,
    EnableDashPayKeys,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    WillBeSent,
    MaybeSent,
    Sent,
    NotSent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchResolved {
    pub wallet_id: String,
    pub artifact: String,
    pub resolution: DispatchResolution,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchResolution {
    Sent,
    NotSent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseView {
    pub id: String,
    pub wallet_id: String,
    pub flow: FlowKind,
    pub state: LeaseStateView,
    pub own_key: bool,
    pub key_expires_in_secs: Option<u64>,
    pub funds_committed: bool,
    pub budgets: Vec<BudgetView>,
    pub in_flight: u32,
    pub call_running: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LeaseStateView {
    Active,
    AwaitingProof,
    Parked { reason: ParkReason },
    NeedsGrant,
    Revoked { cause: RevokeCause },
    Ended,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    ProofWaiting,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetView {
    pub purpose: BudgetPurpose,
    pub ceiling: u64,
    pub spent: u64,
}
impl NetworkSession {
    pub async fn begin_flow(self: &Arc<Self>, wallet_id: WalletId, flow: FlowKind, grants: Vec<String>) -> Result<String, PlatformError>;
    pub fn end_flow(&self, lease: String) -> Result<(), PlatformError>;
    pub fn leases(&self) -> Result<Vec<LeaseView>, PlatformError>;
}
impl DashPay {
    pub async fn grant_request(&self, identity: String, action: GrantAction) -> Result<GrantRequest, PlatformError>;
    pub async fn dispatch_status(&self, artifact: String) -> Result<Option<DispatchState>, PlatformError>;
}
// src/platform/identity.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySummary {
    pub identity: String,
    pub index: u32,
    pub names: Vec<String>,
    pub main_name: Option<String>,
    pub names_updating: bool,
    pub is_main: bool,
    pub balance: Option<u64>,
    pub has_dashpay_keys: bool,
    pub profile: Option<Profile>,
    pub unverified: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityDetail {
    pub summary: IdentitySummary,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityKeyInfo {
    pub id: u32,
    pub purpose: KeyPurpose,
    pub security_level: SecurityLevel,
    pub key_type: KeyType,
    pub public_key: String,
    pub read_only: bool,
    pub disabled_at: Option<u64>,
    pub contract_bound: Option<String>,
    pub contract_bound_document_type: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPurpose {
    Authentication,
    Encryption,
    Decryption,
    Transfer,
    System,
    Voting,
    Owner,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityLevel {
    Master,
    Critical,
    High,
    Medium,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    EcdsaSecp256k1,
    Bls12381,
    EcdsaHash160,
    Bip13ScriptHash,
    EddsaHash160,
}
impl DashPay {
    pub fn identities(&self) -> Result<Vec<IdentitySummary>, PlatformError>;
    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError>;
    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError>;
    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError>;
    pub async fn discover_identities(&self, grant: String) -> Result<u32, PlatformError>;
}
// src/platform/invitations.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvitationStatus {
    Valid { inviter: Option<Counterparty>, funding_duffs: u64, contested_allowed: bool, expires_at: Option<u64> },
    Claimed,
    Invalid { reason: InvitationInvalidReason },
    Expired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationInvalidReason {
    Malformed,
    WrongNetwork,
    AssetLockNotFound,
}
impl NetworkSession {
    pub async fn stash_invitation(self: &Arc<Self>, link: BearerSecret) -> Result<String, InvitationError>;
    pub async fn invitation_status(self: &Arc<Self>, link_id: String) -> Result<InvitationStatus, InvitationError>;
    pub async fn pending_invitations(self: &Arc<Self>) -> Result<Vec<String>, InvitationError>;
    pub async fn forget_invitation(self: &Arc<Self>, link_id: String) -> Result<(), InvitationError>;
}
// src/platform/names.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsernameCheck {
    pub valid: bool,
    pub normalized: String,
    pub contested: bool,
    pub rules: Vec<UsernameRuleCheck>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsernameRuleCheck {
    pub rule: UsernameRule,
    pub passed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsernameRule {
    MinLength,
    MaxLength,
    AllowedCharacters,
    NoEdgeHyphen,
    NoDoubleHyphen,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NameAvailability {
    Invalid { rules: Vec<UsernameRule> },
    Available { contested: bool },
    Taken { owner: Option<String> },
    ContestOpen { ends_at: Option<u64>, contenders: u32 },
    Locked,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NameOutcome {
    Registered,
    ContestStarted { ends_at: Option<u64> },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestStatus {
    pub label: String,
    pub state: ContestState,
    pub ends_at: Option<u64>,
    pub contenders: Vec<ContestContender>,
    pub lock_votes: Option<u32>,
    pub abstain_votes: Option<u32>,
    pub temporary_name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContestState {
    Open,
    Won,
    Lost { winner: Option<String> },
    Locked,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestContender {
    pub identity: String,
    pub votes: Option<u32>,
    pub is_self: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserHit {
    pub identity: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub relation: Relation,
    pub unverified: bool,
}
pub fn check_username(label: &str) -> Result<UsernameCheck, NameError>;
impl DashPay {
    pub async fn name_availability(&self, label: String) -> Result<NameAvailability, NameError>;
    pub async fn register_name(&self, identity: String, label: String, grant: String) -> Result<NameOutcome, NameError>;
    pub async fn set_main_name(&self, identity: String, label: Option<String>) -> Result<(), NameError>;
    pub async fn main_name(&self, identity: String) -> Result<Option<String>, NameError>;
    pub async fn contest_status(&self, identity: String, label: String) -> Result<ContestStatus, NameError>;
    pub async fn search_users(&self, prefix: String, limit: u32) -> Result<Vec<UserHit>, NameError>;
    pub async fn resolve_user(&self, username: String) -> Result<Option<UserHit>, NameError>;
}
// src/platform/notifications.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPage {
    pub pending: Vec<ContactSummary>,
    pub new: Vec<DashPayEvent>,
    pub earlier: Vec<DashPayEvent>,
    pub next_cursor: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPayEvent {
    pub id: u64,
    pub kind: EventKind,
    pub contact: Option<String>,
    pub reference: Option<String>,
    pub at: u64,
    pub read_at: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UsernameRegistered,
    ContestWon,
    ContestLost,
    ContestLocked,
    RequestReceived,
    RequestAccepted,
    ContactEstablished,
    PaymentReceived,
}
impl DashPay {
    pub async fn events(&self, identity: String, cursor: Option<u64>, limit: u32) -> Result<EventPage, PlatformError>;
    pub fn unread_count(&self, identity: String) -> Result<u32, PlatformError>;
    pub async fn mark_read(&self, identity: String, up_to: u64) -> Result<(), PlatformError>;
}
// src/platform/payments.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentLock {
    pub txid: String,
    pub since: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockResolution {
    Sent,
    NotSent,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityFilter {
    All,
    Sent,
    Received,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub txid: String,
    pub direction: ActivityDirection,
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDirection {
    Sent,
    Received,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterparty {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}
impl DashPay {
    pub fn payment_lock(&self, identity: String, contact: String) -> Result<Option<PaymentLock>, ContactError>;
    pub async fn resolve_payment_lock(&self, identity: String, contact: String) -> Result<LockResolution, ContactError>;
    pub async fn contact_activity(&self, identity: String, contact: String, cursor: Option<String>, f: ActivityFilter) -> Result<ActivityPage, ContactError>;
    pub fn frequent_contacts(&self, identity: String, limit: u32) -> Result<Vec<ContactSummary>, ContactError>;
}
// src/platform/profile.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_url: Option<String>,
    pub avatar_hash: Option<String>,
    pub avatar_fingerprint: Option<String>,
    pub updated_at: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLimits {
    pub display_name_max: u32,
    pub public_message_max: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileEdit {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar: AvatarChange,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarChange {
    Keep,
    Remove,
    Set { candidate: String },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarSource {
    File { #[serde(with = "base64_bytes")] bytes: Vec<u8>, crop: Option<CropRect> },
    Url { url: String },
    Gravatar { email: String },
}
impl std::fmt::Debug for AvatarSource;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarCandidate {
    pub id: String,
    pub preview: AvatarImage,
    pub url: Option<String>,
    pub needs_upload: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarSize {
    Small,
    Large,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarImage {
    pub #[serde(with = "base64_bytes")] png: Vec<u8>,
    pub size: AvatarSize,
}
impl DashPay {
    pub fn profile(&self, identity: String) -> Result<Option<Profile>, PlatformError>;
    pub fn profile_limits(&self) -> Result<ProfileLimits, PlatformError>;
    pub async fn prepare_avatar(&self, src: AvatarSource) -> Result<AvatarCandidate, AvatarError>;
    pub fn avatar_upload_available(&self) -> Result<bool, AvatarError>;
    pub async fn upload_avatar(&self, candidate: String) -> Result<String, AvatarError>;
    pub async fn update_profile(&self, identity: String, edit: ProfileEdit, grant: String) -> Result<(), PlatformError>;
    pub async fn avatar(&self, identity: String, size: AvatarSize) -> Result<Option<AvatarImage>, AvatarError>;
}
// src/platform/registration.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub label: String,
    pub temporary_label: Option<String>,
    pub funding: RegistrationFunding,
    pub initial_profile: Option<InitialProfile>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_candidate: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrationFunding {
    CoreBalance,
    Invitation { link_id: String },
    ExistingIdentity { identity: String },
    FaucetAssetLock { key: String, proof: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaucetLockKey {
    pub key: String,
    pub public_key: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationQuote {
    pub contested: bool,
    pub lock_duffs: u64,
    pub fee_duffs: u64,
    pub total_duffs: u64,
    pub remaining_credits: u64,
    pub grant: GrantRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationStatus {
    pub draft: String,
    pub phase: RegistrationPhase,
    pub label: String,
    pub temporary_label: Option<String>,
    pub identity: Option<String>,
    pub txid: Option<String>,
    pub waiting: Option<RegistrationWait>,
    pub holds_key: bool,
    pub funds_committed: bool,
    pub contest_ends_at: Option<u64>,
    pub failure: Option<RegistrationFailure>,
    pub created_at: u64,
    pub updated_at: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationPhase {
    Draft,
    KeysPrepared,
    FundingSent,
    ProofWaiting,
    IdentityRegistered,
    NameRequested,
    NameRegistered,
    Contested,
    ProfileCreated,
    Done,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationWait {
    Unlock,
    Authorize,
    Sync,
    InstantSend,
    ChainLock,
    Network,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationFailure {
    pub phase: RegistrationPhase,
    pub code: String,
    pub retryable: bool,
    pub needed: Option<u64>,
    pub available: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReport {
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}
impl DashPay {
    pub async fn registration_quote(&self, req: RegistrationRequest) -> Result<RegistrationQuote, RegistrationError>;
    pub async fn start_registration(&self, req: RegistrationRequest, grant: String) -> Result<String, RegistrationError>;
    pub async fn registrations(&self) -> Result<Vec<RegistrationStatus>, RegistrationError>;
    pub async fn resume_registration(&self, draft: String, grant: Option<String>) -> Result<(), RegistrationError>;
    pub async fn discard_registration(&self, draft: String) -> Result<(), RegistrationError>;
    pub async fn finish_asset_locks(&self, grant: String) -> Result<FinishReport, RegistrationError>;
    pub async fn prepare_faucet_lock(&self, grant: String) -> Result<FaucetLockKey, RegistrationError>;
}
// src/platform/startup.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashPayStatus {
    NoIdentity { reason: Option<NoIdentityReason> },
    Registering { draft: String },
    ContestPending { identity: String, label: String, ends_at: Option<u64> },
    Ready { main: String },
    StartupIncomplete { startup: StartupStatus },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoIdentityReason {
    WatchOnly,
    WaitingForSync,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupStatus {
    NotRun,
    Starting,
    Ready,
    NoIdentity,
    PartialNoIdentity,
    DiscoveryFailed,
    PartialAccountsPending,
    SeedBindingUnverified,
    IdentityScanIncomplete,
    IdentityUnsettled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPaySyncStatus {
    pub startup: StartupStatus,
    pub last_pass: Option<SyncPassReport>,
    pub pending_contact_crypto: u32,
    pub loops: Vec<SyncLoopStatus>,
    pub quorum_source: QuorumSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPassReport {
    pub started_at: u64,
    pub finished_at: u64,
    pub new_requests: u32,
    pub new_contacts: u32,
    pub new_payments: u32,
    pub failure_code: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLoopStatus {
    pub sync_loop: SyncLoop,
    pub running: bool,
    pub last_run_at: Option<u64>,
    pub next_run_at: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncLoop {
    IdentitySync,
    DashPaySync,
    DpnsSync,
    PlatformAddressSync,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuorumSource {
    Spv,
    TrustedFallback,
    Trusted,
}
impl DashPay {
    pub fn status(&self) -> Result<DashPayStatus, PlatformError>;
    pub fn sync_status(&self) -> Result<DashPaySyncStatus, PlatformError>;
    pub async fn sync_now(&self) -> Result<SyncPassReport, PlatformError>;
}
```

<!-- END GENERATED: dashpay-surface -->

## 4. Error codes

Stable strings; the UI picks the copy. Codes with fields carry them in the variant (`insufficient_credits{needed,
available}` and so on); a binding forwards them as parameters, as m1's review rule M-5 does.

| Error enum | Codes |
|---|---|
| `PlatformError` | `platform.unavailable`, `platform.timeout`, `platform.proof_invalid`, `platform.trust_mismatch`, `platform.context_unavailable`, `platform.signer_unavailable`, `platform.seed_mismatch`, `platform.insufficient_credits`, `platform.grant_invalid`, `platform.grant_exceeded`, `platform.broadcast_unknown`, `platform.will_be_sent`, `platform.cancelled`, `platform.needs_grant`, `platform.lease_revoked`, `platform.lease_expired`, `platform.feature_off`, `platform.not_implemented`, `invalid_argument`, `network_not_open`, `wallet_not_found`, `storage`, `internal` |
| `IdentityError` | `identity.not_found`, `identity.keys_missing` |
| `RegistrationError` | `registration.in_progress`, `registration.funding_insufficient`, `registration.islock_timeout`, `registration.recoverable`, `registration.already_has_username` |
| `NameError` | `name.invalid`, `name.taken`, `name.contest_open`, `name.locked`, `name.unavailable_for_invite` |
| `ContactError` | `contact.ineligible`, `contact.already_contact`, `contact.request_pending`, `contact.self`, `contact.channel_broken`, `contact.payment_locked`, `contact.scan_expired` |
| `InvitationError` | `invitation.invalid`, `invitation.claimed`, `invitation.expired`, `invitation.already_has_identity` |
| `AvatarError` | `avatar.too_large`, `avatar.unsupported`, `avatar.fetch_failed`, `avatar.hash_mismatch`, `avatar.upload_unconfigured` |
| `CreditsError` | `credits.funding_insufficient`, `credits.below_minimum` |

- `PlatformError` carries the m1 common codes (`invalid_argument`, `network_not_open`, `wallet_not_found`, `storage`,
  `internal`), unprefixed as in m1 §4. m1's common `not_implemented` is `platform.not_implemented{call}` here, so a
  host's code table (`ServiceErrorCode`) needs both.
- `IdentityError` is reached through `PlatformError::Identity`, so every domain can return `identity.*`.
- Every other enum wraps `PlatformError` in its `Platform` variant; `code()` passes the inner code through and
  `platform()` returns it.
- **`RegistrationError` also wraps `NameError` and `InvitationError`**: a registration refuses its label with `name.*`
  (including `name.unavailable_for_invite`) and its invitation funding with `invitation.*`. Converting either into a
  `RegistrationError` moves its `Platform` variant to `RegistrationError::Platform`, so each code has one
  representation.
- **`platform.broadcast_unknown`, `platform.will_be_sent` and `platform.cancelled`** are E0-04's provisional and
  cancelled outcomes (DASHPAY §2.6, "Commit points"; E0-04 owns their semantics). Every write that hands off a signed
  artifact can return them:
  `send_request`, `accept_request`, `set_private_details` (when it publishes), `enable_dashpay_keys`, `register_name`,
  `update_profile`, `top_up` and `withdraw`. For registration the state machine records the outcome in the row
  instead (`waiting`, or a `Failed` phase with `retryable`).
  - `broadcast_unknown{artifact}`: the artifact may already be out. **The UI never offers a blind retry**: it says
    "may have been sent", and offers a retry only once `dispatch_status(artifact)` says `NotSent` (or the
    `DispatchResolved` event does, §6). A re-query of the chain or Platform is not enough: a committed entry the
    journal still holds can be resent later, so a retry on "nothing on chain" could pay twice (E0-04 review finding
    2). This mirrors m1's `send.broadcast_unknown`.
  - `will_be_sent{artifact}`: the artifact is committed and the engine will send it again, for example when the
    network is back (E0-04 Q13: "will be sent when the network is back"). **Never retried and never discarded**; the
    UI shows it as pending until `DispatchResolved` arrives.
  - **Retry, discard and what `None` means: §4.1, which is E0-04 §16.6 word for word.** The host offers a retry only
    after `DispatchResolved(NotSent)` or a `dispatch_status` of `Some(NotSent)`, and reads every `None` as unknown,
    whatever the artifact. Only the engine's funding gates apply the asset-lock reading of `None`, with the
    tracked-row check. The row-less artifacts (state transitions: `send_request`, `accept_request`, `register_name`,
    `update_profile`, `set_private_details`, `enable_dashpay_keys`, `withdraw`; and `TxDraft` sends) are §4.1's
    "state transition, `TxDraft` send" rows.
  - `cancelled`: Lock won before the hand-off; nothing was sent, and a retry is safe after an unlock.
- **Leases and grants** (E0-04's `LeaseError`, mapped as E0-04 design rev2 §16.4 has it):

  | E0-04 condition | Code |
  |---|---|
  | `lease.locked`: a lock landed while `begin_flow` redeemed the grants, or Lock won a flow's permit | `platform.cancelled` |
  | the named lease was revoked by a revoking call (lock, close, passphrase change, wallet removed, wallet closed) | `platform.lease_revoked{cause}`; for a write not yet committed this is also E0-04's `Cancelled` outcome |
  | the named lease ended (`end_flow`, the 10-minute idle reaper) | `platform.lease_expired` |
  | `LeaseError::Parked` or `NeedsGrant`: the flow needs a fresh grant (its own key passed `key_until`, an unlock, a scope or passphrase change, a budget at 0, or a lease without the call's purpose) | `platform.needs_grant{purpose}` |
  | a budget refusal: the charge does not fit | `platform.grant_exceeded{purpose, needed, remaining}` |
  | an unknown, used or other-wallet grant or lease id | `platform.grant_invalid` |
  | the dispatch journal is unavailable, or its schema is newer (E0-04 §6.4) | `storage`, with `Notice{DispatchJournalUnavailable}` |

  `purpose` is the budget (`Funding`, `Credits`, `Spend`, `Crypto`); `needed` and `remaining` are in its unit.
  **The UI's reading of `lease_revoked{cause: Lock}`** (and of `cancelled`): nothing was sent by this call; unlock and
  start the flow again. After "Accept and pay" whose accept was `Sent`, that is E0-04's C8: "Accepted — payment
  cancelled. Pay now?".
  For a registration, `needs_grant` shows in the row as `waiting: Authorize` (vault unlocked) or `Unlock` (locked).
- **Credits.** A top-up spends Core duffs, so its shortfall is `credits.funding_insufficient{needed, available}` in
  duffs, not `platform.insufficient_credits` (credits, "top up your credits"). A cap hit is `platform.grant_exceeded`;
  below `top_up_min_duffs` is `credits.below_minimum{min}`. A withdrawal larger than the balance is
  `platform.insufficient_credits`.
- **Contacts.** `contact.ineligible{reason}` carries the `Eligibility` that refused (never `Ok`), so `NoDashPayKeys`
  leads to "Enable DashPay keys". `contact.scan_expired` is an expired `dapk` proof or a stale or used scan id: "this
  QR code has expired; ask for a new one".
- Refusals §3.6 names no code for, so that every DP task uses the same one:
  - a write from a watch-only wallet, or with a locked vault and no lease: `platform.signer_unavailable`;
  - a Platform write before SPV's masternode state has synced ("Waiting for the network to sync", §3.7), and money
    that would move on data verified only through the fallback (§2.2 rule 5): `platform.context_unavailable`;
  - `discard_registration` unless its funding reads `NotSent` or `None` (§4.1; the journal decides, not the
    phase), an unknown draft, candidate or faucet key id, an
    `initial_profile` avatar candidate without a URL: `invalid_argument`;
  - a call for a feature the build or the settings leave off (the faucet, Imgur): `platform.feature_off{feature}`;
  - an outcome that is unknown after a hand-off, a committed artifact the engine will resend, or a flow cancelled by
    Lock before one: `platform.broadcast_unknown`, `platform.will_be_sent`, `platform.cancelled` (above).
- Parameters: `grant_exceeded{purpose, needed, remaining}`, `needs_grant{purpose}`, `lease_revoked{cause}`,
  `broadcast_unknown{artifact}`, `will_be_sent{artifact}`, `insufficient_credits{needed, available}` (credits),
  `funding_insufficient{needed, available}` (duffs, both domains), `below_minimum{min}` (duffs),
  `feature_off{feature}`, `not_implemented{call}`, `keys_missing{purpose}`, `recoverable{draft}`, `invalid{rules}`,
  `ineligible{reason}`, `payment_locked{txid}`.

### 4.1 Funds committed and dispatch status (E0-04 §16.5 and §16.6, word for word)

Copied word for word from the E0-04 design (`docs/design/E0-04-grants-leases.md` at `723d9d3`, amendment 2). The
section and rule references inside (§2a.5, §5.5, §6.2, §6.5, H11, H16, I1, N-3, the J step) are E0-04's. Where
anything else in this contract reads differently, this subsection wins.

#### E0-04 §16.5 Two flags

- **`holds_key`** = `LeaseView.own_key && state ∈ {Active, AwaitingProof}`.
- **`funds_committed`**: true once a funding of the flow has committed and is not definitely unsent. It is the only
  definition, used by `LeaseView`, `RegistrationStatus` and the copy, and E0-08 copies it (N-3):
  - **Mode A:** an asset lock of the flow has its journal entry in `Committing`, `Dispatching`, `Ambiguous` or
    `PreFence`, or has a tracked row with no entry (unknown provenance, §6.5).
  - **Mode B:** a funding marker of the flow exists, or a tracked row matches the draft's own funding key, and the
    funding's status (§2a.5) is not `NotSent`. The marker is taken in the J step that takes the funding call's
    permit, and is durable before the call starts, so the flag turns on with the permit and survives a restart.
  - **Not committed:** no entry and no row (never registered); an `Unsent` entry, whether its lease is live (not
    committed yet) or its origin is dead (never can be); a `Revoked` entry. So a lock that Lock revoked and one
    still being built both read false, and "Lock to cancel" shows exactly while Lock still cancels.

  A contact request's or profile's `MaybeSent` transition does not set it: those are not funding.

#### E0-04 §16.6 `dispatch_status`, retry and discard

- `DashPay::dispatch_status(artifact: String) -> Result<Option<DispatchState>, PlatformError>`, async. `artifact`
  is what `broadcast_unknown{artifact}` and `will_be_sent{artifact}` carry: a txid, a state-transition hash, or a
  funding step id (`registration/<draft>/funding`, `topup/<id>/funding`). Mode B uses the step id whenever the
  engine never saw the funding's txid (§2a.5).
- `DispatchState`: `WillBeSent`, `MaybeSent`, `Sent`, `NotSent`. `None` means the engine has no entry.
- **What it answers**, first matching row wins:

  | Artifact | Evidence | Answer |
  |---|---|---|
  | any | an attempt or a Resend finished `Sent`, or the wallet has seen the transaction (its row is `InstantSendLocked`, `ChainLocked`, `Consumed` or `RecoveredFromChain`) | `Sent` |
  | asset lock, Mode A | entry `Dispatching`, `PreFence` or `Ambiguous` (the catch-up restores a lost row, §6.5) | `WillBeSent` |
  | asset lock, Mode A | entry `Unsent` with a live origin, or `Committing` | `MaybeSent` |
  | asset lock, Mode A | entry `Revoked`, or `Unsent` with a dead origin; Repair's self-spend ChainLocked | `NotSent` |
  | asset lock, Mode A | no entry, but a tracked row of any status | `MaybeSent` (unknown provenance, §6.5) |
  | funding, Mode B | §2a.5's table | as there |
  | state transition, `TxDraft` send | an attempt running or `possibly_out`; a resumable step's marker | `MaybeSent` |
  | state transition, `TxDraft` send | its tombstone (§5.5); a different transition this engine signed is proved executed in its nonce slot (H16) | `NotSent` |
  | any | none of the above | `None` |

- **What `None` means depends on the artifact's kind:**
  - **an asset lock** (a funding txid or step id): never registered. Its entry (in Mode B, its marker) precedes
    tracking and every transport (I1), and an entry outlives its row (§6.2). So with no entry and no row, nothing
    was or can be sent;
  - **a state transition or a `TxDraft` send:** unknown, read exactly as `MaybeSent`. Row-less entries and their
    tombstones live in one process (§5.5), so after a restart `None` is all the engine can say.
- **Who applies the asset-lock reading.** The artifact string does not carry its kind (a txid and a transition hash
  look alike), so:
  - **the host reads every `None` as unknown**, and only `Some(NotSent)` allows a retry it offers (H11). It loses
    nothing by this. It holds only artifacts from `broadcast_unknown` or `will_be_sent`, which the engine reports
    after their record exists, so an asset lock the host holds reads `None` only once its record was erased (a
    wiping removal, or a journal deleted by hand);
  - **the engine's funding gates apply the asset-lock reading.** They ask about a flow's funding step, whose kind
    they know: `discard_registration`, and DP1-02's "Register again", which discards first. With no entry, no
    marker and no tracked row matching the step (in Mode A by its txid, in Mode B by the draft's own funding key or
    the marker's), the step never built a lock, so the gate allows it.
- The `broadcast_unknown` retry rule is "after `DispatchResolved(NotSent)` or a `NotSent` status", never "after a
  re-query shows nothing was sent".
- `discard_registration` is allowed only when the registration's funding reads `NotSent` or `None`; otherwise
  `invalid_argument`. That implies `!funds_committed`, and it also refuses a live `Unsent`, whose flow is still
  building.
- `resume_registration` acts on any state.
- `finish_asset_locks` is not gated by the journal: it resumes committed locks.

## 5. Records

The shapes are in §3. What they mean, where the name does not say:

- **`DashPayStatus.NoIdentity{reason}`.** `None` means the user can join. `WatchOnly`: the wallet has no keys
  ("This wallet can't use DashPay because it has no keys"). `WaitingForSync`: Core or masternode sync has not finished,
  so Join is disabled (F1, DASHPAY §2.2 rule 5).
- **`StartupStatus`.** The library's seven `WalletStartupStatus` values plus our own `NotRun`, `Starting` and
  `IdentityUnsettled`: the bring-up ran with a locked vault and no signers ("identity unsettled", §3.2) and runs again
  at the first unlock.
- **`unverified`** on `IdentitySummary`, `ContactSummary`, `UserHit` and `ScannedContact`: the data was verified only
  through the trusted fallback (§2.2 rule 2) and still has a `dp_trust_unverified` row. "As of" times for offline
  data come from `sync_status().last_pass`. `Counterparty` and the inviter in `InvitationStatus::Valid` carry no flag:
  they are display data, and DP3-04's money-move gate checks `dp_trust_unverified` itself before any payment, whatever
  the record says.
- **`QuorumSource`.** `Spv`; `TrustedFallback` (before masternode sync, reads only, results unverified); `Trusted`
  (the developer toggle, or the degraded mode of §2.2 if E0-10a fails).
- **`RegistrationStatus`** mirrors a `dp_registration` row. `phase` is the §3.4 state.
  - `waiting` says what a parked or slow flow waits for, and so which line the UI shows: `Unlock` ("Unlock to finish";
    the flow parked keyless, and `resume_registration` needs a grant), `Authorize` ("Confirm to finish": the vault is
    unlocked but the flow's grant died with an unlock, a scope or passphrase change, or its budget ran short; the
    resume needs a grant), `Sync` ("Waiting for the
    network to sync": a restored row held until SPV sync, §3.4 Restore rule 1, or a write held by §2.2 rule 5),
    `InstantSend` and `ChainLock` (the proof wait; E0-04 `Parked{ProofWaiting}` is `ChainLock`), `Network` (DAPI or
    the peers unreachable; retried on reconnect). `None` while the flow runs, and after it ends. A grant is needed
    exactly for `Unlock` and `Authorize`: when a ChainLock-parked flow's proof arrives and it needs keys again, the
    row moves to `Unlock` (vault locked or mixing-only) or `Authorize` (unlocked), so the host never prompts during
    the wait and never has to guess. A rebind with "require authentication for every payment" on also moves the row
    to `Authorize` or `Unlock` (E0-04 DEC-67). There is no separate `needs_grant` field.
  - `holds_key`: `LeaseView.own_key && state ∈ {Active, AwaitingProof}` (E0-04 §16.5). The copy is E0-04
    §16.10's, chosen in its order: C2 ("Lock stops new signatures; a transaction already signed may still be sent")
    while a library call of the flow runs; otherwise C3 ("Funds committed — finishing. Lock to stop; you'll finish
    after you unlock") when `funds_committed` and a further signature is needed; otherwise C1 ("Registration in
    progress — Lock to cancel").
  - `funds_committed`: E0-04 §16.5's definition, copied word for word in §4.1 and the only one, shared with
    `LeaseView.funds_committed` and the copy: true once a funding of the flow has committed and is not definitely
    unsent. Mode A means the journal states and the entry-less tracked row listed there, and Mode B (without the
    platform PR) a funding marker or a tracked row matching the draft's own funding key. A live `Unsent` entry and a
    `Revoked` one are not committed, so "Lock to cancel" shows exactly while Lock still cancels. While it is true,
    discard is refused ("Funds committed — finishing", F2).
  - `contest_ends_at` while `Contested`; `failure` when `Failed`: the phase it stopped in, the code, `retryable`, and
    `needed`/`available` for the `*funding_insufficient*` and `insufficient_credits` codes.
- **`RegistrationFunding`.** `CoreBalance`, `Invitation{link_id}` (a `stash_invitation` id),
  `ExistingIdentity{identity}` (the id goes to the row's `identity` column), and `FaucetAssetLock{key, proof}` for
  developer builds:
  - The faucet's `POST /api/asset-lock-proof` takes only a compressed public key and returns the `assetLockProof`
    (hex), the txid and the credits. So the engine keeps the key: `prepare_faucet_lock` derives a fresh registration
    asset-lock key (`m/9'/c'/5'/1'/…`, the `PlatformFunding` scope) and returns its id and public key; the host posts
    the public key to the faucet; `FaucetAssetLock{key, proof}` carries the id and the proof. No private key crosses
    the facade, so the variant needs no `BearerSecret`, and the legacy `/api/faucet` route that returns keys is not
    used.
  - The lock is not one this wallet built or tracks, so DP1-02 funds it through the **invitation-claim path** (the
    proof plus a key the engine holds, as `claim_invitation` does), not through
    `AssetLockFunding::FromExistingAssetLock`, which needs a tracked lock and re-derives its own credit-key path.
  - The outpoint comes from the proof. DP1-02 writes it to `asset_lock_outpoint` in the one canonical text of the §3.6
    note (lower-case hex txid, `:`, decimal vout) and refuses a proof it cannot parse with `invalid_argument`.
- **`InitialProfile`** is the profile entered at Draft, kept in `dp_registration.initial_profile` until
  `ProfileCreated`, across restarts and restores. `avatar_candidate` must name a candidate whose `url` is set: one
  prepared from `Url` or `Gravatar`, or a `File` candidate after `upload_avatar`. The engine stores that URL with the
  candidate's hash and fingerprint, never the candidate id, and **registration never uploads**: no third-party call in
  the funding flow, no image published before funds are committed, and a quote that does not upload. A candidate
  without a URL is `invalid_argument`.
- **`RegistrationQuote`.** `total_duffs = lock_duffs + fee_duffs`; `grant` is the `PlatformOp{max_duffs, max_credits}`
  to ask for: `max_duffs` covers the lock and fee, `max_credits` the identity creation, the name and the profile.
- **`GrantRequest`** (in every quote, and from `grant_request`): the caps of the `PlatformOp` grant an action needs,
  computed with the engine's own fee bound (E0-04 §4.2), so a host never sizes `max_credits` from `cost_table()`. The
  engine charges no more than it quoted; a quote that went stale (a fee change) is `platform.grant_exceeded`, and the
  host quotes again.
- **`FlowKind`** names what a lease is for; `AcceptAndPay` carries both the `PlatformOp` grant and the payment's
  `Spend` grant. **`DispatchState`** and **`DispatchResolved`** are the journal's view of a handed-off artifact (§2.11,
  §6). **`LeaseView`**, `LeaseStateView` (`Active`, `AwaitingProof`, `Parked{reason}`, `NeedsGrant`,
  `Revoked{cause}`, `Ended`) and `BudgetView{purpose, ceiling, spent}` are E0-04 §4.6's view, owned here
  (E0-04 §16.1); `AwaitingProof`'s deadline is `key_expires_in_secs`. **`RevokeCause`**: `Lock`, `Close`,
  `PassphraseChange`, `WalletRemoved`, `WalletClosed`. **`DispatchResolved.wallet_id`** and **`LeaseView.wallet_id`**
  are lower-case hex, as `WalletId` displays.
- **`UsernameCheck.rules`** is the whole checklist with a pass mark per rule. `NameAvailability::Invalid{rules}` and
  `NameError::Invalid{rules}` list only the broken ones.
- **`Relation`** is relative to one of the wallet's identities: the `identity` argument, or the main identity for
  `search_users`, `resolve_user` and `verify_scanned`.
- **`ChannelState`.** `NotEstablished`; `SetupPending` (contact crypto waits for an unlock); `Ready`; `Broken`
  (`payment_channel_broken`).
- **`RequestOutcome`.** `Pending{request}` after a send; `Established{request}` when both directions exist (an accept,
  or a send to someone whose request was already in). `request` is the contact-request document id.
- **`ScannedContact.scan`**: set when the payload carried a `dapk` auto-accept proof; pass it to `send_request`.
- **`IdentityKeyInfo.contract_bound`** and **`contract_bound_document_type`**: keys 4–5 are bound to the DashPay
  contract's `contactRequest`.
- **`EventKind`** covers the §3.5 journal: `UsernameRegistered`, `ContestWon`, `ContestLost`, `ContestLocked`,
  `RequestReceived`, `RequestAccepted`, `ContactEstablished`, `PaymentReceived`. **`DashPayEvent.reference` per kind:**
  the label for the name kinds (`UsernameRegistered`, `Contest*`: "You won @alice"), the contact-request id for the
  request kinds, the txid for `PaymentReceived`. `contact` is empty for the name kinds. The journal's `ref` column holds
  the same value.
- **`ProfileEdit`** is the whole new profile. `AvatarChange`: `Keep`, `Remove`, or `Set{candidate}` with a
  `prepare_avatar` candidate (after `upload_avatar` when `needs_upload`).
- **`AvatarSize`.** `Small` is 128 px, `Large` 256 px. `AvatarImage.png` is the engine-re-encoded thumbnail, never
  the original bytes, as base64 in JSON.
- **`CostTable`.** Credits per action, `credits_per_duff` (1000), the top-up minimum in duffs and the low-credit
  threshold. **`TopUpQuote`**: `total_duffs = duffs + fee_duffs` is what the grant must cover. **`WithdrawQuote`**:
  the credits taken, the fee in credits and the duffs expected.
- **`Counterparty`** is the record history and transaction records gain as `counterparty` (DP3-02); here it is also
  the inviter in `InvitationStatus::Valid`.

## 6. Outside the facade

Parts of DASHPAY §3.6 that change existing engine types. They land with the tasks named, not in E0-08, because each
changes code paths or bindings that would otherwise need behaviour now:

| Item | Task |
|---|---|
| `TxDraft` gains `Recipient::Contact{identity, contact, amount, subtract_fee, note}` | DP3-01 |
| History and transaction records gain `counterparty: Option<Counterparty>` | DP3-02 |
| `NoticeCode` gains `PlatformTrustMismatch` (E0-10b) and `DashPayStartupIncomplete` (E0-05). `dw-ffi` maps `NoticeCode` one to one, and the Swift bindings are frozen until E0-13 | E0-05, E0-10b, E0-13 |
| `EngineEvent::Platform{network, wallet_id, change}` (§3.5) | E0-06 |
| `EngineEvent::DispatchResolved{network, resolved: DispatchResolved}`, sent when a provisional outcome settles. Its sources are E0-04 §4.6's: a Resend is accepted, or the wallet sees the transaction (`Sent`); a reload refuses and cleans up an `Unsent` row (`NotSent`); a row-less artifact settles definitely unsent (`NotSent`); Mode B's derived status moves (E0-04 §2a.5); H16's evidence settles a row-less transition. Its `artifact` is a txid, a state-transition hash or a funding step id (`registration/<draft>/funding`, `topup/<id>/funding`). DP1-02 then moves a registration row back to retryable, and the payment and top-up UIs clear their "may have been sent" or "will be sent" state. With E0-04's `LeaseChanged` and `LockProgress`. The payload record exists now (§3); the variant waits because `dw-ffi` maps `EngineEvent` one to one | E0-04 (P2), E0-13 |
| `NoticeCode` gains `DispatchRecordMissing`, `UnscopedDispatch` and `DispatchJournalUnavailable` | E0-04 (P2), E0-13 |
| `EngineEvent::LeaseChanged{network, lease: LeaseView}` and `LockProgress{network, phase}`, engine-side until E0-13 | E0-04 (P2a), E0-13 |
| m1's `SendError` gains `send.cancelled` for a contact payment that Lock refused (E0-04 §16.11) | DP3-01 (P5 reuses it for M1 sends) |
| Tools ▸ Repair "Unrecorded asset locks" (E0-04 §6.5, §16.10 C9) gets a facade call; engine-internal until then | DP1-02 |
| **The error mapping**: one shared `From` impl per source into `PlatformError`, in `errors.rs`, for `PlatformWalletError`, `dash_sdk::Error`, `EngineError` and `VaultError` (§3.1), with `Internal{detail}` as the fallback. `EngineError::InsufficientCredits` (code `insufficient_credits`, from E0-01) maps to `platform.insufficient_credits`, the code hosts see from this facade. Every later task maps through these impls and extends them in place. **Done (E0-05, Contract-Version 5):** a locked or mixing-only vault, a missing credential and a watch-only wallet (`NoSecret`) are `platform.signer_unavailable`; DAPI that does not answer is `platform.unavailable`, `TimeoutReached` and the seed-binding deadline `platform.timeout`, a proof error `platform.proof_invalid`, a context-provider error `platform.context_unavailable`; persister and vault-store errors `storage`. | **E0-05** (W3): its `sync_now` is the first facade body that returns real library errors, and it lands before every DP task that does (ROADMAP E0-05 row) |

## 7. Decisions on DASHPAY §3.6

§3.6 is a sketch. Where it was ambiguous or contradictory, this contract decided as follows (review DW-E0-08 r1 agreed
with 1–21 and changed 5, 8, 9, 10, 12 and 16; the result is below):

1. **Every call returns `Result`.** The sketch has sync reads with bare return types, but a stub must return
   `platform.not_implemented`, and the finished reads need `network_not_open` and `wallet_not_found`.
2. **`check_username` returns `Result<UsernameCheck, NameError>`**, for the same reason. A bad label is still a
   successful `valid: false` result.
3. **`identity.*` has no calls of its own.** The sketch's identity calls return `PlatformError`, while the code table
   lists an `identity.*` domain. `IdentityError` is a leaf enum inside `PlatformError::Identity`, so the sketch's
   signatures stand and every domain can return `identity.keys_missing{purpose}`.
4. **`platform.*` reaches every domain.** Each domain enum wraps `PlatformError`, and `RegistrationError` also wraps
   `NameError` and `InvitationError`, whose refusals its own flows raise (§4).
5. **The m1 common codes** live in `PlatformError`, unprefixed. §3.6 does not list them, but m1 §4 makes them common
   to every domain.
6. **`CreditsError` has its own codes**: `credits.funding_insufficient{needed, available}` (duffs) and
   `credits.below_minimum{min}`, since a top-up spends duffs and `platform.insufficient_credits` is the wrong unit and
   the wrong copy. Top-up and withdraw gain quote calls, so the host can size the grant.
7. **`eligibility` takes `identity`.** The sketch's `eligibility(contact)` cannot answer `IsSelf`, `AlreadyContact` or
   `Pending*` without knowing which of the wallet's identities asks, and every other contact call takes it.
8. **`search_users`, `resolve_user` and `verify_scanned` keep the sketch's arguments**; their `relation` is relative to
   the main identity.
9. **The `contact.self` code's variant is `ContactError::IsSelf`**, because `Self` is a Rust keyword.
10. **`keys_missing{purpose}`** is typed as `KeyPurpose`, the enum `IdentityKeyInfo.purpose` uses.
11. **`FaucetAssetLock` is `{key, proof}` and carries no secret.** The faucet's proof API takes a public key, so the
    engine derives and keeps the key (`prepare_faucet_lock`). DP1-02 funds it through the invitation-claim path, not
    `FromExistingAssetLock` (§5).
12. **Records the sketch only names** (`SyncPassReport`, `FinishReport`, `ContestStatus`, `CostTable`, `TopUpOutcome`,
    `WithdrawOutcome`, `ActivityPage`, `AvatarCandidate` and the rest) are defined from DASHPAY §3.2–§3.5, §4 (the view
    models) and §5 (the feature catalogue). Their owners may extend them under the rule at the top of this file.
13. **The notices** listed in §3.6 are not added to `NoticeCode` yet (§6).
14. **The invitation calls are on `NetworkSession`.** The sketch puts them on the wallet-bound `DashPay`, but F18 takes
    a link before any wallet exists, and the vault is per network. Before a vault exists the link is a 0600 file that
    moves into the vault at creation; `pending_invitations` and `forget_invitation` serve the replay and the dismissal
    (§2.9).
15. **`send_request` takes `scan: Option<String>`.** The sketch has no way to pass a `dapk` auto-accept proof from
    `verify_scanned` to the request (§3.1 `contacts.rs`: "dapk scan"). A stale id or expired proof is
    `contact.scan_expired`, and `contact.ineligible` carries its `reason`.
16. **`events` and `registrations` are async.** They read app.sqlite (a paged journal and the registration rows), which
    m1 rule 3 keeps off the caller's thread.
17. **`initial_profile` is an `InitialProfile`, not a `ProfileEdit`,** and its avatar must already have a URL.
    `ProfileEdit`'s avatar is an in-memory candidate id, which would mean nothing in a row restored after a restart or
    on another machine (§3.4), and an upload inside registration would add a third-party call to the funding flow.
18. **Bearer credentials are `BearerSecret`**, including `stash_invitation(link)` and `verify_scanned(text)`, which the
    sketch types as `String` (§3.8: never logged). Bindings pass them as bytes, errors never quote them, and `dwcli`
    reads them from stdin (§1).
19. **`StartupStatus` gains `IdentityUnsettled`** for §3.2's locked-vault bring-up, which none of the nine values
    named there covers.
20. **Records gain `unverified`**, which §2.2 rule 2 requires ("marked unverified") but the sketch does not carry.
21. **Refusal codes** §3.6 leaves open are assigned in §4, including `platform.broadcast_unknown` and
    `platform.cancelled` for E0-04's `MaybeSent` and `Cancelled`.
22. **`RegistrationStatus` says why a flow waits** (`waiting`, `holds_key`, `funds_committed`), so the resume UX and
    the one-prompt rule of §2.6 need no new field later; `failure` keeps a failed code's parameters.
23. **The records are split into the §3.1 domain files**, each with its own `impl DashPay` block, and re-exported by
    name, so parallel DP tasks do not conflict and a `pub` helper never becomes API silently.
24. **Byte payloads are base64 strings in JSON**, which fixes the JSON shape E0-09 prints and Tauri carries (a 5 MiB
    avatar would otherwise be about 21 MB of number arrays).
25. **The engine and SDK error mapping has one owner, E0-05**, the first task that returns real errors through the
    facade (§6; ROADMAP E0-05 row).

**From E0-04** (version 2 from its design review r1, findings 2 and 4; version 3 conformed to its design rev2 §16,
which is authoritative for this surface; version 4 to the final design at `723d9d3`, amendment 2, copying §16.5 and
§16.6 word for word in §4.1). E0-04 adopted this contract's names (`will_be_sent`, `GrantAction` with
`identity`, `DispatchState`'s four values, the `Unlock`/`Authorize` waits, `resolution`). A later E0-04 revision that
renames or reshapes them takes the next version bump:

26. **Lease handle.** `NetworkSession.begin_flow(wallet_id, flow, grants) -> lease id`, `end_flow(lease)` and
    `leases() -> Vec<LeaseView>`. A lease id is accepted only by calls of its wallet for a purpose it carries
    (`grant_invalid` and `needs_grant{purpose}` otherwise); the idle reaper ends a vault-key lease after 10 minutes
    (§1, §2.11). `FlowKind` is E0-04 §16.1's closed list; it has no `Discovery`, because the identity scan uses a
    grant, not a lease: `discover_identities` takes an `IdentityScan` grant id only, and a lease id there is
    `platform.grant_invalid`. The async `NetworkSession` calls take `self: &Arc<Self>`, the engine's receiver for calls
    that run on its runtime.
27. **Grant sizing.** `GrantRequest{max_duffs, max_credits}` in `RegistrationQuote`, `TopUpQuote` and
    `WithdrawQuote`, and `grant_request(identity, action)` for the writes without a quote; the engine charges no more
    than it quoted (§5).
28. **`RegistrationWait::Authorize`** ("Confirm to finish") for a flow that needs a new grant on an unlocked vault;
    a grant is required exactly for `Unlock` and `Authorize`, and a ChainLock-parked flow moves to one of them when it
    needs keys again (§5).
29. **Lease and grant codes.** `platform.needs_grant{purpose}`, `platform.lease_revoked{cause}`,
    `platform.lease_expired`, and `platform.grant_exceeded{purpose, needed, remaining}` (it had no parameters);
    `lease.locked` maps to `platform.cancelled`, an unavailable journal to `storage`; `lease_expired` is only for
    `end_flow` and the idle reaper, and an own key past `key_until` is `needs_grant` (§4 table). `RevokeCause`
    includes `WalletClosed`.
30. **`platform.will_be_sent{artifact}`**, distinct from `platform.broadcast_unknown{artifact}`: a committed artifact
    the engine will resend; never retried or discarded (finding 2). Both carry the artifact id.
31. **Retry, discard and `None` follow E0-04 §16.6 word for word** (§4.1; reviews DW-E0-08 r2 N-1 and N-2, r3 C-1).
    `dispatch_status` returns `Option<DispatchState>` with four values, and `None` means the engine has no entry.
    The host reads every `None` as unknown; only the engine's funding gates read an asset lock's `None` as never
    registered, and only with no entry, no marker and no tracked row matching the step (in Mode A by its txid, in
    Mode B by the draft's own funding key or the marker's). Version 3 promised a host-visible `Some(NotSent)` for a
    missing asset-lock or funding entry; that is withdrawn, because a tracked row that survives a journal loss would
    have allowed a second funding. `resume_registration` acts on any state; `finish_asset_locks` is not gated.
    `funds_committed` is §16.5 word for word (r3 C-2): a live `Unsent` entry is not committed.
32. **`DispatchResolved`**, the event payload for a provisional outcome that settles, with E0-04 §4.6's sources and a
    txid, state-transition hash or funding step id as its artifact; the `EngineEvent` variant and the three dispatch
    notices land with E0-04 P2 and E0-13 (§6).
33. **`holds_key`** is `LeaseView.own_key` with the lease `Active` or `AwaitingProof`, and its copy depends on
    `funds_committed` (§5, finding 3).
34. **`resolve_payment_lock` needs positive evidence for `NotSent`** (review r3 G-1, C-7): in this process
    `dispatch_status(txid) == Some(NotSent)`, or a ChainLocked conflicting spend of one of the payment's inputs; never
    "not found on chain or in the wallet". `LockResolution::Unknown` reads like `MaybeSent` and `None`, and the lock
    stays (§2.5).
35. **Names (DP1-03, versions 6 and 7).** `UsernameRule::NoDoubleHyphen`: dash-platform-queries' `is_valid_username` and
    the iOS register path refuse `--`, so the checklist does too. `set_main_name` and `main_name` are new: the main
    name is a per-identity pick in `dp_prefs` that sync never rewrites (#4978); a pick the identity no longer owns is
    skipped, not deleted. Review r1 (no surface change): `register_name` budgets a conservative fee bound per
    document transition (E0-04 §4.2 Q7, until DP1-06's cost table) plus the contest fund against the grant and the
    balance, and refuses before consuming the grant; a new contender needs a known join deadline; the
    oldest-name fallback uses DP1-05's acquisition times. Review r2: Platform is the only source of name state. A
    label is stored before its write only as a record that the write may be in flight; it is never owned, shown or
    pickable. Retries derive their kind from Platform's state at the time; any definitive answer (owned,
    contending, taken, locked, a closed contest) clears it, and a refusal also drops the library's provisional
    copy. Never the pick. On DP1-05 (R5): one main-name rule and one evidence filter, applied by `main_name` and
    `identities()` alike; every main-name row goes through DP1-05's writer. Review r3 (DEC-124): name visibility
    comes from one app-database read of the main-name rows per call, never the choices cache; a refusal for good
    moves a pending label to a refused row before it returns. Review r4 (DEC-129): the rows come from one SELECT, and
    the older snapshot is default-deny. Review r5 (DEC-138, version 7, superseding that default-deny): anything on the
    fallback path that looks like ownership evidence (the snapshot's marketplace rows, a retained pick) is as old as
    the snapshot, so the older snapshot shows no names and no main name, and `IdentitySummary` gains
    `names_updating: bool`, `true` exactly then, so the UI shows "updating". The markers govern the live list only.
