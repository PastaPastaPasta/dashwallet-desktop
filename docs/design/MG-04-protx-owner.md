# MG-04: ProTx owner flows

ROADMAP MG-04 (release 1.1, size L, engine, tiers T1 and T2, stack-independent). It covers:
- ProRegTx for regular masternodes and evonodes, with the collateral `FundNew`, `ExistingUtxo` or `External` (proven
  with a signed message);
- version-2 payloads, and version 3 where the network requires it;
- ProUpRegTx;
- the operator key taken as given, or from the wallet's keychain.

Acceptance:
- each payload equals dashd's for the same inputs (dashd is the oracle);
- registrations are accepted on regtest (and on dashmate where the tier allows);
- there is no operator-key signing anywhere.

Inputs:
- MG-01's `dw-protx`, the vault's masternode keys and the no-operator-signing guard ([`MG-01-salvage.md`](MG-01-salvage.md));
- MG-03's facade and contract ([`MG-03-masternodes.md`](MG-03-masternodes.md), [`mn-engine.md`](../contracts/mn-engine.md));
- MG-05's `MasternodeOwner` scope, the `MasternodeOp{max_duffs}` purpose, and the `masternode_registration` and
  `key_change` flows ([`MG-05-authorization.md`](MG-05-authorization.md));
- MG-00's hand-over row for MG-04, and D25–D28, D38, D39 ([`MG-00-masternodes-ux.md`](MG-00-masternodes-ux.md));
- UX-SPEC §4.25.5 and §4.25.6;
- the MG-06 r1 review (`node-program/reviews/DW-MG-06-r1-opus.md`, M-1 and M-2: a resend uses the first send's fence
  and reservations, and a row whose transaction may be out is never deleted);
- Dash Core **v24.0.0-rc.3** (`0364ed62`). Every "rc.3" citation below is `git show v24.0.0-rc.3:<path>` at that tag.

D-numbers in this file are MG-04's (D1 and up). Other tasks' rows are cited as "MG-05 D4" and so on.

| Part | Where |
|---|---|
| Payload builders, Core's payload rules, the oracle vectors | `rust/crates/dw-protx/src/{payloads,rules,service}.rs`, `tests/oracle_protx.rs`, `testdata/protx_*.json`, `testdata/oracle/dashd/gen_masternode_vectors.py` |
| The owner payload signer | `rust/crates/dw-vault/src/{signer,owner}.rs` |
| Facade calls, records, errors | `rust/crates/dw-engine/src/masternodes/{protx,protx_records}.rs` (facade) |
| Assembly, signing, the fenced hand-off, recovery | `rust/crates/dw-engine/src/masternodes/protx/{build,ops,keys}.rs` (not facade) |
| The prepared tx a ProTx hands to the fence | `PreparedTx::assembled` in `rust/crates/dw-engine/src/send/mod.rs` (one new constructor) |
| Persistence | `rust/crates/dw-appdb/migrations/V2026102101__mg_protx_ops.sql`, `rust/crates/dw-appdb/src/protx_ops.rs` |
| dwcli | `rust/bin/dwcli/src/masternodes.rs`: `mn register`, `mn register-submit`, `mn update-registrar` |
| Regtest suite | `regtest/functional/dwd_protx.py` |

## 1. What Core does, and what the wallet does instead

Core's `protx register*` and `protx update_registrar` RPCs end in `evo::provider::BuildRegistration` and
`UpdateRegistrar` (rc.3 `src/evo/providertx_service.cpp:469-617`, `:864-953`). The wallet follows them step by step.
Where it can't, a D-row says why.

| Step | Core (rc.3) | Wallet |
|---|---|---|
| Payload version | `DeploymentToProtxVersion` at the tip: 2 after v19, **3 once `DEPLOYMENT_V24` is active** (`providertx_service.cpp:94-100,499`; `validation.cpp:6805`; `evo/types.h:15-37`) | **2**, always, at this pin (D1) |
| Collateral `FundNew` | an output of the type's amount to the collateral address, found again after funding by value **and** script; payload `collateralOutpoint = (null, n)`; signature empty (`:503-506,543-564`) | the same; the address is a fresh receive address of this wallet (UX-SPEC §4.25.5) |
| Collateral existing | the coin read from the UTXO set; P2PKH only; locked in memory and kept locked on success (`CollateralLockGuard`, `:567-589,614`); `vchSig` sized to 65 bytes for funding (`:573`) | `ExistingUtxo`: a wallet coin of exactly the amount, persistently locked (D9). `External`: an outpoint and its address, entered (D4) |
| Collateral proof | `signmessage` of `MakeSignString()` with the collateral key, base64-decoded into `vchSig` (`:599-610`; `providertx.cpp:375-395`) | the same message. `ExistingUtxo`: the vault signs it with the coin's address key. `External`: the user signs it in the other wallet and pastes it |
| Prepare / submit | `register_prepare` returns the funded unsigned tx, `collateralAddress` and `signMessage`; `register_submit` requires an empty `vchSig`, inserts the signature and signs the inputs (`:591-597,776-799`; `rpc/evo.cpp:699-748,951-966`) | `prepare_external_registration` / `submit_external_registration`, with the prepared tx persisted (§5) |
| Owner, voting | owner from the address given; voting defaults to the owner key (`:519-520`) | owner: always a keychain owner key of this wallet (MG-00 D25), never shared with voting (UX-SPEC §4.25.5) |
| Operator key | basic BLS public key; its encoding must match the version (`:521-525`) | given (public hex, checked as a G1 point), or this wallet's keychain operator **public** key (D14) |
| Payout | one payout; a list is v3 only (`:528-534`) | one payout address (MG-00 D28) |
| Network fields | `ApplyNetInfo` then `CheckProviderNetworkFields` (`:136-265`; `providertx.cpp:212-263`) | the same rules for v2 (`dw-protx::service`); a service address is required (MG-00 D26, D17) |
| Funding | `fundTransaction` from the payout address or `feeSourceAddress` only, change back to it (`:345-357,542`) | coin selection over the wallet's standard accounts, change to a fresh internal address (D2) |
| Ordering | BIP69 inputs and outputs (random change position, `wallet/spend.cpp:843,1010,1034`) | BIP69 (D3) |
| Inputs hash | `CalcTxInputsHash` after funding (`:318-321`) | the same, in the finalizer |
| Preflight | `CheckSpecialTx` against the tip and UTXO set before signing (`:359-387`) | Core's rules that the SPV state can check (D5) |
| ProUpRegTx | voting and operator keys default to the list entry; payouts default to the state's (`:884-931`); signed by the owner key over `SerializeHash(payload)` (`:324-334,946-950`) | operator and voting "keep" read the synced list; the payout must be named (D10); signed by the vault's owner payload signer (D11) |

### Payload rules mirrored

`dw-protx::rules` holds Core's checks as pure functions, each returning Core's reject reason. The engine runs them
before signing, and the oracle suite runs them against dashd's own payloads:
- `CProRegTx::IsTriviallyValid` (rc.3 `src/evo/providertx.cpp:265-333`): version, type, mode, null keys, the
  operator key's encoding, the payout list (`IsPayoutListTriviallyValid`, `:25-62`: payee not the owner's or the
  voting key's address), the operator reward;
- `CheckProviderNetworkFields` (`:212-263`): v2 Platform ports (the mainnet defaults on mainnet, never 9999, all three
  different), the platform node id non-null;
- `CheckProRegTx` (rc.3 `src/evo/specialtxman.cpp:1148-1310`): the collateral-key reuse (`IsPayoutListKeySafe`,
  `providertx.cpp:146-164`), and against the synced list: a duplicate service, operator key (under either BLS
  scheme, `:1250-1274`), or platform node id (`:1276-1281`);
- `CProUpRegTx::IsTriviallyValid` (`providertx.cpp:459-477`) and `CheckProUpRegTx` (`specialtxman.cpp:1428-1528`):
  the payee not the owner's or the new voting key's address, the operator key not another masternode's, the
  collateral key not reused.

## 2. Facade

All calls are on `Masternodes` (MG-03's handle). Kind as in mn-engine §2: `sync` or `async`.

| Call | Kind | Grant | Does |
|---|---|---|---|
| `registration_keys(wallet, kind)` | async | — | The next unused keychain keys: owner, voting, operator **public** key, and for an evonode the platform node id (D13, D14). Signs nothing. |
| `preview_registration(wallet, request)` | async | — | Validates `RegistrationRequest` (§3) with every rule of §1, plans the transaction and returns `RegistrationPreview`: payload version, fee, what leaves the wallet, the keys and addresses it will use. Locks and reserves nothing. |
| `register(wallet, request, grant)` | async | `MasternodeOp` | `FundNew` and `ExistingUtxo` only. Begins a `masternode_registration` lease from the grant, plans, signs, saves the row, hands the ProRegTx to the fence, records the outcome, ends the lease. Returns the `ProTxOperation`. |
| `prepare_external_registration(wallet, request)` | async | — | `External` only. Plans the transaction, locks its fee inputs (D8), computes `inputsHash` and Core's sign message, and saves the row as `needs_signature`. Returns the operation, with its `SignRequest` (address and message). |
| `submit_external_registration(wallet, operation, signature, grant)` | async | `MasternodeOp` | Checks the base64 signature against the collateral address and the saved message (Core's `CheckStringSig`, `specialtxman.cpp:1079-1094,1295-1300`), inserts it, signs the inputs and sends as `register` does. |
| `refresh_external_registration(wallet, operation)` | async | — | For a `needs_signature` row whose fee inputs were spent (`inputs_spent`): new inputs, a new message, the same terms. The old signature no longer fits. |
| `preview_update_registrar(wallet, request)` | async | — | Validates `UpdateRegistrarRequest` and returns `UpdateRegistrarPreview` (fee, the owner key that signs, which fields change, whether the change PoSe-bans). |
| `update_registrar(wallet, request, grant)` | async | `MasternodeOp` | A ProUpRegTx under a `key_change` lease, signed by the masternode's owner key: a keychain key, or an attached `owner` key (D12). |
| `operations(wallet)` | async | — | Every `ProTxOperation` of the wallet, newest first, with its state derived on read (§5). |
| `resend_operation(wallet, operation, grant)` | async | `MasternodeOp` | Hands a signed, unconfirmed row's saved bytes to the fence again (D7). An update is refused while a later update of its masternode is or may go out (`update_superseded`, D43). |
| `discard_operation(wallet, operation)` | async | — | Ends a row nothing of which may be out: `needs_signature`, `inputs_spent`, `signed` (saved, never handed off) or `not_sent`. Unlocks what the row locked. Refuses any other state (D6). |
| `release_collateral(wallet, pro_tx_hash)` | async | — | Unlocks the collateral coin of one of the wallet's masternodes, and remembers it was released (MG-00 D38, D9). Sends nothing. |
| `lock_collateral(wallet, pro_tx_hash)` | async | — | Locks it again, and clears that mark. |

**Grants and leases (MG-05 §4).** Each signing call takes one grant id and begins its own lease (`FlowKind::
MasternodeRegistration` for `register`, `submit_external_registration` and a registration's `resend_operation`;
`FlowKind::KeyChange` for `update_registrar` and an update's resend), as `Governance::vote` does. It ends the lease
when it returns. The lease's `masternode` budget is charged with the transaction's outflow (foreign outputs plus the
fee, MG-05 §2) in the same J step that hands out the signer, before anything is signed. A `FundNew` collateral pays
this wallet, so only the fee is charged. An external payout is no output. A charge over the cap is
`send.grant_exceeded` and signs nothing; a grant of another purpose, wallet or one already used is the lease's
`platform.grant_invalid` (MG-05 D12), carried as `MasternodesError::Platform`.

**The fence and coin reservations (E0-04 §5, MG-05 §4).** Every ProTx is a row-less `CoreTx{asset_lock: false}`
artifact, handed off through `TxDraft`'s existing path:
- `build.rs` assembles and signs the transaction under the lease (§4), then makes a `PreparedTx` with
  `PreparedTx::assembled`. That reserves its inputs under `SpendOwner::Tx(txid)` and binds the charge to the txid,
  as the CoinJoin branch of `sign_and_reserve` does;
- `TxDraft::broadcast` hands it off. That covers the permit and its deadline, a lock's drain, `Refused`,
  `Deferred`, and the release of a `NotSent` draft's inputs and charge;
- a resend (D7) builds its `PreparedTx` the same way from the saved bytes. It reserves the inputs only if no other
  owner holds one (else `send.outpoint_unavailable`), and is charged again under its own lease. It can't race the
  first send: an operation's in-flight mark (§5) covers the save, the hand-off and the record step.

**Events.** `EngineEvent::Masternodes { network }` after every change of an operation, a collateral lock, or a lease
end, as MG-03 §6.

## 3. Records

```rust
pub enum CollateralChoice {
    FundNew,
    ExistingUtxo { outpoint: String },              // "<txid>-<n>", a wallet coin
    External { outpoint: String, address: String }, // another wallet's coin and its P2PKH address
}
pub enum OperatorChoice { Given { public_key: String }, Keychain { index: u32 }, NextUnused }
pub enum VotingChoice { Keychain { index: u32 }, Address { address: String }, NextUnused }
pub enum PayoutChoice { NewAddress, Address { address: String } }
pub enum PlatformNodeChoice { Given { node_id: String }, Keychain { index: u32 }, NextUnused }
pub struct EvonodeTerms { pub node: PlatformNodeChoice, pub p2p_port: Option<u16>, pub http_port: Option<u16> }

pub struct RegistrationRequest {
    pub kind: MasternodeType,             // MG-03's Regular | Evonode
    pub collateral: CollateralChoice,
    pub service: String,                  // "a.b.c.d[:port]", required (D17)
    pub operator: OperatorChoice,
    pub operator_reward: u16,             // basis points, 0..=10000
    pub owner_index: Option<u32>,         // None: the next unused owner key
    pub voting: VotingChoice,
    pub payout: PayoutChoice,
    pub evonode: Option<EvonodeTerms>,    // Some iff kind == Evonode; ports default to the network's
}

pub enum KeepOr<T> { Keep, Set(T) }
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOr<OperatorChoice>,
    pub voting: KeepOr<VotingChoice>,
    pub payout: PayoutChoice,             // always named (D10)
}

pub struct ProTxKeys { pub owner: KeyPick, pub voting: KeyPick, pub operator: KeyPick, pub platform_node: Option<KeyPick> }
pub struct KeyPick { pub index: u32, pub address: Option<String>, pub public_key: Option<String>, pub node_id: Option<String> }

pub struct RegistrationPreview {
    pub terms: ProTxTerms,                // a NewAddress payout and a FundNew address are empty (D29)
    pub fee: u64, pub total_debit: u64, pub collateral_amount: u64,
}
pub struct UpdateRegistrarPreview {
    pub terms: ProTxTerms,                // the masternode as it reads after the update (D41)
    pub version: u16, pub fee: u64, pub total_debit: u64, pub owner_key: OwnerKeySource,
    pub operator_changes: bool, pub voting_changes: bool, pub payout_changes: Option<bool>, // None: current payout unknown
    pub pose_bans: bool,                  // a new operator key bans until a service update
}
pub enum OwnerKeySource { Keychain { index: u32 }, Attached }

pub enum ProTxOperationKind { Registration, UpdateRegistrar }
pub enum ProTxOperationState {
    NeedsSignature, InputsSpent,          // External, before submit
    NotSent,                              // signed; the one attempt was definitely not sent
    Signed,                               // signed and saved; no attempt finished (a crash, D6)
    Sent,                                 // handed out (or may be); not in a block
    Confirmed { height: u32 },            // in a block of the wallet's history
    Conflicted,                           // an input was spent by another transaction in a block
}
pub struct SignRequest { pub address: String, pub message: String }
pub struct ProTxOperation {
    pub id: String, pub kind: ProTxOperationKind,
    pub pro_tx_hash: Option<String>,      // a registration's once signed (its txid); an update's target
    pub txid: Option<String>, pub state: ProTxOperationState, pub sign_request: Option<SignRequest>,
    pub fee: u64, pub created_at: u64, pub signed_at: Option<u64>, pub sent_at: Option<u64>,
    pub last_error: Option<String>,       // a code (§6), never a message quoting input
    pub terms: ProTxTerms,                // what it registers or changes, for display; its masternode_type
                                          // and collateral (FundNew | ExistingUtxo | External) are here
}
```

The records follow mn-engine §1: `serde`, `Debug`, `Clone`, `PartialEq`; unit enums are snake-case, enums with data
are tagged with `kind`. `ProTxTerms` holds the addresses, keys, service, platform fields and reward the payload
carries, as strings. It holds no secret, and no signature over anything but the collateral message.

## 4. Building a ProTx

`build.rs`, one path for all four flows:
1. **Check** the request with §1's rules, offline first, then against the synced list and the wallet's registrations
   (D5). The list must be synced (`masternodes.list_unavailable` otherwise), because the duplicate checks need it.
2. **Pick keys** (D13): the owner key by index or the next unused one; voting likewise or an address; operator and
   platform node as given or from the keychain (public keys only, D14).
3. **Plan** with `send::plan` over the wallet's spendable standard-account coins (excluding locked, reserved and
   immature ones, as `CoinSource::Any`). The outputs are the `FundNew` collateral (if any). The size adds the
   placeholder payload's serialized length, with a 65-byte signature where Core sizes one (`:573`, `:946`). Change
   goes to a fresh BIP44 internal address. Inputs and outputs are sorted BIP69 (D3).
4. **Placeholder**: `dw_protx::payloads::registration_placeholder` or `update_registrar_placeholder` (version 2).
5. **Charge and sign** (`register`, `submit_external_registration`, `update_registrar`, a resend): begin the lease;
   `charge_tx_signer(Masternode, outflow)` gives the `MasternodeOwner` signer.
6. **Finalize the payload** on the unsigned transaction:
   - `inputsHash`;
   - `FundNew`: the collateral output's index, found by value and script, as Core does (`:552-558`);
   - `ExistingUtxo`: `signer.sign_message(coin address path, MakeSignString)`, base64-decoded;
   - `External`: the pasted signature, checked against the address first;
   - ProUpRegTx: `signer.sign_update_registrar(OwnerKey, &payload)` (D11).
7. **Re-check** the finished payload with §1's rules (the signature and the inputs hash included), then **sign the
   inputs** (P2PKH, `SIGHASH_ALL`, the sighash covering the payload) and make the `PreparedTx`.
8. **Save, then send**: the row with the signed bytes is written durably before the hand-off (§5); then
   `TxDraft::broadcast`; then the outcome is recorded.

A `preview_*` call runs steps 1–4 only. `prepare_external_registration` runs 1–4, computes the inputs hash and the
message, locks the fee inputs and saves the row; it begins no lease and signs nothing.

The wallet never signs as operator. No step reads an operator secret. The operator key is a public key, given or
derived from the keychain's xpub (`masternode_keys`, `Operator`), and `dw-protx/tests/no_operator_signing.rs` keeps
scanning the workspace (MG-01 D6).

## 5. Persistence across crash and restart

app.sqlite, table `masternode_ops` (migration `V2026102101__mg_protx_ops.sql`, scoped by wallet and network, deleted
with the wallet):

| Column | Holds |
|---|---|
| `id` | 16 random bytes, hex |
| `wallet_id`, `network`, `kind`, `masternode_type`, `collateral_kind` | |
| `terms` | `ProTxTerms` as JSON (no secret) |
| `request` | the `RegistrationRequest` / `UpdateRegistrarRequest` as JSON, for `refresh_external_registration` |
| `unsigned_tx`, `sign_address`, `sign_message`, `fee_inputs` | `External` only: the prepared tx (empty `vchSig`), Core's message, the locked fee inputs |
| `signed_tx`, `txid` | the signed bytes, written before any hand-off (save-before-broadcast, as MG-06 D4) |
| `created_at`, `signed_at`, `sent_at`, `last_error` | `sent_at` is set **before** each hand-off that may put the bytes out (first send or resend) |
| `locked_collateral` | whether this row locked an `ExistingUtxo` coin (released on a definite failure) |

**The state is derived on every read**, from the row and the wallet's history (`ProTxOperationState`, §3):
- `needs_signature` / `inputs_spent`: no `signed_tx`; `inputs_spent` when any fee input is no longer an unspent
  wallet coin;
- `confirmed` once the txid is in a block of the wallet's history; `conflicted` once an input of `signed_tx` is spent
  by another transaction in a block;
- else `sent` when `sent_at` is set, `not_sent` when `last_error` holds a definite refusal and `sent_at` is null,
  `signed` otherwise (a crash between the save and the record step).

**Rules** (MG-06 r1 M-1, M-2):
- **A row whose bytes may be out is never deleted.** The only deletes are `discard_operation` on a row with no
  `signed_tx`, or with `sent_at` null and no attempt in flight, both written as `DELETE … WHERE sent_at IS NULL`, the
  call reading back whether it deleted. A definite `NotSent` first attempt marks the row `not_sent` and keeps it.
- **One attempt at a time per operation:** an in-flight set in the session holds `(wallet, operation)` from the
  save to the record step. A second call on it is `masternodes.in_flight`.
- **Reservations after a restart.** In-memory reservations don't survive one. A `needs_signature` row's fee inputs
  are therefore *persistently locked* (D8), and a resend reserves the saved inputs again under `SpendOwner::Tx`,
  refusing coins another owner holds (D7).
- The row is written with `durably` (`synchronous=FULL`), as MG-06's insert and MG-03's markers are.

The ExistingUtxo collateral lock and the FundNew collateral lock are the existing persistent coin locks
(`lock_outpoints`, app.sqlite). Core's `AutoLockMasternodeCollaterals` (rc.3 `src/wallet/wallet.cpp:2515-2525`) is
mirrored by the follower: each pass locks the collateral of the wallet's own registrations unless
`masternodes.collateral_released:<outpoint>` is set (D9).

## 6. Errors

New `MasternodesError` variants, with their §4 codes:

| Variant | Code | When |
|---|---|---|
| `ProtxRefused { reason }` | `masternodes.protx_refused` | a Core rule of §1 fails; `reason` is Core's reject reason verbatim (`bad-protx-dup-key`, `bad-protx-dup-netinfo-entry`, `bad-protx-payee-reuse`, `bad-protx-collateral-reuse`, `bad-protx-platform-p2p-port`, …), which the UI phrases (MG-00's MG-04 row) |
| `CollateralUnavailable` | `masternodes.collateral_unavailable` | `ExistingUtxo`: not an unspent, spendable wallet coin of exactly the type's amount at a P2PKH address, or already the collateral of one of the wallet's masternodes or operations (D23), or held by a payment's claim or by a proposal fee the wallet hasn't seen (D47) |
| `SignatureMismatch` | `masternodes.signature_mismatch` | `External`: the signature isn't base64 of a 65-byte compact signature by the collateral address over the saved message |
| `OperationNotFound` | `masternodes.operation_not_found` | |
| `OperationState { state }` | `masternodes.operation_state` | the call doesn't apply to the operation's state (`state` is its snake-case name) |
| `InFlight` | `masternodes.in_flight` | another attempt on the operation is running |
| `NoOwnerKey` | `masternodes.no_owner_key` | ProUpRegTx: neither a keychain owner key 0–19 nor an attached `owner` key has the owner key id the wallet knows, or no owner key id is known |
| `UpdatePending { operation }` | `masternodes.update_pending` | ProUpRegTx: a key is kept while a loaded wallet's operation `operation` on the same masternode may not be in the synced list yet (D43) |
| `UpdateSuperseded { operation }` | `masternodes.update_superseded` | `resend_operation` of a ProUpRegTx that the later update `operation` of the same masternode would see undone (D43) |
| `KeysExhausted { role }` | `masternodes.keys_exhausted` | every keychain owner (or voting) key 0–19 is in use (D13) |
| `Platform(PlatformError)` | the `platform.*` code | lease errors (MG-05 D12) |

`masternodes.unsupported{feature}` gains `protx_v3` (D1) and `shared_masternodes` for a ProUpRegTx on a
`shared_unsupported` masternode. `invalid_argument` covers malformed input, including a 32-byte secret where a BLS
public key is expected. That message says it looks like a secret key, and echoes nothing (MG-00's MG-04 row,
mn-engine D23). The `send.*` codes of `TxDraft` (`send.grant_exceeded`, `send.insufficient_funds`,
`send.outpoint_unavailable`, `send.cancelled`, …) pass through `Engine(EngineError)`.

## 7. Tests

**T0 (unit, `cargo test`):**
- `dw-protx`:
  - the builders and the rules module, each reject reason;
  - the External flow: prepare, then `finalize_registration` with a given signature;
  - the evonode v2 fields.
- `dw-protx/tests/oracle_protx.rs`, over committed dashd vectors (`testdata/protx_registration.json`,
  `protx_update_registrar.json`):
  - for each flow (regular and evonode; `FundNew`, existing, `register_prepare`/`register_submit`; ProUpRegTx with
    each field changed), the payload rebuilt from the same terms **on dashd's own funded transaction** equals dashd's
    bytes. The signatures are included, since both sides sign RFC 6979 deterministically with the same imported keys;
  - the sign message equals dashd's `signMessage`;
  - the rules module passes dashd's payloads and returns dashd's reject reason for each mutated payload.
- `dw-vault`:
  - `sign_update_registrar` signs at keychain owner paths and with an attached owner key bound to its ProTx hash;
  - it refuses a payload of another masternode for an attached key, and any grant other than `MasternodeOp`;
  - `MasternodeOwner` no longer signs digests at owner paths (D11).
- `dw-engine`:
  - every call's refusals in order;
  - the charge (only the fee for `FundNew`, cap equal to the fee signs and one duff less refuses);
  - save-before-broadcast;
  - a definite `NotSent` keeps the row `not_sent`, and a `MaybeSent` keeps it `sent`;
  - a resend reserves the saved inputs, refuses a coin a prepared draft holds, and runs under the fence;
  - the in-flight mark, with a race test modelled on the MG-06 review's;
  - a restart with a `needs_signature` row (inputs still locked; spent gives `inputs_spent`; refresh gives a new
    message);
  - a lock during the hand-off;
  - collateral locking and release;
  - the contract test (`mn_masternodes_contract.rs`).
- `dwcli`: `mn register`, `mn register-submit`, `mn update-registrar` arguments and output.

**T1 (`regtest/functional/dwd_protx.py`, dashd rc.3).** dwcli registers from the wallet:
- a regular masternode with each collateral kind (External signed by dashd's `signmessage` from a dashd-held coin);
- an evonode with `FundNew` and existing collateral;
- then `protx info` must show the right owner, voting, operator key, payee, service and platform fields;
- ProUpRegTx: change the voting key, then the payout, then the operator key. The operator leg runs on dashd: `bls
  generate` for the new operator, PoSe-banned after the change, then `protx update_service` from dashd revives it.

A second run starts the chain with v24 active (`-vbparams=v24:…:0`, plain BIP9). There it shows that dashd accepts the
wallet's version-2 payloads while dashd's own RPCs build version 3 (D1).

**Mutants** (brief §6; a scratch script applies each, runs the suites, and reports): wrong payload version, payee
script swapped, signature over the wrong hash (ProUpRegTx: the hash with the signature included; ExistingUtxo: the
message without the payload hash), the `FundNew` collateral index off by one, and operator signing attempted (a BLS
secret type or an `update_service` builder added). Each must fail a test.

## 8. Slices

| Slice | Content | Checkpoint |
|---|---|---|
| 1 | This document and the contract delta (§9) | now |
| 2 | ProRegTx regular: `dw-protx::rules`; `build.rs`, `ops.rs`, `keys.rs`; the migration; `registration_keys`, `preview_registration`, `register`, the three External calls, `operations`, `resend_operation`, `discard_operation`, `release_collateral`, `lock_collateral`; dwcli `mn register` and `mn register-submit`; regular oracle vectors; the regular legs of `dwd_protx.py` | report |
| 3 | ProRegTx evonode: platform fields through every collateral kind; evonode vectors; regtest legs | report |
| 4 | ProUpRegTx: the vault owner signer (D11, D12; D31); `preview_update_registrar`, `update_registrar` (keychain and attached owner keys); dwcli `mn update-registrar`; vectors; the regtest legs with dashd's operator | report |
| 5 | The v24-active regtest run, mutants, the desktop gate, code-simplifier, code-review-validator | READY |

## 9. Contract delta (mn-engine.md)

This delta goes into `docs/contracts/mn-engine.md` with slice 2's code, so its contract test stays green at every
commit. That test fails an orphan §2 row, so the rows can't land before the calls.
- `Contract-Version` 3 → 4 (slice 2), → 5 if slice 4 changes a signature;
- §0 files: `protx.rs`, `protx_records.rs` (facade); `protx/build.rs`, `protx/ops.rs`, `protx/keys.rs` (not facade);
- §1 Keys row: "No call signs anything" becomes "§2.3's calls sign under a `MasternodeOp` grant with the
  `MasternodeOwner` scope: the collateral message, a ProUpRegTx's owner signature, and the inputs. None signs as
  operator";
- §2.3 *ProTx management*: the thirteen rows of §2 above, with the refusals in order;
- §3 regenerated;
- §4: the codes of §6;
- §5: `ProTxOperationState`;
- §6: the event after an operation or a collateral lock changes;
- §7: `masternode_ops`, and `masternodes.collateral_released:<outpoint>`.

## D-rows

| D | Decision | Why |
|---|---|---|
| D1 | **Every ProRegTx and ProUpRegTx is version 2 at this pin.** A request only version 3 can carry (a non-IPv4 core address, Platform endpoints on other hosts, a domain, several entries or payees) is `masternodes.unsupported{feature: "protx_v3"}`. The version-3 builder lands with the rust-dashcore pin that can encode and send it (BL row). The oracle compares bytes on chains without v24, where dashd builds version 2. On a v24 chain dashd builds version 3, so there the suite shows dashd accepts the wallet's version 2, with the same `protx info`. | **The network does not require v3 at rc.3.** After v24, `CheckProRegTx` refuses only versions below `BasicBLS` (rc.3 `specialtxman.cpp:1158-1161`). `GetValidatedPayload` refuses only versions above `GetMax` (`:1113`). A v2 ProUpRegTx is a valid change on a v3 masternode: only a downgrade to `LegacyBLS` is refused (`:1132-1146`), and its payout becomes the v3 state's list (`:566-575`). v24 isn't active on mainnet (`chainparams.cpp:213`, `NEVER_ACTIVE`); testnet signals from 2026-09-24 (`:422`). **A v3 payload can't leave this wallet:** the pinned dashcore encodes ProRegTx and ProUpRegTx at version ≤ 2 only (`provider_registration.rs:123,228`), `TransactionPayload` has no raw variant, and dash-spv broadcasts a typed `Transaction` (`client/transactions.rs:32`). **The wallet would misread its own v3 masternode** (MG-03 D25: `shared_unsupported`, no roles, no payouts). Core's RPCs pick the highest version (`providertx_service.cpp:94-100,499`). That is a preference, not a rule, so building v2 is a deviation of choice, not of validity. |
| D2 | The fee comes from coin selection over the wallet's standard accounts, and change goes to a fresh internal address. Core funds only from the payout address or `feeSourceAddress` and returns change there (`providertx_service.cpp:345-357,542`). | MG-00 D25: no fee-source picker. The payout may be another wallet's address. Only `inputsHash` depends on funding, so the oracle rebuilds each payload on dashd's own funded transaction ("the same inputs"). |
| D3 | Inputs and outputs are sorted BIP69. | Core's `fundTransaction` sorts both when the change position is random (rc.3 `wallet/spend.cpp:843,1010,1034`). A `FundNew` payload names its output after sorting, as Core's does (`providertx_service.cpp:552-558`). |
| D4 | `External` takes the outpoint and its collateral address. The wallet checks the address (P2PKH on this network, not this wallet's, not the owner or voting key's), that the outpoint isn't this wallet's coin (that is `ExistingUtxo`), and the signature. The coin's amount, script and unspentness are Core's checks when the block is built. | SPV has no UTXO set and no fetch by outpoint (MG-03 D11), so it can't read another wallet's coin. The message names the payload hash, so a signature by the address proves it controls the key. A mismatched outpoint is refused by Core (`bad-protx-collateral`, `-sig`) and the row stays `sent` (D5). |
| D5 | Before signing, the engine runs Core's rules that the SPV state can check: the trivial-validity rules; network fields; key reuse against the collateral; and duplicates in the synced list (service, operator key under either scheme, platform node id) and in the wallet's own registrations (owner key, collateral). Each refusal carries Core's reject reason. Core's checks that need its full state stay Core's: owner key uniqueness across the whole list, and an External coin. A transaction Core refuses then stays `sent`; the user can resend or wait for `conflicted`. | Dash Core v24 sends no `reject` message (BIP61 removed; rc.3 `net_processing.cpp` has none), so a refused broadcast is silent over P2P. The SPV list carries no owner key (MG-03 §1). A fresh keychain owner key (D13) makes an owner-key clash a key reuse the wallet itself would see. |
| D6 | Operations persist in `masternode_ops`, with the state derived on read. A row with signed bytes that may be out is never deleted. A definite `NotSent` keeps the row as `not_sent` (Discard ends it). Rows are deleted only with `WHERE sent_at IS NULL`, and `sent_at` is set before each hand-off. | MG-06 r1 M-1: a delete raced a resend and lost a fee that had gone out. Here a lost row would hide a registration that later confirms. Deriving the state means a crash leaves no stale state behind. |
| D7 | `resend_operation` needs a `MasternodeOp` grant whose cap covers the saved transaction's outflow. It begins its own lease, reserves the saved inputs under `SpendOwner::Tx(txid)` unless another owner holds one (`send.outpoint_unavailable`), binds the charge, and hands off through the fence as a first send does. | MG-06 r1 M-2: an unfenced, unreserved resend after a restart can double-spend coins another lease holds. The first lease ended with its call, and the bytes move the same duffs if they go out, so the same cap applies. |
| D8 | An `External` registration's fee inputs are locked with the persistent coin lock from prepare until the submit's hand-off settles (released on `Sent`; kept on `MaybeSent` until `confirmed` or `conflicted`). Discard and refresh unlock them. If one is spent anyway (another wallet on the same seed), the row reads `inputs_spent` and `refresh_external_registration` plans a new message. | The signed message covers `inputsHash` (UX-SPEC §4.25.5). In-memory reservations are lost on restart (MG-06 r1 M-2). A lock persists, needs no re-claim race at start, and coin control already honours it. |
| D9 | Collateral locks: an `ExistingUtxo` coin is locked before planning and kept on success. It is unlocked on a definite failure, if this call locked it. A `FundNew` collateral is locked once the wallet holds the transaction. The follower relocks every own registration's collateral unless the user released it; `release_collateral` / `lock_collateral` set and clear that mark. | Core's `CollateralLockGuard` and `AutoLockMasternodeCollaterals` (`providertx_service.cpp:440-458,567,614`; `wallet/wallet.cpp:2515-2525,2867`). MG-00 D38: release is the owner's way to end a masternode, and it must survive a restart and the auto-lock. |
| D10 | ProUpRegTx keeps the operator key and voting key from the synced list when asked to. The **payout must always be named.** The preview reports whether it differs from the wallet's latest record, `None` when the wallet holds none. | Core copies all three from the masternode's state (`providertx_service.cpp:884-931`). SPV has the operator and voting keys (verified), not the payout (MG-03 §1; MG-00 D42's scan isn't built, MG-03 D5). "Keep" from the wallet's records could restore a payout that a ProUpRegTx from elsewhere replaced, and pay rewards to the wrong script. |
| D11 | The owner signature of a ProUpRegTx comes from a new `VaultSigner::sign_update_registrar(OwnerKey, &payload)`. The vault computes `SerializeHash(payload)` itself, and signs compact and recoverable (as `CKey::SignCompact`). `MasternodeOwner` no longer allows `KeyUse::Sign` (a raw digest) at owner paths. That changes MG-05 §1, whose row named this use. | MG-05 D1's argument applies to owner keys: a digest signer at a key signs any sighash for coins sent to that key's address. With the payload built by the vault's caller but hashed by the vault, the engine can only obtain owner signatures over ProUpRegTx payloads. The engine had no other owner-digest caller. MG-12 reviews it. |
| D12 | `OwnerKey::Attached { pro_tx_hash }` signs with the attached `owner` key only for a payload whose `proTxHash` is that masternode's. The engine uses it when no keychain owner key 0–19 has the masternode's owner key id. It also needs the id to be known (a ProRegTx a loaded wallet holds) and the key to match it (MG-03 `KeyCheck::Matched`); otherwise it is `masternodes.no_owner_key`. | MG-05 §6 hands this to MG-04. Binding the key to its ProTx hash makes an attached key useless for any other masternode. Requiring `matched` keeps a wrong key from producing a transaction Core silently refuses (D5). |
| D13 | Keys picked by default: the lowest owner (and voting) keychain index 0–19 whose key id is in none of the wallet's registrations and operations, nor (voting) in the synced list. The lowest operator and platform-node index whose key isn't in the list, the wallet's registrations or its operations. Voting never shares the owner's key. More than 20 owners is `masternodes.keys_exhausted`. | MG-03 scans indexes 0–19 for roles (`VOTING_KEY_INDEXES`). A key beyond them would make a masternode the wallet can't see as its own. UX-SPEC §4.25.5 wants a new owner keypair per masternode and a separate voting key. |
| D14 | "Operator key from the keychain" is the keychain operator **public** key at an index (`masternode_keys(Operator)`, from the xpub, no seed). The registration never derives or touches the secret. The user reveals it later through the existing reveal flow (MG-00 D27). | §5a: the wallet generates no operator secret outside the reveal flow and never signs as operator. |
| D15 | A pasted 64-hex string (a BLS or secp256k1 secret's shape) where a public key or node id is expected is `invalid_argument` with "looks like a secret key". The input is never echoed. | MG-00's MG-04 row; mn-engine D23 (no error repeats a secret-shaped argument). |
| D16 | `operator_reward` is in basis points (u16, at most 10,000). The UI converts the percent. | Core's payload unit (`nOperatorReward`, hundredths of a percent): rc.3 `rpc/evo.cpp:909-916` parses the RPC's percent with two decimals (`ParseFixedPoint(…, 2, …)`) and refuses outside 0–10,000; `rpc/evo.cpp:83` divides by 100 for display; `providertx_service.cpp:526`. MG-00's MG-04 row. |
| D17 | The service address is required. | MG-00 D26. Core accepts none (the masternode starts PoSe-banned, `specialtxman.cpp:1171-1176`); that flow stays on the CLI of a node. |
| D18 | Evonode v2 fields: the platform node id given (40 hex) or from the keychain (`PlatformNode`); the ports given or the network's defaults (`dw-protx::params::default_ports`). Core's rules: the mainnet defaults on mainnet, never 9999, all three ports different, the node id non-null and not in the list. | `CheckProviderNetworkFields` (`providertx.cpp:212-263`), `CheckProRegTx` (`specialtxman.cpp:1276-1281`). UX-SPEC §4.25.5: "as the operator gives them", or filled for a self-operator. |
| D19 | `prepare_external_registration` needs no grant: it signs nothing, and locks coins as the user's coin control may. `submit_external_registration` needs the `MasternodeOp` grant: it signs the inputs, and is charged. | MG-05 §1: a grant is needed to sign. UX-SPEC §4.25.5 asks for the passphrase on the Prove step, not on Review. |
| D20 | dwcli gets `mn register`, `mn register-submit` and `mn update-registrar`, over the facade only. MG-08 owns the rest of `mn`. | The brief. `register-submit` is needed to finish an External registration from the CLI. |
| D21 | T2 stays read-only. The program has no 4,000 tDASH and no testnet masternode host. The evonode is proven by payload equality against dashd's `register_evo` / `register_prepare_evo` / `register_fund_evo` on regtest, and by regtest acceptance. T2 would add an evonode registered on testnet, with its Platform identity created and its credits claimable (MG-03's T2 read). | The brief. |
| D22 | The fee rate is `TxDraft`'s (the relay minimum, 1,000 duffs/kB). | Fee rate isn't a payload field. It changes nothing the oracle compares. |
| D23 | An `ExistingUtxo` coin that is already the collateral of one of the wallet's masternodes, or of an open operation, is `masternodes.collateral_unavailable`. | Core would let a new ProRegTx with the same collateral *replace* the masternode (`specialtxman.cpp:1234-1246`, `:389-394`). Replacing an operator is ProUpRegTx's job (UX-SPEC §4.25.6). UX-SPEC §4.25.5 shows such a coin as "In use". |
| D24 | `masternode_ops` is not carried by `.dwbackup` (dw-appdb `rows.rs` `NOT_EXPORTED`). | A row is this installation's working state: its locks are this `app.sqlite`'s, and a restored wallet re-derives its masternodes from the provider transactions in its history (MG-03). Importing a `sent` row elsewhere would lock coins another installation may have spent. A restored wallet shows no operation for a transaction that may already be out: the registration appears as its masternode once the follower sees the transaction on chain (MG-03, with its collateral relocked by D28), and since no row exists, the wallet has nothing to re-send. |
| D25 | Every row's inputs are locked with the persistent coin locks (`lock_manual`) from its save until it is `confirmed` or `conflicted` (released by `operations()` and the follower) or discarded; this extends D8 to every row. Lock time and `nSequence` are the wallet's ordinary send's: lock time 0 and every input final (`0xffffffff`), as key-wallet's `TransactionBuilder` builds them (pin 8fe0a381 `transaction_builder.rs:739,746`, used by dw-engine `send/mod.rs` `Resolved::builder`), the send's recorded deviation from dash-qt (`m1-engine.md` §2.7.1, QT-066). Core's wallet sets the tip height, one time in ten up to 100 blocks earlier, with `SEQUENCE_FINAL - 1` (rc.3 `wallet/spend.cpp:722-766`, the offset at `:754`, the call at `:1031`, the sequence at `:1025-1029`). | Memory reservations end with the call and the process: after a restart, a row whose bytes may be out must keep its coins from the next send, or two transactions spend them (the manager's slice-2 crash tests). Core keeps them through its own mempool and wallet, which the SPV wallet doesn't have. The user can still unlock a coin in coin control; a second spend then conflicts at the mempool, and nothing is lost. Lock time: the manager's slice-2 ruling, so a ProTx doesn't stand out from the wallet's other transactions. The ruling expected the tip height if dw's send has no random offset; dw's send has no anti-fee-sniping at all (lock time 0), so matching it means 0, and the final sequence with it. Neither field is in any payload hash, so the oracle and the signatures are unaffected; anti-fee-sniping for every send is a follow-up for the send path, not this row. |
| D26 | A transaction without outputs (`ExistingUtxo`, `External`) is funded with a 0-value `OP_RETURN` output, which is dropped again when change is added. | Core's `fundTransaction` does exactly this (rc.3 `wallet/interfaces.cpp:460-462, 501-505`), so a payload's `inputsHash` and the transaction's shape match dashd's for the same coins. |
| D27 | The funding planner is the wallet's own: largest coin first over the automatic-selection candidates (`CoinSource::Any`: not locked, reserved, immature or mixed-only) whose keys the `MasternodeOwner` signer may sign, adding coins until amount and fee are covered; change below dust goes to the fee. | Core's `fundTransaction` is also largest-first (rc.3 `wallet/interfaces.cpp:486-497`), over one address's coins (D2 is the deviation). `TxDraft`'s selector can't take a special payload's size or the dummy output into account. |
| D28 | The follower locks the collateral of each of the wallet's masternodes it holds unspent, every pass, unless the user released it (`masternodes.collateral_released:<txid>-<n>`, wallet scope); `lock_collateral` clears the release. | Core locks every masternode collateral the wallet holds at load and again when a spend of it is abandoned (rc.3 `wallet/wallet.cpp:2515-2525` `AutoLockMasternodeCollaterals`, `:1218-1220`, `LockProTxCoins` at `:2867`); its `lockunspent` unlock lasts until the restart, which the release outlives (MG-00 D38: a released coin stays spendable). |
| D29 | `preview_registration` issues nothing: a `NewAddress` payout and a `FundNew` collateral address are empty in its terms, and planned with a stand-in P2PKH script of the same size. | A preview the user cancels must not use up receive addresses (gap limit). The fee doesn't change: both are 25-byte P2PKH scripts. |
| D30 | A given operator key is refused when the list holds it under either BLS scheme (the key, or its legacy serialization), also before v24. | Core compares `pubKeyOperator` as stored (rc.3 `evo/specialtxman.cpp:1250-1264`, `HasUniqueProperty`); a legacy and a basic serialization of one key are the same secret. Refusing both is stricter, and only ever refuses a key someone else's masternode already uses. |
| D31 | The vault's owner signer for ProUpRegTx (D11, D12) moves from slice 2 to slice 4, with MG-05's row update and the refusal test. | Slice 2 signs only the collateral message and the inputs, which the `MasternodeOwner` scope already allows. The signer lands with its first caller. |
| D32 | Slice 2's crash tests (manager, checkpoint 1): the hand-off stops after the sign, after the save, and after the library returned but before the outcome is recorded; the session closes and reopens on the same data. After the restart: no row and no lock (after the sign), a `signed` row with its inputs locked (after the save), a `sent` row that `discard_operation` refuses (after the hand-off); a new registration doesn't spend the first row's inputs, and a resend of the first sends its same bytes once more. | The brief's "nothing is sent twice; nothing is lost; no row whose transaction may be out is deleted", tested in `masternodes/protx/tests.rs` against a fake library. |
| D33 | A request's own checks run before the list is read: `invalid_argument`, `masternodes.unsupported{protx_v3}` and a port Core refuses come before `masternodes.list_unavailable`; an operation id that isn't 32 lower-case hex is `invalid_argument`; `wallet_not_found` doesn't repeat the id. | mn-engine §2's order for MG-03's calls ("arguments are checked first") and MG-03 D23. |
| D34 | `last_error` keeps a send failure's `send.*` code (`send.no_peers`, `send.broadcast_unknown`, …), the strings of dw-ffi's and dw-app's `SendCode`, mapped in the engine by an exhaustive match. | The engine's own `code()` of a send failure is `send`, which says nothing about whether to resend. |
| D35 | dwcli also gets `mn operations`, `mn resend`, `mn discard`, `mn register-refresh`, `mn release-collateral` and `mn lock-collateral`, each one facade call. Its grant is `MasternodeOp` capped at the previewed fee (`register`) or the row's fee. A send failure's code is `send.<variant>`. | The regtest suite and a user without the UI need to see and finish what `register` started. They're thin; MG-08 can restyle them. |
| D36 | The evonode's regtest legs are `FundNew` (the keychain's node id, the network's default ports) and `ExistingUtxo` (a node id and ports given). An evonode with an `External` collateral is shown by the engine test `mg04_evonode_external_signs_over_its_platform_fields` and the `prepared_evo` vector (dashd's `register_prepare_evo` / `register_submit`), not on regtest. The legs take the expected node id from the preview's terms (index 0), since dwcli has no keys call, and read Core's ports from `protx info`'s `addresses` (`platform_p2p`, `platform_https`). | §7 T1 names the evonode with `FundNew` and existing collateral. The External flow differs for an evonode only in the payload fields, which its sign message covers through the payload hash, and the regular X leg already runs Core's `signmessage` path. The node-id derivation is the engine's (`masternode_keys`, MG-00) and is tested there. rc.3 prints the scalar `platformP2PPort` / `platformHTTPPort` only under `-deprecatedrpc=service` (`evo/dmnstate.cpp:119-137`); otherwise a version-2 state's ports are in `addresses` at the Core address (`:146-168`, names `netinfo.h:95-100`). |
| D37 | A ProTx's coin selection may spend a coin of exactly a collateral amount (1,000 or 4,000 DASH) that isn't yet a registered collateral, as the wallet's ordinary send may; it holds back only locked coins and the wallet's masternodes' collaterals (D9, D28). The user keeps a coin meant as a collateral by locking it in coin control, or by registering it first. | Core's selection for every send, a ProTx's included, is `CoinType::ALL_COINS`, which takes any amount (rc.3 `wallet/spend.cpp:277-280`); only `ONLY_MASTERNODE_COLLATERAL` picks by amount (`:269-271`), for a collateral picker. Core funds a ProTx from the payout or fee-source address only (D2), so there the spend is limited to that address's coins. Holding back collateral-sized coins in the ProTx alone would make it select differently from the wallet's send; a rule for both belongs to the send path (MG-00). Found by regtest: with largest-first selection (as the send's, `send/mod.rs` `Resolved::builder`), the regular legs' fees and the evonode `FundNew` spent the 4,000 DASH coin the `ExistingUtxo` leg meant to register, so the suite sends that coin just before W, and W runs before V. |
| D38 | An owner payload signature (`KeyUse::OwnerPayload`) is reachable only through the `MasternodeOwner` scope. `SignerScope::Full`, which a `Spend` grant's signer has and which signs any use at any path, excludes it; the voting and DashPay scopes never had it. | A ProUpRegTx replaces a masternode's operator key, voting key and payout. MG-05 binds that to a `MasternodeOp` grant capped at the fee and a `key_change` lease (§2); a `Full` signer handed out for a payment must not be able to make one. Tested in `dw-vault`'s `full_scope_allows_everything_but_owner_payloads` and `only_a_masternode_op_grant_signs_owner_payloads`. |
| D39 | A kept operator key is the list's key in the basic serialization: a version-1 entry's legacy bytes are re-encoded (`dw_protx::bls::basic_form`); bytes that parse in neither form are `masternodes.protx_refused{bad-protx-key-null}`. Whether the operator key changes (`operator_changes`, `pose_bans`) compares the keys under both forms. | Core keeps `dmn->pdmnState->pubKeyOperator` and re-encodes a legacy one for a basic payload (rc.3 `evo/providertx_service.cpp:902-907`). Its ban on an operator change compares the lazy keys, not their bytes (`evo/specialtxman.cpp:538,557-561`), so a re-encoded key is no change. |
| D40 | `bad-protx-collateral-reuse` (the new voting key, or the owner key, at the collateral's address) is checked when the wallet knows the collateral's script: its own coin, or the address of one of its `External` registrations (the row's terms). For a masternode whose collateral is another wallet's and was registered elsewhere, only Core checks it. | Core reads the collateral coin from the UTXO set (rc.3 `evo/specialtxman.cpp:1486-1500`, `evo/providertx.cpp:148-153`); SPV has no UTXO lookup (MG-03 D11's `collateral_lookup`). A transaction Core would refuse there fails at the hand-off, as D5 accepts for the checks the wallet can't run. Widening the check is BL-302 (manager, slice 4 checkpoint). |
| D41 | `UpdateRegistrarPreview` also carries `terms`, and an update's row stores them: type and service from the list, the owner address from the ProRegTx (with its keychain index), the voting and operator keys as they will be (kept or set, with the keychain index of a set one), the payout (empty for a preview's `NewAddress`, D29), the operator reward from the wallet's record (0 when no loaded wallet holds the ProRegTx). The collateral and platform fields are empty: a ProUpRegTx carries neither. | §3's record had only the flags; the confirm screen (MG-08) shows what the masternode will read, and a saved row counts its new keys as in use (`InUse::add_terms`, D13) until it is discarded. |
| D42 | dwcli `mn update-registrar <hash> --payout <new or an address>`, with `--operator-key K` or `--operator-index N`, `--voting-address A` or `--voting-index N`, and `--preview`: a key not given is kept; `--payout` is required. Its grant is `MasternodeOp` capped at the preview's `total_debit`. | D10: the payout is always named, so the command line can't change it by omission. The cap follows D35. |
| D43 | A kept key is the synced list's at its height (`list_state.height`), which may trail the tip by the blocks since dash-spv's last list update. So a keep (of the operator or the voting key) is refused while an operation of any loaded wallet on the same masternode (a ProUpRegTx, or the ProRegTx itself) is `signed`, `not_sent` (it can still be resent) or `sent`, or `confirmed` above the list's height: `masternodes.update_pending{operation}`, naming it. All loaded wallets count because an attached owner key updates a masternode from any of them (D24). Giving every key is allowed. A resend of a ProUpRegTx is refused while another update of the masternode saved no earlier (by `created_at`, in seconds, so two updates of one second block each other's resend) is `signed`, `not_sent`, `sent` or `confirmed`: `masternodes.update_superseded{operation}`, checked after the row's state; the user discards this row, or sends a new update with every key (the named operation may be another loaded wallet's). `update_registrar`, and `resend_operation` of an update, hold one lock from these checks until the row is saved (or the resend handed off), so two can't both pass. A `conflicted` row isn't in the way. The preview shows the kept keys. The regtest legs retry while `update_pending` refuses (the wallet's history can reach a block before the list does). | Core keeps from its tip's state. SPV has no other source of a masternode's current keys (D10). A ProUpRegTx built on a stale key would quietly restore it: a quick second update could hand back an operator key the first rotated out (manager, slice 4 checkpoint); a resent older update could revert a newer one's voting key or payout, which Core's mempool doesn't stop when neither changes the operator key (rc.3 `txmempool.cpp:1579-1590,1683-1687`). Not covered: an unloaded wallet's operations, and an update from another wallet or Core before the list shows it; Core's own RPC has the same window for a transaction it hasn't seen. Two updates that both give every key can still be mined in either order (follow-up). Tested by `mg04_a_keep_waits_until_the_list_shows_the_last_update`, `mg04_a_resendable_update_blocks_a_keep_and_a_later_update_blocks_its_resend`, `mg04_an_update_resends_unless_a_later_one_exists` and `mg04_a_keep_waits_for_another_loaded_wallets_update`. |
| D44 | An update's row: `kind: update_registrar`, `pro_tx_hash` the masternode's (display order), the request as JSON, its fee inputs locked until it settles (D25), no collateral lock. A resend runs under a `key_change` lease. | §5's columns; `resend_operation` already maps the kind to `FlowKind::KeyChange` (§2). |
| D45 | `dwcli mn --sync-height` also waits until the list reads synced for 2 s in a row (8 checks 250 ms apart). | At each start dash-spv replays the stored lists, which read synced, then asks its peer for the tip's QRInfo, and the list reads unsynced (`masternodes.list_unavailable`) until the answer (MG-03 D3: only a synced list counts). A call landing in that window failed twice in the v24-active run's first tries (steps 4 and 8, SPV debug log: the call read the list 4 ms after the QRInfo request). A test-tool wait, not an engine rule: the desktop app is one long session. |
| D46 | The v24-active run (`DWD_MG04_V24=1`): every wallet ProRegTx and ProUpRegTx is version 2 and dashd accepts it, while dashd's own `protx update_service` builds version 3 (asserted). Core's own masternodes then hold a version-3 state (extended addresses), and the step-8 refusal on one reads `masternodes.unsupported{feature: "shared_masternodes"}`, not `no_owner_key`: the masternode check comes first (§2.3). The default run keeps `no_owner_key`. | D1: this pin builds version 2 only. A version-3 entry is refused as unsupported (D30's `has_extended_net_info`). |
| D47 | MG-04 and MG-06 (own proposals) meet in the coins. (a) A proposal fee the wallet hasn't seen may be out, and a [Submit] resends it, claiming its coins again (MG-06 D10). An `ExistingUtxo` collateral (preview and registration) is refused (`collateral_unavailable`) if a payment claims it in the session, or if a saved proposal fee the wallet hasn't seen spends it, as after a restart that dropped the claim (`NetworkSession::unseen_proposal_fee_inputs`; a failed fee, MG-06 D5, has no resend and holds nothing). (b) The other way round, MG-06's resend now refuses a coin user-locked since the fee was built, which is how MG-04 locks a collateral (D9) and a row's inputs (D25); MG-06 D10 is amended to say so. `FundNew`, update and `External` funding still take any unclaimed, unlocked coin (`protx_candidates`, `CoinSource::Any`), so an unseen fee's coin can fund a ProTx after a restart. The ProTx and the fee then conflict, and one of them is dropped: no coin is lost, as with any payment under MG-06 D10. | Before this, a fee left unsent over a restart could spend a collateral locked since and drop the masternode, or a registration could take a coin an out fee spends. The resend can't tell MG-04's locks from coin control (both are `lock_manual`), so it refuses all of them: the safe direction, at the cost of dash-qt's lock-blind Resend. Validator 10-21 r1: one Major (D10 contradiction, fixed by the amendment) and one Minor (the restart direction, fixed by (a)); r2 ACCEPT, its Minor (a failed fee held its coins forever) fixed. Tests: `mg04_a_coin_an_unseen_proposal_fee_spends_is_not_a_collateral`, `governance::proposals::tests::{an_unseen_fees_coins_are_held_from_a_collateral, a_failed_fees_coins_are_not_held, a_resend_refuses_a_user_locked_coin}`. Six mutants killed: the `existing_coin` check, each of the two call sites, the unseen and the failed filters, and the resend's `user_locked` filter. |
| D48 | In the v24-active run, the wallet's part ends before dashd's operator revive: `operations` is read once U3 is mined, then the revive and its Core-side checks run with no further wallet call. The default run keeps the original order. | The revive is dashd's `protx update_service`, which builds a version-3 ProUpServTx under v24 (D46). The wallet's dashcore pin (`8fe0a381`, platform's) rejects it (`parse failed: unsupported ProUpServTx version`, `provider_update_service.rs:182`), so a block holding one can't be decoded. When the wallet's filter matches that block, dash-spv asks for it every 15 s and never takes it, and the wallet never scans its height. Here the block's coinbase paid one of the wallet's masternodes, which depends on the payee order, so two v24 runs hung at `operations` (10-20 and 10-21; the block, the payee and the decode error were checked on the kept datadir). rust-dashcore `dev` decodes version 3 since #1138 (0d215f3f4) and #1152 (d775b1bf8), 10-11; picking them up needs a pin move (DEC-09), outside MG-04. Until then, a v24-active network stalls a wallet whose filter matches a block with any version-3 ProTx (follow-up BL row). |
