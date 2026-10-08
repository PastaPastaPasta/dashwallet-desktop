# dashwallet-desktop — Architecture & Delivery Design (Opus)

Status: proposal for reconciliation. Written 2026-10-05 against the four research reports in `docs/research/`
and code at dashpay/platform `v5.0-dev@bc321362b9` (pinning rust-dashcore `e4208c90`), Dash Core
`develop@3789ec0c719e`, dashwallet-ios `develop@7c0064d6b2`, DashUIKit `e8d9243`.

Claims marked **(verified)** were checked in code for this document. Claims marked **UNVERIFIED** were not.

---

## 0. Decisions at a glance

| # | Decision | One-line justification |
|---|---|---|
| D1 | **One Rust engine crate stack of our own, built directly on the `platform-wallet` Rust library**, with no `platform-wallet-ffi`, no `rs-unified-sdk-ffi`, and no `SwiftDashSDK`. | The FFI handle store is hard-wired to `PlatformWalletManager<FFIPersister>` (verified, `rs-platform-wallet-ffi/src/handle.rs:150`). That forces persistence through the 46-slot C vtable and rules out the Rust SQLite persister. The Rust library is generic over the persister (`PlatformWalletManager<P: PlatformWalletPersistence>`, verified, `manager/mod.rs:330`). dash-evo-tool already consumes it this way (research 04). |
| D2 | **Persistence is entirely in Rust**: `platform-wallet-storage::SqlitePersister` for wallet state, plus our own `app.sqlite` for app metadata. **Swift links no SQLite at all.** | SqlitePersister implements the persistence trait (verified, `persister.rs:1301`) and runs on all three OSes. It removes the 11.6k-line SwiftData handler problem completely. |
| D3 | **The binding is UniFFI** (proc-macro) over a coarse-grained façade crate `dw-ffi`. Its output is one static library, `libdashwallet_core`, delivered as an SE-0482 artifact bundle on **all three OSes, macOS included**. | Typed records, enums, errors, async functions and callback interfaces, with no hand-written marshalling. Agents cannot write memory bugs at the boundary. The same generated Swift runs everywhere. The shape still matches iOS: Rust engine → C ABI → Swift SDK layer → adapter → view models. |
| D4 | **Secrets never live in Swift.** A Rust vault (`dw-vault`) holds the seed under a DEK. The DEK is wrapped by an Argon2id passphrase key (dash-qt parity), and optionally also by an OS-held key for biometric quick unlock. A Rust `key_wallet::signer::Signer` implementation signs inside Rust. | `platform-wallet` signs through the pluggable `key_wallet::signer::Signer` trait (verified, `key-wallet/src/signer.rs:82`), so no mnemonic resolver needs to cross the FFI. Rust also enforces auth grants, mixing-only mode and spend limits. |
| D5 | **UI = Option B.** Native SwiftUI on macOS. SwiftCrossUI on Linux (GtkBackend) and Windows (WinUIBackend, gated). Both share **one** `@Observable` view-model layer. | This is the proven probe shape (research 04 §2). It gives the best macOS app and lets DashUIKit be reused directly on macOS. |
| D6 | **The Windows fallback is SwiftCrossUI GtkBackend, not egui.** | An egui fallback would fork the view-model layer and break "built like iOS". GTK-on-Windows keeps every line of shared code. |
| D7 | **CoinJoin and governance get their own P2P sessions** (crate `dw-p2p`, using the `dashcore` message codec). They do not ride on dash-spv's connection pool. | The dash-spv `NetworkManager` is `pub(super)` inside the client (verified, `client/core.rs:108`). CoinJoin needs direct masternode connections anyway. Keeping these features separate means a mixing bug cannot wedge SPV sync. No upstream patch is needed on the critical path. |
| D8 | **Governance list, votes and tallies are synced over SPV** with `govsync`, and vote signatures are verified against the SPV masternode list. An optional dashd RPC data source only adds PoSe score, last-paid and next-payment, mempool stats and credit-pool stats. | Core serves `MNGOVERNANCESYNC` to any peer once it is synced (verified, `governance/net_governance.cpp:86-135`). The research's "full node in practice" is too pessimistic. |
| D9 | **The RPC console is a local wallet console** (`dw-console`). It uses Core's console syntax and Core command names. Unknown commands can optionally be forwarded to a user-configured dashd. | This handles QT-145 honestly. |
| D10 | **Desktop credential model:** one wallet passphrase is the root credential. Biometric quick unlock is offered where hardware-backed (Touch ID, Windows Hello). A 4–8-digit **app PIN exists only in "unencrypted" mode**, where it is honestly labelled as a UI lock. | A 4-digit PIN cannot protect a seed against offline attack on a desktop. The iOS lockout policy is kept as UX throttling on top of the passphrase prompt. |
| D11 | **One product.** DashPay and Platform are always compiled in and runtime-gated. CoinJoin mixing, governance voting and creation, ProTx tooling, sweep and invitation creation are all IN. CrowdNode is behind a flag. | These are the brief's defaults, accepted. |
| D12 | **Upstream changes are minimised and isolated.** The default is to add capability in our own crates. Only closed types get upstream PRs (proxy support, coin-lock exclusion, rustls on Linux, restore completeness). These are carried as `[patch]` git branches and land in dashpay repos from same-repo branches. | This keeps us buildable against a pinned platform revision. |

---

## 1. Architecture

### 1.1 Layer diagram

```
┌──────────────────────────────── UI (per OS) ─────────────────────────────────┐
│ macOS: MacUI (SwiftUI screens) + DashUIMac (DashUIKit + mac ports)          │
│ Linux/Windows: CrossUI (SwiftCrossUI screens) + DashUICross (DashUIKit spec)│
│   views are dumb: render VM state, forward intents, no SDK/fee/auth logic   │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     │ @Observable, @MainActor
┌────────────────────────────────────▼─────────────────────────────────────────┐
│ WalletFeatures — ViewModels, route enums, formatters, L10n                   │
│ (imports: Foundation, Observation, WalletRuntime, AppServices,               │
│  PlatformServices, DesignTokens)                                             │
└───────────────┬───────────────────────────────────────┬──────────────────────┘
                │ protocols                             │ protocols
┌───────────────▼───────────────────┐   ┌───────────────▼──────────────────────┐
│ WalletRuntime  (≈ iOS             │   │ AppServices (≈ iOS Models/,          │
│ Infrastructure/SwiftDashSDK)      │   │ Infrastructure/)                     │
│ Host, LifecycleQueue, SPVCoord,   │   │ Rates, HTTPClient, BIP70, Explore,   │
│ WalletState, TxSender, CoinJoin-, │   │ Uphold/Coinbase/Topper/SwapKit/Maya, │
│ Governance-, Masternode-,         │   │ CTX/PiggyCards, CrowdNode, ZenLedger,│
│ Identity-, Contacts-, Shielded-,  │   │ Tax/CSV, NotificationDispatcher      │
│ AuthenticationGate                │   │                                      │
└───────────────┬───────────────────┘   └───────────────┬──────────────────────┘
                └───────────────────┬───────────────────┘
┌───────────────────────────────────▼──────────────────────────────────────────┐
│ DashKit — hand-written Swift SDK layer (≈ SwiftDashSDK's role): Amount,      │
│ Network, EngineClient, EventBus (AsyncStream per domain), errors, SecretBytes│
└───────────────────────────────────┬──────────────────────────────────────────┘
┌───────────────────────────────────▼──────────────────────────────────────────┐
│ DashWalletCore — UniFFI-generated Swift (committed, CI-checked)              │
│ DashWalletCoreFFI — C module + libdashwallet_core.{a,lib} (SE-0482 bundle)   │
└───────────────────────────────────┬──────────────────── C ABI ───────────────┘
┌───────────────────────────────────▼──────────────────────────────────────────┐
│ Rust: dw-ffi (UniFFI façade, thin) → dw-engine (orchestration)               │
│   dw-vault  dw-appdb  dw-coinjoin  dw-governance  dw-protx  dw-p2p  dw-compat│
│   dw-uri  dw-message  dw-psbt  dw-sweep  dw-console  dw-chaindata  dw-desktop│
│ ── upstream (pinned git) ──                                                  │
│   platform-wallet  platform-wallet-storage  dash-sdk  dpp                    │
│   dash-spv  key-wallet  key-wallet-manager  dashcore  dashcore-rpc           │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 Language per layer

| Layer | Language | Why |
|---|---|---|
| Engine, gap features, persistence, vault, protocol codecs, console, compat parsers, QR encode/decode, zip | **Rust** | It shares the engine's types and tokio runtime, and it is the only language that can touch `platform-wallet` natively. The protocol code (CoinJoin, governance, ProTx) is consensus-adjacent and needs the same `dashcore` types. |
| SDK layer, adapter, services, view models | **Swift 6.3.3** (Foundation + Observation only) | Same as iOS. It is portable to Linux and Windows, and it runs headless under `swift test` everywhere. |
| macOS views | SwiftUI | Native, accessible, works with XCUITest, and can reuse DashUIKit. |
| Linux/Windows views | SwiftCrossUI 0.10.x (our fork, pinned) | Shares the view-model layer (probe verified). |
| Windows/Linux OS integration that needs WinRT or D-Bus (tray, toasts, Windows Hello, autostart, single-instance IPC, clipboard images, portals) | Rust (`dw-desktop`, behind UniFFI) | Mature crates exist (`ksni`, `notify-rust`, `windows`, `auto-launch`, `interprocess`, `arboard`, `rfd` with the xdg-portal feature). Writing Swift↔WinRT and Swift↔libdbus glue is the riskier path. On macOS these services are native Swift/AppKit. |

### 1.3 Solving "SwiftDashSDK on non-Apple"

**Options considered:**

| Option | Verdict |
|---|---|
| (a) Upstream split of SwiftDashSDK into a portable core and a SwiftData adapter | ✗ Too large and too slow for us. The central `PlatformWalletManager` (3.2k lines, Combine) and the persistence handler (11.6k lines, 206 SwiftData references) would need rewriting inside someone else's App-Store-frozen schema discipline. Even when finished, persistence would still cross the C vtable. |
| (b) Fork SwiftDashSDK | ✗ We would own 79k lines of drift, 109 frozen-schema files, and a persistence model we don't want. |
| (c) Thin Swift wrappers over the existing `platform-wallet-ffi` C surface | ✗ We would still have to implement the 46-slot persistence vtable in Swift, because the handle type is fixed to `FFIPersister`. Swift would also handle mnemonics through the resolver. That is the same problem as (a) with fewer tools. |
| **(d) Our own Rust façade over the `platform-wallet` library, UniFFI to Swift** | ✓ **Chosen.** Persistence (`SqlitePersister`) and signing (`Signer`) are native Rust plug-ins. The FFI is typed and generated. One code path covers every OS, so what we test on Linux is what ships on macOS. dash-evo-tool proves the crates are consumable from Rust. |

**Cost we accept.** `platform-wallet-ffi` (~59k lines) contains hard-won lifecycle guards. For example, `core_wallet_tx_builder_finalize` re-checks wallet generation after signing so it never publishes a handle for a removed wallet (platform#4185, verified at `core_wallet/transaction_builder.rs:124-200`).

**Rule:** every `dw-engine` operation that has a platform-wallet-ffi counterpart must cite that counterpart in its doc comment. Reviewers diff the guard logic against it. Any logic that exists only in the FFI is upstreamed into `platform-wallet` where possible, which benefits iOS too, or else replicated.

### 1.4 Rust workspace (`rust/`)

Every crate has a single owning workstream (§5).

| Crate | Owns | Key deps | WS |
|---|---|---|---|
| `dw-ffi` | The UniFFI surface only: `#[uniffi::export]` objects, records and errors. One module per domain: `api/{engine,wallet,sync,send,vault,coinjoin,governance,masternode,identity,dpns,dashpay,shielded,platform_address,tokens,dashconnect,compat,console,desktop,qr}.rs`. No logic beyond type mapping. `crate-type = ["staticlib","cdylib","lib"]`, `lib name = "dashwallet_core"`. | everything below | 02 owns `lib.rs`; each domain module is owned by its feature WS |
| `dw-engine` | `Engine` (tokio runtime, logging), `NetworkSession` (one active network: `dash_sdk::Sdk`, `PlatformWalletManager<SqlitePersister>`, `SpvRuntime` config, event pump), wallet registry (open/close/load-on-startup, blank wallets), history read model, Core tx classification (dash-qt's 19 types plus iOS categories), coin control (candidate sets, user locks, dust protection), fee policy, rescan and birth height, automatic backups, fixture mode | platform-wallet, platform-wallet-storage, dash-spv, dash-sdk, `dw-vault`, `dw-appdb` | 02 (L1), 07 (Platform modules under `dw-engine/src/platform/`) |
| `dw-vault` | DEK/KEK slots, Argon2id, XChaCha20-Poly1305, OS-store slot (via `platform-wallet-storage` keyring backends), lock states (`NoKeys/Unencrypted/Locked/UnlockedMixingOnly/Unlocked`), `AuthGrant`, `VaultSigner: key_wallet::signer::Signer`, Core-compatible BIP39 seed derivation, zeroize/mlock | platform-wallet-storage (`secrets` feature primitives), key-wallet | 03 |
| `dw-appdb` | `app.sqlite` per network (rusqlite + refinery): address book, address/tx labels, tx metadata (tax category, memo, historical fiat rate, service, icon), receive requests, user UTXO locks, CoinJoin salt and session journal, wallet governance proposals (`g_object` equivalent), vote history, gift cards, swap orders, notification dedup, settings KV | rusqlite (same bundled `libsqlite3-sys 0.36`, verified: only one version in the lock file) | 02 |
| `dw-p2p` | Minimal outbound Dash P2P session: version/verack handshake, `RawNetworkMessage` codec from `dashcore`, `Unknown{command,payload}` passthrough for `dsa/dsi/dsf/dss/dsc/dssu/dsq/dstx/govobj/govobjvote/govsync`, SOCKS5, rate limiting, peer selection from the SPV masternode list | dashcore, tokio, tokio-socks | 05 |
| `dw-governance` | `CGovernanceObject` and `CGovernanceVote` serialization and hashing (port of `dash/src/governance/{object,vote}.cpp`), `govsync` full and per-object vote sync, vote signature verification against SML voting keys, status and threshold logic, superblock math (cycle 16616 / window 1662 on mainnet; 24/8 on test/devnet; 20/10 on regtest), proposal JSON (exact key order), collateral transaction (1 DASH `OP_RETURN <hash>`), vote signing via `VaultSigner` | dw-p2p, dashcore, dw-vault | 05 |
| `dw-protx` | ProRegTx (fund / existing UTXO / external collateral + `signmessage` proof), ProUpServTx (wraps platform-wallet's revive), ProUpRegTx, ProUpRevTx builders and payload signing, BLS operator key generation (basic scheme), v24 shared-masternode envelope protocol (`dash-shared-mn-session` v1 JSON, fingerprints, rounds, standby dissolution hex) | dashcore special_transaction payloads (present, verified), key-wallet BLS, dw-vault | 05 |
| `dw-coinjoin` | CoinJoin client, ported from Core `src/coinjoin/client.cpp` with dashj as the SPV reference: denominations (verified constants in research 02 §9.1), collateral creation, queue join/create, session state machine, multi-session, denom goal and hard cap, post-V24 promotion/demotion, the "fully mixed" salt rule, dash-qt's progress formula, status strings, dsq BLS verification against the operator keys in the SML | dw-p2p, platform-wallet (CoinJoin account m/9'/c'/4'/a'), dw-vault, dw-appdb | 06 |
| `dw-compat` | Core BIP39 quirks (weak checksum acceptance, no NFKD, salt cut at 256 bytes), `wallet.dat` readers (SQLite `main` table; a read-only BDB 4.8 btree parser ported from Bitcoin Core 28 `src/wallet/migrate.cpp`), Core crypter (`mkey` SHA-512 EVP_BytesToKey + AES-256-CBC, `ckey` IV = Hash(pubkey)[0:16]), `dumpwallet` parse/write, `importdescriptors` / `listdescriptors` JSON with descriptor checksums, dash-qt preferences import (QSettings + settings.json), stretch goal: SQLite descriptor `wallet.dat` writer | dashcore, key-wallet, rusqlite, aes/cbc/sha2 | 04 |
| `dw-uri` | `dash:` URI parse/generate byte-identical to Core `GUIUtil::parseBitcoinURI`, plus iOS extensions (`pay:`, `dashwallet:`, `r=` detection, `sender/user/currency/local`), deep-link classifier (`dashpay://`, `dash-key:`, `dash-st:`, invitations), address classifier (Base58 Core / bech32m Platform / Orchard shielded) | dashcore | 04 |
| `dw-message` | `signmessage`/`verifymessage` ("DarkCoin Signed Message:\n", 65-byte compact, base64) | dashcore `sign_message` (verify present in Rust, research 01) | 04 |
| `dw-psbt` | PSBT create/sign/combine/finalize/analyze over `key_wallet::psbt` (present, verified dir), binary/base64 I/O, HWI-protocol external signer (enumerate, signtx, displayaddress) | key-wallet | 04 |
| `dw-sweep` | WIF / BIP38 / paper-wallet sweep. Phase 1 finds UTXOs through Insight (privacy disclosed). Phase 2 uses a compact-filter scan (upstream U6). Builds the sweep transaction to an HD wallet address. Also sweeps loose keys from dumpwallet / wallet.dat imports. | dw-chaindata, dashcore | 04 |
| `dw-console` | Console grammar (whitespace/comma args, quoting, nested calls `f(g(x) 1)`, `[key]`/`[0]` indexing), `help-console`, history redaction list (Core's 9 sensitive commands), command table mapped to engine calls, optional dashd JSON-RPC passthrough | dw-engine, dashcore-rpc | 04 |
| `dw-chaindata` | Optional data sources: dashd JSON-RPC (PoSe, last paid, next payment, mempool, credit pool, `estimatesmartfee`) and an Insight client (fee lookup, phrase repair, unconfirmed check, sweep UTXOs) | dashcore-rpc, reqwest (rustls) | 04 |
| `dw-desktop` | Windows/Linux OS integration: tray (Linux `ksni` StatusNotifierItem, no GTK3; Windows `Shell_NotifyIcon` on its own message-pump thread), notifications (`notify-rust`), autostart (`auto-launch`), single-instance IPC (`interprocess` local socket), clipboard images (`arboard`), file dialogs (`rfd`, xdg-portal), DPAPI, Windows Hello (`windows` crate, KeyCredentialManager) | `cfg(not(target_os="macos"))` | 08 |
| `dw-qr` (module inside `dw-engine`) | QR encode (`qrcode`, ECC L, 255-char limit) → module matrix. QR decode (`rqrr`) from RGBA buffers | — | 02 |
| `dwcli` (binary) | Headless CLI over `dw-engine`: create/restore/sync/send/mix/vote/register. Acts as the regtest test driver and also hosts the console. | dw-engine, dw-console | 02 |
| `dw-testkit` | Regtest orchestration helpers, fixture builders, golden-vector loaders | — | 13 |

**Upstream pins** (in `rust/Cargo.toml`): `platform-wallet`, `platform-wallet-storage` (features `sqlite,secrets,shielded`), `dash-sdk`, `dpp`, `rs-sdk-trusted-context-provider` from `git = "https://github.com/dashpay/platform", rev = "bc321362b9…"`. rust-dashcore crates use **exactly** the revision platform pins (`e4208c90…`, verified in platform `Cargo.toml`); a mismatch would duplicate types. Toolchain is `1.98.1`, matching platform's `rust-toolchain.toml` (verified). The dev machine has 1.97.1, so `rustup` installs 1.98.1 at bootstrap.

### 1.5 The FFI boundary (UniFFI)

**Mechanics:**
- UniFFI proc-macros with Swift bindings, pinned to the latest stable at bootstrap (≥0.29, exact version UNVERIFIED).
- `uniffi-bindgen generate --library libdashwallet_core.a --language swift` produces `DashWalletCore.swift`, `dashwallet_coreFFI.h` and the modulemap.
- The generated Swift is **committed** under `Sources/DashWalletCore/Generated/`. This lets Swift-only agents build without regenerating. CI regenerates it and fails on any diff.
- The `DashWalletCore` target compiles in Swift 5 language mode (`swiftLanguageModes: [.v5]`), so the generated code's Sendable gaps don't block Swift 6 strict concurrency elsewhere.

**Object model** (example signatures; final shapes are owned by each workstream):

```rust
// dw-ffi/src/api/engine.rs
#[derive(uniffi::Object)] pub struct Engine { /* Arc<dw_engine::Engine> */ }
#[uniffi::export(async_runtime = "tokio")]
impl Engine {
    #[uniffi::constructor]
    pub fn new(config: EngineConfig, observer: Arc<dyn EngineObserver>) -> Result<Arc<Self>, EngineError>;
    pub async fn open_network(&self, network: DashNetwork, opts: SessionOptions) -> Result<Arc<NetworkSession>, EngineError>;
    pub fn data_layout(&self) -> DataLayout;                 // paths per OS (for "Open data folder")
}

#[uniffi::export(callback_interface)]
pub trait EngineObserver: Send + Sync { fn on_event(&self, event: EngineEvent); }

#[derive(uniffi::Enum)]
pub enum EngineEvent {                     // debounced per domain in Rust (≤4 Hz), never a firehose
    Sync(SyncSnapshot),                    // phases: headers/filterHeaders/filters/masternodes/finished, peers, tip, stall
    Peers(Vec<PeerInfo>),
    Balances { wallet: WalletId, balances: WalletBalances },          // confirmed/unconfirmed/immature/locked + coinjoin/mixed + platform + shielded
    HistoryChanged { wallet: WalletId, txids: Vec<String> },          // pull the page again; rows are not pushed
    LockState(VaultLockState),
    CoinJoin { wallet: WalletId, status: CoinJoinStatus },
    Governance(GovernanceSyncState),
    Masternodes(MasternodeListState),
    Platform(PlatformDomainEvent),                                    // identity/contacts/dpns/shielded/addresses
    Notice(EngineNotice),                                             // warnings, alert banner, backup failures
}

// dw-ffi/src/api/send.rs
#[derive(uniffi::Object)] pub struct TxDraft;
#[uniffi::export(async_runtime = "tokio")]
impl TxDraft {
    pub fn set_recipients(&self, r: Vec<Recipient>) -> Result<(), SendError>;   // address, amount, subtract_fee, label
    pub fn set_source(&self, s: CoinSource);          // Any | FullyMixedOnly (CoinJoin page) | Outpoints(Vec<OutPoint>)
    pub fn set_fee(&self, f: FeeMode);                 // Recommended{target_blocks} | PerKb(duffs)
    pub fn set_change(&self, c: ChangePolicy);         // Auto | Address(String) | ToFee
    pub async fn prepare(&self, grant: AuthGrantId) -> Result<PreparedTx, SendError>;  // build+sign+reserve, NEVER broadcasts
    pub async fn broadcast(&self, p: Arc<PreparedTx>) -> Result<BroadcastOutcome, SendError>;
    pub async fn abandon(&self, p: Arc<PreparedTx>);
}
```

**Rules:**
1. **Coarse-grained, task-shaped APIs.** "Prepare a send", not "add an input". Expect roughly 300–400 exported functions, against ~894 in the unified C surface.
2. **Errors** are `#[derive(uniffi::Error)]` enums per domain. Each carries a stable `code` plus user-safe context. Swift maps codes to dash-qt and iOS copy (QT-062, IOS-051). Rust never produces user-facing English.
3. **Async everywhere I/O happens.** Every async export runs on the engine's tokio multi-thread runtime. Sync exports must be O(1) or pure reads of in-memory snapshots.
4. **Events are signals; data is pulled.** iOS's lesson (arch review T11/T12/T26) is one typed, debounced stream per domain. View models re-query paged data such as `history_page(query)` or `utxos(filter)` when a signal arrives.
5. **Secrets cross the FFI exactly once, in one direction.** User passphrases and biometric-released wrap keys come in as `Vec<u8>` and are zeroized on the Rust side after use. Mnemonic reveal goes out only through `Vault.reveal_secret(grant) -> SecretBytes`. On the Swift side it is held in `DashKit.SecretBytes` (`[UInt8]`, zeroed on deinit) and rendered transiently. No other API returns secret material.
6. **Fixture mode.** `EngineConfig.mode = .fixture(path)` loads a canned `wallet.sqlite` + `app.sqlite` + event script with networking disabled. UI and view-model agents work and test without regtest.

### 1.6 Swift package and targets (single root `Package.swift`)

One root package avoids cross-package version juggling for agents. `Apps/macOS` is a thin XcodeGen app that depends on the package's library products.

| Target | Kind | Role | Allowed imports (enforced by `scripts/lint-imports.sh` in CI) |
|---|---|---|---|
| `DashWalletCoreFFI` | binaryTarget (SE-0482 `staticLibrary` artifact bundle) | C module + `libdashwallet_core` | — |
| `DashWalletCore` | library (generated) | UniFFI Swift | Foundation, DashWalletCoreFFI |
| `DashKit` | library | Ergonomic SDK layer: `Amount` (Int64 duffs, credits 1000:1), `DashNetwork`, `EngineClient` (actor), `EventBus` (`AsyncStream` per domain, fan-out), error mapping, `SecretBytes` | Foundation, DashWalletCore |
| `WalletRuntime` | library | Adapter layer (§1.12): `WalletHost`, `LifecycleQueue`, `SPVCoordinator`, `WalletState`, `TransactionSender`, `CoinJoinCoordinator`, `GovernanceService`, `MasternodeService`, `IdentityCoordinator`, `ContactsService`, `DPNSService`, `ShieldedCoordinator`, `PlatformAddressSync`, `AuthenticationGate`, `FeatureGates` | Foundation, Observation, DashKit, PlatformServices |
| `AppServices` | library | Rates/`CurrencyExchanger`, typed `HTTPClient` (endpoint enums, token hook, ETag, Date-header → secure time), BIP70/72, Explore sync and queries (through Rust), integrations, tax/CSV, `NotificationDispatcher`, faucet | Foundation, Observation, DashKit, PlatformServices |
| `PlatformServices` | library | Protocols only: `QuickUnlockStore`, `Clipboard`, `URLOpener`, `Notifier`, `FileDialogs`, `Autostart`, `SingleInstance`, `TrayController`, `QRCapture`, `PowerAssertion` (App Nap), `SessionEndObserver`, `Geolocation`, `ScreenCaptureGuard` | Foundation |
| `PlatformServicesMac` | library `#if os(macOS)` | AppKit, Security (data-protection keychain + `SecAccessControl .biometryCurrentSet`), LocalAuthentication, UserNotifications, ServiceManagement (`SMAppService`), ScreenCaptureKit, AVFoundation, CoreLocation | Apple frameworks |
| `PlatformServicesDesktop` | library `#if os(Windows) \|\| os(Linux)` | Thin Swift wrappers over `dw-desktop` UniFFI objects | Foundation, DashKit |
| `WalletFeatures` | library | **All view models**, `AppRoute`/`SidebarItem`/`SheetRoute` enums, `AmountFormatter` (dash-qt truncation and thin-space rules, iOS fiat), `L10n` tables, input validators | Foundation, Observation, WalletRuntime, AppServices, PlatformServices, DesignTokens |
| `DesignTokens` | library (generated) | Colors (light/dark RGBA pairs from `SharedAssets.xcassets` + DashUIKit `Media.xcassets`), type scale (`DashTextStyle`), spacing, radii, network hue shifts | Foundation |
| `DashUIMac` | library `#if os(macOS)` | DashUIKit (our fork with `.macOS(.v14)`, `Toast`/`SearchBar`/`AddressFieldView` ported to AppKit, `BottomSheet` → macOS sheet) plus desktop components: `DataTable`, `CoinControlTable`, `StatusBar`, `QRView`, `AmountField` | SwiftUI, DashUIKit, DesignTokens |
| `DashUICross` | library | SwiftCrossUI re-implementations with the **same names and props** as DashUIKit (`DashButton`, `MenuItem`, `TransactionView`, `EnterAmountView`, `Toast`, …) | SwiftCrossUI, DesignTokens |
| `MacUI` | library | SwiftUI screens, scenes, menus, `MenuBarExtra`, Settings scene | SwiftUI, DashUIMac, WalletFeatures |
| `CrossUI` | library | SwiftCrossUI screens | SwiftCrossUI, DashUICross, WalletFeatures |
| `DashWalletCross` | executable (Linux/Windows) | `@main` composition root for CrossUI | everything Cross |
| `Apps/macOS/DashWallet` | Xcode app target (XcodeGen `project.yml`; `.xcodeproj` gitignored) | `@main` composition root, Info.plist, entitlements, assets, XCUITests | MacUI, PlatformServicesMac |
| Tests | `DashKitTests`, `WalletRuntimeTests`, `AppServicesTests`, `WalletFeaturesTests` (headless; run on all 3 OSes), `DashUIMacSnapshotTests` (macOS), `CrossUISmokeTests` (fork's exported `DummyBackend`) | Swift Testing | — |

macOS deployment target is **14.0** (`@Observable` in SwiftUI). `Observations` (macOS 26-only) is banned in shared code.

### 1.7 Persistence

| Store | File (per network dir) | Owner | Contents |
|---|---|---|---|
| Wallet state | `wallet.sqlite` | `platform-wallet-storage::SqlitePersister` (WAL, online backup) | accounts, address pools, core txs, UTXOs, IS locks, sync state, identities and keys (public), contacts, Platform addresses, asset locks, invitations, DPNS states, token balances, DashPay, tracked MNs, shielded viewing keys |
| Shielded tree | `shielded_tree.sqlite` | platform-wallet `ShieldedStore` file store | notes, nullifiers, commitment tree |
| SPV chain | `spv/` | dash-spv flat files + lockfile | headers, filter headers, filters, MN state, peers |
| App metadata | `app.sqlite` | `dw-appdb` (refinery, append-only timestamped migrations; duplicate versions refused, per the iOS lesson) | address book, labels, tx metadata, receive requests, UTXO locks, CoinJoin salt and session journal, proposals, vote history, gift cards, swap orders, notification dedup |
| Secrets | `vault/` | `dw-vault` | encrypted records (§1.8) |
| Explore DB | `explore.db` | `dw-engine` read-only queries (FTS4) | merchants/ATMs (downloaded from Firebase Storage, checksum-zipped, as on iOS) |
| UI prefs | `settings.json` | Swift `SettingsStore` (Codable, atomic write, `.bak` on corruption → QT-007 Reset/Abort) | window geometry, unit, digits, theme, fonts, tab visibility, filters, coin-control sort, third-party URLs, CoinJoin UI toggles |
| Global prefs | `../global.json` | Swift | last network, language, autostart, tray behaviour |
| Logs | `logs/` | Rust `tracing` rolling files + Swift `swift-log` file handler | exported as a zip by Rust (IOS-112) |
| Automatic backups | `backups/<wallet>.YYYY-MM-DD-HH-MM.dwbackup` (rotate 10) | `dw-engine` | encrypted bundle: vault records + `app.sqlite` + `wallet.sqlite` online backup |

**Data root per OS:**
- macOS: `~/Library/Application Support/org.dashfoundation.DashWallet/<mainnet|testnet|devnet-<name>|regtest>/`
- Windows: `%APPDATA%\Dash\DashWallet\<net>\`
- Linux: `$XDG_DATA_HOME/dashwallet/<net>/` (Flatpak: `~/.var/app/org.dashfoundation.DashWallet/data/…`)

`--datadir` and the first-run chooser (QT-004) override the root. Each network is isolated, which satisfies QT-002.
Whatever the umask, the app creates every directory on the path to a database or secret 0700, and the
databases, secrets and settings files 0600, because platform-wallet-storage refuses a database below a
group-writable directory. The default root and the directories the engine creates lose group and other access
when found with it; a user-chosen root and its parents are not changed, and the storage error names the one to
fix. Modes are only changed through descriptors opened with `O_NOFOLLOW` relative to the parent's (dw-fs,
`PrivateFileSystem`), never by path. Exports outside the data root (CSV, PSBT, log zip) are created 0600.

**Restore completeness gap (verified).** SqlitePersister does **not** attest `WALLET_RESTORE`: "token balances and the DashPay overlay have no load readers, so a full restore remains lossy" (`persister.rs:1311-1313`). Mitigation:
- Engine side: after `load()`, the engine forces a Platform re-sync of token balances and the DashPay payment overlay. This costs time but loses no data.
- Upstream U5 adds the load readers.
- Gate G5 tests kill-and-restart equivalence.

### 1.8 Secrets, encryption and auth

**Key hierarchy (`dw-vault`):**

```
DEK (256-bit random, per network vault)
 ├─ records: XChaCha20-Poly1305(DEK, payload, AAD = record_id ‖ network ‖ schema_ver)
 │    wallet/<id>/mnemonic, wallet/<id>/mnemonic_passphrase, wallet/<id>/seed|xprv (imports),
 │    identity-key/<id>/<path> (imported only), tracked-mn/<proTx>/<role>, integration/<svc>/token
 └─ wrap slots (any one unwraps DEK):
      slot P  "passphrase":  KEK = Argon2id(passphrase, salt, calibrated ≥0.5 s, m ≥ 256 MiB, t ≥ 3)
      slot B  "quick unlock": KEK held by the OS behind biometrics (macOS: data-protection keychain item,
                              SecAccessControl .biometryCurrentSet; Windows: Hello KeyCredential signature
                              over a fixed challenge → HKDF; Linux: not offered)
      slot O  "os-store":    DEK itself in the OS secret store — ONLY in unencrypted mode
                              (macOS keychain, Windows DPAPI-protected file, Linux Secret Service via
                              platform-wallet-storage keyring backends)
```

- dash-qt "Encrypt Wallet" means slot P exists and slot O is deleted.
- "Change passphrase" re-wraps the DEK only; the seed stays the same, which matches dash-qt semantics.
- There is no "decrypt", which also matches Core.
- AEAD and Argon2 primitives come from the same crates `platform-wallet-storage` uses (`argon2 0.5.3`, `chacha20poly1305 0.10.1`, `memsec`). The slot logic is ours. Its exact reuse of storage's vault API is UNVERIFIED.

**Lock states (QT-022, QT-111/112):**
- `NoKeys` — watch-only.
- `Unencrypted`.
- `Locked` — DEK not in memory.
- `UnlockedMixingOnly` — DEK in memory, but `VaultSigner` only accepts `SignPurpose::CoinJoin{session}`.
- `Unlocked`.

GUI unlock has no timeout, as in dash-qt. Auto-lock (IOS-015) drops to `Locked`, or to `UnlockedMixingOnly` while mixing is running.

**Auth grants (enforcing iOS's "one auth primitive").** Swift `AuthenticationGate` collects a credential: passphrase, biometric, or app PIN in unencrypted mode. It calls `vault.authorize(purpose, credential) → AuthGrant{id, scope, expires}`.

| Grant scope | Required by |
|---|---|
| `Spend{max_duffs}` | `TxDraft.prepare` |
| `RevealSecret` | showing the recovery phrase or keys |
| `SignMessage` | message signing |
| `MasternodeOp` | ProTx operations |
| `Governance` | votes and proposals |
| `PlatformOp` | identity, DPNS, DashPay, credits, shielded |
| `ChangeCredential` | passphrase/PIN changes |
| `Wipe` | wallet removal |

- Biometric grants carry the iOS spend allowance (default 0.5 DASH; options 0 / 0.1 / 0.5 / 1 / 5) and the 7-day passphrase freshness rule. Rust enforces both, so a buggy view cannot bypass them.
- Setting "Require authentication for every payment": default **on** (iOS behaviour). Turning it off gives dash-qt behaviour, where an unlocked wallet signs without re-prompting.

**Lockout policy (IOS-012).** iOS's `6^(n−3)·60 s` waits run on passphrase/PIN attempts, using a monotonic secure-time ratchet persisted in the vault header. It is explicitly **UX throttling**: the real protection is Argon2id cost. There is no "disabled at 8" wipe on desktop. Instead, after 8 failures the UI offers "Restore with recovery phrase".

**Forgot passphrase / PIN (IOS-014):**
1. Enter the recovery phrase.
2. Rust checks it against `wallet_id`.
3. A new vault is created with the new passphrase.
4. Imported non-HD secrets cannot be recovered; the user is warned.

**Seed safety ordering (iOS rule 3), inside `create_wallet`:**
1. Generate the mnemonic.
2. Write the vault record and fsync.
3. Read it back and compare.
4. Register the wallet in the manager and persist.
5. On failure, roll back only the provisional record.

### 1.9 Networking and trust

- **L1:** dash-spv embedded in `platform-wallet::SpvRuntime`. Masternode sync is always on (needed for IS/CL and CoinJoin). Peers come from DNS seeds, with an optional user peer list. "Change peers" after a 45 s stall (IOS-023) is supported.
- **Platform proofs:** start with `rs-sdk-trusted-context-provider` (HTTPS quorum service, iOS parity). At M6, add `SpvContextProvider` in `dw-engine` backed by `SpvRuntime::get_quorum_public_key` (verified at `spv/runtime.rs:340`), falling back to the trusted provider until the SPV masternode list is synced. This removes iOS's trusted-HTTPS dependency once SPV is ready.
- **TLS on Linux:** `rs-sdk-trusted-context-provider` uses default reqwest (native-tls → openssl) on everything except Android (verified, its `Cargo.toml:60-64`). Interim: add `openssl = { features = ["vendored"] }` under `cfg(target_os="linux")` in `dw-ffi`, giving a static OpenSSL and no runtime libssl. Upstream U4 makes the Linux path match Android's rustls.
- **Proxy/Tor (QT-023, QT-138):** dash-spv has no proxy support (verified: no socks/proxy symbols in `dash-spv/src`). Upstream U1 adds a SOCKS5 connector plus onion `AddrV2` peers. Until U1 lands, the proxy UI is shown but disabled with "requires engine update". `dw-p2p`, `dw-chaindata` and the Swift `HTTPClient` honour the proxy from day one.
- **App HTTP:** one `HTTPClient` in AppServices, per the iOS §2.7 pattern. It has an allow-list of hosts (research 03 §1.20) and no third-party analytics. Firebase is replaced by plain HTTPS GETs to the public Firebase Storage URL for the Explore DB; no Firebase SDK.

### 1.10 UI per OS, information architecture, design system

**Window model.** The same on every OS, adapted to each OS's idioms:
- **Main window:** `NavigationSplitView`.
  - Sidebar sections:
    - Home (dash-qt Overview + iOS balance hero, breakdown card, shortcuts, recent txs)
    - Send
    - Receive
    - Transactions
    - Transfer (internal Core↔Shielded↔Platform↔credits)
    - Contacts (runtime: identity present)
    - Explore
    - CoinJoin (QT-012: shown when enabled)
    - Masternodes (opt-in)
    - Governance (opt-in)
  - Cmd/Alt+1…N shortcuts are renumbered by visible items (QT-012).
  - Wallet selector in the toolbar, shown when 2+ wallets are open (QT-014).
  - dash-qt status bar at the bottom: unit, HD, lock, proxy, connections, governance clock, sync.
- **Tools window** (separate): Information, Console, Network Traffic, Peers, Repair, plus iOS Sync Info (Core/Platform/DashPay/Shielded) and a developer-gated Storage Explorer.
- **Settings window:** dash-qt tabs Main, Wallet, CoinJoin, Network, Display, Appearance, plus Security, Currency, Notifications, Integrations, Advanced/Developer.
- **Lock screen:** a full-window overlay with Quick Receive (watch-only data, so it works while locked) and Scan to Send (IOS-013).
- **Menu bar / tray companion (IOS-117 + QT-028/029):** balance (respects hide), receive QR, request amount, scan/pay, last tx, plus dash-qt tray actions.
- Menus follow QT-015…018 exactly. On macOS they are a SwiftUI `CommandMenu`. On Cross platforms they are a SwiftCrossUI menu bar (Windows), or GTK header-bar menus with matching accelerators (Linux).

**Visual language:** iOS/DashUIKit everywhere.
- White cards on `#F7F7F7` (dark: `#1E1F24` on `#141519`), Dash blue `#008DE4`, near-black text, the DashUIKit type scale, no purple.
- SF Pro on macOS. Inter is bundled for Windows/Linux. Montserrat and Roboto Mono are bundled for the dash-qt font options (QT-140).
- dash-qt themes map as Light → Light, Dark → Dark. **Traditional → "Native"**: system accent with stock control styles, which is cheap in both toolkits.
- Testnet/regtest/devnet branding uses dash-qt hue shifts plus the iOS badge, and window title text on every non-mainnet network. This deliberately fixes dash-qt quirk #1.

**Tokens and assets:**
- `scripts/gen-tokens` (Swift script) reads both `.xcassets` catalogs and emits `DesignTokens/Generated/{Colors,Typography}.swift` plus `tokens.json`.
- Icons are exported from the PDF/SVG vector sets to `Resources/Icons/*.svg`. SwiftUI uses them through an asset catalog. SwiftCrossUI gets them as PNG @1x/@2x, rasterised at build time by the token script.
- Never hand-copy a hex value (DashUIKit rule).

**DashUIKit on macOS:**
- Open an upstream PR to `dashpay/DashUIKit` adding `.macOS(.v14)` and AppKit ports of `Toast`, `SearchBar` and `AddressFieldView`. These are the only three components compiled out on macOS (research 03 §3.5, built).
- Pin our fork branch until the PR merges.

**DashUICross:** re-implements the ~35 components with identical names, props and states. Each component has a gallery screen in `DashWalletCross --gallery` for visual QA.

### 1.11 View-model sharing and observation

- `@MainActor @Observable final class XxxViewModel`.
  - Inputs: protocol-typed services from `AppEnvironment`, a composition-root struct built once in each app's `@main`. No `static let shared` (iOS arch review T18).
  - Outputs: plain value state (`String`, `Int64` duffs, domain enums, `Data` for QR matrices), plus `route: AppRoute?` / `sheet: SheetRoute?`.
- Engine events flow: `EngineObserver.on_event` (Rust thread) → `EventBus` actor → per-domain `AsyncStream` → VM `Task { for await … }` on the main actor → re-query → assign. Coalescing follows iOS's "reload pass in flight / requested again" pattern, shared in `WalletRuntime.Coalescer`.
- **Banned in shared code:** Combine, `ObservableObject`, SwiftUI, AppKit, SwiftCrossUI, SwiftData, CoreData, CryptoKit, Security, `os.log`.
- **Flows are state machines:** send, registration phases, CoinJoin start, ProTx wizard, proposal create/resume, shared-MN rounds, swap orders. Each is a VM enum with exhaustive transitions and is unit-tested.
- **Sync gating (iOS rule 6):** never gate on SPV `synced`. Gate on `SPVCoordinator.syncDone`, which treats dash-spv's steady `waitForEvents` at ≈1.0 progress as done.
- **Model unknowns as unknown (iOS rule 7):** balances are `Amount?`, fees `Amount?`, PoSe `Int?` with a "requires full-node data source" reason enum.

Example:

```swift
@MainActor @Observable
public final class SendViewModel {
    public private(set) var entries: [RecipientEntry] = [RecipientEntry()]
    public private(set) var phase: SendPhase = .editing            // .editing/.authorizing/.preparing/.confirm(PreparedTxSummary)/.broadcasting/.done(txid)/.failed(SendFailure)
    public var source: CoinSourceChoice = .any                     // .fullyMixed on the CoinJoin page (QT-051)
    private let sender: TransactionSending; private let auth: AuthenticationGating
    public init(env: AppEnvironment, page: SendPage) { … }
    public func paste(_ text: String) { … }                        // dash: URI fills entries (QT-054) via DashKit.URI (Rust dw-uri)
    public func review() async { … }                               // validate → auth (Spend grant) → prepare → .confirm (3 s countdown in view)
    public func confirm() async { … }                              // broadcast ONLY here (iOS rule 4)
}
```

### 1.12 Runtime lifecycle (the iOS adapter shape)

| Desktop type | iOS counterpart | Notes |
|---|---|---|
| `WalletHost` | `SwiftDashSDKHost` | Owns `Engine` and the active `NetworkSession`. Idempotent `start(network:)`. |
| `LifecycleQueue` | `SerialAsyncLifecycleQueue` | **Every** start/stop/network switch/wallet add/switch/remove/wipe goes through it. Exposes `LifecycleTransition` for the overlay (IOS-018). |
| `SPVCoordinator` | `SwiftDashSDKSPVCoordinator` | `SyncSnapshot` with phase rows, damped progress (10 % max delta, 3.25 s peak delay), peers, stall detection, `rotatePeers()`. |
| `WalletState` | `SwiftDashSDKWalletState` | Balance buckets: Core four-bucket, CoinJoin, mixed, Platform credits, shielded. |
| `TransactionSender` | `SwiftDashSDKTransactionSender` | Auth → prepare → confirm → broadcast. Also handles the CoinJoin sweep (≤500 inputs per chunk) and resend/abandon. |
| `PlatformAddressSync`, `ShieldedCoordinator`, `IdentityCoordinator`, `ContactsService`, `DPNSService`, `MarketplaceService`, `VotingService` | same-named iOS adapters | These wrap the `dw-ffi` Platform APIs. The iOS 2.3k-line god object is split by domain. |
| `CoinJoinCoordinator`, `GovernanceService`, `MasternodeService`, `ConsoleService` | — (new) | dash-qt features. |

**Order:** start runs host → SPV → (identity present) Platform/BLAST → DashPay loop → CoinJoin (if enabled and allowed). Stop runs in reverse.

**Background behaviour:**
- Closing the window minimizes to the tray if configured (QT-030). Sync and mixing continue.
- macOS App Nap is suppressed during sync and mixing (`ProcessInfo.beginActivity`).
- Windows session end (`WM_QUERYENDSESSION`, ShutdownBlockReason) and macOS `applicationShouldTerminate` run the reverse stop under the non-closable shutdown window (QT-008).

### 1.13 OS integration matrix

| Capability | macOS | Windows | Linux |
|---|---|---|---|
| Single instance + URI hand-off (QT-001) | NSApplication + `application(_:open:)` (LaunchServices already single-instances) | `dw-desktop` local socket `DashWallet-<net>`; second instance forwards argv URIs and exits 0 | same (abstract Unix socket) |
| URI schemes `dash pay dashwallet dashpay dashid dash-key dash-st` (QT-150, IOS-048) | `CFBundleURLTypes` | MSIX `windows.protocol` extensions | `.desktop` `MimeType=x-scheme-handler/…` |
| Autostart with `--min` (QT-009) | hidden (dash-qt parity); `SMAppService` available but off | MSIX `windows.startupTask` | XDG autostart `.desktop` (Flatpak: Background portal) |
| Tray / menu bar | `MenuBarExtra` | `dw-desktop` Shell_NotifyIcon thread | `dw-desktop` `ksni` (StatusNotifierItem; GNOME needs the AppIndicator extension, disclosed) |
| Notifications (QT-031/032, IOS-116) | UserNotifications | `notify-rust` (WinRT toast) | `notify-rust` (D-Bus) / Flatpak portal |
| Quick unlock | Touch ID (keychain ACL) | Windows Hello | — (hidden) |
| OS secret store (unencrypted mode) | data-protection keychain (needs signed app + keychain-access-groups) | DPAPI file | Secret Service (libsecret over D-Bus) |
| QR capture (IOS-043) | image file, clipboard, ScreenCaptureKit region, AVFoundation camera | file, clipboard, screen region (Graphics Capture), camera at M6 | file, clipboard, screenshot portal, camera at M6 (UNVERIFIED portal camera) |
| Maps (IOS-097) | MapKit | list + "open in OpenStreetMap" | list + OSM link |
| Location (IOS-097 "Nearby") | CoreLocation | Windows.Devices.Geolocation (via `dw-desktop`) | GeoClue portal, else manual ZIP/city entry |
| Screen-capture warning while the phrase is shown (IOS-006) | `NSWindow.sharingType = .none` | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` | not possible; show a warning banner |

### 1.14 Full-node-only features — handled honestly

| Feature | SPV answer (default) | With the optional "dashd RPC" data source (Settings → Advanced → Data sources) |
|---|---|---|
| RPC console (QT-145) | Local console: ~80 Core-named commands over the engine (`getwalletinfo`, `getbalance(s)`, `getnewaddress`, `listunspent`, `lockunspent`, `listtransactions`, `gettransaction`, `sendtoaddress`, `sendmany`, `signmessage`, `verifymessage`, `validateaddress`, `getblockcount`, `getbestchainlock`, `getpeerinfo`, `setban`, `masternodelist` (SML), `protx list/info` (SML + replay), `gobject list/get/getcurrentvotes/prepare/submit`, `gobject vote-many`, `getgovernanceinfo`, `coinjoin start/stop/status/reset`, `getcoinjoininfo`, `coinjoinsalt`, `dumpwallet`, `importwallet`, `listdescriptors`, `upgradetohd`, `bls generate/fromsecret`, `walletpassphrase`, `walletlock`, `encryptwallet`, `rescanblockchain`, `abandontransaction`, `help`, `help-console`). Unknown command → "Not available in SPV mode." | Unknown commands are forwarded to dashd with a "(remote)" tag. Wallet commands always stay local. |
| Mempool stats, credit pool (QT-144) | "—" with tooltip "Requires full-node data source" | filled from `getmempoolinfo`, `getcreditpoolinfo` |
| MN PoSe score, last paid, next payment (QT-119/122) | Status valid/banned from the SML. Owner/payout addresses and shares by replaying ProRegTx/ProUp* found in our wallet plus `protx` from Platform evonode data where available. Other columns "—". | `protx info` / `masternode winners` |
| Governance list, tallies, funded status, budget, clock (QT-26, 128–134) | **SPV-native via `dw-governance` govsync**, with votes verified against SML voting keys. Superblock budget computed from height. "Funded" derived from trigger objects. | Cross-check option |
| Fee estimation (QT-057) | Recommended = 1000 duff/kB (min relay). The target dropdown is kept for parity, with honest text: "Dash blocks are rarely full; all targets use the minimum relay fee." | `estimatesmartfee` |
| Rebuild index (QT-148) | "Reset chain data and resync" (deletes `spv/`, keeps wallets) | n/a |
| Node options: prune, dbcache, par, RPC server, UPnP/NAT-PMP, listen (QT-136/138) | Not shown. A "Not applicable to this wallet (SPV)" footnote lists them. | n/a |

---

## 2. Gap-filling plan

| Gap | Where it is built | Approach and reference | Upstream? | Milestone |
|---|---|---|---|---|
| CoinJoin mixing (QT-041…051, IOS-058) | `dw-coinjoin` + `dw-p2p` | Port Core `coinjoin/client.cpp`, `coinjoin.cpp`, `common.cpp`, `util.cpp` (tx builders); dashj is the SPV reference. Mix on the **DIP9 CoinJoin account m/9'/c'/4'/0'** (key-wallet `AccountType::CoinJoin`). dash-qt descriptor wallets watch this path, dashj and Android mix there, and iOS reads that balance, so funds are visible in all three ecosystems. Own-collateral validity is approximated: only collateral inputs that are confirmed or IS-locked are used. The salt is per wallet in `app.sqlite`; imported dash-qt wallets use their `cj_salt`. | no (uses the candidate-set workaround for coin selection; U3 preferred) | M3 |
| Governance (QT-026, 128–134) | `dw-governance` + `dw-p2p` | Port `governance/{object,vote,common,signing}.cpp` serialization and hashing. `govsync` full sync, then per-object vote sync. Votes are signed with the **voting key** (ECDSA, `VaultSigner`). Proposal collateral is a normal tx through `TxDraft` with an `OP_RETURN` output. "Resume" polls confirmations from wallet history (≥1 to broadcast, 6 to leave "Confirming"). | no | M3 |
| ProRegTx / ProUpRegTx / ProUpRevTx, shared MN (QT-123…127) | `dw-protx` | Payload types exist in `dashcore` (verified). Builders fund through `TxDraft` + `set_special_payload` semantics in our own builder over key-wallet. Owner/payout/voting keys come from key-wallet's provider accounts. Operator BLS generation is the basic scheme. The operator secret is shown once and never stored (QT-124). Shared-MN envelopes follow dash-qt's JSON (`sharedmn*.cpp`). | no | M3 |
| BIP21 / URIs (QT-149, IOS-048) | `dw-uri` | Table-driven port of `guiutil.cpp:281-389`. Test vectors come from Core `src/qt/test/uritests.cpp` plus iOS `DWURLParser` cases. | no | M1 |
| Message verify (QT-100) | `dw-message` | Wraps `dashcore::sign_message` (`is_signed_by_address`, `recover_pubkey`). | no | M2 |
| Fee estimation (QT-057) | `dw-engine::fee` | Static policy + optional RPC (§1.14). | no | M2 |
| PSBT (QT-076…079), watch-only (QT-114) | `dw-psbt`, `dw-engine` | Uses key-wallet `psbt`. Watch-only wallet from an xpub needs platform-wallet registration of a `WatchOnly` wallet (U7). Interim: an external-signable wallet with no vault record. | U7 (small) | M2 |
| HWI external signer (QT-080) | `dw-psbt` | Spawns the user-configured script using the HWI JSON protocol. Signing uses PSBT round-trips through key-wallet's async `Signer`. | no | M6 |
| Coin control / user UTXO locks / dust protection / CoinJoin-only spend / no-change-to-fee (QT-051, 068–075) | `dw-engine::coins` | Interim: compute the candidate UTXO set (minus locks, minus dust-locked, or fully-mixed only). Pass it with key-wallet `TransactionBuilder::add_inputs` and `CoreWallet::finalize_transaction_with_options(…, reservation_only = true)`. This restriction "only removes candidates" and coin selection still runs over the added set (verified, `rs-platform-wallet-ffi/src/core_wallet/transaction_builder.rs:745-765` + `wallet/core/transaction.rs:616`). "Excess to fee": change policy `ToFee` through our own finalize wrapper. Preferred: U3 adds an exclusion set and `change_to_fee` in key-wallet. | U3 (nice-to-have) | M2 |
| dash-qt BIP39 quirks (QT-104) | `dw-compat::bip39core` + `dw-vault` | Strict BIP39 first. On failure, Core's XOR-mask checksum, with a warning. Seed = PBKDF2-HMAC-SHA512(mnemonic bytes, ("mnemonic"+pass)[..256], 2048) with no NFKD. A Core-quirk seed is stored as a **seed-type** vault record so derivation is exact. key-wallet's `Wallet::from_mnemonic` hard-codes an empty passphrase (research 01), so every passphrase restore goes through the seed path anyway. | no | M1 |
| Restore scan lookahead 1000 (QT-105) | `dw-engine` | `set_gap_limit` exists on the platform-wallet core wallet (verified, `wallet/core/wallet.rs:141`). Imported-from-Core wallets get a gap of 1000 on BIP44 external/internal and the DIP9 CoinJoin chain until the first full scan completes, then the defaults. | no | M1 |
| dumpwallet import/export (QT-107/109) | `dw-compat::dump` | Parse the header (mnemonic, passphrase, hdseed, xprv, counters) → rebuild HD. Loose WIF keys and P2SH scripts are **swept** into the HD wallet after confirmation (key-wallet has no loose-key accounts). Labels go to the address book. Export writes Core's exact format. | U6 for trustless sweep discovery | M2 (HD), M5 (sweep) |
| `wallet.dat` import (QT-106) | `dw-compat::walletdat` | SQLite descriptor: read the `main` table, decrypt `walletdescriptorckey` with `mkey`, extract mnemonic and passphrase. BDB 4.8: read-only btree page parser (port of Bitcoin Core 28 `BerkeleyRODatabase`), then `hdchain`/`chdchain` (AES-256-CBC, IV = chain-id[0:16]). | no | M2 (SQLite), M6 (BDB) |
| Export for dash-qt (QT-109, 154) | `dw-compat` | (a) Mnemonic + passphrase with `upgradetohd` instructions; exported only if the phrase also passes Core's check. (b) dumpwallet file. (c) `importdescriptors` JSON. (d) Stretch: SQLite descriptor `wallet.dat` with Core encryption. | no | M2 / M6 (d) |
| Sweep paper wallet / WIF / BIP38 (IOS-056) | `dw-sweep` | Phase 1: Insight UTXO lookup (disclosed privacy cost; an invalid answer can only make the broadcast fail). Phase 2: compact-filter scan from a user date (U6). | U6 | M5 / M6 |
| Phrase repair (IOS-008) | `dw-compat::repair` (Rust, rayon) + `dw-chaindata` Insight | Same algorithm as iOS; English wordlist for on-chain confirmation. | no | M5 |
| Invitation creation (IOS-078) | `dw-engine/src/platform/invitation.rs` | platform-wallet has create/claim/parse invitations (research 01). | no | M4 |
| Encrypted backups (QT-110, 116) | `dw-engine::backup` | `.dwbackup` = versioned container (vault records still DEK-encrypted, wrap slot P only, `app.sqlite`, `wallet.sqlite` online backup), signed with a MAC under the DEK. CoinJoin is **not** gated on backups (dash-qt quirk #10 fixed). | no | M2 |
| Proxy / Tor (QT-138) | upstream dash-spv | SOCKS5 connector + onion AddrV2. | **U1** | M6 |
| Traffic counters, ban list (QT-146/147) | upstream dash-spv + `SpvRuntime` accessors | `ban_peer`/`unban_peer`/`disconnect_peer` exist in dash-spv's network manager (verified, `manager.rs:1758-1797`). Per-peer bytes and ping are missing. Expose them through platform-wallet. | **U2** | M2 (peers), M6 (traffic graph if U2 is late; until then the graph shows engine-side counters from `dw-p2p` + SPV aggregate) |
| Linux TLS | upstream | rustls on Linux. | U4 (interim: vendored OpenSSL) | M0 |
| Restore completeness | upstream platform-wallet-storage | Load readers for token balances and the DashPay overlay. | **U5** (interim: forced re-sync) | M4 |

**Upstream policy (how we stay buildable):**
1. `rust/Cargo.toml` pins platform `bc321362b9` and rust-dashcore `e4208c90`.
2. Any upstream change is (a) a PR to `dashpay/rust-dashcore` or `dashpay/platform` **from a same-repo branch** (the team rule: platform CI skips fork PRs), and (b) carried meanwhile through a `[patch."https://github.com/dashpay/…"]` entry pointing at that branch's commit SHA. It is never a path dependency.
3. Each patch has a line in `rust/PATCHES.md` (upstream PR URL, reason, removal condition).
4. **Bumping platform** is a dedicated PR. It runs the full regtest and compat suites and updates `PATCHES.md`. Bumps happen at platform release tags, never on `-dev` heads between tags, unless a fix we need lands.
5. **Rule of first resort:** if a feature can live in a `dw-*` crate using public upstream APIs, it must. U1–U7 are the complete current list of things that cannot.

---

## 3. Repo layout, build, packaging, CI

### 3.1 Tree

```
dashwallet-desktop/
├─ Package.swift                    # single root SwiftPM package (targets in §1.6)
├─ Package.resolved
├─ Sources/
│  ├─ DashWalletCore/Generated/     # UniFFI output (committed; CI diff-check)
│  ├─ DashKit/  WalletRuntime/  AppServices/  PlatformServices/
│  ├─ PlatformServicesMac/  PlatformServicesDesktop/
│  ├─ WalletFeatures/<Feature>/     # one folder per feature: Home, Send, Receive, Transactions, CoinControl,
│  │                                #   CoinJoin, Masternodes, Governance, Wallets, Security, Onboarding, Lock,
│  │                                #   Identity, DashPay, DPNS, Marketplace, Shielded, Transfer, Explore,
│  │                                #   BuySell, Swap, DashSpend, Tools, Console, Settings, Tray …
│  ├─ DesignTokens/Generated/
│  ├─ DashUIMac/  DashUICross/
│  ├─ MacUI/<Feature>/  CrossUI/<Feature>/
│  └─ DashWalletCross/main.swift
├─ Tests/                           # DashKitTests, WalletRuntimeTests, AppServicesTests, WalletFeaturesTests,
│                                   #   DashUIMacSnapshotTests, CrossUISmokeTests
├─ Artifacts/DashWalletCore.artifactbundle/   # gitignored; produced by scripts/build-core
├─ Apps/macOS/project.yml           # XcodeGen; DashWallet.app + DashWalletUITests (XCUITest)
├─ rust/
│  ├─ Cargo.toml  Cargo.lock  rust-toolchain.toml (1.98.1)  PATCHES.md  deny.toml
│  ├─ crates/dw-ffi  dw-engine  dw-vault  dw-appdb  dw-p2p  dw-coinjoin  dw-governance  dw-protx
│  │        dw-compat  dw-uri  dw-message  dw-psbt  dw-sweep  dw-console  dw-chaindata  dw-desktop  dw-testkit
│  └─ bin/dwcli
├─ testdata/                        # golden vectors shared by Rust AND Swift tests (JSON):
│  │                                #   amount_format.json, uri_cases.json, bip39_core_quirks.json, tx_class.json,
│  │                                #   coinjoin_progress.json, governance_objects.json, csv_export.json …
│  └─ fixtures/                     # fixture-mode bundles (wallet.sqlite+app.sqlite+events.jsonl), dash-qt wallets
├─ tests/
│  ├─ regtest/                      # pytest + vendored Dash Core test_framework (pinned v24.0.0), docker-compose
│  └─ compat/                       # dash-qt v23.1.8 / v24 interop suites
├─ Localization/                    # en source tables per feature + imported translations (Transifex)
├─ Resources/  Icons/  Fonts/ (Inter, Montserrat, Roboto Mono — OFL/Apache)
├─ packaging/
│  ├─ macos/  (entitlements, dmg layout, Sparkle appcast template, notarize.sh)
│  ├─ windows/ (AppxManifest.xml, assets, msix.ps1, appinstaller template)
│  └─ linux/  (org.dashfoundation.DashWallet.yml flatpak, .desktop, metainfo.xml, tarball.sh)
├─ scripts/  build-core(.sh|.ps1)  gen-bindings  gen-tokens  lint-imports.sh  disk-guard.sh  bootstrap.sh
├─ docs/  research/  design/  contracts/ (per-domain API notes)  adr/
└─ .github/workflows/  rust.yml  swift.yml  apps.yml  regtest.yml  compat.yml  release.yml
```

### 3.2 `Package.swift` (sketch)

```swift
// swift-tools-version:6.2
import PackageDescription
let crossUI: [Target.Dependency] = [.product(name: "SwiftCrossUI", package: "swift-cross-ui"),
                                    .product(name: "DefaultBackend", package: "swift-cross-ui")]
let package = Package(
  name: "DashWalletDesktop",
  defaultLocalization: "en",
  platforms: [.macOS(.v14)],
  products: [.library(name: "MacUI", targets: ["MacUI"]), .executable(name: "dash-wallet", targets: ["DashWalletCross"])],
  dependencies: [
    .package(url: "https://github.com/dashpay/swift-cross-ui", branch: "dwd-0.10"),   // our fork, pinned by revision in Package.resolved
    .package(url: "https://github.com/dashpay/DashUIKit", branch: "macos-support"),   // until upstream merges macOS
    .package(url: "https://github.com/apple/swift-log", from: "1.6.0"),
  ],
  targets: [
    .binaryTarget(name: "DashWalletCoreFFI", path: "Artifacts/DashWalletCore.artifactbundle"),
    .target(name: "DashWalletCore", dependencies: ["DashWalletCoreFFI"], swiftSettings: [.swiftLanguageMode(.v5)]),
    .target(name: "DashKit", dependencies: ["DashWalletCore", .product(name: "Logging", package: "swift-log")]),
    .target(name: "PlatformServices"),
    .target(name: "PlatformServicesMac", dependencies: ["PlatformServices"]),          // body #if os(macOS)
    .target(name: "PlatformServicesDesktop", dependencies: ["PlatformServices", "DashKit"]),
    .target(name: "WalletRuntime", dependencies: ["DashKit", "PlatformServices"]),
    .target(name: "AppServices", dependencies: ["DashKit", "PlatformServices"]),
    .target(name: "DesignTokens"),
    .target(name: "WalletFeatures", dependencies: ["WalletRuntime", "AppServices", "DesignTokens"], resources: [.process("Resources")]),
    .target(name: "DashUIMac", dependencies: ["DesignTokens", .product(name: "DashUIKit", package: "DashUIKit", condition: .when(platforms: [.macOS]))]),
    .target(name: "MacUI", dependencies: ["DashUIMac", "WalletFeatures", "PlatformServicesMac"]),
    .target(name: "DashUICross", dependencies: ["DesignTokens"] + crossUI),
    .target(name: "CrossUI", dependencies: ["DashUICross", "WalletFeatures", "PlatformServicesDesktop"] + crossUI),
    .executableTarget(name: "DashWalletCross", dependencies: ["CrossUI"]),
    .testTarget(name: "WalletFeaturesTests", dependencies: ["WalletFeatures"], resources: [.copy("../../testdata")]),
    // … DashKitTests, WalletRuntimeTests, AppServicesTests, DashUIMacSnapshotTests, CrossUISmokeTests
  ])
```

### 3.3 Rust build to artifact bundle

**`scripts/build-core --triple <t> --profile dev|release`:**
1. Run `disk-guard` (abort if <15 GB free).
2. `cargo build -p dw-ffi --profile <p> --target <t>` with `CARGO_TARGET_DIR=${DWD_TARGET:-~/workspace/.cargo-target/dwd}`.
3. `uniffi-bindgen generate --library … --language swift --out-dir Sources/DashWalletCore/Generated`. The header and modulemap move into the bundle.
4. Write `Artifacts/DashWalletCore.artifactbundle/<variant>/{libdashwallet_core.a|dashwallet_core.lib, include/}` and merge `info.json` variants (`supportedTriples`). On Windows the modulemap carries `link` directives taken from `cargo rustc -- --print native-static-libs` (expected `ws2_32 bcrypt ntdll userenv advapi32 dbghelp crypt32 secur32 ncrypt ole32 …`, UNVERIFIED final list). Zipped bundles avoid the SwiftPM #10077 naming bug.
5. Linux uses vendored OpenSSL (static), so no system libssl is needed. Link `m dl pthread`.

**Profiles:**
- `dev`: `debug = "line-tables-only"`, `[profile.dev.package."*"] opt-level = 2`, so halo2/BLS/x11 aren't unusably slow.
- `release`: `lto = "thin"`, `codegen-units = 4`, `panic = "unwind"`. Panics are caught at the UniFFI boundary and become `EngineError::Internal`; never abort the GUI process.
- `dist`: `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, with separate debug-symbol upload.

**Local disk discipline** (60 GB free; the release + fat-LTO footprint for this graph is about 25 GB, UNVERIFIED):
- **One** target directory for all agents.
- Agents build only the host triple at the `dev` profile, never `release`/`dist`. CI produces release bundles. The macOS agent may download the latest CI artifact bundle with `scripts/fetch-core.sh` instead of compiling Rust at all when not touching Rust.
- `cargo clean -p dw-*` instead of a full clean.
- `sccache` is local only. `.build/` is shared by the single root package. Xcode DerivedData lives in `~/workspace/.derived/dwd`.
- SwiftPM and git write operations run outside the Claude sandbox (known constraint).

### 3.4 Per-OS build and packaging

| | macOS | Windows | Linux |
|---|---|---|---|
| Toolchain | Xcode 26.6 / Swift 6.3.3, Rust 1.98.1; targets `aarch64-apple-darwin` + `x86_64-apple-darwin` | Swift 6.3.3 (`winget Swift.Toolchain`, VS 2022 + Win11 SDK), Rust MSVC `x86_64-pc-windows-msvc` (arm64 later), LLVM (libclang for `rs-x11-hash` bindgen), protoc | swift:6.3.3-noble image, libgtk-4-dev, clang, protoc; `x86_64`/`aarch64-unknown-linux-gnu` |
| Build | `xcodegen` → `xcodebuild -scheme DashWallet` (Release) | `swift build -c release --product dash-wallet` | `swift build -c release --static-swift-stdlib --product dash-wallet` |
| App bundle | Universal `.app` (lipo the two Rust slices into one variant `supportedTriples: [arm64-apple-macosx, x86_64-apple-macosx]`) | exe + Swift runtime DLLs + resources | binary + resources |
| Package | Developer ID sign, hardened runtime, entitlements (`keychain-access-groups`, `network.client`, optional camera/location) → `notarytool` → staple → DMG. **Sparkle 2** auto-update (EdDSA appcast). | **MSIX** (makeappx + signtool; Azure Trusted Signing or OV cert). Declares a framework dependency on **Windows App Runtime 1.5**, the protocol handlers and the startup task. `.appinstaller` handles auto-update. | **Flatpak** on Flathub (GNOME runtime with GTK4 + `org.freedesktop.Sdk.Extension.swift6`; finish-args: network, wayland/x11 fallback, `--talk-name=org.freedesktop.secrets`, `--talk-name=org.kde.StatusNotifierWatcher`, notification/background/screenshot portals). Also a `tar.gz` portable build (static stdlib + bundled resources). AppImage is not offered: GTK4 AppImages are brittle. |

**Reproducibility.** The Rust core is built `--locked` with `SOURCE_DATE_EPOCH`, and the artifact-bundle SHA-256 is published. Guix reproducible builds are a post-1.0 goal.

### 3.5 CI (GitHub Actions)

| Workflow | Trigger | Jobs |
|---|---|---|
| `rust.yml` | PR touching `rust/**` | fmt, clippy `-D warnings`, `cargo nextest` (linux x86_64), `cargo deny`; build-check `dw-ffi` on macos-arm64 and windows-2025 (dev profile) |
| `swift.yml` | every PR | build the core bundle (cache keyed by `Cargo.lock` + rust sources hash) on 3 OSes. `swift test` for DashKit/WalletRuntime/AppServices/WalletFeatures on **macOS, ubuntu-24.04 (x86_64 + arm64), windows-2025**. `lint-imports`. UniFFI diff check. Token diff check. |
| `apps.yml` | PR touching UI | macOS: xcodegen + build + snapshot tests + XCUITest smoke. Linux: build CrossUI + Xvfb launch + AT-SPI smoke (dogtail). Windows: build + launch smoke (process alive 30 s, no crash dump). |
| `regtest.yml` | nightly + PR label `regtest` | docker-compose regtest network (§4.3) on linux; suites `l1`, `coinjoin`, `governance`, `protx`, `restore` |
| `compat.yml` | nightly | dash-qt/dashd v23.1.8 and v24.x interop suites (§4.4) |
| `release.yml` | tag `v*` | dist profile, universal mac DMG notarized, MSIX signed, Flatpak bundle + Flathub PR, checksums, SBOM (`cargo cyclonedx` + Swift deps) |

Rust caching uses sccache with a GitHub cache backend. Release builds run on larger runners. A self-hosted Mac runner is optional and not assumed.

---

## 4. Testing strategy

### 4.1 Pyramid and ownership

| Layer | Tooling | Runs on | Gate |
|---|---|---|---|
| Rust unit + property tests | `cargo nextest`, `proptest` for every codec (governance objects/votes, CoinJoin messages, ProTx payloads, URI, dumpwallet, BDB pages) | linux CI; dev host | every PR |
| Golden vectors (cross-language) | `testdata/*.json` consumed by **both** Rust tests and Swift tests (amount formatting/truncation, URI parse/generate, tx classification, CSV bytes, CoinJoin progress formula, BIP39 Core quirks, fully-mixed rule) | all | every PR |
| Engine integration (offline) | `dw-engine` tests in fixture mode + `platform-wallet-storage` round-trips (kill/restart equivalence, G5) | linux | every PR |
| Swift view models | Swift Testing. Fakes for every service protocol, plus "engine-backed" tests against a fixture-mode `Engine`. Async flows driven by `withObservationTracking` assertions (probe pattern). | macOS, Linux, Windows | every PR |
| UI | macOS: XCUITest on critical flows (onboarding create/restore, unlock, send + confirm countdown, receive QR, coin control, CoinJoin start/stop, vote, settings) + swift-snapshot-testing via `ImageRenderer` (light/dark, 3 locales incl. RTL). Linux: dogtail/AT-SPI smoke under Xvfb (launch, navigate sidebar, open send, cancel). Windows: launch smoke only until G3 passes; then FlaUI. | per OS | UI PRs |
| Regtest end-to-end | `dwcli` driving a local regtest network (§4.3) | linux docker | nightly + label |
| Testnet / devnet | Platform flows (identity, DPNS, DashPay, shielded, tokens, DashConnect) using `dwcli` + faucet (`faucet.thepasta.org` CAP) | linux | nightly, non-blocking, alerting |
| Compatibility | dash-qt interop (§4.4) | linux docker | nightly; release blocking |
| Security | `cargo audit`/`deny`. Fuzzing (cargo-fuzz) of BDB parser, wallet.dat SQLite reader, dumpwallet parser, URI parser, P2P message decoders. Vault tests (wrong passphrase, tamper, rollback). Secret-in-log grep test. External audit of `dw-vault`, `dw-coinjoin`, `dw-protx`, `dw-compat` before mainnet 1.0 (G8). | linux | nightly / release |

### 4.2 Fixture mode

`testdata/fixtures/<name>/` contains `wallet.sqlite`, `app.sqlite`, `vault/` (test passphrase `test`) and `events.jsonl` (scripted engine events with relative timestamps).

Fixtures: `empty`, `funded-l1`, `heavy-history-5k`, `coinjoin-mixing`, `mn-owner`, `gov-voter`, `identity-dashpay`, `shielded`, `multiwallet`, `watch-only`, `locked-encrypted`.

Fixtures are generated by `dwcli fixture record` against regtest/testnet and committed (small, deterministic). UI agents use them for development, snapshots and XCUITest.

### 4.3 Regtest harness

- `regtest/docker-compose.yml` runs official `dashd` v24.0.0 release binaries (downloaded, SHA-pinned) in one container.
- The **vendored Dash Core functional `test_framework`** (`DashTestFramework`: masternodes, quorums, ChainLocks, InstantSend, sporks) provides `MNs=5–8` for CoinJoin (regtest min participants 2, UNVERIFIED exact value) and governance (cycle 20, window 10).
- Our client under test is `dwcli` (SPV against the regtest node), which exits non-zero on assertion failure.

**Suites:**

| Suite | What it checks |
|---|---|
| `l1` | Create/restore wallet, sync, receive, send with IS lock, CL confirm, coin control, UTXO lock persistence, dust lock, abandon/resend, rescan from birth height |
| `coinjoin` | Two dwcli clients plus dashd wallets mix 2 rounds; balances; fully-mixed rule vs dashd `getcoinjoininfo`; CoinJoin-only send with no change |
| `governance` | Create proposal → collateral → resume → submit → dashd `gobject list` sees it; vote-many with voting keys → `gobject getcurrentvotes` matches; tallies equal dashd's |
| `protx` | Register (fund new / existing UTXO / external collateral with signmessage proof), update service, update registrar, revoke → `protx info` on dashd; shared MN 3-party via `protx shared_*` RPC counterparts |
| `restore` | dashd-generated wallets (legacy BDB, descriptor SQLite, encrypted, CoinJoin-mixed by Core on BIP44 with gaps > 20) restored via mnemonic, dumpwallet and wallet.dat → balance and address-set equality |

### 4.4 dash-qt compatibility suite (release blocking)

1. **Import:** dashd v23.1.8 (BDB) and v24 (descriptor) wallets, encrypted and not, with known mnemonic and passphrase (including non-ASCII and >256-byte passphrases, and weak-checksum phrases) → our import → identical first 1000 addresses per chain, identical balance, labels carried over.
2. **Export:**
   - Our mnemonic+passphrase → dashd blank wallet + `upgradetohd` → `getbalance` equal and `listtransactions` superset.
   - Our dumpwallet → dashd legacy `importwallet` → keys present.
   - Our `importdescriptors` JSON → dashd descriptor wallet → addresses equal.
   - Stretch: our `wallet.dat` → dashd `restorewallet`.
3. **Signatures:** `signmessage` and `verifymessage` cross-check both directions.
4. **URIs:** Core `uritests.cpp` vectors + generated URIs parsed by dash-qt's parser (built as a tiny C++ test harness from Core sources, or exercised through `dashd` where possible).
5. **Network acceptance:** our ProTx, governance objects and votes, and CoinJoin transactions are accepted by dashd (covered by §4.3).
6. **CSV:** our transaction CSV is byte-identical to dash-qt's format for the same history (fixture wallet loaded in both).

---

## 5. Delivery plan for an agent swarm

### 5.1 Milestones (exit criteria are the acceptance gates)

| Milestone | Calendar (est.) | Exit criteria |
|---|---|---|
| **M0 Foundations** | wk 1–2 | Repo skeleton. `dw-ffi` "hello engine" (async fn + callback) builds on mac/linux/windows. Artifact-bundle pipeline. UniFFI Swift runs under `swift test` on all 3 OSes (**G0, G1**). SwiftCrossUI probe on Linux GTK with AT-SPI tree (**G2**). Windows WinUI probe outcome recorded (**G3** start). Tokens generator. CI skeleton. `lint-imports`. Fixture format. API skeletons for M1 domains merged. |
| **M1 L1 wallet core** | wk 3–6 | `dw-engine` create/restore (incl. Core quirks, lookahead 1000), SPV sync, balances, history, receive, send (prepare/confirm/broadcast), IS/CL status. `dw-vault` (passphrase, lock states, grants, Touch ID). `dw-appdb` v1. `dwcli`. Regtest `l1` suite green. Swift: DashKit, WalletRuntime core, VMs Onboarding/Lock/Home/Send/Receive/Transactions. macOS + Linux UI for those screens. |
| **M2 dash-qt L1 parity** | wk 7–11 | QT shell/menus/status/tray/notifications. Coin control + locks + dust. Multiwallet, encrypt/change/unlock. Address book. Sign/verify. URI + OS handler. Tx table filters/CSV/details/abandon/resend. Options window. Tools (info/peers/repair/console-local). PSBT. dumpwallet + SQLite wallet.dat import/export. Backups. Windows UI shell (G3 decision applied). `restore` compat suite green. |
| **M3 Masternodes, governance, CoinJoin** | wk 9–16 (overlaps M2/M4) | `dw-p2p`, `dw-coinjoin` (regtest + testnet mixing), `dw-governance` (mainnet sync measured, **G7**), `dw-protx` (all wizards + shared MN), Masternodes & Governance tabs, gov clock, MN keychain/tracked MNs/evonode tools (IOS-080…083). Regtest `coinjoin`/`governance`/`protx` green (**G6**). |
| **M4 Platform parity** | wk 10–17 (parallel) | Identities, DPNS + contests + marketplace, DashPay contacts/profiles/notifications, invitations create/claim, Platform addresses (BLAST), shielded + internal transfer, token purchase approval, DashConnect (testnet). U5 workaround in place. |
| **M5 iOS services parity** | wk 14–20 | Rates/fiat everywhere, tax categories + CSV tax export + ZenLedger, Buy/Sell (Topper/Uphold/Coinbase/Dash DEX/Maya), Explore + DashSpend + gift cards, CrowdNode (flag), BIP70, phrase repair, sweep (Insight path), faucet, 43+21-locale localization, accessibility pass, dash-qt prefs import. |
| **M6 Hardening & release** | wk 19–24 | BDB wallet.dat, wallet.dat export (stretch), HWI signer, filter-scan sweep (U6), SPV context provider, proxy/Tor (U1), traffic graph (U2), Windows Hello, camera QR, perf (5k-tx history < 200 ms page), external audit (**G8**), signed/notarized packages on 3 OSes, auto-update, release candidates. |

### 5.2 Workstreams

At most ~10 are active at once. Each is one agent, or a lead plus a helper.

| WS | Scope (owns these paths) | Inputs | Outputs | Acceptance tests |
|---|---|---|---|---|
| **WS-01 Infra & Release** | `rust/Cargo.toml` (workspace + patches), `scripts/`, `.github/`, `packaging/`, `Apps/macOS/project.yml`, `Package.swift` (structure only) | this design | build-core on 3 OSes, artifact bundles, CI matrix, packaging pipelines, disk guard, fetch-core | G0/G1 probes green; `swift.yml` matrix green on an empty-feature build; signed DMG/MSIX/Flatpak of a hello app by M2 |
| **WS-02 Engine core (L1)** | `dw-engine` (except `src/platform/`), `dw-appdb`, `dw-ffi/src/{lib,engine,wallet,sync,send,coins,history,qr}.rs`, `dwcli` | research 01, platform-wallet & -storage APIs, platform-wallet-ffi as reference for guards | L1 engine + façade + CLI + fixtures | `cargo nextest -p dw-engine -p dw-appdb`; regtest `l1`; G5 restart equivalence; golden `tx_class.json`, `csv_export.json` |
| **WS-03 Vault & Auth** | `dw-vault`, `dw-ffi/src/vault.rs`, `WalletRuntime/Auth/`, `PlatformServicesMac/QuickUnlock*`, Windows Hello in `dw-desktop/src/hello.rs` | §1.8 | vault, grants, signer, Core-quirk seeds, lockout ratchet, quick unlock | vault tamper/rollback/wrong-pass tests; grant enforcement tests (spend over the biometric allowance rejected in Rust); BIP39 quirk vectors; Touch ID manual checklist |
| **WS-04 Compat & utilities** | `dw-compat`, `dw-uri`, `dw-message`, `dw-psbt`, `dw-sweep`, `dw-console`, `dw-chaindata`, `dw-ffi/src/{compat,console,uri,psbt}.rs` | research 02 §15, §18, §19; Core sources; Bitcoin Core 28 `migrate.cpp` | importers/exporters, URI, console, PSBT, sweep, phrase repair | `uri_cases.json`; dash-qt compat suite (§4.4); fuzz targets run 10 min/night without crash; console grammar tests from Core `rpcconsole` behaviour |
| **WS-05 MN & Governance** | `dw-p2p`, `dw-governance`, `dw-protx`, `dw-ffi/src/{governance,masternode}.rs` | research 02 §10–11; Core `governance/`, `evo/`, `qt/masternode*.cpp`, `sharedmn*.cpp` | gov sync/vote/create, ProTx builders, shared MN, MN list model | regtest `governance` + `protx`; `governance_objects.json` hashes equal to Core; G7 mainnet measurement report |
| **WS-06 CoinJoin** | `dw-coinjoin`, `dw-ffi/src/coinjoin.rs` | research 02 §9; Core `coinjoin/`; dashj | mixing client + status + progress | regtest `coinjoin`; `coinjoin_progress.json` matches dash-qt formula; testnet 4-round mix soak (24 h, no stuck reservations) |
| **WS-07 Platform engine** | `dw-engine/src/platform/**`, `dw-ffi/src/{identity,dpns,dashpay,shielded,platform_address,tokens,dashconnect,invitation}.rs` | research 01 §3, research 03 §1.9–1.13; platform-wallet APIs; iOS adapter files as behaviour reference | Platform façade with the same capabilities iOS uses | testnet nightly suite (identity register from Core/addresses/shielded; DPNS register + contest vote; contact request round-trip between two dwcli identities; shield/unshield; invitation create→claim) |
| **WS-08 Swift SDK, adapter, OS services** | `Sources/{DashKit,WalletRuntime (excl. Auth),PlatformServices*}`, `dw-desktop` (excl. hello) | façade APIs | EngineClient, EventBus, Host/LifecycleQueue/SPVCoordinator/…, tray/notify/autostart/single-instance/URI registration | `WalletRuntimeTests` on 3 OSes (lifecycle ordering, network switch, coalescing, syncDone semantics); single-instance + URI hand-off integration test per OS |
| **WS-09 App services & integrations** | `Sources/AppServices/**`, `dw-engine/src/explore.rs` | research 03 §1.4, §1.14–1.17, §2.6–2.7 | rates, HTTP client, BIP70, integrations, Explore, tax/CSV, notifications, faucet | `AppServicesTests` with recorded HTTP fixtures (no live calls in CI); BIP70 test vectors (iOS `BIP70_TESTING.md`); integration toggles hidden when keys absent |
| **WS-10 View models** | `Sources/WalletFeatures/**`, `Localization/en/*` | VM contracts per feature (`docs/contracts/vm-<feature>.md`), service protocols | every VM + routes + formatters + strings | `WalletFeaturesTests` on 3 OSes; ≥90 % line coverage on VMs; every QT/IOS item has at least one VM test named `test_QT_123_…` / `test_IOS_045_…` |
| **WS-11 macOS UI & design system** | `Sources/{DesignTokens,DashUIMac,MacUI}`, `Apps/macOS/` (code), DashUIKit fork PR | VMs, tokens, DashUIKit | all SwiftUI screens, menus, MenuBarExtra, Settings, XCUITests, snapshots | XCUITest critical flows; snapshot suite; VoiceOver audit checklist per screen |
| **WS-12 Cross UI** | `Sources/{DashUICross,CrossUI,DashWalletCross}`, SwiftCrossUI fork patches (a11y modifiers, DummyBackend export, #787 workaround) | VMs, tokens | all SwiftCrossUI screens + gallery | Linux AT-SPI smoke; `CrossUISmokeTests` on DummyBackend; Windows launch smoke; G3 report |
| **WS-13 QA, compat harness, L10n** | `regtest`, `compat`, `testdata/`, `dw-testkit`, `Localization/` (imports), checklist tracker `docs/parity.md` | research 02/03 checklists | harnesses, fixtures, golden vectors, translation import (iOS Transifex `dash-mobile-wallets` + Core `dash_en.xlf`, matched by English key), parity tracker | suites run in CI; tracker shows each QT/IOS item → test → status |

### 5.3 Conflict-avoidance rules (swarm protocol)

1. **Path ownership.** `docs/OWNERS.md` maps globs to WS, and a CI check rejects PRs that touch another WS's paths unless the PR carries a `contract-change` label and the owner's approval.
2. **Contract-first.** Before implementing, a WS lands its façade types and function signatures in `dw-ffi/src/api/<domain>.rs`. Bodies return `EngineError::NotImplemented` or fixture data. The PR also includes `docs/contracts/<domain>.md` and the regenerated bindings. View-model agents code against these immediately. A signature change after merge needs a `contract-change` PR that bumps `docs/contracts/CHANGELOG.md`.
3. **Append-only shared files.** These are the hot spots, and each uses per-feature files instead of edits to a shared list:
   - `dw-ffi/src/lib.rs` module list
   - `AppRoute` / `SidebarItem` — extend with `AppRoute+<Feature>.swift`
   - `AppEnvironment` — `AppEnvironment+<Feature>.swift`
   - L10n — one table per feature
   - `OWNERS.md`
4. **One PR per task, small** (≤600 changed lines excluding generated files). Branch naming `ws<NN>/<topic>`. Conventional commits.
5. **Generated artifacts are regenerated by scripts only:** UniFFI Swift, tokens. Never hand-edit. CI diff-checks.
6. **Local build hygiene for agents:**
   - Shared target dir.
   - Dev profile, host triple only.
   - Swift-only agents use `scripts/fetch-core.sh` (prebuilt bundle from CI) rather than compiling Rust.
   - Run `disk-guard` before builds.
   - Never `cargo clean` the shared dir.
   - SwiftPM and git operations run outside the sandbox.
7. **Definition of done** for any checklist item:
   - VM test plus engine test (where applicable).
   - Screen on macOS **and** Cross (or an explicit `ui-pending:<os>` tracker state).
   - Strings in L10n.
   - Accessibility label on icon-only controls.
   - Tracker row updated.
8. **Review:** every Rust PR in `dw-vault`, `dw-coinjoin`, `dw-protx`, `dw-compat` or `dw-engine::send` needs a second-agent review plus a citation of the reference implementation (Core file:line or platform-wallet-ffi function) for protocol and guard logic.

### 5.4 Checklist mapping — dash-qt (QT-001…154)

| IDs | Area | Primary WS | Support | M |
|---|---|---|---|---|
| QT-001 | single instance + URI hand-off | 08 | 11, 12 | M2 |
| QT-002 | per-network data/settings, `--testnet/--regtest/--devnet/--chain` | 08 | 02 | M1 |
| QT-003 | network branding / testnet units | 10 | 11, 12 | M1 |
| QT-004 | data-dir chooser | 10 | 08, 11, 12 | M2 |
| QT-005 | splash with phases + Q quit | 10 | 11, 12 | M2 |
| QT-006 | CLI flags (`--min`, `--resetguisettings`, `--lang`, `--windowtitle`, font overrides) | 08 | 10 | M2 |
| QT-007 | corrupt settings Reset/Abort | 08 | 10 | M2 |
| QT-008 | shutdown window, session end | 08 | 11, 12 | M2 |
| QT-009 | autostart | 08 | 01 | M2 |
| QT-010 | fatal/internal error dialogs | 10 | 11, 12 | M1 |
| QT-011…013 | title, geometry, sidebar tabs + shortcuts, no-wallet panel | 11, 12 | 10 | M2 |
| QT-014 | wallet selector | 10 | 02 | M2 |
| QT-015…018 | File / Settings / Window / Help menus | 11, 12 | 10 | M2 |
| QT-019 | drag-drop URI | 11, 12 | 04 | M2 |
| QT-020…022 | unit selector, HD icon, lock icon | 10 | 03 | M2 |
| QT-023 | proxy icon | 10 | 02 (U1) | M6 |
| QT-024, 025, 027 | connections, sync spinner/progress, sync overlay | 10 | 02, 08 | M1 |
| QT-026 | governance clock | 05 | 10 | M3 |
| QT-028…030 | tray icon/menu, minimize to tray/on close | 08 | 11, 12 | M2 |
| QT-031…033 | tx notifications, batching, CoinJoin suppression | 09 | 08, 02 | M2 |
| QT-034, 036…039 | balances, truncation/digits, out-of-sync, recent list, discreet mode | 10 | 02 | M1 |
| QT-035 | watch-only column | 10 | 02 | M2 |
| QT-040 | alert banner | 10 | 02 | M2 |
| QT-041…050 | CoinJoin panel, progress, fully-mixed rule, start/stop, protocol, settings, advanced UI, disable conditions, per-wallet state, status text | 06 | 10, 02 | M3 |
| QT-051 | CoinJoin send page | 06 | 02, 10 | M3 |
| QT-052…056 | recipients, fields, URI paste, validation, amount parsing | 10 | 02, 04 | M1 |
| QT-057, 058 | fee modes, caps | 02 | 10 | M2 |
| QT-059…063 | confirm dialog + countdown, duplicates, unlock, errors, after-send | 10 | 02, 03 | M1 |
| QT-064…067 | IS/CL semantics, spend zero-conf change, BIP69/nSequence, P2SH + reject Platform addr | 02 | 04 | M1 |
| QT-068…074 | coin control panel/dialog/menu/CJ filtering/size estimate/custom change/spent auto-unselect | 02 | 10 | M2 |
| QT-075 | dust protection + Unlock dust | 02 | 10 | M2 |
| QT-076…079 | PSBT controls, create unsigned, load, operations dialog | 04 | 10 | M2 |
| QT-080 | external signer (HWI) | 04 | 03, 10 | M6 |
| QT-081, 082, 084, 085 | receive form, request dialog, QR rules, URI generation | 10 | 02, 04 | M1 |
| QT-083 | requested-payments history | 02 | 10 | M2 |
| QT-086…094 | 19 tx types, status model, table, filters, context menu, abandon/resend, details, CSV, 3rd-party URLs | 02 | 10 | M2 |
| QT-095…098 | address books, selection mode, purposes/errors | 02 | 10 | M2 |
| QT-099, 100 | sign / verify message | 04 | 10 | M2 |
| QT-101 | multiwallet open/close/load-on-startup | 02 | 10 | M2 |
| QT-102, 103 | create dialog interlocks, mnemonic verify | 10 | 03, 02 | M1 |
| QT-104, 105 | mnemonic restore w/ Core quirks, 1000 lookahead | 04 | 02, 03 | M1 |
| QT-106 | wallet.dat restore (SQLite M2 / BDB M6) | 04 | 13 | M2/M6 |
| QT-107…109 | dumpwallet import, hdseed/xprv/listdescriptors import, export for dash-qt | 04 | 02 | M2 |
| QT-110, 116 | backup wallet, automatic backups | 02 | 03 | M2 |
| QT-111, 113 | encrypt/change/unlock/lock, show recovery phrase | 03 | 10 | M1 |
| QT-112 | unlock for mixing only | 03 | 06 | M3 |
| QT-114 | watch-only/blank/upgradetohd | 02 | 04 (U7) | M2 |
| QT-115 | migrate legacy | 04 | — | M6 |
| QT-117 | rescan birthday/full | 02 | 10 | M2 |
| QT-118, 120, 121, 123…127 | MN tab, owned detection, context menu, register wizard, operator-secret gate, update/revoke, shared MN create/maintain | 05 | 10, 03 | M3 |
| QT-119, 122 | PoSe/last paid/next payment, full details | 05 | 04 (chaindata) | M3 |
| QT-128…134 | governance tab, columns/status, context menu, vote, create, resume, info panel | 05 | 10 | M3 |
| QT-135…141 | options dialog, tabs, appearance, reset | 10 | 08, 11, 12 | M2 |
| QT-142 | import dash-qt prefs | 04 | 08 | M5 |
| QT-143 | information tab | 02 | 10 | M2 |
| QT-144 | info [F] fields | 04 | 05 | M3 |
| QT-145 | console (local M2; gov/MN/CJ commands M3) | 04 | 10 | M2/M3 |
| QT-146 | traffic graph | 02 (U2) | 10 | M6 |
| QT-147 | peers table, ban/unban | 02 | 10 | M2 |
| QT-148 | repair: rescan / reset chain data | 02 | 10 | M2 |
| QT-149 | URI parsing | 04 | — | M1 |
| QT-150 | OS URI registration, Open URI dialog | 08 | 01, 10 | M2 |
| QT-151 | 21 locales | 13 | 10 | M5 |
| QT-152 | units | 10 | 04 | M1 |
| QT-153 | help/about/CoinJoin info | 10 | 11, 12 | M2 |
| QT-154 | Core-format encryption for wallet.dat export | 04 | 03 | M6 |

### 5.5 Checklist mapping — iOS (IOS-001…123)

| IDs | Area | Primary WS | Support | M |
|---|---|---|---|---|
| IOS-001 | onboarding carousel + demo (fixture mode powers demo) | 10 | 11, 12 | M5 |
| IOS-002…004, 007 | create 12/24, backup warnings, verify chips, restore 10 languages | 10 | 03, 02 | M1 |
| IOS-005 | backup reminder | 10 | — | M2 |
| IOS-006 | screen-capture guard | 08 | 11, 12 | M2 |
| IOS-008 | phrase repair | 04 | 10 | M5 |
| IOS-009 | existing-wallet detection (vault inventory on first run / reinstall) | 03 | 10 | M2 |
| IOS-010…017 | credential set, biometrics, lockout + secure time, lock screen, forgot, auto-lock, spending confirmation/limits, auth gate | 03 | 10, 08 | M1 (010, 013, 017) / M2 (rest) |
| IOS-018 | lifecycle overlay | 08 | 10 | M1 |
| IOS-019…022 | balance hero, hide, breakdown, network badge | 10 | 02 | M1 (Platform/shielded parts M4) |
| IOS-023 | sync banners/peers/change peers | 10 | 08 | M1 |
| IOS-024 | rate warnings | 09 | 10 | M5 |
| IOS-025 | shortcut bar | 10 | 11, 12 | M2 (integration actions M5) |
| IOS-026 | time-skew dialog | 09 | 10 | M5 |
| IOS-027…030 | history by day, filters, rich rows, grouped rows | 10 | 02, 09 | M2 |
| IOS-031, 032 | tx details + explorer actions | 10 | 09 | M2 |
| IOS-033 | asset-lock recovery | 07 | 10 | M4 |
| IOS-034 | remove unconfirmed / drop & rescan | 02 | 09 | M2 |
| IOS-035 | shielded/Platform activity rows | 07 | 10 | M4 |
| IOS-036…040 | tax categories, reclassify intro, historical rate, CSV tax export, ZenLedger | 09 | 02 (appdb), 10 | M5 |
| IOS-041…046, 051, 052 | payments entry, address classification, QR capture, amount entry + fiat + Max, routes (Core), Core confirm, guards, success | 10 | 02, 04, 08 | M1 (QR capture M2) |
| IOS-045 (non-Core routes), 047 | Platform/shielded routes, step progress | 07 | 10 | M4 |
| IOS-048 | URI handling + OS registration | 04 | 08 | M1/M2 |
| IOS-049 | BIP70/72 | 09 | 02 | M5 |
| IOS-050 | pay DashPay contact | 07 | 10 | M4 |
| IOS-053…055 | receive toggle, rotation + watcher, request amount | 10 | 02, 07 | M1 (Core) / M4 (Platform, shielded) |
| IOS-056 | sweep (IN) | 04 | 10 | M5 (Insight) / M6 (filters) |
| IOS-057 | CoinJoin recovery + move mixed coins | 06 | 02, 10 | M3 |
| IOS-058 | CoinJoin mixing (IN) | 06 | 10 | M3 |
| IOS-059…064 | shielded balance/sync, shield/unshield, finish stuck, internal transfer, advanced mode, BLAST | 07 | 10 | M4 |
| IOS-065…077 | Join DashPay, username form, registration funding, contests, profile, credits withdraw, identities screen, contacts, add contact, contact profile, notifications, edit profile (avatar upload via 09), claim invitation | 07 | 10, 09 | M4 |
| IOS-078 | create/share/reclaim invitations (IN) | 07 | 10 | M4 |
| IOS-079 | contested-username voting | 07 | 05, 10 | M4 |
| IOS-080…083 | MN list/detail, evonode status/withdraw/unban, tracked MNs + vault, MN keychain viewer | 05 | 07, 03, 10 | M3 |
| IOS-084 | username marketplace | 07 | 10 | M4 |
| IOS-085, 086 | DashConnect + token purchase approval | 07 | 10 | M4 |
| IOS-087…094 | Buy & Sell, Topper, Uphold, Coinbase, CrowdNode (flag), Dash DEX sell/buy, swap orders | 09 | 10, 02 | M5 |
| IOS-095…103 | Explore, DB sync, merchants, ATMs, filters, POI, CTX, PiggyCards, gift cards | 09 | 10 | M5 |
| IOS-104, 105 | local currency, notification toggle | 10 | 09 | M2/M5 |
| IOS-106 | network switch + devnet settings | 08 | 10 | M1 |
| IOS-107 | About + tech info | 10 | 08 | M2 |
| IOS-108, 109 | security menu, wipe | 03 | 10 | M2 |
| IOS-110 | multi-wallet management | 02 | 10 | M2 |
| IOS-111 | xpub export | 02 | 10 | M2 |
| IOS-112 | export logs + support | 08 | 10 | M2 |
| IOS-113 | Core sync info, rescan options, birth height, drop unconfirmed | 02 | 10 | M2 |
| IOS-114 | Platform/DashPay/Shielded sync info | 07 | 10 | M4 |
| IOS-115 | storage explorer (dev toggle) | 02 | 10 | M5 |
| IOS-116 | local notifications + deep-link routing | 09 | 08 | M2 (tx) / M5 (rest) |
| IOS-117 | tray companion | 08 | 11, 12 | M2 |
| IOS-118 | 43 locales, plurals, RTL | 13 | 10, 11, 12 | M5 |
| IOS-119 | visual parity | 11 | 12 | M1→M5 |
| IOS-120 | accessibility labels | 11, 12 | 13 | M5 |
| IOS-121 | testnet faucet | 09 | 10 | M2 |
| IOS-122 | announcements channel | — deferred (§7) | — | post-1.0 |
| IOS-123 | secure seed storage | 03 | — | M1 |

---

## 6. Risks and go/no-go gates

| Gate | When | Test | Pass | Fallback if it fails |
|---|---|---|---|---|
| **G0** Rust stack on all OSes | M0 wk1 | `cargo build -p dw-ffi` (dev) for aarch64-apple-darwin, x86_64-unknown-linux-gnu (docker), x86_64-pc-windows-msvc (GH runner, with LLVM + protoc installed) | links. `nm`/`dumpbin` show `uniffi_*` symbols. A smoke test opens an Engine and creates a wallet offline. | Windows MSVC failures in `rs-x11-hash`/bindgen/halo2: (1) patch via `[patch]` (cc flags, pregenerated bindings). (2) Build Windows with `x86_64-pc-windows-gnu` as a **cdylib DLL**. The C ABI crosses toolchains, so Swift links an import library. This costs one extra DLL. |
| **G1** UniFFI Swift off-Apple | M0 wk1–2 | async fn + callback interface + records under `swift test` on Linux and Windows, Swift 6.3.3 | green | Hand-written cbindgen C façade with the same object model (handles + vtables) plus thin Swift wrappers. More code, same architecture. Decide by the end of M0. |
| **G2** SwiftCrossUI Linux | M0 | probe with List, Table, TextField, sheet, menu, Image (QR); AT-SPI dump under Xvfb | builds; labelled widgets visible in AT-SPI | Patch the fork (a11y modifiers). If GTK4 fundamentally fails: Linux ships the Rust egui UI over `dw-engine` directly. This is a last resort that duplicates the VM layer. |
| **G3** SwiftCrossUI Windows (WinUI #787) | M0 → decision end of M1 | Narrator/FlaUI walk of the probe; MSIX install on a clean VM with Windows App Runtime | no UIA crash; Text/Button/TextField exposed | (1) 2-week fork fix (create controls via `XamlReader.Load` so automation peers attach; track microsoft-ui-xaml#11028). (2) **Ship Windows on GtkBackend** (functional; a11y limited, disclosed). The VM layer stays shared. |
| **G4** Disk / build time | M1 | Measure a clean dev build and an incremental build of `dw-ffi`; a release build in CI | dev clean ≤ 15 min, incremental ≤ 3 min, local target dir ≤ 20 GB | `opt-level` tuning; `shielded` feature off in dev builds (`--no-default-features --features dev-lite`); Swift agents use prebuilt bundles only |
| **G5** Persistence equivalence | M2 | kill -9 at random points during sync/send/register → restart → state equals the reference (balances, history, identities, tokens, DashPay overlay) | equal after re-sync ≤ 60 s | Engine-side re-sync of the lossy domains (already planned) plus upstream U5 |
| **G6** CoinJoin interop | M3 | regtest mixing with dashd MNs, then a 24 h testnet soak | completes rounds, no stuck reservations or collateral loss beyond protocol fees | Ship mixing as "Experimental" (off by default) at 1.0; move-mixed-coins and balance stay GA |
| **G7** Governance SPV sync | M3 | mainnet `govsync` from 3 peers: bytes, time, tallies vs a reference node | ≤ 5 min on 50 Mbit, tallies within 1 % | Tallies shown only with the dashd data source; voting and proposal creation still work SPV |
| **G8** Security | M6 | external audit of vault/CoinJoin/ProTx/compat; secret-leak tests | no high findings open | release slips; no mainnet release without it |

**Other risks:**

| Risk | Mitigation |
|---|---|
| SwiftCrossUI bus factor ≈ 1, pre-1.0 churn | Fork under `dashpay/`, pin, upstream patches. Views stay thin, so all logic lives in VMs. |
| Divergence from platform-wallet-ffi guard logic | The citation rule in §1.3. Port the FFI's tests for the same scenarios into `dw-engine`. |
| Platform API churn on bumps | Pin to tags; bump PRs run the full suites; `PATCHES.md`. |
| Partner integrations (Uphold/Coinbase/Topper/SwapKit/CTX/PiggyCards) need desktop OAuth clients and redirect URIs | Request desktop client registrations at M3. Integrations are hidden automatically when keys are absent. Loopback-redirect OAuth (RFC 8252) for desktop. |
| Linux secret store absent (headless/minimal DEs) | Encrypted mode needs no OS store. Unencrypted mode is refused when Secret Service is unavailable, with a clear message. |
| Two view trees double UI effort | VM-first with fixtures. UI workstreams run in parallel. Component parity is enforced by the gallery. |
| Windows Swift runtime DLL weight / conflicts | MSIX isolation; ship the runtime DLLs privately; track static-stdlib progress (swiftlang/swift#83446). |
| GNOME lacks a tray | AppIndicator extension disclosed; tray is optional; minimize-to-tray disabled when no StatusNotifierWatcher is present. |

---

## 7. Product decisions taken here (resolving "decide" items)

1. **IOS-056 sweep: IN.** IOS-058 mixing: IN. IOS-078 invitation creation: IN. CrowdNode: behind the flag `crowdnode` (default off, auto-on for existing accounts detected on chain).
2. **IOS-122 announcements: deferred past 1.0.** CloudKit has no desktop equivalent, and an unsigned remote-message channel in a wallet is an attack surface. If added later: a signed JSON feed, verified against a pinned key.
3. **IOS-123 / QT-111:** passphrase vault by default, as described in §1.8. "Skip encryption" is allowed under Advanced in the create flow (dash-qt checkbox parity). In that case the DEK sits in the OS store and an optional app PIN is offered.
4. **QT-113: the mnemonic passphrase IS shown** behind a full unlock and its own reveal toggle. dash-qt's omission strands users who restore elsewhere.
5. **QT-116: automatic encrypted backups ON** (rotate 10). CoinJoin is never gated on backups for HD wallets (fixes dash-qt quirk #10).
6. **dash-qt quirks:**
   - Fixed: #1 (title text on all non-mainnet networks), #2, #6 (sending never overwrites our own receive labels; a send label only fills empty labels), #8 (the proposal payment date takes effect), #10, #15, #17.
   - Kept for parity: #7 (duplicate recipient is a confirmation, not an error).
   - #11: one threshold formula everywhere (Core's list-margin formula with the min-quorum floor), documented.
7. **Fees:** static policy + optional RPC (§1.14). There is no RBF (Dash has none).
8. **Regtest/devnet:** fully supported, hidden behind the Developer toggle (devnet also by CLI flag).
9. **Loose keys** from dumpwallet/wallet.dat/WIF are **swept** into the HD wallet, never kept as loose keys. One mnemonic restores everything.
10. **Multisig wallet creation:** out of scope (dash-qt has none). P2SH sending is supported. PSBT signing of foreign multisig inputs is limited to what key-wallet PSBT supports.
11. **Platform proof trust:** trusted HTTPS provider until M6, then SPV-backed with trusted fallback.
12. **Theme "Traditional" → "Native"** (stock controls). Montserrat / Roboto Mono font options kept.

---

## 8. Unverified items (verify in M0 or note as risk)

- UniFFI Swift output compiling and running on **Windows** with Swift 6.3.3 (G1). Linux is expected to work. The exact UniFFI version is still to be chosen.
- MSVC build of `rs-x11-hash` (bindgen/libclang), grovedb-commitment-tree and halo2 (G0).
- Exact Windows native-static-libs list.
- The release + LTO disk footprint (≈25 GB estimate).
- Whether `platform-wallet-storage`'s secrets API can be reused directly for multi-slot DEK wrapping, or only its primitives and keyring backends (§1.8).
- The "no change, excess to fee" policy (QT-051) through a finalize wrapper. Candidate restriction itself is verified (§2). If the wrapper isn't possible, U3 becomes blocking for the CoinJoin send page at M3.
- Regtest CoinJoin minimum participants and whether `DashTestFramework` can host mixing sessions out of the box.
- Mainnet governance object and vote volume (G7).
- GTK4 accessibility on Windows (GtkBackend fallback) — assume weak.
- `ksni` tray coexisting with the SwiftCrossUI GTK main loop. It is pure D-Bus, so it is expected to be fine.
- Flatpak portal camera access for webcam QR.
- `SMAppService`/autostart policy on macOS (hidden per dash-qt anyway).
- dash-evo-tool's consumption of `platform-wallet-storage` (taken from research 04, not re-checked).
