# dashwallet-desktop — architecture and delivery design (Fable)

Status: proposal, 2026-10-05. Author: lead-architect pass (Fable 5.1). Competing design by another architect to be reconciled.
Inputs: `docs/DESIGN-BRIEF.md`, `docs/research/01..04`. Code verified against `dashwallet-desktop-deps/platform` @ `bc321362b9` (v5.0-dev), rust-dashcore `e4208c90` (cargo checkout), `dashwallet-ios` @ `7c0064d6`, `dash` (Core) local checkout, `dash-evo-tool` @ `9d85c170`. Items I could not verify cheaply are tagged **UNVERIFIED**; a ledger is in §8.

---

## 0. Decisions (read this first)

| # | Decision | One-line reason |
|---|---|---|
| D1 | **One Rust engine archive, owned by this repo** (`dashwallet-engine`, staticlib + cdylib). It links the four upstream FFI crates unchanged and adds our `dwe_*` surface plus the gap crates. | Every gap (CoinJoin, governance, ProTx, compat, PSBT, BIP21/70, fee policy, persistence, secrets) is engine work; one archive keeps dedupe of secp/tokio/blst and one build pipeline. |
| D2 | **Persistence lives in Rust**: `platform-wallet-storage::SqlitePersister` (already a `PlatformWalletPersistence` impl, used by dash-evo-tool) via one contained upstream patch (U1) that lets the existing 415 `platform_wallet_*` functions run on a Rust-side persister. No host-side 46-slot vtable, no SwiftData. | Removes the single largest Apple-only coupling (11.6k-line Swift handler) and the historical source of iOS persistence bugs; identical store on all three OSes; headless-testable in Rust. |
| D3 | **Secrets live in Rust** (`dashwallet-vault`): Argon2id + XChaCha20-Poly1305 vault file under the *wallet passphrase* (dash-qt "Encrypt Wallet" semantics) + OS keyring (Keychain / Secret Service / Credential Manager) for the optional device-unlock key. Lock states Unencrypted / Locked / Unlocked / UnlockedForMixingOnly are engine states; the mnemonic resolver and Platform signer are Rust callbacks inside the engine. | Brief mandates passphrase-at-rest + OS store; `platform-wallet-storage::secrets` already provides the crypto and keyring backends for all three OSes. |
| D4 | **Swift binding = upstream split of SwiftDashSDK into `SwiftDashSDKCore` (portable) + `SwiftDashSDK` (Apple persistence/keychain), consumed from our platform fork branch; plus our own `DashEngineKit` for the `dwe_*` surface.** No SwiftData, Combine, CryptoKit, Security or os.log in the dependency path of the desktop app. | Keeps "same stack as iOS" (same Swift wrappers over the same FFI) while making the package compile on Linux/Windows. Gate G3 decides the fallback (own manager layer) by end of week 2. |
| D5 | **Rust is linked through SE-0482 `staticLibrary` artifact bundles on all three OSes** (universal macOS slice, linux-gnu x86_64 + aarch64, windows-msvc x86_64). No XCFramework. | Proven in `scratch/shared-vm-probe` on macOS; one mechanism, one build script, one CI artifact. |
| D6 | **UI: SwiftUI on macOS (flagship), SwiftCrossUI on Linux (GtkBackend) and Windows (WinUIBackend, gated by G2; fallback GtkBackend on Windows).** One shared `WalletCore` of `@MainActor @Observable` ViewModels; two thin view trees. Rust/egui is *not* a UI fallback. | Option B from research 04; both front ends consume the same stdlib `@Observable` objects (probe verified). The Windows fallback keeps the VM layer shared; an egui fallback would not. |
| D7 | **DashUIKit is the design system**: forked to add macOS support (SwiftUI), and re-implemented component-for-component in SwiftCrossUI (`DashUIKitCross`), with tokens generated from the asset catalogs. | Visual parity with iOS at the token/component level; "no AI purple", Dash blue `#008DE4`. |
| D8 | **Full-node-only dash-qt features are handled by an optional "Connect to my Dash Core node" link (`NodeLink`, JSON-RPC)** plus SPV-native substitutes: governance via P2P `govsync` (own `dash-governance` crate), masternode list via SML, a local wallet command console when no node is linked, a real RPC console when one is. Features that need a full node are labelled so in the UI, never faked. | Honest, and it gives power users everything dash-qt had. |
| D9 | **Platform proofs verify against SPV-derived quorum keys by default** (`dash_sdk_create_with_callbacks` → `SpvRuntime::get_quorum_public_key`, which exists), with the trusted HTTPS quorum service as a startup/opt-in fallback. | Desktop has the CPU, disk and uptime to be trust-minimised; iOS chose trusted for mobile reasons. |
| D10 | **App metadata (labels, address book, tax categories, gift cards, swap orders, vote history, requested payments) is a Rust SQLite (`dashwallet-appdb`)** next to the persister DB; UI preferences are a Swift JSON `settings.json` per network (dash-qt key names where they map). | Durable, migratable, cross-platform, joinable with wallet rows; lets the dash-qt import/export code read labels directly. |
| D11 | **Product scope**: CoinJoin mixing, governance (list/vote/create/resume), full ProTx suite incl. v24 shared masternodes, private-key sweep, invitation creation, PSBT, dumpwallet/wallet.dat import/export are **IN**. HWI external signer (QT-080), CrowdNode, and `wallet.dat` *writing* (QT-109d) are behind flags / stretch. | Brief defaults, plus explicit calls on the question marks. *(Amended by DEC-01, DASHPAY §5a: "full ProTx suite" means the owner-side flows in milestone MG; operator-signed ProUpServTx and ProUpRevTx stay with the node and its CLI.)* |
| D12 | **Networks**: mainnet default; testnet, devnet, regtest selectable in-app (not only via CLI as in dash-qt). Separate data dir, settings and stores per network. | iOS parity (network switch) and dash-qt parity (per-network settings). |
| D13 | **Toolchain pins**: Swift 6.3.3 everywhere (Xcode 26.6; Linux via swiftly; Windows via winget), Rust 1.98.1 (platform's `rust-toolchain.toml`), SwiftCrossUI 0.10.x pinned exact and forked under `dashpay/`. | One toolchain triple across CI. |
| D14 | **Delivery**: 7 milestones, 18 workstreams, 6–9 agents in parallel, interfaces frozen per milestone (`dwe.h` + JSON schemas + Swift protocols). | §6. |

---

## 1. Architecture

### 1.1 Layers

```
┌────────────────────────────────────────────────────────────────────────────┐
│ Apps                                                                       │
│  Apps/macOS/DashWallet (SwiftUI, Xcode target)   Apps/Desktop (SwiftCrossUI)│
│  views only: no SDK calls, no fee math, no auth calls (iOS rule #1)        │
├────────────────────────────────────────────────────────────────────────────┤
│ Design system                                                              │
│  DashUIKit (fork, +macOS)  │  DashUIKitCross (SwiftCrossUI)  │ DashDesignTokens│
├────────────────────────────────────────────────────────────────────────────┤
│ WalletCore  (Swift, Foundation + Observation only)                         │
│  @MainActor @Observable ViewModels, flows as state machines, services,     │
│  formatting, validation, navigation-as-data, PlatformServices protocols    │
├────────────────────────────────────────────────────────────────────────────┤
│ DashEngineKit (Swift)                                                      │
│  EngineRuntime (lifecycle queue)  EngineEvents  Vault  Query  Commands     │
│  + SwiftDashSDKCore (upstream split: KeyWallet/DPP/Address/Models/        │
│    Voting/Utils + PlatformWallet wrappers without SwiftData/Combine)       │
├──────────────────────────── C ABI ─────────────────────────────────────────┤
│ libdashwallet_engine.{a,lib}  (Rust; one archive; SE-0482 bundle)          │
│  dwe_* (ours)  │ platform_wallet_* core_wallet_* │ dash_sdk_* │ wallet_* … │
│  dashwallet-engine ──┬── dashwallet-vault  ── platform-wallet-storage::secrets│
│                      ├── dashwallet-appdb                                   │
│                      ├── dash-coinjoin-client  dash-governance             │
│                      ├── dash-protx-builder    dash-wallet-compat          │
│                      ├── dash-payments         dash-node-rpc (dashcore-rpc)│
│                      └── platform-wallet(-ffi) → dash-spv, key-wallet,     │
│                          dash-sdk, dpp; platform-wallet-storage::SqlitePersister│
└────────────────────────────────────────────────────────────────────────────┘
```

Languages per layer: Rust for everything that touches consensus, networking, keys, persistence and compatibility formats; Swift for ViewModels, services that are purely app-level (HTTP integrations, formatting, flows), and views. No C++/Obj-C. No JavaScript.

### 1.2 The engine (Rust)

**Crate `dashwallet-engine`** (`rust/crates/dashwallet-engine`), `crate-type = ["staticlib", "cdylib", "rlib"]`, output `libdashwallet_engine.a` / `dashwallet_engine.lib|dll`.

```toml
[dependencies]
rs-unified-sdk-ffi = { path = "../../../deps/platform/packages/rs-unified-sdk-ffi", features = ["shielded"] }
platform-wallet-ffi = { ..., features = ["shielded", "sqlite-persister"] }   # U1
platform-wallet-storage = { ..., features = ["sqlite", "secrets", "shielded"] }
platform-wallet = { ..., features = ["serde", "shielded"] }
dash-sdk = { ..., features = ["core_spv", "core_quorum-validation", ...] }
dashwallet-vault = { path = "../dashwallet-vault" }
dashwallet-appdb  = { path = "../dashwallet-appdb" }
dash-coinjoin-client = { path = "../dash-coinjoin-client" }
dash-governance = { path = "../dash-governance" }
dash-protx-builder = { path = "../dash-protx-builder" }
dash-wallet-compat = { path = "../dash-wallet-compat" }
dash-payments = { path = "../dash-payments" }
dash-node-rpc = { path = "../dash-node-rpc" }
```

`src/lib.rs` is `pub use rs_unified_sdk_ffi::*;` (so all ~894 upstream symbols stay exported from our archive, exactly as `rs-unified-sdk-ffi` does today) plus `pub mod ffi;` for ours.

**FFI style for `dwe_*` (ours).** Two tiers:

1. **Typed, zeroizing calls for secrets and lifecycle** (cbindgen C header `dwe.h`):
   ```c
   DweResult dwe_engine_create(const DweEngineConfig* cfg, DweEngine** out);   // data_dir, network, devnet, log cfg
   DweResult dwe_engine_start(DweEngine*);  DweResult dwe_engine_stop(DweEngine*);  void dwe_engine_destroy(DweEngine*);
   DweResult dwe_subscribe(DweEngine*, DweEventCallback cb, void* ctx, DweSubscription** out);  // JSON events
   DweResult dwe_vault_create(DweEngine*, const uint8_t* passphrase, size_t len, DweVaultPolicy policy);
   DweResult dwe_vault_unlock(DweEngine*, const uint8_t* passphrase, size_t len, DweUnlockScope scope /*Full|MixingOnly*/);
   DweResult dwe_vault_lock(DweEngine*);   DweResult dwe_vault_change_passphrase(...);  DweResult dwe_vault_encrypt(...);
   DweResult dwe_vault_quick_unlock(DweEngine*, const uint8_t* pin, size_t len);          // PIN/biometric fast path
   DweResult dwe_vault_reveal_mnemonic(DweEngine*, const uint8_t wallet_id[32], DweSecureBuf* out);  void dwe_secure_free(DweSecureBuf*);
   DweResult dwe_wallet_import_mnemonic(DweEngine*, const uint8_t* phrase, size_t len, const uint8_t* bip39_pass, size_t plen,
                                        DweImportOptions* opts /*birth height, core-compat seed, lookahead*/, uint8_t out_wallet_id[32]);
   DweResult dwe_wallet_create(DweEngine*, uint32_t word_count, DweLanguage, uint8_t out_wallet_id[32]);
   DweResult dwe_sweep_private_key(DweEngine*, const uint8_t* wif, size_t len, const uint8_t wallet_id[32], DweSecureBuf* out_plan_json);
   DweResult dwe_platform_wallet_manager(DweEngine*, PlatformWalletManagerHandle* out);  // the upstream Handle, for SwiftDashSDKCore wrappers
   DweResult dwe_sdk_handle(DweEngine*, const void** out);                               // the upstream dash_sdk handle
   ```
2. **A JSON command bus for everything else**:
   ```c
   DweResult dwe_invoke(DweEngine*, const char* method, const uint8_t* params_json, size_t len, DweString* out_json);
   void dwe_string_free(DweString*);
   ```
   Methods are namespaced strings (`"history.page"`, `"coinjoin.start"`, `"gov.vote"`, `"protx.register.prepare"`, `"compat.import_dumpwallet"`, `"node.rpc"`), params/results are serde structs with JSON Schema exported by `cargo run -p dashwallet-engine --bin dwe-schema` into `Swift/DashEngineKit/Schemas/*.json`; Swift `Codable` types are generated from them (`tools/gen-engine-types.swift`). Why a bus: ~150 new methods written by 6+ agents in parallel with **zero header merge conflicts**, and the same bus drives `dwe-cli` for headless tests and the local "command console" (QT-145 substitute). Secrets never travel the bus.

**Threading.** The engine owns one tokio runtime (reuses `platform-wallet-ffi`'s `runtime.rs` pattern). `dwe_invoke` is synchronous for reads (SQLite) and returns a `request_id` for long operations whose completion arrives as an event. Events: one callback thread → Swift marshals to `@MainActor`.

**Linking per OS** (`tools/build-engine.sh`, output `Artifacts/DashEngineFFI.artifactbundle`):

| OS | Triple(s) | Artifact | Notes |
|---|---|---|---|
| macOS 15+ | `aarch64-apple-darwin` + `x86_64-apple-darwin`, `lipo` → one universal `.a` | bundle variant `macos-arm64_x86_64`, `supportedTriples: ["arm64-apple-macosx","x86_64-apple-macosx"]` | Link `SystemConfiguration`, `Security`, `CoreFoundation` (reqwest native-tls). Headers are target-independent (cbindgen). |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | `.a` per triple | `OPENSSL_STATIC=1` + `openssl-src` vendored (dash-evo-tool recipe) so the archive has no libssl runtime dep; needs `protoc` and `libclang` at build time only. Build in `swift:6.3.3-noble` + rustup image. |
| Windows | `x86_64-pc-windows-msvc` (primary, G2) | `.lib` + `info.json` listing system libs `ws2_32 bcrypt ntdll userenv kernel32 advapi32 dbghelp crypt32 secur32` (from `--print native-static-libs`) | Rust `/MD` matches Swift. Fallback W2: `x86_64-pc-windows-gnu` **DLL** (dash-evo-tool proves the whole stack builds on windows-gnu) consumed via a `systemLibrary` target + import lib; safe because every FFI allocation is freed by a Rust `*_free` function (no CRT objects cross the boundary). |

Bundle `info.json` (one artifact, N variants) follows `scratch/shared-vm-probe/Artifacts/DashProbeFFI.artifactbundle/info.json`. The umbrella header `DashEngineFFI.h` `#include`s `dash-network.h`, `key-wallet-ffi.h`, `rs-sdk-ffi.h`, `platform-wallet-ffi.h`, `dwe.h` in that order (same as `build_ios.sh` `inject_modulemap`), `module.modulemap`: `module DashEngineFFI { umbrella header "DashEngineFFI.h" export * }`.

**Profiles.** `release` = `lto = "fat", codegen-units = 1, opt-level = 3, panic = "abort", strip = "symbols"` (mirrors `release-ios`). `dev-fast` = `opt-level = 1`, no LTO, incremental, for the laptop. All machines share `CARGO_TARGET_DIR=~/.cache/cargo-target/dashwallet-desktop` and `sccache` (G0).

### 1.3 Persistence

Three stores per network, all under `<AppData>/DashWallet/<network>/` (macOS `~/Library/Application Support/DashWallet/mainnet`, Linux `$XDG_DATA_HOME/DashWallet/mainnet`, Windows `%LOCALAPPDATA%\DashWallet\mainnet`), with a first-run directory chooser (QT-004):

| File | Owner | Content |
|---|---|---|
| `wallet.sqlite` (+ WAL) | `platform-wallet-storage::SqlitePersister` (`SqlitePersisterConfig::new(path)`, `JournalMode::Wal`, `auto_backup_dir = backups/`) | All `PlatformWalletChangeSet` domains: wallets, accounts, address pools, core txs/UTXOs/ISLocks/sync state, identities + keys (public), contacts, Platform addresses, asset locks, invitations, DPNS states, tokens, DashPay, tracked masternodes, shielded viewing keys/notes (feature `shielded`). 18 refinery migrations upstream. |
| `appdb.sqlite` | `dashwallet-appdb` (rusqlite + refinery) | `address_book(address, label, purpose, created)`, `tx_meta(txid, label, memo, tax_category, rate, rate_ccy, service, custom_icon)`, `requested_payments`, `utxo_locks(txid, vout, reason{manual,dust,reservation})`, `gift_cards`, `swap_orders`, `vote_history`, `gov_proposals_mine`, `coinjoin_salt`, `mn_shared_sessions`, `node_link`, `migrations`. |
| `spv/` | dash-spv `DiskStorageManager` | headers, filter headers, filters, MN state, peers, lock file. |
| `shielded_tree.sqlite` | platform-wallet shielded file store | Orchard commitment tree. |
| `backups/` | engine | SQLite online backups: pre-migration, pre-delete, on demand (QT-110), rotating 10 (QT-116 → "document why unnecessary": we *do* keep rotating backups of `wallet.sqlite` + `appdb.sqlite`; they contain no keys). |
| `settings.json` | Swift `PreferencesStore` | UI preferences, dash-qt key names where they map (`DisplayDashUnit`, `digits`, `fCoinControlFeatures`, `fShowMasternodesTab`, …) so QT-142 import is a key copy. |
| `vault/` | `dashwallet-vault` | `vault.dwv` (Argon2id + XChaCha20-Poly1305; `platform-wallet-storage::secrets::SecretStore::file`), `quick.dwq` (PIN-wrapped KEK + lockout state), never in backups. |

**How the engine wires the persister (upstream patch U1, the only non-additive-looking change, actually additive):** in `rs-platform-wallet-ffi`, introduce
```rust
pub enum HostPersister { Callbacks(FFIPersister), #[cfg(feature="sqlite-persister")] Sqlite(Arc<SqlitePersister>) }
impl PlatformWalletPersistence for HostPersister { /* delegate every method */ }
pub type PlatformWalletManagerHandle = HandleStorage<PlatformWalletManager<HostPersister>>;   // was <FFIPersister>
#[no_mangle] pub unsafe extern "C" fn platform_wallet_manager_create_with_sqlite(sdk_ptr, db_path, event_handler, out_handle) -> PlatformWalletFFIResult;
```
Verified: `handle.rs:150` aliases the handle to `PlatformWalletManager<FFIPersister>`, `PlatformWalletManager::new(sdk, persister: Arc<P>, app_handler)` (`manager/mod.rs:475`), and `SqlitePersister: PlatformWalletPersistence + Send + Sync` with an object-safety check in `platform-wallet-storage/src/lib.rs`. Cost: one new file, one alias change, `FFIPersister`-specific call sites behind a `match` (count UNVERIFIED; expected small — the manager is generic over `P` everywhere). Until merged upstream, the patch lives on our fork branch `desktop/v5.0-dev` of `dashpay/platform`, pulled in as the `deps/platform` submodule and `[patch]`-ed in `rust/Cargo.toml`.

**Reads.** The engine's query layer (`dashwallet-engine::query`) opens a second, read-only rusqlite connection to `wallet.sqlite` (WAL → concurrent readers) and `ATTACH`es `appdb.sqlite`, so history pages, UTXO lists with labels/locks/rounds, address books, and masternode "owned" detection are single SQL queries returning JSON rows. Schema is upstream-owned: `tests/schema_fingerprint.rs` pins the migration set (`platform-wallet-storage` exposes fingerprint helpers behind `__test-helpers`) and fails loudly on drift.

**Gate G5** (M1): a parity audit of `SqlitePersister` vs every `PlatformWalletChangeSet` field and vs the Swift handler's capability flags (`store_commits_inline`, `persistence_capabilities`, `persists_durably`); plus run platform-wallet's `RecordingPersister`/`FaultyPersister` lifecycle tests against `SqlitePersister`. Known gap to confirm: `pending_contact_crypto` (no FFI slot today; the Rust persister may or may not carry it — UNVERIFIED).

### 1.4 Secrets, lock states, authentication

`dashwallet-vault` (Rust):

```rust
pub enum LockState { Unencrypted, Locked, Unlocked, UnlockedForMixingOnly }
pub struct Vault { store: SecretStore /* file(vault.dwv, passphrase) */, keyring: Option<SecretStore> /* os() */, kek: Zeroizing<[u8;32]>, state: LockState }
pub trait SecretSource { fn mnemonic(&self, wallet_id) -> Result<Zeroizing<String>>; fn identity_key(&self, wallet_id, path) -> ...; fn tracked_mn_key(&self, net, protx, role) -> ...; }
```

- **Wallet passphrase = dash-qt semantics.** "Encrypt Wallet" (default on at creation, QT-102) wraps the vault KEK with Argon2id(passphrase). Declining = `Unencrypted`: the KEK is stored only in the OS keyring (`SecretStore::os()`, service `org.dash.DashWallet`, account `vault.kek.<network>`). Encrypting later keeps the same seed (QT-111). Change passphrase re-wraps only the KEK (same as Core's `mkey` re-encryption). No decrypt-to-unencrypted (Core parity).
- **Quick unlock (iOS semantics, optional):** a 4–8 digit PIN or OS biometric (Touch ID via `LocalAuthentication` on macOS, Windows Hello via `PlatformServicesWindows`, none on Linux) unlocks a keyring-held copy of the KEK, bounded by the iOS lockout policy (`3 free, 6^(n−3)·60 s, disabled at 8`, IOS-012) and a `SecureTime` ratchet fed by HTTPS `Date` headers (IOS-026). Biometric spend allowance (0.5 DASH default) and the 7-day PIN-freshness rule are engine policy, not UI policy.
- **Mixing-only unlock** (QT-112): the engine's `MnemonicResolver` answers derivation requests only for `AccountType::CoinJoin` and address-pool refills while in `UnlockedForMixingOnly`; `core_wallet_tx_builder_finalize` for a user send triggers a full-unlock prompt (event `vault.unlock_required{scope:Full}`), then returns to mixing-only.
- **Platform signing:** `dash_sdk_signer_create_with_ctx` is created *inside the engine* with Rust `extern "C"` thunks that read identity keys from the vault (replaces `KeychainSigner.swift`). Identity private keys are persisted by the resolver/persister pair into the vault (replaces `MnemonicResolverAndPersister.swift`).
- **One auth gate in Swift**: `AuthenticationGate` in WalletCore (watchdog 120 s) is the only path that calls `dwe_vault_unlock`/`quick_unlock`; every sensitive VM action (send, reveal phrase, keys, votes, wipe, exports) goes through it (IOS-017, iOS arch rule #5).
- Memory hygiene: `Zeroizing`, `memsec` from the storage crate; Swift never holds a mnemonic longer than one `withUnsafeBytes` scope (`DweSecureBuf` + `dwe_secure_free`).

### 1.5 Swift binding: solving "SwiftDashSDK on non-Apple"

Facts (verified): 154/257 files import SwiftData, all but six inside `Persistence/`; `PlatformWalletManager.swift` (3,232 lines, 7 `ModelContainer/Context` refs) is a Combine `ObservableObject`; `Package.swift` declares `.macOS(.v15)` and a binary XCFramework target; no `#if os()` anywhere. With persistence and secrets moved into the engine (D2/D3), the desktop needs **none** of `Persistence/`, `Security/`, `Core/Wallet/WalletStorage.swift`, `FFI/KeychainSigner.swift`, `FFI/MnemonicResolverAndPersister.swift`, `Services/DataManager.swift`.

**Upstream patch U3 — split `packages/swift-sdk` into two targets of the same package:**

| Target | Contents | Rules |
|---|---|---|
| `SwiftDashSDKCore` (new, portable) | `KeyWallet/`, `DPP/`, `Models/`, `Address/`, `Voting/`, `Utils/` (minus `TestnetFaucet` CryptoKit → swift-crypto), `Helpers/` (`WIFParser` CommonCrypto → swift-crypto), `Config/`, `DashNetwork.swift`, `ConcurrencyCompat.swift`, `SDK.swift` (os.log → `swift-log`), and `PlatformWallet/` wrappers (`ManagedCoreWallet`, `CoreTransactionBuilder`, `ManagedIdentity`, `IdentityManager`, `DpnsMarketplace`, `TokenActions`, shielded/address/DashPay sync wrappers) **with `PlatformWalletManager` refactored to `PlatformWalletManagerCore`**: `@Observable` (Observation), persistence injected as `enum PersistenceBackend { case hostCallbacks(PersistenceHandling) ; case engineSqlite(path: URL) }`, signer/resolver injected as protocols. | Imports allowed: Foundation, Observation, swift-log, swift-crypto, `DashSDKFFI`/`DashEngineFFI`. `#if canImport(Darwin)` only for `Darwin` vs `Glibc`/`ucrt`. CI job compiles it on `ubuntu-24.04` and `windows-latest`. |
| `SwiftDashSDK` (existing name, Apple) | `Persistence/`, `Security/`, `WalletStorage`, `KeychainSigner`, `MnemonicResolverAndPersister`, `DataManager`, `PlatformWalletManager` as a thin subclass/extension that supplies `.hostCallbacks(PlatformWalletPersistenceHandler)` and Keychain-backed signer/resolver. | Unchanged behaviour for dashwallet-ios. |

The binary target becomes selectable: the package keeps `DashSDKFFI.xcframework` for iOS and accepts an environment/`Package.swift` switch (`DASH_SDK_FFI_BUNDLE=1`) to use `DashEngineFFI.artifactbundle` instead; both expose the same C module name through a `DashSDKFFI` shim module. (**UNVERIFIED**: two binary targets with the same module name in one package are not allowed; the shim is a `systemLibrary`-free re-export module `DashSDKFFI` → `@_exported import DashEngineFFI`. Settle in G3.)

**Gate G3 (end of M0):** `SwiftDashSDKCore` compiles on Linux with a stub engine and `swift test` runs its offline `KeyWallet`/`DPP` tests. **Fallback if upstream refuses or the refactor exceeds 2 agent-weeks:** `DashEngineKit` vendors the pure wrapper directories by script (`tools/vendor-swift-sdk.sh`, tracked by upstream commit hash) and we write `EngineWalletManager` ourselves over the 415 `platform_wallet_*` functions; `PlatformWallet/` wrappers are vendored file-by-file as they are found SwiftData-free. The desktop never depends on the Apple target either way.

**`DashEngineKit`** (ours, `Swift/DashEngineKit`): `EngineRuntime` (serial async lifecycle queue: start host → SPV → Platform address sync; stop in reverse; `switchNetwork`, `switchWallet`, `addWallet`, `wipe` — the shape of `SwiftDashSDKWalletRuntime.swift`), `EngineEvents` (C callback → `AsyncStream<EngineEvent>`), `Vault` (typed wrappers over `dwe_vault_*`), `EngineQuery`/`EngineCommand` (generated Codable types over `dwe_invoke`), `EngineSDK` (hands the upstream `sdk`/manager handles to `SwiftDashSDKCore` types).

### 1.6 WalletCore: ViewModels, services, observation, events

Rules (from research 04 §4.2, enforced by a lint script `tools/lint-walletcore.sh` in CI):
1. `WalletCore` imports only `Foundation`, `Observation`, `DashEngineKit`, `SwiftDashSDKCore`, `swift-log`. Never SwiftUI/AppKit/UIKit/Combine/SwiftData/SwiftCrossUI.
2. State: `@MainActor @Observable final class`. No `ObservableObject`/`@Published`. No UI value types (`Color`, `Image`, `Font`); expose `String`, `Int64` duffs, `Decimal`, domain enums, `Data` for QR payloads.
3. Navigation as data: `enum Route` per area; both UIs own a `NavigationPath`.
4. Long work in actors/services; engine callbacks → `Task { @MainActor in vm.apply(event) }`.
5. Platform services behind protocols injected from a composition root (`AppContainer`), stubs for demo mode (IOS-001) and tests.

**Change feed.** One typed, debounced stream per domain (fixing the iOS debt noted in research 03 §2.3): `EngineChangeFeed` yields `.core(walletId)`, `.spv(SyncSnapshot)`, `.platform(walletId)`, `.shielded`, `.dashpay`, `.coinjoin(SessionStatus)`, `.governance`, `.masternodes`, `.vault(LockState)`, `.peers`. VMs re-query through `EngineQuery` on their domain's tick (coalesced; `reloadPassInFlight` pattern). Balances are pushed, not polled at 1 Hz.

**Sync gating** (iOS rule #6): `SyncingActivityMonitor` derives `.syncDone` from headers/filters/MN-list phases, never from dash-spv `state == .synced` (steady state is `waitForEvents`).

Module map (`Swift/WalletCore/Sources/WalletCore/`): `Lifecycle/` (AppRuntimeVM, OnboardingVM, WalletLifecycleOverlayVM, NetworkSwitchVM, WalletsVM), `Auth/` (AuthenticationGate, LockScreenVM, SecurityVM, PinPolicy), `Home/` (OverviewVM, BalanceVM, ShortcutsVM, SyncStatusVM, DiscreetMode), `History/` (HistoryVM, TxFilter, TxDetailVM, TaxVM, CSVExport), `Send/` (SendVM, RecipientEntry, FeeVM, CoinControlVM, ConfirmVM, PSBTVM, SweepVM, URIParser glue, BIP70VM), `Receive/` (ReceiveVM, RequestAmountVM, RequestedPaymentsVM, QRPayload), `AddressBook/`, `SignVerify/`, `CoinJoin/` (CoinJoinPanelVM, CoinJoinSettingsVM, CoinJoinSendVM), `Masternodes/` (MasternodeListVM, RegisterWizardVM, MaintenanceVMs, SharedMNSessionVM, MasternodeKeysVM, TrackedMasternodesVM), `Governance/` (ProposalListVM, VoteVM, CreateProposalVM, ResumeProposalVM, GovernanceInfoVM, GovernanceClockVM), `Platform/` (IdentityVM, DPNSVM, ContactsVM, ProfileVM, InvitationsVM, MarketplaceVM, ContestVotingVM, TokensVM, ShieldedVM, PlatformAddressesVM, InternalTransferVM, DashConnectVM), `Integrations/` (RatesService, UpholdVM, CoinbaseVM, TopperVM, CrowdNodeVM(flag), SwapVM, ExploreVM, DashSpendVM, ZenLedgerVM), `Tools/` (ToolsInfoVM, ConsoleVM, PeersVM, TrafficVM, RepairVM, NodeLinkVM, LogsVM, StorageExplorerVM(dev)), `Settings/` (OptionsVM per tab, PreferencesStore, DevnetSettingsVM), `Notifications/` (NotificationDispatcher, producers), `Formatting/` (DashUnits, AmountFormatter, FiatFormatter, DateGrouping), `Navigation/`.

### 1.7 UI per OS

| | macOS | Linux | Windows |
|---|---|---|---|
| Toolkit | SwiftUI (macOS 15+), Xcode app target | SwiftCrossUI 0.10.x `GtkBackend` (GTK 4) | SwiftCrossUI `WinUIBackend` (G2) → fallback `GtkBackend` |
| Components | `DashUIKit` fork (adds `.macOS(.v15)`, ports `Toast`/`SearchBar`/`AddressFieldView`/`BottomSheet`→sheet) | `DashUIKitCross` (same names/props/states, SwiftCrossUI) | same |
| Shell | `WindowGroup` + `Settings` scene + `MenuBarExtra` (tray, IOS-117/QT-028) + Commands (menus, QT-015..018) | SwiftCrossUI `WindowGroup`, menus via `Menu`/`commands`, tray via `PlatformServicesLinux` (StatusNotifier D-Bus) | tray via `PlatformServicesWindows` (Shell_NotifyIcon through swift-winrt/Win32) |
| Fonts | SF Pro (system) | Inter bundled (closest metrics) | Segoe UI Variable / Inter |
| QR | `CoreImage` | `qrencode` via Rust (`dash-payments::qr` → PNG bytes) | same |
| a11y | full | GTK4 AT-SPI on native widgets | weak (see G2) |
| Tests | XCUITest | dogtail/AT-SPI under Xvfb (best effort) | deferred |

Design language: tokens generated from `SharedAssets.xcassets` + DashUIKit `Media.xcassets` by `tools/gen-tokens.swift` into `DashDesignTokens` (`Color.dash.*`, `DashTextStyle` scale 34/28/22/20/17/16/15/13/12/11 with explicit line heights, icon catalogue as PDF→SVG). Desktop-specific idioms (dash-qt parity): a left sidebar with Overview / Send / Receive / Transactions / CoinJoin* / Masternodes* / Governance* / Contacts(DP) / Explore (Cmd/Alt+1…N renumbered by visibility, QT-012), a status bar (unit selector, HD icon, lock icon, proxy icon, peers icon, governance clock, sync spinner/progress, QT-020..027), window title `Dash Wallet - <wallet> - [network]`. Themes: Light (default) / Dark / System; dash-qt's "Traditional" is dropped (no CSS layer to map it to). Discreet mode (Ctrl/Cmd+Shift+D) masks the Overview only (QT-039).

The Cross app also builds on macOS with `AppKitBackend` (the probe does), so agents can develop and screenshot the Linux/Windows UI on the Mac before the Linux/Windows runners exist.

### 1.8 Trust model, networking, full-node honesty

- **Core P2P:** dash-spv via `platform_wallet_manager_spv_start` (data dir, peers, `restrict_to_configured_peers`, devnet LLMQ params). Masternode sync forced on (needed for ChainLocks/ISLocks and CoinJoin). SOCKS5/Tor proxy settings (QT-138) map to dash-spv `ClientConfig` (**UNVERIFIED** that dash-spv has a proxy option; if not, upstream patch U7b adds `socks5_proxy: Option<SocketAddr>` to `PeerNetworkManager` using `tokio-socks`).
- **Platform (DAPI):** `dash_sdk_create_with_callbacks` with a context provider that answers quorum-key lookups from `SpvRuntime::get_quorum_public_key` once the MN list is synced, and from the trusted HTTPS service (`quorums.<net>.networks.dash.org`) before that or when the user picks "Trusted quorum service" in Settings → Network. The engine exposes `platform.proof_source` in diagnostics so the UI can show which one is active. (**UNVERIFIED**: `dash_sdk::SdkBuilder::with_context_provider` accepts a custom provider in the Rust API — it does in rs-sdk generally; the FFI variant `dash_sdk_create_with_callbacks` is verified to exist.)
- **NodeLink** (`dash-node-rpc`, wraps rust-dashcore's `rpc-client`/`rpc-json`, verified present in the pinned tree): Settings → Network → "Connect to my Dash Core node" (URL, cookie file or user/pass, TLS optional). When linked: masternode PoSe/last-paid/next-payment columns (QT-119/122), governance tallies/fundable set via `gobject list`/`getsuperblockbudget` (QT-128/129/134), mempool and credit-pool stats (QT-144), `estimatesmartfee` (QT-057), and a **real RPC console** (QT-145) with the dash-qt redaction list. When not linked those UI areas show "needs a Dash Core node" with a link to the setting — never fabricated values (iOS rule #7).
- **Governance without a node:** `dash-governance` syncs objects and votes over P2P `govsync`/`govobj`/`govobjvote` from full-node peers (feasible per research 02 §11.5; performance UNVERIFIED — bounded by syncing votes only for proposals in the current cycle).
- **External HTTP** (Swift `HTTPClient`, one typed client with endpoint enums, ETag cache, `Date`-header hook): hosts exactly as research 03 §1.20; no Firebase SDK — the Explore DB is fetched from the same GCS bucket over plain HTTPS (`storage.googleapis.com/download/storage/v1/b/dash-wallet-firebase.appspot.com/o/explore%2Fexplore-v4.db?alt=media`; **UNVERIFIED** that the bucket allows unauthenticated GET — fallback: mirror on `dashhq.org`).
- **Announcements** (IOS-122): a signed JSON feed on GitHub Pages (`PastaPastaPasta/dashwallet-desktop-announce`, Ed25519 signature verified in the engine) replaces CloudKit.

### 1.9 Process model

- Single instance per data dir (QT-001): engine takes the dash-spv lock file; a second launch with `dash:` URIs forwards them over a local socket (`PlatformServices.SingleInstance`: Unix domain socket / Windows named pipe) and exits.
- Tray/menu-bar companion (IOS-117): balance (respects autohide), receive QR, request amount, Send/CoinJoin/Receive/Sign/Verify/Options/Tools/Exit (QT-029), minimize-to-tray/on-close on Linux/Windows (QT-030), start-on-login (QT-009; LaunchAgent on macOS too — dash-qt hid it, we don't).
- Background sync while the window is closed is allowed only while the vault is `Unlocked`/`Unencrypted` or the user opted into "sync while locked" (public data only — SPV needs no keys; address-pool refills that need keys queue until unlock). Notifications via `UNUserNotificationCenter` / D-Bus `org.freedesktop.Notifications` / WinRT toasts.
- Shutdown window that cannot be closed (QT-008): `EngineRuntime.stop()` with the platform-wallet stop timeouts (15 s SPV join); session-end blocking on Windows via `ShutdownBlockReasonCreate`.

---

## 2. Gap-filling plan

Principle: **everything consensus- or wire-format-bearing is a Rust crate in this repo with exhaustive tests; upstream patches are additive, feature-gated, and small; our Cargo workspace `[patch]`es the platform fork so we are always buildable.** Fork branches: `PastaPastaPasta/platform:desktop/v5.0-dev` (rebased weekly by WS-00 onto `dashpay/platform:v5.0-dev`), `PastaPastaPasta/rust-dashcore:desktop/<pin>` only if needed.

| Gap | Where it is built | Design | Upstream touch |
|---|---|---|---|
| **CoinJoin mixing client** (QT-041..051, IOS-057/058) | `rust/crates/dash-coinjoin-client` | `codec/`: `dsa`(`CCoinJoinAccept`: denom, collateral tx), `dsq`(`CCoinJoinQueue`: denom, masternodeOutpoint, nTime, fReady, BLS sig), `dsi`(`CCoinJoinEntry`: inputs ≤9, collateral, outputs), `dsf`(session id + final tx), `dss`(signed inputs), `dsc`(session id, msg id), `dssu`(session id, state, msg id), `dstx`(tx, MN outpoint, BLS sig, time), `senddsq`. `session/`: state machine per session (Idle → Queue → AcceptingEntries → Signing → Complete/Error), 30 s queue / 15 s signing timeouts, 3–20 participants mainnet / 2–20 testnet, multi-session 1–10. `denoms.rs`: the five denominations (10.0001 … 0.00100001 DASH), bitmask (`1<<i`, i=0 largest), collateral 0.0001–0.0004, min balance 0.00140001, V24 `PROMOTION_RATIO=10`/`GAP_DIVISOR=5`. `rounds.rs`: rounds tracking via input-chain walk (dash-qt `GetRealOutpointCoinJoinRounds`), **"fully mixed" = rounds ≥ N and (rounds ≥ N+3 or odd `SHA256(outpoint‖cj_salt)`)** with the per-wallet salt in `appdb.coinjoin_salt` (imported from `cj_salt` on wallet.dat import so balances match Core, QT-043). `progress.rs`: dash-qt's exact weighted formula (QT-042). `planner.rs`: create-denominations / make-collaterals / collateral-change txs, denominations goal 50 / hard cap 300, target amount. `mn_select.rs`: pick from SML (verified `qrinfo`/`mnlistdiff` engine) with the "used masternodes" ring, verify `dsq` operator BLS sig against the SML entry, approximate own-collateral validity (confirmed, right amount, not spent in our view). Mixing outputs go to the **ordinary external chain `m/44'/c'/0'/0/i`** when "Core-compatible" (default, QT-018.3 compat) — the key-wallet `CoinJoin` account (`m/9'/c'/4'`) remains for dashj-style wallets and recovery (gap 100). Network: own outbound connections to the session masternode using `dash_spv::network::Peer` (pub), plus `dsq` subscription via a message tap on the SPV pool (U2). Masternode-side test double in `tests/mock_mn.rs`. | **U2** rs-platform-wallet `SpvRuntime::{message_tap(types), send_to_peer, masternode_list_engine, peer_stats}` + dash-spv `DashSpvClient::subscribe/network_handle` (additive). Verified: `NetworkMessage::Unknown{command,payload}` is forwarded to the client (`manager.rs:900`) and `MessageType::Unknown` is subscribable. |
| **Governance** (QT-026, 128–134) | `rust/crates/dash-governance` | `object.rs` (`CGovernanceObject` fields incl. `vchData` JSON with the exact key order `name,payment_address,payment_amount,url,start_epoch,end_epoch,type`), `vote.rs` (`CGovernanceVote{masternodeOutpoint, nParentHash, nVoteOutcome, nVoteSignal, nTime, vchSig}` — ECDSA for regular voting keys; BLS vector length `BLS_SIG_SIZE` per Core `vote.h:198`), `sync.rs` (`govsync` → inv → `govobj`/`govobjvote`, per-object vote fetch, dedupe, validity), `tally.rs` (Y/N/A, margin vs `max(minQuorum, weightedMNs/10)`, Evo weight 4, statuses Funded/Lapsed/Confirming/Pending/Passing/Failing/Voting/Unfunded), `clock.rs` (cycle 16616/1662 mainnet, 24/8 test/dev, 20/10 regtest; superblock ETA, budget committed), `proposal.rs` (1 DASH `OP_RETURN <hash>` collateral tx via `core_wallet_tx_builder_add_op_return`, `gobject prepare`/`submit` equivalents, resume at ≥1 conf, stored in `appdb.gov_proposals_mine`; epoch bug of dash-qt **not** copied: we honour the chosen payment date). Vote signing with the owner-wallet voting keys from `AccountType::ProviderVotingKeys` or tracked-MN keys; "voting too often" (1 h) enforced client-side with the server rejection surfaced. `NodeLink` source for tallies when linked. | none (uses U2 tap + `broadcast`). |
| **ProTx builders** (QT-123–127) | `rust/crates/dash-protx-builder` | Payload structs exist in `dash` (`provider_registration.rs` incl. `payload_collateral_string`, `provider_update_{service,registrar,revocation}.rs`, `BLSSignature`); we add: `ProRegTxBuilder` (type MN/Evo, collateral: fund-new / existing UTXO / external with `MakeSignString` proof `payout|operatorReward|ownerAddr|votingAddr|payloadHash` signed by the collateral key or pasted), v24 fields (`netInfo` address lists, Platform P2P/HTTPS, node id), fee source address, `ProUpRegTxBuilder` (owner-key ECDSA sig, only changed fields), `ProUpRevTxBuilder` (reason 0–3, operator BLS sig), `ProUpServTxBuilder` for the non-revive case (reuse `PW/masternode/update_service.rs` signing path), v24 `ProUpShareTx`/`ProUpSharedRegistrar` + `shared_session.rs` (JSON envelope `dash-shared-mn-session` v1, fingerprint `XXXX-XXXX`, session code, 2–8 shares ≥100 DASH = 1000, early period ≤420480, three-round coordinator/participant state machines, 2 MiB cap, cross-network refusal, persistent coin reservations in `appdb.utxo_locks`). BLS basic scheme via `blsful` (already in graph). Operator secret: shown once with `masternodeblsprivkey=` line, "type last 4 chars" gate, never persisted (QT-124). | Possibly rust-dashcore: `ProUpShareTx` payload types if absent at the pin (**UNVERIFIED**; grep showed only the four ProTx + asset lock/unlock, coinbase, mnhf, quorum commitment → likely **needs U8** in rust-dashcore `special_transaction/provider_update_share.rs`). |
| **BIP21 / URIs** (QT-149/150, IOS-048) | `dash-payments::uri` | Core-exact parse (`dash:` only, reject `dash://`, `req-*` unknown → reject, `IS` ignored, amount always DASH, last repeated key wins, trailing `/` trimmed) **extended** with the iOS keys (`r`, `sender`, `user`, `currency`, `local`, `pay:`/`dashwallet://`/`dashpay://`/`dashid:`/`dash-key:`/`dash-st:` schemes) in a second, superset parser; generation in dash-qt's fixed order and 255-char QR limit. | none |
| **BIP70/72** (IOS-049; dash-qt dropped it, CTX gift cards need it) | `dash-payments::bip70` | protobuf (`prost`) PaymentRequest/Payment/PaymentACK, X.509 chain verification with `rustls-webpki` + OS roots (`rustls-native-certs`), network/expiry checks, "sign now, submit on ACK" via `core_wallet_signed_payment_finalize_with_deliverable` + `_broadcast/_release` (verified exports). | none |
| **Message verify** (QT-100) | `dash-payments::sign_message` | `dash::sign_message::{MessageSignature::from_base64, is_signed_by_address}` with `DASH_SIGNED_MSG_PREFIX = "\x19DarkCoin Signed Message:\n"` (verified). Sign uses `core_wallet_sign_message`. | none |
| **Fee policy** (QT-057/058) | `dash-payments::fees` | Dash has a flat market and SPV has no estimator: "Recommended" = 1000 duff/kB (`FeeRate::normal`) shown honestly as "network minimum"; confirmation-target picker is shown **only** when NodeLink supplies `estimatesmartfee`; "Custom per kB" clamped ≥1000; caps 0.1 DASH max + absurd-fee check; `feefilter` from peers (parsed by dash-spv, unused) raises the floor when seen. | none |
| **Coin control, UTXO locks, dust protection** (QT-068–075) | engine `query` + `appdb.utxo_locks` + `core_wallet_tx_builder_add_inputs_from_outpoints`/`use_only_added_inputs` | Locks persist in appdb and are passed as an exclusion set to the builder (the builder has no lock concept: **the exclusion set is applied by selecting inputs ourselves** when any lock exists — `use_only_added_inputs` with our own selection honouring strategy, or U9: `core_wallet_tx_builder_exclude_outpoints`, preferred). Dust protection: engine watches incoming foreign UTXOs ≤ threshold (default 10,000 duffs) and locks them with reason `dust`; "Unlock dust UTXO" clears. Size estimate 148/34/10 formula. Custom change address via `set_change_address`. | **U9** (small, preferred). |
| **PSBT** (QT-076–079) | `dash-payments::psbt` over `key-wallet::psbt` (verified: `from_unsigned_tx`, `combine`, `sign`, `extract_tx`, `fee`) | Create-unsigned from the builder's unsigned tx (needs the builder to expose an unsigned draft: **U10** `core_wallet_tx_builder_build_unsigned` or use key-wallet's `build_unsigned_reserved` directly from our crate on a watch-only wallet), load (binary/base64, <100 MiB), analyse, sign with vault keys, broadcast. HWI (QT-080) is a flagged stretch: PSBT round-trip through `hwi` CLI if present. | U10 |
| **Sweep private key / paper wallet** (IOS-056, dash-qt `importprivkey`) | engine `dwe_sweep_private_key` → `dash-payments::sweep` | WIF/BIP38 (`key-wallet-ffi` bip38) decode → scan the address's UTXOs (SPV: temporary filter match via `platform_wallet_manager_spv_rescan_filters` on an ad-hoc watch script — **U11** `spv_scan_scripts(scripts, from_height)`; fallback NodeLink `scantxoutset`/Insight API) → build a tx spending them to the wallet's next receive address (`key-wallet` `add_inputs` + PSBT `sign` with the loose key) → confirm → broadcast. Loose keys are never stored. | U11 |
| **dash-qt BIP39 quirks + seed** (QT-104/105) | `dash-wallet-compat::bip39_core` | `weak_checksum_ok(words)` (XOR mask `2 ^ cs_len`), `core_seed(mnemonic, passphrase)` = PBKDF2-HMAC-SHA512(bytes, `("mnemonic"+pass)[..256]`, 2048) **without NFKD**; import path: strict BIP39 first, else Core-weak with warning; wallet created via `platform_wallet_manager_create_wallet_from_seed_with_birth_height` (verified) with lookahead 1000 on `44'/c'/0'/{0,1}` and `9'/c'/4'/0'/0` (**U12**: per-wallet gap-limit override in key-wallet/platform-wallet; today 30/30 + CoinJoin 100 fixed in `gap_limit.rs`). | U12 |
| **dumpwallet import/export, descriptors JSON** (QT-107–109) | `dash-wallet-compat::dumpfile` | Parser for the header (`# mnemonic:`, `# mnemonic passphrase:`, `# HD seed:`, `# extended private masterkey:`, counters) and key lines (`WIF time [label=|reserve=1|change=1] # addr= hdkeypath=`), scripts (`script=1`); import = rebuild HD from mnemonic/seed/xprv (`from_seed_bytes`/`from_extended_key` exist), labels → appdb, birthday = earliest key, loose non-HD keys → offered **one-click sweep** (no loose-key account in the engine, by design). Export = dumpwallet-format text (labels %xx-encoded), `importdescriptors` JSON with checksums, and the "Blank wallet + `upgradetohd`" instructions card. | none |
| **wallet.dat import** (QT-106/115/154) | `dash-wallet-compat::walletdat` | Format sniff (BDB magic `62 31 05 00` @12; SQLite header + `application_id` network magic @68), SQLite reader of `main(key,value)`; **own read-only BDB 4.8 btree page parser** (`bdb.rs`: page header, overflow pages, key/value pairs — G7 gate, ~1.5k lines + fixtures generated by dashd in Docker); record decoders for `mkey`, `(c)hdchain`, `(c)key`, `walletdescriptor*`, `name`, `purpose`, `keymeta`, `hdpubkey`, `cscript`, `watchs`, `lockedutxo`, `flags`, `cj_salt`, `g_object`; Core encryption (`sha512` EVP_BytesToKey-style loop with `iterations`, AES-256-CBC, IV `Hash(pubkey)[0:16]`) to decrypt with the passphrase; seed lookahead from `mapAccounts[0]` counters / descriptor `next_index`. Writing a SQLite descriptor `wallet.dat` (QT-109d) is a stretch behind a flag; the encryption writer (QT-154) only ships if the writer ships. | none |
| **Watch-only wallets, xpub import** (QT-114) | engine `wallet.import_xpub` | key-wallet `Wallet::from_xpub`/`new_watch_only` exist; FFI lacks a first-class "add watch-only wallet" → **U13** `platform_wallet_manager_create_wallet_watch_only(xpub, birth_height)`. | U13 |
| **Address book** (QT-095–098) | `appdb.address_book` + engine `addressbook.*` | purpose `send`/`receive`/`unknown`, dash-qt duplicate rules and error strings, CSV export, selection mode; "sending overwrites labels" quirk **not** copied (we only fill empty labels). | none |
| **Requested payments** (QT-083) | `appdb.requested_payments` | | none |
| **Notifications batching** (QT-031–033) | WalletCore `NotificationDispatcher` | 100 ms batching, ≥100 → summary, suppressed during initial sync, CoinJoin popups toggle. | none |
| **Rescan / repair** (QT-117/148, IOS-113) | engine over `spv_rescan_filters`, `spv_clear_storage`, birth-height edit | | none |
| **Peers / traffic / ban** (QT-146/147) | U2 `peer_stats` + dash-spv `disconnect_peer` (verified) + reputation/ban (dash-spv has `reputation.rs`; ban durations → **U7** expose `ban_peer(addr, until)`) | | U7 |

**Upstream patch register** (all on the fork branch, each a separate PR to `dashpay/platform` or `dashpay/rust-dashcore`): U1 HostPersister + `create_with_sqlite`; U2 SPV message tap/peer stats/MN engine accessor; U3 swift-sdk split; U7 ban/proxy/peer stats in dash-spv; U8 ProUpShareTx payloads (rust-dashcore, if missing); U9 builder `exclude_outpoints`; U10 builder unsigned draft; U11 ad-hoc script scan; U12 per-wallet gap limits; U13 watch-only wallet import; U6 (nice-to-have) reqwest rustls feature. Rule: no patch may change an existing exported symbol's signature.

---

## 3. Repository layout, build, packaging, CI

```
dashwallet-desktop/
├── Package.swift                        # ONE SwiftPM manifest for all Swift targets (see below)
├── rust/
│   ├── Cargo.toml                       # workspace; [patch."https://github.com/dashpay/platform"] → deps/platform/packages/*
│   ├── Cargo.lock  rust-toolchain.toml (1.98.1)  .cargo/config.toml (target dir, sccache, per-target linker/system libs)
│   └── crates/
│       ├── dashwallet-engine/           # staticlib+cdylib; src/{lib.rs, engine.rs, runtime.rs, events.rs, query/, commands/, ffi/{dwe.rs, secure.rs}}; cbindgen.toml; build.rs; bin/dwe-schema.rs
│       ├── dashwallet-vault/            # vault.rs, lock.rs, quick_unlock.rs, lockout.rs, secure_time.rs, resolver.rs, signer.rs
│       ├── dashwallet-appdb/            # schema/, migrations/V001__initial.rs…, address_book.rs, tx_meta.rs, utxo_locks.rs, gift_cards.rs, swap_orders.rs, votes.rs
│       ├── dash-coinjoin-client/        # codec/, session/, denoms.rs, rounds.rs, progress.rs, planner.rs, mn_select.rs, tests/mock_mn.rs
│       ├── dash-governance/             # object.rs, vote.rs, sync.rs, tally.rs, clock.rs, proposal.rs
│       ├── dash-protx-builder/          # register.rs, update_registrar.rs, update_service.rs, revoke.rs, shared/{envelope.rs, session.rs, dissolve.rs}
│       ├── dash-wallet-compat/          # bip39_core.rs, dumpfile.rs, descriptors.rs, walletdat/{sniff.rs, sqlite.rs, bdb.rs, records.rs, crypt.rs}
│       ├── dash-payments/               # uri.rs, bip70/, sign_message.rs, fees.rs, psbt.rs, sweep.rs, qr.rs, units.rs
│       ├── dash-node-rpc/               # client.rs (dashcore-rpc), console.rs (nested-call parser, redaction), fallbacks.rs
│       └── dwe-cli/                     # headless CLI over the engine: `dwe --network regtest --data-dir … invoke history.page '{...}'`
├── deps/
│   └── platform/                        # git submodule → PastaPastaPasta/platform@desktop/v5.0-dev (sparse: packages/{rs-*,swift-sdk,dapi-grpc,...})
├── Swift/
│   ├── DashEngineKit/Sources/{DashEngineKit, DashEngineFFIShim}, Schemas/*.json, Generated/*.swift
│   ├── WalletCore/Sources/WalletCore/…  (module map in §1.6)
│   ├── PlatformServices/                # protocols: SecretUnlockUI, Clipboard, URLOpener, Notifications, Tray, SingleInstance, FileDialogs, Biometrics, Autostart, Camera, Screenshot, PowerAssertion
│   ├── PlatformServicesApple/  PlatformServicesLinux/  PlatformServicesWindows/
│   ├── DashDesignTokens/                # generated: Colors.swift, Typography.swift, Icons/ (SVG), tokens.json
│   └── DashUIKitCross/                  # SwiftCrossUI components mirroring DashUIKit names
├── Apps/
│   ├── macOS/DashWallet.xcodeproj, DashWallet/ (App.swift, Scenes/, Views/…, Resources/, Info.plist, DashWallet.entitlements), DashWalletUITests/
│   └── Desktop/Sources/DashWalletDesktop/ (SwiftCrossUI app: App.swift, Views/…), Resources/
├── Artifacts/DashEngineFFI.artifactbundle/   # gitignored; built by tools/build-engine.sh or fetched by tools/fetch-engine.sh
├── tools/ build-engine.sh fetch-engine.sh gen-tokens.swift gen-engine-types.swift vendor-swift-sdk.sh lint-walletcore.sh l10n/ (tx pull/push, strings→.xcstrings/.po)
├── tests/ regtest/ (docker-compose.yml: dashd v24 + v23.1.8 + dashmate local devnet; harness.py) compat/ (dash-qt round-trip suites) fixtures/
├── packaging/ macos/ (entitlements, Sparkle later, notarize.sh, create-dmg) windows/ (wix/Product.wxs, bundle.wxs: Swift runtime + WinAppRuntime chain) linux/ (org.dash.DashWallet.yml flatpak, AppImage recipe, .desktop, metainfo.xml, icons)
├── docs/ (design/, research/, adr/, checklists/ QT.md IOS.md)
└── .github/workflows/ engine.yml rust-tests.yml swift-linux.yml swift-windows.yml macos.yml lint.yml regtest.yml compat.yml release.yml
```

**Root `Package.swift`** (swift-tools 6.2, `platforms: [.macOS(.v15)]`):
targets `DashEngineFFI` (`.binaryTarget(path: "Artifacts/DashEngineFFI.artifactbundle")`), `DashEngineFFIShim` (re-exports as `DashSDKFFI` for `SwiftDashSDKCore`), `DashEngineKit`, `WalletCore`, `PlatformServices`, `PlatformServicesApple/Linux/Windows` (sources compile everywhere; bodies behind `#if os()`), `DashDesignTokens`, `DashUIKitCross` (dep `swift-cross-ui` exact 0.10.0 → our fork tag), `DashWalletDesktop` (executable; deps `SwiftCrossUI`, `DefaultBackend`), tests `WalletCoreTests`, `DashEngineKitTests`, `DashUIKitCrossTests`. Dependencies: `deps/platform/packages/swift-sdk` (path; product `SwiftDashSDKCore`), `dashpay/DashUIKit` fork (branch `desktop`), `swift-log`, `swift-crypto`, `swift-cross-ui` fork. The macOS Xcode project adds the root package as a local package and links `WalletCore`, `DashEngineKit`, `DashUIKit`, `PlatformServicesApple`.

**Builds.**
- Engine: `tools/build-engine.sh --targets macos,linux-x86_64,linux-aarch64,windows-msvc --profile release` → per-target `cargo build -p dashwallet-engine`, cbindgen headers, `lipo`, bundle assembly, `nm`/`dumpbin` symbol-count assertion (≥ 894 + ours). Linux and Windows cross-builds run in CI; locally the laptop builds macOS `dev-fast` only (disk).
- macOS app: `xcodebuild -scheme DashWallet -configuration Release archive` → Developer ID signing, hardened runtime, entitlements (keychain-access-group, network client, camera for QR, user-selected files), `notarytool submit --wait`, `stapler`, `create-dmg`. Minimum macOS 15.
- Linux: `swift build -c release --product DashWalletDesktop` in `swift:6.3.3-noble` with `libgtk-4-dev`; Flatpak (`org.freedesktop.Platform 24.08` + `org.freedesktop.Sdk.Extension.swift6`, GTK 4 from the runtime) as the primary artifact; AppImage via `linuxdeploy` + bundled Swift runtime `.so`s as secondary; `.deb` later.
- Windows: `swift build -c release` on `windows-latest` (VS 2022, winget Swift 6.3.3, Windows App Runtime 1.5 if WinUI); MSI with WiX v4 (`Product.wxs` + `Bundle.wxs` chaining the Swift runtime redistributable and Windows App Runtime); Authenticode signing (cert to be procured — open item). Static Swift stdlib on Windows is not finished (research 04 §3) → runtime DLLs ship in the MSI.
- Versioning: SemVer; engine ABI version in `dwe_engine_abi_version()`; app refuses an engine bundle with a different major.

**CI** (GitHub Actions; self-hosted macOS arm64 `mac-runner-1`, `ubuntu-core` pool, GitHub-hosted `windows-latest`):

| Workflow | Trigger | What |
|---|---|---|
| `engine.yml` | PR touching `rust/` or `deps/`; nightly | Build engine for all four targets (matrix), cbindgen diff check, symbol assertion, upload `DashEngineFFI.artifactbundle` (keyed by `platform` submodule SHA + `rust/` tree hash; cache hit skips). |
| `rust-tests.yml` | PR | `cargo nextest` per crate, `cargo clippy -D warnings`, `cargo deny`, `cargo fmt --check`; schema fingerprint test. |
| `swift-linux.yml` | PR | Download bundle → `swift build` + `swift test` (WalletCore, DashEngineKit, SwiftDashSDKCore offline tests) in the Swift container; `lint-walletcore.sh`. |
| `swift-windows.yml` | PR (allowed-failure until G2 passes, then required) | Same on `windows-latest`. |
| `macos.yml` | PR | `xcodebuild test` (unit + XCUITest smoke), `swift test` for packages, SwiftUI snapshot tests. |
| `regtest.yml` | nightly + label | Docker regtest harness: wallet create/restore/send/receive/IS/CL, coin control, PSBT, sweep, governance object relay (dashd peers), ProTx registration on a dashmate local devnet. |
| `compat.yml` | nightly | dash-qt round trips (§4.4). |
| `release.yml` | tag | Engine release bundles, signed/notarized dmg, MSI, Flatpak + AppImage, SHA256SUMS, GitHub release; appcast JSON. |

---

## 4. Testing strategy

1. **Rust unit/property tests** (every gap crate; `proptest` for codecs; fixtures generated from dashd in Docker for CoinJoin messages, governance objects, ProTx payloads, dumpwallet files, `wallet.dat` of both formats with and without encryption; golden vectors from Core's functional tests where they exist). Targets: ≥90 % line coverage on `codec/`, `walletdat/`, `bip39_core`, `uri`, `denoms/rounds/progress`.
2. **Engine integration tests** (`dashwallet-engine/tests/`): create engine in a temp dir, vault create/lock/unlock state machine, import mnemonic → persister rows present, query layer JSON shapes (schema-validated), event ordering, restart/reload from persister (watch-only rebuild), backup/restore.
3. **Swift ViewModel tests** (Swift Testing, headless, run on macOS + Linux CI + Windows CI): every VM against a `FakeEngine` (`EngineProtocol` conformer fed by JSON fixtures) and against the real engine on regtest (`regtest` tag). Covers flows as state machines (send: auth → prepare → confirm → broadcast; registration phases; CoinJoin start/stop; MN wizard steps), formatting (units, thin spaces, truncation vs rounding), filters, CSV columns byte-exact.
4. **Compatibility tests vs dash-qt** (`compat`, nightly): Docker `dashd` v24 and v23.1.8 (regtest): (a) dashd creates descriptor + legacy wallets (encrypted and not), mines to them, mixes denominations via the functional-test helpers, `dumpwallet`, copy `wallet.dat` → our `dwe-cli compat.import_*` → assert same addresses, balances, labels, `cj_salt`, fully-mixed set; (b) our wallet → export mnemonic/dumpwallet/descriptors → dashd `createwallet blank` + `upgradetohd` / `importwallet` / `importdescriptors` → `rescanblockchain` → balances equal; (c) sign/verify message cross-check; (d) `dash:` URI vectors; (e) ProTx: our `register_prepare` payload == dashd `protx register_prepare` payload for the same inputs; shared-MN envelope round trip with dash-qt (manual until a headless dash-qt driver exists).
5. **Regtest integration** (`regtest`): `docker-compose` with 1 dashd (regtest, `-txindex`), a 3-node dashmate local devnet for Platform/DAPI, and a 4-masternode regtest cluster built on Core's `DashTestFramework` (python) for ChainLocks/IS/governance relay; CoinJoin end-to-end runs **on testnet nightly** with small amounts (regtest mixing needs ≥3 cooperating participants — our mock-masternode tests cover the protocol, testnet covers interop; **UNVERIFIED** whether Core's regtest MN framework can host real mixing sessions).
6. **UI tests**: XCUITest on macOS for the top-20 flows (onboarding, create/restore, send, receive, history filters, CoinJoin start/stop, MN wizard happy path, governance vote, settings); SwiftUI snapshot tests for DashUIKit components light/dark; Linux: dogtail scripts under Xvfb asserting the AT-SPI tree for the same flows (best effort, non-blocking); Windows: deferred until #787 is fixed; the `agentic-qa` skill drives release QA on real machines.
7. **Security tests**: vault fuzzing (cargo-fuzz on envelope/KDF parsing), zeroization assertions (`memsec`), lockout policy property tests with a fake clock, "no secret in logs" grep gate, dependency audit (`cargo deny`, `cargo audit`), reproducible engine builds across two runners (hash compare).
8. **Performance budgets**: cold start to window < 1.5 s (engine create + load from persister on a 10k-tx wallet), history page query < 30 ms, full mainnet SPV sync on a fresh wallet < 30 min on a laptop, CoinJoin session memory < 50 MB.

---

## 5. Delivery plan for an agent swarm

### 5.1 Milestones

| M | Name | Exit criteria | Weeks (6–9 agents) |
|---|---|---|---|
| M0 | Foundation + gates | Repo skeleton, engine builds for macOS + Linux, `dwe_engine_create/start` with SQLite persister (U1) on regtest, `SwiftDashSDKCore` compiles on Linux (G3), shared-VM probe reproduced in-repo on SwiftUI + Gtk (G1), Windows spike result (G2), CI green. | 2 |
| M1 | Wallet core | Create/restore (incl. Core-compat seed), vault + lock states + quick unlock, SPV sync with status, balances, history with filters, send (Core→Core) with confirm, receive + requests, address book, sign/verify, URIs, settings/network switch, macOS shell with sidebar/status bar/menus; Cross shell on Linux with the same screens. | 4 |
| M2 | dash-qt power features | Coin control + locks + dust, PSBT, sweep, multiwallet, backups, dumpwallet/wallet.dat import, exports, Tools window (info/console-local/peers/traffic/repair), NodeLink, tray. | 4 |
| M3 | CoinJoin + Masternodes + Governance | Mixing end-to-end on testnet, CoinJoin tab/panel/settings, MN tab + ProTx suite + shared MN, governance list/vote/create/resume + clock. | 5 |
| M4 | Platform/DashPay | Identities, DPNS + contests + marketplace, contacts/profiles/notifications, invitations (create + claim), Platform addresses, shielded, internal transfer, DashConnect, tokens (balances/transfer), masternode voting on contests, Platform sync-info screens. | 5 |
| M5 | Integrations + polish | Rates, Buy/Sell (Topper/Uphold/Coinbase), Swap, Explore/ATMs/DashSpend, ZenLedger, notifications, announcements, localization (Transifex), a11y pass, Windows packaging, Flatpak/AppImage, dmg notarization, auto-update check. | 4 |
| M6 | Hardening + release | Compat suite green against v24.0.0 final, agentic QA on macOS; Linux/Windows QA by agents on those OSes; security review; 1.0. | 3 |

Dependencies: M1 needs M0; M2/M3/M4 run in parallel after M1 (they touch disjoint crates/VM folders); M5 after M1 (integrations only need the send/receive VMs); M6 last.

### 5.2 Workstreams (scope · inputs · outputs · acceptance)

Conventions: each workstream owns named directories; cross-workstream contracts are (a) `dwe` method names + JSON schemas, (b) Swift protocols in `WalletCore/Contracts/`, (c) `PlatformServices` protocols. Contracts change only through a small PR labelled `contract` reviewed by WS-00. One agent per workstream at a time (two for WS-05/WS-10); file-level ownership avoids merge conflicts.

| WS | Name (owner dirs) | Scope | Inputs | Outputs | Acceptance |
|---|---|---|---|---|---|
| **WS-00** | Foundation & build (`rust/Cargo.toml`, `Package.swift`, `tools/`, `.github/`, `deps/`) | Repo skeleton, fork branches, `[patch]` wiring, engine build script + artifact bundle, CI matrix, caches/disk policy (G0), weekly upstream rebase, contract review. | Research 01/04, scratch probe. | Green CI on 4 targets; `tools/build-engine.sh`; `docs/adr/0001-engine.md`… | `swift build` + `swift test` pass on macOS and Linux containers with the real engine; Windows build job runs (allowed-failure). |
| **WS-01** | Engine core (`dashwallet-engine`, `dashwallet-appdb`, U1, U2) | `dwe_engine_*`, runtime, events, JSON bus, query layer, appdb schema v1, `dwe-cli`, `dwe-schema`, U1 HostPersister PR, U2 SPV tap PR. | §1.2/1.3, platform FFI headers. | `dwe.h`, `Schemas/*.json`, `dwe-cli`. | Engine integration tests; regtest: create wallet, sync, receive, `history.page` returns the tx; restart reloads from SQLite. |
| **WS-02** | Vault & auth (`dashwallet-vault`, `WalletCore/Auth`, `PlatformServices{Apple,Windows}.Biometrics`) | Passphrase vault, keyring, lock states, mixing-only, quick unlock PIN + lockout + secure time, biometrics, resolver/signer thunks, AuthenticationGate, lock screen VM, security settings VM, wipe. | §1.4, IOS-010..017, QT-111..113. | `dwe_vault_*`, `AuthenticationGate`, `LockScreenVM`, `SecurityVM`. | State-machine tests; lockout property tests; Keychain/Secret Service/Credential Manager smoke on each OS; "secrets never in logs" gate. |
| **WS-03** | Swift binding & SDK split (`deps/platform/packages/swift-sdk` fork, `Swift/DashEngineKit`) | U3 split PR, `PlatformWalletManagerCore` (@Observable, injected persistence/signer), `DashEngineKit` runtime/events/vault/query wrappers, generated Codable types, `EngineProtocol` + `FakeEngine`. | §1.5. | `SwiftDashSDKCore` product; `DashEngineKit`. | Compiles + offline tests on Linux and Windows CI; `EngineRuntime` lifecycle tests against regtest engine. |
| **WS-04** | Lifecycle & wallets (`WalletCore/Lifecycle`, `Home/SyncStatus`, `Settings/Network`) | Onboarding/demo mode, create (12/24), restore (all languages + Core-compat), phrase repair (Insight), reinstall detection, lifecycle overlay, network switch, devnet settings, multiwallet (list/switch/rename/remove/add/accounts), rescan/birth height, data-dir chooser, SyncingActivityMonitor, peers rotation. | IOS-001..009, 018, 023, 106, 110, 113; QT-002, 004, 014, 101..105, 110, 117. | VMs + engine methods `wallet.*`, `spv.*`. | VM tests with FakeEngine; regtest restore-with-birth-height finds funds; dash-qt phrase (weak checksum) restores with warning. |
| **WS-05** | L1 money (`WalletCore/{Home,History,Send,Receive,AddressBook,SignVerify,Formatting}`, `dash-payments`, U9–U11) | Balances/overview/discreet mode, history (types, statuses, filters, details, CSV both column sets, tax categories, metadata, rebroadcast/abandon/remove-unconfirmed), send (recipients, URIs, BIP70, fee policy, confirm, errors, coin control, locks, dust, PSBT, sweep, custom change), receive (requests, QR, watcher, rotation), address book, sign/verify, units. | QT-034..040, 052..100, 149, 152; IOS-019..022, 027..056. | VMs; `dash-payments`; engine `history.*`, `send.*`, `receive.*`, `addressbook.*`, `psbt.*`, `sweep.*`. | Byte-exact CSV/URI/units tests; regtest send/receive/IS/CL; PSBT round trip with dashd; sweep of a dashd `dumpprivkey` key. |
| **WS-06** | CoinJoin (`dash-coinjoin-client`, `WalletCore/CoinJoin`) | Protocol client, planner, rounds/salt rule, progress formula, settings, Overview panel, CoinJoin send page, coin-control filters, tx types/filters, info dialog, status strings. | QT-041..051, 071; IOS-057/058; Core `src/coinjoin/*` as spec. | crate + VMs + engine `coinjoin.*`. | Mock-MN protocol tests; testnet mixing to 4 rounds of 0.1 DASH nightly; fully-mixed set equals dashd's for an imported wallet.dat (compat). |
| **WS-07** | Masternodes & ProTx (`dash-protx-builder`, `WalletCore/Masternodes`, U8) | MN tab (SML list, filters, owned detection, details), register wizard, update service/registrar/revoke, shared MN sessions, operator-secret gate, masternode keychain viewer, tracked MNs, evonode status/withdraw/unban (existing FFI), epoch blocks. | QT-118..127; IOS-080..083. | crate + VMs + engine `protx.*`, `mn.*`. | Payload equality vs dashd `protx *_prepare`; regtest devnet registration end-to-end; envelope round-trip fixtures from dash-qt. |
| **WS-08** | Governance (`dash-governance`, `WalletCore/Governance`) | govsync client, tallies/statuses, clock, vote dialog + signing, create/resume proposal, info panel, NodeLink source. | QT-026, 128..134. | crate + VMs + engine `gov.*`. | Regtest: proposal created by us is listed by dashd `gobject list`; our vote appears in `gobject getcurrentvotes`; tallies equal dashd's on testnet snapshot. |
| **WS-09** | dash-qt compatibility (`dash-wallet-compat`, `WalletCore/Lifecycle/Import*`, `compat`) | BIP39 quirks, dumpwallet in/out, descriptors JSON, wallet.dat SQLite + BDB readers + decryption, export instructions UI, backups UI, settings import (QT-142), (stretch) wallet.dat writer. | QT-104..109, 115, 116, 142, 154; research 02 §18. | crate + engine `compat.*` + compat harness. | Nightly compat suite green for v24 + v23.1.8 fixtures; BDB parser fuzzed. |
| **WS-10** | Platform & DashPay (`WalletCore/Platform`) | Identities, DPNS + contests + marketplace, contacts/profile/notifications, invitations create+claim, Platform addresses/BLAST, shielded, internal transfer, DashConnect, tokens, contest voting, Platform sync-info. Uses `SwiftDashSDKCore` wrappers. | IOS-059..079, 084..086, 114. | VMs. | VM tests; dashmate local devnet: register identity + name, contact request accept, shield/unshield, transfer. |
| **WS-11** | Integrations & notifications (`WalletCore/Integrations`, `Notifications`, `HTTPClient`) | Rates (CTX), Topper, Uphold, Coinbase, CrowdNode (flag), Swap/Maya/NEAR, Explore DB + merchants/ATMs/filters/POI, DashSpend CTX/PiggyCards, gift cards, ZenLedger, faucet, notifications + batching, announcements, time skew. | IOS-024, 036..040, 087..103, 116, 121, 122; QT-031..033. | services + VMs. | Contract tests against recorded fixtures (sandbox hosts); notification batching tests. |
| **WS-12** | macOS app (`Apps/macOS`, `DashUIKit` fork, `PlatformServicesApple`, `packaging/macos`) | SwiftUI shell: sidebar/tabs/shortcuts, status bar, menus/commands, Settings scene (Options tabs), tray/MenuBarExtra, URI/scheme registration, single instance, dmg/notarize, Sparkle-ready manifest, theming, a11y labels, all screens for M1–M5 VMs. | QT-001..030, 135..141, 150, 153; IOS-104..107, 117, 119, 120. | app + UI tests. | XCUITest smoke; notarized dmg installs on a clean VM. |
| **WS-13** | Cross-platform app (`Apps/Desktop`, `DashUIKitCross`, `PlatformServicesLinux/Windows`, `packaging/{linux,windows}`) | SwiftCrossUI shell mirroring WS-12 screens, Gtk + WinUI backends, tray/notifications/autostart/single-instance/file dialogs per OS, Flatpak/AppImage/MSI. | same IDs as WS-12. | app + packages. | Builds on Linux + Windows CI; AT-SPI smoke on Linux; MSI installs on a clean Windows VM. |
| **WS-14** | Design system (`DashDesignTokens`, `tools/gen-tokens`, `DashUIKit` fork macOS port, `DashUIKitCross`) | Token generation, DashUIKit macOS port (Toast/SearchBar/AddressField/BottomSheet→sheet), ~35 Cross components, icon pipeline, Inter font bundling, light/dark. | Research 03 §3; IOS-119/120. | packages. | Snapshot tests light/dark for every component on both toolkits. |
| **WS-15** | Tools & node (`WalletCore/Tools`, `dash-node-rpc`, U7) | Information tab (SPV + NodeLink data), local command console + RPC console (redaction, history, nested calls), peers table/ban/disconnect, traffic graph, repair, logs export, developer screens (storage explorer, sync info), NodeLink settings. | QT-143..148, 145; IOS-111, 112, 114, 115. | VMs + crate. | Console parser tests (`getblock(getblockhash(0) 1)[tx][0]`); peers/traffic against regtest. |
| **WS-16** | Localization (`tools/l10n`, resources) | Transifex pipeline from `dash-mobile-wallets` (43 locales, English-key strings) + a new `dashwallet-desktop` resource for dash-qt-only strings (21 locales via Core's `dash_en.xlf` import); plural rules; RTL; `.xcstrings` for macOS, gettext `.po`/own catalog for Cross; language picker. | IOS-118; QT-151. | catalogs + pipeline. | 100 % keys present in `en`; pseudo-locale build; RTL snapshot. |
| **WS-17** | QA & integration (`tests/`, `agentic-qa`) | Regtest harness, compat harness, UI test suites, perf budgets, release checklist, agentic QA runs. | §4. | harness + reports. | Nightly green; QA report per milestone. |

### 5.3 Parallelisation map

```
M0: WS-00 ─┬─ WS-01 ─┬─ WS-02
           ├─ WS-03 ─┤
           └─ WS-14   └─ WS-13 (G1/G2 spikes)        (6 agents)
M1: WS-04, WS-05(x2), WS-02, WS-12, WS-13, WS-14, WS-17     (8 agents)
M2: WS-05, WS-09, WS-15, WS-04, WS-12, WS-13, WS-17         (7)
M3: WS-06, WS-07, WS-08, WS-12, WS-13, WS-17, WS-01(U-patches)  (7)
M4: WS-10(x2), WS-12, WS-13, WS-17, WS-16                    (6)
M5: WS-11, WS-16, WS-12, WS-13, WS-14, WS-17                 (6)
M6: WS-17 + all owners on defects                            (5–9)
```

### 5.4 Checklist mapping

**dash-qt (QT-001..154)**

| IDs | Workstream(s) | Notes |
|---|---|---|
| QT-001, 004–010 | WS-12 / WS-13 (+ WS-00 for CLI flags) | QT-005 splash → lifecycle overlay; QT-009 also on macOS. |
| QT-002, 003 | WS-04, WS-12/13 | per-network dirs/settings; icon tint + tDASH units. |
| QT-011–019 | WS-12 / WS-13 | menus/window/tabs/drag-drop. |
| QT-020–025, 027 | WS-12/13 (views), WS-04 (sync data), WS-05 (unit) | |
| QT-026 | WS-08 (+WS-12/13 view) | governance clock. |
| QT-028–030 | WS-12 / WS-13 | tray. |
| QT-031–033 | WS-11 | notifications. |
| QT-034–040 | WS-05 (+WS-12/13 views) | overview, discreet mode. |
| QT-041–051 | WS-06 | QT-048 keypool gates become "vault locked" gates. |
| QT-052–067 | WS-05 | QT-057 confirmation targets only with NodeLink. |
| QT-068–075 | WS-05 | coin control, locks, dust. |
| QT-076–079 | WS-05 | PSBT. |
| QT-080 | WS-05 (flag, stretch) | HWI. |
| QT-081–085 | WS-05 | receive. |
| QT-086–094 | WS-05 | history. |
| QT-095–098 | WS-05 | address book. |
| QT-099–100 | WS-05 | sign/verify. |
| QT-101–103, 110, 114, 117 | WS-04 | multiwallet, create, mnemonic verify, backups UI, watch-only, rescan. |
| QT-104–109, 115, 116, 154 | WS-09 | compat. |
| QT-111–113 | WS-02 | encryption/unlock/recovery phrase. |
| QT-118–127 | WS-07 | QT-119/122 [F] columns via WS-15 NodeLink. |
| QT-128–134 | WS-08 | |
| QT-135–141 | WS-12 / WS-13 (+WS-04 network tab, WS-06 CoinJoin tab, WS-05 wallet tab) | QT-140 themes Light/Dark/System. |
| QT-142 | WS-09 | preferences import. |
| QT-143–148 | WS-15 | |
| QT-149 | WS-05 | URI parsing. |
| QT-150 | WS-12 / WS-13 | scheme registration, Open URI dialog. |
| QT-151 | WS-16 | |
| QT-152 | WS-05 | units. |
| QT-153 | WS-12 / WS-13 | help/about. |

**iOS (IOS-001..123)**

| IDs | Workstream(s) | Notes |
|---|---|---|
| IOS-001–009 | WS-04 | onboarding/create/restore/repair/reinstall. |
| IOS-010–017, 123 | WS-02 | PIN/biometrics/lockout/auth gate/secrets. |
| IOS-018, 023 | WS-04 | overlay, sync status. |
| IOS-019–022 | WS-05 (+WS-12/13) | balance hero/hide/breakdown/badge. |
| IOS-024 | WS-11 | rate warnings. |
| IOS-025 | WS-12 / WS-13 | shortcuts bar. |
| IOS-026 | WS-02 | time skew (secure time). |
| IOS-027–035 | WS-05 | history. |
| IOS-036–039 | WS-05 | tax categories, rate stamping, CSV. |
| IOS-040 | WS-11 | ZenLedger. |
| IOS-041–049, 051–055 | WS-05 | send/receive; BIP70 in `dash-payments`. |
| IOS-050 | WS-10 | pay to contact (DIP-15). |
| IOS-056 | WS-05 | sweep — IN. |
| IOS-057, 058 | WS-06 | mixing — IN; move-mixed-coins kept. |
| IOS-059–064 | WS-10 | shielded, Platform addresses, internal transfer, advanced mode. |
| IOS-065–078 | WS-10 | DashPay; IOS-078 invitation creation — IN. |
| IOS-079, 084–086 | WS-10 | contest voting, marketplace, DashConnect. |
| IOS-080–083 | WS-07 | masternode tooling. |
| IOS-087–094 | WS-11 | buy/sell/swap; IOS-091 CrowdNode behind flag. |
| IOS-095–103 | WS-11 | explore/DashSpend/gift cards. |
| IOS-104–107 | WS-12 / WS-13 | settings/about views (VMs in WS-04/05). |
| IOS-108, 109 | WS-02 | security, wipe. |
| IOS-110, 113 | WS-04 | wallets, sync info. |
| IOS-111 | WS-05 | extended public key. |
| IOS-112, 114, 115 | WS-15 | logs, developer screens. |
| IOS-116 | WS-11 | notifications. |
| IOS-117 | WS-12 / WS-13 | tray companion. |
| IOS-118 | WS-16 | |
| IOS-119, 120 | WS-14 (+WS-12/13) | tokens/components, a11y. |
| IOS-121, 122 | WS-11 | faucet, announcements. |

Every ID appears exactly once as a primary owner; `docs/checklists/QT.md` and `IOS.md` carry the per-ID status and are updated by the owning workstream's PRs (CI fails if a PR touches a checklist row owned by another workstream without the `contract` label).

---

## 6. Risks and go/no-go gates

| Gate | When | Pass criteria | Fallback |
|---|---|---|---|
| **G0 Disk/build time** | M0 week 1 | Engine release build (4 targets) runs on CI only; laptop `dev-fast` macOS build fits in ≤25 GB target dir with sccache; one shared `CARGO_TARGET_DIR`; engine bundles cached by hash so Swift agents never build Rust. | Move all Rust builds to the Mac Studio / `ubuntu-core` runners; laptop fetches bundles. |
| **G1 Linux** | M0 week 1–2 | In-repo probe (`WalletCore` + engine bundle + GtkBackend) builds in `swift:6.3.3-noble`, `swift test` passes headless, AT-SPI tree shows labelled widgets under Xvfb. | Linux ships GtkBackend without a11y guarantees for 1.0; file upstream a11y work (G4). |
| **G2 Windows** | M0 week 2 (time-box 4 agent-days) | (a) `cargo build --target x86_64-pc-windows-msvc -p dashwallet-engine --features shielded` succeeds (risks: `rs-x11-hash` C via cc + bindgen/libclang, `blst` asm, halo2/orchard, bundled sqlite, `ring`); (b) Swift app links the `.lib` with the native-static-libs list; (c) WinUIBackend window with List/TextField does not crash under a UIA client (issue #787) and exposes text. | (a) fails → **W2**: build the engine as a windows-gnu **DLL** (dash-evo-tool proves the stack on `x86_64-pc-windows-gnu`), ship it next to the exe, Swift links the import lib via a `systemLibrary` target. (c) fails → **GtkBackend on Windows** for 1.0 (GTK 4 via gvsbuild, bundled DLLs; weak a11y, documented), revisit WinUI when microsoft-ui-xaml#11028 is fixed. Rust/egui is not a fallback. |
| **G3 SDK split** | M0 end | `SwiftDashSDKCore` compiles on Linux + Windows; upstream maintainers accept the split direction (PR opened, not necessarily merged). | Vendor-by-script into `DashEngineKit` + own `EngineWalletManager` (§1.5). |
| **G4 SwiftCrossUI governance** | M0 end | Fork pinned at 0.10.0 under `dashpay/`; maintainer open to a11y modifiers + exporting `DummyBackend` for headless view tests. | Keep the fork; accept bus-factor risk; views stay thin so the toolkit is swappable. |
| **G5 SqlitePersister parity** | M1 week 1 | Field-by-field audit vs `PlatformWalletChangeSet`; platform-wallet lifecycle tests pass on `SqlitePersister`; `pending_contact_crypto` and shielded notes covered. | Add missing writers upstream (platform-wallet-storage is upstream-owned and active); worst case our engine implements `PlatformWalletPersistence` itself over our own schema (large, ~6k lines; avoid). |
| **G6 CoinJoin interop** | M3 mid | Three consecutive nightly testnet runs complete 4 rounds; fully-mixed set equals dashd's for an imported wallet; no masternode bans us. | Ship CoinJoin behind a "beta" flag in 1.0; keep "move mixed coins". |
| **G7 BDB reader** | M2 mid | Parses 100 % of generated fixtures (v23.1.8 legacy wallets, encrypted and not, with 10k keys) and survives 1 h of fuzzing. | Ship SQLite `wallet.dat` + dumpwallet import only; document `dash-wallet dump` → our dump importer as the legacy path (it gets every record out, research 02 §18.1). |
| **G8 MSVC/WinUI runtime size** | M5 | MSI ≤ 200 MB with Swift runtime + WinAppRuntime bootstrapper; clean-VM install works. | Drop WinUI (GtkBackend), static-link swift-winui as 0.7 did. |
| **R1 Upstream drift** | continuous | Weekly rebase job of the fork branch; `[patch]` compile check; cbindgen header diff. | Pin and defer; patches are additive by rule. |
| **R2 Platform trust via SPV** | M4 | Proof verification against SPV quorum keys matches trusted-provider results on testnet for 24 h. | Default to the trusted service, keep SPV as opt-in. |
| **R3 Governance over P2P** | M3 | Full govsync of mainnet objects + current-cycle votes < 10 min on a laptop. | NodeLink-only governance tallies; voting/creation still works without a node. |
| **R4 Two view trees** | continuous | Views stay ≤ 150 lines; any logic found in a view fails `lint-walletcore` review. | — |
| **R5 Signing certificates** | M5 | Apple Developer ID + Authenticode cert procured. | Unsigned Windows build with SmartScreen warning documented. |

---

## 7. Open product decisions I made (flag for reconciliation)

1. Mnemonic passphrase is **shown** in "Show Recovery Phrase" (behind full unlock, separate "reveal" step); dash-qt never shows it, but our users may need it to restore into dash-qt (`upgradetohd` needs it).
2. "Traditional" theme dropped; Light/Dark/System only.
3. dash-qt's label-overwrite-on-send and the proposal payment-date bug are **not** reproduced.
4. HWI external signer deferred (flag); PSBT in.
5. CoinJoin mixes on the external chain for Core compatibility by default, with an "Android/dashj-compatible (DIP-9)" option.
6. Governance tallies without a node come from P2P govsync; the UI marks them "from peers, may lag".
7. Explore DB fetched over plain HTTPS from the existing GCS bucket (no Firebase SDK); if the bucket needs auth, mirror on dashhq.org.
8. Announcements via a signed GitHub Pages feed.
9. Start-on-login offered on macOS too.

---

## 8. Verification ledger

Verified by reading code this session: `SqlitePersister` implements `PlatformWalletPersistence` and is `Send + Sync` (`rs-platform-wallet-storage/src/lib.rs`); `SqlitePersisterConfig` fields (`config.rs:151`); `SecretStore::{file, os, set, get, delete, reprotect}` (`secrets/store.rs`); `PlatformWalletManager<P>::new(sdk, Arc<P>, handler)` (`manager/mod.rs:475`); FFI handle alias `PlatformWalletManager<FFIPersister>` (`handle.rs:150`); `platform_wallet_manager_create*` signatures (`manager.rs:70–151`); `SpvRuntime::get_quorum_public_key` (`spv/runtime.rs:340`) and `connected_peers`, `masternodes_by_voting_key_blocking`; `dash_sdk_create_with_callbacks` export; dash-spv `NetworkManager` trait (`message_receiver`, `send_message`, `broadcast`), `MessageType::Unknown`, `Unknown` messages forwarded (`manager.rs:900`), `Peer`/`PeerNetworkManager`/`MessageDispatcher` public, `disconnect_peer`, `masternode_list_engine()` on the client; `key-wallet::psbt` API; `Wallet::{from_mnemonic (hard-codes empty passphrase), from_seed, from_seed_bytes, from_extended_key, from_xpub, new_watch_only}`; `Mnemonic::{to_seed, validate, normalize_phrase}`; `DASH_SIGNED_MSG_PREFIX`; ProTx payload types incl. `payload_collateral_string`; Core wire names `dsa/dsi/dsf/dss/dsc/dssu/dstx/dsq/senddsq/govsync/govobj/govobjvote` and `CGovernanceVote` fields; dash-evo-tool Windows build = `x86_64-pc-windows-gnu` with `OPENSSL_STATIC=1`, features `shielded` on `platform-wallet` and `platform-wallet-storage`; platform Linux CI builds `platform-wallet-storage`/`-ffi`/`rs-unified-sdk-ffi` with `--all-features`; the scratch probe's SE-0482 bundle layout and passing `swift test`.

**UNVERIFIED** (to be closed by the named gate/workstream): MSVC build of the engine (G2); `rs-x11-hash` under MSVC; count of `FFIPersister`-specific call sites in platform-wallet-ffi (U1 cost); whether `SqlitePersister` persists `pending_contact_crypto` (G5); `ProUpShareTx` payload types in rust-dashcore at the pin (U8); dash-spv proxy support (U7b); `SdkBuilder` custom context-provider ergonomics in Rust (R2); govsync performance (R3); GCS bucket unauthenticated access; two binary targets with one module name in SwiftPM (G3 shim); regtest masternode framework hosting real CoinJoin sessions (WS-17); Xcode consumption of SE-0482 bundles (only `swift build` verified).

---

## Appendix A — initial `dwe_invoke` method catalogue (owner in brackets)

`engine.status`, `engine.diagnostics`, `engine.set_log_level` [WS-01] · `wallet.list`, `wallet.create`, `wallet.import_mnemonic`, `wallet.import_xpub`, `wallet.rename`, `wallet.remove`, `wallet.accounts`, `wallet.set_birth_height`, `wallet.backup`, `wallet.restore_backup` [WS-04] · `spv.start`, `spv.stop`, `spv.progress`, `spv.peers`, `spv.rotate_peers`, `spv.rescan`, `spv.clear`, `spv.traffic`, `spv.ban`, `spv.disconnect` [WS-04/15] · `balance.get`, `history.page`, `history.filters_meta`, `tx.detail`, `tx.raw`, `tx.rebroadcast`, `tx.abandon`, `tx.remove_unconfirmed`, `tx.set_meta`, `history.export_csv` [WS-05] · `send.prepare`, `send.confirm`, `send.release`, `send.max`, `fee.policy`, `coincontrol.utxos`, `coincontrol.lock`, `coincontrol.unlock`, `dust.settings`, `psbt.create`, `psbt.load`, `psbt.analyze`, `psbt.sign`, `psbt.broadcast`, `uri.parse`, `uri.build`, `bip70.fetch`, `bip70.pay`, `message.sign`, `message.verify`, `receive.next_address`, `receive.request`, `receive.requests`, `addressbook.*`, `units.format` [WS-05] · `coinjoin.start|stop|status|settings|progress|info` [WS-06] · `mn.list`, `mn.detail`, `mn.owned`, `protx.register.{prepare,sign_external,submit}`, `protx.update_service`, `protx.update_registrar`, `protx.revoke`, `protx.shared.{create,import,approve,sign,combine,broadcast,dissolve,rotate,reward}`, `mn.keys.derive`, `mn.tracked.*` [WS-07] · `gov.sync_status`, `gov.proposals`, `gov.votes`, `gov.vote`, `gov.proposal.create`, `gov.proposal.resume`, `gov.proposal.broadcast`, `gov.info`, `gov.clock` [WS-08] · `compat.sniff`, `compat.import_dumpwallet`, `compat.import_walletdat`, `compat.import_descriptors`, `compat.export_dumpwallet`, `compat.export_descriptors`, `compat.export_instructions`, `compat.import_prefs` [WS-09] · `platform.*` is **not** on the bus — Platform calls go through `SwiftDashSDKCore` wrappers over the upstream handles (`dwe_platform_wallet_manager`, `dwe_sdk_handle`) [WS-10] · `node.link`, `node.unlink`, `node.status`, `node.rpc`, `console.exec` [WS-15].

## Appendix B — key Swift types

`DashEngineKit`: `EngineRuntime` (actor; `start()`, `stop()`, `switchNetwork(_:)`, `switchWallet(_:)`, `addWallet(_:)`, `wipe()`), `EngineEvent` (enum, Codable), `EngineChangeFeed`, `Vault` (`createEncrypted`, `unlock(scope:)`, `lock`, `quickUnlock`, `lockState: AsyncStream<LockState>`), `EngineQuery`/`EngineCommand` (generated), `EngineProtocol` + `FakeEngine`.
`WalletCore/Contracts`: `AuthenticationGating`, `RatesProviding`, `TransactionSource`, `NotificationPosting`, `PreferencesStoring`, `NodeLinking`.
`PlatformServices`: `SecretUnlockUI`, `Clipboard`, `URLOpener`, `Notifications`, `Tray`, `SingleInstance`, `FileDialogs`, `Biometrics`, `Autostart`, `Camera`, `ScreenCapture`, `PowerAssertion`, `OpenAtLogin`.
`DashUIKit`/`DashUIKitCross`: `DashButton`, `DashSwitch`, `SearchBar`, `AddressFieldView`, `NumericKeyboardView`, `EnterAmountView`, `DashAmount`, `DashBalanceView`, `MenuItem`, `TransactionView`, `ConverterCard`, `CoinSelector`, `RadioButtonRow`, `NavigationBar`, `TopIntroView`, `Toast`, `SystemMessageView`, `LoadingSpinner`, `SuccessIllustration`, `ErrorIllustration` — identical names, props and states on both toolkits.
