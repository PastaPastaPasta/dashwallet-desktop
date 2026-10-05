# M1 engine contract (`dw-ffi`)

Status: **contract**, 2026-10-05. Code: `rust/crates/dw-ffi/src/api/*.rs`; generated Swift:
`Sources/DashWalletCore/Generated/DashWalletCore.swift`. Design background: DESIGN-opus §1.5 (FFI rules),
§1.8 (vault), §1.12 (runtime). The Swift side of the contract is [`m1-swift.md`](m1-swift.md).

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
| Wallet id | 64-char lowercase hex `String`. Every call that takes one parses it first and returns `invalid_argument` for a malformed id, even when the rest is a stub. |
| Txid | 64-char lowercase hex, display (RPC) byte order. |
| Amounts | Duffs. `u64` for quantities, `i64` for signed net amounts (history). |
| Times | UNIX seconds `u64`. `None` means unknown; never 0 as a placeholder. |
| Unknown values | `Option`. A balance, fee or height the engine does not know is `None` (iOS rule 7). |
| Secrets in | `Vec<u8>` (Swift `Data`): passphrases, phrases, quick-unlock keys. Rust wraps them in `Zeroizing` on entry. |
| Secrets out | Only `generate_mnemonic` and `Vault.reveal_mnemonic`, as bytes. DashKit copies them into `SecretBytes` at once and zeroes the `Data`. Residual risk: UniFFI frees the returned `RustBuffer` without zeroing it (B decides on a zeroing transfer type). |
| Async | Every `async` export spawns its work on the engine's tokio runtime (`NetworkSession::on_runtime` pattern) and only awaits the join handle, so UniFFI's Swift executor never blocks. |
| Sync | A sync export is O(1) or reads in-memory state. No SQLite or network I/O on the caller's thread: E1/E2 keep the data a sync call needs (wallet names, sync snapshot, peers) in memory. |
| Pure functions | `units`, `uri`, `verify_message`, `generate_mnemonic`, `check_mnemonic` are free functions; they need no session and run on the caller's thread. |
| Events | Signals, not data (DESIGN-opus §1.5 rule 4). Hosts re-query on arrival. Rust debounces each domain to ≤ 4 Hz and **always delivers the last change of a burst** (review H2). |
| Errors | One `uniffi::Error` enum per domain. Each variant has a stable code (§4). `detail` strings are diagnostics for logs; Rust never produces user-facing copy. |
| Grants | Calls that spend, reveal, sign or wipe take a `grant_id` from `Vault.authorize`. The engine checks purpose, expiry and (for `Spend`) the debit cap. |

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
| `NetworkSession.start_spv()` / `stop_spv()` / `spv_running()` | async/async/sync | dash-spv with masternode sync on. | `EngineError` | QT-024/025 | M0 |
| `NetworkSession.platform_context_ready()` / `is_open()` / `network()` | sync | State reads. | — | — | M0 |

### 2.2 Vault (`vault.rs`) — owner B

`NetworkSession.vault() -> Vault` returns a handle to the network's vault (sync, cheap).

| Call | Kind | Semantics | Errors (besides common) | Serves | Status |
|---|---|---|---|---|---|
| `Vault.status()` | sync | `VaultStatus { state, encrypted, quick_unlock_enrolled, failed_attempts, retry_after_secs, wallets_with_secrets }`. `state` ∈ `NoVault, NoKeys, Unencrypted, Locked, UnlockedMixingOnly, Unlocked`. | — | QT-022, IOS-013 | stub (returns `network_not_open` on a closed session) |
| `Vault.create(passphrase: Option<bytes>)` | async | `Some` = encrypted (slot P, Argon2id), `None` = unencrypted (slot O, OS store). Leaves the vault unlocked. | `vault.already_exists`, `vault.passphrase_rejected`, `vault.os_store_unavailable` | QT-102, QT-111, IOS-010 | stub |
| `Vault.encrypt(new_passphrase, grant_id)` | async | dash-qt "Encrypt Wallet": add slot P, delete slot O. `ChangeCredential` grant. | `vault.already_encrypted`, `vault.grant_invalid` | QT-111 | stub |
| `Vault.unlock(passphrase, scope: Full\|MixingOnly)` | async | Unwraps the DEK. Failed attempts count toward the IOS-012 throttle (`6^(n−3)·60 s`). | `vault.wrong_passphrase{failed_attempts, retry_after_secs}`, `vault.throttled`, `vault.not_encrypted` | QT-111, QT-112, IOS-012/013 | stub |
| `Vault.lock()` | sync | Drops the DEK, revokes all grants. Idempotent. | — | QT-111, IOS-015 | stub |
| `Vault.change_passphrase(old, new)` | async | Re-wraps the DEK; seed unchanged. | `vault.wrong_passphrase`, `vault.throttled`, `vault.passphrase_rejected` | QT-111 | stub |
| `Vault.authorize(purpose, credential)` | async | Issues `AuthGrant { id, purpose, expires_at, single_use }`. Credentials: `Passphrase{bytes}`, `QuickUnlock{wrap_key}` (M2), `Unencrypted`. A passphrase also unlocks a locked vault. Purposes: `Spend{max_duffs}`, `RevealSecret`, `SignMessage`, `ChangeCredential`, `Wipe`, `MasternodeOp`, `Governance`, `PlatformOp`. | `vault.wrong_passphrase`, `vault.throttled`, `vault.mixing_only`, `vault.quick_unlock_unavailable` | IOS-016/017 | stub |
| `Vault.revoke_grant(grant_id)` | sync | Unknown ids ignored. | — | IOS-017 | stub |
| `Vault.reveal_mnemonic(wallet_id, grant_id)` | async | `RevealedMnemonic { phrase, bip39_passphrase }` as bytes (DESIGN R1: passphrase shown on reveal). `RevealSecret` grant. | `vault.no_secret`, `vault.grant_invalid`, `vault.grant_purpose_mismatch`, `vault.locked` | QT-113, IOS-006 | stub |
| `Vault.enroll_quick_unlock(grant_id)` / `remove_quick_unlock()` | async | Biometric slot B (M2). | `vault.quick_unlock_unavailable` | IOS-011 | stub (M2) |

### 2.3 Wallets (`wallet.rs`) — owners B (generate/check/import) and E1 (registry)

| Call | Kind | Semantics | Errors (besides common) | Serves | Owner | Status |
|---|---|---|---|---|---|---|
| `generate_mnemonic(word_count, language)` | sync, free | Fresh phrase (12/15/18/21/24 words) as UTF-8 bytes. **Stores nothing.** The host shows it, runs the verify step, then calls `import_wallet`. | `wallet.unsupported_word_count` | QT-102/103, IOS-002…004 | B | stub |
| `check_mnemonic(phrase)` | sync, free | `MnemonicCheck { word_count, unknown_word_indices, language, checksum: Valid\|CoreOnly\|Invalid }` for live restore validation. `CoreOnly` = fails BIP39, passes Dash Core's weak check. | — | QT-104, IOS-007 | B | stub |
| `NetworkSession.import_wallet(mnemonic, bip39_passphrase, options)` | async | Stores phrase and passphrase in the vault, then registers the wallet, in DESIGN-opus §1.8 seed-safety order. `ImportOptions { name, birth_height, core_compat, lookahead }`. Returns the wallet id. Over an existing watch-only wallet with the same id: attach the keys, or `wallet.watch_only_exists`. | `wallet.invalid_mnemonic`, `wallet.already_exists`, `wallet.watch_only_exists`, `wallet.no_vault`, `wallet.vault_locked`, `wallet.name_rejected` | QT-104/105, IOS-007, IOS-123 | B | **partial**: an empty passphrase with `name`, `core_compat`, `lookahead` unset registers the wallet as M0 did (phrase **not stored**, review H1); anything else → `NotImplemented`. Invalid phrase and duplicate already map to distinct errors (M6 fixed). |
| `NetworkSession.create_wallet(word_count)` | async | **M0, superseded.** Registers a wallet and returns the phrase as `String`. | `EngineError` | — | B removes it | M0 |
| `NetworkSession.list_wallets()` | sync | **M0, superseded by `wallet_infos`.** | `EngineError` | — | E1 removes it | M0 |
| `NetworkSession.wallet_infos()` / `wallet_info(id)` | sync | `WalletInfo { wallet_id, name, watch_only, has_mnemonic, hd, birth_height, created_at, balances }`, creation order. | — | QT-014, QT-021, QT-035, QT-101, IOS-110 | E1 | stub |
| `NetworkSession.balances(id)` | sync | `WalletBalances { confirmed, unconfirmed, immature, locked, total }`. | `EngineError` | QT-034, IOS-019/021 | E1 | M0 |
| `NetworkSession.remove_wallet(id, grant_id)` | async | Unloads, deletes wallet rows, vault records and app metadata. `Wipe` grant. Emits `WalletRemoved`. | `wallet.grant_invalid` | QT-101, IOS-109 | E1 (+B for vault records) | stub |
| `NetworkSession.rename_wallet(id, name)` | async | 1–64 chars after trimming; stored in `dw-appdb`. | `wallet.name_rejected` | QT-014, IOS-110 | E1 | stub |

### 2.4 Sync (`sync.rs`) — owner E1

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.sync_snapshot()` | sync | `SyncSnapshot { running, phases: [SyncPhaseProgress{phase, current_height, target_height, done}], active_phase, tip_height, tip_time, chainlock_height, connected_peers, caught_up, seconds_since_progress }`. Phases: `Headers, FilterHeaders, Filters, Masternodes`. `caught_up` = dash-spv steady state (the iOS `syncDone` gate). Raw values; damping is Swift's. | — | QT-024/025/027, IOS-023 | stub |
| `NetworkSession.peers()` | sync | `[PeerInfo { address, user_agent, protocol_version, best_height, ping_ms, connected_since, inbound, bytes_sent, bytes_received }]`. | — | QT-147, IOS-023 | stub |
| `NetworkSession.rotate_peers()` | async | Drop current peers, connect to new ones ("Change peers"). | `sync.spv_not_running` | IOS-023 | stub |
| `NetworkSession.rescan(from: WalletBirth\|Genesis\|Height{h})` | async | Schedules a filter rescan for every wallet; progress via `Sync` events. | `sync.height_out_of_range`, `sync.spv_not_running` | QT-117, QT-148, IOS-113 | stub |

### 2.5 History (`history.rs`) — owner E1

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.history_page(id, query)` | async | `HistoryQuery { filter, sort, cursor, limit (1..=500) }` → `HistoryPage { records, next_cursor, total_matching }`. Filter: `types`, `categories`, `statuses`, `date_from` (inclusive), `date_to` (**exclusive**, dash-qt), `text` (case-insensitive address/label/txid), `min_amount` (absolute), `watch_only`. Empty list / `None` = any. Records are dash-qt `TransactionRecord`s: one per non-change output for sends, fee on the first; `record_index` orders them. | `history.invalid_query`, `history.stale_cursor` | QT-086…089, QT-094 (rows for CSV), IOS-027/028 | stub |
| `NetworkSession.tx_detail(id, txid)` | async | `TxDetail { records, status, timestamp, block_height, block_hash, fee, size_bytes, inputs, outputs, message, label, raw_hex }`. | `history.tx_not_found` | QT-092, IOS-031 | stub |

Classification: `TxType` is dash-qt's 19-value enum in dash-qt's order (research 02 §4.1); `TxCategory`
is the iOS filter category (`Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform,
Other`). `TxStatus { kind, confirmations, instant_locked, chain_locked, matures_in }` follows dash-qt's
rules: depth < 0 Conflicted; 0 Unconfirmed/Abandoned; < 6 and not ChainLocked Confirming; ChainLock
confirms at once; coinbase Immature/NotAccepted. `counts_toward_balance = false` renders brackets.

### 2.6 Receive (`receive.rs`) — owner E1 (requests stored in E2's `dw-appdb`)

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `current_receive_address(id)` | async | First unused external address as `AddressInfo { address, chain, index, derivation_path, used, label, balance, tx_count }`. Re-queried after `HistoryChanged`, which rotates it once paid. | `receive.gap_limit` | IOS-053/054 | stub |
| `next_receive_address(id, label)` | async | Issues and labels a fresh address. | `receive.gap_limit` | QT-081 | stub |
| `addresses(id, AddressFilter{chain, used})` | async | Standard BIP44 account addresses. | — | QT-096 | stub |
| `create_receive_request(id, amount, label, message)` | async | Stores a `ReceiveRequest { id, created_at, address, amount, label, message, uri }` on a fresh address; `uri` from dw-uri `format_bitcoin_uri`. | — | QT-081/082/085, IOS-055 | stub |
| `receive_requests(id)` / `delete_receive_request(id, request_id)` | async | Newest first. | `receive.request_not_found` | QT-083 | stub |

### 2.7 Send (`send.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `NetworkSession.new_tx_draft(id)` | sync | Empty draft: source `Any`, fee `Recommended{6}`, change `Auto`. | — | QT-052 | stub |
| `NetworkSession.max_spendable(id, source, fee)` | async | Largest single-recipient amount with the fee subtracted. | `send.insufficient_mixed_funds` | QT-053, IOS-044 | stub |
| `TxDraft.set_recipients([Recipient{address, amount, subtract_fee_from_amount, label, message}])` | sync | Offline validation; errors carry the recipient `index`. Platform (DIP-18) addresses are rejected on L1. | `send.no_recipients`, `send.invalid_address`, `send.platform_address`, `send.invalid_amount`, `send.dust_amount`, `send.duplicate_address` | QT-052…056, QT-060, QT-067 | stub |
| `TxDraft.set_source(Any\|FullyMixedOnly\|Outpoints)` | sync | Coin control and the CoinJoin send page. | `send.outpoint_unavailable` | QT-051, QT-068…071 | stub |
| `TxDraft.set_fee(Recommended{target_blocks}\|PerKb{duffs_per_kb})` | sync | On SPV every target uses the minimum relay fee (DESIGN-opus §1.14). | `invalid_argument` | QT-057 | stub |
| `TxDraft.set_change(Auto\|Address{address})` | sync | Custom change address. | `send.invalid_change_address` | QT-073 | stub |
| `TxDraft.estimate()` | async | `TxEstimate { fee, size_bytes, input_count, change, total_sent }`; nothing signed or reserved. | balance errors, `send.tx_too_large` | QT-072, IOS-044 | stub |
| `TxDraft.prepare(grant_id)` | async | Select coins, build, sign through `VaultSigner`, reserve inputs. **Never broadcasts.** Returns `PreparedTx`; `PreparedTx.summary()` = `PreparedTxSummary { txid, fee, fee_rate_per_kb, size_bytes, inputs, outputs, total_sent, total_debit }`. `Spend` grant; `total_debit` must be ≤ `max_duffs`. | `send.amount_exceeds_balance{available}`, `send.amount_with_fee_exceeds_balance{fee, available}`, `send.absurd_fee`, `send.watch_only`, `send.vault_locked`, `send.grant_invalid`, `send.grant_exceeded` | QT-058/059/061, QT-064/065/066, IOS-046 | stub |
| `TxDraft.broadcast(prepared)` | async | Announces, records in history, emits `HistoryChanged`/`Balances`. | `send.prepared_tx_spent`, `send.no_peers`, `send.broadcast_rejected{reason}` | QT-062/063, IOS-052 | stub |
| `TxDraft.abandon(prepared)` | async | Releases reserved inputs. Idempotent. | — | IOS rule 4 | stub |

### 2.8 Coins (`coins.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `utxos(id, UtxoFilter{include_locked, fully_mixed_only, min_confirmations})` | async | `Utxo { outpoint, address, amount, confirmations, block_height, timestamp, instant_locked, chain_locked, user_locked, reserved, label, is_change, is_coinbase, coinjoin_denominated, coinjoin_rounds, spendable }`. | — | QT-068…071, QT-075 | stub |
| `lock_outpoints` / `unlock_outpoints(id, outpoints)` | async | Persisted user locks (dw-appdb). | `coins.outpoint_not_found` | QT-070 | stub |
| `locked_outpoints(id)` | async | — | — | QT-070 | stub |

### 2.9 Labels and address book (`labels.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `address_book(id, purpose, search)` | async | Per-wallet entries (dash-qt keeps the book per wallet). `search` matches label or address. | — | QT-095…097 | stub |
| `save_address_book_entry(id, address, label, purpose, replace)` | async | Add, or relabel with `replace`. A `Receive` entry labels one of the wallet's own addresses. | `labels.invalid_address`, `labels.duplicate_address`, `labels.own_address` | QT-095, QT-098 | stub |
| `delete_address_book_entry(id, address)` | async | Send entries only. | `labels.entry_not_found`, `labels.receive_entry_not_deletable` | QT-095 | stub |
| `set_tx_label(id, txid, label)` | async | `None` clears. | — | QT-090 | stub |

### 2.10 Message (`message.rs`) — owner E2 (signing through B's `VaultSigner`)

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `verify_message(network, address, message, signature)` | sync, free | `Ok(())` when valid. dash-qt result texts map 1:1 from the codes. | `message.invalid_address`, `message.address_no_key`, `message.malformed_signature`, `message.pubkey_not_recovered`, `message.not_signed` | QT-100 | **works** |
| `NetworkSession.sign_message(id, address, message, grant_id)` | async | Base64 65-byte compact signature, magic `"DarkCoin Signed Message:\n"`. `SignMessage` grant. | `message.address_not_mine`, `message.address_no_key`, `message.watch_only`, `message.vault_locked`, `message.grant_invalid` | QT-099 | stub |

### 2.11 URI and QR (`uri.rs`) — owner E2

| Call | Kind | Semantics | Errors | Serves | Status |
|---|---|---|---|---|---|
| `parse_payment_uri(network, text)` | sync, free | dash-qt `handleURIOrFile` + `parseBitcoinURI`, address checked on `network`. `PaymentUri { address, amount, label, message }`; amount 0 → `None`. A negative amount (dash-qt accepts it, then fails to send) is `uri.invalid_amount`. | `uri.double_slash`, `uri.not_dash_uri`, `uri.unparsable`, `uri.bip70_unsupported`, `uri.invalid_address{problem}`, `uri.invalid_amount` | QT-054, QT-149, IOS-048 | **works** |
| `build_payment_uri(address, amount, label, message)` | sync, free | dash-qt `formatBitcoinURI`, byte for byte. | `uri.invalid_amount` | QT-085 | **works** |
| `classify_address(network, text)` | sync, free | `Core{script_hash}`, `Platform`, `Shielded`, `Invalid{problem}`. | — | QT-055, QT-067, IOS-042 | **works** |
| `qr_matrix(text)` | sync, free | `QrMatrix { size, modules }`, row-major, `true` = dark, ECC L, no quiet zone; > 255 chars → `uri.too_long_for_qr`. Implement with the `qrcode` crate in dw-uri (no image crosses the FFI). | `uri.too_long_for_qr` | QT-084, IOS-053 | stub |

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
| `WalletRemoved {network, wallet_id}` | `remove_wallet` done | reload wallet list | E1 |
| `SpvStateChanged {network, running}` | SPV started/stopped | status bar | M0 |
| `Sync {network, snapshot}` | snapshot changed (≤ 4 Hz, trailing edge kept) | `SPVCoordinator` | E1 |
| `Balances {network, wallet_id, balances}` | balance buckets changed | Home / status bar | E1 |
| `HistoryChanged {network, wallet_id, txids}` | tx added or status changed (`txids` empty = reload all) | re-query `history_page`, current receive address | E1 |
| `LockState {network, state}` | vault lock state changed | lock screen, status bar | B |
| `Notice {network, code, detail}` | `PlatformContextUnavailable`, `SpvError`, `UncleanShutdown`, `SyncStalled`, `BackupFailed` | banner / log | E1 |
| `SyncProgress`, `PeersChanged`, `WalletChanged` | **M0**; E1 removes them once `Sync`, `Balances`, `HistoryChanged` are emitted and C has migrated | — | M0 |

The observer callback runs on an engine thread, must return quickly and must not call back into the
engine synchronously.

## 4. Error codes

Common to every domain (same code everywhere): `invalid_argument`, `network_not_open`, `wallet_not_found`,
`storage`, `not_implemented`, `internal`. `UnitsError` and `UriError` (pure) have only `invalid_argument`
and `not_implemented` of these. Domain codes:

| Domain enum | Codes |
|---|---|
| `VaultError` | `vault.no_vault`, `vault.already_exists`, `vault.locked`, `vault.wrong_passphrase`, `vault.throttled`, `vault.passphrase_rejected`, `vault.not_encrypted`, `vault.already_encrypted`, `vault.grant_invalid`, `vault.grant_purpose_mismatch`, `vault.mixing_only`, `vault.no_secret`, `vault.quick_unlock_unavailable`, `vault.os_store_unavailable`, `vault.corrupt` |
| `WalletError` | `wallet.invalid_mnemonic`, `wallet.unsupported_word_count`, `wallet.already_exists`, `wallet.watch_only_exists`, `wallet.no_vault`, `wallet.vault_locked`, `wallet.grant_invalid`, `wallet.name_rejected` |
| `SyncError` | `sync.spv_not_running`, `sync.height_out_of_range`, `sync.spv` |
| `HistoryError` | `history.invalid_query`, `history.stale_cursor`, `history.tx_not_found` |
| `ReceiveError` | `receive.request_not_found`, `receive.gap_limit` |
| `SendError` | `send.no_recipients`, `send.invalid_address`, `send.platform_address`, `send.invalid_amount`, `send.dust_amount`, `send.duplicate_address`, `send.amount_exceeds_balance`, `send.amount_with_fee_exceeds_balance`, `send.insufficient_mixed_funds`, `send.outpoint_unavailable`, `send.absurd_fee`, `send.tx_too_large`, `send.invalid_change_address`, `send.watch_only`, `send.vault_locked`, `send.grant_invalid`, `send.grant_exceeded`, `send.prepared_tx_spent`, `send.no_peers`, `send.broadcast_rejected` |
| `CoinsError` | `coins.outpoint_not_found` |
| `LabelsError` | `labels.invalid_address`, `labels.duplicate_address`, `labels.own_address`, `labels.entry_not_found`, `labels.receive_entry_not_deletable` |
| `MessageError` | `message.invalid_address`, `message.address_no_key`, `message.malformed_signature`, `message.pubkey_not_recovered`, `message.not_signed`, `message.address_not_mine`, `message.watch_only`, `message.vault_locked`, `message.grant_invalid` |
| `UriError` | `uri.double_slash`, `uri.not_dash_uri`, `uri.unparsable`, `uri.bip70_unsupported`, `uri.invalid_address`, `uri.invalid_amount`, `uri.too_long_for_qr` |
| `UnitsError` | `units.unparsable` |
| `EngineError` (M0 calls) | `invalid_config`, `invalid_argument`, `network_not_open`, `storage_in_use`, `storage`, `wallet_not_found`, `invalid_mnemonic`, `wallet_already_exists`, `wallet`, `sdk`, `spv`, `io`, `not_implemented`, `internal` |

Each Rust enum has `code()` returning these strings; DashKit maps the generated Swift cases to the same
strings (`DashKitError.code`, `ServiceErrorCode`). Swift chooses dash-qt / iOS copy by code (QT-062,
IOS-051). The send codes cover dash-qt's `SendCoinsReturn` statuses.

## 5. Work routed to owners (M0 review findings)

Done in this change: M6 (distinct invalid-mnemonic / already-exists errors), M8 (headless resolve keeps
`Package.resolved`), M9 (`build-core.sh` target-dir default), L1 (bundle variants stamped with a hash of
`rust/`; stale variants are left out of `info.json`), L5 (`@attr(args) import` lint), L7 (edition 2024
everywhere, README test command, unused dev-dep, redundant script, DesignTokensTests in Docker), and the
`tests/` vs `Tests/` collision (harness moved to `regtest/`).

### E1 engine-core
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
- Create `dw-appdb` (`app.sqlite` per network, refinery migrations, DESIGN-opus §1.7) with tables for
  wallet names (used by E1), address book, tx/address labels, receive requests, UTXO locks.
- Implement §2.7–§2.11 stubs: `TxDraft`/`PreparedTx` state (reserved inputs released on `abandon` and on
  drop of an un-broadcast `PreparedTx`), coin selection, fee policy, `sign_message` through `VaultSigner`,
  `qr_matrix` (`qrcode` crate in dw-uri).
- **L4** `dw-uri` `printf_2f` (iOS `local=` amounts): match printf's sign handling for negative and
  negative-zero values.

### B vault
- Create `dw-vault` and implement §2.2 and `generate_mnemonic`, `check_mnemonic`, the vault side of
  `import_wallet` and `remove_wallet`, `VaultSigner`.
- **H1** `import_wallet` must write the vault record, fsync, read it back and compare **before**
  registering the wallet (DESIGN-opus §1.8); roll back only the provisional record on failure. Remove
  `create_wallet`. Import over an existing watch-only wallet attaches the keys or returns
  `wallet.watch_only_exists`.
- **M5** Secrets cross the FFI as bytes only (done in the contract); remove the remaining `String` paths
  (`CreatedWallet.mnemonic`) with `create_wallet`.
- `core_compat` seeds go through `dw_compat::bip39core::core_seed`; `lookahead` defaults to 1000 for
  dash-qt restores (QT-105).
- Decide whether returned secret bytes need a zeroing transfer type (UniFFI frees `RustBuffer` unzeroed).

### C Swift runtime (see m1-swift.md)
- **M2** `EngineClient.open` caches the session object; after an engine-side close it returns a closed
  session. Re-open when `isOpen()` is false.
- **M4 (Swift)** `EventBus` subscribers buffer with `bufferingNewest(256)`: lifecycle events
  (`sessionOpened/Closed`, `walletCreated/Removed`, `lockStateChanged`) can be dropped under load. Give
  lifecycle events an unbounded or separate stream.
- **L2** Release the engine off the main thread (see E1 L2).
- **L3** `SecretBytes` wiping: use a non-elidable wipe (e.g. `memset_s` / volatile writes) and avoid
  intermediate `Array` copies; the WalletRuntime `SecretBuffer` conformance should wrap `SecretBytes`.
