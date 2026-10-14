# M4 DashPay engine contract (`dw-engine` facade)

Contract-Version: 21

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
DP1-01's `keys_policy.rs`, E0-05's `bringup.rs`, `runtime.rs`, `runtime_tests.rs` and `startup_status.rs`, which
run the bring-up and the loops that `startup.rs` reads, DP1-03's `names_net.rs` and `names_tests.rs`, E0-06's changeset tap `journal.rs` with `journal_tests.rs` and its trust records `provenance.rs`, DP2-01's contacts read model `contacts_model.rs` with its usernames `contact_names.rs` and `contacts_tests.rs`, DP2-03's user search, links and scans `users.rs` with `users_tests.rs`, DP2-04's private details `private_details.rs` with `private_details_tests.rs`, DP2-05's bell session tests `notifications_tests.rs`, DP1-06's cost table `credit_costs.rs`, balance refreshes `balances.rs` and top-up flow `topup.rs` with `topup_tests.rs`, DP1-04's contest watch `contest_watch.rs` with `contest_watch_tests.rs`, and DP3-02's contact payment attribution `attribution.rs` with `payments_tests.rs`) are in its `NOT_FACADE` list; a new file must join one of the two lists. Each domain file
holds its records and its own `impl DashPay` block, so parallel DP tasks edit different files, and no `impl DashPay`
lives anywhere else in the crate. A record the facade returns is `pub` and re-exported by name from `mod.rs`; a helper
type is `pub(crate)`.

| File | Owner (ROADMAP) | Calls |
|---|---|---|
| `dashpay.rs` | E0-08 | `NetworkSession.dashpay`, `wallet_id`; `BearerSecret` |
| `startup.rs` | E0-05 bring-up | `status`, `sync_status`, `sync_now` |
| `identity.rs` | DP1-01, DP1-05, DP6-01 | `identities`, `set_main_identity`, `identity_detail`, `refresh_balance`, `discover_identities` |
| `registration.rs` | DP1-02 | `registration_quote`, `start_registration`, `start_registration_past_unknown_funding`, `registrations`, `resume_registration`, `discard_registration`, `finish_asset_locks`, `prepare_faucet_lock` |
| `names.rs` | DP1-03, DP1-04, DP2-03 | `check_username`, `name_availability`, `register_name`, `set_main_name`, `main_name`, `contest_status`, `search_users`, `resolve_user` |
| `contacts.rs` | DP2-01…DP2-04 | `contacts`, `contact`, `pending_setup_count`, `eligibility`, `send_request`, `accept_request`, `ignore`, `unignore`, `set_private_details`, `enable_dashpay_keys`, `my_user_link`, `verify_scanned` |
| `payments.rs` | DP3-01, DP3-02 | `payment_lock`, `resolve_payment_lock`, `contact_activity`, `frequent_contacts` |
| `notifications.rs` | DP2-05 | `events`, `unread_count`, `mark_read` |
| `profile.rs` | DP4-01, DP4-02 | `profile`, `profile_limits`, `prepare_avatar`, `avatar_upload_available`, `upload_avatar`, `update_profile`, `avatar` |
| `credits.rs` | DP1-06, DP6-02 | `cost_table`, `top_up_quote`, `top_up`, `pending_top_ups`, `resume_top_up`, `resolve_top_up`, `withdraw_quote`, `withdraw` |
| `invitations.rs` | DP5-01, DP5-02 | `NetworkSession.stash_invitation`, `invitation_status`, `pending_invitations`, `forget_invitation` |
| `flows.rs` | E0-04 (design rev2 §16, §7) | `NetworkSession.begin_flow`, `end_flow`, `leases`, `grant_request`, `dispatch_status` |
| `errors.rs` | E0-05 (the mapping, §6) and each domain's owner | the error enums of §4 |

## 1. Conventions (changes to m1-engine.md §1)

| Topic | Rule |
|---|---|
| Ids | Wallet: `WalletId` (the facade is bound to one wallet). Identities and contacts: Base58 `String`. Txids: lower-case hex. Drafts, candidates, scans, faucet keys, invitation links, activity cursors: opaque `String` ids the engine issued, each 1–64 characters of `[0-9a-z_-]` (`dwcli dashpay session` checks them by that form), as is the `<id>` of a funding step id (`registration/<id>/funding`, `topup/<id>/funding`). |
| Amounts | Duffs and credits are `u64`; the field name or doc says which. |
| Every call returns `Result` | Including the in-memory reads DASHPAY §3.6 sketched without one (`status`, `identities`, `contacts`, …) and `check_username`. A stub has to return `platform.not_implemented`, and the finished reads need `network_not_open` and `wallet_not_found`. |
| Sync and async | A sync call reads in-memory state only (m1 rule 3): no SQLite on the caller's thread. An async call touches the network or persistence; once implemented it runs on the engine runtime (`NetworkSession::on_runtime`), so any executor may poll it. The sync reads are served from caches their owners keep in the session's Platform runtime, filled at bring-up and refreshed on each `Platform` signal: the status snapshot (`status`, `sync_status`; E0-05), the identity list with the main identity (`identities`, `profile`; DP1-05), the contacts read model (`contacts`, `contact`, `pending_setup_count`, `frequent_contacts`, `my_user_link`; DP2-01), the payment locks (`payment_lock`; DP3-01), the unread counts (`unread_count`; DP2-05, kept by the changeset tap, which recounts after each journal write) and the static tables (`profile_limits`, `cost_table`, `avatar_upload_available`). The contacts read model (DP2-01) reads the library's memory on each call instead, under a non-blocking read lock, and keeps the result as a snapshot that answers only while a writer holds that lock (§2.4). Paged or unbounded reads (`events`, `registrations`) are async. |
| Static tables before Platform answers | `profile_limits` returns the built-in 25 / 140 until the DashPay contract is fetched; `cost_table` prices the SDK's current protocol version (the seed floor until Platform's proved metadata raises it). Neither waits for the network. |
| Grants and leases | `grant: String` is either a grant id from `Vault.authorize` or a lease id from `NetworkSession.begin_flow` (§2.11). The purposes, caps and budgets are E0-04's (DASHPAY §2.6). A flow that spans calls ("Accept and pay": `accept_request`, then the payment's `TxDraft.prepare`, §2.3) redeems its grants once with `begin_flow`, from one credential prompt, and passes the lease id to each call, so the 120 s grant lifetime cannot run out between them; `end_flow` releases it. **A lease id is accepted only by a call of the lease's own wallet, and only for a purpose the lease carries**: another wallet's lease is `platform.grant_invalid`, and a lease without the purpose the call needs (a `ProfileEdit` lease passed to `top_up`) is `platform.needs_grant{purpose}`; a write that names the flows whose leases it takes refuses a lease of another flow with `platform.grant_invalid` (§7 item 41, E0-04 amendment 3). The idle reaper ends a vault-key lease after **10 minutes** with no call, no permit and no running flow task; an own-key lease's key is dropped at its `key_until` (the flow then needs a grant again), so a host that abandons a sheet leaks nothing. Every write's quote says what to authorize (`grant: GrantRequest`, or `grant_request(identity, action)`), and the engine charges no more than it quoted. Where Platform meters the fee against its state (IdentityUpdate, `contactRequest`), the quote is a bound and the charge is measured afterwards (DEC-195, §7 item 41). |
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
  never the panic's message. A facade error for a panicking engine task is `internal` with the detail
  `TASK_PANICKED`, and counts the same. In one-shot mode the panic is the command's one line, with exit status 1. In
  a session the panic answers the request, then a health probe runs: the session must be open, the vault must answer
  without panicking, the wallet list without an error, and the engine runtime must run a task, all within 10 s. The
  session goes on if the probe passes; otherwise it ends with `session_poisoned` as its last line.
- **How dwcli ends the engine (DEC-110).** Any panic in any engine task marks the engine poisoned for the process
  (`dw_engine::engine_poisoned`), whatever became of the task's result. Before stopping SPV (`--spv`) or shutting the
  engine down, dwcli probes a poisoned engine as above. Every SPV stop and the engine's shutdown run under a deadline:
  `--shutdown-timeout SECS` (or `DWCLI_SHUTDOWN_TIMEOUT`), 10 s by default; a thread the deadline needs that the OS
  will not start counts as a missed deadline (DEC-118). A failed probe, a missed deadline or a poisoned session
  abandons the engine: dwcli neither stops SPV nor shuts the engine down (either may never finish), wipes the
  passphrase it read, then writes the lines it owes and exits with status 1, whatever the command's own result; stderr
  says why. The command's line is unchanged: a write that went through still reads `ok: true`. The engine is never
  dropped on dwcli's own thread (its drop may wait for wedged workers); it and the key material it holds are left to
  the OS. The last lines are written best effort: a failed write or flush (a closed pipe, a full disk) changes neither
  the exit status nor the skipped shutdown. A healthy shutdown takes well under the deadline, and its exit status is
  the command's.
- **Callers must drain stdout.** As any CLI, dwcli blocks on a write to a stdout that is full and that nobody reads,
  its last lines included, and does not exit until the write goes through or fails (DEC-106). A DashPay command wipes
  the passphrase dwcli read before its last line, healthy or abandoned, so a blocked write holds no copy of it. A
  panic on dwcli's own thread is answered (`internal`) and the engine still ends through the shutdown and its deadline
  above.
- **Follow-ups (Opus-high review of E0-09).** (F3) A session probes the engine only after a request whose error is a
  panic (`TASK_PANICKED`); an engine-task panic that surfaces as another error, or in a task nobody awaits, sets the
  poison flag but is probed only at the session's end. A later round may snapshot the flag around each request and
  probe when it flips. (F4) `--passphrase-file` naming a pipe (`/dev/stdin`, `<(…)`) is read through `std::fs::read`,
  whose probe and growth leave unwiped copies; read it into a capped zeroizing buffer as bearer inputs are.
- `--no-platform` (a chain with no Platform) refuses every DashPay command, a session included, with
  `platform.feature_off{feature: "platform"}` before anything is opened.
- Arguments clap refuses exit with status 2 and no JSON line. stderr names the error kind, the arguments involved where
  clap names them, and the usage, never a refused value. `setup` replaces the code of what failed before the command
  ran (a storage error opening the network, for example); `message` says which.

## 2. Calls

Every call's status today: **stub**, except §2.10, DP1-05's `identities`, `set_main_identity` and
`discover_identities` (§2.1), DP1-03's names calls in §2.3 (`check_username`, `name_availability`,
`register_name`, `set_main_name`, `main_name`), DP2-01's contacts read model in §2.4 (`contacts`, `contact`,
`pending_setup_count`), and DP2-03's user search, links and scans (`search_users` and `resolve_user` in §2.3,
`my_user_link` and `verify_scanned` in §2.4), DP1-02's registration calls in §2.2, and DP1-06's `refresh_balance` (§2.1) and credits calls in §2.8 (not the withdrawals). Each row says whether its call is implemented. Kind is `sync`, `async` or `free, pure` (§1).

### 2.1 Status and identity

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `status()` | sync | The banner state (F1) shared by the Home card, the chip and the Contacts empty state: `NoIdentity{reason}`, `Registering{draft}`, `ContestPending{identity, label, ends_at}`, `Ready{main}`, `StartupIncomplete{startup}`. | `PlatformError` |
| `sync_status()` | sync | Tools ▸ Information's DashPay card (F23): startup status, last pass, pending contact crypto, loops, quorum source. **works** (E0-10b): `last_pass` is `None` until sync passes report one. | `PlatformError` |
| `sync_now()` | async | Runs one DashPay pass now and reports it. | `PlatformError` |
| `identities()` | sync | The wallet's identities by index: names (the library's order), main name, credit balance (`None` = unknown) with `balance_as_of` (the Platform block time, ms, of the balance shown, once a refresh read it) and `balance_stale` (it may be behind: spent by a write since its refresh, or neither refreshed by this process nor stored by Platform in the last ten minutes; the engine refreshes it in the background and sends `Platform{Identities}` when the flag may have changed, §7 item 44), and its `credit_level` against the cost table (`None` while the balance is unknown; DP1-06), whether the DashPay keys 4–5 exist, profile. `is_main` marks the `dp_main_identity` choice while the wallet still has that identity, else the lowest index. `names` and `main_name` are as `main_name(identity)` (§2.3) has them: names the identity owns by Platform's evidence (not a label in a contest or one whose write may be in flight), and its pick while owned, else the temporary or won contested name, else the name acquired first; `None` while it owns none. Read from the library's memory, or the last snapshot while a sync pass writes it; which names show comes from the app database's main-name rows, read in one statement on each call (a failed read fails the call), never from a cache. From the live list, a label whose write may be in flight or that Platform refused is hidden. From the older snapshot, `names` is empty, `main_name` is `None` and `names_updating` is `true` (show "updating", not "no name"); the other fields still come from it. `names_updating` is `false` on a live read. **Implemented (DP1-05).** | `PlatformError` |
| `set_main_identity(identity)` | async | Writes `dp_main_identity`; one of the wallet's identities, else `identity.not_found`. **Implemented (DP1-05).** | `PlatformError` (`identity.not_found`) |
| `identity_detail(identity)` | async | The summary plus revision and public keys. | `PlatformError` (`identity.not_found`) |
| `refresh_balance(identity)` | async | Fetches the credit balance, proved, and stores it with its block time (an older answer never overwrites a newer one), so `identities()` shows it; a spend marked before stays `balance_stale` until a balance of the spend's end or later is read (§7 item 44); `None` = not found on Platform yet. Needs no key: works with the vault locked. **Implemented (DP1-06).** | `PlatformError` (`identity.not_found`) |
| `discover_identities(grant)` | async | Same-seed discovery (DP1-05) past the highest identity index on file; returns the number of identities it stored. What it finds then gets the rest of a recovery in the background: a bring-up of the wallet (DashPay pass, contact accounts) and a names pass, at once while SPV runs, otherwise at the next start. Also forgets Platform's earlier proof that the seed owns none, so the next start's bring-up looks again (DP6-01's "find"). `grant` is an `IdentityScan` grant id only (E0-04 §3.2); a lease id there is `platform.grant_invalid`. A grant confirmed with the passphrase on a locked or mixing-only vault scans through its key hold (E0-04 §3.5). A vault lock during the scan is `platform.cancelled`, as is the wallet's removal, unload or a failed restore's rollback, which waits for the call to end and admits no new one until it is done; identities the scan stored before a lock or an error still get their bring-up and names pass; running out of time with nothing stored is `platform.timeout`. **Implemented (DP1-05).** | `PlatformError` |

### 2.2 Registration (DASHPAY §3.4)

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `registration_quote(req)` | async | The real costs for `req`: contested, lock, fee, total, credits left, and the `grant` to ask for (`max_duffs` and `max_credits`). Refuses a bad label (`name.*`) and bad invitation funding (`invitation.*`, `name.unavailable_for_invite`) before any prompt. | `RegistrationError` |
| `start_registration(req, grant)` | async | Persists a `Draft` row and runs the state machine; returns the draft id. `req.funding` is stored as DP1-02's versioned `funding` encoding (§3.4 "Registration rows"), not as this record's serde form. An `initial_profile` avatar must already have a URL (§5); registration never uploads. `FaucetAssetLock` is refused with `platform.feature_off{feature: "faucet"}` outside developer builds. | `RegistrationError` |
| `start_registration_past_unknown_funding(req, grant, past)` | async | `start_registration` past earlier registrations whose funding is unknown (DEC-197, "In progress" below). `past` names each blocker the user confirmed, as `registration.in_progress` names it. Call it only after the user confirmed the wedge copy. A named blocker passes only if every funding step it waits on is wedged; anything else still returns `registration.in_progress`. SPV must be synced (`registration.not_synced`), and each passed step's funding must have been released at least an hour ago (`registration.funding_unknown_recent { draft, wait_secs }`). A malformed name is `invalid_argument`; a well-formed one that blocks nothing is ignored. | `RegistrationError` |
| `registrations()` | async | Every `dp_registration` row of the wallet, with what each waits for (§5), and, read-only, every outstanding funding step whose row is gone (draft `registration/<n>/funding`; §2.2 "The registration ledger"). | `RegistrationError` |
| `resume_registration(draft, grant)` | async | Advances a parked flow; it acts on any state (§4.1). `grant` is required exactly when the row's `waiting` is `Unlock` or `Authorize`, and ignored otherwise. | `RegistrationError` |
| `discard_registration(draft)` | async | Refused (`invalid_argument`) while the row is committed by the registration ledger (§2.2): it holds a lock not yet consumed, a funding step of its own may still deliver a lock, or its funding call runs; and for a row this build cannot read. A `Done` row, or one whose identity exists with nothing of its funding outstanding, may be discarded although its `funds_committed` reads `true`; so may a `Cancelled` one (`registration.funded_elsewhere`, `registration.funding_cancelled`). | `RegistrationError` |
| `finish_asset_locks(grant)` | async | Tools ▸ Repair "Finish transfers": resumes tracked asset locks that no flow finished. Not gated by the journal: it resumes committed locks and builds none. It also applies the wallet's proved top-up locks (DP1-06, §2.8 `resume_top_up`), which needs a grant with `max_duffs` ≥ 1 (none spent); without one they count as `still_pending`. `FinishReport` sums registrations and top-ups. Platform's unproved "already consumed" for a pending top-up lock counts it `resumed` and `still_pending` (it stays pending); for a settled one (historical or resolved, §7 item 44) it counts in none of the fields. | `RegistrationError` |
| `prepare_faucet_lock(grant)` | async | Developer builds only. Derives a fresh registration asset-lock key (`m/9'/c'/5'/1'/…`) and returns its id and compressed public key for the faucet's `POST /api/asset-lock-proof` (§5). Outside developer builds: `platform.feature_off{feature: "faucet"}`. | `RegistrationError` |

**Done (DP1-02, Contract-Version 14, item 40; its review rounds added `registration.coins_locked` (Sol r1, DEC-171),
`RegistrationStatus.name_failure` and `profile_failure` (Opus-high), `RegistrationError::InProgress { draft }` and
`RegistrationWait::FundingUnknown` (the ownership model), the row failures `registration.funded_elsewhere` and
`registration.funding_cancelled` (its rev2), `start_registration_past_unknown_funding`, `registration.not_synced` and
`registration.funding_unknown_recent` (DEC-197), and `RegistrationWait::NeedsFinish` (DEC-198 am.1)).** What the
bodies decided where the rows leave room:

- **Costs.** The lock is 0.03 DASH (3 000 000 duffs) for a plain name and 0.25 DASH for a contested one, as the iOS
  wallet locks, raised when the credits below need more. The fee is E0-04's `fee_bound_worst` over every spendable
  coin of the funding sources (n × 149 + 300 bytes at 1 duff/byte), a true upper bound on the library's selection.
  `grant.max_credits` is IdentityCreate (bounded at 300 000 000 credits until DP1-06's cost table) plus
  `register_name`'s grant for the label and the temporary label, plus 300 000 000 with an `initial_profile`.
  `start_registration` checks the grant's Funding cap (without redeeming it) and the balance against lock + fee
  before it writes a row (`platform.grant_exceeded`, `registration.funding_insufficient`), and the funding step
  checks the lease again. The balance is the engine's spendable set (the coin snapshot's auto-selectable coins), not
  the library's. The pinned asset-lock builder cannot be told to skip a coin, so while any coin of the funding
  accounts is locked by the user (or the dust protection) or reserved by an engine payment (the coin snapshot's held
  funding coins), `start_registration` and the funding step refuse with the new code `registration.coins_locked`
  (retryable once the coin is unlocked or the payment ends) and fund nothing. A tracked lock's inputs are no such
  coin: the library reserves those itself. The funding step checks under the wallet's coin fence (E0-04 §2a.6) and
  holds it through the library's coin selection and signing, until the library tracks the lock it built (DEC-173 (3)):
  "Lock unspent", the dust protection's locks and every engine build wait for it, so a lock lands before the check
  (refused) or after the build, never between. From then on the built lock's inputs read reserved in every coin view,
  derived from the library's tracked `Built` and `Broadcast` locks rather than stored, so no engine build spends them;
  they are free again once spent or untracked (positively not sent, rejected later, swept), with no reservation of the
  engine's to clean up. Reservations are owner-keyed (DEC-179): a coin is reserved while any payment or CoinJoin
  session claims it, and a lock's untracking releases none of those claims. With dust protection on, a dust coin it
  locked blocks registration until the user unlocks it. The funding call permit (`funding_call_permit`) takes the
  fence into the call.
- **Signers.** Identity creation from the tracked lock signs with the lock's one-time key, the `PlatformFunding`
  signer: a grant that resumes a flow before `IdentityRegistered` needs `max_duffs > 0` (the quote's grant does). It
  spends nothing.
- **The registration ledger (DP1-02 ownership model, rev2).** Every decision about which registration lock is
  whose, and every gate, reads one ledger of the wallet, built in one job of its registration actor: the rows, the
  library's tracked locks and identities, the drafts whose funding call runs, and P2b's funding steps (each
  `registration/<n>/funding` step with its key, status, the active attempt's signed txids, the resolved attempts'
  tombstones and whether its marker was written under the durable-record gate, in one read of the lease table). A
  journal that cannot be read fails the decision closed (`storage`, nothing written). A step's claim is `None` (never
  marked, resolved `NotSent`, or resolved with tombstones under a durable record), `Exact` (its calls signed these
  txids) or `Unknown` (marked and possibly sent, with no txid known); `Unknown` is never read as "no funding". A
  recorded outpoint whose txid is a tombstone is voided: a row's live outpoint is its recorded one unless voided. A
  lock is consumed when it is tracked `Consumed`, its identity is discovered, or a row holds it with an identity. A
  step's signer is its draft's row, or, once that row is gone, its heir: the oldest readable Core-balance row at the
  step's index with no live outpoint, no identity and no step of its own whose funding key (registration, index,
  account 0) is exactly the marker's. The step's holder is the row whose live outpoint is a lock the step signed; its
  row is the holder, else the signer. A step is outstanding until its claim is `None` or every lock it signed is
  consumed and held, or consumed with no open row of the step left to receive the identity; with no row it is an
  orphan. A row is `Funded` (it holds a live lock not consumed), `Awaiting` (an outstanding step, and it is open: no
  live lock, no identity), `Closed` (an outstanding step it is no longer open for), `Settled` (its identity exists, or
  its live lock is consumed, nothing outstanding), `Cancelled` (open, no outstanding step, its funding call not
  running, and another row holds its step's lock (`registration.funded_elsewhere`), or it records a voided outpoint
  or its step's claim is `None` while the marker stands (`registration.funding_cancelled`), or it already failed with
  that code) or `Unfunded`; a row this build cannot read is committed when it records an outpoint or identity, has a
  step, or a lock no row holds may be its. A lock no row holds live and not consumed is `Dead` when it is a tombstone
  or a non-outstanding step signed it, `Step` while an outstanding step signed it, `Parked` while a row this build
  cannot read may own it (DEC-173 (2)), `Contested` while an `Unknown` step with no lock of its own may have built it
  (its index, or a lost one, unless its amount differs from the step's row's), else `Free`. A `Step` lock may go only
  to its step's row while that row is open, its funding call running or not, else (that row gone, or readable and not
  open) to a recovery row; a `Free` lock to an open `Unfunded` row whose funding call does not run, or a recovery row;
  a `Contested` lock only to a recovery row, and only while every step contesting it is wedged ("The wedge" below);
  no other lock to anyone. A row is committed while `Funded`, `Awaiting` or `Closed`, or while its funding call runs;
  the gates read that. `RegistrationStatus.funds_committed` keeps E0-04 §16.5's meaning: committed, or a Core-balance
  row that holds or held a lock (an outpoint, an identity, a phase past funding), so a `Settled` or `Done` row still
  reads `true`; a `Cancelled` row reads `false`. Terminal evidence is written by the run for its own row at the top of
  `KeysPrepared`, `FundingSent` and `ProofWaiting`, and by "Finish transfers" for every row, never while the row's
  funding call runs: a consumed lock the row's step signed, held by no row, goes to the open row with the identity
  discovery reports for it (outpoint, identity, the identity's index, phase `IdentityRegistered`, the failure
  cleared; no funding and no identity creation follow), and until discovery reports it the row waits at `Network`; a
  voided outpoint is cleared (back to `KeysPrepared`), the exact tombstone only; a `Cancelled` row is failed with its
  code, not retryable, and its automatic backup is taken again. The rest of this section names what each path does
  with the ledger.
- **In progress.** `registration.in_progress { draft }` names what blocks a new registration: a row with no failure
  that is not `Done`, a retryable failed row, or a committed row that is not `Done`, never a `Cancelled` one (a row
  this build cannot read while committed); else an orphan funding step (`draft` is then `registration/<n>/funding`).
  **The wedge (DEC-197).** A funding step whose call released a signature and recorded no lock reads `MaybeSent` for
  good, and Repair refuses it. Its row stays committed (`waiting: FundingUnknown`) and can't be discarded. Nothing
  attaches the step's lock to it (DEC-198). The only exit is `start_registration_past_unknown_funding`, which starts a
  new draft past the step once SPV is synced and an hour has passed since the release. The new draft has its own index
  and lock. If the earlier lock confirms, it goes to neither row: "Finish transfers" recovers it (DEC-198 am.1), and a
  new draft whose index it leaves unattributed meanwhile waits `NeedsFinish`. Before that call, the
  UI shows this copy and the user confirms it: "An earlier registration's payment could not be confirmed, and it may
  still appear on the network. You can start a new registration now. The earlier payment may still confirm. If it does,
  its funds show up as a transfer this wallet can recover later with Tools ▸ Repair ▸ Finish transfers, as a separate
  identity, so you may have paid for two registrations. The earlier registration can't be discarded." On
  `registration.funding_unknown_recent`: "The earlier registration's payment was sent less than an hour ago. Try again
  in {minutes} minutes." With `wait_secs` none, the step shows no release time (P2b's `funding_release.at`;
  the call fails closed): "This wallet can't yet tell when the earlier registration's payment was sent. Try again later." On
  `registration.not_synced`: "Wait until the wallet has finished syncing, then try again."
  A `Cancelled` row is discarded, never resumed: to register again the user starts a new registration. A row this build cannot run reads as failed, not retryable: a
  `funding`, `initial_profile`, `phase` or lock outpoint it does not know is `registration.unsupported`, an
  `Invitation` row is `invitation.invalid` (until DP5-02 the invitation-claim path does not exist;
  `start_registration` refuses `Invitation` funding with `platform.not_implemented`), and `FaucetAssetLock` is
  `platform.feature_off`. The failure is derived when read, never written: an unknown row is never funded, advanced
  or deleted (`discard_registration` refuses it), and stays as written for the build that can read it. Every
  encoding of the row, the profile included, is read before anything is funded.
- **`label`** is stored as typed; a `temporary_label` is kept only for a contested label and must itself be
  uncontested (`invalid_argument`). An `initial_profile` naming an avatar candidate is `invalid_argument` until
  DP4-02's `prepare_avatar`.
- **Funding failures (H12).** The funding call runs under P2b's funding call permit (E0-04 §2a.5, §2a.6): the
  step's marker is durable before the library starts, and the library signs with the permit's funding signer, which
  records the sighash of every signature. When the library stores the lock it built (`Built`, through the changeset
  tap, which writes the outpoint to the row: §3.4 Restore rule 3), before the broadcaster sees it, the engine checks
  each input against those sighashes and journals the lock's txid and inputs as the funding step's `signed_lock`
  before the persister takes it. Before the call an `Awaiting` row records its own step's lock if it is tracked, and
  otherwise fails `platform.broadcast_unknown`, not retryable, funding nothing; a `Closed` row fails the same way; a
  `Cancelled` row stops; an `Unfunded` row checks its index (below), then funds. After the call a fresh ledger
  decides, never the call's result alone: the row's live outpoint, when the step signed it (the tap or "Finish
  transfers" wrote it), or else its own step's tracked lock is recorded (`ProofWaiting` after an InstantSend
  timeout); an `Ok` naming a lock the step did not sign is logged and never recorded. Without one, a step whose
  claim is `Exact` or `Unknown` fails the row `platform.broadcast_unknown`; only a `None` claim reads the call's
  error: a `NotSent` (P2b's `NotSent` row superseded the marker; nothing was signed) sends the row back to
  `KeysPrepared`, retryable, with the library's code, a refusal before the call fails with the refusal's code, and
  anything else fails `platform.broadcast_unknown`. Such a row counts as in progress and refuses discard while it is
  `Awaiting`; the tap or "Finish transfers" gives it its step's lock when it is back (the row resumes at
  `FundingSent`, no second row). The row's record of its lock, and the tap's, are retried; one that still fails ends
  the run with `storage`, the row unfailed at `KeysPrepared` and its index, `Awaiting` its signed lock.
- **Settled after the identity.** Once the identity exists the flow cannot fail for good: its funds are spent. A name
  step settles only on a refusal positively identified as one a retry would meet again, from an explicit list:
  `name.taken`, `name.contest_open`, `name.locked`, `name.invalid`, `name.unavailable_for_invite`, and for either
  step `platform.grant_exceeded`, `platform.insufficient_credits`, `platform.invalid_argument`,
  `platform.seed_mismatch`, `platform.feature_off`, `platform.not_implemented` and `identity.keys_missing`. The name
  step records the code in `name_failure` and the flow goes on to the profile; the profile step records it in
  `profile_failure`; either way the row ends `Done`, and counts as in progress no longer. Claiming a name for the
  identity is the names flow's (`register_name`), publishing the profile `update_profile`'s. Anything else waits at
  `Network`: an outcome that may yet land (`broadcast_unknown`, `will_be_sent`), a proof or trust failure, the network,
  `internal` (the SDK's `ExecutionNotProved` and `IdentityNonceNotFound` map there) and any error the library
  flattened (platform-wallet's DPNS registration error, with DP1-03's reconciliation lookup inconclusive). Both steps
  read Platform before they write, so the retry finds what landed. A closed session stops the run and a `storage`
  error ends it, the row as it was; parks stay the step's own. A row an earlier build failed for good after its identity is
  settled too: it counts as in progress no longer and may be discarded (a row this build cannot read never is). The
  profile step reads Platform's profile first, as Platform keeps one profile document per identity: one that says
  what the row's does (the fields the row leaves out are not compared) counts as published, a different one is
  revised under `update_profile`'s rules (the fields the row sets replace), and without one the profile is created.
  A refused put is checked against Platform once more, so a put that landed meanwhile counts as published; a recheck
  that fails decides nothing and follows the read's rules (a trust or proof failure and the network wait at
  `Network`, `storage` ends the run, a closed session stops it), never settling the row. A step killed after the put
  therefore resumes to the same profile.
- **Indexes.** A flow takes its identity index after the sync hold, one past every index the wallet's identities,
  tracked registration locks and rows use, chosen from the rows read in the write's own job of the wallet's
  registration actor, so no two flows take one index. The hold (Restore rule 1)
  lasts until SPV has synced and the bring-up's discovery has answered which indexes the seed uses (`Ready`,
  `NoIdentity`, `PartialAccountsPending`, `SeedBindingUnverified`). Until SPV has synced the flow waits at `Sync`; a
  bring-up that runs again only at the first unlock (`IdentityUnsettled`) parks it at `Unlock`; while the bring-up
  runs, or ended without that answer (`NotRun`, `PartialNoIdentity`, `DiscoveryFailed`, `IdentityScanIncomplete`),
  the run waits at `Startup` with its lease and has the bring-up run again (§7 item 46). The
  funding also waits at `Sync`, with nothing signed and no failure, while P2b's gate holds another tracked
  `Built`/`Broadcast` lock the catch-up has not resent (`will_be_sent` naming that lock, not the step). The
  index is a field of the row, never derived from the library: a lock the library rebuilt from the chain says index
  0 whatever index funded it, so a lock's index is known only while it is live (`Built`) or when it is not 0. A flow
  owns what is at its index only by its own lock: a lock whose known index is the row's and that no row holds or
  signed, and the identity derived from that lock's outpoint. An identity at the index derived from a lock another
  row holds or signed, or one another row holds, sends a flow that is not yet funded to a new index; so does another
  row at the index that holds a lock or identity, is committed, or is older. Anything the flow cannot attribute (an
  identity at the index whose lock is `Contested` or `Parked`, or that no lock proves, a `Free`, `Contested` or
  `Parked` lock whose index is unknown, another draft's outstanding step at the index) stops the flow before
  funding, waiting at `ChainLock`; another draft's step at the index that is no longer outstanding (settled or
  cancelled) sends it to a new index. New indexes skip the key index of every step in the ledger, standing, settled
  or cancelled, so no row funds at another draft's step index. An identity with no lock to prove it is never adopted; the row waits until it is discarded
  (`register_name` then names the identity).
- **Waits.** A running flow waits for the sync, for the network (backing off to 60 s), for its proof and for a lock
  the library untracked to be tracked again, in its own task, keeping its lease, so one grant carries it through.
  The `ChainLock` wait has no limit, in bounded waits (the library's own unbounded wait gives up after 180 s). An own
  key is dropped when the InstantSend window ends; a key given later lasts its own hold. A flow parks, ending its run
  and lease, only at a step that needs an authority it lacks, or at a bring-up hold above. A row no task drives
  reports the wait derived from the row and the world, never a wait left by a run that ended.
- **`resume_registration`** with a grant uses it whatever `waiting` says, so the flow does not stop at its next
  signing step; without one the flow runs its keyless steps (the sync hold, the proof wait) and parks at the first
  that signs. A flow that runs already takes the grant: its parked lease through `rebind_lease`, or, when it has no
  lease that signs, a new lease handed to its run. A row failed for good is `invalid_argument`.
- **One owner per row (DEC-171), one actor per wallet (DEC-173).** A run, `discard_registration`,
  `resume_registration` and "Finish transfers" act on a row only under its claim: they raise the row's persisted
  `owner` generation, and every write and delete names the generation it holds. Every claim, every read of a row
  that leads to a write and every write runs as a job of the wallet's registration actor, one at a time in the order
  sent, and runs to its end whatever happens to the task that sent it: no claim or write of an aborted task lands
  after a newer one, and an unloaded wallet's flows are forgotten in a job, so a claim sent before the unload is
  forgotten with them. A run whose generation was taken stops at its next write or at its funding call. Evidence the library reports (the
  changeset tap's outpoint, and "Finish transfers" giving a stranded lock to a waiting row or to a new row) is read
  and written in one job, under the owner read with it. The tap's lock goes, among the rows at its known index with
  no lock that the ledger lets take it, to the row whose funding call runs, else to the oldest. `resume_registration` asks the running
  flow to take its grant rather than start another run.
- **`discard_registration`** takes the flow's claim between library calls: a call running refuses it, and so do a
  row the ledger reads committed and a row this build cannot read, and then the flow runs on from its row as it is (a run it stopped
  starts again). It never waits for a run. A funding call's outcome is written before the call counts as ended. The
  row is deleted only under the claim and while its phase and outpoint are the ones the decision read.
- **`finish_asset_locks`** first writes the ledger's terminal evidence for every row (above), then, on a fresh
  ledger, gives every tracked registration lock of account 0 that no row holds and that is not consumed a row, as the
  ledger lets it: a `Step` lock to its step's row while that row is open, its funding call running or not, else to a
  recovery row when that row is gone or readable and not open; a `Free` lock to the row at its known index, else
  (index unknown) to the oldest `Unfunded` row waiting for a lock (no failure, or a retryable one) whose amount it
  covers, else to a recovery row. A recovery row is a new row with label `""` at a fresh index, which skips the name
  step; it is only ever made for unspent value (a consumed lock settles its step instead). A `Contested` lock
  goes to a recovery row only when every step contesting it is wedged; a `Parked` or `Dead` lock goes nowhere. `still_pending` starts from the ledger read after these writes: the outstanding steps no
  row holds a lock of, plus the `Parked`, `Contested` and `Free` locks (an `Unknown` step and a lock it contests
  count as two). While the funding hold lasts (the indexes the seed uses are not known yet) it writes nothing and
  reports the same count. It
  then runs each row funded by evidence (an outpoint, or a funded phase) once under the grant, without waiting for a
  proof; a `Built` lock is resumed briefly so the library broadcasts it again. A row without its proof parks alone:
  the grant's lease keeps its key for the other rows. `resumed` counts the rows it ran. A row whose run fails (a
  `storage` error) is logged and counted in `resumed` and `still_pending`; the other rows run on.
- **`prepare_faucet_lock`**, and `FaucetAssetLock` funding: no build is a developer build yet, so
  `platform.feature_off{feature: "faucet"}`.
- **`waiting`** is derived, never stored: the running flow's own wait, else, for a row no task drives, for an
  `Awaiting` row (failed or not) `FundingUnknown` when a step of its is `Unknown`, `Network` while discovery has not
  reported the identity its step's consumed lock created, and `ChainLock` otherwise, `Sync` or `Startup`
  before funding while the hold lasts, `ChainLock` for a lock without a proof or evidence it cannot attribute
  (`NeedsFinish` when only locks "Finish transfers" recovers past wedged steps leave its index unattributed),
  otherwise `Unlock` or `Authorize` by the vault's state. An orphan step's entry waits the same way and is read-only:
  `resume_registration` and `discard_registration` refuse its draft. `failure.needed`/`available` of `registration.funding_insufficient` are computed when read.
- **`contest_ends_at`** is the end (UNIX s) on the identity's contest watch row for the label (DP1-04, item 45): set
  when `register_name` learned it or the watch read it from the end-date index; `None` while unknown and once the
  contest resolved (the row is gone).
- **On E0-04.P2b (Mode B):** the funding step runs under P2b's funding call permit, which charges the Funding budget
  (the cap is also checked before the call), writes the funding marker, counts released signatures and records the
  signed lock (above). The identity step runs under P2b's step permit (`registration/<draft>/identity`, resumable):
  a call that released a signature and failed reads `MaybeSent`, and one whose signer met the lock parks the flow.
  Still waiting on P2b: `DispatchResolved` for a flow's steps, and
  the catch-up pass that resumes parked flows at session open and unlock. Until then a parked flow moves on
  `resume_registration` or "Finish transfers". The Funding cap holds per lock: a host lease passed by id that funds a
  second registration is charged by P2b's permits, not here. `register_name` accepts a vault grant or a lease id
  (§1); the flow's name step runs under its own lease through `register_name_for_flow`.

### 2.3 Names

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `check_username(label)` | free, pure | The rule checklist (F4: 3–23 characters, `[A-Za-z0-9-]`, no edge hyphen, no `--`), the normalized label and whether it is contested. A bad label is `valid: false`, not an error. | `NameError` |
| `name_availability(label)` | async | `Invalid{rules}`, `Available{contested}`, `Taken{owner}`, `ContestOpen{ends_at, contenders}`, `Locked` or `Unknown`. | `NameError` |
| `register_name(identity, label, grant)` | async | An extra name, or the name of a registration that parked before it. Its cost is budgeted, not capped: an estimated fee bound plus, for a contested label, the contest fund to join, which the grant's `max_credits` and the identity's balance must cover before the grant is redeemed or anything is sent (`platform.grant_exceeded`, `platform.insufficient_credits`; a refused grant stays usable). The fee bound is an estimate until DP1-06's cost table and E0-04's per-transition accounting; Platform may charge more. A contested label returns `ContestStarted{ends_at}`; joining another identity's contest whose join deadline is unknown is `platform.unavailable`, with nothing spent. A plain name registered while the identity's own contest is open becomes its temporary name. `grant` may be a lease id (§1); a vault grant id is redeemed into a one-shot lease that ends with the call. Either way the write runs under a call permit of the lease, charged the cost in credits, so a vault lock waits for it (E0-04 §2a.3, P2b). What became of a write comes from Platform only: a label whose write may be in flight is no owned name, and a retry (or the identity's next registration) asks Platform and records the answer. | `NameError` |
| `set_main_name(identity, label)` | async | Picks which owned name the identity shows; `None` clears the pick. A name the identity does not own by Platform's evidence (a label whose write may be in flight included) is `invalid_argument`. The pick is the user's and sync never rewrites it (#4978). | `NameError` |
| `main_name(identity)` | async | The name the identity shows: the pick while owned, else the temporary name during an open contest, else the label it contended for once won, else the name it got first by Platform's acquisition time (the marketplace row's `$transferredAt`, else `$createdAt`, else the library's stamp; untimed names last). `None` if it owns none. | `NameError` |
| `contest_status(identity, label)` | async | The own contest as Platform's vote state shows it now (one vote-state query): `state` `Open` (voting, or ended and waiting for the block that resolves it), `Won`, `Lost{winner}` (`winner` `None`: resolved with no winner) or `Locked`; `ends_at` (UNIX s) from the watch row, else from the end-date index while open; the contenders of the first page in Platform's order with their votes and `is_self`; the lock and abstain votes; the temporary name pref. An outcome found for a watched contest is applied as the contest watch applies it (item 45): journaled, the label out of the open contests, the row gone. A label the identity neither contends for (on a page that lists every contender) nor watches is `invalid_argument`; one it watches that DAPI does not show yet is `Open`. **Implemented (DP1-04).** | `NameError` (`identity.not_found` for an identity the wallet lacks; `invalid_argument` as above or for an id that is not Base58; `name.invalid` for a bad label) |
| `search_users(prefix, limit)` | async | DPNS prefix search. Only the prefix goes to DAPI: trimmed and normalized (homograph-safe, lower case); blank or `limit` 0 is `invalid_argument`, a prefix no label could start with (anything outside `[a-z0-9-]` after normalizing, a `.dash` suffix included) returns no hits without a query, and `limit` is capped at 100. Hits in Platform's order, each with the label as Platform spells it and the identity's profile (the hits' profiles in one query, `$ownerId In`, once per identity; a failed fetch shows no profile). `relation` is relative to the wallet's main identity (`IsSelf` for it; `Stranger` with no main identity or no contacts snapshot yet). `unverified` (§5): the label as that identity's unless the names' fetch verified, and the profile shown unless the profiles' fetch did. Search writes no trust records. **Implemented (DP2-03).** | `NameError` |
| `resolve_user(username)` | async | Exact lookup: trimmed, one `.dash` suffix dropped (any case), normalized; `None` if no such name, and without a query for a label DPNS cannot hold. Hit as in `search_users`. **Implemented (DP2-03).** | `NameError` |

### 2.4 Contacts

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `contacts(identity, q)` | sync | The sections asked for (empty = all; each once, in the query's order), sorted, filtered by `text`. From the library's memory: the identity's sent and incoming requests, established contacts, ignored senders, cached contact profiles and deferred contact-crypto queue, and the Core wallet's DashPay accounts. One relation per contact, by precedence: `Contact`, then `Ignored`, then `PendingIncoming`, then `PendingOutgoing` (ignoring a contact's identity leaves the contact). Sections: `Requests` (incoming), `Contacts`, `Pending` (outgoing), `Hidden` (contacts with `hidden` set, and ignored senders, which un-ignore from there). `channel`: `NotEstablished` unless a contact; `Broken` as the library flags it; `SetupPending` while an account op (`RegisterReceiving`, `RegisterExternal`) is queued, or while the account that pays the contact is missing or was not built from the contact's current request, which the library builds or rebuilds with the signer; else `Ready`. `since` is the request a row rests on (for a contact the later of the two), `last_activity_at` the latest of that and its DashPay payments' block times (unconfirmed and failed payments do not count), all UNIX seconds from authoritative times only (DEC-114), so a restore that replays the same contacts sorts them the same way; ignored senders have neither. Declared residual: the library replaces a contact's request in place when either side rotates its key, so `since` and that request's time then show the rotation. Requests and Pending are newest first whatever `sort`; Contacts and Hidden follow it: `DisplayName` by the shown name (the first non-blank of alias, display name and username, else the id; case-folded), `Username` with known usernames first, `DateAdded` and `LastActivity` newest first with unknown times last; ties by id. `text` (trimmed; blank filters nothing) matches the username, display name or alias case-insensitively, or, from four characters, the start of the id. `display_name` is the cached contact profile's. `username` is the contact's DPNS name (of the first ten domains Platform returns for the contact, those whose owner and `records.identity` are the contact, the first by normalized label), fetched in the background, one contact at a time with a 30 s limit each, and kept in memory for an hour (a failure retries after a minute): a read never waits for it, and a name that lands or changes signals `Platform{Contacts}` for the identities that showed it. `unverified` (§5): the contact's `profile` if one is shown, its `dpns_label` if a username is shown (the record of that spelling as the contact's), and each `contact_request` the relation rests on (none for an ignored sender), each by the content shown (DEC-183). A contact's identity has no record of its own: its verified requests bind the keys and the xpub the channel uses. The library keeps no contact usernames, so the engine reports each one as a fetch lands it to E0-10b's re-fetch, which verifies the names shown; a fetch that finds the name gone withdraws it, and one no longer shown for another reason (a forgotten wallet, a dropped relation) stays listed for the session, where at worst it is re-fetched while unverified. While any writer holds the wallet's state (a sync pass, SPV processing a block) the last snapshot answers with `updating: true` (none yet: empty sections, and an identity it lacks is no error); once the writer is done the engine rebuilds the snapshot and signals `Platform{Contacts}` for each of the wallet's identities (`Identities` when it has none). Reads write nothing and make no news: catch-up silence is the tap's (DEC-125/135/139). Re-query on `Platform{Contacts}`, `Identities` and `Payments`, and on the wallet's `HistoryChanged`: a block that mines an already-confirmed (InstantSend) payment moves `last_activity_at` with no Platform change. **Implemented (DP2-01).** | `ContactError` (`identity.not_found` on a live read of an identity the wallet lacks; `invalid_argument` for an id that is not Base58) |
| `contact(identity, contact)` | sync | One contact in any relation, as in `contacts`, with its profile, private details and both request times; `None` if unknown. While a writer holds the state and the snapshot lacks the identity, `platform.unavailable` (not "unknown"): re-query on the `Platform{Contacts}` that follows. `publish_state` is `DeferredUntilTwoContacts` for a contact while the identity has fewer than two, else `Published` when the details shown are what Platform holds for the contact (the digest DP2-04 records, held in memory and loaded in the background on a wallet's first read, item 42), else `Local`. `payment_lock` is `None` until DP3-01. **Implemented (DP2-01).** | `ContactError` (as `contacts`; `platform.unavailable` as above) |
| `pending_setup_count()` | sync | Over every identity of the wallet: contacts whose channel is `SetupPending`, and requests with a queued auto-accept ("Unlock to finish setting up N contacts"). From the last snapshot while a writer holds the state; `platform.unavailable` with none yet. Re-query on `Platform{Contacts}` for any of the wallet's identities and on `Identities`. **Implemented (DP2-01).** | `ContactError` |
| `eligibility(identity, contact)` | async | Before any prompt: `Ok`, `NoDashPayKeys`, `IsSelf`, `AlreadyContact`, `PendingOutgoing`, `PendingIncoming`, `ContactCannotReceive`. The wallet's own state answers first, in that order after `IsSelf`: a contact, our request out, theirs in (an ignored sender's is gone, so it is neither), then our identity's own DashPay keys (an enabled ECDSA `ENCRYPTION` and `DECRYPTION` key; their contract bounds are not checked, as on iOS); none of these asks Platform. Only then is the contact fetched: unknown to Platform is `identity.not_found`, one with no enabled ECDSA `ENCRYPTION` or `DECRYPTION` key (the SDK's recipient key) `ContactCannotReceive`, else `Ok`. A fetch that fails is its `platform.*` error, never `Ok`. Writes nothing and needs no grant. **Implemented (DP2-02).** | `ContactError` (`identity.not_found` for an identity the wallet lacks, or a contact Platform does not know; `invalid_argument` for an id that is not Base58) |
| `send_request(identity, to, scan, grant)` | async | Sends a contact request. `scan` is the id in `ScannedContact.code` (`AutoAcceptClaim{scan}`) when the request comes from a `dapk` QR: the engine then sends it with the auto-accept proof (`send_contact_request_with_external_signer` with `AutoAcceptProofSource::SignWithKey`, the key and expiry of the scan, so that the send does not re-resolve the username the scan verified; DP2-02). The send claims the scan first, atomically: while one send holds it another is `contact.request_pending`, and a claim makes one proof (taking the proof source consumes it), so one scan never makes two requests. A send refused before it signs (a locked vault, an ineligible target, a refused grant) releases the scan for a retry; once it has signed the proof, the request may have been broadcast, so the scan is spent whatever happens next (fail closed). A scan within 60 s of its expiry when the send would redeem its grant is `contact.scan_expired`, the grant left unused (the key goes out with a request Platform receives a little later). Scans go with their wallet (removal, unload) and with the session's close, a claimed one included; the 16-scan bound never evicts a claimed one. An unknown, spent or expired scan is `contact.scan_expired`; a scan for another identity than `to` is `invalid_argument`. The outcome is `Pending` like any send's: the code's key is only claimed to be the recipient's (`verify_scanned`), so the auto-accept is unconfirmed until the recipient accepts. A scan whose shown name or profile was unverified (§5) says so to the send, which confirms before it auto-accepts on Platform's word alone. An ineligible target is `contact.ineligible{reason}` with the `eligibility` answer. With an unverified scan, while Platform writes wait for masternode sync (§4, `TrustProvider::writes_wait`, the `Enforcing` mode only), the send is `platform.context_unavailable`: once SPV's quorums have synced, the library's proof-checked fetch of the recipient confirms what was shown (Q8). Then, before the grant is redeemed: the identity's proved balance below the quote (`CONTACT_REQUEST_CREDITS`) is `platform.insufficient_credits`, a grant capped below it `platform.grant_exceeded`. The document is signed with the identity's first `HIGH` or `CRITICAL` authentication key that the vault holds, never the `MASTER` key (§2.11, item 41); an identity with no such key is `identity.keys_missing`, the grant left unused (its redemption checks that the vault holds the key, item 41). The ECDH runs through the vault's contact crypto. The write runs under a call permit charged the quote from the lease's credits budget (item 39): the quote goes back if the write fails before it signs anything (`Cancelled`), and is kept once a signature is out, whatever the outcome (item 41). A write that fails is read back from Platform: if the request is there it is the outcome, else the write's error. `Pending{request}`, or `Established` when the contact's request was already in. Every write of an identity (this, `accept_request`, `enable_dashpay_keys`, DP1-03's name writes, and DP1-02's name and profile writes) runs alone, under one per-identity lock. **Implemented (DP2-02).** | `ContactError` |
| `accept_request(identity, from, grant)` | async | Sends the reverse request, as `send_request` signs and reads back. Needs the contact's request in the library (`invalid_argument` "no contact request from this identity" otherwise; `contact.already_contact` for a contact; `contact.self`); our own DashPay keys (`contact.ineligible{NoDashPayKeys}`); and the quote (`CONTACT_REQUEST_CREDITS`) as `send_request`. When our request is already on Platform the library adopts it and sends nothing. Returns `Established{request}`, `request` our side's. An "Accept and pay" lease (`AcceptAndPay`) runs the accept as one of its calls, so the idle reaper never ends it during the accept and the payment's `prepare` still finds its `Spend` part (E0-04 §16). **Implemented (DP2-02).** | `ContactError` |
| `ignore(identity, contact)` | async | Adds to `ignored_senders` and drops the sender's pending request. Local and persisted; nothing goes to Platform, so no grant. Idempotent. **Implemented (DP2-02).** | `ContactError` (`contact.self`; `identity.not_found` for an identity the wallet lacks) |
| `unignore(identity, contact)` | async | Removes it again, and the next sync pass fetches the sender's requests anew; a sender not ignored is a no-op. Local, no grant. **Implemented (DP2-02).** | `ContactError` (as `ignore`) |
| `set_private_details(identity, contact, d, grant)` | async | Alias, note, hidden for an established contact (`invalid_argument` "not a contact of this identity" otherwise; `contact.self`). The alias and note are trimmed, blank is none; DIP-15's encoding over 2031 bytes is `contact.details_too_long{bytes, max}`. Saved with the contact in the library. With fewer than two contacts the publish waits (DIP-15: one `contactInfo` links to the pair's requests): `DeferredUntilTwoContacts`, nothing sent, the grant left unused; the next edit after the second contact publishes. Without a grant: `Local`, or `Published` when Platform already holds these details; an edit of details Platform holds for the contact is `platform.needs_grant{credits}` and does not save the edit, because the library's DashPay pass would put Platform's back over it (with nothing recorded for the contact, the edit reads Platform first, which may apply Platform's details; item 42). With a grant, details that are both shown and recorded as Platform's are `Published` with nothing sent and the grant unused; otherwise the encrypted `contactInfo` is created or updated, as `send_request` writes: refused before the grant is redeemed for no DashPay keys (`contact.ineligible{NoDashPayKeys}`), a watch-only identity (`invalid_argument`), the proved balance below the quote (`DOCUMENT_10K_CREDITS`), a document key the vault does not hold, or more than 1,000 `contactInfo` documents (`platform.not_implemented{call}`, item 42) or a failed fetch of them; a lease of another flow than `PrivateDetails` is `platform.grant_invalid`; signed with the first `HIGH` or `CRITICAL` authentication key, under the call permit and the identity's write lock. A write that signed nothing puts back the details shown before the edit (with none recorded, the edit stays, `Local`). One that failed after signing is read back: details Platform holds are `Published`; otherwise the edit stays, `Local`, with the write's error, until a later read finds it landed or puts Platform's back (item 42). **Implemented (DP2-04).** | `ContactError` |
| `enable_dashpay_keys(identity, grant)` | async | Adds the DashPay `ENCRYPTION` and `DECRYPTION` keys to an identity that lacks them (F9), derived as registration derives them, with ids after the highest the identity has on Platform or in the library (4–5 for a base identity). A key of either purpose counts whatever its contract bounds (unchecked, as on iOS). Reads the identity from Platform first: nothing missing is `Ok` with nothing sent and the grant left unused. The quote (`KEY_UPDATE_CREDITS`) bounds the fee (item 41): a proved balance below it is `platform.insufficient_credits`, a grant capped below it `platform.grant_exceeded`, an identity with no enabled `MASTER` key that the vault holds `identity.keys_missing`, all before the grant is redeemed. The IdentityUpdate is signed by the `MASTER` key and by each added key (Platform's proof of possession), and by no other key. The added keys' public keys come from the call permit's identity signer (`derive_public_keys`), which hands out public keys and counted signatures only: its vault signer never leaves the type. Afterwards the identity is read back into the library: a failed write whose keys are there is `Ok`. Logs the credits charged, measured as the proved balance before less after, against the quote. **Implemented (DP2-02).** | `ContactError` |
| `my_user_link(identity)` | sync | iOS's user link (dashwallet-ios `DashPayUserLink`): `dashpay://user?id=<Base58 identity>&username=<main name>`, the name as `main_name` gives it, percent-encoded (every character outside RFC 3986's unreserved set; a DPNS label has none, so the bytes are iOS's). An identity with no name is `contact.no_username`; while a sync pass holds the identities, `platform.unavailable` (also for an identity the last snapshot lacks, or before the first). The main name may change once the names pass has Platform's timestamps (DP1-03), and the link with it; any of the identity's names verifies. **Implemented (DP2-03).** | `ContactError` (`identity.not_found` for an identity the wallet lacks; `invalid_argument` for an id that is not Base58) |
| `verify_scanned(text)` | async | A plain user link or a DIP-15 `dash:?du=&dapk=` payload, verified against Platform. A user link is parsed as iOS parses it (scheme, host and names case-insensitive; `id` and `username` once each, nothing else) and verifies as iOS's scanner does: its username must belong to its `id` now, else `contact.scan_unverified` ("couldn't be verified on the Dash network"). A DIP-15 code has `du` and `dapk` once each and nothing else (the merchant form `dash:<address>?du=…` is not a contact code, and an empty parameter such as a trailing `&` is refused); `dapk` is the 38-byte key blob (type 0, expiry, 32-byte key), whose expiry, UNIX seconds and the owner's hardened derivation index, must be below 2^31. An expired code is `contact.scan_expired` before any query; a `du` with no identity is `contact.scan_unverified`. The key stays in the engine as a scan for this wallet (16 at most; expired ones go at the next scan, then the oldest; all go with the wallet and with the session), and the result's `code` is `AutoAcceptClaim{scan, expires_at}`; a user link's is `UserLink`. A scan begun before its wallet was removed or unloaded keeps no key (`wallet_not_found`), even if the wallet is back by the time Platform answers. Neither form is signed: a verified scan shows that its username belongs to its identity now, not who made the QR. A code's key derives on a hardened path of its owner's seed (`m/9'/coin'/16'/expiry'`) that nothing on Platform reveals, so no scanner can tie it to the identity; only the recipient can check the proof, when the request arrives. `AutoAcceptClaim` is that claim, unconfirmed until the recipient accepts. Each form trims its username, then drops one `.dash`, once: a user link as iOS does (case-sensitive), a code in any case; the label is then normalized, and nothing drops a second `.dash`. Anything else is `invalid_argument`, which never quotes the text. `username` is Platform's spelling, `relation` and `unverified` as in `search_users`. **Implemented (DP2-03).** | `ContactError` |

### 2.5 Payments and activity (the `TxDraft` recipient is in `send/`, §6)

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `payment_lock(identity, contact)` | sync | The lock an ambiguous broadcast set (`dp_payment_lock`), if any. | `ContactError` |
| `resolve_payment_lock(identity, contact)` | async | Reconciles the lock with E0-04 §16.6's evidence (§4.1) for the locked payment's txid. `Sent` (an attempt or a Resend finished `Sent`, or the wallet has seen the transaction) clears the lock. `NotSent` clears it only on **positive evidence**: in this process, `dispatch_status(txid)` is `Some(NotSent)` (the send's tombstone), or a ChainLocked conflicting spend of one of its inputs exists. Never "not found on chain or in the wallet": a peer may be withholding it. Anything else is `Unknown`, which reads like `MaybeSent` and `None`: the lock stays. | `ContactError` |
| `contact_activity(identity, contact, cursor, f)` | async | Payments to and from the contact, newest first, filtered All / Sent / Received; 50 per page, `cursor` the previous page's `next_cursor`. Attributed as history records are (item 47): one item per transaction and direction, `amount` what the contact got or sent (no fee), mined ones by block time with unmined ones first (`timestamp` the block time, else first-seen), abandoned, not-accepted and conflicted ones left out. A contact never paid is an empty page, not an error. Re-query on `Platform{Payments}`, `Platform{Contacts}` and the wallet's `HistoryChanged`. **Implemented (DP3-02).** | `ContactError`: unknown wallet; `invalid_argument` (an id, or a cursor this call did not issue); `contact.self`; `identity.not_found` (not this wallet's) |
| `frequent_contacts(identity, limit)` | sync | The Pay screen's frequent strip (Android's rule): at most `limit` (clamped to 1–20) established contacts that are not hidden, the ones `identity` paid most often in the last 90 days first, then the ones that paid it most often; ties by the latest payment, then the id; none without a payment. In memory (the contacts read model and the history); while a writer holds the library's state it answers from the last view it read, empty before one. **Implemented (DP3-02).** | `ContactError`: as `contacts` |

### 2.6 Notifications

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `events(identity, cursor, limit)` | async | The bell, from `dp_events` (§5 "the feed"). `pending` is the identity's incoming requests, as `contacts`' `Requests` section has them (newest first, the same `ContactSummary`), on the first page only (`cursor` `None`). Then up to `limit` journal rows (clamped to 1–200) with an id below `cursor`, newest first, split into `new` (unread) and `earlier` (read); `next_cursor` is the last row's id while older rows remain, else `None`. Rows of a sender the identity currently ignores are left out (a local mute, as iOS); a hidden contact's are not (iOS, the only app with hidden contacts, filters neither its bell nor its notifications on it). Reads write nothing. Re-query on `Platform{Events}` and `Platform{Contacts}`. **Implemented (DP2-05).** | `PlatformError` (`identity.not_found` for an identity the wallet lacks, as `contacts`; `invalid_argument` for an id that is not Base58) |
| `unread_count(identity)` | sync | The bell count: the identity's unread rows, ignored senders' included only until `ignore` marks them read. From memory, the counts the changeset tap keeps: counted at the wallet's bring-up and after a `.dwbackup` import, recounted after every journal write and `mark_read`. Before the first count (a wallet with no bring-up yet) it answers 0 and starts it; a count signals `Platform{Events}` for each identity whose count moved. A hidden contact's events count. **Implemented (DP2-05).** | `PlatformError` (`invalid_argument` for an id that is not Base58) |
| `mark_read(identity, up_to)` | async | Marks the identity's unread events with id ≤ `up_to` read (the time is the call's), recounts and signals `Platform{Events}` when one changed; ids above `i64::MAX` mean all. Idempotent. `ignore` does the same for the ignored sender's events. It marks the rows of the wallet the call was admitted for: a removal waits for a mark in flight, and a call that outlives the wallet (removed, or reimported) marks nothing. **Implemented (DP2-05).** | `PlatformError` (`storage` when app.sqlite fails; `wallet_not_found` once the wallet it was admitted for is removed or reimported) |

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
| `cost_table()` | sync | The credit costs of the DashPay actions at the network's protocol version, for "≈ N contact requests" and the low-credit warnings (§5 `CostTable`, §7 item 44). The table is the network's: it reads no wallet. Refused only when the SDK's protocol version is one this build does not know (`internal`). **Implemented (DP1-06).** | `CreditsError` |
| `top_up_quote(identity, duffs)` | async | The fee bound and total in duffs, the credits a top-up of `duffs` buys, an estimate that errs low as far as measured (`duffs × credits_per_duff` less the table's `top_up_fee`; Platform's metered charge was far below it on testnet, §7 item 46), and the `grant` to ask for (`max_duffs` = the total, no credits). Refuses an identity that is not the wallet's (`identity.not_found`), below `top_up_min_duffs` (`credits.below_minimum{min}`), an identity with a top-up not applied yet (`credits.top_up_pending{txid}`), above the spendable balance (`credits.funding_insufficient{needed, available}`), and a held-back coin the asset-lock builder would select (`credits.coins_locked`). Until the dispatch journal is read it is `platform.unavailable` (retry). **Implemented (DP1-06).** | `CreditsError` |
| `top_up(identity, duffs, grant)` | async | Funds the identity from the Core balance: an asset lock of `duffs`, then Platform's IdentityTopUp from it (§7 item 44). The quote's refusals and the funding step's (below), checked under the identity's write lock before the grant is redeemed, so a refusal keeps the grant. Held back as a registration is: SPV not caught up is `platform.context_unavailable`; the bring-up running, not run, or its identity discovery incomplete or failed is `platform.unavailable`, and the call has it run again (§7 item 46); identities waiting for an unlock is `platform.signer_unavailable`. A grant whose funding cap is below the quote's total is `platform.grant_exceeded` before it is redeemed, so a smaller top-up can still use it. Until the dispatch journal is read the call is `platform.unavailable` (retry). Once the lock is funded the call returns `TopUpOutcome{txid, state}`: `Applied{credits_added, balance}`, `ConsumptionUnknown{balance}` (Platform answered "already consumed", which nothing proves: nothing added now, and the lock stays pending), `ProofWaiting` (no InstantSend lock in its window), or `NotApplied{error}` (the apply failed); nothing is lost in the last three (`pending_top_ups`, `resume_top_up`). A funding whose outcome is unknown is `platform.broadcast_unknown{artifact}` with the funding step `topup/<index>-<k>/funding` (`dispatch_status`), and every later top-up of the identity is refused the same until that step settles. `grant` may be a `TopUp` lease id. **Implemented (DP1-06).** | `CreditsError` |
| `pending_top_ups()` | async | The wallet's top-up locks Platform has not taken as far as the wallet can prove (in flight, proved and not applied, or funded by a top-up of this wallet and final with its consumption unknown), by identity index: txid, identity, duffs, whether the lock has its proof, and `consumption_unknown` (§7 item 44). Until the dispatch journal is read it is `platform.unavailable` (retry), never a list that cannot tell the wallet's own locks apart. **Implemented (DP1-06).** | `CreditsError` |
| `resume_top_up(txid, grant)` | async | Applies the tracked top-up lock `txid`, waiting up to 30 s for its proof first (a lock not yet sent is sent again; one a funding call of this process is still recording is `platform.will_be_sent`); the outcome as `top_up`'s. A lock the library holds applied (Platform's proved apply) reads `AlreadyApplied` with the balance read again; any other, a resolved or historical one too, is applied again (Platform takes a lock once). The grant is redeemed only for the apply, so `AlreadyApplied`, `ProofWaiting` and `platform.will_be_sent` keep it. The apply signs with the lock's one-time funding key, which only a grant with a funding budget unlocks: the grant needs `max_duffs` ≥ 1, none of which is spent. Tools ▸ Repair "Finish transfers" (`finish_asset_locks`) also applies every proved top-up lock, under its grant. **Implemented (DP1-06).** | `CreditsError` |
| `resolve_top_up(txid)` | async | The user's resolution of the pending top-up lock `txid` that has its proof (they saw its credits arrive): a `consumption_unknown` one, or one a resume answered `ConsumptionUnknown` before its ChainLock let the library mark it final. Recorded in the dispatch journal, so it no longer pends nor blocks the identity's top-ups, across restarts, a removal and import of the wallet, and whatever status the library moves the lock to. `resume_top_up` and Finish still apply it, so a lock Platform had not taken is not lost. Any other txid (unknown, without a proof, applied or resolved) is `invalid_argument`; until the journal is read, or when its write fails, `platform.unavailable`. Needs no grant. **Implemented (DP1-06, Sol r1).** | `CreditsError` |
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
| `NetworkSession.end_flow(lease)` | sync | Releases the lease; idempotent. The session holds the lease from `begin_flow` until `end_flow` or the idle reaper; naming it in a call never ends it. In-flight hand-offs finish first (E0-04 §4.1). | `PlatformError` |
| `NetworkSession.leases()` | sync | The live leases as `LeaseView`s (E0-04 §4.6): flow, state, own key and its seconds left, `funds_committed`, budgets, permits in flight, and whether a library call of the flow runs (which picks the copy, E0-04 §16.10). Re-queried on E0-04's `LeaseChanged` (§6). | `PlatformError` |
| `grant_request(identity, action)` | async | The `GrantRequest` for one write that has no quote of its own: `send_request`, `accept_request`, `register_name`, `update_profile`, `set_private_details` (publishing) and `enable_dashpay_keys`. `GrantAction` carries no payload except the label, so the request is a worst-case bound for the action, which the engine budgets against (E0-04 owns enforcing it). The other actions get E0-04's per-action credit bound (`flows.rs`, no duffs) until DP1-06's cost table. `RegisterName` is DP1-03's quote: no duffs, credits for the estimated fee bound plus the label's fund to join at the contest's current size; until DP1-06 and E0-04's accounting the fee part is an estimate, not a ceiling the engine enforces. | `PlatformError` |
| `dispatch_status(artifact)` | async | What the engine knows about a handed-off artifact: a txid, a state-transition hash, or a funding step id (`registration/<draft>/funding`, `topup/<id>/funding`), as `broadcast_unknown{artifact}` and `will_be_sent{artifact}` carry. `WillBeSent`, `MaybeSent`, `Sent` or `NotSent`; `None` means the engine has no entry. Answered by E0-04 §16.6's table (§4.1). The host reads every `None` as unknown, and only `Some(NotSent)` allows a retry it offers; it calls this before it offers any retry. | `PlatformError` |

## 3. Surface (generated)

The exact public surface: records, enums, error enums, signatures and the headers of every trait impl, with the
`derive`, `serde` and `cfg` attributes that fix what a binding and the JSON see. Do not edit by hand (see the top of
this file).

<!-- BEGIN GENERATED: dashpay-surface -->
<!-- surface-sha256: 4ce1a0ba0ce2e321acc609bc29b9a84a495b73524dfc277b95c30706520c581d version: 21 -->

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
    pub updating: bool,
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
    ContactCannotReceive,
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
    pub code: ScannedCode,
    pub relation: Relation,
    pub unverified: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScannedCode {
    UserLink,
    AutoAcceptClaim { scan: String, expires_at: u64 },
}
impl DashPay {
    pub fn contacts(&self, identity: String, q: ContactQuery) -> Result<ContactsPage, ContactError>;
    pub fn contact(&self, identity: String, contact: String) -> Result<Option<ContactDetail>, ContactError>;
    pub fn pending_setup_count(&self) -> Result<u32, ContactError>;
    pub fn my_user_link(&self, identity: String) -> Result<String, ContactError>;
    pub async fn verify_scanned(&self, text: BearerSecret) -> Result<ScannedContact, ContactError>;
}
impl DashPay {
    pub async fn eligibility(&self, identity: String, contact: String) -> Result<Eligibility, ContactError>;
    pub async fn send_request(&self, identity: String, to: String, scan: Option<String>, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn accept_request(&self, identity: String, from: String, grant: String) -> Result<RequestOutcome, ContactError>;
    pub async fn ignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn unignore(&self, identity: String, contact: String) -> Result<(), ContactError>;
    pub async fn set_private_details(&self, identity: String, contact: String, d: PrivateDetails, grant: Option<String>) -> Result<PublishState, ContactError>;
    pub async fn enable_dashpay_keys(&self, identity: String, grant: String) -> Result<(), ContactError>;
}
// src/platform/credits.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTable {
    pub protocol_version: u32,
    pub contact_request: u64,
    pub profile_update: u64,
    pub contact_info: u64,
    pub enable_dashpay_keys: u64,
    pub credits_per_duff: u64,
    pub top_up_fee: u64,
    pub top_up_min_duffs: u64,
    pub low_credits: u64,
    pub empty_credits: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditLevel {
    Ok,
    Low,
    Empty,
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
    pub state: TopUpState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopUpState {
    Applied { credits_added: Option<u64>, balance: Option<u64> },
    AlreadyApplied { balance: Option<u64> },
    ConsumptionUnknown { balance: Option<u64> },
    ProofWaiting,
    NotApplied { error: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTopUp {
    pub txid: String,
    pub identity: Option<String>,
    pub identity_index: u32,
    pub duffs: u64,
    pub proved: bool,
    pub consumption_unknown: bool,
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
    pub async fn pending_top_ups(&self) -> Result<Vec<PendingTopUp>, CreditsError>;
    pub async fn resume_top_up(&self, txid: String, grant: String) -> Result<TopUpOutcome, CreditsError>;
    pub async fn resolve_top_up(&self, txid: String) -> Result<(), CreditsError>;
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
    InProgress { draft: String },
    FundingInsufficient { needed: u64, available: u64 },
    CoinsLocked,
    IslockTimeout,
    Recoverable { draft: String },
    AlreadyHasUsername,
    Unsupported,
    FundedElsewhere,
    FundingCancelled,
    NotSynced,
    FundingUnknownRecent { draft: String, wait_secs: Option<u64> },
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
    ScanUnverified,
    NoUsername,
    DetailsTooLong { bytes: u32, max: u32 },
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
    CoinsLocked,
    TopUpPending { txid: String },
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
    pub balance_as_of: Option<u64>,
    pub balance_stale: bool,
    pub credit_level: Option<CreditLevel>,
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
pub const MAX_NEWS: usize;
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
    pub name_failure: Option<String>,
    pub profile_failure: Option<String>,
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
    FundingUnknown,
    NeedsFinish,
    Startup,
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
    pub async fn start_registration_past_unknown_funding(&self, req: RegistrationRequest, grant: String, past: Vec<String>) -> Result<String, RegistrationError>;
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
| `RegistrationError` | `registration.in_progress`, `registration.funding_insufficient`, `registration.coins_locked`, `registration.islock_timeout`, `registration.recoverable`, `registration.already_has_username`, `registration.unsupported`, `registration.funded_elsewhere`, `registration.funding_cancelled`, `registration.not_synced`, `registration.funding_unknown_recent` |
| `NameError` | `name.invalid`, `name.taken`, `name.contest_open`, `name.locked`, `name.unavailable_for_invite` |
| `ContactError` | `contact.ineligible`, `contact.already_contact`, `contact.request_pending`, `contact.self`, `contact.channel_broken`, `contact.payment_locked`, `contact.scan_expired`, `contact.scan_unverified`, `contact.no_username`, `contact.details_too_long` |
| `InvitationError` | `invitation.invalid`, `invitation.claimed`, `invitation.expired`, `invitation.already_has_identity` |
| `AvatarError` | `avatar.too_large`, `avatar.unsupported`, `avatar.fetch_failed`, `avatar.hash_mismatch`, `avatar.upload_unconfigured` |
| `CreditsError` | `credits.funding_insufficient`, `credits.below_minimum`, `credits.coins_locked`, `credits.top_up_pending` |

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
  cancelled. Pay now?". **Lock stops new signatures; a write already signed when Lock lands may still broadcast until
  E0-04 P3's dispatch fence**: Lock revokes a write's lease, and its drain waits for the write's call permit (to the
  permit's deadline, E0-04 H3), but rs-sdk does not yet treat its `DispatchScope` as a hand-off fence, so a state
  transition signed before Lock (a contact request, an accept, a key upgrade) can still go out after it. Such a
  write never answers `lease_revoked{Lock}` or `cancelled`: its call returns the library's outcome, read back from
  Platform after a failure (§7 item 41).
  For a registration, `needs_grant` shows in the row as `waiting: Authorize` (vault unlocked) or `Unlock` (locked).
- **Credits.** A top-up spends Core duffs, so its shortfall is `credits.funding_insufficient{needed, available}` in
  duffs, not `platform.insufficient_credits` (credits, "top up your credits"). A cap hit is `platform.grant_exceeded`;
  below `top_up_min_duffs` is `credits.below_minimum{min}`. A withdrawal larger than the balance is
  `platform.insufficient_credits`. `credits.coins_locked` is `registration.coins_locked` for a top-up (a held-back coin
  the pinned asset-lock builder would select). `credits.top_up_pending{txid}` is an earlier top-up of the identity not
  applied yet: the host offers `resume_top_up(txid)`, never a second funding; when its `PendingTopUp` is
  `consumption_unknown`, or a resume answered `ConsumptionUnknown`, also "My credits arrived" (`resolve_top_up(txid)`).
- **Contacts.** `contact.ineligible{reason}` carries the `Eligibility` that refused (never `Ok`), so `NoDashPayKeys`
  leads to "Enable DashPay keys". `contact.scan_expired` is an expired `dapk` proof or a stale or used scan id: "this
  QR code has expired; ask for a new one". `contact.scan_unverified` is a scanned user link or code whose username
  Platform does not give to it (iOS: "… couldn't be verified on the Dash network. The QR code may be outdated.").
  `contact.no_username` is `my_user_link` for an identity with no name. `contact.details_too_long{bytes, max}` is
  `set_private_details` with an alias and note whose DIP-15 encoding exceeds `max` (2031) bytes.
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
  `ineligible{reason}`, `payment_locked{txid}`, `details_too_long{bytes, max}`.

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
- **`unverified`** on `IdentitySummary`, `ContactSummary`, `UserHit` and `ScannedContact` (DEC-125, positive
  provenance): once the trusted-quorum fallback has served a read in this installation (a persisted latch, the
  global setting `trust.fallback_used_at`), Platform data counts as unverified unless dw-appdb's `dp_trust_verified`
  has a record for it. The latch is durable before use (DEC-135, DEC-139, DEC-143): the fallback serves a fetch only
  after the latch is committed and a full WAL checkpoint has made it power-loss durable (app.sqlite runs
  `synchronous=NORMAL`, under which a commit alone is not synced); if either fails, that fetch fails or degrades as
  if there were no fallback. Durability is established, never inferred: each process checkpoints before its first
  fallback use, even when the latch is already on file, since a row left by a refused checkpoint is readable but
  unsynced. A latch read back at open, or one that cannot be read, still counts as set for classification. Records are written by E0-10b when a fetch verifies against SPV-held quorum keys (kinds
  `identity` for an identity and its keys, `profile`, `contact_request` keyed `sender:recipient:$createdAt`,
  `dpns_label` keyed by the homograph-normalized label with a digest binding the spelling shown and the owner,
  `payment` keyed by txid; a contact's identity has no record of its own, its verified `contact_request` binds the
  keys and the xpub the channel uses); the table is global and not
  exported, so a restored wallet's data verifies again. A record is bound to content (DEC-183): it stores a digest of
  the entity's canonical bytes as verified, naming its encoding version (`v2:` and a SHA-256), and data counts as
  verified only while the digest of what the library holds now equals it. An entity the library replaced after it
  verified (from any source, the fallback included) reads as unverified until a fetch of the new content verifies;
  records written before the digest existed verify nothing. A record says that this content verified, not that it is
  current. An identity's balance is not covered. Absence fails closed: data the library keeps in memory after
  a refused store, or that reached the store by any path, is unverified until a record exists. An `IdentitySummary`
  is unverified if its identity, its shown profile or any name in its `names` is (a hidden contested or pending name
  does not count); a `ContactSummary` as §2.4 `contacts` lists. Before the latch is set nothing is unverified. Builds before E0-10b served the trusted service's
  keys without setting it, so an installation that already holds wallets starts latched (dw-appdb migration
  `trust_digest`): its data reads as unverified until re-fetched. "As of" times for offline data come from `sync_status().last_pass`. `Counterparty` and the inviter in
  `InvitationStatus::Valid` carry no flag: they are display data, and DP3-04's money-move gate checks
  `dp_trust_verified` (with the latch) itself before any payment, whatever the record says.
- **`QuorumSource`.** `Spv`; `TrustedFallback` (before masternode sync, reads only, results unverified); `Trusted`
  (the developer toggle, or the degraded mode of §2.2). E0-10a failed, so 1.0 reports `Trusted` (DEC-168) until H-09
  turns on full enforcement. E0-10b's provider (`docs/design/E0-10b-trust-provider.md`) refuses a quorum key SPV and
  the trusted service disagree on with `platform.trust_mismatch`; any other quorum key it cannot supply (an unknown
  quorum after masternode sync, a fallback latch that cannot be made durable, the trusted service unreachable while
  SPV does not verify the quorum) fails the proof as `platform.context_unavailable`, not `proof_invalid`: the proof
  was not judged.
- **`RegistrationStatus`** mirrors a `dp_registration` row. `phase` is the §3.4 state.
  - `waiting` says what a parked or slow flow waits for, and so which line the UI shows: `Unlock` ("Unlock to finish";
    the flow parked keyless, and `resume_registration` needs a grant), `Authorize` ("Confirm to finish": the vault is
    unlocked but the flow's grant died with an unlock, a scope or passphrase change, or its budget ran short; the
    resume needs a grant), `Sync` ("Waiting for the
    network to sync": a restored row held until SPV sync, §3.4 Restore rule 1, or a write held by §2.2 rule 5),
    `InstantSend` and `ChainLock` (the proof wait; E0-04 `Parked{ProofWaiting}` is `ChainLock`), `Network` (DAPI or
    the peers unreachable; retried on reconnect), `Startup` ("Waiting for DashPay to start": SPV has synced but the
    wallet's DashPay bring-up has not answered which identity indexes the seed uses; the engine runs it again, §7
    item 46), `NeedsFinish` ("An earlier registration's payment showed up. Run
    Tools ▸ Repair ▸ Finish transfers to recover it, and this registration continues": "The wedge" below). `None` while the flow runs, and after it ends. A grant is needed
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
  - `name_failure` and `profile_failure`: the code a name or initial-profile step ended with after the identity was
    registered (§2.2 "Settled after the identity"), on a `Done` row. The UI offers `register_name` for the identity,
    or the profile edit.
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
  or a send to someone whose request was already in). `request` is our request's `$createdAt` in ms, the id the
  journal uses for requests (`EventKind` below): the library's `ContactRequest` carries no document id.
- **`ScannedContact.code`** (`ScannedCode`, tagged `kind`): `user_link`, or `auto_accept_claim{scan, expires_at}`
  when the payload carried a `dapk` key, unconfirmed until the recipient accepts (§2.4 `verify_scanned`); pass `scan`
  to `send_request`. `unverified` is the shown name's or profile's provenance, as on `UserHit` (either one
  unverified sets it); it says nothing about the QR key's claim.
- **`IdentityKeyInfo.contract_bound`** and **`contract_bound_document_type`**: keys 4–5 are bound to the DashPay
  contract's `contactRequest`.
- **`EventKind`** covers the §3.5 journal: `UsernameRegistered`, `ContestWon`, `ContestLost`, `ContestLocked`,
  `RequestReceived`, `RequestAccepted`, `ContactEstablished`, `PaymentReceived`. **`DashPayEvent.reference` per kind:**
  the label for the name kinds (`UsernameRegistered`, `Contest*`: "You won @alice"), the contact-request id for the
  request kinds, the txid for `PaymentReceived`. `contact` is empty for the name kinds. The journal's `ref` column holds
  the same value, with two refinements the changeset tap (E0-06) writes and the reader (DP2-05) follows:
  - **name kinds** store `<label>@<stamp>`, so a name that leaves and comes back is a new event (dw-appdb's `dp_events`
    rule; DPNS labels have no `@`, and the reader drops `@…` for `reference`). The stamp is the name's `acquired_at`
    (ms) for `UsernameRegistered`, omitted with the `@` when the library has none, and the contest's `ends_at` for the
    `Contest*` kinds, with the watch row's label as written (DP1-04 writes every outcome the same way, item 45);
  - **request kinds** use the request's `$createdAt` in ms as its id: the library's `ContactRequest` carries no
    document id, and with `contact` it names the request. `RequestReceived` cites the incoming request;
    `RequestAccepted` (ours came first) and `ContactEstablished` (we accepted theirs) cite the contact's request.
  - **contact events are once per relationship:** a rotated request (a key rotation re-sends it with a newer
    `$createdAt`) is no news. `RequestReceived` is skipped once the contact has any contact row; `RequestAccepted`
    and `ContactEstablished` once it has either of them.
  - **catch-up (DEC-114, DEC-125 monotone boundary, DEC-135, DEC-139, DEC-143):** each wallet has a catch-up
    boundary, the local, unexported setting `dashpay.catch_up_before` (UNIX seconds). Every recovery phase start
    moves it to `max(boundary, now)` in one atomic upsert whose returned value the cache takes (also only ever
    raised), so overlapping starts cannot lower it. The phase starts are `discover_identities`, a names pass, and a
    bring-up that starts a recovery, at its Platform pass's admission (after key acquisition): a restore's (no
    identity on file, not created here), or that of a wallet with identities a discovery stored that no bring-up has
    replayed yet, whichever path admitted it (the bring-up the discovery queues, or the one that replaces it when SPV
    was stopped). That discovery marker is consumed only after the advance it guards has completed, so an attempt
    stopped before its pass (or during the advance), or an advance that could not be stored, leaves it for the next. The marker is in memory: a session
    closed, or the wallet unloaded, before that bring-up drops it, like the names pass it owes. A plain warm start is no recovery
    and never moves the boundary, so what arrived while the app was closed is news and notifies. The first
    bring-up in this installation sets the boundary if absent. Apart from the discovery marker, nothing is carried
    between phases, and nothing moves the boundary back. An event is stored read (no OS notification, DASHPAY §2.7) when its **authoritative** time predates the
    boundary: a request's `$createdAt`, a relationship's later request, a payment's confirmed block time and a name's
    marketplace row time (transfer, else creation), the last two read from the persister. Local fetch or
    observation time (a name's `acquired_at`, the history's first-seen) is never an age. Everything else, an
    unconfirmed payment or an event with no authoritative time, is news, inside a phase or not.
  - **declared residual (DEC-114, unchanged by DEC-125):** an old event with no authoritative time may show as
    unread once. It is cosmetic: no trust decision or money movement depends on read state, and no reconciliation
    is built for it.
  - **trust is not the tap's (DEC-125):** provenance is positive (see `unverified` above), so nothing the tap writes
    or fails to write can make data count as verified, and `store` is never refused for trust. Journal rows and
    signals follow the persister's acceptance and stay best-effort display data. The `dp_trust_unverified` table
    of the `dashpay` migration is dormant: nothing writes it, and it stays only because the schema is append-only
    and the `.dwbackup` wallet rows (E0-07) list it.
  - **contest watches:** a label in `dp_contest_watch` that becomes an owned name journals only its
    `UsernameRegistered`, stored read; its `ContestWon` (ref the row's label and `ends_at`) is DP1-04's watch's to
    journal, aged by the block that resolved the poll (item 45). The watch deletes the row once the contest
    resolved and, for a win, the library lists the name; a row left behind would hide a later purchase of the label.
  - **the feed (DP2-05):** `DashPayEvent` is a journal row as stored, with `reference` as above (`@…` dropped, `None`
    when empty) and `contact` `None` when empty; `at` is when the row was journaled (UNIX s, local), `read_at` when it
    was read (stored read: the journal time). A row of a kind this build does not know is left out. Read state is
    `dp_events.read_at`, so it travels in `.dwbackup` with the rows (dwbackup-v1 §3.1); the catch-up boundary does
    not, so a restore from a bundle keeps the bundle's read state and journals later history read.
  - **news (DP2-05):** a row the tap's write **inserted** unread, of a kind that notifies, is news:
    `EngineEvent::DashPayNews` (§6). The kinds that notify are `RequestReceived`, `RequestAccepted` and the
    `Contest*` kinds. `ContactEstablished` is our own accept and `UsernameRegistered` our own name (a restore
    journals the identity's names unread while their marketplace time is unknown, DEC-114 (4); iOS notifies
    contacts only), and `PaymentReceived` is announced by the transaction's `NewTransactions` (DASHPAY F16: OS
    notifications through the existing notifier). The
    dedup key is the journal's: `(wallet_id, identity, kind, contact, ref)`, with the once-per-relationship rule
    above, so a replayed changeset, a restart, a re-sync, the same request reached by a sync pass and by an accept's
    read-back, and a key rotation add no row and no news; rows a `.dwbackup` import or catch-up writes (stored read)
    are never news. DP1-04's contest outcomes are journaled through the same writer (`ChangesetTap::journal`) and
    become news with no change here. While the vault is locked news is sent all the same: it carries public data only (ids, a kind, a label, a
    request time, a txid), never an alias, a note or an amount, and a host's notification text names the contact by
    username or display name, never by alias. A removed wallet's undelivered news is dropped, and its rows go with its
    tombstone (DEC-210), whose trigger also drops a write still in flight, so it makes no news; a write that began
    before the removal and lands after it makes no news and leaves no count either (the tap's per-wallet epoch,
    which a restore's rollback moves too).
- **`ProfileEdit`** is the whole new profile. `AvatarChange`: `Keep`, `Remove`, or `Set{candidate}` with a
  `prepare_avatar` candidate (after `upload_avatar` when `needs_upload`).
- **`AvatarSize`.** `Small` is 128 px, `Large` 256 px. `AvatarImage.png` is the engine-re-encoded thumbnail, never
  the original bytes, as base64 in JSON.
- **`CostTable`** (DP1-06). The protocol version priced; credits per action (`contact_request`, `profile_update`,
  `contact_info`: an upper bound, the document at its largest priced with every tree it writes at 2^48 entries and
  Drive's estimated processing doubled, §7 item 44; `enable_dashpay_keys`: the
  IdentityUpdate fee plus the two keys' storage); `credits_per_duff` (1000); `top_up_fee`, what Drive requires a
  top-up's lock to hold before it processes it (`calculate_min_required_fee`), which the quote subtracts: not the fee
  charged, which Platform meters and which was far below it on testnet, so the quote's credits err low as far as
  measured (§7 item 46); `top_up_min_duffs` (0.01 DASH, or `top_up_fee` in duffs if larger); and the thresholds,
  Android's: `empty_credits` is the dearer of a contact request and a profile update, `low_credits` ten of them.
  **`CreditLevel`**: `Empty` below `empty_credits`, `Low` at or below `low_credits`, else `Ok`.
  **`TopUpQuote`**: `total_duffs = duffs + fee_duffs` is what the grant must cover.
  **`TopUpOutcome`**: the lock's `txid` and its `TopUpState` (§2.8); `AlreadyApplied` means the library holds the
  lock applied before this call, so the call added nothing; `ConsumptionUnknown` means Platform answered "already
  consumed" without proof, so the call added nothing and the lock stays pending. **`PendingTopUp`**: a lock
  `pending_top_ups` lists; `proved` means `resume_top_up` applies it at once; `consumption_unknown` means a lock a
  top-up of this wallet funded is final and Platform's consumption of it unknown (`resolve_top_up`). **`WithdrawQuote`**:
  the credits taken, the fee in credits and the duffs expected.
- **`Counterparty`** is the record history and transaction records gain as `counterparty` (DP3-02, item 47); here
  it is also the inviter in `InvitationStatus::Valid`. In a history record its `username` comes from the contacts'
  username cache (filled in the background; a landing name sends `Platform{Contacts}`) and its `display_name` and
  `avatar_url` from the cached profile.

## 6. Outside the facade

Parts of DASHPAY §3.6 that change existing engine types. They land with the tasks named, not in E0-08, because each
changes code paths or bindings that would otherwise need behaviour now:

| Item | Task |
|---|---|
| `TxDraft` gains `Recipient::Contact{identity, contact, amount, subtract_fee, note}` | DP3-01 |
| History and transaction records gain `counterparty: Option<Counterparty>`. **Done (DP3-02, item 47):** on m1's `TxRecord` (so `history_page` and `tx_detail`'s records); `dw-app`'s `TxRow` binds it (optional), `dw-ffi` withholds it until E0-13 | DP3-02 |
| `NoticeCode` gains `PlatformTrustMismatch` (E0-10b) and `DashPayStartupIncomplete` (E0-05). `dw-ffi` maps `NoticeCode` one to one, and the Swift bindings are frozen until E0-13. **Done (E0-05, E0-10b):** both are engine-side, and `dw-ffi` withholds them until E0-13 binds them. `PlatformTrustMismatch` goes out once per quorum and session; its detail names the LLMQ type and quorum hash | E0-05, E0-10b, E0-13 |
| `NetworkSession` gains `quorum_source()` (sync) and the developer toggle `trusted_quorums_only()` / `set_trusted_quorums_only(on)` (the global setting `trust.trusted_only`): quorum keys from the trusted service only, SPV not consulted. **Done (E0-10b).** Not a user setting; the binding exposes it in developer builds | E0-10b, E0-13 |
| `EngineEvent::Platform{network, wallet_id, change: PlatformChange}` (§3.5). **Done (E0-06):** `PlatformChange` is `Identities` (identities, their keys and profiles), `Contacts{identity}`, `Registration{draft}` (`draft` is the `dp_registration` id as decimal text; `None` for a lock no row names, a top-up), `Payments{identity}`, `Names`, and (DP2-05) `Events{identity}`: the identity's feed or unread count changed (rows journaled, read or recounted). The tap sends no `Startup` change: the startup status has its own snapshot (E0-05). `dw-ffi` withholds the variant until E0-13 binds it | E0-06, E0-13 |
| `EngineEvent::DispatchResolved{network, resolved: DispatchResolved}`, sent when a provisional outcome settles. Its sources are E0-04 §4.6's: a Resend is accepted, or the wallet sees the transaction (`Sent`); a reload refuses and cleans up an `Unsent` row (`NotSent`); a row-less artifact settles definitely unsent (`NotSent`); Mode B's derived status moves (E0-04 §2a.5); H16's evidence settles a row-less transition. Its `artifact` is a txid, a state-transition hash or a funding step id (`registration/<draft>/funding`, `topup/<id>/funding`). DP1-02 then moves a registration row back to retryable, and the payment and top-up UIs clear their "may have been sent" or "will be sent" state. With E0-04's `LeaseChanged` and `LockProgress`. The payload record exists now (§3); the variant waits because `dw-ffi` maps `EngineEvent` one to one | E0-04 (P2), E0-13 |
| `NoticeCode` gains `DispatchRecordMissing`, `UnscopedDispatch` and `DispatchJournalUnavailable` | E0-04 (P2), E0-13 |
| `EngineEvent::DashPayNews{network, wallet_id, identity, events: Vec<DashPayEvent>, more}`: the OS-notification signal (§5 "news"). **Done (DP2-05):** on the `Platform` clock (≤ 4 Hz per session, trailing edge kept), after the flush's `Platform` changes; one per identity with news, at most `MAX_NEWS` (32) events in arrival order and `more` counting the rest. Hosts show one notification per event for up to 3, else one summary ("N new DashPay notifications"), skip it while the window is frontmost (the bell updates), honour the notifications toggle, and open the bell on a click; the bell (`events`) is the state behind it. Engine-side for `dw-ffi`; `dw-app` forwards it as `dashPayNews` | DP2-05, E0-13 |
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

**From E0-10b** (version 9, DEC-183, no surface change):

36. **Content-bound provenance.** `unverified` (§5) compares a digest of the content shown with the digest the
    verification record stores, not the record's presence: a replaced entity reads as unverified until its new content
    verifies, and pre-DEC-183 records (dw-appdb migration `trust_digest`) verify nothing. `contact_request` records
    (key `sender:recipient:$createdAt`, `provenance::contact_request_key`) follow the same model, so the contacts read
    model (DP2-01) checks content as well as the key. A `dpns_label` record binds the spelling shown and the owner
    (encoding `v2`; `v1` records verify nothing), so another spelling of the same normalized label, or the label
    shown for another identity, is unverified. Records are per network (each network's app.sqlite). The re-fetch also
    verifies contacts' cached profiles and the contact labels the read model reports (`Provenance::show_label`); a
    contact's identity has no record class. An `IdentitySummary`'s flag covers the names it shows, and an
    installation holding wallets from before E0-10b starts latched (migration `trust_digest`).

**From DP2-01** (version 10):

37. **The contacts read model** (§2.4). `ContactsPage` gains `updating: bool`: while a writer holds the wallet's
    state the last snapshot answers, and with none yet the page is empty, so the UI shows "updating", not "no
    contacts". `contact` and `pending_setup_count` are `platform.unavailable` then. Hosts re-query on
    `Platform{Contacts}`, `Identities`, `Payments` and the wallet's `HistoryChanged`. Rows flag `unverified` from the
    shown profile, the shown username (reported to the re-fetch with `Provenance::show_label`) and the requests the
    relation rests on, by content (item 36).
38. **User search, links and scans (DP2-03, version 11).** `search_users`, `resolve_user`, `my_user_link` and
    `verify_scanned` are implemented (§2.3, §2.4). `ContactError` gains `scan_unverified` (a scanned link or code
    whose username Platform does not give to it, as iOS's scanner refuses one) and `no_username` (`my_user_link` for
    an identity with no name). A `dapk` scan is kept as a session-lived key and expiry for the wallet that scanned
    it; `send_request` (DP2-02) reads it with the identities it names, signs the proof with
    `AutoAcceptProofSource::SignWithKey` without re-resolving the username, and spends the scan once sent, so a
    send refused before broadcast can retry. Only the bare contact form `dash:?du=&dapk=` is a contact code.
    Then (Sol r1): `ScannedContact.scan` became `code: ScannedCode` (`UserLink` or `AutoAcceptClaim{scan,
    expires_at}`), since a code's key cannot be tied to its identity by a scanner and its auto-accept is only
    claimed; a send claims its scan atomically (another is `contact.request_pending`) and spends it once it has
    signed, fail closed; scans begun before their wallet left keep no key, and the session's close drops them all.
    Each form drops one `.dash`, once. Then (Sol r2): a claim makes one proof (`proof_source` consumes it and
    returns the guard that spends the scan), and `ScannedContact.unverified` is the shown name's or profile's
    provenance, not the key's.

**From E0-04 P2b** (no surface change):

39. **Mode B under leases and call permits.** `register_name` accepts a lease id as its `grant`, as §1 promises
    every write; a vault grant id runs under a one-shot lease. Each library write runs under a call permit charged
    its quote. `dispatch_status` answers Mode B funding step ids (`registration/<draft>/funding`,
    `topup/<id>/funding`) by E0-04 §2a.5; `DispatchResolved` also fires for a sighting of the bytes in another wallet
    and for a resumable step whose call returned Ok. Repair's "Resolve stuck funding" is engine-internal
    (`NetworkSession::resolve_stuck_funding`) until DP1-02 gives it its facade call.

**From DP1-02** (version 14):

40. **Identity registration** (§2.2, "Done (DP1-02 …)"). The registration rows get real bodies:
    `registration_quote`, `start_registration`, `registrations`, `resume_registration`, `discard_registration`,
    `finish_asset_locks`, `prepare_faucet_lock` and `start_registration_past_unknown_funding` (DEC-197). New:
    `RegistrationStatus.name_failure` and `profile_failure`; `RegistrationWait::FundingUnknown` and `NeedsFinish`;
    `RegistrationError::InProgress { draft }`, `NotSynced` and `FundingUnknownRecent { draft, wait_secs }`; the codes
    `registration.coins_locked`, `registration.funded_elsewhere`, `registration.funding_cancelled`,
    `registration.not_synced` and `registration.funding_unknown_recent`.

**From DP2-02** (version 15, DEC-195):

41. **Contact requests, eligibility and the key upgrade** (§2.4). `eligibility`, `send_request`, `accept_request`,
    `ignore`, `unignore` and `enable_dashpay_keys` are implemented. `Eligibility` gains `ContactCannotReceive` (the
    contact has no DashPay keys, so no request can reach it); E0-13's bindings regenerate for it.
    - **Quotes are bounds, charges are measured (DEC-195).** Platform meters an IdentityUpdate's and a
      `contactRequest`'s fee against its live state, so no client can know the charge before the broadcast; the fee
      table's `identity_update` is an admission floor, never charged. `CONTACT_REQUEST_CREDITS` and `KEY_UPDATE_CREDITS`
      (`flows.rs`) are the bounds the balance and the grant are checked against, and the acceptance is charged ≤ quote,
      measured as the proved balance before less after, with no other write of the identity between (the per-identity
      lock). T2 measurement (testnet, 2026-10-11, credits): `enable_dashpay_keys` adding keys 4–5, quote 100,000,000,
      charged 61,330,100; `send_request`, quote 150,000,000, charged 94,048,500; `accept_request`, quote 150,000,000,
      charged 81,967,400 (DP1-06's cost table can start from these).
    - **One write lock per identity.** Every Platform write that spends an identity's credits or moves its revision
      or nonce holds it: contact requests and accepts, the key upgrade, DP1-03's name writes, and DP1-02's name
      and profile writes.
    - **Each flow signs with its own keys only.** The vault's PlatformOp grant names identity indices, not key ids,
      so the engine's identity signer carries the flow's key ids and refuses any other: `ContactRequest`, `Accept`
      and `AcceptAndPay` sign with the identity's first `HIGH` or `CRITICAL` authentication key that the vault
      holds, never `MASTER`;
      `EnableDashPayKeys` with `MASTER` and the keys it adds (dpp signs each added key with itself).
    - **A lease of another flow is `platform.grant_invalid`.** Each write names the flows whose leases it takes
      (`send_request`: `ContactRequest`; `accept_request`: `Accept`, `AcceptAndPay`; `enable_dashpay_keys`:
      `EnableDashPayKeys`); a lease without a credits budget is `platform.needs_grant{credits}`, one with less left
      than the quote `platform.grant_exceeded`. Each write runs under a call permit charged the quote from the
      lease's credits budget (item 39), and signs only through the permit's signer: a write that fails before it
      signs anything ends the permit `Cancelled` and the quote goes back; once a signature is out the quote is kept
      whatever the outcome (fail closed), so a lease id carrying one quote pays for one write that signed.
    - **Refused before the grant is redeemed:** eligibility, the balance against the quote, a flow with no key to sign
      with (`identity.keys_missing`), and a scan within 60 s of its expiry (`contact.scan_expired`).
    - **`RequestOutcome.request`** is the request's `$createdAt` (ms), as the journal's request kinds (§5).
    - **A grant is redeemed only while the vault holds a key the write signs with** (Sol r1–r3). The redemption is
      the authority: dw-vault `redeem_grant_holding_key` checks the write's candidate keys (its flow's enabled ECDSA
      authentication keys at the flow's levels, in id order: a document key, or for the key upgrade the master key)
      and redeems the `PlatformOp` grant in one section under the guard of the vault's record writes. No candidate
      held, the wallet's seed included, is `identity.keys_missing` with every grant left unused. A wallet removal
      takes the same guard to delete the seed: one that came first is that refusal; one that starts during the
      section waits until the grant is redeemed, the key still held, and the grant is then used. A Lock ends every
      grant (E0-04), so one in between refuses the redemption. Earlier, before a scan's expiry check, each
      candidate is asked in turn through the grant without using it (`grant_identity_key_matches`, a yes-or-no
      answer that signs nothing); that check is advisory. A lease id, redeemed already, is checked by its permit's
      signer (`can_sign_with`); a refusal drops the permit before any signature, so its quote goes back.
    - **No signature is made after its seed is deleted** (Sol r3). Every deletion of a seed from the live vault
      (`wipe_wallet_secret` on removal, `delete_wallet_secret`, and a recovery's replacement of the vault, whose old
      file it keeps on disk) first waits for every vault operation still running and keeps new ones out until the
      seed is gone, holding the vault's operation gate exclusively as Lock does (a wipe or delete without ending the
      epoch; a recovery ends it anyway). A vault signer reads the seed only inside an operation, so one that read it
      releases its signature before the deletion; while the seed is absent, one that starts finds none and signs
      nothing, as do signers that can no longer be issued. What the vault hands out (a revealed phrase or
      masternode key, a discovery's scan key, an auto-accept key) is the caller's once released and signs no write
      here; removal ends the discoveries first (DP1-05). A signature made before the removal may still broadcast
      after it, until E0-04 P3's dispatch fence: the residual below, as for Lock. No seed is replaced in place
      (Sol r4): `store_wallet_secret` refuses a wallet that already holds different records
      (`vault.wallet_secret_exists`, checked and written under the record-write guard, nothing written), so a new
      seed under an id goes in only after one of these deletions; the same records again only re-seal them. The
      vault does not bind wallet ids to seeds: the engine stores a seed only under the id it derives from it, and
      keeps the one the vault holds (a concurrent import of the same seed that stored first, perhaps with another
      derivation flag, included).
    - **E0-04's lease rule follows this one** (E0-04 amendment 3): a lease of another flow, passed to a write that
      names its flows, is `platform.grant_invalid`; `platform.needs_grant` only when no grant or lease carries the
      purpose.
    - **Residual (accepted, E0-04 P3):** Lock stops new signatures; a write already signed when Lock lands may still
      broadcast until E0-04 P3's dispatch fence (§4, the Lock reading). So may one signed before a wallet removal
      deleted its seed. DP2-02 builds no fence of its own.
    - **Residual (accepted, Sol r4):** wipe and delete keep the vault's epoch. When the same seed is imported again
      with no Lock between the removal and the import (same seed, same epoch), a signer issued before the removal
      and a grant not yet redeemed work again, with no fresh unlock. A Lock in between, or a recovery (which ends
      the epoch), ends both.
    - **Residual (accepted, Sol r5):** an `accept_request` that adopts our request already on Platform signs
      nothing and returns `Ok`, so its call permit ends `Sent` (E0-04 P2b's `finish(ok)`) and the quote stays charged
      to the lease's credits budget though no Platform credits were spent. Conservative: the lease pays for at most
      one write either way.

**From DP2-04** (version 16):

42. **Private details** (§2.4). `set_private_details` is implemented, and `contact`'s `publish_state` can now be
    `Published`. New: `ContactError::DetailsTooLong { bytes, max }` (`contact.details_too_long`); E0-13's bindings
    regenerate for it.
    - **What Platform holds is recorded as a digest.** app.sqlite `dp_contact_info` keeps, per (wallet, identity,
      contact), SHA-256 over a domain and the details' DIP-15 encoding; no plaintext. Details read `Published` when
      their digest equals the row's. A publish that landed writes the row, and so does each read of the identity's
      documents; a contact Platform holds nothing for loses its row, and so does a contact no longer established.
      `.dwbackup` does not carry the table (dwbackup-v1 §3.1): a restore rebuilds it from the read. `contact` reads
      the rows from memory (§1): each wallet's are loaded in the background on its first read, which signals
      `Platform{Contacts}` for the identities that have any (until then `publish_state` reads `Local`), and every
      record and forget writes through.
    - **The read (a settle).** A settle requires a verified seed binding: first the vault's prompt-free DashPay
      crypto must derive the wallet's own account-0 xpub (platform-wallet `verify_seed_binds`), checked in every
      settle, whatever a contact-crypto drain reported; a failed check changes nothing, and a proven mismatch makes
      the wallet's startup `SeedBindingUnverified` (noticed once). The check needs the wallet's BIP44 account 0
      (desktop creates a wallet from a seed with it, `WalletAccountCreationOptions::Default`); a wallet without it
      never settles. Then the engine fetches the identity's `contactInfo` documents
      (`$ownerId`, ordered by `$updatedAt`, pages of 100, at most 1,000 counted as Platform returns them, parsed or
      not) and opens each with that crypto. One deadline, 30 s, covers the check, the fetch and the opens; it and the
      session's close are checked before each open, the deadline again after the last open and before the apply phase.
      Past it the settle fails with `platform.timeout` and applies nothing. The apply phase (the details saved with
      each established contact and their `dp_contact_info` rows: local writes only, bounded by the established
      contacts, no network) runs to its end under the lock once it has started within the deadline. A document whose keys the wallet does not hold or whose seal does
      not open is skipped, with a warning; so is one whose plaintext does not decode, as the library skips it. A
      locked vault, a failed fetch, the deadline, or documents of which none opens fail the read, which then changes
      nothing and never guesses `Published`. A settle runs in the background (a bring-up or an unlock does not wait
      for it) after each bring-up of a wallet with an identity and after each unlock drain, under the identity's write
      lock (held across the read, so an edit of the identity waits for it), and applies Platform's details to each
      established contact. A record that changes, with or without the details, signals `Platform{Contacts}` so a host
      re-reads `publish_state`. That is how a restore gets its details back
      (acceptance: `recovery_tests::a_restored_wallet_gets_its_private_details_back`, with the vault unlocked at the
      bring-up, and locked until after it). A settle or an edit writes nothing of a wallet once it has left the wallet
      manager (a removal, an unload, a restore's rollback): each of their writes lands only while the wallet it read
      is still the manager's, and the wallet leaves only after the writes in flight have landed, so a removal's delete
      of its rows comes after them.
    - **Writes after a wallet's removal (DEC-210).** Any other app.sqlite write of a wallet still under way when the
      wallet is removed (a DashPay choice or preference, the journal written as the library stores a changeset, a coin
      lock, first-seen times) lands nothing either. The removal records the wallet (`removed_wallets`), committed before
      the transaction that deletes its rows; from then on an insert of a row of that wallet, in any table the removal
      clears and in its two settings scopes, is skipped, and, once its rows are gone, an update finds none.
      `add_receive_request` and a new registration flow then fail (`storage`). If that delete fails, or the record
      cannot be written, an import of the wallet in the same session deletes the rows the removal left before its first
      write, and is refused (`storage`, "the removal of wallet … could not be finished") while it cannot; an open that
      forgets a record deletes them first too, and keeps the record while it cannot. A removal that could not write the
      record leaves its rows to an import in a later session (residual). The record lasts until an import of the same
      wallet (a seed, a watch-only xpub, a restore), which takes it out before its first write (an import that then
      fails to register the wallet records it again), or until an open of the network's `app.sqlite` while no other
      instance of it is open, in this process or another (DEC-210 (3b)). Each open instance holds a shared lock on
      `app.sqlite.lock` for its whole life, a job a closed session left running included; an open forgets the record
      only if an exclusive lock on that file is granted at once, and otherwise keeps it. The lock file is named after
      the database, so each has one name: an open refuses (`storage`) a database or lock file that is a symlink, not a
      regular file, or a file with a second hard link, a name starting with `file:` (which SQLite reads as a URI), and
      an open whose database or lock file changed between its check and its use; symlinked directories above the
      database are fine, since they lead to the same lock file. Imports, restores and their rollback, removals, and the
      open and close of a wallet run one at a time per network within a process (the engine's wallet lifecycle lock,
      taken after the operation's admission and before the wallet's barrier, discoveries and private-details fence), so
      a wallet is never recorded as removed while it is registered, and two imports of the same seed run one after the
      other. `.dwbackup` never carries the record (dwbackup-v1 §2.3). **Residuals (accepted, DEC-210 (5) and (3b)):** a
      write of the removed wallet still under way when the same wallet is imported again lands among the new import's
      rows; an insert that is skipped shows in the log only as "no rows" (`raise_setting`) or as a held index
      (`adopt_registration_lock`); on a file system whose advisory locks are not kept per open file (an NFS emulation of
      `flock`), an open in the same process forgets the record while another instance is open; a long restore holds the
      wallet lifecycle lock until its last bundle is done, so a removal or another import waits for it; and two
      processes on one network directory are kept apart only by the record and the lock file, not by the wallet
      lifecycle lock, which is per process. Renaming, moving or bind-mounting the database file while an instance is
      open gives it a name with another lock file (SQLite does not support moving an open database either), and deleting
      or replacing `app.sqlite.lock` after an open has returned leaves that instance's lock on a file later opens no
      longer see; the app never removes the lock file itself. On Windows only a final symlink is refused: hard links are
      not counted, file identities are not compared (std has no file id there), and other names of one file (an 8.3
      short name, a trailing dot, `::$DATA`) lead to another lock file.
    - **An edit of published details needs a grant.** The library's DashPay pass applies Platform's `contactInfo` over
      local details on every pass (remote wins), so a local-only edit of a contact Platform holds details for would
      go back silently. It is `platform.needs_grant{credits}` and the edit is not saved. Re-saving the same details
      needs no grant. With no record for the contact (Platform held nothing at the last read, or no read succeeded),
      an edit without a grant first reads Platform when the vault is prompt-free, so details there (another device's
      first publish too) are known before the edit is let through; that read applies them, as any read does.
      **Residual (accepted):** when that read cannot run or fails (vault locked, offline), the edit is saved, `Local`,
      and the library's next pass may put Platform's details back over it. **Residual (accepted):** the read runs on
      every such edit of a contact that has no published details, under the identity's write lock, so an edit can
      take up to the read's 30 s deadline.
    - **No publish at the second contact by itself.** Details deferred below two contacts publish with the next edit
      that has a grant. Follow-up (a later Contract-Version): publish every deferred contact's details at once when
      the second contact is established, from one grant.
    - **A stale record.** With a grant, details count as already on Platform only when the contact shows them and
      the record matches; a record left over from before another device's edit does not skip the write.
    - **A failed write.** One that signed nothing (`Cancelled`) puts back the details shown before the edit, since
      the library saved the edit first; with nothing recorded the edit stays, `Local`. One that signed may still land, so it is read back: details Platform holds are `Published` and
      recorded. Otherwise (other details, or the read-back failed) the edit stays, `Local`, with the write's error,
      and the identity is read again 60 s later, up to 5 tries (a try while the vault needs a prompt reads nothing),
      then at the next bring-up or unlock: details that landed late become `Published` without another write, and
      details that did not are replaced by Platform's. **Residual (accepted):** these retries are not merged, so
      several failed writes of one identity each schedule their own.
    - **Refused before the grant is redeemed:** `contact.self`, `contact.details_too_long` (the engine's cap is
      DIP-15's plaintext limit, 2031 bytes encoded; dwcli keeps its 256-byte argument caps), a contact that is not
      established (`invalid_argument`), a watch-only identity (`invalid_argument`, no new surface), no DashPay keys
      (`contact.ineligible{NoDashPayKeys}`), the balance below `DOCUMENT_10K_CREDITS`, no document key the vault
      holds (`identity.keys_missing`), and more than 1,000 `contactInfo` documents
      (`platform.not_implemented{call}`, see below), or a failed fetch of them. A lease of another flow is `platform.grant_invalid`. `PrivateDetails` signs
      with the identity's first `HIGH` or `CRITICAL` authentication key, never `MASTER` (item 41).
    - **Residual (accepted): `acceptedAccounts` is empty.** platform-wallet writes an empty list and never reads the
      field. Routing is unaffected. A `contactInfo` is encrypted to its owner's own keys, so only the owner's devices
      read it, and a contact pays to the xpub in the owner's `contactRequest` (account 0,
      `register_external_contact_account`). DIP-15 defines `acceptedAccounts` as the contact's account references the
      owner accepted; desktop uses account 0 only. iOS goes through the same rs-platform-wallet call, so it also
      writes an empty list. Android's dash-wallet has no `contactInfo` yet (a TODO in `IdentityRepository.kt`; it picks
      a request by timestamp). The cost: a document another client wrote with accepted accounts loses them when
      desktop rewrites it.
    - **Residual (accepted): the newest document wins.** Another device's later edit of the same contact replaces
      ours, at the next read here and at the library's next pass.
    - **Residual (accepted): the digest is unsalted.** A row's digest of short details could be matched by guessing,
      but wallet.sqlite holds the same details in plaintext next to it.
    - **The library's publish is not bounded.** platform-wallet's `set_contact_info_with_external_signer` fetches and
      opens every `contactInfo` document the identity holds before it writes, with no cap or deadline of its own.
      The engine reads them first, within the read's cap and deadline, and refuses a publish over 1,000 before the
      grant is redeemed. **Residual (accepted, DEC-206):** documents added between that read and the library's own
      fetch are still read without a bound, under the identity's write lock; that fetch only reads documents owned by
      the wallet's own identity. A bounded publish API in platform-wallet closes it at the next fork bump. A publish costs two fetches, and a failed one's read-back a third.
    - **T2 is pending tDASH.** `private_details_tests::t2_testnet_private_details` (ignored,
      `DW_DP204_TESTNET_FIXTURE`) is written. Fixture wallets A and B stay read-only, and C has one contact (A), so the
      run needs a new wallet D, an identity with DashPay keys funded with tDASH. D asks C, C accepts, and C publishes
      details for A and D (charged ≤ quote); the same details again send nothing; and C restored into a fresh engine
      shows them `Published`. It waits on pasta's tDASH, alongside DP1-02's testnet registration.
43. **Notifications (DP2-05, version 17).** `events`, `unread_count` and `mark_read` are implemented (§2.6) over E0-06's
    journal, with no new table: read state is `dp_events.read_at`, which `.dwbackup` already carries and wallet removal
    already deletes. `PlatformChange` gains `Events{identity}`, and `EngineEvent` gains `DashPayNews` with the public
    const `MAX_NEWS` (§6); the rules are §5's "the feed" and "news".
    - **Historic is the catch-up boundary's**, not "before the first completed sync": an event whose authoritative
      time predates the wallet's boundary is stored read and never news (DASHPAY §2.7). A seed restore therefore
      makes no news for the history it replays, and a `.dwbackup` restore none for the rows it imports.
    - **Pages walk an index.** `events` reads `dp_events_page` (`wallet_id, identity, id DESC`; app.sqlite
      migration `V2026101303`) from the cursor down, so a page reads the rows it returns and the ignored ones it
      skips, never the identity's whole journal. An index only: `.dwbackup` classification is unchanged.
    - **A mark is the admitted wallet's.** `mark_read` (and `ignore`'s mark) writes under DP2-04's per-wallet write
      fence, holding the wallet it was admitted for: a removal, an unload or a close waits for a mark in flight,
      and a mark that outlives the wallet fails with `wallet_not_found`, so a reimport's restored rows stay unread.
    - **A journal write is its wallet's.** E0-06's writer journals a library changeset for the wallet as the store
      call began: if the wallet's removal (or a restore's rollback) overtakes it before its rows are written, it
      writes none and makes no news, even into the same wallet imported again. The check is made with app.sqlite
      held, after the removal's rows are gone and before any readmission. A removal whose app.sqlite delete then
      fails has still moved the wallet on once it left the wallet manager, so such a write is dropped then too.
      A restore's rollback that keeps the wallet (keys it attached to a watch-only wallet) drops such a write's
      rows too; a watch-only wallet has no identities, so no DashPay changeset is in flight there.
    - **News only from inserted rows.** The tap's journal write reports the rows it added; nothing else (a read, an
      import, a replay) produces news, so a restart cannot announce again what a previous session announced.
    - **Hidden contacts** are not filtered (DASHPAY §3.5 journals nothing for hide or unhide; iOS's
      `DashPayContactsNotificationProducer` and `DashPayNotificationsReadState` never consult `isHidden`). **Ignored
      senders** are a local mute: the library stops ingesting their requests, `ignore` marks their unread events read,
      and `events` leaves their rows out until `unignore`.
    - **Residual (accepted): a restore's names may count unread.** DEC-114 (4): a name journaled before its
      marketplace time is known is stored unread, so a restored identity's names can show in the bell's `new` and
      count once. They are never OS news (`UsernameRegistered` does not notify); the public-testnet T2 run saw two.
    - **A restored contest watch** (closed by DP1-04). `dp_contest_watch` travels in `.dwbackup`; the watch, not
      the name snapshot, journals its `ContestWon`, aged by the resolving block, so an old win a restore finds is
      stored read like any history (item 45).
    - **Residual (accepted): an ignored sender's late row.** The library stops ingesting an ignored sender's
      requests, so no new row comes from them; one that did (a journal write that had failed before the ignore)
      would count unread while `events` leaves it out, until a `mark_read(u64::MAX)` or the next `ignore`.
    - **Cadence.** `MAX_NEWS` caps one event, not a time window: a steady stream can bring up to 4 `DashPayNews`
      a second per identity, and §6's "up to 3, else a summary" is per event.
    - **Residual (accepted): a payment is not news here.** An unread `PaymentReceived` counts and shows in the bell,
      but the OS notification is M2's `NewTransactions` one, with no contact name; DP3-02 added `counterparty` to
      the records, and the notification is unchanged (item 47).

**From DP1-06** (version 18):

44. **Credits: refresh, the cost table, top-up and the low-credit levels** (§2.1, §2.8). `refresh_balance`,
    `cost_table`, `top_up_quote` and `top_up` are implemented; new: `pending_top_ups`, `resume_top_up`,
    `resolve_top_up`, `TopUpState` (with `ConsumptionUnknown`), `PendingTopUp` (with `consumption_unknown`),
    `CreditLevel` and `IdentitySummary`'s `credit_level`, `balance_as_of` and `balance_stale`, `CostTable`'s
    `protocol_version`, `top_up_fee` and `empty_credits`, and the codes `credits.coins_locked` and
    `credits.top_up_pending{txid}`. E0-13's bindings regenerate for them; dwcli gains `identity top-up-pending`,
    `identity top-up-resume` and `identity top-up-resolve`. The withdrawals stay stubs (DP6-02).
    - **The cost table is the network's.** It prices the SDK's protocol version (Platform's proved metadata raises it
      from the seed floor) with that version's `FeeVersion`: the documents through Drive's own document cost
      estimator (`drive::drive::document::cost`, the one wasm-sdk exposes), every field present at its maximum size;
      the key upgrade as the IdentityUpdate fee plus 2 × 1,792 bytes of key
      storage; the top-up fee as dpp's `IdentityTopUpTransition` minimum. The estimator follows protocol version 14's
      insert methods and refuses older ones, so a network below 14 (testnet reports 13 today) is priced at the first
      version it accepts, with the network's own fee version.
    - **A document's price is an upper bound, not the estimate** (review Sol r1). The estimator's storage and its
      exact processing (the fixed operations) are what Drive charges; the rest of the processing depends on the
      trees' depth, which the estimator takes from an assumed number of existing documents (1,000 by default) and
      which it estimates within a factor of two (Drive's own test). The price therefore assumes every tree the
      document writes holds 2^48 entries (an AVL tree that deep holds more than 2^33, beyond any DashPay contract's
      size) and doubles the estimated part. A test asserts the price is at least the estimate at 1,000, 1,000,000,
      2^32 and 2^48 entries for each document and every known protocol version. At protocol version 9 and later the
      prices are 139,638,280 (`contact_request`), 222,187,240 (`profile_update`) and 230,453,000 (`contact_info`),
      against estimates at 1,000 entries of 99,314,660, 118,437,540 and 129,768,820. Measured on Drive at protocol
      version 13 (each document at its largest, 64,000 inserts of fresh owners and index values into one contract),
      the dearest single insert was 103,078,900, 133,172,080 and 146,105,500 (a tree rebalance; the trend at 64,000
      was 100,279,600, 121,522,880 and 135,218,840): above the default estimate for `profile_update` and
      `contact_info`, below the price for all three. The write grants' caps stay (150,000,000 per request, 300,000,000
      per document) and cover the prices. No price is an engine constant; the engine's constants
      are sizes (the key bytes) and counts (ten actions for Low). T0: a change of any fee field used changes the
      table. The write paths' grant bounds (`flows.rs` `CONTACT_REQUEST_CREDITS`, `DOCUMENT_10K_CREDITS`,
      `KEY_UPDATE_CREDITS`) stay as they are, and a test asserts they cover the table at every known protocol
      version. Measured charges are below the table: `send_request` 94,048,500 against 139,638,280,
      `enable_dashpay_keys` 61,330,100 against 98,301,600 (testnet, item 41).
    - **The low-credit levels are Android's** (`CreditBalanceInfo`), with the price from the table: `Empty` below one
      of the dearer of a contact request and a profile update, `Low` at or below ten. The UI's copy is Android's
      (DASHPAY F8): Low "Your credit balance is low" / "Top-up your credits to continue making changes to your
      profile and adding contacts"; Empty "Your credit balance is fully depleted" / "You can continue to use DashPay
      for payments but you cannot update your profile or add more contacts until you top up your credit balance";
      buttons "Maybe later" / "Buy Credits".
    - **A top-up keeps no table of its own.** Its state is the library's tracked `IdentityTopUp` asset lock and the
      dispatch journal's funding step (E0-04 P2b): identity `i`'s top-ups fund under `topup/<i>-<k>/funding`, `k` the
      first step the funding gate admits. A step that funded a lock (`Sent`), one whose marker stands over nothing
      sent (a crash before its signature) and one Repair resolved are passed; one read `MaybeSent` or `WillBeSent`
      refuses the top-up with its code, so a funding whose outcome is unknown is never repeated; one read `NotSent`
      is funded again under the same step. A `topup/` step matches only the locks its own funding call signed (its
      `signed_lock` records), never another top-up's lock under the same key, across restarts too; P2b's earlier
      refusal of `topup/` steps is lifted for steps under an `IdentityTopUp` key.
    - **One lock, one top-up; only a proof settles a lock** (Sol r1). While a top-up lock of the identity pends, a
      new top-up is `credits.top_up_pending{txid}`. A lock pends while it is out or proved and not applied, and also
      when a top-up of this wallet funded it and the library holds it `RecoveredFromChain` (final, Platform's
      consumption unknown: a restart lost the apply, or Platform answered "already consumed"). Only Platform's proved
      apply settles a lock (the library marks it `Consumed`, and the dispatch journal records it settled, below); the "already consumed" answer is unauthenticated, so
      it reads `ConsumptionUnknown`, concludes nothing and hides nothing, in this process or after a restart (the
      library marks the lock `RecoveredFromChain` when it can get its ChainLock proof, and otherwise leaves it as it
      was, still pending). A lock is the wallet's own top-up's when its funding step's `signed_lock` record names
      it; the journal keeps those records for good (a `Sent` step is never compacted, so a wallet's removal keeps
      them too). Such a lock pends, `consumption_unknown`, until a proved apply (`resume_top_up` or Finish) or the
      user's `resolve_top_up`. Both are recorded in the dispatch journal's `top_up_settled` table (schema 8, keyed
      by wallet and lock, never deleted), and so is a lock of the wallet's own the library holds `Consumed` without
      a record yet (one applied before the table existed, or whose record failed): the library's `Consumed` mark
      lives in `wallet.sqlite`, which a removal erases, so a wallet removed and imported again would otherwise read
      every past top-up back `consumption_unknown`, each blocking. A lock recorded settled is settled whatever its
      status (the library's live advance may overwrite `RecoveredFromChain`). A recovered lock no top-up of this
      wallet funded (a restore reads every earlier top-up back from the chain) is historical: settled, neither
      pending nor blocking. Finish offers every lock the library does not hold applied to Platform, settled ones
      too (asking spends nothing; a lock Platform had not taken credits the identity), counts an unproved "already
      consumed" as `still_pending` for a pending lock, and counts a settled one only when Platform applies it
      (another answer, such as a cached refusal of the same transition, counts nothing, as `pending_top_ups` lists
      nothing for it). Until the dispatch journal is read no recovered lock can be told apart: the quote, the
      top-up, the pending list and the resolution are `platform.unavailable` (the host retries; review L5, before
      the top-up looks at its step, which would otherwise read `MaybeSent`), and Finish counts every recovered lock
      as the wallet's own (fail closed). The apply builds
      nothing: `FromExistingAssetLock` signs only the proof, with the lease's funding signer taken before the funding
      call charges the budget, under a plain call permit. The identity's write lock (item 41) is held from
      before the grant is redeemed to the apply (Finish and resume read the lock again under it), so `credits_added` is the proved balance after less the one read before, with no other
      write of the identity between.
    - **Restart at any point loses nothing and funds nothing twice** (T0, `topup_tests.rs`): a crash before the
      funding marker leaves the step unused, and the next top-up funds under it; one before the funding signature
      passes its step; one after the lock is tracked leaves it pending (`resume_top_up` waits for its proof and
      applies it); one before the apply leaves it proved (Finish or resume applies it); one after Platform took it
      reads `ConsumptionUnknown`, with the credits added once, and pends until resolved. A vault Lock before the
      funding refuses the call with nothing funded; one after leaves the lock pending (`NotApplied`), and a new
      grant resumes it. These scripted crashes keep the fake library's state; a storage-backed fake keeps its locks
      in the wallet's own store, so its restarts read them, their identity index and a `RecoveredFromChain` mark
      back from `wallet.sqlite`, and a false "already consumed" stays pending and blocking across a restart until
      Platform's honest apply credits the lock once.
    - **The grant is redeemed last.** The quote's refusals, the step's, and the grant's funding cap (read without
      redeeming it, as a registration does) come first, so a refused top-up leaves the grant usable (Sol r1).
    - **The balance shown follows the spending** (Sol r1, `balances.rs`). Every credit-spending write (a name, a
      contact request or accept, the key upgrade, a profile, private details) marks its identity's balance spent
      and, once its write lock is released, refreshes it in the background (proved, stored with its block time);
      each DashPay pass the engine runs, and a pass every 60 s while SPV runs, refresh every stale balance: spent
      since its refresh, or neither refreshed by this process (its clock) nor stored by Platform (its block time) in
      the last ten minutes, so a clock ahead or a stalled chain does not refetch it forever. A refresh clears a
      spend only with a balance whose block time is at or after the spend's end by this machine's clock (a node a
      block behind answers the balance before it; Platform may have none); a failed refresh leaves it stale.
      `IdentitySummary.balance_as_of` and `balance_stale` show how fresh the balance (and so `credit_level`) is, and
      each mark and refresh sends `Platform{Identities}`, so the host reads the flag again. A background refresh
      holds neither the session nor its gate; `close` aborts and awaits them before the wallets unload, and none
      starts after.
    - **Residual (accepted):** the quote's credits subtract `top_up_fee`; Platform's charge is metered at execution
      (T3 measures it; item 46 corrects what this said about it). `credits_added` reports what was added. A lock Platform took
      whose "already consumed" the library cannot prove final stays pending until its ChainLock lets the library
      mark it and the user resolves it. A machine clock behind Platform's by more than a write's round trip can
      clear a spend with the balance before it; the next refresh past ten minutes corrects it. A user's resolution of a lock Platform had not taken hides it from the
      pending list and the top-up check, not from `resume_top_up` or Finish. No proved query of a lock's
      consumption exists to settle it without the user.

**From DP1-04** (version 19):

45. **The contest watch (DP1-04, version 19).** `contest_status` is implemented (§2.3) and the engine watches the
    wallet's own contests until their vote resolves (`contest_watch.rs`, not facade). No surface or schema change: the
    rows are E0-07's `dp_contest_watch`, the outcomes E0-06's `Contest*` kinds through `ChangesetTap::journal`.
    - **Rows.** `register_name` (and the registration flow's name step through it) writes one when Platform shows
      the identity contending, with the end it learned; each pass also adopts every open contest the library holds
      (`contested_dpns_names`) with no row, end unknown, in the spelling of the identity's `contested_name` row when
      it folds the same (the library keeps the folded label). One row per contest however the two race (one
      transaction, DPNS's normalized compare); a registration clears an `unplaced` streak. `last_state` keeps what
      the watch last saw: `open`, `won:<ms>:<since>`, `unplaced:<since>`. Nothing repopulates the library's open
      contests on a seed restore, so a restored wallet's contests are watched only through rows a backup restored.
    - **Outcomes** (DASHPAY §3.5): from `get_contested_dpns_vote_state`'s winner, as both mobile apps map it: this
      identity → `ContestWon`, another identity → `ContestLost`, `Locked` → `ContestLocked` (nobody gets the name,
      now or later at this pin; no outcome returns the contest fund; another name can be requested), `NoWinner`
      (Drive stores it only for a poll with no contender) → `ContestLost`. No winner while the identity is among
      the contenders is still voting, and so is an open poll's full page of 100 contenders (a contest allows
      1,000) that does not list the identity: it may be on a later page, so the row is asked again each time it is
      due. A resolved poll lists every contender (Drive answers its stored result, whatever the limit). A poll whose
      complete list lacks the identity (resolved, or open and short of a full page), or no poll at all, is no
      outcome (`contest_status` refuses it with `invalid_argument` unless the label is watched); such a row stops
      being watched after a day, with no event. The event's age is the time of the
      block that resolved the poll (authoritative), so catch-up silence applies: a restore that finds an old outcome
      stores it read.
    - **After an outcome.** A loss or lock is journaled, then the label leaves the library's open contests and the
      row goes. Only the watch journals `ContestWon`: the tap's name path stores a watched label's
      `UsernameRegistered` read and journals no win of its own, whichever comes first (the marketplace may list a
      name before its name-state row and before the watch asks), so a win's age is always its resolving block. A
      win is journaled at once; the row stays (`won:<ms>:<since>`, no further query) until the library lists the
      name; the watch asks the marketplace loop for a pass at most every 10 minutes meanwhile, and stops waiting
      after a day (a later listing is a plain `UsernameRegistered`). The event's ref is
      `label@ends_at` with the row's stored end, so a retry names the same event; the journal write comes first, and
      a failed write is retried at the next pass (the dedup key makes it one row).
    - **Cadence and budget.** The supervisor (E0-05) runs a pass for every wallet right after the loops start (a
      contest that ended while the app was closed shows at once), after a wallet is brought up while SPV runs, and
      when `register_name` writes a row; then when the next row is due: every 10 minutes while its end is unknown
      or more than an hour away, every minute from an hour before the end to an hour after it, every 10 minutes
      after that, a won row every minute (no query); at most one pass a second, however it is asked for. No rows, no
      timer. A pass makes one vote-state request per due row (one page), in turn, at most 16 rows per wallet (the
      longest unchecked first, the rest next pass), and one end-date request (one page of 100 polls, the
      soonest-ending from an hour ago on) while a due row's end is unknown; a row whose end is past that page keeps
      the 10-minute cadence. It has 20 s.
      `PlatformCadence.contest_ending_soon` now only speeds the marketplace loop (m1-engine item 8).
    - **Not before a bring-up.** A wallet with no catch-up boundary yet (a restore whose pass would come before its
      first bring-up here) is not watched and arms no timer; its bring-up re-arms the watch, so an old outcome a
      restore finds is stored read.
    - **A pass is its wallet's.** Its reads are unfenced; every write (the row, the journal, the open contests)
      holds DP2-04's write fence for the wallet object it read, with the tap epoch read under it, so a pass that
      outlives the wallet (removed, or reimported) writes nothing, and a removal waits for one in flight. A removal
      also ends the wallet's pass and forgets its schedule.

**From the live testnet run** (version 20):

46. **A held funding runs the bring-up again; the top-up quote errs low (dw-tdash B2, B5, version 20).**
    `RegistrationWait::Startup` is new; no other surface change.
    - **The wedge.** On testnet a registration waited at `Network` for over 10 minutes with SPV caught up and DAPI
      answering: the session's bring-up had ended `PartialNoIdentity` (the created-here budget of 3 s overran, or
      the library's own outcome), and nothing in the session ran it again, so only a restart freed the flow. A
      wallet whose bring-up Platform's proof of no identity (7 days) had skipped held its registrations at `Sync`
      the same way, restarts included.
    - **The hold.** Once SPV has synced, a bring-up running or ended without the seed's identity indexes
      (`Starting`, `NotRun`, `PartialNoIdentity`, `DiscoveryFailed`, `IdentityScanIncomplete`) holds a funding at
      `Startup`, never `Network`. A registration run waits there in the run with its lease, polling every 5 s, as it
      waits at `Sync`; a run that does not wait (Finish transfers) parks. `IdentityUnsettled` still parks at
      `Unlock`. `top_up` held there is `platform.unavailable`.
    - **The rerun.** Each poll of a run waiting at `Startup`, each `top_up` held there, and each read of a
      registration no run drives that the hold shows at `Startup` (`registrations`: the host prompts no resume until
      the hold lifts) asks the session's supervisor (E0-05) to run that wallet's bring-up again, in the background. The first request runs it at
      once; later ones only once 30 s, then 1, 2, 4 and 8 minutes, then every 10 minutes, have passed since the
      last request, for as long as flows keep asking and the outcome keeps holding them. Nothing runs while SPV
      is stopped (the next start brings every wallet up and the schedule starts over), for a wallet without
      keys, or while a bring-up of the wallet runs; an outcome that answers ends the schedule. A rerun gets the
      library's 20 s budget even for a wallet created here (nothing waits for SPV then), and it runs the
      discovery whatever Platform proved before: the funding needs this session's answer (Restore rule 1). Its
      outcome is recorded as any bring-up's; `Notice{DashPayStartupIncomplete}` only when it differs from the
      outcome the rerun was asked for.
    - **The quote errs low.** `top_up_fee` is Drive's `calculate_min_required_fee` for IdentityTopUp: what a lock
      must hold before Drive processes the top-up (50,000 duffs plus the top-up's base cost: 50,500,000 credits at
      protocol version 13), not the fee charged. T3 on testnet (protocol version 13) funded a 1,000,000-duff lock:
      Platform charged 7,897,240 credits and added 992,102,760, against a quote of 949,500,000. So
      `TopUpQuote.credits` is an estimate below what a top-up adds as far as measured (T3 now asserts it), not "at
      most"; nothing proves the metered charge stays below `top_up_fee`. `credits_added` reports what was added.

**From DP3-02** (version 21):

47. **Contact payments: attribution, activity and the frequent strip (DP3-02, version 21).** `contact_activity` and
    `frequent_contacts` are implemented (§2.5); history and transaction records carry `counterparty` (§6). No surface
    change in the facade; m1's `TxRecord` gains the field. `attribution.rs` (not facade) builds it; the notes are in
    `docs/design/DP3-02-notes.md`.
    - **Attribution (DIP-15).** A record is a contact's when its output pays an address of a DashPay account: one
      of `identity`'s receival accounts with that contact (a receive record: *from* the contact) or one of its
      external accounts (a send record the wallet funded: *to* the contact). The pools a wallet holds always cover
      what it detected, so a received payment is attributed after a restore even when its coin was spent. A send to
      an external address past the resident pool (a restore regrows external pools only to the gap) takes the
      library's word: its `Sent` payment entries for the txid, when they name exactly one (identity, contact) pair
      and the transaction has exactly one unattributed payee record. Anything else stays unattributed.
    - **Not attributed:** a payment between two identities of one wallet (a SendToSelf), mixed or foreign-funded
      debits (`Other`), coinbase, CoinJoin and Platform transfers. The history text search does not match contact
      names.
    - **One source.** Activity and the strip read the same attribution as history, so the three agree. A sent
      activity amount sums the transaction's outputs to the contact (no fee, no change).
    - **Cursor.** 63 characters, `[0-9a-f]` plus the direction: the block time (unmined: the end of time), the
      txid's first 46 hex digits, `s` or `r`. Pages never repeat or skip an item; a payment that lands meanwhile
      shows on a re-query, not inside later pages.
    - **The strip** counts one per transaction and direction; an unmined payment counts as now. Its contacts are
      `contacts`' established and not hidden ones.
    - **Residual (accepted):** a restored send whose address is past the pool and whose library entry is not yet
      rebuilt (the DashPay pass after a restore) shows no counterparty until the pass; the host re-queries on
      `Platform{Payments}`. The OS notification for a payment is unchanged (M2's `NewTransactions`).
