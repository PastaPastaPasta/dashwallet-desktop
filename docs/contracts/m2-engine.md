# M2 engine contract (`dw-ffi`)

Status: **contract**, 2026-10-06. Milestone M2 "dash-qt L1 parity" (DESIGN-opus §5.1). Code:
`rust/crates/dw-ffi/src/api/{multiwallet,tx_actions,fees,tools,console,compat,backup,psbt,vault_m2,desktop}.rs`
plus the additions to M1 enums listed in §5. Generated Swift: `Sources/DashWalletCore/Generated/DashWalletCore.swift`.
The Swift side is [`m2-swift.md`](m2-swift.md). Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions)
still applies.

Every M2 call exists in the FFI now. Each stub first does what the finished call will do before any work:
parses its wallet id, txid and outpoints (`invalid_argument`) and checks the session (`network_not_open`). Then
it returns its domain's `NotImplemented { call }`, where `call` is `"<Object>.<method>"` or the free-function
name. No call fakes success. Two calls hold behaviour already: `parse_psbt` refuses input over 100 MiB
(`psbt.too_large`), and `desktop_quick_unlock_provider` returns `Unavailable`, which is the truth until slot B
ships. Implementers replace the stub body. They do not change a signature without updating this file,
m2-swift.md and the generated bindings in the same change. `rust/crates/dw-ffi/src/api/m2_tests.rs` tests the
stubs and checks the §4 codes against this file.

## 0. Owners

| Owner | Scope (paths) |
|---|---|
| **R1 engine-tools** | `dw-engine` (multiwallet, history actions, fees, tools, notifications, rescan), new crate `dw-console`, FFI `multiwallet.rs`, `tx_actions.rs`, `fees.rs`, `tools.rs`, `console.rs`, the `NewTransactions` / `WalletLoadChanged` events |
| **R2 compat** | `dw-compat` (dump, walletdat, export), new crate `dw-psbt`, `dw-engine::backup`, FFI `compat.rs`, `backup.rs`, `psbt.rs`, the regtest `restore` suite and §4.4 items 1–3 of DESIGN-opus |
| **S1 desktop services** | `dw-vault` slot B and recovery (`vault_m2.rs`, `enroll_quick_unlock`), new crate `dw-desktop` (FFI `desktop.rs`), `Sources/PlatformServices*`, `Sources/WalletRuntime` (adapters for every M2 protocol), `Sources/DashKit` mappings of the M2 calls |
| **V1 view models** | `Sources/WalletFeatures/**` (m2-swift.md §3) |
| **U UI** (later phase) | MacUI and CrossUI screens for all of the above |

S1 writes the WalletRuntime adapters and DashKit wrappers for every engine call here: the protocols in
m2-swift.md are the boundary, so V1 works against fakes until an adapter lands. R1 and R2 test through
`dw-engine`, the FFI and regtest. They do not edit Swift.

## 1. Item → owner

Primary owner first. "+" = supporting owner. Every item also needs U (MacUI + CrossUI screens) unless it has no
screen. Checklist text: research 02 §22 (QT) and research 03 §4 (IOS).

| Item | Area | Owner | Engine / service surface |
|---|---|---|---|
| QT-001 | single instance + URI hand-off | S1 + V1 | `acquire_single_instance`, `forward_to_primary`; macOS LaunchServices; `SingleInstanceCoordinating` |
| QT-004 | data-dir chooser, free-space check | S1 + V1 | `DataDirectoryInspecting` (Swift) |
| QT-005 | splash with phases, Q quits | S1 + V1 | `StartupProgressing` (Swift) |
| QT-006 | CLI flags (`--min`, `--resetguisettings`, `--lang`, `--windowtitle`, `--datadir`, network) | S1 | `LaunchOptions.parse` (Swift) |
| QT-007 | corrupt settings Reset/Abort | S1 + V1 | `SettingsStore.recoveredFromCorruption` (M1) + `OptionsProviding.reset` |
| QT-008 | shutdown window | S1 + V1 | `ShutdownCoordinating` (Swift), `Engine.shutdown` (M1) |
| QT-009 | autostart with `--min` | S1 | `set_autostart` / `autostart_enabled`; macOS hidden (dash-qt parity) |
| QT-011 | title + window geometry | V1 | `ShellModel` (Swift) |
| QT-012 | tab bar + shortcuts | V1 | `ShellModel` |
| QT-015…018 | File / Settings / Window / Help menus | V1 | `ShellModel.menus` |
| QT-019 | drag-and-drop URI | S1 + V1 | `parse_payment_uri` (M1) |
| QT-021 | HD status icon | V1 | `WalletInfo.hd` (M1) |
| QT-022 | lock icon, 4 states | V1 | `VaultStatus` (M1) |
| QT-028…030 | tray icon/menu, minimize to tray / on close | S1 + V1 | `TrayIcon`; macOS `MenuBarExtra` |
| QT-031…033 | tx notifications, batching, CoinJoin suppression | S1 + R1 | event `NewTransactions`, `tx_notices`; `SystemNotifying` |
| QT-035 | watch-only balance column | R1 + V1 | balances of watch-only wallets (§5), `TxRecord.involves_watch_only` |
| QT-040 | alert banner | R1 + V1 | `warnings` |
| QT-057, 058 | fee targets, fee caps | R1 + V1 | `fee_policy` |
| QT-068…071, 073 | coin control dialog, menu, CoinJoin filter, custom change | V1 + R1 | `utxos`, locks (M1), `coin_selection_summary` |
| QT-072 | size/fee estimate | R1 + V1 | `coin_selection_summary` |
| QT-074 | spent coins unselected | V1 + R1 | `CoinSelectionSummary.unavailable` |
| QT-075 | dust protection + "Unlock dust UTXO" | R1 + V1 | `set_dust_protection` (M1), `TxDetailExtras.dust_locked_outputs`, `DustReceive` records |
| QT-076…079 | PSBT controls, create unsigned, load, operations dialog | R2 + V1 | `TxDraft.create_unsigned`, `parse_psbt`, `analyze_psbt`, `sign_psbt`, `broadcast_psbt` |
| QT-086 | all 19 tx types | R1 | `TxType` of `history_page` (§5); display strings are V1's |
| QT-088 | watch-only column, brackets | V1 + R1 | `TxRecord` (M1) |
| QT-090 | context menu (copy raw / full details) | V1 + R1 | `TxDetail.raw_hex` (M1), `tx_detail_extras` |
| QT-091 | abandon / resend | R1 + V1 | `abandon_transaction`, `resend_transaction`, `TxDetailExtras.can_*` |
| QT-092 | every details field | R1 + V1 | `tx_detail` (M1) + `tx_detail_extras` |
| QT-093 | CSV, exact bytes incl. watch-only column | R1 + V1 | `export_history_csv` |
| QT-095, 096 | address book CSV export + QR | V1 | M1 calls; CSV is a two-column Swift writer |
| QT-101 | multiwallet open/close/load-on-startup | R1 + V1 | `wallet_load_states`, `load_wallet`, `unload_wallet`, `set_load_on_startup`, event `WalletLoadChanged` |
| QT-106 | wallet.dat restore (SQLite; BDB M6) | R2 + V1 | `inspect_wallet_file`, `import_wallet_dat` |
| QT-107 | dumpwallet import (HD) | R2 + V1 | `import_dump_wallet` |
| QT-108 | hdseed / xprv / listdescriptors import | R2 + V1 | `import_key_material` |
| QT-109 | export for dash-qt | R2 + V1 | `export_for_core`, `core_mnemonic_compatibility`, `Vault.reveal_mnemonic` (M1) |
| QT-110 | backup wallet to a file | R2 + V1 | `backup_wallet`, `restore_backup` |
| QT-114 | watch-only wallets | R1 + V1 | `import_watch_only` |
| QT-116 | automatic backups, "Show Automatic Backups" | R2 + V1 | `automatic_backups`, `backup_policy`, `Notice{BackupFailed}` |
| QT-117 | rescan birthday/full with progress + cancel | R1 + V1 | `rescan` (M1), `rescan_progress`, `cancel_rescan` |
| QT-135…141 | Options dialog, tabs, appearance, reset | V1 + S1 | `OptionsProviding` (Swift); dust/backup/fee settings via engine calls |
| QT-143 | Information tab | R1 + V1 | `node_info` |
| QT-145 | console (local) | R1 + V1 | `console_execute`, `console_commands`, `console_redact` |
| QT-147 | peers table, ban/unban | R1 + V1 | `peers` (M1), `disconnect_peer`, `ban_peer`, `unban_peer`, `banned_peers` |
| QT-148 | repair: rescan / reset chain data | R1 + V1 | `rescan`, `reset_chain_data` |
| QT-150 | OS URI registration, Open URI dialog | S1 + V1 | `register_uri_schemes`; Info.plist / `.desktop` / WiX |
| QT-153 | help, about, CoinJoin info | V1 | `core_version` (M1), `LaunchOptions.helpText` |
| IOS-005 | backup reminder 24 h after first funds | V1 | `BackupReminderTracking` (Swift) |
| IOS-006 | screen-capture guard on Windows/Linux | S1 | `set_window_capture_excluded`; Linux banner |
| IOS-009 | existing-wallet detection, Keep / Delete All | V1 + R1 + S1 | `Engine.existing_networks`, `Vault.destroy` |
| IOS-011 | biometric enrollment, default 0.5 DASH limit | S1 + V1 | `enroll_quick_unlock` (M1 stub), `quick_unlock_policy`, `BiometricKeyStoring` |
| IOS-014 | forgot passphrase (passphrase model) | V1 + S1 | `Vault.recover_with_mnemonic` |
| IOS-015 | auto-lock timer | S1 + V1 | `AutoLockControlling` (Swift) → `Vault.lock` (M1) |
| IOS-016 | spending confirmation + biometric limit, enforced in Rust | S1 + V1 | `set_quick_unlock_spend_limit`, `vault.quick_unlock_limit_exceeded` |
| IOS-025 | shortcut bar | V1 | Swift only |
| IOS-027 | history by day | V1 | `history_page` (M1) |
| IOS-028 | history filters | V1 | `HistoryFilter.categories` (M1) |
| IOS-029 | rich rows | V1 | `TxRecord` (M1); merchant/contact icons M4/M5 |
| IOS-030 | grouped CoinJoin rows | V1 + R1 | `TxType` CoinJoin values (§5) |
| IOS-031 | tx details | V1 + R1 | `tx_detail` + `tx_detail_extras`; Insight fee lookup is M5 |
| IOS-032 | detail actions: copy txid, explorer, raw hex | V1 | `TxDetail.raw_hex` (M1); explorer URLs are settings |
| IOS-034 | remove unconfirmed + drop & rescan | R1 + V1 | `drop_unconfirmed`, `abandon_transaction` |
| IOS-043 | QR from image file / clipboard (webcam M6) | S1 | `decode_qr_codes`; `QRImageDecoding` |
| IOS-048 | URI OS registration | S1 | as QT-150; parser done in M1 |
| IOS-104 | local currency list (rates M5) | V1 | Swift only |
| IOS-105 | notifications toggle reflecting OS permission | S1 + V1 | `SystemNotifying.authorization` |
| IOS-107 | About + tech info | V1 | `node_info`, `core_version` |
| IOS-108 | security menu | V1 | M1 vault calls + quick unlock |
| IOS-109 | wipe with phrase confirmation | V1 + S1 | `remove_wallet` (M1) per wallet, then `Vault.destroy` |
| IOS-110 | multi-wallet management | V1 + R1 | as QT-101 + `rename_wallet`/`remove_wallet` (M1) |
| IOS-111 | xpub export | R1 + V1 | `account_xpub` |
| IOS-112 | export logs | S1 + V1 | `Engine.export_logs` |
| IOS-113 | sync info, rescan options, birth height, drop unconfirmed | R1 + V1 | `rescan`, `rescan_progress`, `set_birth_height`, `drop_unconfirmed` |
| IOS-116 | local notifications (tx) + deep links | S1 + R1 | as QT-031; `DesktopNotifier` |
| IOS-117 | tray companion | S1 + V1 | `TrayIcon`; macOS `MenuBarExtra` (M1 partial) |
| IOS-121 | testnet faucet shortcut | V1 | web faucet link only; the in-app PoW faucet is AppServices work (M5), shown as unavailable until then |

## 2. Calls

Status of every call below: **stub** (typed `NotImplemented` after the argument and session checks), except
where noted.

### 2.1 Wallet lifecycle (`multiwallet.rs`) — owner R1

| Call | Kind | Semantics | Errors (besides common) | Serves |
|---|---|---|---|---|
| `Engine.existing_networks()` | async | `[NetworkDataInfo { network, directory, has_wallet_state, has_vault, has_os_store_key }]` for every network with data under the root. Reads files and the OS store; opens nothing. | — | IOS-009 |
| `NetworkSession.wallet_load_states()` | sync | `[WalletLoadState { wallet_id, name, loaded, load_on_startup, watch_only }]`, creation order, in memory. | — | QT-101, IOS-110 |
| `load_wallet(id)` / `unload_wallet(id)` | async | dash-qt Open / Close Wallet. Unload stops tracking, drops the wallet from memory, abandons its unsent `PreparedTx`s and keeps its data. An unloaded wallet is absent from `wallet_infos` and wallet-scoped calls return `wallet_not_found` for it. Load resumes the scan from its last processed height. Both idempotent; emit `WalletLoadChanged`. | `wallet_not_found` (not registered) | QT-101 |
| `set_load_on_startup(id, bool)` | async | dash-qt's settings.json `"wallet"` list, kept in dw-appdb. The host follows dash-qt: Create/Open/Restore add, Close removes. A new network session loads only listed wallets; with an empty list (first M2 start), every wallet. | — | QT-101 |
| `import_watch_only(xpub, WatchOnlyOptions{name, birth_height, lookahead})` | async | Registers a BIP44-account watch-only wallet with no vault record (interim for upstream U7, DESIGN-opus §2): the registration platform-wallet stores for any wallet (metadata, account xpub, address pools), then a load from wallet.sqlite. `watch_only = true`; sends fail with `send.watch_only`; `TxDraft.create_unsigned` works. The id is key-wallet's network-scoped digest of the account key (a seed wallet's id digests its root key), so importing the matching phrase later adds a second wallet instead of attaching keys. | `wallet.invalid_xpub`, `wallet.already_exists`, `wallet.name_rejected`, `invalid_argument` (lookahead) | QT-114, QT-077 |
| `account_xpub(id, account)` | async | `AccountXpub { account, derivation_path, xpub }` (`tpub` off mainnet). Public data: no grant. Watch-only wallets return their imported key. | `invalid_argument` (account not derived) | IOS-111 |

### 2.2 Transaction actions and exports (`tx_actions.rs`) — owner R1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `tx_detail_extras(id, txid)` | async | `TxDetailExtras { is_coinbase, total_credit, total_debit, net, matures_in, in_mempool, abandoned, can_abandon, can_resend, dust_locked_outputs, last_announced_at }`. `in_mempool` is `None` unless a peer announced the tx since start: SPV sees no mempool (§6). | `history.tx_not_found` | QT-075, QT-091, QT-092, IOS-031 |
| `abandon_transaction(id, txid)` | async | dash-qt Abandon: allowed when not abandoned, unconfirmed, not InstantSend-locked and not coinbase (SPV cannot know mempool membership, so `InMempool` is never the refusal). The transaction and its recorded descendants leave key-wallet and wallet.sqlite (persisted as a sweep that releases every input) and show as `Abandoned` (amount in brackets, kept in app.sqlite); their change leaves the balances. The coins they spent come back through a filter rescan from the coins' blocks, which needs SPV running. If it confirms later anyway, it shows as confirmed again. Emits `HistoryChanged`, `Balances`. | `history.tx_not_found` (as `tx_action.tx_not_found`), `tx_action.refused{refusal}` | QT-091, IOS-034 |
| `resend_transaction(id, txid)` | async | dash-qt Resend: hands the stored tx to dash-spv again. Allowed when unconfirmed, not abandoned, not coinbase, not IS-locked and funded by the wallet. Returns after a 2 s hand-off window (`no_peers` when dash-spv refused to send); there is no verdict. dash-spv sends a txid once per session (its own timer rebroadcasts), and platform-wallet already re-sends unconfirmed own transactions when a session loads. | `tx_action.refused{refusal}`, `tx_action.spv_not_running`, `tx_action.no_peers` | QT-091 |
| `drop_unconfirmed(id?)` | async | iOS bulk "remove unconfirmed": abandons every eligible unconfirmed tx of the wallet (or of all wallets), then schedules a rescan from the earliest first-seen height. Returns the count. | `tx_action.spv_not_running` (the rescan) | IOS-034, IOS-113 |
| `export_history_csv(id, filter, sort, unit, type_names, utc_offset_secs)` | async | dash-qt §4.7 bytes: columns `Confirmed`, `Watch-only` (watch-only wallets only), `Date` (`yyyy-MM-ddTHH:mm:ss` at `utc_offset_secs`), `Type`, `Label`, `Address`, `Amount (<unit name>)`, `ID`. Every field quoted, `"` doubled, `,` separators, `\n` line ends, signed amounts with no separators. `type_names` = 19 localized `TxType` strings in enum order; empty = dash-qt English. Golden vectors: `testdata/csv_export.json` (R1 adds them). | `history.invalid_query` (filter, or `type_names` count not 0 or 19) | QT-093 |
| `tx_notices(id, txids)` | async | `[TxNotice { txid, record_index, amount, timestamp, tx_type, address, label, coinjoin_internal }]` for a `NewTransactions` event: one per dash-qt record. Unknown txids are skipped. | — | QT-031…033, IOS-116 |

### 2.3 Fees and coin control (`fees.rs`) — owner R1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `fee_policy()` | sync | `FeePolicy { source: MinimumRelay, min_relay_per_kb: 1000, max_custom_per_kb: 10_000_000, max_tx_fee: 10_000_000, max_broadcast_rate_per_kb: 10_000_000, targets }` with dash-qt's targets 2/4/6/12/24/48/144/504/1008 (§6). | — (`SendError` domain) | QT-057, QT-058 |
| `coin_selection_summary(id, outpoints, pay_amounts, fee, all_change_to_fee)` | async | dash-qt panel values: `quantity, amount, bytes (148·in + 34·(out+1) + 10, −34 without change), fee, after_fee, change, change_to_fee, insufficient_funds, fee_tolerance_per_input, unavailable`. `unavailable` lists chosen coins that are spent or gone, for QT-074. Reads coins only. | `invalid_argument` (outpoint) | QT-072, QT-074 |

### 2.4 Tools window (`tools.rs`) — owner R1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `node_info()` | sync | `NodeInfo { client_version, user_agent, data_dir, startup_time, network, connections_in, connections_out, local_addresses (empty), tip_height/time/hash, best_chainlock, masternodes, evonodes, mempool_tx_count: None, mempool_usage_bytes: None }`. MN counts come from the SML and are `None` before the masternode phase finishes. `tip_hash` and `best_chainlock.block_time` are `None` (dash-spv exposes neither through platform-wallet). The user agent is the one the SPV client now announces (`/dashwallet-desktop:<version>/`). | — | QT-143, IOS-107 |
| `warnings()` | sync | `[EngineWarning { code: PrereleaseBuild\|UncleanShutdown\|SyncStalled\|ClockSkew\|PlatformContextUnavailable, detail }]`, most severe first. Re-query on `Notice` and `Sync`. `UncleanShutdown`: the previous session left its open-session marker. `ClockSkew` is never sent: dash-spv does not expose peer time offsets. | — | QT-040 |
| `disconnect_peer(address)` | async | Drops one peer; dash-spv may dial a replacement. Needs the dash-spv network-manager accessors (upstream U2; `ban_peer`, `unban_peer` and `disconnect_peer` exist there). **`NotImplemented` until U2** (with `ban_peer`, `unban_peer`, `banned_peers`). | `sync.peer_not_found`, `sync.spv_not_running` | QT-147 |
| `ban_peer(address, duration_secs)` / `unban_peer(subnet)` / `banned_peers()` | async | Ban list persisted with the SPV data; `BannedPeer { subnet, banned_until }`. | `sync.peer_not_found`, `invalid_argument` | QT-147 |
| `rescan_progress()` | sync | `Option<RescanProgress { from_height, current_height, target_height, started_at }>`. | — | QT-117, IOS-113 |
| `cancel_rescan()` | async | `abortrescan`: stops at the current height and keeps what was found. `false` when none ran. | — | QT-117 |
| `reset_chain_data()` | async | QT-148 "Rebuild Index" equivalent: deletes `spv/` and keeps wallets, vault and app data. Needs SPV stopped. The next `start_spv` resyncs and rescans from each birth height. | `sync.spv_running` | QT-148 |
| `set_birth_height(id, height)` | async | Stores a new birth height. If it is lower than the old one, a rescan from it is scheduled. | `sync.height_out_of_range` | IOS-113 |

Changed M1 behaviour: `rescan` returns `sync.rescan_in_progress` while a rescan runs ("Wallet is currently
rescanning"); `peers()` fills more `PeerInfo` fields once U2 lands (until then only addresses, as in M1). A
rescan from below a wallet's birth height lowers (and stores) the birth height to the rescan start, because
dash-spv never scans below it; dash-qt's `rescanblockchain` scans from any height. The height checks of `rescan`
and `set_birth_height` use the best known height (SPV tip or the wallets' processed heights); with none known,
`rescan` refuses any height above 0 (as in M1) and `set_birth_height` skips the check.

### 2.5 Console (`console.rs`) — owner R1 (`dw-console`)

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `console_commands()` | sync, free | `[ConsoleCommand { name, category, sensitive, available }]` sorted by name: about 80 Core names (DESIGN-opus §1.14) plus `help`, `help-console`. Unknown or unavailable ones answer `console.not_available`. In M2, 31 are available: wallet info/balances/addresses/labels, `listunspent`/`lockunspent`/`listlockunspent`, `listtransactions`/`gettransaction`, `sendtoaddress`, `signmessage`/`verifymessage`/`validateaddress`, `abandontransaction`, `rescanblockchain` (no stop height)/`abortrescan`, `listwallets`/`loadwallet`/`unloadwallet`, `walletpassphrase`/`walletlock`, `getblockcount`, `getbestchainlock`, `getconnectioncount`, `getpeerinfo`, `getnetworkinfo`, `uptime`, `help`, `help-console`. Results carry only fields an SPV wallet knows. | — | QT-145 |
| `console_redact(line)` | sync, free | dash-qt history redaction: the arguments of `importprivkey`, `importmulti`, `sethdseed`, `signmessagewithprivkey`, `signrawtransactionwithkey`, `upgradetohd`, `walletpassphrase`, `walletpassphrasechange` and `encryptwallet` become `(…)`. The host echoes and stores only this text. `line` is bytes and is zeroized. | `console.parse_error` | QT-145 |
| `NetworkSession.console_execute(wallet_id?, line, grant_id?)` | async | dash-qt grammar: whitespace or comma separators, quotes, nested calls, `[key]`/`[0]` indexing. Runs on engine calls with Core names and result shapes. `ConsoleOutput { text, is_json }` (JSON with 2-space indent). Commands that spend, sign or reveal first fail with `console.authorization_required{purpose, wallet_id}`; the host authorizes and repeats the line with the grant. `walletpassphrase` maps to `Vault.unlock`, `walletlock` to `Vault.lock`. M3 adds the governance, masternode and CoinJoin commands. | `console.parse_error`, `console.rpc_error{code, message}` ("message (code N)"), `console.not_available{command}`, `console.authorization_required`, `console.wallet_required` | QT-145 |

Console output is RPC data in Core's English, as dashd prints it. It is the one Rust-produced text the host
shows as is. Error copy still comes from the code.

### 2.6 Compatibility import/export (`compat.rs`) — owner R2

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `Engine.inspect_wallet_file(path)` | async | `WalletFileKind`: `DumpWallet{network, has_mnemonic, has_hd_seed, has_xprv, loose_key_count, script_count, label_count}`, `WalletDatSqlite{encrypted, has_mnemonic}`, `WalletDatBdb{encrypted}`, `DwBackup{network, wallet_count, created_at, format_version}`, `Psbt`, `Unknown`. Detects by content and decrypts nothing. | `compat.file_unreadable` | QT-106, QT-107, QT-110 |
| `import_dump_wallet(path, ImportOptions)` | async | Rebuilds HD from the header: mnemonic + passphrase (Core quirks), else HD seed, else xprv. The chain counters raise the lookahead. Labels go to the address book. Loose WIF keys and scripts are counted in the report, not imported (sweep is M5). Vault seed-safety order as `import_wallet`. `ImportReport { wallet_id, labels_imported, keys_not_imported, scripts_not_imported, core_compat_seed }`. | `compat.unsupported_format`, `compat.corrupt`, `compat.no_hd_chain`, `compat.network_mismatch`, `compat.already_exists`, `compat.no_vault`, `compat.vault_locked` | QT-107 |
| `import_wallet_dat(path, wallet_passphrase?, ImportOptions)` | async | SQLite descriptor wallet.dat: reads `main` and decrypts `walletdescriptorckey` with `mkey` and the passphrase to get mnemonic and passphrase. **BDB returns `NotImplemented{call: "import_wallet_dat.bdb"}` until M6.** | as above + `compat.passphrase_required`, `compat.wrong_passphrase` | QT-106 |
| `import_key_material(KeyMaterial, ImportOptions)` | async | `HdSeed{seed}` (16..=64 bytes), `Xprv{xprv}`, `Descriptors{json}` (`listdescriptors true`, BIP44 account 0 of one master key). The result has `has_mnemonic = false`. Bytes are zeroized. | `compat.invalid_key_material`, `compat.network_mismatch`, `compat.already_exists`, `compat.no_vault`, `compat.vault_locked` | QT-108 |
| `export_for_core(id, CoreExportFormat, dest_path, grant_id)` | async | `DumpWallet` (Core's format byte for byte) or `ImportDescriptorsJson`. Rust writes the file, mode 0600, and never replaces an existing file. Needs a `RevealSecret` grant for the wallet, redeemed after the checks. `ExportReport { path, format, key_count, warnings }`. | `compat.grant_invalid`, `compat.watch_only`, `compat.destination_unwritable`, `compat.vault_locked` | QT-109 |
| `core_mnemonic_compatibility(id)` | async | `{ core_compatible, warnings }`: whether "phrase + passphrase + `upgradetohd`" rebuilds this wallet in dash-qt (English phrase that passes Core's check). The phrase itself comes from `Vault.reveal_mnemonic`. | `compat.watch_only` | QT-109 |

Warnings: `MnemonicNotCoreCompatible`, `CoinJoinAccountNotScannedByLegacyCore` (the DIP9 account; DESIGN.md R2).
Release-blocking checks are DESIGN-opus §4.3 `restore` and §4.4 items 1–3, in `regtest/` (R2).

### 2.7 Backups (`backup.rs`) — owner R2

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `backup_wallet(id, dest_path, backup_passphrase?)` | async | Writes a `.dwbackup` (vault records still under the data key, a passphrase wrap slot, the wallet's app.sqlite rows, an online backup of wallet.sqlite, MAC under the data key). Encrypted vault: uses the vault passphrase slot, needs the vault unlocked, `backup_passphrase` must be `None`. Unencrypted vault: `backup_passphrase` is required. Replaces no file. `BackupInfo { path, wallet_id, created_at, size_bytes, automatic }`. | `backup.vault_locked`, `backup.passphrase_required`, `backup.destination_unwritable`, `invalid_argument` | QT-110 |
| `restore_backup(path, passphrase?)` | async | Verifies the MAC, re-encrypts the records under this vault's key (vault unlocked) and registers the wallets in seed-safety order. Returns their ids. | `backup.wrong_passphrase`, `backup.corrupt`, `backup.unsupported_version`, `backup.network_mismatch`, `backup.already_exists`, `backup.vault_locked` | QT-110 |
| `automatic_backups(id?)` | async | Newest first. The engine writes `backups/<wallet>.YYYY-MM-DD-HH-MM.dwbackup` when a wallet loads or is created and the data key is available (vault unlocked or unencrypted), keeps the newest `keep`, and sends `Notice{BackupFailed}` on failure. CoinJoin is not gated on backups (dash-qt quirk #10). | — | QT-116 |
| `backup_policy()` / `set_backup_policy(keep)` | sync / async | `BackupPolicy { keep (0..=10, default 10), directory }`. Lowering `keep` deletes the oldest automatic backups. | `invalid_argument` | QT-116 |

### 2.8 PSBT (`psbt.rs`) — owner R2 (`dw-psbt`)

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `TxDraft.create_unsigned()` | async | dash-qt "Create Unsigned": plans as `estimate` does and returns a PSBT with UTXO and derivation data. Signs and reserves nothing, needs no grant, works for watch-only wallets. The host copies `to_base64()` to the clipboard and offers Save (`to_bytes()`). | `SendError` codes of `estimate` | QT-076, QT-077 |
| `parse_psbt(data)` | sync, free | Binary or base64, at most 100 MiB. **Works now** for the size check. | `psbt.too_large`, `psbt.invalid` | QT-078 |
| `Psbt.to_base64()` / `to_bytes()` / `unsigned_txid()` | sync | Encodings. | — | QT-077…079 |
| `NetworkSession.analyze_psbt(wallet_id?, psbt)` | async | `PsbtAnalysis { outputs[{address, amount, is_mine}], fee, total, unsigned_inputs, status: MissingInputInfo\|NeedsSignatures\|Complete, signability: NoWallet\|WatchOnly\|NoMatchingKeys\|CanSign, external_sent }` (dash-qt's dialog lines). | `psbt.network_mismatch` | QT-079 |
| `sign_psbt(id, psbt, grant_id)` | async | Signs the wallet's inputs through `VaultSigner` and returns a new `Psbt`. Needs a `Spend{max_duffs}` grant for the wallet with `max_duffs ≥ external_sent`, redeemed after the checks. | `psbt.watch_only`, `psbt.vault_locked`, `psbt.grant_invalid`, `psbt.grant_exceeded{max_duffs}` | QT-079 |
| `broadcast_psbt(psbt)` | async | Finalizes, refuses rates above 0.1 DASH/kB and broadcasts with `TxDraft.broadcast`'s verdict rules (m1-engine.md §2.7.1). Returns the txid. | `psbt.not_complete`, `psbt.fee_rate_too_high`, `psbt.no_peers`, `psbt.broadcast_rejected{reason}`, `psbt.broadcast_unknown{reason}` | QT-079 |

### 2.9 Vault additions (`vault_m2.rs`, `vault.rs`) — owner S1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `Vault.enroll_quick_unlock(grant_id)` (M1 signature) | async | Adds slot B and returns the wrap key, which the host stores in the OS biometric store (macOS: keychain item with `.biometryCurrentSet`). `ChangeCredential` grant issued with the passphrase. Linux: `vault.quick_unlock_unavailable`. | `vault.quick_unlock_unavailable`, `vault.grant_invalid`, `vault.not_encrypted` | IOS-011 |
| `Vault.authorize(…, QuickUnlock{wrap_key})` (M1) | async | Now accepted: issues `Spend` grants up to the spending limit and `SignMessage` grants. Refuses `RevealSecret`, `ChangeCredential` and `Wipe` (`vault.credential_required`), any spend above the limit (`vault.quick_unlock_limit_exceeded{limit_duffs}`), and any request once the passphrase is older than `passphrase_max_age_secs` (`vault.passphrase_stale`). `Vault.unlock` with quick unlock is a host flow: authorize first, then unlock with the passphrase only when needed. | as listed | IOS-011, IOS-016 |
| `Vault.quick_unlock_policy()` | sync | `QuickUnlockPolicy { enrolled, spend_limit_duffs (default 50_000_000), passphrase_max_age_secs (604_800), last_passphrase_at }`. | — | IOS-016 |
| `Vault.set_quick_unlock_spend_limit(grant_id, duffs)` | async | One of 0, 10_000_000, 50_000_000, 100_000_000, 500_000_000. `ChangeCredential` grant. | `invalid_argument`, `vault.grant_invalid` | IOS-016 |
| `Vault.recover_with_mnemonic(wallet_id, mnemonic, bip39_passphrase, new_passphrase)` | async | Forgot passphrase (DESIGN-opus §1.8). Checks that the phrase derives `wallet_id`, then replaces the vault with a new one encrypted with `new_passphrase` and holding that phrase. The throttle resets and slot B is removed. Other wallets' secrets are dropped and listed in `wallets_without_secrets` (they become watch-only until their phrases are imported). | `vault.recovery_mismatch`, `vault.passphrase_rejected`, `wallet_not_found` | IOS-014 |
| `Vault.destroy(credential)` | async | Deletes the vault files, the OS-store key and the quick-unlock item once no registered wallet has secrets (`vault.not_empty`). This is the last step of "Delete All" / wipe, after `remove_wallet` for each wallet. Credential rule: the `Wipe` row of m1-engine.md §2.2. Returns the `NoVault` status. | `vault.not_empty`, `vault.credential_required`, `vault.wrong_passphrase`, `vault.throttled` | IOS-009, IOS-109 |

### 2.10 Desktop OS services (`desktop.rs`, crate `dw-desktop`) — owner S1

The surface is the same on every OS, so the bindings do not depend on the build host. On macOS the Swift
implementations in `PlatformServicesMac` serve the same protocols, and these calls return `desktop.unsupported`
unless noted.

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `acquire_single_instance(key, InstanceObserver)` | sync, free | `Some(InstanceGuard)` = primary; it delivers later launches' args to `on_forwarded`. `None` = another instance holds `key` (`DashWallet-<network>`): call `forward_to_primary(key, args)` and exit 0. Windows: named pipe; Linux: abstract Unix socket. | `desktop.os_error` | QT-001 |
| `forward_to_primary(key, args)` | sync, free | `false` when no primary listens. | `desktop.os_error` | QT-001 |
| `register_uri_schemes(app_id, exec_path, schemes)` | sync, free | Per-user registration for the Linux tarball and unpackaged Windows builds. Packages register at install time. | `desktop.os_error` | QT-150, IOS-048 |
| `autostart_enabled(app_id)` / `set_autostart(AutostartEntry, enabled)` | sync, free | Windows Startup shortcut; Linux XDG autostart (Flatpak: Background portal), launched with `--min --network=<net>`. macOS: hidden (dash-qt). | `desktop.unsupported`, `desktop.os_error` | QT-009 |
| `TrayIcon(TraySpec, TrayObserver)`, `set_tooltip`, `set_items`, `set_visible` | object | Windows `Shell_NotifyIcon` thread; Linux StatusNotifierItem (`ksni`). `desktop.unsupported` when the session has no tray host (GNOME without AppIndicator, disclosed in Options). | `desktop.unsupported` | QT-028…030, IOS-117 |
| `DesktopNotifier(app_id, NotificationObserver)`, `notify(DesktopNotification)` | object | WinRT toast / D-Bus `org.freedesktop.Notifications` / Flatpak portal; clicks come back with `deep_link`. | `desktop.unsupported`, `desktop.os_error` | QT-031, IOS-116 |
| `set_window_capture_excluded(hwnd, excluded)` | sync, free | Windows `WDA_EXCLUDEFROMCAPTURE`; returns whether it took effect. Linux: `desktop.unsupported` (the host shows a warning banner). | `desktop.unsupported` | IOS-006 |
| `decode_qr_codes(image)` | sync, free | Every QR code in a PNG/JPEG/BMP (file or clipboard), in reading order. Pure Rust; used on all OSes. | `desktop.no_qr_code`, `desktop.image_unreadable` | IOS-043 |
| `desktop_quick_unlock_provider()` | sync, free | `TouchId` / `WindowsHello` / `Unavailable`. **Works**: returns `Unavailable` until Windows Hello (M6). Touch ID is Swift's. | — | IOS-011 |
| `windows_hello_wrap_key(challenge)` | sync, free | Windows Hello signature over the vault challenge → HKDF wrap key. **M6**: `NotImplemented`. | — | IOS-011 |
| `Engine.export_logs(dest_path, extra_files)` | async | Zips the Rust logs of every network plus the Swift log files into `dest_path`. `LogExport { path, file_count, size_bytes }`. | `desktop.os_error` | IOS-112 |

## 3. Events (additions to `EngineEvent`)

| Event | When | Host reaction | Emitter |
|---|---|---|---|
| `NewTransactions { network, wallet_id, txids, catch_up }` | Transactions seen for the first time, batched over 100 ms (dash-qt). Never for status changes. `catch_up` = SPV not caught up (dash-qt shows nothing during initial sync; iOS sends one catch-up summary). | `tx_notices` → notifier: ≥ 100 rows in a batch become one summary ("Received and sent multiple transactions"); CoinJoin-internal rows are hidden unless "show CoinJoin popups" is on | R1 |
| `WalletLoadChanged { network, wallet_id, loaded }` | `load_wallet` / `unload_wallet` | reload wallet list and load states | R1 |

DashKit maps both. They are lifecycle events (queued, never dropped) because notifications cannot be
re-queried. `Notice{BackupFailed}` is now sent (§2.7).

## 4. Error codes

Common codes are unchanged (m1-engine.md §4). `DesktopError` has only `invalid_argument`, `not_implemented`
and `internal` of them. Domain codes (a dw-ffi test compares these rows with the enums):

| Domain enum | Codes |
|---|---|
| `TxActionError` | `tx_action.tx_not_found`, `tx_action.refused`, `tx_action.spv_not_running`, `tx_action.no_peers` |
| `ConsoleError` | `console.parse_error`, `console.rpc_error`, `console.not_available`, `console.authorization_required`, `console.wallet_required` |
| `CompatError` | `compat.file_unreadable`, `compat.unsupported_format`, `compat.corrupt`, `compat.passphrase_required`, `compat.wrong_passphrase`, `compat.no_hd_chain`, `compat.network_mismatch`, `compat.invalid_key_material`, `compat.already_exists`, `compat.no_vault`, `compat.vault_locked`, `compat.grant_invalid`, `compat.watch_only`, `compat.destination_unwritable` |
| `BackupError` | `backup.vault_locked`, `backup.passphrase_required`, `backup.wrong_passphrase`, `backup.corrupt`, `backup.unsupported_version`, `backup.network_mismatch`, `backup.already_exists`, `backup.destination_unwritable` |
| `PsbtError` | `psbt.invalid`, `psbt.too_large`, `psbt.network_mismatch`, `psbt.not_complete`, `psbt.fee_rate_too_high`, `psbt.watch_only`, `psbt.vault_locked`, `psbt.grant_invalid`, `psbt.grant_exceeded`, `psbt.no_peers`, `psbt.broadcast_rejected`, `psbt.broadcast_unknown` |
| `DesktopError` | `desktop.unsupported`, `desktop.os_error`, `desktop.no_qr_code`, `desktop.image_unreadable` |
| `VaultError` (M2 additions) | `vault.quick_unlock_limit_exceeded`, `vault.passphrase_stale`, `vault.not_empty`, `vault.recovery_mismatch` |
| `WalletError` (M2 additions) | `wallet.invalid_xpub` |
| `SyncError` (M2 additions) | `sync.spv_running`, `sync.rescan_in_progress`, `sync.peer_not_found` |

Calls in `fees.rs` use `SendError` (`fee_policy`) and `CoinsError` (`coin_selection_summary`). `tx_detail_extras`,
`export_history_csv` and `tx_notices` use `HistoryError`. `TxDraft.create_unsigned` uses `SendError`. None of
them adds a code. Parameters that the UI shows (review M-5 rule): `limit_duffs`, `max_duffs`, `size_bytes`,
`duffs_per_kb`, `version`, and `code` of `console.rpc_error`. DashKit forwards them in `ServiceError.parameters`.

## 5. Changes to M1 behaviour

- **History (R1).** `history_page` produces the remaining dash-qt types and statuses: `Abandoned` (from
  `abandon_transaction`), `Conflicted`, `CoinJoinSend` (the send flow's `DS=1` mark), `DustReceive` (dust
  protection) and the CoinJoin internal types. `TxRecord.involves_watch_only` is true for every record of a
  watch-only wallet. Balances of watch-only wallets are computed like any other's: the host shows them in
  dash-qt's watch-only column (QT-035). dash-qt mixes spendable and watch-only scripts in one wallet; we do not,
  so the two columns never both have values.
- **Wallet registry (R1).** `wallet_infos` lists loaded wallets only (§2.1). `import_wallet` over a watch-only
  wallet with the same id attaches the keys and emits `WalletCreated` (M1 already reserved this path).
- **Rescan (R1).** `rescan` returns `sync.rescan_in_progress` while a rescan runs.
- **Vault (S1).** `VaultCredential::QuickUnlock` is accepted (§2.9); `enroll_quick_unlock` stops returning
  `NotImplemented` on macOS.
- **Notices.** `BackupFailed` is sent by the backup rotation (R2).

## 6. Full-node-only features (DESIGN-opus §1.14)

| Feature | M2 answer |
|---|---|
| Fee estimation | `fee_policy().source = MinimumRelay`: every target is 1000 duff/kB. The host keeps dash-qt's target list and says all targets use the minimum relay fee. `NodeEstimate` waits for the dashd RPC data source (post-M2). |
| Mempool size, InstantSend counters, credit pool, quorum health (Information tab) | `None` in `NodeInfo`. The host shows "—" with "Requires full-node data source". |
| "Not in memory pool" (abandon) | `in_mempool` is `None`; abandon is allowed without proof and the host warns that the transaction may still confirm. |
| Rebuild index | `reset_chain_data`. |
| Prune, dbcache, par, RPC server, UPnP/NAT-PMP, listen | Not offered. The Options dialog shows a "Not applicable to this wallet (SPV)" footnote. |
| Proxy / Tor | Shown, disabled with "requires engine update" until U1 (M6). The numeric-IP validation is still V1's. |
| Network traffic graph | M6 (U2). |
| Console commands without an engine equivalent | `console.not_available` ("Not available in SPV mode."). |

## 7. Open points routed to owners

- **R1**: the peer ban/disconnect calls need dash-spv's network-manager accessors exposed through
  platform-wallet's `SpvRuntime` (U2, small). Until then they stay `NotImplemented`; `peers()` keeps M1 behaviour.
- **R1**: the watch-only wallet needs a key-wallet account built from an xpub. If platform-wallet cannot register
  one without a seed, fall back to U7 and keep `import_watch_only` `NotImplemented`. The vault must not hold a
  fake seed.
- **R2**: `.dwbackup` format version 1 is specified in the R2 PR (`docs/contracts/dwbackup-v1.md`) before any
  writer lands.
- **S1**: decide whether quick unlock on Windows ships in M2 (DESIGN-opus §5.1 says M6) and keep
  `desktop_quick_unlock_provider` truthful either way.
- **S1**: the DashKit `EngineProtocol` grows by these calls in S1's adapter PRs, one domain per PR. `FakeEngine`
  and `WalletDemo.DemoEngine` follow the engine's rules for each call they implement (no fake success).
