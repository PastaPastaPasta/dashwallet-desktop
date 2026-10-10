# M1 engine contract (`dw-ffi`)

Status: **contract**, 2026-10-05. Code: `rust/crates/dw-ffi/src/api/*.rs`; generated Swift:
`Sources/DashWalletCore/Generated/DashWalletCore.swift`. Design background: DESIGN-opus §1.5 (FFI rules),
§1.8 (vault), §1.12 (runtime). The Swift side of the contract is [`m1-swift.md`](m1-swift.md). M2 adds calls,
events and error codes and changes some M1 behaviour: see [`m2-engine.md`](m2-engine.md) §5.

Every M1 call exists in the FFI now. A call whose engine side has not landed returns its domain's
`NotImplemented { call }` error, where `call` is `"<Object>.<method>"` or the free-function name. No call
fakes success. Implementers replace the stub body; they do not change a signature without updating this
file and the Swift seams in the same change.

Owners:
- **E1 engine-core**: sync, history, receive, balances, events, wallet list/info/remove/rename.
- **E2 engine-send**: send, coins, labels/address book, message, uri, units, the `dw-appdb` crate.
- **B vault**: vault, generate/check/import, `VaultSigner`, Core-quirk seeds through `dw-compat`.
- **C Swift runtime**: DashKit and the WalletRuntime adapters (see m1-swift.md).

## 1. Conventions

| Topic | Rule |
|---|---|
| Wallet id | 64-char lowercase hex `String`. Every call that takes one parses it first and returns `invalid_argument` for a malformed id (upper case included), even when the rest is a stub. |
| Txid | 64-char lowercase hex, display (RPC) byte order. |
| Amounts | Duffs. `u64` for quantities, `i64` for signed net amounts (history). |
| Times | UNIX seconds `u64`. `None` means unknown; never 0 as a placeholder. |
| Unknown values | `Option`. A balance, fee or height the engine does not know is `None` (iOS rule 7). |
| Secrets in | `Vec<u8>` (Swift `Data`): passphrases, phrases, quick-unlock keys. Rust wraps them in `Zeroizing` on entry. |
| Secrets out | Only `generate_mnemonic` and `Vault.reveal_mnemonic`, as bytes. DashKit copies them into `SecretBytes` at once and zeroes the `Data`. Residual risk: UniFFI frees the returned `RustBuffer` without zeroing it (B decides on a zeroing transfer type). |
| Async | Every `async` export spawns its work on the engine's tokio runtime (`NetworkSession::on_runtime` pattern) and only awaits the join handle, so UniFFI's Swift executor never blocks. |
| Sync | A sync export is O(1) or reads in-memory state. No SQLite or network I/O on the caller's thread: E1/E2 keep the data a sync call needs (wallet names, sync snapshot, peers, balances) in memory. |
| Close | `close_network` waits for admitted calls to finish; a call that starts once the close has begun returns `network_not_open` (review M1). Every session call is admitted, send, coins and labels included (final review M2), so a close waits for an in-flight `prepare` or `broadcast` (up to ~60 s). |
| Pure functions | `units`, `uri`, `verify_message`, `generate_mnemonic`, `check_mnemonic` are free functions; they need no session and run on the caller's thread. |
| Events | Signals, not data (DESIGN-opus §1.5 rule 4). Hosts re-query on arrival. Rust debounces each domain to ≤ 4 Hz and **always delivers the last change of a burst** (review H2). |
| Errors | One `uniffi::Error` enum per domain. Each variant has a stable code (§4). `detail` strings are diagnostics for logs; Rust never produces user-facing copy. |
| Grants | Calls that spend, reveal, sign or wipe take a `grant_id` from `Vault.authorize`. Grants are single-use and bound to the wallet they were issued for (§2.2). The engine checks purpose, wallet, expiry and, for `Spend`, that what leaves the wallet (`total_debit` = `external_sent` + fee, §2.7.1) is at most `max_duffs`. |

Object model:

```
Engine ──open_network──▶ NetworkSession ──vault()──────▶ Vault
                                         ──new_tx_draft──▶ TxDraft ──prepare──▶ PreparedTx
```

## 2. Calls

Status: **works** = implemented and tested through the FFI; **M0** = earlier working call kept as is;
**stub** = returns `NotImplemented`.

### 2.1 Engine and session (`engine.rs`, `session.rs`) — owner E1

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `Engine(config, observer)` | sync ctor | Builds the tokio runtime and logging. | `EngineError` | QT-002 | M0 |
| `Engine.open_network(network, options)` | async | Opens (or returns the open) session: data dir, SDK, `PlatformWalletManager<SqlitePersister>`, loads wallets. | `EngineError` | QT-002, IOS-106 | M0 |
| `Engine.close_network(network)` / `shutdown()` | async | Stops SPV, drains persistence, releases the DB. | `EngineError` | QT-008 | M0 |
| `Engine.network_dir(network)` | sync | Data directory ("Open data folder"). | — | QT-143 | M0 |
| `NetworkSession.start_spv()` / `stop_spv()` / `spv_running()` | async/async/sync | dash-spv with masternode sync on. **E0-05 (DASHPAY §2.5, §3.2):** `start_spv` returns once the start is scheduled (measured 0.5–4 ms); an engine task first runs each wallet's DashPay bring-up (`start_wallet_subsystems`, every wallet at once: 3 s for a wallet created here, 20 s otherwise, counted from the start and bounding all of it, key acquisition included; skipped for a watch-only wallet and for one Platform proved within the last 7 days has no identity), then starts SPV whatever the outcome, then the `identity_sync`, `dashpay_sync` and `dpns_sync` loops. `SpvStateChanged{running: true}` reports the actual start; a dash-spv failure at that point is `Notice{SpvError}`. Idempotent while starting or running; configuration errors are still returned by the call. `stop_spv` cancels a running bring-up, waits for its blocking key work (at most 5 s) and quiesces the loops first; SPV is stopped either way, and work that did not end is returned as `sdk` ("SPV stopped, but this Platform work did not end: dashpay_sync, …"); the next start drains it again inside its task before its loops start (retried every 30 s), so `start_spv` itself never waits for it. Close cancels the bring-up, waits for its key work (at most 5 s; longer is an unclean close: `Notice{UncleanShutdown}`, marker kept) and lets the manager's shutdown drain the loops once (§2.1a item 6). A restore signals its wallets' bring-up only once it has committed, and until then no entry point (a start already listing wallets, an unlock, a wallet-added signal) admits one for them; a wallet refused meanwhile is brought up once the last restore marking it ends, unless that restore rolled it back. Removing or closing a wallet ends its bring-up. `spv_running()` is true while starting; calls that need SPV running (`rescan`, `rotate_peers`, a broadcast) still fail with `sync.spv_not_running` / `send.no_peers` until `spv_state()` is `Running`. `SessionOptions.no_platform` (dwcli `--no-platform`; not in dw-ffi until E0-13) is for a chain without Platform such as a plain dashd regtest: no bring-up, no loops. | `EngineError` | QT-024/025 | M0, E0-05 |
| `NetworkSession.spv_state()` | sync | `Stopped` \| `Starting` (bring-up or dash-spv start) \| `Running`. Not in dw-ffi until E0-13 (frozen Swift bindings). | — | DASHPAY §2.5 | E0-05 |
| `NetworkSession.dashpay_startup(wallet_id)` | sync | The wallet's last bring-up: `DashPayStartup{startup: StartupStatus, read_only, identity, contact_accounts_pending, identity_scan_incomplete, finished_at}`. `StartupStatus` is the M4 facade's (`NotRun`, `Starting`, the library's seven, `IdentityUnsettled`). `read_only`: watch-only, DashPay is read-only. A bring-up with a locked vault has no keys and reports `IdentityUnsettled` (unless the library says `Ready`); the first unlock runs it again. A lock while a bring-up holds keys, or after it built them, makes anything short of `Ready` or `NoIdentity` `IdentityUnsettled`. `read_only` is read from the vault at each call. | `wallet_not_found` | DASHPAY §3.2 | E0-05 |
| `NetworkSession.platform_loops()` / `set_platform_cadence(cadence)` / `dashpay_sync_soon()` | sync | Loop status (`SyncLoopStatus{sync_loop, running, last_run_at, next_run_at}`); the cadence `PlatformCadence{window_visible, contest_ending_soon}`: `dashpay_sync` 15 s visible, 60 s hidden (becoming visible also runs a pass), the contest watch 10 min, 1 min when a contest ends within the hour; `dashpay_sync_soon` runs a pass now in the background (Contacts or the bell opened, after a DashPay write; the engine does it after an unlock). Nothing runs while SPV is stopped. | — | DASHPAY §3.2 | E0-05 |
| `NetworkSession.platform_context_ready()` / `is_open()` / `network()` | sync | State reads. | — | — | M0 |

**E0-04** (design `docs/design/E0-04-grants-leases.md`, approved, DEC-73; lands in its phase P2a). The engine's
`NetworkSession::lock_vault()` becomes the lock every path runs: it revokes every flow lease, runs its own vault
gate, drains the hand-offs already admitted (each to its deadline, H = 10 s) and returns a `LockReport`, within
`max(H, T_gate)` of its call. It is engine-side (dwcli, tests); the chosen stack binds it in E0-13. dw-ffi keeps the
synchronous `Vault.lock()` (§2.2). The new events `LockProgress{Draining | Done(LockReport)}`, `LeaseChanged` and
`DispatchResolved`, and the notices `DispatchRecordMissing`, `UnscopedDispatch` and `DispatchJournalUnavailable`,
stay engine-side until E0-13, so §3 does not list them.

### 2.1a DashPay bring-up: DASHPAY §3.2 interpretations (E0-05)

DASHPAY §3.2 left these open; E0-05 implements them as below (rulings by pasta, 2026-10-09). `platform/bringup.rs` and
`platform/runtime.rs` hold the code.

1. **When the bring-up runs.** §3.2 says "if the wallet has identities, a restore is in progress, or discovery is
   unsettled"; "restore in progress" is defined nowhere. It runs for every wallet with keys unless the wallet has no
   identity on file and Platform proved, on this installation within the last 7 days, that its seed owns none (the
   wallet-local marker `dashpay.no_identity`, which a `.dwbackup` does not carry). After 7 days, and after DP6-01's
   "find" (`forget_proven_absence`), discovery runs again.
2. **No identity signer.** §3.2 lists `identity_signer?`. The bring-up and the unlock drain pass none: the DIP-15
   auto-accept pass submits state transitions, which unattended work never does (§2.6, E0-04).
3. **`IdentityUnsettled`.** Any non-`Ready` outcome of a bring-up without keys (a locked vault), or one the vault locked
   during. The first unlock runs it again. It sends no notice: the unlock settles it.
4. **The `Platform{Startup}` signal.** `EngineEvent::Platform` is E0-06's. Until then the outcome is
   `Notice{DashPayStartupIncomplete}` for the unsettled statuses plus `dashpay_startup`.
5. **"`is_spv_running` reports Starting".** `spv_running()` stays `bool` (the Swift bindings are frozen) and is true
   while starting; `spv_state()` carries `Starting`.
6. **Close order (deviation).** §3.2 orders close as: cancel a running bring-up, quiesce the loops, stop SPV,
   `manager.shutdown`. Close cancels the bring-up (and waits for its key work), then lets `manager.shutdown` drain the
   loops once, sealed; the library's shutdown stops SPV *before* it drains the loops. Draining in close as well would
   wait twice (10 s each) for a pass stuck on the network and make the close unclean. `stop_spv` keeps §3.2's order:
   it quiesces the loops before stopping SPV.
7. **Wallets added while SPV runs.** §3.2 does not cover them. A wallet registered, given keys or opened while SPV runs
   gets its bring-up without holding SPV; the DIP-15 rescan reconcile heals contacts found late. A restore signals
   only once it has committed; until then its wallets are marked, and every admission (the start's listing, each
   bring-up, the unlock work) refuses a marked wallet under the same lock it reads the seed with. A refused wallet is
   readmitted when the last restore marking it ends (overlapping restores), unless that restore's rollback removed it.
   Signals are stamped with when their event happened; the supervisor skips a bring-up signal older than the wallet's
   last admitted pass, so one event brings a wallet up once (a restore's commit and its readmission are one event),
   and signals arriving while a pass runs make one follow-up pass.
8. **Contest cadence.** The engine has no contest list yet; the host or DP1-04 sets `PlatformCadence.contest_ending_soon`.
9. **Passphrase change.** A passphrase change is not treated as a lock by the bring-up; E0-04 §8.6 owns it.

### 2.2 Vault (`vault.rs`) — owner B

`NetworkSession.vault() -> Vault` returns a handle to the network's vault (sync, cheap).

Status: every row marked **works** is tested in `dw-vault/tests/` (`vault.rs`, `race.rs`) and through the
engine and FFI tests (review H2).

| Call | Kind | Semantics | Errors (besides common) | Serves | Status |
|---|---|---|---|---|---|
| `Vault.status()` | sync | `VaultStatus { state, encrypted, quick_unlock_enrolled, failed_attempts, retry_after_secs, wallets_with_secrets }`. `state` ∈ `NoVault, NoKeys, Unencrypted, Locked, UnlockedMixingOnly, Unlocked`. | — | QT-022, IOS-013 | **works** |
| `Vault.create(passphrase: Option<bytes>)` | async | `Some` = encrypted (slot P, Argon2id), `None` = unencrypted (slot O, OS store). Leaves the vault unlocked. | `vault.already_exists`, `vault.passphrase_rejected`, `vault.os_store_unavailable` | QT-102, QT-111, IOS-010 | **works** |
| `Vault.encrypt(new_passphrase, grant_id)` | async | dash-qt "Encrypt Wallet": add slot P, delete slot O (the OS store copy of the data key). Leaves the vault locked. `ChangeCredential` grant (no wallet). | `vault.already_encrypted`, `vault.grant_invalid`, `vault.grant_purpose_mismatch` | QT-111 | **works** |
| `Vault.unlock(passphrase, scope: Full\|MixingOnly)` | async | Unwraps the DEK. Failed attempts count toward the IOS-012 throttle (`6^(n−3)·60 s`), which is persisted in the vault file and survives a restart. Attempts (unlock, change passphrase, passphrase grants) run one at a time, so parallel attempts cannot all pass the throttle check. | `vault.wrong_passphrase{failed_attempts, retry_after_secs}`, `vault.throttled`, `vault.not_encrypted` | QT-111, QT-112, IOS-012/013 | **works** |
| `Vault.lock()` | sync | Drops the DEK, revokes all grants and invalidates redeemed grants and signers (including those holding a grant's own key). Idempotent. **E0-04** (P2a) keeps it synchronous. It first revokes every flow lease and sets the lock barrier, even on a vault that is already `Locked` (so an own-key lease loses its key), then runs its own vault gate inline and returns the `VaultStatus`. The drain of hand-offs already admitted ends engine-side as `LockProgress::Done(LockReport)` (§2.1; E0-04 design §8.1). | — | QT-111, IOS-015 | **works** (the E0-04 part: P2a) |
| `Vault.change_passphrase(old, new)` | async | Re-wraps the DEK; records and seed unchanged; lock state unchanged. Ends the vault's epoch (review DW-E0-03 r3 m3): every grant (pending or redeemed), grant token and signer issued before it is refused afterwards, so a leaked old passphrase authorizes nothing more. | `vault.wrong_passphrase`, `vault.throttled`, `vault.passphrase_rejected`, `vault.not_encrypted` | QT-111 | **works** |
| `Vault.authorize(purpose, wallet_id: Option<String>, credential)` | async | Issues `AuthGrant { id, purpose, expires_at, single_use }`, single use, TTL 120 s. **Wallet binding** (review M-6): `wallet_id` is required for every purpose except `ChangeCredential`, which takes `None` (else `invalid_argument`); the grant is refused for any other wallet. **Credentials**: `Passphrase{bytes}`, `QuickUnlock{wrap_key}` (M2), `Unencrypted` (= no credential); see the requirement table below. A passphrase does **not** change the lock state (dash-qt re-lock parity, review M4): on a `Locked` or `UnlockedMixingOnly` vault the unwrapped key is held by this grant only and is dropped when the grant is used, expires, is revoked or the vault locks. Purposes: `Spend{max_duffs}`, `RevealSecret`, `SignMessage`, `ChangeCredential`, `Wipe`, `PlatformOp{max_duffs, max_credits}`, `IdentityScan` (`MasternodeOp` and `Governance` were removed in M3 with the parked masternode and governance domains, m3-engine.md). `PlatformOp`'s `max_duffs` caps Core funding (asset locks) and `max_credits` state-transition spending; a cap of 0 grants nothing of that kind. `IdentityScan` is wallet-scoped and uncapped, and is the only grant `Vault::scan_key` takes (E0-04 design §3.1, §3.2). In dw-vault since E0-04 P1. The frozen FFI enum still has a cap-less `PlatformOp`, which dw-ffi maps to `PlatformOp{0, 0}` (no funding, no credits) and which has no `IdentityScan`; E0-13 binds both. | `vault.wrong_passphrase`, `vault.throttled`, `vault.not_encrypted`, `vault.locked`, `vault.mixing_only`, `vault.credential_required`, `vault.quick_unlock_unavailable`, `vault.quick_unlock_limit_exceeded`, `vault.passphrase_stale`, `invalid_argument` (wallet binding) | IOS-016/017 | **works** |
| `Vault.authorize_set(purposes, wallet_id: Option<String>, credential)` | async | **E0-04** (design §3.3; `dw_vault::Vault::authorize_set`). One credential check (one Argon2id run, one throttle count) issues one grant per purpose, all with the same wallet binding and TTL; every purpose must be allowed for the credential, or nothing is issued. On a vault with no full-scope key each grant carries its own copy of the key, as single grants do. "Accept and pay" asks for `[PlatformOp{0, accept_cost}, Spend{amount + fee}]` and prompts once. With `QuickUnlock`, the combined value of the whole set must be within the spend limit (below). An empty set is `invalid_argument`. `authorize` is the one-purpose set. | as `authorize` | DASHPAY §2.3 | **works** in dw-vault (E0-04 P1, `dw-vault/tests/grants.rs`); the host binding comes with E0-13 (the Swift shells are frozen) |
| `Vault.revoke_grant(grant_id)` | sync | Unknown ids ignored. | — | IOS-017 | **works** |
| `Vault.reveal_mnemonic(wallet_id, grant_id)` | async | `RevealedMnemonic { phrase, bip39_passphrase }` as bytes (DESIGN R1: passphrase shown on reveal). `RevealSecret` grant for `wallet_id`. | `vault.no_secret`, `vault.grant_invalid`, `vault.grant_purpose_mismatch` (other purpose or other wallet), `vault.locked` | QT-113, IOS-006 | **works** |
| `Vault.enroll_quick_unlock(grant_id)` / `remove_quick_unlock()` | async | Biometric slot B (M2). | `vault.quick_unlock_unavailable` | IOS-011 | stub (M2): `NotImplemented` from dw-vault |

**Credential requirements** (Rust enforces these in `dw_vault::Vault::authorize`, review M5; Swift's
`AuthenticationGate.requirement(for:)` must ask for at least as much):

| Vault state | `RevealSecret`, `Wipe`, `ChangeCredential` | `Spend`, `SignMessage`, `PlatformOp`, `IdentityScan` (E0-04) |
|---|---|---|
| `NoVault` | `vault.no_vault` | `vault.no_vault` |
| `NoKeys`, `Unencrypted` | none (`Unencrypted`); a passphrase gives `vault.not_encrypted` | none (`Unencrypted`); a passphrase gives `vault.not_encrypted` |
| `Locked` | passphrase (else `vault.credential_required`) | passphrase (else `vault.locked`) |
| `UnlockedMixingOnly` | passphrase (else `vault.credential_required`) | passphrase (else `vault.mixing_only`) |
| `Unlocked` | passphrase (else `vault.credential_required`) | none or passphrase; the host decides with "require authentication for every payment" |

`QuickUnlock` stands in for the passphrase where slot B is enrolled (M2); without it, it returns
`vault.quick_unlock_unavailable`. It issues `Spend`, `SignMessage` and, since E0-04 (P1), `PlatformOp`, capped at the
spend limit: a grant's value is `max_duffs` for `Spend`, `max_duffs + ceil(max_credits / 1000)` for `PlatformOp`
(`dw_vault::CREDITS_PER_DUFF` = 1000 credits per duff) and 0 for `SignMessage`. `authorize_set` checks one sum over
the whole set, `Spend` grants included, and the passphrase must be fresh, as for `Spend`
(`vault.passphrase_stale`; DEC-67; E0-04 design §3.7). Anything above the limit is
`vault.quick_unlock_limit_exceeded`, and the host asks for the passphrase. `RevealSecret`, `Wipe`,
`ChangeCredential` and `IdentityScan` (which releases the master key) are never issued by quick unlock
(`vault.credential_required`). Every `PlatformOp` yields the `DashPayCrypto` scope (below), so a quick-unlock
`PlatformOp{0, 0}`, of value 0, gives a flow contact crypto (never a signature) even at a spend limit of 0, as
DEC-67 intends for Touch ID DashPay writes.

**Using a grant.** The call a grant authorizes redeems it (consumes it) and names the wallet it acts on:
`TxDraft.prepare` (`Spend`, the draft's wallet), `sign_message` (`SignMessage`), `remove_wallet` (`Wipe`),
`reveal_mnemonic` (`RevealSecret`), `encrypt` (`ChangeCredential`). A grant of another purpose or for
another wallet is refused and left in place; in the send, message and wallet domains this is their
`grant_invalid` code. Before it plans, `TxDraft.prepare` checks without consuming the grant
(`dw_vault::Vault::check_grant`) that a key is available, so a locked or mixing-only vault fails with
`send.vault_locked` unless the grant carries its own key; grant errors come from the redemption after the
plan. A passphrase
grant on a locked or mixing-only vault signs, reveals or wipes with its own key and leaves the vault as it
was. A redeemed token (`dw_vault::GrantToken`) is bound to the vault instance that redeemed it and to that
vault's epoch: it carries a random id made when the vault was opened and the epoch at redemption, and every use
compares both in constant time (review DW-E0-03 r2 M1). Another vault, even one holding the same wallet id at
the same epoch number, refuses it (`vault.grant_invalid`), as does the same vault file opened again; a lock,
unlock, scope change or passphrase change of its own vault ends it. Within its epoch a token is not single-use:
the grant is, but its token works until the epoch ends, and removing or storing a wallet does not end it (a
`Wipe` token can wipe the same wallet again after a re-import, which derives the same wallet id only from the
same seed).

**Platform signer scopes** (roadmap E0-03, DASHPAY §3.3; engine-internal, not on the FFI). A redeemed
`PlatformOp` grant no longer yields a full-scope signer. `dw_vault::Vault::platform_signer` issues scoped
signers from it instead (one token may issue several; a token with its own key only through its hold,
`platform_signer_held`, below), and each refuses every path and use outside its scope
before reading the seed. The token's caps bound the scopes (E0-04 design §3.4; `vault.grant_purpose_mismatch`
otherwise): `PlatformFunding{max_duffs}` needs a token `max_duffs` that is not 0 and at least the scope's (the
engine asks for the funding budget left), `PlatformIdentity` a token `max_credits` that is not 0, and
`DashPayCrypto` comes with any `PlatformOp`. The identity-scan key (`Vault::scan_key`, below), which is the master
key, has its own grant, `IdentityScan`: `scan_key` refuses a `PlatformOp` token (`vault.grant_purpose_mismatch`),
so a capped flow token cannot release it (E0-04 design §3.2).

| Scope | Uses | Paths |
|---|---|---|
| `PlatformIdentity` | sign, public key (no chain code) | DIP-13 ECDSA identity keys `m/9'/coin'/5'/0'/0'/i'/k'` |
| `DashPayCrypto` | never signs. Public keys; ECDH and the account-reference mask; contactInfo AES keys; export of the auto-accept key | xpubs of `m/9'/coin'/15'/a'/<user>/<friend>` (DIP-14 256-bit children), `m/9'/coin'/16'/expiry'` and `m/44'/coin'/0'`; ECDH and mask with identity keys; contactInfo `…/k'/65536'\|65537'/n'` under an identity key; export of `m/9'/coin'/16'/expiry'` only |
| `PlatformFunding{max_duffs}` | sign, public key (no chain code); one extended public key | BIP44 and BIP32 addresses, DIP-15 receiving addresses, asset-lock credit keys `m/9'/coin'/5'/{1',2',3'}/…` with a non-hardened last step; the xpub of an identity's top-up account `m/9'/coin'/5'/2'/i'`. The vault signs sighashes and cannot check the debit, so the cap is enforced in two places (E0-04 design §3.4, §3.6): capped by the token (`platform_signer` refuses a `max_duffs` above the token's, E0-04 P1); the debit is checked at `register` (Mode A) or against the worst-case fee bound before the library call (Mode B), engine-side (E0-04 P2). No engine flow builds one yet (registration is DP1-02). |

`Vault::dashpay_crypto_signer` (the background crypto signer) needs no grant. It is issued only while the full
key needs no prompt (`Unencrypted`, or `Unlocked` with scope Full), and refused while `Locked` (`vault.locked`)
or `UnlockedMixingOnly` (`vault.mixing_only`). It signs nothing; the one key it exports is a DIP-15 auto-accept
key. `Vault::scan_key` (the identity-scan master key) needs a redeemed `IdentityScan` grant for its wallet, which
the unattended bring-up authorizes with `Credential::None`, so it too works without a prompt only in those two
states; a passphrase `IdentityScan` grant on a locked vault releases it only through its hold
(`Vault::scan_key_held`). Both stop working when the vault locks, changes unlock scope or changes its passphrase. Both are engine-only:
`crates/dw-ffi/clippy.toml` forbids `dashpay_crypto_signer`, `scan_key`, `scan_key_held`, `ScanKey::master_key`,
`VaultScanKey::resolve`/`resolver` and `open_backup_bundle` (review DW-E0-03 r3) in dw-ffi
(`clippy -D warnings` fails), and a dw-ffi test scans its sources for them. The engine adapters
(`dw_engine::platform::signers`) implement dpp's `Signer<IdentityPublicKey>`, platform-wallet's
`ContactCryptoProvider` and `ScanKeyResolver` over these.

Derived scalars stay in dw-vault, with two exceptions:

- the DIP-15 auto-accept key (`m/9'/coin'/16'/expiry'`), which DIP-15 hands out on purpose as a bearer
  credential for contact auto-acceptance; it leaves as a secp256k1 `SecretKey`, which does not erase itself;
- the wallet's **master** extended private key, for platform-wallet's identity scan. `ScanKeyResolver`
  (`manager/startup.rs:90-110`) requires it: discovery derives every probed identity key from it, so nothing
  narrower serves. Its only consumer is the engine's call of `start_wallet_subsystems`, which invokes the
  resolver at most once and only on the branch that scans, and holds the key in its `ScanKeyGuard` (erased on
  drop) for that call. A key already resolved is outside the vault: `lock()` cannot revoke it, and it is erased
  only when dropped. E0-05 must therefore cancel (drop) a running bring-up on lock, so the guard drops with it.
  Follow-up: ask upstream for a resolver that derives and returns the probed public keys instead.

**Key holds and the epoch** (E0-04 design §3.5 and §15's P1 follow-ups; engine-internal). A grant authorized on a
vault with no full-scope key (`Locked`, `UnlockedMixingOnly`) carries its own copy of the data key.
`Vault::hold_key(tokens)` moves that copy out of every token of a grant set into one `KeyHold`, or fails changing
none: every token, vault-key ones included, must be this vault's of the current epoch (`vault.grant_invalid`,
`vault.locked`); every token must carry its own key, not be held already and never have issued a signer, even one
since dropped (`invalid_argument`); `Ok(None)` for a valid set of vault-key tokens, and a mixed set is
`invalid_argument`. A held token issues signers only through its hold
(`Vault::platform_signer_held`, `signer_held`, `scan_key_held`; `invalid_argument` anywhere else). Dropping the hold
erases the key in place: every use copies it under the hold's mutex inside the vault gate, so an operation under way
finishes with its copy and every later call of a held signer is `vault.locked` (a held token issues nothing once
its hold is gone: only that hold issues for it). **Which tokens may issue directly:** a
vault-key token (an unlocked vault), and an own-key token only for `Spend` and `SignMessage`. That is the M1
exception: send, PSBT, message and CoinJoin drop their token as soon as they hold the signer, which keeps its own
copy, until E0-04 P5 moves them onto holds. An own-key `PlatformOp` or `IdentityScan` token issues nothing directly
(`invalid_argument`). `Vault::epoch()` (not secret) changes on every lock, every unlock that changes the lock state
or scope, and every passphrase change, encrypt, recover and destroy, each of which ends every grant, token and
signer of the old epoch, held ones included; the engine compares it around vault calls (E0-04 design §8.6).

**Concurrency** (review H1). All changes of the vault file (create, encrypt, change passphrase, record
writes, throttle updates) and all passphrase checks are serialized by one vault-level writer lock. Each
write starts from the in-memory file and changes only its own part (a slot, the throttle, or records); the
file on disk is read only when the vault opens and to verify a record write. A disk snapshot is never
installed over the in-memory state, so a passphrase change running beside a wallet import cannot drop the
imported seed.

**Lock against running operations** (review DW-E0-03 B1, r2 M2, r3 m1/m2). Every signer call holds a vault
operation gate from its epoch check until its result is released. So does every use of a grant token's key, from
the token check to the end of the operation: the secret reads (`reveal_mnemonic`, `export_wallet_secret`,
`with_revealed_seed`, `open_backup_bundle` of the vault's own bundle) and the token-authorized writes
(`wipe_wallet_secret`, `encrypt`, `enroll_quick_unlock`, `set_quick_unlock_spend_limit`, with their file
write). `seed_derivation` and `core_mnemonic_check` hold it while they read the secret. `lock()`, `unlock()` and
every other epoch change take the gate exclusively, so `lock()` returns only after those operations already
running have finished (a signature takes about a millisecond; a gated write, one file write), and every later
call is `Locked`. A gated operation uses only a key already in memory: an unencrypted vault's key is loaded from
the OS store before the gate, so a keyring that blocks (on its unlock prompt, say) never holds `lock()`. A result
leaves the vault only through one release check (`dw_vault::Vault::gated`): holding the mutex that every epoch
change holds, the epoch the operation started under must still be current, or the result is dropped and the
call is `Locked`. Because that mutex orders each release wholly before or after each epoch change, and `lock()`
changes the epoch before it returns, no signature, shared secret, ciphertext, exported key, secret read or
token-authorized write of the old epoch is made or released after `lock()` returns. This is structural, not a
timing claim; the lock-race tests check it on the vault's own log of releases and epoch changes, in that mutex's
order, under 16 concurrent signers and 300 locks per vault mode. What a caller does with a result released
before the lock can finish after it: a signature it already holds, the provider key `with_revealed_seed`'s
closure derives, the dump-wallet keys derived from `export_wallet_secret`. A flow that must not use such a
result after a lock ("Lock to cancel") hands it to a transport only through E0-04's dispatch fence. The lock revokes
its lease in one step under the lease table's mutex, after which the fence admits no new hand-off under it, and the
lock waits for the hand-offs already admitted, each to its deadline (DASHPAY §2.6; E0-04 design §5, §8).
Not gated: `backup_bundle` (no grant or epoch; it
returns ciphertext only), `open_backup_bundle` through a bundle's passphrase slot (no vault key), and record
writes under the vault's own key (`store_wallet_secret` and the import rollback's `delete_wallet_secret`). An
unencrypted vault's read that loads the key and then finds it dropped by a lock before the gate loads it again
(at most three tries) rather than failing `vault.locked`; `authorize` with no credential issues a grant only
while that key is in memory.

**Backup bundles** (review DW-E0-03 r3). `open_backup_bundle` opens a bundle this vault wrote with the vault's
key only under a `RevealSecret` grant for the bundle's wallet, which it redeems, as `reveal_mnemonic` does;
without one, every bundle needs its passphrase slot. So a restore never hands out the phrase for less than a
reveal needs: nothing on an unencrypted vault, the vault passphrase on an encrypted one, unlocked or not. The
engine's `restore_backup` gets that grant itself (`backup.rs` `open_bundle`): with no credential on an
unencrypted vault. On an encrypted one, a bundle whose slot takes the current vault passphrase, or a backup
passphrase, opens only through that slot, which the vault's throttle does not count, so a mistyped backup
passphrase never throttles unlock. Only a bundle that just the vault's key opens with the current passphrase
(`Vault::own_key_only`: made before a passphrase change, or an automatic backup from before `encrypt`) has the
passphrase checked as the vault's, after its slot was tried; a wrong one then counts. A restore on a locked vault
is refused before any bundle is opened. Before r3, an unlocked vault opened its own bundles with no grant and no
passphrase.

**Integrity.** Records and the manifest are authenticated with the data key; a changed, deleted, swapped
or individually rolled-back record, or a changed manifest, makes unlock and reads fail with
`vault.corrupt`. A changed wrapped key or KDF parameter fails like a wrong passphrase
(`vault.wrong_passphrase`; the AEAD cannot tell them apart). The throttle counter is outside the AEAD
(UX throttling only; Argon2id cost is the protection). A whole-file rollback to an older, self-consistent
vault file is not detected across restarts (it needs a trusted monotonic anchor outside the file); while
the app runs such a file is ignored and overwritten by the next write.

### 2.3 Wallets (`wallet.rs`) — owners B (generate/check/import) and E1 (registry)

| Call | Kind | Semantics | Errors (besides common) | Serves | Owner | Status |
|---|---|---|---|---|---|---|
| `generate_mnemonic(word_count, language)` | sync, free | Fresh phrase (12/15/18/21/24 words) as UTF-8 bytes. **Stores nothing.** The host shows it, runs the verify step, then calls `import_wallet`. | `wallet.unsupported_word_count` | QT-102/103, IOS-002…004 | B | **works** |
| `check_mnemonic(phrase)` | sync, free | `MnemonicCheck { word_count, unknown_word_indices, language, checksum: Valid\|CoreOnly\|Invalid }` for live restore validation. `CoreOnly` = fails BIP39, passes Dash Core's weak check. | — | QT-104, IOS-007 | B | **works** |
| `NetworkSession.import_wallet(mnemonic, bip39_passphrase, options)` | async | Stores phrase and passphrase in the vault, then registers the wallet, in DESIGN-opus §1.8 seed-safety order. `ImportOptions { name, birth_height, core_compat, lookahead }`. Returns the wallet id. Over an existing watch-only wallet with the same id: attach the keys, or `wallet.watch_only_exists`. | `wallet.invalid_mnemonic`, `wallet.already_exists`, `wallet.watch_only_exists`, `wallet.no_vault`, `wallet.vault_locked`, `wallet.name_rejected`, `invalid_argument` (lookahead) | QT-104/105, IOS-007, IOS-123 | B, E1 | **works**: the seed is stored and read back before registration, so without a usable vault the call fails with `wallet.no_vault` / `wallet.vault_locked` and registers nothing (review H-1). Re-import over a registered wallet without a seed attaches the keys (emits `WalletCreated`). `name` is stored in dw-appdb (`None` = "Wallet N"). `lookahead` (1..=1000) raises the gap limit of BIP44 account 0's chains; `core_compat` defaults it to 1000. The raised gap is stored with the wallet (dw-appdb) and applied again when the session opens (M2, R2), as dash-qt keeps its 1000-key pool. No watch-only wallets exist yet, so `wallet.watch_only_exists` is never returned. |
| `NetworkSession.wallet_infos()` / `wallet_info(id)` | sync | `WalletInfo { wallet_id, name, watch_only, has_mnemonic, hd, birth_height, created_at, balances }`, creation order (wallets without a stored creation time last). `watch_only` = the vault holds no seed for it. `balances` is `None` until the scan has processed the birth block (review M-3). | `wallet_not_found` | QT-014, QT-021, QT-035, QT-101, IOS-110 | E1 | **works** |
| `NetworkSession.balances(id)` | sync | `Option<WalletBalances { confirmed, unconfirmed, immature, locked, total, coinjoin }>`; `None` = not known yet. `coinjoin` is the spendable balance of the DIP9 CoinJoin accounts (dash-qt's "fully mixed" rule is WS-06). | `EngineError` | QT-034, IOS-019/021 | E1 | **works** |
| `NetworkSession.remove_wallet(id, grant_id)` | async | Deletes the vault records, then unloads the wallet and deletes its wallet rows, app metadata and dispatch-journal rows. `Wipe` grant, redeemed after the wallet is found. Deleting the records needs a full-scope key: the vault's, or the grant's own key when it was issued with the passphrase on a locked or mixing-only vault (§2.2). For a wallet with a seed the grant is checked first without consuming it, so a locked or mixing-only vault with a grant that carries no key is refused (`wallet.vault_locked`) before anything is redeemed or deleted. The records are deleted with the redeemed token's key. The removal fails closed (E0-04 DEC-134): if the vault records cannot be deleted (vault locked meanwhile, write failure) the call returns that error, the wallet stays listed and usable, and nothing else of it is removed. The wallet's leases are revoked and the grant is consumed either way. A later failure, once the seed is gone, is returned and leaves the wallet without a seed; removing it again (after reopening the network if it is no longer listed) finishes the removal. Emits `WalletRemoved` as soon as the rows are gone. | `wallet.vault_locked`, `wallet.grant_invalid`, `wallet_not_found`, `storage` | QT-101, IOS-109 | E1 (+B for vault records) | **works** |
| `NetworkSession.rename_wallet(id, name)` | async | 1–64 chars after trimming, no control characters; stored in `dw-appdb`. | `wallet.name_rejected` | QT-014, IOS-110 | E1 | **works** |

`list_wallets` (M0) is removed; use `wallet_infos`.

### 2.4 Sync (`sync.rs`) — owner E1

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.sync_snapshot()` | sync | `SyncSnapshot { running, phases: [SyncPhaseProgress{phase, current_height, target_height, done}], active_phase, tip_height, tip_time, chainlock_height, connected_peers, caught_up, seconds_since_progress }`. Phases: `Headers, FilterHeaders, Filters, Masternodes`. `caught_up` = dash-spv steady state (every reported phase `Synced`, or idle in `WaitForEvents` at its target — the iOS `syncDone` gate). Raw values; damping is Swift's. The 45 s stall rule is the engine's: it sends `Notice{SyncStalled}` once per stall. | — | QT-024/025/027, IOS-023 | **works**. On a single regtest node without quorums the masternode phase never finishes (dashd fails `getqrinfo`), so `caught_up` stays false there. |
| `NetworkSession.peers()` | sync | `[PeerInfo { address, user_agent, protocol_version, best_height, ping_ms, connected_since, inbound, bytes_sent, bytes_received }]`. | — | QT-147, IOS-023 | **works**: dash-spv reports only addresses, so every other field is `None` (`connected_since` = when this engine saw the connection; `inbound` is always false). |
| `NetworkSession.rotate_peers()` | async | Drop current peers, connect to new ones ("Change peers"). | `sync.spv_not_running` | IOS-023 | **works** as an SPV client restart (dash-spv has no per-peer disconnect); with `spv_peers` configured the same peers are dialled again. |
| `NetworkSession.rescan(from: WalletBirth\|Genesis\|Height{h})` | async | Schedules a filter rescan for every wallet; progress via `Sync` events. | `sync.height_out_of_range` (above the header tip), `sync.spv_not_running` | QT-117, QT-148, IOS-113 | **works**: rewinds each wallet's filter checkpoint in memory; a restart before the rescan ends needs another call. |

### 2.5 History (`history.rs`) — owner E1

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.history_page(id, query)` | async | `HistoryQuery { filter, sort, cursor, limit (1..=500) }` → `HistoryPage { records, next_cursor, total_matching }`. Filter: `types`, `categories`, `statuses`, `date_from` (inclusive), `date_to` (**exclusive**, dash-qt), `text` (case-insensitive address/label/txid, ≤ 256 chars), `min_amount` (absolute), `watch_only`. Empty list / `None` = any. Records are dash-qt `TransactionRecord`s: one per non-change output for sends, fee on the first; `record_index` orders them. Cursors are keyset cursors (they name the last record returned): new transactions between pages never invalidate them, so a CSV export can page through a running sync; `stale_cursor` only for a cursor of another filter/sort or a malformed one. | `history.invalid_query`, `history.stale_cursor`, `wallet_not_found` | QT-086…089, QT-094 (rows for CSV), IOS-027/028 | **works** |
| `NetworkSession.tx_detail(id, txid)` | async | `TxDetail { records, status, timestamp, block_height, block_hash, fee, size_bytes, inputs, outputs, message, label, raw_hex }`. Input `amount`/`address` are `None` when the spent output is not in the wallet's history. | `history.tx_not_found`, `invalid_argument` (txid not 64 lower-case hex) | QT-092, IOS-031 | **works** |

The history read model is kept in memory per session: persisted records from `wallet.sqlite` at
open (platform-wallet evicts chainlocked records from its own memory), then wallet events.
`timestamp` is the earlier of the first-seen time (dw-appdb `tx_meta.created_at`) and the block
time. Not produced yet, for lack of data: `Conflicted` and `Abandoned` (platform-wallet deletes
provably beaten spends, so they leave the history), `CoinJoinSend` (needs the send flow's
`DS=1` mark), `DustReceive` (dust protection, E2) and `RecvWithCoinJoin` (never assigned by
dash-qt either). Labels: the transaction label, else the address label (dw-appdb).

Classification: `TxType` is dash-qt's 19-value enum in dash-qt's order (research 02 §4.1); `TxCategory`
is the iOS filter category (`Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform,
Other`). `TxStatus { kind, confirmations, instant_locked, chain_locked, matures_in }` follows dash-qt's
rules: depth < 0 Conflicted; 0 Unconfirmed/Abandoned; < 6 and not ChainLocked Confirming; ChainLock
confirms at once; coinbase Immature/NotAccepted. `counts_toward_balance = false` renders brackets.

### 2.6 Receive (`receive.rs`) — owner E1 (requests stored in E2's `dw-appdb`)

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `current_receive_address(id)` | async | First unused external address of BIP44 account 0 as `AddressInfo { address, chain, index, derivation_path, used, label, balance, tx_count }`. Re-queried after `HistoryChanged`, which rotates it once paid. | `receive.gap_limit` | IOS-053/054 | **works** |
| `next_receive_address(id, label)` | async | Issues and labels a fresh address: unused, not reserved by key-wallet, never issued before (issued addresses are dw-appdb `receive` address-book entries and request addresses, so issuance survives restarts). `gap_limit` once every address inside the gap is issued. | `receive.gap_limit` | QT-081 | **works** |
| `addresses(id, AddressFilter{chain, used})` | async | BIP44 account 0 addresses, receiving then change, by index. `balance` from the account's UTXOs; `tx_count` from the history. | — | QT-096 | **works** |
| `create_receive_request(id, amount, label, message)` | async | Stores a `ReceiveRequest { id, created_at, address, amount, label, message, uri }` on a freshly issued address; `uri` from dw-uri `format_bitcoin_uri`. Amount 0 = any amount; above 21 M DASH is `invalid_argument`. | `receive.gap_limit`, `invalid_argument` | QT-081/082/085, IOS-055 | **works** |
| `receive_requests(id)` / `delete_receive_request(id, request_id)` | async | Newest first. | `receive.request_not_found` | QT-083 | **works** |

### 2.7 Send (`send.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.new_tx_draft(id)` | sync | Empty draft: source `Any`, fee `Recommended{6}`, change `Auto`. Checks the wallet is registered (in-memory). | `wallet_not_found` | QT-052 | **works** |
| `NetworkSession.max_spendable(id, source, fee)` | async | The spendable sum of `source` for dash-qt "Use available balance" (review M-4): the host puts `max_spendable − other recipients' amounts` into the entry **and sets `subtract_fee_from_amount`**, so the fee comes out of it at any rate and with any number of recipients. Excludes user-locked, reserved, immature and untrusted unconfirmed coins. `fee` is validated only. | `invalid_argument`, `send.outpoint_unavailable` | QT-053, IOS-044 | **works** |
| `TxDraft.set_recipients([Recipient{address, amount, subtract_fee_from_amount, label, message}])` | sync | Offline validation; errors carry the recipient `index`. Address on the session network; Platform (DIP-18) addresses rejected; amount 1..=21M DASH and at least the output's dust threshold (546 duffs P2PKH, 540 P2SH); no address twice. | `send.no_recipients`, `send.invalid_address`, `send.platform_address`, `send.invalid_amount`, `send.dust_amount`, `send.duplicate_address` | QT-052…056, QT-060, QT-067 | **works** |
| `TxDraft.set_source(Any\|FullyMixedOnly\|Outpoints)` | sync | `Any`: every coin of the BIP44, BIP32 and DashPay-receiving accounts that is mature, trusted (confirmed, InstantSend-locked or own change), not user-locked and not reserved; CoinJoin coins are never pooled with them. `Outpoints`: exactly these coins, **all of them** (Dash Core coin control); they may be unconfirmed but not locked, reserved, immature or CoinJoin-account coins (checked by `estimate`/`prepare`). `FullyMixedOnly`: `not_implemented` until CoinJoin rounds are tracked (WS-06). | `invalid_argument` (empty or repeated outpoints), `not_implemented` | QT-051, QT-068…071 | **works** (`FullyMixedOnly`: stub) |
| `TxDraft.set_fee(Recommended{target_blocks}\|PerKb{duffs_per_kb})` | sync | `Recommended`: target 1..=1008; on SPV every target pays the minimum relay fee, 1000 duff/kB (DESIGN-opus §1.14). `PerKb`: 1000..=10,000,000 duff/kB. | `invalid_argument` | QT-057 | **works** |
| `TxDraft.set_change(Auto\|Address{address})` | sync | `Auto`: a fresh BIP44 internal address, derived only when the payment has change. `Address`: any L1 address of the network; a foreign one counts against the spend cap. | `send.invalid_change_address` | QT-073 | **works** |
| `TxDraft.estimate()` | async | Plans the payment (§2.7.1) against the wallet's current coins and dry-runs key-wallet's builder over the plan (unsigned, nothing reserved): `TxEstimate { fee, size_bytes, input_count, change, total_sent }` (`total_sent` after subtract-fee shares). Nothing signed or reserved. `send.outpoint_unavailable{outpoint}` also names a chosen coin the builder would leave out (worth no more than its own input fee at the draft's rate). | balance errors, `send.amount_too_small_after_fee`, `send.outpoint_unavailable`, `send.absurd_fee`, `send.tx_too_large` | QT-072, IOS-044 | **works** |
| `TxDraft.prepare(grant_id)` | async | Plans and dry-runs the build as `estimate` does, then redeems the `Spend` grant (single-use; refused if `total_debit = external_sent + fee > max_duffs`), builds with key-wallet's builder through platform-wallet's reservation-only finalize, signs through dw-vault's `VaultSigner`, checks the transaction against the plan and reserves the inputs. **Never broadcasts.** A balance error or a coin-control choice the builder would not spend does not consume the grant. `PreparedTx.summary()` = `PreparedTxSummary { txid, fee, fee_rate_per_kb, size_bytes, inputs, outputs[{address, amount, is_change, label, is_mine}], total_sent, total_debit, external_sent }`. | `send.amount_exceeds_balance{available}`, `send.amount_with_fee_exceeds_balance{fee, available}`, `send.amount_too_small_after_fee{index}`, `send.outpoint_unavailable`, `send.absurd_fee`, `send.watch_only`, `send.vault_locked` (locked or mixing-only), `send.grant_invalid`, `send.grant_exceeded{max_duffs}` | QT-058/059/061, QT-064/065/066, IOS-046 | **works** |
| `TxDraft.broadcast(prepared)` | async | Announces through platform-wallet's SPV broadcaster and waits for dash-spv's acceptance verdict (peer echo, InstantSend lock or block; up to ~60 s). Outcomes and reservations: §2.7.1 "Broadcast". Accepted → `BroadcastOutcome { txid, peers_announced: None }` (dash-spv does not report the count). Once accepted or unknown, stores the payment's message and adds the recipients to the address book (§2.7.1 "Address book"). | `send.prepared_tx_spent`, `send.no_peers`, `send.broadcast_rejected{reason}`, `send.broadcast_unknown{reason}`, `invalid_argument` (another draft's `PreparedTx`) | QT-062/063, IOS-052 | **works** |
| `TxDraft.abandon(prepared)` | async | Releases the reserved inputs (key-wallet's reservation, owner-guarded, and the engine's). Idempotent while pending or released. Releasing the last reference to a pending `PreparedTx` does the same; releasing one whose outcome is unknown releases nothing. | `send.prepared_tx_spent` (sent or outcome unknown), `invalid_argument` (another draft's) | IOS rule 4 | **works** |

#### 2.7.1 Payment rules (settles review H-3, H-4, M-4, M-5, M-7, M-8 and the send Lows)

- **Plan.** Automatic selection runs key-wallet's branch-and-bound selector over the candidate set; coin control
  spends every chosen coin. Sizes are key-wallet's estimate (148 bytes per input, outputs at their real script
  length, +10, a budgeted change output). Change is kept only above its dust threshold, otherwise it goes to the
  fee. The builder is then handed exactly the planned inputs plus the change as an explicit output, and the
  engine checks the signed transaction matches the plan (inputs, output values, fee) before reserving it.
- **Subtract fee (QT-053).** The recipients that ticked it pay the size-based fee, split equally, the first of
  them paying the remainder (Dash Core). Selection for such a payment targets the amounts alone. A dust remainder
  that cannot become change goes to the fee and is paid by the wallet. A recipient left below its dust threshold
  is `send.amount_too_small_after_fee{index}` (review M-5).
- **Spend cap (H-3, H-4 option A).** `external_sent` = value paid to scripts the wallet does not own (recipients
  and a foreign custom change address); `total_debit` = inputs − outputs back to the wallet = `external_sent +
  fee`. A `Spend{max_duffs}` grant caps `total_debit`, fee included (fix-review L4: `sign_psbt` caps the same
  outflow, so a quick-unlock spending limit means the same in Send and PSBT); the fee alone is also bounded by
  `send.absurd_fee` (fee > 0.1 DASH, dash-qt `-maxtxfee`). The host authorizes `max_duffs = Σ recipient amounts +
  estimate.fee` (plus `estimate.change` when the change address is foreign). Should the fee grow between
  `estimate` and `prepare` (the wallet's coins changed), `prepare` refuses with `send.grant_exceeded` and the
  host reviews again. Spend grants are single-use (all grants are) and
  bound to the draft's wallet (review M-6, §2.2).
- **Max (M-4).** See `max_spendable`.
- **Coin control and the builder (final review Low).** key-wallet's selector, given exactly the planned inputs,
  leaves out a chosen coin worth no more than its own input fee. `estimate` and `prepare` build the plan unsigned
  first (no reservation) and report such a coin as `send.outpoint_unavailable{outpoint}` before any grant is
  redeemed; coins reserved by another pending payment are refused the same way at planning time.
- **Broadcast (final review M1, M3).** The first `broadcast` of a pending `PreparedTx`:
  - accepted → `Ok`; the inputs are spent;
  - never sent (SPV stopped, no connected peer, rejected before dispatch) → `send.no_peers` /
    `send.broadcast_rejected`; the inputs are released and the `PreparedTx` is spent. This matches the iOS wallet
    and platform-wallet, which release key-wallet's reservation on such a rejection so the payment is rebuilt
    (dash-qt has no such outcome: it commits the transaction to its own wallet and relays it later). **Hosts
    re-run review after `send.no_peers`: a new `prepare` with a new `Spend` grant**; the old handle is spent;
  - no verdict within ~60 s → `send.broadcast_unknown{reason}`; the inputs stay reserved and the same handle may
    be broadcast again (same transaction, same txid). From then on the transaction may be on the network, so it is
    never released again: a repeated `broadcast` returns `Ok` when accepted and `send.broadcast_unknown` for any
    other outcome (the reason says what that attempt saw: SPV not running, no peers, no verdict); `abandon` is
    refused and dropping the handle releases nothing;
  - not dispatched (review L5): any other error of a first `broadcast` (`network_not_open`, `wallet_not_found`,
    `invalid_argument`, `wallet`, `storage`, `spv`, `io`) is raised before the transaction is handed to the network
    (platform-wallet's finalized broadcast returns only `Ok`, a stale reservation, a rejection or an unconfirmed
    outcome). The `PreparedTx` stays pending: it can be broadcast again as a first dispatch or abandoned, and
    dropping it releases its inputs. Hosts treat these as definite failures; only `send.broadcast_unknown` means
    the outcome of a first broadcast is unknown.

  An unknown-outcome payment's inputs stay `reserved` (excluded from selection and from `max_spendable`) until
  the wallet sees them spent, by the payment itself or by a conflicting transaction: every coin read drops the
  reservation of a coin the wallet no longer holds unspent. Over SPV the broadcast is fed into the wallet's own
  mempool view, so its inputs usually show as spent (the transaction appears unconfirmed in the history) right
  away. Reservations live in the session: closing the network (or restarting) drops them. Nothing that merely
  elapses releases them, as with platform-wallet's pending-spend fence: re-selecting those coins could sign a
  second spend of coins the first transaction may still take.
- **Address book after a send (final review M7).** Written only once a broadcast was accepted or its outcome is
  unknown, never for a payment that was not sent. Like dash-qt's `sendCoins`, every recipient not yet in the
  address book is added (Send, or Receive for one of the wallet's own addresses) with its label, or unlabelled;
  unlike dash-qt a label is never replaced: a listed entry without a label gets the recipient's label, a listed
  entry with a label (either purpose) is left as it is. The engine is the only writer of these entries; hosts do
  not add recipients themselves. The payment's `message`s are stored with the transaction at the same time.
- **Draft binding (Low).** A `TxDraft` keeps its session alive but works only while it is open
  (`network_not_open` after close) and while the wallet is registered (`wallet_not_found` after removal). A
  `PreparedTx` belongs to the draft that made it. `estimate` has no revision token: the host discards an estimate
  that returns after the draft changed (it knows the order of its own calls).
- **FullyMixedOnly (Low).** Dash Core's `ONLY_FULLY_MIXED` (QT-051) has no change output: the excess goes to the
  fee. That is the rule when the source lands; until then it is `not_implemented`.
- **Not dash-qt (QT-066).** key-wallet builds version-3 transactions with `nSequence = 0xffffffff` and locktime 0
  (dash-qt: `SEQUENCE_FINAL − 1` with anti-fee-sniping locktime); BIP69 ordering matches. `send.duplicate_address`
  is an error, not dash-qt's confirm question (QT-060): the host asks first and merges or removes the duplicate.

### 2.8 Coins (`coins.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `utxos(id, UtxoFilter{include_locked, fully_mixed_only, min_confirmations})` | async | Every unspent output of the wallet's funds accounts, largest first: `Utxo { outpoint, address, amount, confirmations, block_height, timestamp, instant_locked, chain_locked, user_locked, reserved, label, is_change, is_coinbase, coinjoin_denominated, coinjoin_rounds, spendable }`. `timestamp` is the block time (`None` unconfirmed or after key-wallet pruned the record); `reserved` = an input of a pending `PreparedTx`; `coinjoin_rounds` is always `None` (not tracked); `fully_mixed_only` is `not_implemented`. Applies dust protection first. | `not_implemented` | QT-068…071, QT-075 | **works** |
| `lock_outpoints` / `unlock_outpoints(id, outpoints)` | async | Persisted in dw-appdb (`utxo_locks`). Lock: the outpoint must be an unspent output of the wallet. Unlock deletes a user lock or releases a dust lock (dust protection then leaves the coin alone). | `coins.outpoint_not_found`, `invalid_argument` | QT-070 | **works** |
| `locked_outpoints(id)` | async | Active locks (user and dust), oldest first. | — | QT-070 | **works** |
| `dust_protection()` / `set_dust_protection(threshold: Option<u64>)` | async | dash-qt dust attack protection: `None` = off, else 1..=1,000,000 duffs. Coins ≤ threshold whose funding transaction spent none of the wallet's coins (not change, not coinbase) are dust-locked when coins are next read. | `invalid_argument` | QT-075 | **works** |

### 2.9 Labels and address book (`labels.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `address_book(id, purpose, search)` | async | Per-wallet entries, sorted by label (case-insensitive, unlabelled last) then address. `search` is dash-qt's case-insensitive wildcard (`*`, `?`) match anywhere in the label or address. | — | QT-095…097 | **works** |
| `save_address_book_entry(id, address, label, purpose, replace)` | async | Add, or relabel with `replace` (same purpose). Send entries are other people's addresses; a Receive entry labels one of the wallet's own addresses. Stored canonical. | `labels.invalid_address`, `labels.duplicate_address` (listed and not `replace`, or listed with the other purpose), `labels.own_address` (Send entry for an own address), `invalid_argument` (Receive entry for a foreign address) | QT-095, QT-098 | **works** |
| `delete_address_book_entry(id, address)` | async | Send entries only. | `labels.entry_not_found`, `labels.receive_entry_not_deletable` | QT-095 | **works** |
| `set_tx_label(id, txid, label)` | async | `None` or empty clears. `txid` must be 64 lowercase hex. | `invalid_argument` | QT-090 | **works** |

### 2.10 Message (`message.rs`) — owner E2 (signing through B's `VaultSigner`)

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `verify_message(network, address, message, signature)` | sync, free | `Ok(())` when valid. dash-qt result texts map 1:1 from the codes. | `message.invalid_address`, `message.address_no_key`, `message.malformed_signature`, `message.pubkey_not_recovered`, `message.not_signed` | QT-100 | **works** |
| `NetworkSession.sign_message(id, address, message, grant_id)` | async | Base64 65-byte compact signature, magic `"DarkCoin Signed Message:\n"`. `SignMessage` grant, redeemed only after the address checks pass. Signs through dw-vault's `VaultSigner`. | `message.address_not_mine`, `message.address_no_key`, `message.watch_only`, `message.vault_locked`, `message.grant_invalid` | QT-099 | **works** |

### 2.11 URI and QR (`uri.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `parse_payment_uri(network, text)` | sync, free | dash-qt `handleURIOrFile` + `parseBitcoinURI`, address checked on `network`. `PaymentUri { address, amount, label, message }`; amount 0 → `None`. A negative amount or one above the maximum supply (21 M DASH; dash-qt accepts both, then fails to send) is `uri.invalid_amount`. | `uri.double_slash`, `uri.not_dash_uri`, `uri.unparsable`, `uri.bip70_unsupported`, `uri.invalid_address{problem}`, `uri.invalid_amount` | QT-054, QT-149, IOS-048 | **works** |
| `build_payment_uri(address, amount, label, message)` | sync, free | dash-qt `formatBitcoinURI`, byte for byte. An amount above 21 M DASH is `uri.invalid_amount`. | `uri.invalid_amount` | QT-085 | **works** |
| `classify_address(network, text)` | sync, free | `Core{script_hash}`, `Platform`, `Shielded`, `Invalid{problem}`. | — | QT-055, QT-067, IOS-042 | **works** |
| `qr_matrix(text)` | sync, free | `QrMatrix { size, modules }`, row-major, `true` = dark, ECC L, no quiet zone; > 255 chars → `uri.too_long_for_qr`. Implement with the `qrcode` crate in dw-uri (no image crosses the FFI). | `uri.too_long_for_qr` | QT-084, IOS-053 | **works** |

The iOS superset parser (`pay:`, `dashwallet:`, `sender/user/currency/local`) and deep-link classification
stay in dw-uri for M2 (IOS-048 OS registration).

### 2.12 Units (`units.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `format_amount(amount, unit, network, style)` | sync, free | `AmountStyle`: `Plain`, `WithUnit`, `Floored{digits ≤ 8}`, `Privacy{hidden}` (discreet mode), `Gui{signed, truncate}`. Separators are U+2009. Off mainnet the unit names are `tDASH`-style. | `invalid_argument` (digits > 8) | QT-020, QT-036, QT-039, QT-152 | **works** |
| `parse_amount(text, unit)` | sync, free | `BitcoinUnits::parse` rules; range checks are the caller's. | `units.unparsable` | QT-056 | **works** |
| `unit_name(unit, network)` | sync, free | `DASH`, `mtDASH`, `μDASH`, `tduffs`, … | — | QT-152 | **works** |

## 3. Events (`EngineEvent`, `engine.rs`)

| Event | When | Host reaction | Emitter |
|---|---|---|---|
| `SessionOpened` / `SessionClosed {network}` | session lifecycle | lifecycle overlay | M0 |
| `WalletCreated {network, wallet_id}` | wallet registered (create or import) | reload wallet list | M0 |
| `WalletRemoved {network, wallet_id}` | `remove_wallet` deleted the wallet's rows (sent before the vault records are deleted) | reload wallet list | E1 |
| `SpvStateChanged {network, running}` | SPV started/stopped | status bar | M0 |
| `Sync {network, snapshot}` | snapshot changed (≤ 4 Hz, trailing edge kept) | `SPVCoordinator` | E1 |
| `Balances {network, wallet_id, balances: Option}` | balance buckets changed, or became known (≤ 4 Hz per wallet set, trailing edge kept) | Home / status bar | E1 |
| `HistoryChanged {network, wallet_id, txids}` | tx added or status changed, including confirmations of young transactions on a new block; `txids` empty = reload all (rescan) | re-query `history_page`, current receive address | E1 |
| `LockState {network, state}` | vault lock state changed | lock screen, status bar | B (emitted) |
| `Notice {network, code, detail}` | `PlatformContextUnavailable`, `SpvError`, `UncleanShutdown`, `SyncStalled` (engine, once per 45 s stall), `WalletSecretNotDeleted` (no longer sent since E0-04 DEC-134: `remove_wallet` fails closed instead; kept in the bindings); `DashPayStartupIncomplete` (E0-05: a wallet's bring-up ended `PartialNoIdentity`, `DiscoveryFailed`, `PartialAccountsPending`, `SeedBindingUnverified` or `IdentityScanIncomplete`, or the unlock drain was refused for the seed binding; `detail` is `wallet <id>: <status>` with the snake_case status; withheld by dw-ffi until E0-13 binds it); `BackupFailed` is never sent yet (no automatic backups) | banner / log | E1 |

`WalletCreated` is also sent when keys are attached to a registered wallet. The M0 events
`SyncProgress`, `PeersChanged` and `WalletChanged` are removed.

The observer callback runs on an engine thread, must return quickly and must not call back into the
engine synchronously.

## 4. Error codes

Common to every domain (same code everywhere): `invalid_argument`, `network_not_open`, `wallet_not_found`,
`storage`, `not_implemented`, `internal`. `UnitsError` and `UriError` (pure) have only `invalid_argument`
and `not_implemented` of these. Domain codes:

| Domain enum | Codes |
|---|---|
| `VaultError` | `vault.no_vault`, `vault.already_exists`, `vault.locked`, `vault.wrong_passphrase`, `vault.throttled`, `vault.passphrase_rejected`, `vault.not_encrypted`, `vault.already_encrypted`, `vault.grant_invalid`, `vault.grant_purpose_mismatch`, `vault.credential_required`, `vault.mixing_only`, `vault.no_secret`, `vault.quick_unlock_unavailable`, `vault.os_store_unavailable`, `vault.corrupt` |
| `WalletError` | `wallet.invalid_mnemonic`, `wallet.unsupported_word_count`, `wallet.already_exists`, `wallet.watch_only_exists`, `wallet.no_vault`, `wallet.vault_locked`, `wallet.grant_invalid`, `wallet.name_rejected` |
| `SyncError` | `sync.spv_not_running`, `sync.height_out_of_range`, `sync.spv` |
| `HistoryError` | `history.invalid_query`, `history.stale_cursor`, `history.tx_not_found` |
| `ReceiveError` | `receive.request_not_found`, `receive.gap_limit` |
| `SendError` | `send.no_recipients`, `send.invalid_address`, `send.platform_address`, `send.invalid_amount`, `send.dust_amount`, `send.duplicate_address`, `send.amount_exceeds_balance`, `send.amount_with_fee_exceeds_balance`, `send.amount_too_small_after_fee`, `send.insufficient_mixed_funds`, `send.outpoint_unavailable`, `send.absurd_fee`, `send.tx_too_large`, `send.invalid_change_address`, `send.watch_only`, `send.vault_locked`, `send.grant_invalid`, `send.grant_exceeded`, `send.prepared_tx_spent`, `send.no_peers`, `send.broadcast_rejected`, `send.broadcast_unknown` |
| `CoinsError` | `coins.outpoint_not_found` |
| `LabelsError` | `labels.invalid_address`, `labels.duplicate_address`, `labels.own_address`, `labels.entry_not_found`, `labels.receive_entry_not_deletable` |
| `MessageError` | `message.invalid_address`, `message.address_no_key`, `message.malformed_signature`, `message.pubkey_not_recovered`, `message.not_signed`, `message.address_not_mine`, `message.watch_only`, `message.vault_locked`, `message.grant_invalid` |
| `UriError` | `uri.double_slash`, `uri.not_dash_uri`, `uri.unparsable`, `uri.bip70_unsupported`, `uri.invalid_address`, `uri.invalid_amount`, `uri.too_long_for_qr` |
| `UnitsError` | `units.unparsable` |
| `EngineError` (M0 calls) | `invalid_config`, `invalid_argument`, `network_not_open`, `storage_in_use`, `storage`, `wallet_not_found`, `invalid_mnemonic`, `wallet_already_exists`, `wallet`, `sdk`, `spv`, `io`, `not_implemented`, `internal` |

Each FFI error enum exports `code()` returning these strings (a method on the generated Swift type, e.g.
`SendError.code()`); DashKit maps the generated Swift cases to the same strings (`DashKitError.code`,
`ServiceErrorCode`) and may read `code()` instead. A dw-ffi test checks the `EngineError` row against its
variants. Swift chooses dash-qt / iOS copy by code (QT-062,
IOS-051). The send codes cover dash-qt's `SendCoinsReturn` statuses.

`send.cancelled` joins `SendError` with DP3-01, for a contact payment that Lock refused before its hand-off (E0-04
design §4.6), and with E0-04 P5 for M1 sends. It is bound in the chosen stack in E0-13; the frozen Swift shells see
it mapped onto an existing send code.

## 5. Work routed to owners (M0 review findings)

Done in this change: M6 (distinct invalid-mnemonic / already-exists errors), M8 (headless resolve keeps
`Package.resolved`), M9 (`build-core.sh` target-dir default), L1 (bundle variants stamped with a hash of
`rust/` and the cargo profile; stale variants are left out of `info.json`), L5 (`@attr(args) import` lint), L7 (edition 2024
everywhere, README test command, unused dev-dep, redundant script, DesignTokensTests in Docker), and the
`tests/` vs `Tests/` collision (harness moved to `regtest/`).

### E1 engine-core
Status: done in m1b/e1-engine-core: §2.1 removals, §2.3 registry calls, §2.4, §2.5, §2.6, the E1
events in §3, H2, M1, M3, M4, M7, L2 (the drop thread; C still releases off the main thread), L6,
review M-3 (Option balances), the Low items on `SyncError` mapping, the stall-rule owner (engine)
and lower-case wallet ids. Regtest: `regtest/harness/tests/test_l1_sync.py`.
Original routing:
- Implement §2.1 removals, §2.3 registry calls, §2.4, §2.5, §2.6 and the E1 events in §3.
- **H2** `SessionEventBridge::progress_due` drops the last progress update of a burst. Debounce with a
  trailing edge (timer flush) for `Sync`, `Balances` and `HistoryChanged`.
- **M1** `NetworkSession::close` takes the manager while other calls may be running on it. Make close wait
  for (or cancel) in-flight operations, and make calls that start after close return `network_not_open`.
- **M3** `LazyTrustedContext::get_or_try_init` does a blocking DNS check. Only the first attempt runs in
  `spawn_blocking`; the lazy retries come through the sync `ContextProvider` methods (`provider()`), which
  the SDK calls from tokio workers. Move retries to a background task (or `block_in_place`) so no worker
  blocks on DNS.
- **M4** `on_wallet_event` emits one `WalletChanged` per platform-wallet event (a block can produce
  hundreds). Debounce per wallet and emit `Balances` / `HistoryChanged` instead.
- **M7** `create_private_dir` chmods the network dir to 0700. When the user chose the data root (QT-004,
  `--datadir`) the engine must only restrict directories it created.
- **L2** The last `Arc<Engine>` can drop on a Swift thread and block it in the runtime's shutdown. Make
  `Drop` hand the runtime to a background thread (`Runtime::shutdown_background`) — coordinate with C.
- **L6** Devnet names are case-sensitive but data dirs live on case-insensitive file systems: lowercase
  (or reject upper-case) names in `DashNetwork::validate`.
- Keep `wallet_infos`, `sync_snapshot`, `peers` sync calls in-memory (cache names from dw-appdb).
- Remove `list_wallets` and the M0 events once C has migrated.

### E2 engine-send
Status: `dw-appdb`, §2.7–§2.12 work (except `FullyMixedOnly`), tested in dw-engine (`send::plan` unit tests with a
key-wallet builder parity check, `send::flow_tests` offline flows that sign through the vault), dw-ffi
(`send_tests`) and the regtest suite `regtest/harness/tests/test_l1_send.py` (dwcli over SPV against dashd).
Review findings H-3, H-4 (option A), M-4, M-5, M-7 (engine side), M-8 (contract) and the send Lows are settled in
§2.7.1; Swift-side follow-ups are in m1-swift.md §3 (SendViewModel). Final review (scratch/m1/final-review.md),
engine side, done in m1fix/engine: M1 (`send.no_peers` keeps releasing; hosts prepare again), M2 (send, coins and
labels calls admitted by the session gate; `dust_protection`, `set_dust_protection`, `tx_label` and `tx_message`
now run on the engine runtime instead of the caller's executor), M3 (unknown-outcome payments stay reserved and
re-broadcastable, released only when seen spent or at close), M7 (address-book rule above, written only after a
sent or unknown broadcast), M8 (§2.3), the coin-control dry run, the URI maximum-supply check, the build-stamp
profile and the exported `code()`. Regtest: `test_rebroadcast_after_unknown_outcome` in `test_l1_send.py`. Still
owed:
- `FullyMixedOnly` (and `utxos.fully_mixed_only`, `coinjoin_rounds`) need per-coin CoinJoin rounds (WS-06).
- Grant ↔ wallet binding (review M-6): done in the vault (§2.2).
- `nSequence`/locktime differ from dash-qt (key-wallet builder), see §2.7.1.

### B vault
Status on main: dw-vault, §2.2 (except quick unlock, M2), `generate_mnemonic`, `check_mnemonic`,
the vault side of `import_wallet`, `VaultSigner` and `sign_message` are done and tested through the
engine and the FFI. H1 and M5 are fixed: `create_wallet` is no longer exported, and the import order
is derive → store and read back → register, rolling back only the records this call wrote.
Final-review fixes (m1fix/vault): H1 (one writer lock over every vault-file read-modify-write and
passphrase check; writes start from the in-memory file), H2 (`dw-vault/tests/`), M4 (passphrase grants keep
the lock state), M5 (Rust enforces the credential table in §2.2), the throttle Low (attempts serialized),
previous M-6 (grants bound to a wallet). `remove_wallet` deletes the records with the redeemed `Wipe`
token (`Vault::wipe_wallet_secret`), which carries the key of a passphrase grant on a locked vault.
Still owed:
- Watch-only import (`wallet.watch_only_exists`) once watch-only wallets exist.
- `lookahead` (QT-105, default 1000 for dash-qt restores) needs a per-wallet gap limit upstream (U12);
  until then `ImportOptions.lookahead` returns `NotImplemented`.
- Decide whether returned secret bytes need a zeroing transfer type (UniFFI frees `RustBuffer` unzeroed;
  `VaultCredential` and `RevealedMnemonic` are no longer `Clone`, and Rust moves rather than copies them).

### C Swift runtime (see m1-swift.md)
- **M2** `EngineClient.open` caches the session object; after an engine-side close it returns a closed
  session. Re-open when `isOpen()` is false.
- **M4 (Swift)** `EventBus` subscribers buffer with `bufferingNewest(256)`: lifecycle events
  (`sessionOpened/Closed`, `walletCreated/Removed`, `lockStateChanged`) can be dropped under load. Give
  lifecycle events an unbounded or separate stream.
- **L2** Release the engine off the main thread (see E1 L2).
- **L3** `SecretBytes` wiping: use a non-elidable wipe (e.g. `memset_s` / volatile writes) and avoid
  intermediate `Array` copies; the WalletRuntime `SecretBuffer` conformance should wrap `SecretBytes`.
