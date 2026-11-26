//! Tests of shielded_pool.rs (0.1 E1): the protocol-version gate, the
//! status and history read from the library, the quotes' fees and
//! refusals, and the spends on requests_tests.rs's fixture over a scripted
//! library ([`FakeShielded`]): what a spend charges its lease, which
//! refusals keep the grant, and how the library's failures map. Then the
//! real library on topup_tests.rs's fixture: the viewing key binds from the
//! vault once and from storage after a restart with the vault locked, and
//! the facade shows the mobile wallets' address for "abandon … about".
//! The testnet round trip is the smoke (`/work/scratch/dw-rel/e1/smoke.md`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::address::Payload;
use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::{Address, Network, OutPoint, PubkeyHash, Txid};
use dpp::address_funds::{OrchardAddress, PlatformAddress};
use dpp::identity::state_transition::asset_lock_proof::InstantAssetLockProof;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::prelude::AssetLockProof;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::{ArtifactKind, ArtifactOrigin, NewShieldedArtifact};
use dw_vault::VaultError;
use platform_wallet::changeset::{
    AssetLockChangeSet, AssetLockEntry, PlatformWalletChangeSet, PlatformWalletPersistence,
};
use platform_wallet::wallet::shielded::note_selection::{ShieldedFeeKind, select_notes_with_fee};
use platform_wallet::wallet::shielded::store::StalePendingSpend;
use platform_wallet::wallet::shielded::{
    OrchardKeySet, ShieldedActivityEntry, ShieldedActivityKind as Kind,
    ShieldedActivityStatus as Status, ShieldedDirection as Direction, ShieldedNote,
};
use platform_wallet::{
    AssetLockFundingType, AssetLockStatus, PlatformWalletError, TrackedAssetLock,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::credits::FundAmount;
use super::errors::{PlatformError, ShieldedError, ShieldedSyncWait};
use super::flows::FlowKind;
use super::lease::{CallPermit, FundingKey, Lease};
use super::registration::{
    RegistrationWait, SHIELDED_REST_SECS, ShieldedCheck, ShieldedFacts, shielded_readiness,
};
use super::registration_flow::{FundError, LockInfo, LockStatus, Spendable};
use super::requests::tests::Fixture;
use super::shield_drain::{self as drain, DrainCoin};
use super::shielded::{
    SHIELDED_ACTIVATION_VERSION, ShieldState, ShieldedAccount, ShieldedActivityKind,
    ShieldedActivityStatus, ShieldedDirection, ShieldedScanProgress, ShieldedSpendState,
    UnresolvedSpend,
};
use super::shielded_pool::{
    self as pool, Applied, Ledger, MAX_LOCK_INPUTS, ShieldedLib, SpendError, SyncState, on_pool,
};
use super::unfinished_locks::SpentRead;
use super::withdrawal::{Rules, expected_duffs};
use crate::{DashNetwork, ImportOptions, NetworkSession};

mod to_platform;
mod unfinished;

const LATEST: u32 = 15;
const NOTE: u64 = 300_000_000;
/// Lane V's reference: iOS and Android show this shielded address for
/// "abandon … about" on mainnet.
const ABANDON_MAINNET: &str =
    "dash1zrlpvqcgyx2rag5d5rkhq0wy69hz3q37xe54xlrreqgp8htpv3gmm2l52gznxha0fd3kq9s4l67rj";

/// A shielded address of an unrelated regtest wallet.
fn payee() -> OrchardAddress {
    let keys = OrchardKeySet::from_seed(&[7; 64], Network::Regtest, 0).unwrap();
    let raw = keys.viewing_keys().default_address.to_raw_address_bytes();
    OrchardAddress::from_raw_bytes(&raw).unwrap()
}

fn payee_text() -> String {
    payee().to_bech32m_string(Network::Regtest)
}

/// The `n`th address of the wallet's Platform payment account.
fn own_platform(n: u8) -> PlatformAddress {
    PlatformAddress::P2pkh([n.wrapping_add(0x50); 20])
}

fn platform_text(address: PlatformAddress) -> String {
    address.to_bech32m_string(Network::Regtest)
}

fn core_address(n: u8) -> Address {
    Address::new(
        Network::Regtest,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([n; 20])),
    )
}

fn note(position: u64, value: u64) -> ShieldedNote {
    ShieldedNote {
        position,
        cmx: [position as u8; 32],
        nullifier: [position as u8 + 100; 32],
        block_height: 1,
        is_spent: false,
        value,
        note_data: Vec::new(),
    }
}

/// The signed transition of spend `n` of `kind`'s family, publishing
/// `nullifiers` against `anchor`.
pub(super) fn signed_spend(
    n: u8,
    kind: &platform_wallet::wallet::shielded::ShieldedDispatchKind,
    anchor: [u8; 32],
    nullifiers: &[[u8; 32]],
) -> Vec<u8> {
    use dpp::identity::core_script::CoreScript;
    use dpp::serialization::PlatformSerializable;
    use dpp::shielded::SerializedAction;
    use dpp::state_transition::StateTransition;
    use dpp::state_transition::identity_top_up_from_shielded_pool_transition::IdentityTopUpFromShieldedPoolTransition;
    use dpp::state_transition::identity_top_up_from_shielded_pool_transition::v0::IdentityTopUpFromShieldedPoolTransitionV0;
    use dpp::state_transition::shielded_transfer_transition::ShieldedTransferTransition;
    use dpp::state_transition::shielded_transfer_transition::v0::ShieldedTransferTransitionV0;
    use dpp::state_transition::shielded_withdrawal_transition::ShieldedWithdrawalTransition;
    use dpp::state_transition::shielded_withdrawal_transition::v0::ShieldedWithdrawalTransitionV0;
    use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::IdentityCreateFromShieldedPoolTransition;
    use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::v0::IdentityCreateFromShieldedPoolTransitionV0;
    use dpp::state_transition::unshield_transition::UnshieldTransition;
    use dpp::state_transition::unshield_transition::v0::UnshieldTransitionV0;
    use platform_wallet::wallet::shielded::ShieldedDispatchKind as K;
    let actions: Vec<SerializedAction> = (nullifiers.iter())
        .map(|&nullifier| SerializedAction {
            nullifier,
            rk: [n; 32],
            cmx: [0x30 + n; 32],
            encrypted_note: vec![0x40 + n; 216],
            cv_net: [0x50 + n; 32],
            spend_auth_sig: [0x60 + n; 64],
        })
        .collect();
    let (proof, binding_signature) = (vec![0x70; 512], [0x71; 64]);
    let st = match kind {
        K::Transfer => StateTransition::ShieldedTransfer(ShieldedTransferTransition::V0(
            ShieldedTransferTransitionV0 {
                actions,
                value_balance: 1_000 + u64::from(n),
                anchor,
                proof,
                binding_signature,
            },
        )),
        K::Unshield => StateTransition::Unshield(UnshieldTransition::V0(UnshieldTransitionV0 {
            output_address: PlatformAddress::P2pkh([0x51; 20]),
            actions,
            unshielding_amount: 100_000_000 + u64::from(n),
            anchor,
            proof,
            binding_signature,
        })),
        K::Withdraw => StateTransition::ShieldedWithdrawal(ShieldedWithdrawalTransition::V0(
            ShieldedWithdrawalTransitionV0 {
                actions,
                unshielding_amount: 100_000_000 + u64::from(n),
                anchor,
                proof,
                binding_signature,
                core_fee_per_byte: 1,
                pooling: dpp::withdrawal::Pooling::Never,
                output_script: CoreScript::from_bytes(vec![0x76, 0xa9, 0x14]),
            },
        )),
        K::IdentityTopUp { identity_id } => StateTransition::IdentityTopUpFromShieldedPool(
            IdentityTopUpFromShieldedPoolTransition::V0(
                IdentityTopUpFromShieldedPoolTransitionV0 {
                    identity_id: (*identity_id).into(),
                    actions,
                    top_up_amount: 100_000_000 + u64::from(n),
                    anchor,
                    proof,
                    binding_signature,
                },
            ),
        ),
        K::IdentityCreate { identity_id } => StateTransition::IdentityCreateFromShieldedPool(
            IdentityCreateFromShieldedPoolTransition::V0(
                IdentityCreateFromShieldedPoolTransitionV0 {
                    public_keys: Vec::new(),
                    denomination: 100_000_000,
                    actions,
                    anchor,
                    proof,
                    binding_signature,
                    send_to_address_on_creation_failure: PlatformAddress::P2pkh([0x52; 20]),
                    identity_id: (*identity_id).into(),
                },
            ),
        ),
        K::PrivateDocumentCreate => panic!("no spend of the engine's is a private document"),
    };
    st.serialize_to_bytes().unwrap()
}

fn entry(kind: Kind, amount: u64, status: Status) -> ShieldedActivityEntry {
    ShieldedActivityEntry {
        id: [amount as u8; 32],
        kind,
        direction: Direction::Out,
        amount,
        fee: None,
        counterparty: None,
        memo: None,
        block_height: None,
        status,
        created_at_ms: 0,
        min_note_position: None,
        note_cmxs: Vec::new(),
        spent_nullifiers: Vec::new(),
    }
}

fn lock(n: u8, status: LockStatus) -> LockInfo {
    LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
        registration: false,
        identity_index: 0,
        account: 0,
        amount: 1_000_000 * u64::from(n),
        status,
    }
}

/// A shield's asset-lock transaction of its own (input `0x66…:n`, one
/// credit output), for a tracked row of the real library.
fn lock_transaction(n: u8) -> dashcore::Transaction {
    use dashcore::transaction::special_transaction::TransactionPayload;
    use dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
    dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0x66; 32]), u32::from(n)),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: vec![dashcore::TxOut {
                    value: 1_000_000,
                    script_pubkey: core_address(n).script_pubkey(),
                }],
            },
        )),
    }
}

/// The InstantSend proof of output 0 of `tx`.
fn instant_proof(tx: &dashcore::Transaction) -> AssetLockProof {
    let islock = InstantLock {
        txid: tx.txid(),
        ..InstantLock::default()
    };
    AssetLockProof::Instant(InstantAssetLockProof::new(islock, tx.clone(), 0))
}

/// The dispatch hook's answer to a create, as a test scripts it.
pub(super) type CreateHook = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// What a spend handed the library.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spent {
    operation: &'static str,
    to: String,
    credits: u64,
    memo: Vec<u8>,
    /// The full viewing key of the key set the lease released.
    fvk: [u8; 96],
}

/// What the scripted library holds of a send that failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum AfterFailure {
    /// Nothing: its notes are free.
    #[default]
    Free,
    /// Its notes, resolved with no proof of either outcome.
    Held,
    /// Its signed transition, still armed, and its reservations: the write
    /// that ends the spend failed, so a later pass may send it.
    Armed,
    /// Unknown: the read of the held spends fails.
    Unreadable,
    /// Unknown: the read of the stored transitions fails.
    StoredUnreadable,
}

/// platform-wallet's shielded side, scripted.
pub(crate) struct FakeShielded {
    version: u32,
    /// What a proved read of the version returns.
    refreshed: u32,
    pub(crate) refreshes: AtomicU32,
    bound: bool,
    /// What a bind says while not `bound`.
    unbound_as: ShieldedAccount,
    ledger: Option<Ledger>,
    notes: Vec<ShieldedNote>,
    /// The unspent notes from now on, when set (a pass moved them).
    notes_now: Mutex<Option<Vec<ShieldedNote>>>,
    activity: Vec<ShieldedActivityEntry>,
    locks: Vec<LockInfo>,
    /// The locks the wallet's shield steps signed; `None` while the
    /// journal is unread.
    own: Option<HashSet<Txid>>,
    /// The locks the journal records settled.
    settled: Mutex<HashSet<Txid>>,
    /// Every settle, in order.
    settles: Mutex<Vec<Txid>>,
    /// The next apply's answer.
    applies: Mutex<Option<Result<Applied, PlatformError>>>,
    /// The answers after it, in order.
    queued: Mutex<std::collections::VecDeque<Result<Applied, PlatformError>>>,
    /// The outpoint of every apply, in order.
    applied: Mutex<Vec<OutPoint>>,
    /// The next proof wait holds here, as the library's does until its
    /// proof or its bound.
    proof_hold: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// The coordinator has no dispatch hook.
    unhooked: AtomicBool,
    /// The library's key set spends are off.
    no_keyset: AtomicBool,
    /// What the next `sync_now` does before it returns (a pass's writes).
    pub(super) on_sync: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Platform's shielded note count; unknown when `None`.
    pub(super) pool_notes: Mutex<Option<u64>>,
    /// What an identity create asks of the dispatch hook before it signs
    /// (`Err`: the hook refused it).
    pub(super) create_hook: Mutex<Option<CreateHook>>,
    /// The answers of the next reads of the hook, in order, before
    /// `unhooked` answers.
    hook_reads: Mutex<std::collections::VecDeque<bool>>,
    /// The library cannot spend from the key set.
    pub(super) no_keyset_spends: AtomicBool,
    /// The spends the library resolved unproved and holds the notes of.
    held: Mutex<HashSet<[u8; 32]>>,
    /// What `holds` reads; `held` is not derived from it.
    holds: Mutex<Vec<pool::Hold>>,
    /// Every note, spent ones too; `notes` when `None`.
    all_notes: Mutex<Option<Vec<ShieldedNote>>>,
    /// `all_notes` cannot be read.
    notes_unreadable: AtomicBool,
    /// How many times `all_notes` was read.
    notes_reads: AtomicU32,
    /// What a send that fails (not an unconfirmed one) leaves.
    after_failure: Mutex<AfterFailure>,
    /// The library's held spends cannot be read.
    held_unreadable: AtomicBool,
    /// Every acknowledgement of a spend's outcome, in order.
    pub(super) acks: Mutex<Vec<[u8; 32]>>,
    /// The library's stored transitions cannot be read.
    stored_unreadable: AtomicBool,
    /// The receive addresses issued.
    issued: AtomicU32,
    /// The session a shield's funding records its lock in; none for tests
    /// that fund nothing.
    funder: Mutex<Option<(std::sync::Weak<NetworkSession>, crate::WalletId)>>,
    /// The locks the fundings built, proved.
    funded: Mutex<Vec<LockInfo>>,
    syncs: AtomicU32,
    spendable: Spendable,
    /// The coins a funding may select.
    coins: Vec<u64>,
    receive: String,
    /// The wallet's Platform payment account; its pool read fails when
    /// `None`.
    platform: Mutex<Option<pool::PlatformPool>>,
    network: Network,
    /// The next spend's failure, after the keys are out.
    fails: Mutex<Option<PlatformWalletError>>,
    spent: Mutex<Vec<Spent>>,
    /// As the library keeps them: the records of unconfirmed spends, and
    /// their activity rows (shown before `activity`).
    unresolved: Mutex<Vec<UnresolvedSpend>>,
    /// The anchored reservations, as the library holds them in memory: an
    /// unconfirmed spend adds its notes' (`reserved(n)`).
    reserved: Mutex<Vec<StalePendingSpend>>,
    /// An unconfirmed spend's record write fails (`arm_redrive_record`):
    /// the library keeps its reservations but no record.
    loses_record: AtomicBool,
    rows: Mutex<Vec<ShieldedActivityEntry>>,
    /// What the next pass finds of the unconfirmed spends: `Confirmed`
    /// (their notes spent) or `Failed` (the prune backstop released them).
    scan_finds: Mutex<Option<Status>>,
    /// The ledger the next pass establishes (the notes it found), when set.
    scan_ledger: Mutex<Option<Ledger>>,
    /// The library's sync loop as the status reads it.
    sync: Mutex<SyncState>,
    /// The locks of unknown consumption, as `app.sqlite` keeps them.
    unknown: Mutex<HashSet<Txid>>,
    /// The shield lock rows, when set: exactly these (else `locks` and
    /// `funded`, each with its status's proof, the payload's credit
    /// outputs counted by txid).
    lock_rows: Mutex<Option<Vec<super::unfinished_locks::KindLock>>>,
    /// The credit script every lock's row names (without `real_rows`).
    script: Mutex<Option<dashcore::ScriptBuf>>,
    /// The keys the next apply signs with, through its signer, before it
    /// answers; and whether each signed.
    sign_next: Mutex<Vec<key_wallet::bip32::DerivationPath>>,
    signed: Mutex<Vec<bool>>,
    /// The next funding's inputs and credit script, when set (with
    /// `real_rows`): a drain of the CoinJoin account into Shielded, funded
    /// under the shield's own step (MIX-SHIELD).
    funds_next: Mutex<Option<(Vec<OutPoint>, dashcore::ScriptBuf)>>,
    /// Asked during each spend's library call; its answers, in order.
    probe: Mutex<Option<Box<dyn Fn() -> bool + Send>>>,
    probed: Mutex<Vec<bool>>,
    /// The shield locks are the library's own tracked rows, in the
    /// funder's wallet and store, read through the real [`pool::PwShielded`]
    /// with its journal and `app.sqlite` marks (DEC-1051 tests).
    real_rows: bool,
    /// The next apply answers as the library does on Platform's "already
    /// consumed" (`PlatformWalletError::AssetLockAlreadyConsumed`), after
    /// its transition was signed.
    consumed: AtomicBool,
    /// Platform's proved spent read of each outpoint; unread when unset.
    pub(super) spent_reads: Mutex<std::collections::HashMap<OutPoint, SpentRead>>,
    /// The locks Platform proved partly used, unless `real_rows`.
    pub(super) partly_used: Mutex<HashSet<OutPoint>>,
    /// The next applies end with these library errors, in order, after
    /// their transitions were signed, mapped as `PwShielded::apply` maps
    /// them.
    lib_errors: Mutex<std::collections::VecDeque<PlatformWalletError>>,
    /// What the next apply leaves of the lock's real row.
    row_after: Mutex<RowAfter>,
    /// The coin reads and the funding permits asked for.
    coin_reads: AtomicU32,
    permits: AtomicU32,
    /// What holds a shield's funding back (`funding_hold`), whether the
    /// bring-up runs (`startup_running`) and whether SPV is stopped.
    hold: Mutex<Option<RegistrationWait>>,
    startup_running: AtomicBool,
    spv_stopped: AtomicBool,
    /// CoinJoin account 0's coins (`drain_coins`).
    drain: Mutex<Vec<DrainCoin>>,
    /// The coins a drain's build spends, as the library selects them when
    /// it builds (every coin of the account, so also one that came after
    /// the plan); the plan's when unset.
    pub(super) drain_built: Mutex<Option<Vec<DrainCoin>>>,
    /// Each drain's inputs.
    drained: Mutex<Vec<Vec<OutPoint>>>,
    /// Per drain funding: whether the wallet's CoinJoin account was
    /// claimed for it (`claim_sweep` refused `MoveRunning`).
    drain_claimed: Mutex<Vec<bool>>,
    /// Per drain funding: the smallest lock it may build.
    pub(super) drain_floors: Mutex<Vec<u64>>,
    /// BIP44 account 0 as a Core Max reads it (`core_coins`); a Core Max's
    /// funding is a drain's over these, its build `drain_built`'s.
    pub(super) core: Mutex<super::core_max::CoreCoins>,
    /// How the next funding (a shield's or a drain's) ends once its
    /// inputs are signed; it funds a proved lock when unset.
    pub(super) fund_end: Mutex<Option<FundEnd>>,
    /// The rest clock as `app.sqlite` keeps it; `None` when unreadable.
    rest: Mutex<Option<pool::RestClock>>,
    /// The clock's writes return `Ok` and keep nothing.
    drops_rest: AtomicBool,
    /// A withdrawal runs the engine's hook steps in the funder's session
    /// first, as the library's first send does: the payout's accept, then
    /// the artifact, unless `drops_artifact`.
    hooks: AtomicBool,
    drops_artifact: AtomicBool,
}

/// A funding that fails after its signatures, as platform-wallet ends one
/// at the pin (`build.rs` `broadcast_funded_asset_lock_with_funding`),
/// mapped by the engine's own `fund_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FundEnd {
    /// The broadcaster refused the lock: its tracked row removed and its
    /// inputs released (`TransactionBroadcast`).
    Rejected,
    /// A drain built below its floor, dropped before it was tracked or
    /// sent (`AssetLockTransaction`).
    BelowFloor,
    /// No verdict from the broadcaster (`TransactionBroadcastUnconfirmed`);
    /// the row is lost here, so only the step's record knows the lock.
    Unknown,
    /// Sent, and no proof within the wait (`FinalityTimeout`): its row
    /// stays `Broadcast` with no proof, stored (`real_rows`, shields only).
    ProofTimeout,
    /// Its wallet removed or re-created after the last signature, before
    /// the tracking write: nothing tracked or sent
    /// (`WalletRemovedBeforeSend`).
    RemovedBeforeSend,
    /// Over the asset lock's input cap after the last signature: nothing
    /// tracked or sent (`AssetLockTooManyInputs`).
    OverInputCap,
}

impl FundEnd {
    /// Whether the library tracked the lock first, so the session's tap
    /// recorded it as the step's.
    fn tracked(self) -> bool {
        !matches!(
            self,
            Self::BelowFloor | Self::RemovedBeforeSend | Self::OverInputCap
        )
    }

    fn error(self) -> FundError {
        use platform_wallet::PlatformWalletError as E;
        super::registration_flow::fund_error(match self {
            Self::Rejected => E::TransactionBroadcast("rejected".into()),
            Self::BelowFloor => E::AssetLockTransaction("below the required minimum".into()),
            Self::Unknown => E::TransactionBroadcastUnconfirmed("no verdict".into()),
            Self::ProofTimeout => E::FinalityTimeout(OutPoint::null()),
            Self::RemovedBeforeSend => E::WalletRemovedBeforeSend("w".into()),
            Self::OverInputCap => E::AssetLockTooManyInputs {
                count: 101,
                max: 100,
            },
        })
    }
}

/// What a library call leaves of a lock's tracked row (`real_rows`), as
/// platform-wallet changes it at the pin (`pool::resume`'s doc).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RowAfter {
    #[default]
    Unchanged,
    /// "Already consumed" with its ChainLock proof: `RecoveredFromChain`
    /// (`orchestration.rs:478-521`, `sync/tracking.rs:351`).
    Recovered,
    /// Chain-locked locally: `ChainLocked` with the outpoint's ChainLock
    /// proof (`sync/recovery.rs:1693`, `sync/proof.rs:443-455`).
    ChainLocked,
    /// A ChainLock proof refused, the record's InstantSend proof back
    /// (`sync/tracking.rs:564-620`).
    InstantSend,
    /// A ChainLock proof refused with nothing better: `Broadcast`, no proof.
    Broadcast,
}

impl Default for FakeShielded {
    fn default() -> Self {
        Self {
            version: LATEST,
            refreshed: LATEST,
            refreshes: AtomicU32::new(0),
            bound: true,
            unbound_as: ShieldedAccount::Binding,
            ledger: Some(Ledger {
                spendable: 2 * NOTE,
                synced_index: Some(42),
            }),
            notes: vec![note(1, NOTE), note(2, NOTE)],
            notes_now: Mutex::default(),
            activity: Vec::new(),
            locks: Vec::new(),
            own: Some(HashSet::new()),
            settled: Mutex::default(),
            settles: Mutex::default(),
            applies: Mutex::new(None),
            queued: Mutex::default(),
            applied: Mutex::default(),
            proof_hold: Mutex::default(),
            unhooked: AtomicBool::new(false),
            no_keyset: AtomicBool::new(false),
            on_sync: Mutex::default(),
            pool_notes: Mutex::new(None),
            create_hook: Mutex::new(None),
            hook_reads: Mutex::default(),
            no_keyset_spends: AtomicBool::new(false),
            held: Mutex::default(),
            holds: Mutex::default(),
            all_notes: Mutex::default(),
            notes_unreadable: AtomicBool::new(false),
            notes_reads: AtomicU32::new(0),
            after_failure: Mutex::default(),
            held_unreadable: AtomicBool::new(false),
            acks: Mutex::default(),
            stored_unreadable: AtomicBool::new(false),
            issued: AtomicU32::new(0),
            funder: Mutex::default(),
            funded: Mutex::default(),
            syncs: AtomicU32::new(0),
            spendable: Spendable {
                duffs: 10_000_000_000,
                inputs: 1,
                protected: 0,
            },
            coins: vec![10_000_000_000],
            receive: core_address(9).to_string(),
            platform: Mutex::new(Some(pool::PlatformPool {
                receive: own_platform(0),
                addresses: (0..3).map(own_platform).collect(),
            })),
            network: Network::Regtest,
            fails: Mutex::new(None),
            spent: Mutex::new(Vec::new()),
            unresolved: Mutex::default(),
            reserved: Mutex::default(),
            loses_record: AtomicBool::new(false),
            rows: Mutex::default(),
            scan_finds: Mutex::new(None),
            scan_ledger: Mutex::default(),
            sync: Mutex::new(SyncState {
                last_sync_at: Some(1_700_000_000),
                ..SyncState::default()
            }),
            unknown: Mutex::default(),
            lock_rows: Mutex::default(),
            script: Mutex::new(Some(dashcore::ScriptBuf::new())),
            sign_next: Mutex::default(),
            signed: Mutex::default(),
            funds_next: Mutex::default(),
            probe: Mutex::new(None),
            probed: Mutex::default(),
            real_rows: false,
            consumed: AtomicBool::new(false),
            lib_errors: Mutex::default(),
            row_after: Mutex::default(),
            coin_reads: AtomicU32::new(0),
            permits: AtomicU32::new(0),
            hold: Mutex::new(None),
            startup_running: AtomicBool::new(false),
            spv_stopped: AtomicBool::new(false),
            drain: Mutex::default(),
            drain_built: Mutex::default(),
            drained: Mutex::default(),
            drain_claimed: Mutex::default(),
            drain_floors: Mutex::default(),
            core: Mutex::default(),
            fund_end: Mutex::default(),
            rest: Mutex::new(Some(pool::RestClock::default())),
            drops_rest: AtomicBool::new(false),
            hooks: AtomicBool::new(false),
            drops_artifact: AtomicBool::new(false),
            spent_reads: Mutex::default(),
            partly_used: Mutex::default(),
        }
    }
}

impl FakeShielded {
    /// A funding whose lock `txid` over `inputs` was signed, then failed
    /// as `end`: recorded as its step's when the library tracked it.
    fn signed_and_failed(
        &self,
        end: FundEnd,
        txid: Txid,
        inputs: Vec<OutPoint>,
        sighashes: Vec<[u8; 32]>,
    ) -> FundError {
        if end.tracked() {
            let (session, wallet) = self.funder();
            session.leases.record_signed_locks(
                wallet,
                &[super::lease::StoredLock {
                    txid,
                    inputs,
                    sighashes: Some(sighashes),
                    built: true,
                }],
            );
        }
        end.error()
    }

    /// The session and wallet a shield's funding records its lock for.
    fn funder(&self) -> (Arc<NetworkSession>, crate::WalletId) {
        let funder = self.funder.lock().unwrap().clone();
        let (session, wallet) = funder.expect("a test that funds a shield");
        (session.upgrade().expect("the fixture's session"), wallet)
    }

    /// The real library of the funder's wallet (`real_rows`).
    async fn real(&self) -> pool::PwShielded {
        let (session, wallet) = self.funder();
        pool::PwShielded::open(&session, wallet).await.unwrap()
    }

    /// The funder's wallet's tracked row of `outpoint`.
    async fn row(&self, outpoint: OutPoint) -> Option<TrackedAssetLock> {
        let (session, wallet) = self.funder();
        let w = session.wallet(&wallet).await.unwrap();
        w.state().await.tracked_asset_locks.get(&outpoint).cloned()
    }

    /// Tracks `lock` as the library does: in the wallet's state and,
    /// through the session's store, in `wallet.sqlite`.
    async fn track(&self, lock: TrackedAssetLock, stored: bool) {
        let (session, wallet) = self.funder();
        let (outpoint, entry) = (lock.out_point, AssetLockEntry::from(&lock));
        let w = session.wallet(&wallet).await.unwrap();
        w.state_mut()
            .await
            .tracked_asset_locks
            .insert(outpoint, lock);
        if stored {
            let changes = PlatformWalletChangeSet {
                asset_locks: Some(AssetLockChangeSet {
                    asset_locks: [(outpoint, entry)].into(),
                    removed: Default::default(),
                }),
                ..Default::default()
            };
            (session.live().unwrap().store)
                .store(wallet.0, changes)
                .unwrap();
        }
    }

    /// Changes the real row of `outpoint` as `after` says.
    async fn change_row(&self, outpoint: OutPoint, after: RowAfter) {
        let Some(mut lock) = self.row(outpoint).await else {
            panic!("a tracked row of {outpoint}");
        };
        let chain = || {
            Some(AssetLockProof::Chain(ChainAssetLockProof::new(
                100,
                outpoint.into(),
            )))
        };
        (lock.status, lock.proof) = match after {
            RowAfter::Unchanged => return,
            RowAfter::Recovered => (AssetLockStatus::RecoveredFromChain, chain()),
            RowAfter::ChainLocked => (AssetLockStatus::ChainLocked, chain()),
            RowAfter::InstantSend => (
                AssetLockStatus::InstantSendLocked,
                Some(instant_proof(&lock.transaction)),
            ),
            RowAfter::Broadcast => (AssetLockStatus::Broadcast, None),
        };
        self.track(lock, true).await;
    }

    /// A library whose withdrawals pass the engine's hook steps in
    /// `session` for `wallet` ([`Self::dispatch_withdrawal`]).
    pub(crate) fn hooked_to(session: &Arc<NetworkSession>, wallet: crate::WalletId) -> Self {
        let lib = Self::default();
        *lib.funder.lock().unwrap() = Some((Arc::downgrade(session), wallet));
        lib.hooks.store(true, Ordering::SeqCst);
        lib
    }

    /// The transition the `n`th spend signs.
    pub(crate) fn st_hash(n: u8) -> [u8; 32] {
        [0xb0 + n; 32]
    }

    /// The hook's steps for the next withdrawal, as `EngineDispatch`
    /// takes them: the payout's accept, then the artifact. A refusal is
    /// the library's `ShieldedDispatchRefused`, nothing sent.
    async fn dispatch_withdrawal(&self) -> Result<(), SpendError> {
        let (session, wallet) = self.funder();
        let n = self.spent.lock().unwrap().len() as u8;
        let st_hash = Self::st_hash(n);
        let kind = platform_wallet::wallet::shielded::ShieldedDispatchKind::Withdraw;
        let refused = |reason: String| {
            SpendError::Library(Box::new(PlatformWalletError::ShieldedDispatchRefused {
                reason,
            }))
        };
        let named = (session.accept_payout_spend(wallet, &kind, st_hash).await).map_err(refused)?;
        if self.drops_artifact.load(Ordering::SeqCst) {
            return Err(refused("the artifact was not written".into()));
        }
        let artifact = NewShieldedArtifact {
            activity: [n; 32],
            origin: ArtifactOrigin::Dispatched,
            st_hash: Some(st_hash),
            kind: Some(ArtifactKind::Withdraw),
            anchor: Some([7; 32]),
            identity_id: None,
            selected: vec![[n.wrapping_add(100); 32]],
            published: vec![[n.wrapping_add(100); 32]],
        };
        match session
            .leases
            .record_shielded_artifact(wallet, artifact)
            .await
        {
            Ok(true) => (session
                .confirm_payout_dispatch(wallet, named, st_hash)
                .await)
                .map_err(refused),
            other => Err(refused(format!("the artifact was not written: {other:?}"))),
        }
    }

    /// The next withdrawal's artifact write fails after the accept.
    pub(crate) fn drop_artifacts(&self) {
        self.drops_artifact.store(true, Ordering::SeqCst);
    }

    /// A failed send's notes stay held, as the pinned library leaves a
    /// hooked spend whose broadcast failed (`Unresolved(Cancelled)`).
    pub(crate) fn hold_failed(&self) {
        *self.after_failure.lock().unwrap() = AfterFailure::Held;
    }

    /// The addresses the withdrawals paid, in order.
    pub(crate) fn withdrawn_to(&self) -> Vec<String> {
        (self.spent().into_iter())
            .filter(|s| s.operation == "withdraw")
            .map(|s| s.to)
            .collect()
    }

    /// The full viewing keys of the key sets the spends were released.
    pub(crate) fn spent_fvks(&self) -> Vec<[u8; 96]> {
        self.spent().into_iter().map(|s| s.fvk).collect()
    }

    fn at(version: u32, refreshed: u32) -> Self {
        Self {
            version,
            refreshed,
            ..Self::default()
        }
    }

    fn spent(&self) -> Vec<Spent> {
        self.spent.lock().unwrap().clone()
    }

    pub(crate) fn fail_with(&self, e: PlatformWalletError) {
        *self.fails.lock().unwrap() = Some(e);
    }

    /// A spend under `keys`, which the adapter releases before the library
    /// runs (and so before its dispatch hook).
    fn spend(
        &self,
        keys: OrchardKeySet,
        operation: &'static str,
        to: String,
        credits: u64,
        memo: &[u8],
    ) -> Result<(), SpendError> {
        if let Some(probe) = &*self.probe.lock().unwrap() {
            self.probed.lock().unwrap().push(probe());
        }
        // The 0-based index of this spend.
        let n = self.spent.lock().unwrap().len() as u8;
        self.spent.lock().unwrap().push(Spent {
            operation,
            to,
            credits,
            memo: memo.to_vec(),
            fvk: keys.viewing_keys().to_fvk_bytes(),
        });
        match self.fails.lock().unwrap().take() {
            Some(e) => {
                // The library arms its re-drive record and keeps the row
                // pending (`arm_redrive_record`).
                if matches!(e, PlatformWalletError::ShieldedSpendUnconfirmed { .. }) {
                    self.reserved.lock().unwrap().extend(reserved(n));
                    if !self.loses_record.load(Ordering::SeqCst) {
                        self.unresolved.lock().unwrap().push(unresolved(n));
                    }
                    let mut row = entry(Kind::Sent, credits, Status::Pending);
                    row.id = [n; 32];
                    self.rows.lock().unwrap().push(row);
                } else {
                    match *self.after_failure.lock().unwrap() {
                        AfterFailure::Free => {}
                        AfterFailure::Held => {
                            self.held.lock().unwrap().insert([n; 32]);
                        }
                        AfterFailure::Armed => {
                            self.reserved.lock().unwrap().extend(reserved(n));
                            self.unresolved.lock().unwrap().push(unresolved(n));
                        }
                        AfterFailure::Unreadable => {
                            self.held_unreadable.store(true, Ordering::SeqCst)
                        }
                        AfterFailure::StoredUnreadable => {
                            self.stored_unreadable.store(true, Ordering::SeqCst)
                        }
                    }
                }
                Err(SpendError::Library(Box::new(e)))
            }
            None => Ok(()),
        }
    }
}

/// The reservations of the `n`th spend, unconfirmed: two notes, its
/// anchor, its activity row.
fn reserved(n: u8) -> [StalePendingSpend; 2] {
    [
        ([n.wrapping_add(100); 32], [7; 32], Some([n; 32]), None),
        ([n.wrapping_add(200); 32], [7; 32], Some([n; 32]), None),
    ]
}

/// The library's record of the `n`th spend, unconfirmed.
fn unresolved(n: u8) -> UnresolvedSpend {
    UnresolvedSpend {
        activity_id: hex::encode([n; 32]),
        transition: hex::encode(Sha256::digest([n; 8])),
        attempts: 0,
    }
}

impl ShieldedLib for FakeShielded {
    fn network(&self) -> Network {
        self.network
    }

    fn protocol_version(&self) -> u32 {
        self.version
    }

    async fn refresh_protocol_version(&self) -> u32 {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.refreshed
    }

    async fn bind(&self) -> Result<ShieldedAccount, PlatformError> {
        Ok(if self.bound {
            ShieldedAccount::Bound
        } else {
            self.unbound_as
        })
    }

    async fn address(&self) -> Option<[u8; 43]> {
        self.bound.then(|| payee().to_raw_bytes())
    }

    async fn ledger(&self) -> Result<Option<Ledger>, PlatformError> {
        Ok(self.scan_ledger.lock().unwrap().or(self.ledger))
    }

    async fn unspent(&self) -> Result<Vec<ShieldedNote>, PlatformError> {
        let now = self.notes_now.lock().unwrap().clone();
        Ok(now.unwrap_or_else(|| self.notes.clone()))
    }

    async fn activity(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ShieldedActivityEntry>, PlatformError> {
        let rows = self.rows.lock().unwrap().clone();
        Ok((rows.iter().chain(&self.activity))
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn sync_now(&self) -> bool {
        self.syncs.fetch_add(1, Ordering::SeqCst);
        if let Some(on_sync) = self.on_sync.lock().unwrap().take() {
            on_sync();
        }
        if let Some(ledger) = self.scan_ledger.lock().unwrap().as_mut() {
            ledger.synced_index = ledger.synced_index.or(Some(1));
        }
        if let Some(found) = self.scan_finds.lock().unwrap().take() {
            self.unresolved.lock().unwrap().clear();
            for row in self.rows.lock().unwrap().iter_mut() {
                row.status = found;
            }
        }
        true
    }

    fn sync_state(&self) -> SyncState {
        *self.sync.lock().unwrap()
    }

    fn prover_ready(&self) -> bool {
        true
    }

    async fn transfer(
        &self,
        permit: &CallPermit,
        to: &OrchardAddress,
        credits: u64,
        memo: [u8; 36],
    ) -> Result<(), SpendError> {
        let to = to.to_bech32m_string(Network::Regtest);
        self.spend(permit.shielded_keys()?, "transfer", to, credits, &memo)
    }

    async fn withdraw(
        &self,
        permit: &CallPermit,
        to: &Address,
        credits: u64,
    ) -> Result<(), SpendError> {
        let keys = permit.shielded_keys()?;
        if self.hooks.load(Ordering::SeqCst) {
            self.dispatch_withdrawal().await?;
        }
        self.spend(keys, "withdraw", to.to_string(), credits, &[])
    }

    async fn receive_address(&self) -> Result<String, PlatformError> {
        self.issued.fetch_add(1, Ordering::SeqCst);
        Ok(self.receive.clone())
    }

    async fn unshield(
        &self,
        permit: &CallPermit,
        to: &PlatformAddress,
        credits: u64,
    ) -> Result<(), SpendError> {
        self.spend(
            permit.shielded_keys()?,
            "unshield",
            platform_text(*to),
            credits,
            &[],
        )
    }

    async fn identity_create(
        &self,
        permit: &CallPermit,
        create: &pool::ShieldedCreate,
    ) -> Result<(), SpendError> {
        permit.identity_signer(&[create.index])?;
        let hook = self.create_hook.lock().unwrap().clone();
        if let Some(hook) = hook
            && let Err(reason) = hook().await
        {
            return Err(SpendError::Library(Box::new(
                PlatformWalletError::ShieldedDispatchRefused { reason },
            )));
        }
        self.spend(
            permit.shielded_keys()?,
            "identity_create",
            platform_text(create.fallback),
            create.denomination,
            &[],
        )
    }

    async fn pool_notes(&self) -> Option<u64> {
        *self.pool_notes.lock().unwrap()
    }

    async fn platform_pool(&self) -> Result<pool::PlatformPool, PlatformError> {
        (self.platform.lock().unwrap().clone()).ok_or(PlatformError::Storage {
            detail: "the payment account could not be read".into(),
        })
    }

    async fn shield_locks(&self) -> Vec<LockInfo> {
        if self.real_rows {
            return self.real().await.shield_locks().await;
        }
        (self.shield_lock_rows().await.into_iter())
            .map(|l| l.info)
            .collect()
    }

    async fn shield_lock_rows(&self) -> Vec<super::unfinished_locks::KindLock> {
        if self.real_rows {
            return self.real().await.shield_lock_rows().await;
        }
        if let Some(rows) = &*self.lock_rows.lock().unwrap() {
            return rows.clone();
        }
        let funded = self.funded.lock().unwrap().clone();
        let infos: Vec<LockInfo> = self.locks.iter().cloned().chain(funded).collect();
        let outputs = |txid: Txid| infos.iter().filter(|l| l.outpoint.txid == txid).count();
        (infos.iter())
            .map(|l| super::unfinished_locks::KindLock {
                info: l.clone(),
                proof: l.status.has_proof(),
                credit_outputs: outputs(l.outpoint.txid),
            })
            .collect()
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        if self.real_rows {
            let (session, wallet) = self.funder();
            return session.leases.shield_lock_txids(wallet);
        }
        self.own.clone()
    }

    async fn unknown_locks(&self) -> HashSet<Txid> {
        if self.real_rows {
            return self.real().await.unknown_locks().await;
        }
        self.unknown.lock().unwrap().clone()
    }

    async fn mark_unknown(&self, txid: Txid) {
        if self.real_rows {
            return self.real().await.mark_unknown(txid).await;
        }
        self.unknown.lock().unwrap().insert(txid);
    }

    /// The field's, and with a funder the session's own records.
    async fn partly_used(&self) -> HashSet<OutPoint> {
        if self.real_rows {
            return self.real().await.partly_used().await;
        }
        let mut out = self.partly_used.lock().unwrap().clone();
        let funder = self.funder.lock().unwrap().clone();
        if let Some((session, wallet)) = funder.and_then(|(s, w)| Some((s.upgrade()?, w))) {
            let kind = crate::platform::unfinished_locks::LockKindId::Shielded;
            let kept = crate::platform::unfinished_locks::remaining_of(&session, wallet, kind);
            out.extend(kept.await.into_keys());
        }
        out
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        if self.real_rows {
            return self.real().await.settled_locks().await;
        }
        self.settled.lock().unwrap().clone()
    }

    async fn spent(&self, outpoint: OutPoint) -> SpentRead {
        (self.spent_reads.lock().unwrap().get(&outpoint)).map_or(SpentRead::Unread, |r| *r)
    }

    async fn settle(&self, txid: Txid) -> Result<(), PlatformError> {
        self.settles.lock().unwrap().push(txid);
        if self.real_rows {
            return self.real().await.settle(txid).await;
        }
        self.settled.lock().unwrap().insert(txid);
        Ok(())
    }

    async fn hooked(&self) -> bool {
        if let Some(answer) = self.hook_reads.lock().unwrap().pop_front() {
            return answer;
        }
        !self.unhooked.load(Ordering::SeqCst)
    }

    fn keyset_spends(&self) -> bool {
        !(self.no_keyset.load(Ordering::SeqCst) || self.no_keyset_spends.load(Ordering::SeqCst))
    }

    async fn rest_clock(&self) -> Result<pool::RestClock, PlatformError> {
        (self.rest.lock().unwrap().clone()).ok_or(PlatformError::Storage {
            detail: "the rest clock cannot be read".into(),
        })
    }

    async fn record_rest(
        &self,
        seen: Vec<[u8; 32]>,
        (at, now): (u64, u64),
        scanned: bool,
    ) -> Result<(), PlatformError> {
        let mut rest = self.rest.lock().unwrap();
        let clock = rest.as_mut().ok_or(PlatformError::Unavailable)?;
        if self.drops_rest.load(Ordering::SeqCst) {
            return Ok(());
        }
        let at = if clock.scanned { now } else { at };
        for cmx in seen {
            clock.seen.entry(cmx).or_insert(at);
        }
        clock.scanned |= scanned;
        Ok(())
    }

    async fn held_spends(&self) -> Result<HashSet<[u8; 32]>, PlatformError> {
        if self.held_unreadable.load(Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "the dispositions cannot be read".into(),
            });
        }
        Ok(self.held.lock().unwrap().clone())
    }

    async fn ack_spend(&self, st_hash: [u8; 32]) -> Result<bool, PlatformError> {
        self.acks.lock().unwrap().push(st_hash);
        Ok(self.held.lock().unwrap().remove(&st_hash))
    }

    async fn holds(&self) -> Result<Vec<pool::Hold>, PlatformError> {
        if self.held_unreadable.load(Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "the dispositions cannot be read".into(),
            });
        }
        Ok(self.holds.lock().unwrap().clone())
    }

    async fn all_notes(&self) -> Result<Vec<ShieldedNote>, PlatformError> {
        self.notes_reads.fetch_add(1, Ordering::SeqCst);
        if self.notes_unreadable.load(Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "the notes cannot be read".into(),
            });
        }
        let all = self.all_notes.lock().unwrap().clone();
        Ok(all.unwrap_or_else(|| self.notes.clone()))
    }

    async fn unresolved(&self) -> Result<Vec<UnresolvedSpend>, PlatformError> {
        if self.stored_unreadable.load(Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "the stored spends cannot be read".into(),
            });
        }
        Ok(self.unresolved.lock().unwrap().clone())
    }

    async fn reservations(&self) -> Result<Vec<StalePendingSpend>, PlatformError> {
        Ok(self.reserved.lock().unwrap().clone())
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        self.coin_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.coins.clone())
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.spendable)
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        Ok(1_000)
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        *self.hold.lock().unwrap()
    }

    fn startup_running(&self) -> bool {
        self.startup_running.load(Ordering::SeqCst)
    }

    fn spv_running(&self) -> bool {
        !self.spv_stopped.load(Ordering::SeqCst)
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        self.permits.fetch_add(1, Ordering::SeqCst);
        let (session, _) = self.funder();
        let fence =
            (session.coin_fence(lease.wallet).await).map_err(|e| FundError::Refused(e.into()))?;
        (session
            .funding_call_permit(lease, step.clone(), duffs, step, key, fence)
            .await)
            .map_err(FundError::Refused)
    }

    /// A new lock of the wallet's coins (input `0x66…:n`), signed by the
    /// permit's funding signer and recorded as the step's own, proved; with
    /// `real_rows` also the library's tracked row, in the wallet's state
    /// only while the call runs (the session's store refuses a lock its
    /// call's signatures do not cover yet).
    async fn fund(&self, duffs: u64, permit: &CallPermit) -> Result<OutPoint, FundError> {
        let n = self.funded.lock().unwrap().len() as u8 + 0x70;
        let (inputs, script) = match self.funds_next.lock().unwrap().take() {
            Some((inputs, script)) => (inputs, Some(script)),
            None => (
                vec![OutPoint::new(
                    Txid::from_byte_array([0x66; 32]),
                    u32::from(n),
                )],
                None,
            ),
        };
        let end = self.fund_end.lock().unwrap().take();
        if let Some(end) = end.filter(|e| *e != FundEnd::ProofTimeout) {
            let txid = Txid::from_byte_array([n; 32]);
            permit.note_signed(txid.to_byte_array());
            return Err(self.signed_and_failed(end, txid, inputs, vec![txid.to_byte_array()]));
        }
        let timeout = end == Some(FundEnd::ProofTimeout);
        assert!(
            !timeout || self.real_rows,
            "a proof timeout needs real rows"
        );
        let outpoint = if self.real_rows {
            use dashcore::transaction::special_transaction::TransactionPayload;
            let mut transaction = lock_transaction(n);
            transaction.input = (inputs.iter())
                .map(|&previous_output| dashcore::TxIn {
                    previous_output,
                    ..Default::default()
                })
                .collect();
            if let (Some(script), Some(TransactionPayload::AssetLockPayloadType(payload))) =
                (&script, &mut transaction.special_transaction_payload)
            {
                payload.credit_outputs[0].script_pubkey = script.clone();
            }
            let outpoint = OutPoint::new(transaction.txid(), 0);
            let (proof, status) = if timeout {
                (None, AssetLockStatus::Broadcast)
            } else {
                (
                    Some(instant_proof(&transaction)),
                    AssetLockStatus::InstantSendLocked,
                )
            };
            let row = TrackedAssetLock {
                operation_id: None,
                shielded_recipient: None,
                out_point: outpoint,
                proof,
                transaction,
                account_index: 0,
                funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
                identity_index: 0,
                amount: duffs,
                status,
            };
            self.track(row, timeout).await;
            outpoint
        } else {
            OutPoint::new(Txid::from_byte_array([n; 32]), 0)
        };
        let (session, wallet) = self.funder();
        let sighash = outpoint.txid.to_byte_array();
        permit.note_signed(sighash);
        session.leases.record_signed_locks(
            wallet,
            &[super::lease::StoredLock {
                txid: outpoint.txid,
                inputs,
                sighashes: Some(vec![sighash]),
                built: true,
            }],
        );
        self.funded.lock().unwrap().push(LockInfo {
            outpoint,
            amount: duffs,
            ..lock(n, LockStatus::InstantSendLocked)
        });
        match end {
            Some(end) => Err(end.error()),
            None => Ok(outpoint),
        }
    }

    async fn wait_proof(&self, _: OutPoint, _: Duration) -> Result<(), FundError> {
        crate::coins::pause_at(&self.proof_hold).await;
        Err(FundError::Refused(PlatformError::Unavailable))
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<dashcore::ScriptBuf> {
        if self.real_rows {
            return self.real().await.credit_script(outpoint).await;
        }
        self.script.lock().unwrap().clone()
    }

    /// Signs with `sign_next` through the shield's own `Witnessed` wrapper,
    /// as the library's transition does, then answers as scripted.
    async fn apply<S>(&self, outpoint: OutPoint, signer: &S) -> Result<Applied, PlatformError>
    where
        S: key_wallet::ExtendedPubKeySigner
            + key_wallet::Signer<Error = dw_vault::SignerError>
            + Send
            + Sync,
    {
        self.applied.lock().unwrap().push(outpoint);
        if self.real_rows {
            let after = std::mem::take(&mut *self.row_after.lock().unwrap());
            self.change_row(outpoint, after).await;
        }
        let witnessed = pool::Witnessed::new(signer);
        let paths = std::mem::take(&mut *self.sign_next.lock().unwrap());
        for path in &paths {
            let signed = key_wallet::Signer::sign_ecdsa(&witnessed, path, [7; 32]).await;
            self.signed.lock().unwrap().push(signed.is_ok());
        }
        if self.consumed.swap(false, Ordering::SeqCst) {
            // As platform-wallet answers it, mapped by the real function.
            let consumed = PlatformWalletError::AssetLockAlreadyConsumed(outpoint);
            return pool::applied_from(Err(consumed), outpoint, true);
        }
        if let Some(e) = self.lib_errors.lock().unwrap().pop_front() {
            return pool::applied_from(Err(e), outpoint, true);
        }
        let answer = (self.applies.lock().unwrap().take()).expect("a scripted apply");
        *self.applies.lock().unwrap() = self.queued.lock().unwrap().pop_front();
        answer
    }

    async fn drain_coins(
        &self,
        _: &crate::coins::CoinFence,
    ) -> Result<Vec<DrainCoin>, ShieldedError> {
        Ok(self.drain.lock().unwrap().clone())
    }

    async fn fund_drain(
        &self,
        coins: &[DrainCoin],
        total_duffs: u64,
        minimum_lock_duffs: u64,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        self.drain_lock((coins, total_duffs), minimum_lock_duffs, permit, false)
            .await
    }

    async fn core_coins(
        &self,
        _: &crate::coins::CoinFence,
    ) -> Result<super::core_max::CoreCoins, PlatformError> {
        Ok(self.core.lock().unwrap().clone())
    }

    async fn fund_core_max(
        &self,
        coins: &[DrainCoin],
        total_duffs: u64,
        minimum_lock_duffs: u64,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        self.drain_lock((coins, total_duffs), minimum_lock_duffs, permit, true)
            .await
    }
}

impl FakeShielded {
    /// A new lock of the planned coins (or `drain_built`'s), shown to the
    /// permit's drain signer as the library's builder shows it, then each
    /// input signed through it over a sighash of the whole transaction, as
    /// `SIGHASH_ALL` commits to every input and output and the payload,
    /// recorded under its own txid as the step's lock, proved. A signer's
    /// refusal fails the build, as the library's does: nothing is stored.
    pub(super) async fn drain_lock(
        &self,
        (coins, total_duffs): (&[DrainCoin], u64),
        minimum_lock_duffs: u64,
        permit: &CallPermit,
        instant_locked: bool,
    ) -> Result<OutPoint, FundError> {
        self.drain_floors.lock().unwrap().push(minimum_lock_duffs);
        let (session, wallet) = self.funder();
        self.drain_claimed.lock().unwrap().push(matches!(
            session.coinjoin.claim_sweep(wallet),
            Err(crate::coinjoin::CoinJoinFailure::MoveRunning)
        ));
        let signer = drain::drain_signer(
            permit,
            coins,
            total_duffs,
            minimum_lock_duffs,
            fake_credit_script(),
            instant_locked,
        )?;
        let built = (self.drain_built.lock().unwrap().clone()).unwrap_or_else(|| coins.to_vec());
        let (tx, sighashes) = sign_fake_drain(&signer, permit, &built).await?;
        let n = self.funded.lock().unwrap().len() as u8 + 0x90;
        let outpoint = OutPoint::new(tx.txid(), 0);
        let inputs: Vec<OutPoint> = coins.iter().map(|c| c.outpoint).collect();
        if let Some(end) = self.fund_end.lock().unwrap().take() {
            return Err(self.signed_and_failed(end, outpoint.txid, inputs, sighashes));
        }
        session.leases.record_signed_locks(
            wallet,
            &[super::lease::StoredLock {
                txid: outpoint.txid,
                inputs: inputs.clone(),
                sighashes: Some(sighashes),
                built: true,
            }],
        );
        self.drained.lock().unwrap().push(inputs);
        self.funded.lock().unwrap().push(LockInfo {
            outpoint,
            amount: total_duffs,
            ..lock(n, LockStatus::InstantSendLocked)
        });
        Ok(outpoint)
    }
}

/// The library's drain build of `built` ([`fake_drain_build`]) shown to
/// the drain `signer` as the library's builder shows it, then each input
/// signed through it over a sighash of the whole transaction, as
/// `SIGHASH_ALL` commits to every input and output and the payload. A
/// refusal fails the build, as the library's does.
pub(super) async fn sign_fake_drain(
    signer: &super::signers::CountingFundingSigner,
    permit: &CallPermit,
    built: &[DrainCoin],
) -> Result<(dashcore::Transaction, Vec<[u8; 32]>), FundError> {
    let (tx, utxos) = fake_drain_build(built);
    if key_wallet::Signer::approve_inputs(signer, &tx, &utxos)
        .await
        .is_err()
        && let Some(refusal) = signer.drain_refusal()
    {
        return Err(drain::drain_refused(permit, refusal));
    }
    let unsigned = dashcore::consensus::serialize(&tx);
    let mut sighashes = Vec::new();
    for (i, coin) in built.iter().enumerate() {
        let sighash: [u8; 32] = Sha256::new()
            .chain_update(&unsigned)
            .chain_update((i as u32).to_le_bytes())
            .finalize()
            .into();
        if let Err(e) = key_wallet::Signer::sign_ecdsa(signer, &coin.path, sighash).await {
            return Err(match signer.drain_refusal() {
                Some(refusal) => drain::drain_refused(permit, refusal),
                None if signer.capped() => FundError::TooManyInputs {
                    max: MAX_LOCK_INPUTS as u64,
                },
                None => FundError::NotSent(PlatformError::Internal {
                    detail: format!("the drain's signer refused: {e}"),
                }),
            });
        }
        sighashes.push(sighash);
    }
    Ok((tx, sighashes))
}

/// The credit script of the fake's drain locks.
pub(super) fn fake_credit_script() -> dashcore::ScriptBuf {
    dashcore::ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([0x5c; 20]))
}

/// The library's drain lock of `coins` as its builder hands it to the
/// signer: every coin in, one burn of Σ − the fee bound, one credit of it
/// to [`fake_credit_script`]; and the coins as the wallet's UTXOs.
fn fake_drain_build(coins: &[DrainCoin]) -> (dashcore::Transaction, Vec<key_wallet::Utxo>) {
    use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
    use dashcore::blockdata::transaction::special_transaction::asset_lock::AssetLockPayload;
    let utxos: Vec<key_wallet::Utxo> = (coins.iter())
        .map(|c| {
            let address = Address::p2pkh(
                &dashcore::PublicKey::from_slice(&[2; 33]).unwrap(),
                Network::Testnet,
            );
            let txout = dashcore::TxOut {
                value: c.value,
                script_pubkey: address.script_pubkey(),
            };
            let mut u = key_wallet::Utxo::new(c.outpoint, txout, address, 100, false);
            u.is_confirmed = c.confirmed;
            u.is_instantlocked = !c.confirmed;
            u
        })
        .collect();
    let total: u64 = coins.iter().map(|c| c.value).sum();
    let credit = total.saturating_sub(crate::fees::asset_lock_fee_bound(coins.len()));
    let tx = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: (coins.iter())
            .map(|c| dashcore::TxIn {
                previous_output: c.outpoint,
                ..Default::default()
            })
            .collect(),
        output: vec![dashcore::TxOut {
            value: credit,
            script_pubkey: dashcore::ScriptBuf::new_op_return(&[]),
        }],
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: vec![dashcore::TxOut {
                    value: credit,
                    script_pubkey: fake_credit_script(),
                }],
            },
        )),
    };
    (tx, utxos)
}

fn run<T: Send + 'static, Fut>(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    body: impl FnOnce(Arc<NetworkSession>, Arc<FakeShielded>) -> Fut,
) -> T
where
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let fut = body(Arc::clone(&fx.s), Arc::clone(lib));
    fx.engine.block_on(fut)
}

fn code<T: std::fmt::Debug>(r: Result<T, ShieldedError>) -> &'static str {
    r.unwrap_err().code()
}

fn version(v: u32) -> &'static PlatformVersion {
    PlatformVersion::get(v).unwrap()
}

/// The library's fee for a spend of `credits` from the default notes.
fn fee_of(credits: u64, kind: ShieldedFeeKind) -> u64 {
    let notes = FakeShielded::default().notes;
    select_notes_with_fee(&notes, credits, 2, kind, version(LATEST))
        .unwrap()
        .2
}

#[test]
fn the_activation_is_platforms_and_testnet_and_mainnet_are_past_it() {
    assert_eq!(SHIELDED_ACTIVATION_VERSION, 12);
    // The SDK's floor for both (`dash_sdk::sdk::min_protocol_version`).
    for network in [Network::Mainnet, Network::Testnet] {
        assert!(dash_sdk::sdk::min_protocol_version(network) >= SHIELDED_ACTIVATION_VERSION);
    }
}

/// Below the activation every call but the status refuses
/// `shielded.not_active` after one proved read; a proved version at or
/// above it opens them; a held version above it reads nothing.
#[test]
fn the_gate_refuses_below_the_activation_after_a_proved_read() {
    let fx = Fixture::new(0);
    let below = Arc::new(FakeShielded::at(11, 11));
    let status = run(&fx, &below, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(!status.active);
    assert_eq!(
        (status.protocol_version, status.activation_version),
        (11, 12)
    );
    assert!(!status.ready && status.balance == 0 && status.pending_shields.is_empty());
    assert_eq!(
        below.refreshes.load(Ordering::SeqCst),
        0,
        "the status reads none"
    );

    let refused = run(&fx, &below, |s, lib| async move {
        let lib = &*lib;
        vec![
            code(pool::receive_address(lib).await),
            code(pool::history(lib, 0, 10).await),
            code(pool::send_quote(lib, &payee_text(), 1).await),
            code(pool::unshield_quote(lib, 1, None).await),
            code(pool::shield_quote(&s, lib, 1).await),
            code(pool::sync(&s, lib).await),
        ]
    });
    assert!(
        refused.iter().all(|c| *c == "shielded.not_active"),
        "{refused:?}"
    );
    assert_eq!(below.refreshes.load(Ordering::SeqCst), 6);

    let proved = Arc::new(FakeShielded::at(11, 12));
    run(&fx, &proved, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(proved.refreshes.load(Ordering::SeqCst), 1);

    let held = Arc::new(FakeShielded::at(12, 0));
    run(&fx, &held, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(held.refreshes.load(Ordering::SeqCst), 0);
}

/// A shield from the Platform addresses passes the pool's gates before
/// anything else (C-routes C1-6): active after a proved read, then the
/// account's viewing keys bound; its rules are the proved version's.
#[test]
fn the_platform_shield_gate_is_active_then_bound() {
    let fx = Fixture::new(0);
    let gate = |lib: FakeShielded| {
        run(&fx, &Arc::new(lib), |_, lib| async move {
            (pool::platform_shield_gate(&*lib).await).map(|v| v.protocol_version)
        })
    };
    let below_unbound = FakeShielded {
        bound: false,
        ..FakeShielded::at(11, 11)
    };
    assert_eq!(code(gate(below_unbound)), "shielded.not_active");
    let unbound = FakeShielded {
        bound: false,
        ..FakeShielded::at(11, 12)
    };
    assert_eq!(
        gate(unbound),
        Err(ShieldedError::not_synced(ShieldedSyncWait::Account))
    );
    assert_eq!(gate(FakeShielded::at(11, 12)), Ok(12));
}

/// The status shows each held spend and the credits of the unspent notes
/// they hold, each note once; nothing held reads 0, and holds that cannot
/// be read show none. The balance stays the ledger's.
#[test]
fn the_status_shows_the_held_spends_and_their_unspent_notes() {
    let fx = Fixture::new(0);
    let status_of = |lib: FakeShielded| {
        let lib = Arc::new(lib);
        let status = run(&fx, &lib, |_, lib| async move {
            pool::status(&*lib).await.unwrap()
        });
        (status, lib.notes_reads.load(Ordering::SeqCst))
    };
    // No hold: 0, and the notes are not read.
    let (none, reads) = status_of(FakeShielded::default());
    assert!(none.held_spends.is_empty());
    assert_eq!((none.held, reads), (0, 0));

    let spent = ShieldedNote {
        is_spent: true,
        ..note(5, 17)
    };
    let all = vec![
        note(1, NOTE),
        note(2, NOTE),
        note(3, 11),
        note(4, 13),
        spent,
    ];
    let hold = |n: u8, notes: &[&ShieldedNote]| pool::Hold {
        transition: [n; 32],
        activity: [n + 1; 32],
        nullifiers: notes.iter().map(|note| note.nullifier).collect(),
    };
    // A note the wallet does not have (never scanned, or purged).
    let unknown = note(9, 7);
    let holds = vec![
        hold(0xa0, &[&all[2], &all[3]]),
        hold(0xb0, &[&all[3], &all[4]]),
        // Its one note seen spent: listed, holding nothing.
        hold(0xc0, &[&all[4]]),
        hold(0xd0, &[&unknown]),
    ];
    let held = || FakeShielded {
        holds: Mutex::new(holds.clone()),
        all_notes: Mutex::new(Some(all.clone())),
        ..FakeShielded::default()
    };
    let (status, reads) = status_of(held());
    assert_eq!(reads, 1);
    assert_eq!(status.held, 11 + 13);
    assert_eq!(status.held, pool::held_credits(&holds, &all));
    assert_eq!(status.balance, 2 * NOTE);
    let shown: Vec<_> = (status.held_spends.iter())
        .map(|h| (h.activity_id.clone(), h.transition.clone(), h.credits))
        .collect();
    let spend = |n: u8, credits| (hex::encode([n + 1; 32]), hex::encode([n; 32]), credits);
    assert_eq!(
        shown,
        [
            spend(0xa0, 11 + 13),
            spend(0xb0, 13),
            spend(0xc0, 0),
            spend(0xd0, 0)
        ]
    );

    let notes_fail = held();
    notes_fail.notes_unreadable.store(true, Ordering::SeqCst);
    let (status, _) = status_of(notes_fail);
    assert!(status.held_spends.is_empty());
    assert_eq!((status.held, status.balance), (0, 2 * NOTE));

    let unreadable = held();
    unreadable.held_unreadable.store(true, Ordering::SeqCst);
    let (status, _) = status_of(unreadable);
    assert!(status.held_spends.is_empty());
    assert_eq!((status.held, status.balance), (0, 2 * NOTE));
}

#[test]
fn the_status_reads_the_ledger_pending_rows_and_open_shields() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        activity: vec![
            entry(Kind::Sent, 100, Status::Pending),
            entry(Kind::Unshield, 50, Status::Pending),
            entry(Kind::Received, 999, Status::Confirmed),
            entry(Kind::Sent, 7, Status::Pending),
        ],
        locks: vec![
            lock(3, LockStatus::Broadcast),
            lock(1, LockStatus::InstantSendLocked),
            lock(2, LockStatus::Consumed),
            lock(4, LockStatus::RecoveredFromChain),
            // Applied in an earlier session, back from SPV's rescan.
            lock(5, LockStatus::ChainLocked),
        ],
        settled: Mutex::new(HashSet::from([lock(5, LockStatus::ChainLocked)
            .outpoint
            .txid])),
        ..FakeShielded::default()
    });
    let status = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.active && status.ready);
    assert_eq!(status.balance, 2 * NOTE);
    assert_eq!(status.synced_index, Some(42));
    // The leading pending rows: the sort puts pending first.
    assert_eq!(status.pending, 150);
    assert_eq!(status.last_sync_at, Some(1_700_000_000));
    assert!(status.prover_ready && !status.syncing);
    let shields: Vec<_> = (status.pending_shields.iter())
        .map(|p| (p.duffs, p.proved))
        .collect();
    assert_eq!(shields, [(1_000_000, true), (3_000_000, false)]);

    // Unbound, or bound with nothing restored or scanned: not a reading.
    for lib in [
        FakeShielded {
            bound: false,
            ..FakeShielded::default()
        },
        FakeShielded {
            ledger: None,
            ..FakeShielded::default()
        },
    ] {
        let lib = Arc::new(lib);
        let status = run(&fx, &lib, |_, lib| async move {
            pool::status(&*lib).await.unwrap()
        });
        assert!(status.active && !status.ready);
        assert_eq!((status.balance, status.synced_index), (0, None));
        let refused = run(&fx, &lib, |_, lib| async move {
            code(pool::send_quote(&*lib, &payee_text(), 1).await)
        });
        assert_eq!(refused, "shielded.not_synced");
    }
}

#[test]
fn the_receive_address_is_the_accounts_bech32m_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let address = run(&fx, &lib, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(address.address, payee_text());
    assert!(address.address.starts_with("tdash1z"));
    assert_eq!(address.account, 0);
}

/// What holds an unbound account is the status's `account`: a first bind
/// the vault refused says unlock, unlock fully, or watch-only; a pass that
/// held the bind back says binding. None of them gives an address, and
/// none is a reading.
#[test]
fn an_unbound_account_says_why_and_gives_no_address() {
    let fx = Fixture::new(0);
    for why in [
        ShieldedAccount::Locked,
        ShieldedAccount::MixingOnly,
        ShieldedAccount::WatchOnly,
        ShieldedAccount::Binding,
    ] {
        let lib = Arc::new(FakeShielded {
            bound: false,
            unbound_as: why,
            ..FakeShielded::default()
        });
        let (status, address) = run(&fx, &lib, |_, lib| async move {
            let status = pool::status(&*lib).await.unwrap();
            (status, code(pool::receive_address(&*lib).await))
        });
        assert_eq!(status.account, Some(why));
        assert!(status.active && !status.ready);
        assert_eq!(address, "shielded.not_synced");
    }
    let bound = Arc::new(FakeShielded::default());
    let status = run(&fx, &bound, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert_eq!(status.account, Some(ShieldedAccount::Bound));
    // Not active: no account at all.
    let inactive = Arc::new(FakeShielded {
        version: SHIELDED_ACTIVATION_VERSION - 1,
        ..FakeShielded::default()
    });
    let status = run(&fx, &inactive, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert_eq!(status.account, None);
}

/// The vault's refusals of a first bind map to what the user can do; any
/// other vault error stays the call's error.
#[test]
fn a_vault_refusal_of_the_viewing_key_maps_to_an_account_state() {
    assert_eq!(
        pool::vault_blocked(VaultError::Locked),
        Ok(ShieldedAccount::Locked)
    );
    assert_eq!(
        pool::vault_blocked(VaultError::MixingOnly),
        Ok(ShieldedAccount::MixingOnly)
    );
    assert_eq!(
        pool::vault_blocked(VaultError::NoSecret),
        Ok(ShieldedAccount::WatchOnly)
    );
    assert!(pool::vault_blocked(VaultError::NoVault).is_err());
}

/// Bound and not scanned yet, the account's address reads: a note paid to
/// it while the first pass runs is the pass's to find, and shows once the
/// pass establishes the account.
#[test]
fn the_address_reads_before_the_first_scan_and_its_note_shows_after() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        ledger: None,
        ..FakeShielded::default()
    });
    *lib.sync.lock().unwrap() = SyncState {
        syncing: true,
        last_sync_at: None,
        scan: Some(ShieldedScanProgress {
            checked: 4_000,
            total: 10_000,
        }),
        last_pass_failed: false,
    };
    let (early, before) = run(&fx, &lib, |_, lib| async move {
        let address = pool::receive_address(&*lib).await.unwrap();
        (address, pool::status(&*lib).await.unwrap())
    });
    assert_eq!(early.address, payee_text());
    assert_eq!(before.account, Some(ShieldedAccount::Bound));
    assert!(!before.ready && before.syncing);
    assert_eq!(
        before.scan,
        Some(ShieldedScanProgress {
            checked: 4_000,
            total: 10_000
        })
    );
    // Mid-pass, a payment to `early` lands in the tree; the pass finds it.
    *lib.scan_ledger.lock().unwrap() = Some(Ledger {
        spendable: NOTE,
        synced_index: None,
    });
    *lib.sync.lock().unwrap() = SyncState {
        last_sync_at: Some(1_700_000_100),
        ..SyncState::default()
    };
    let (after, address) = run(&fx, &lib, |_, lib| async move {
        lib.sync_now().await;
        let status = pool::status(&*lib).await.unwrap();
        (status, pool::receive_address(&*lib).await.unwrap())
    });
    assert!(after.ready && !after.syncing);
    assert_eq!(after.balance, NOTE);
    assert_eq!(after.scan, None);
    assert_eq!(address, early);
}

/// The status passes the hub's reading of the last pass through, and the
/// scan progress only while a pass runs.
#[test]
fn the_status_reports_a_failed_pass_and_no_progress_between_passes() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    *lib.sync.lock().unwrap() = SyncState {
        syncing: false,
        last_sync_at: Some(1_700_000_000),
        scan: Some(ShieldedScanProgress {
            checked: 1,
            total: 2,
        }),
        last_pass_failed: true,
    };
    let status = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.ready && status.last_pass_failed);
    assert_eq!(status.scan, None);
}

#[test]
fn a_send_quote_takes_the_fee_of_the_notes_it_would_select() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let quote = run(&fx, &lib, |_, lib| async move {
        pool::send_quote(&*lib, &format!(" {} ", payee_text()), NOTE).await
    })
    .unwrap();
    let fee = fee_of(NOTE, ShieldedFeeKind::Base);
    assert!(fee > 0);
    assert_eq!(
        (quote.credits, quote.fee_credits, quote.total_credits),
        (NOTE, fee, NOTE + fee)
    );
    assert_eq!(quote.expected_duffs, None);
    assert_eq!(
        (quote.grant.max_duffs, quote.grant.max_credits),
        (0, NOTE + fee)
    );

    let refused = run(&fx, &lib, |_, lib| async move {
        let lib = &*lib;
        vec![
            // Another network's, a Core address, not an address.
            pool::send_quote(lib, ABANDON_MAINNET, 1).await.unwrap_err(),
            pool::send_quote(lib, &core_address(1).to_string(), 1)
                .await
                .unwrap_err(),
            pool::send_quote(lib, "tdash1zxyz", 1).await.unwrap_err(),
            pool::send_quote(lib, &payee_text(), 0).await.unwrap_err(),
            pool::send_quote(lib, &payee_text(), 2 * NOTE)
                .await
                .unwrap_err(),
        ]
    });
    let codes: Vec<_> = refused.iter().map(ShieldedError::code).collect();
    assert_eq!(
        codes,
        [
            "shielded.invalid_address",
            "shielded.invalid_address",
            "shielded.invalid_address",
            "shielded.below_minimum",
            "shielded.insufficient_funds",
        ]
    );
    let needed = 2 * NOTE + fee_of(NOTE, ShieldedFeeKind::Base);
    assert_eq!(
        refused[4],
        ShieldedError::InsufficientFunds {
            needed,
            available: 2 * NOTE,
        }
    );
}

#[test]
fn an_unshield_quote_follows_platforms_withdrawal_rules() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let rules = Rules::at(version(LATEST)).unwrap();
    let credits = rules.min.max(NOTE / 2);
    let quote = run(&fx, &lib, move |_, lib| async move {
        pool::unshield_quote(&*lib, credits, None).await
    })
    .unwrap();
    let fee = fee_of(credits, ShieldedFeeKind::Withdrawal);
    assert_eq!(
        (quote.fee_credits, quote.total_credits),
        (fee, credits + fee)
    );
    assert_eq!(
        quote.expected_duffs,
        Some(expected_duffs(credits, version(LATEST)).unwrap())
    );
    assert!(fee > fee_of(credits, ShieldedFeeKind::Base));

    let refused = run(&fx, &lib, move |_, lib| async move {
        let lib = &*lib;
        let mainnet = Address::new(
            Network::Mainnet,
            Payload::PubkeyHash(PubkeyHash::from_byte_array([1; 20])),
        );
        vec![
            code(pool::unshield_quote(lib, rules.min - 1, None).await),
            code(pool::unshield_quote(lib, rules.max + 1, None).await),
            code(pool::unshield_quote(lib, credits, Some(&mainnet.to_string())).await),
            code(pool::unshield_quote(lib, credits, Some(&payee_text())).await),
        ]
    });
    assert_eq!(
        refused,
        [
            "shielded.below_minimum",
            "invalid_argument",
            "shielded.invalid_address",
            "shielded.invalid_address",
        ]
    );
}

/// A send charges its quote to the lease and hands the library the key set
/// of the wallet's account 0; every refusal before the keys keeps the
/// grant and reaches no library spend.
#[test]
fn a_send_charges_its_quote_and_a_refusal_keeps_the_grant() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let grant = fx.grant(10 * NOTE);
    for (to, credits, memo) in [
        (payee_text(), NOTE, Some("x".repeat(37))),
        (ABANDON_MAINNET.to_owned(), NOTE, None),
        (payee_text(), 0, None),
        (payee_text(), 2 * NOTE, None),
    ] {
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            pool::send(
                &s,
                wallet,
                &*lib,
                (&to, credits),
                memo.as_deref(),
                &g,
                false,
            )
            .await
        });
        assert!(refused.is_err());
        assert!(fx.unused(&grant));
    }
    assert!(lib.spent().is_empty());

    let lease = fx.flow(FlowKind::ShieldedSpend, vec![grant]);
    let l = lease.clone();
    let sent = run(&fx, &lib, move |s, lib| async move {
        let to = format!("{} ", payee_text());
        pool::send(&s, wallet, &*lib, (&to, NOTE), Some("thanks"), &l, false).await
    })
    .unwrap();
    assert_eq!((sent.credits, sent.balance), (NOTE, 2 * NOTE));
    assert_eq!(
        (sent.state, sent.unresolved),
        (ShieldedSpendState::Done, None)
    );
    // The balance after is read after a pass finds the change.
    assert_eq!(lib.syncs.load(Ordering::SeqCst), 1);
    assert_eq!(sent.to, payee_text());
    let fee = fee_of(NOTE, ShieldedFeeKind::Base);
    assert_eq!(fx.credits_spent(&lease), NOTE + fee);
    let fvk = fx.s.vault.shielded_viewing_key(&wallet.0).unwrap();
    let mut memo = b"thanks".to_vec();
    memo.resize(36, 0);
    assert_eq!(
        lib.spent(),
        [Spent {
            operation: "transfer",
            to: payee_text(),
            credits: NOTE,
            memo,
            fvk,
        }]
    );

    // Another flow's lease is not a shielded spend's.
    let other = fx.flow(FlowKind::Withdraw, vec![fx.grant(NOTE)]);
    let refused = run(&fx, &lib, move |s, lib| async move {
        code(pool::send(&s, wallet, &*lib, (&payee_text(), 1), None, &other, false).await)
    });
    assert_eq!(refused, "platform.grant_invalid");
}

/// Opus r1 m1: a pass that moves the notes between a send's first quote
/// and its library call cannot raise the fee the library builds above what
/// the grant confirmed. The send quotes again under the pass hold, where
/// nothing moves them, and charges that: a total above the grant is
/// `platform.grant_exceeded` before the grant is redeemed or any key
/// leaves the vault.
#[test]
fn a_spend_quotes_again_under_the_hold_and_refuses_a_fee_above_its_grant() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    // The user confirmed the quote of the notes as they are.
    let quoted = NOTE + fee_of(NOTE, ShieldedFeeKind::Base);
    let grant = fx.grant(quoted);
    // A pass is in flight; it moves the notes before it ends: the send
    // now needs three smaller ones, so a higher fee.
    let smaller: Vec<_> = (3..7).map(|n| note(n, NOTE / 2)).collect();
    let higher = select_notes_with_fee(&smaller, NOTE, 2, ShieldedFeeKind::Base, version(LATEST))
        .unwrap()
        .2;
    assert!(NOTE + higher > quoted);
    let (s, l, lib2) = (Arc::clone(&fx.s), grant.clone(), Arc::clone(&lib));
    let sent = fx.engine.block_on(async move {
        let pass = s.platform.shielded_spends.read().await;
        let send = {
            let (s, lib) = (Arc::clone(&s), Arc::clone(&lib2));
            tokio::spawn(async move {
                pool::send(&s, wallet, &*lib, (&payee_text(), NOTE), None, &l, false).await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        *lib2.notes_now.lock().unwrap() = Some(smaller);
        drop(pass);
        send.await.unwrap()
    });
    assert_eq!(code(sent), "platform.grant_exceeded");
    assert!(lib.spent().is_empty(), "no key left the vault");
    assert!(fx.unused(&grant), "the refusal keeps the grant");
}

/// Once the key set is out the quote stays charged whatever the library
/// returns; its failures map to the facade's kinds.
#[test]
fn a_failed_spend_keeps_its_quote_and_maps_the_librarys_error() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let rules = Rules::at(version(LATEST)).unwrap();
    let credits = rules.min.max(NOTE / 2);
    let pool_text = "state transition broadcast failed: Shielded pool has insufficient notes \
                     for outgoing transition: pool has 3 notes but minimum 250 required";
    let cases: [(PlatformWalletError, bool, ShieldedError); 3] = [
        (
            PlatformWalletError::ShieldedBroadcastFailed(pool_text.into()),
            true,
            ShieldedError::PoolTooSmall {
                notes: 3,
                required: 250,
            },
        ),
        (
            PlatformWalletError::ShieldedNoRecordedAnchor("tree moved".into()),
            false,
            ShieldedError::NotSynced {
                reason: ShieldedSyncWait::Tree,
            },
        ),
        (
            PlatformWalletError::ShieldedInsufficientBalance {
                available: 5,
                required: 9,
            },
            true,
            ShieldedError::InsufficientFunds {
                needed: 9,
                available: 5,
            },
        ),
    ];
    for (failure, unshield, want) in cases {
        lib.fail_with(failure);
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let l = lease.clone();
        let got = run(&fx, &lib, move |s, lib| async move {
            match unshield {
                true => pool::unshield(&s, wallet, &*lib, credits, None, &l, false)
                    .await
                    .map(drop),
                false => pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l, false)
                    .await
                    .map(drop),
            }
        });
        assert_eq!(got.unwrap_err(), want);
        let kind = match unshield {
            true => ShieldedFeeKind::Withdrawal,
            false => ShieldedFeeKind::Base,
        };
        assert_eq!(fx.credits_spent(&lease), credits + fee_of(credits, kind));
    }
    // Unconfirmed text is not the pool's floor.
    assert_eq!(
        pool::spend_error(PlatformWalletError::ShieldedBroadcastFailed(
            "rejected".into()
        ))
        .code(),
        PlatformError::from(PlatformWalletError::ShieldedBroadcastFailed(
            "rejected".into()
        ))
        .code()
    );
}

#[test]
fn an_unshield_pays_the_address_given_or_a_fresh_receive_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 2);
    for to in [None, Some(core_address(3).to_string())] {
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let t = to.clone();
        let out = run(&fx, &lib, move |s, lib| async move {
            pool::unshield(&s, wallet, &*lib, credits, t.as_deref(), &lease, false).await
        })
        .unwrap();
        let want = to.unwrap_or_else(|| lib.receive.clone());
        assert_eq!(out.to, want);
        let last = lib.spent().pop().unwrap();
        assert_eq!(
            (last.operation, last.to, last.credits),
            ("withdraw", want, credits)
        );
    }
}

#[test]
fn history_shows_the_librarys_rows() {
    let fx = Fixture::new(0);
    let mut memo = b"lunch".to_vec();
    memo.resize(36, 0);
    let platform = PlatformAddress::P2pkh([4; 20]);
    let rows = vec![
        ShieldedActivityEntry {
            counterparty: Some(payee().to_raw_bytes().to_vec()),
            memo: Some(memo),
            fee: Some(20),
            created_at_ms: 1_700_000_000_999,
            ..entry(Kind::Sent, 100, Status::Pending)
        },
        ShieldedActivityEntry {
            counterparty: Some(core_address(5).script_pubkey().to_bytes()),
            block_height: Some(77),
            ..entry(Kind::Withdrawal, 200, Status::Confirmed)
        },
        ShieldedActivityEntry {
            counterparty: Some(platform.to_bytes()),
            ..entry(Kind::Unshield, 30, Status::Failed)
        },
        ShieldedActivityEntry {
            direction: Direction::In,
            ..entry(
                Kind::ShieldFromIdentity {
                    identity_id: [1; 32],
                },
                40,
                Status::Confirmed,
            )
        },
        entry(Kind::ShieldFromAssetLock, 50, Status::Unknown),
    ];
    let lib = Arc::new(FakeShielded {
        activity: rows,
        ..FakeShielded::default()
    });
    let shown = run(&fx, &lib, |_, lib| async move {
        pool::history(&*lib, 0, 500).await.unwrap()
    });
    let kinds: Vec<_> = shown.iter().map(|a| (a.kind, a.status)).collect();
    assert_eq!(
        kinds,
        [
            (ShieldedActivityKind::Sent, ShieldedActivityStatus::Pending),
            (
                ShieldedActivityKind::Withdrawal,
                ShieldedActivityStatus::Confirmed
            ),
            (
                ShieldedActivityKind::Unshield,
                ShieldedActivityStatus::Failed
            ),
            (
                ShieldedActivityKind::Shield,
                ShieldedActivityStatus::Confirmed
            ),
            (
                ShieldedActivityKind::ShieldFromCore,
                ShieldedActivityStatus::Unknown
            ),
        ]
    );
    let sent = &shown[0];
    assert_eq!(sent.id, hex::encode([100u8; 32]));
    assert_eq!(sent.counterparty.as_deref(), Some(payee_text().as_str()));
    assert_eq!(sent.memo.as_deref(), Some("lunch"));
    assert_eq!(
        (sent.fee_credits, sent.created_at),
        (Some(20), Some(1_700_000_000))
    );
    assert_eq!(sent.direction, ShieldedDirection::Out);
    assert_eq!(shown[1].counterparty, Some(core_address(5).to_string()));
    assert_eq!(shown[1].block_height, Some(77));
    assert_eq!(
        shown[2].counterparty,
        Some(platform.to_bech32m_string(Network::Regtest))
    );
    assert_eq!(shown[3].direction, ShieldedDirection::In);
    assert!(shown[4].counterparty.is_none() && shown[4].created_at.is_none());

    let paged = run(&fx, &lib, |_, lib| async move {
        let page = pool::history(&*lib, 1, 2).await.unwrap();
        (page.len(), code(pool::history(&*lib, 0, 501).await))
    });
    assert_eq!(paged, (2, "invalid_argument"));
}

#[test]
fn a_shield_quote_refuses_while_a_lock_pends_or_core_falls_short() {
    let fx = Fixture::new(0);
    let quote = |lib: FakeShielded, duffs: u64| {
        run(&fx, &Arc::new(lib), move |s, lib| async move {
            pool::shield_quote(&s, &*lib, duffs).await
        })
    };
    let pool_fee = pool::shield_pool_fee(version(LATEST)).unwrap();
    let min = pool_fee / 1_000 + 1;
    let q = quote(FakeShielded::default(), 1_000_000).unwrap();
    assert_eq!((q.fee_duffs, q.total_duffs), (1_000, 1_001_000));
    assert_eq!(q.credits, 1_000_000_000 - pool_fee);
    assert_eq!((q.grant.max_duffs, q.grant.max_credits), (1_001_000, 0));

    assert_eq!(
        quote(FakeShielded::default(), min - 1).unwrap_err(),
        ShieldedError::BelowMinimum { min, max: None }
    );
    let pending = FakeShielded {
        locks: vec![
            lock(2, LockStatus::Consumed),
            lock(1, LockStatus::Broadcast),
        ],
        ..FakeShielded::default()
    };
    assert_eq!(code(quote(pending, 1_000_000)), "shielded.shield_pending");
    let short = FakeShielded {
        spendable: Spendable {
            duffs: 1_000_500,
            inputs: 1,
            protected: 0,
        },
        ..FakeShielded::default()
    };
    assert_eq!(
        quote(short, 1_000_000).unwrap_err(),
        ShieldedError::FundingInsufficient {
            needed: 1_001_000,
            available: 1_000_500,
        }
    );
    let locked = FakeShielded {
        spendable: Spendable {
            protected: 1,
            ..FakeShielded::default().spendable
        },
        ..FakeShielded::default()
    };
    assert_eq!(code(quote(locked, 1_000_000)), "shielded.coins_locked");
}

#[test]
fn resume_refuses_an_unknown_lock_and_waits_for_a_proof() {
    let fx = Fixture::new(0);
    let wallet = fx.wallet;
    let grant = fx.grant(0);
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::Broadcast)],
        ..FakeShielded::default()
    });
    let txid = lock(1, LockStatus::Broadcast).outpoint.txid.to_string();
    let g = grant.clone();
    let (unknown, waiting) = run(&fx, &lib, move |s, lib| async move {
        let unknown = code(pool::resume(&s, wallet, &*lib, &"ab".repeat(32), &g).await);
        let waiting = pool::resume(&s, wallet, &*lib, &txid, &g).await.unwrap();
        (unknown, waiting)
    });
    assert_eq!(unknown, "invalid_argument");
    assert_eq!(waiting.state, super::shielded::ShieldState::ProofWaiting);
    assert!(fx.unused(&grant));
}

/// #879: a close that starts while "Check again" waits for a shield lock's
/// proof does not wait for it: the press is "waiting for proof", with
/// nothing applied.
#[test]
fn a_close_ends_a_shield_resumes_proof_wait_as_waiting_for_proof() {
    let fx = Fixture::new(0);
    let wallet = fx.wallet;
    let grant = shield_grant(&fx, 1);
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::Broadcast)],
        ..FakeShielded::default()
    });
    let hold = Arc::new(crate::coins::TestPause::default());
    *lib.proof_hold.lock().unwrap() = Some(Arc::clone(&hold));
    let txid = lock(1, LockStatus::Broadcast).outpoint.txid.to_string();
    let (s, run) = (Arc::clone(&fx.s), Arc::clone(&lib));
    let press = fx.s.rt.spawn(async move {
        let _op = s.enter().await.map_err(PlatformError::from)?;
        pool::resume(&s, wallet, &*run, &txid, &grant).await
    });
    assert!(hold.wait_reached(Duration::from_secs(30)), "the proof wait");
    let s = Arc::clone(&fx.s);
    let closing = fx.s.rt.spawn(async move { s.close().await });
    let resumed = fx
        .engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), press).await });
    hold.release.notify_one();
    let resumed = resumed.expect("the close ended the proof wait").unwrap();
    fx.engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(60), closing).await })
        .expect("the close ends")
        .unwrap();
    assert_eq!(resumed.unwrap().state, ShieldState::ProofWaiting);
    assert!(lib.applied.lock().unwrap().is_empty(), "no apply");
}

/// E0-04 Q15, ruling (c): a spend whose broadcast the library could not
/// confirm is `pending`, never an error, with the transition the library
/// keeps to re-send (its hash) in the outcome and the status. A pass that
/// sees it executed resolves it to done (the row confirms), one where the
/// prune backstop released the notes to not executed (the row fails), and
/// no second transition is built either way: the facade has no resend.
#[test]
fn an_unconfirmed_spend_stays_pending_until_a_scan_resolves_it() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 2);
    let unconfirmed = || PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "transfer",
        reason: "proof fetch failed".into(),
    };
    let row = |n: u8| {
        let id = hex::encode([n; 32]);
        let lib = Arc::clone(&lib);
        let history = run(&fx, &lib, |_, lib| async move {
            pool::history(&*lib, 0, 10).await.unwrap()
        });
        (history.into_iter().find(|r| r.id == id)).unwrap().status
    };
    let status = || {
        run(&fx, &lib, |_, lib| async move {
            pool::status(&*lib).await.unwrap()
        })
    };

    // No pass can be admitted while a spend's library call runs (one could
    // re-send what the library stores on an unknown result unchecked).
    let s = Arc::clone(&fx.s);
    *lib.probe.lock().unwrap() = Some(Box::new(move || {
        s.platform.shielded_spends.try_read().is_err()
    }));

    for (n, unshield, found, want) in [
        (
            0u8,
            false,
            Status::Confirmed,
            ShieldedActivityStatus::Confirmed,
        ),
        (1, true, Status::Failed, ShieldedActivityStatus::Failed),
    ] {
        lib.fail_with(unconfirmed());
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let l = lease.clone();
        let syncs = lib.syncs.load(Ordering::SeqCst);
        let out = run(&fx, &lib, move |s, lib| async move {
            if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &l, false).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l, false).await
            }
        })
        .unwrap();
        assert_eq!(out.state, ShieldedSpendState::Pending);
        assert_eq!(out.unresolved, Some(unresolved(n)));
        let kind = if unshield {
            ShieldedFeeKind::Withdrawal
        } else {
            ShieldedFeeKind::Base
        };
        // The balance it leaves, not the notes less those it holds.
        assert_eq!(out.balance, 2 * NOTE - credits - fee_of(credits, kind));
        assert_eq!(
            lib.probed.lock().unwrap().last(),
            Some(&true),
            "passes held"
        );
        assert!(
            fx.s.platform.shielded_spends.try_read().is_ok(),
            "and let go"
        );
        // The quote stays charged; no pass ran for it.
        assert_eq!(fx.credits_spent(&lease), credits + fee_of(credits, kind));
        assert_eq!(lib.syncs.load(Ordering::SeqCst), syncs);
        assert_eq!(status().unresolved_spends, [unresolved(n)]);
        assert_eq!(row(n), ShieldedActivityStatus::Pending);

        // The session's next pass resolves it.
        *lib.scan_finds.lock().unwrap() = Some(found);
        run(
            &fx,
            &lib,
            |_, lib| async move { assert!(lib.sync_now().await) },
        );
        assert_eq!(row(n), want);
        assert!(status().unresolved_spends.is_empty());
        assert_eq!(
            lib.spent().len(),
            usize::from(n) + 1,
            "no second transition"
        );
    }
}

/// A refusal as the host reads it: `not_synced:<reason>`, or the code.
fn wait_of<T: std::fmt::Debug>(r: Result<T, ShieldedError>) -> String {
    match r {
        Err(ShieldedError::NotSynced { reason }) => format!("not_synced:{reason}"),
        Ok(_) => "ok".to_owned(),
        Err(e) => e.code().to_owned(),
    }
}

/// DEC-1058 #4, at the pool layer with the fake library (not the host's
/// session admission): the refusals of a shield, a quote, a send, an
/// unshield, a resume and an address read that wait for something still
/// syncing are `shielded.not_synced` with what it waits for, never a
/// Platform refusal ("Dash Platform did not answer"); what does not clear on
/// its own keeps its code.
/// - `account`: the account is not bound, or bound and not scanned yet
///   (the seven calls that bind; `shield_quote` reads no shielded account).
///   For now this also covers a vault that cannot bind (locked,
///   mixing-only, watch-only); 0.1.x gives that its own refusal.
/// - `core` / `startup`: a shield's funding waits for Core (SPV catching
///   up, Platform writes waiting for masternode sync) or for a running
///   DashPay bring-up. A stopped SPV is still `platform.context_unavailable`,
///   a bring-up that ended without Platform's answers `platform.unavailable`,
///   a locked vault `platform.signer_unavailable`.
/// - `journal`: the dispatch journal is not read (the session reads it
///   before it opens, so only a closing one, as here); one that cannot be
///   (unopenable, a newer schema) is still `platform.unavailable`.
/// - `tree`: platform-wallet's `ShieldedNoRecordedAnchor` (the spend-error
///   table in `a_failed_spend_keeps_its_quote_and_maps_the_librarys_error`).
#[test]
fn a_refusal_while_shielded_syncs_is_not_synced_with_what_it_waits_for() {
    let fx = Fixture::new(0);
    let wallet = fx.wallet;
    let credits = NOTE / 2;
    let txid = "11".repeat(32);
    // The calls named, as the host makes them, each with its refusal.
    let all = |fx: &Fixture, lib: &Arc<FakeShielded>, names: &[&str]| -> Vec<(String, String)> {
        let grant = fx.grant(10 * NOTE);
        let txid = txid.clone();
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        run(fx, lib, move |s, lib| async move {
            let lib = &*lib;
            let mut out = Vec::new();
            for name in names {
                let wait = match name.as_str() {
                    "send_quote" => wait_of(pool::send_quote(lib, &payee_text(), credits).await),
                    "unshield_quote" => wait_of(pool::unshield_quote(lib, credits, None).await),
                    "shield_quote" => wait_of(pool::shield_quote(&s, lib, 100_000).await),
                    "shield" => wait_of(
                        pool::shield(
                            &s,
                            wallet,
                            lib,
                            FundAmount::Duffs { duffs: 100_000 },
                            &grant,
                        )
                        .await,
                    ),
                    "resume" => wait_of(pool::resume(&s, wallet, lib, &txid, &grant).await),
                    "drain_quote" => wait_of(drain::quote(&s, wallet, lib).await),
                    "drain" => wait_of(drain::drain(&s, wallet, lib, &grant).await),
                    "send" => {
                        let to = (payee_text(), credits);
                        wait_of(
                            pool::send(&s, wallet, lib, (&to.0, to.1), None, &grant, false).await,
                        )
                    }
                    "unshield" => {
                        wait_of(pool::unshield(&s, wallet, lib, credits, None, &grant, false).await)
                    }
                    "address" => wait_of(pool::receive_address(lib).await),
                    other => unreachable!("{other}"),
                };
                out.push((name, wait));
            }
            out
        })
    };
    let expect = |names: &[&str], wait: &str| -> Vec<(String, String)> {
        (names.iter())
            .map(|n| ((*n).to_owned(), wait.to_owned()))
            .collect()
    };

    // The account: not bound (a first bind needs the vault unlocked).
    let unbound = Arc::new(FakeShielded {
        bound: false,
        ..FakeShielded::default()
    });
    let needs_account = [
        "send_quote",
        "unshield_quote",
        "shield",
        "resume",
        "drain",
        "send",
        "unshield",
        "address",
    ];
    assert_eq!(
        all(&fx, &unbound, &needs_account),
        expect(&needs_account, "not_synced:account")
    );
    // Bound, not scanned yet: what reads the notes waits.
    let unscanned = Arc::new(FakeShielded {
        ledger: None,
        ..FakeShielded::default()
    });
    let reads_notes = ["send_quote", "unshield_quote", "send", "unshield"];
    assert_eq!(
        all(&fx, &unscanned, &reads_notes),
        expect(&reads_notes, "not_synced:account")
    );

    // A shield's funding hold.
    let lib = Arc::new(FakeShielded::default());
    // (hold, bring-up running, SPV stopped, refusal)
    for (hold, running, stopped, want) in [
        (RegistrationWait::Sync, false, false, "not_synced:core"),
        (
            RegistrationWait::Sync,
            false,
            true,
            "platform.context_unavailable",
        ),
        (RegistrationWait::Startup, true, false, "not_synced:startup"),
        (
            RegistrationWait::Startup,
            false,
            false,
            "platform.unavailable",
        ),
        (
            RegistrationWait::Network,
            false,
            false,
            "platform.unavailable",
        ),
        (
            RegistrationWait::Unlock,
            false,
            false,
            "platform.signer_unavailable",
        ),
    ] {
        *lib.hold.lock().unwrap() = Some(hold);
        lib.startup_running.store(running, Ordering::SeqCst);
        lib.spv_stopped.store(stopped, Ordering::SeqCst);
        assert_eq!(
            all(&fx, &lib, &["shield", "drain"]),
            expect(&["shield", "drain"], want),
            "{hold:?} running {running} stopped {stopped}"
        );
    }
    *lib.hold.lock().unwrap() = None;
    lib.startup_running.store(false, Ordering::SeqCst);
    lib.spv_stopped.store(false, Ordering::SeqCst);

    // The dispatch journal: not read (closed, as a closing session's is),
    // then found unopenable.
    let journaled = [
        "shield_quote",
        "shield",
        "resume",
        "drain_quote",
        "drain",
        "send",
        "unshield",
    ];
    fx.engine.block_on(fx.s.leases.close_journal());
    assert!(fx.s.leases.journal_loading());
    assert_eq!(
        all(&fx, &lib, &journaled),
        expect(&journaled, "not_synced:journal")
    );
    fx.s.leases.load_journal(None, Vec::new(), Vec::new());
    assert!(!fx.s.leases.journal_loading() && !fx.s.leases.journal_ready());
    assert_eq!(
        all(&fx, &lib, &journaled),
        expect(&journaled, "platform.unavailable")
    );
    // These refusals stop before dispatch: no spend, and no funding permit
    // of the shield. (A `tree` refusal comes later, after the spend's permit,
    // with the quote kept charged: the spend-error table's test.)
    assert!(lib.spent().is_empty() && unbound.spent().is_empty() && unscanned.spent().is_empty());
    assert_eq!(lib.permits.load(Ordering::SeqCst), 0);
}

/// DEC-1057: a spend of unknown result whose record the library could not
/// write (`arm_redrive_record` failed: no `unresolved`) is marked by the
/// engine before the send returns, from the reservations the library made
/// for it (its activity, anchor and nullifiers), in the dispatch journal.
/// The next send and unshield of the wallet are refused with
/// `shielded.unrecorded_spend` until confirmed, before any key leaves the
/// vault, then run. A marker that cannot be written (here: no reservation
/// of a new activity to mark) makes the spend `platform.broadcast_unknown`,
/// never a quiet pending.
#[test]
fn a_spend_the_library_kept_no_record_of_is_marked_and_gates_the_next_spends() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 4);
    let unconfirmed = || PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "transfer",
        reason: "proof fetch failed".into(),
    };
    let lease = |fx: &Fixture| fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
    lib.loses_record.store(true, Ordering::SeqCst);
    assert!(fx.s.leases.journal_ready());

    lib.fail_with(unconfirmed());
    let l = lease(&fx);
    let out = run(&fx, &lib, move |s, lib| async move {
        pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l, false).await
    })
    .unwrap();
    assert_eq!(
        (out.state, out.unresolved),
        (ShieldedSpendState::Pending, None)
    );
    let fee = fee_of(credits, ShieldedFeeKind::Base);
    assert_eq!(out.balance, 2 * NOTE - credits - fee);
    let markers = fx
        .engine
        .block_on(fx.s.leases.unrecorded_spends(wallet))
        .unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!((markers[0].activity, markers[0].anchor), ([0; 32], [7; 32]));
    let mut want: Vec<_> = reserved(0).iter().map(|r| (r.0, false)).collect();
    want.sort();
    assert_eq!(markers[0].nullifiers, want);
    assert_eq!(fx.s.leases.journal_synchronous(), Some(2), "written FULL");

    // Gated: refused before any key leaves the vault, the grant unused.
    let spent = lib.spent().len();
    for unshield in [false, true] {
        let grant = fx.grant(10 * NOTE);
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            code(if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &g, false).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &g, false).await
            })
        });
        assert_eq!(refused, "shielded.unrecorded_spend");
        assert!(fx.unused(&grant));
    }
    assert_eq!(lib.spent().len(), spent);
    // Confirmed, both run.
    for unshield in [false, true] {
        let l = lease(&fx);
        let out = run(&fx, &lib, move |s, lib| async move {
            if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &l, true).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l, true).await
            }
        })
        .unwrap();
        assert_eq!(out.state, ShieldedSpendState::Done);
    }
    assert_eq!(lib.spent().len(), spent + 2);

    // The fourth spend (index 3) finds its reservations there already:
    // nothing new to mark.
    lib.reserved.lock().unwrap().extend(reserved(3));
    lib.fail_with(unconfirmed());
    let l = lease(&fx);
    let failed = run(&fx, &lib, move |s, lib| async move {
        code(pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l, true).await)
    });
    assert_eq!(failed, "platform.broadcast_unknown");
    let markers = fx
        .engine
        .block_on(fx.s.leases.unrecorded_spends(wallet))
        .unwrap();
    assert_eq!(markers.len(), 1, "only the first spend's");
}

/// Q15's lease confinement for the facade's own passes: a shielded call
/// runs in `on_pool`'s task under the session's operation guard, so the
/// close waits for it and never drops it partway (a shield between its
/// lock and the apply, a spend after its keys are out).
#[test]
fn a_close_waits_for_a_shielded_call_and_never_drops_it() {
    use std::sync::atomic::AtomicBool;
    let fx = Fixture::new(0);
    let (wallet, session) = (fx.wallet, Arc::clone(&fx.s));
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let finished = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&finished);
    std::thread::scope(|scope| {
        let call = scope.spawn(|| {
            fx.engine
                .block_on(on_pool(&session, wallet, move |_, _| async move {
                    entered_tx.send(()).unwrap();
                    released.await.ok();
                    f.store(true, Ordering::SeqCst);
                    Ok(())
                }))
        });
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        let network = fx.s.network.clone();
        let closing = scope.spawn(|| fx.engine.block_on(fx.engine.close_network(network)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!closing.is_finished(), "the close waits for the call");
        release.send(()).unwrap();
        assert!(closing.join().unwrap().unwrap());
        assert!(finished.load(Ordering::SeqCst), "the call ran to its end");
        call.join().unwrap().unwrap();
    });
}

/// A `PlatformOp` grant of the fixture's wallet for `max_duffs`.
fn shield_grant(fx: &Fixture, max_duffs: u64) -> String {
    let purpose = dw_vault::GrantPurpose::PlatformOp {
        max_duffs,
        max_credits: 0,
    };
    let granted =
        fx.s.vault
            .authorize(purpose, Some(&fx.wallet.0), dw_vault::Credential::None);
    granted.unwrap().id
}

/// Resumes the pending shield `n` with `answer` as the library's.
fn resume_with(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    n: u8,
    answer: Result<Applied, PlatformError>,
) -> ShieldState {
    *lib.applies.lock().unwrap() = Some(answer);
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    let txid = lock(n, LockStatus::Broadcast).outpoint.txid.to_string();
    run(fx, lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid, &grant).await
    })
    .unwrap()
    .state
}

fn pending_of(fx: &Fixture, lib: &Arc<FakeShielded>) -> Vec<(String, bool, bool)> {
    let pending = run(fx, lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap().pending_shields
    });
    (pending.into_iter())
        .map(|p| (p.txid, p.proved, p.consumption_unknown))
        .collect()
}

/// DEC-1045 (c): Platform's "already consumed" is no settle evidence. A
/// lock it answers so pends "Checking" however often a resume hears it,
/// until a proved apply settles it, and each resume applies the same
/// outpoint (b). A lock of ours a restore read back from the chain pends
/// the same; one no shield step signed is history. Neither blocks a new
/// shield (it may never settle); any other pending lock does. Nothing is
/// told apart before the journal is read.
#[test]
fn a_shield_lock_of_unknown_consumption_pends_until_a_proved_apply() {
    let fx = Fixture::new(0);
    let txid = |n: u8| lock(n, LockStatus::Broadcast).outpoint.txid;
    let quote = |lib: &Arc<FakeShielded>| {
        run(&fx, lib, |s, lib| async move {
            pool::shield_quote(&s, &*lib, 1_000_000).await.map(drop)
        })
    };
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::InstantSendLocked)],
        own: Some(HashSet::from([txid(1)])),
        ..FakeShielded::default()
    });
    // Not applied yet: it blocks a new shield.
    assert_eq!(code(quote(&lib)), "shielded.shield_pending");
    for _ in 0..2 {
        let state = resume_with(&fx, &lib, 1, Ok(Applied::Taken));
        assert_eq!(state, ShieldState::ConsumptionUnknown);
        assert!(lib.settles.lock().unwrap().is_empty(), "nothing settled");
        assert_eq!(pending_of(&fx, &lib), [(txid(1).to_string(), true, true)]);
        quote(&lib).unwrap();
    }
    let state = resume_with(&fx, &lib, 1, Ok(Applied::Done));
    assert!(matches!(state, ShieldState::Applied { .. }), "{state:?}");
    assert_eq!(*lib.settles.lock().unwrap(), [txid(1)]);
    assert!(pending_of(&fx, &lib).is_empty());
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [lock(1, LockStatus::Broadcast).outpoint; 3]
    );

    // The library holds ours `RecoveredFromChain`, as a restore reads it
    // back; lock 6 is another install's; lock 2 is ours, not applied.
    let lib = Arc::new(FakeShielded {
        locks: vec![
            lock(1, LockStatus::RecoveredFromChain),
            lock(6, LockStatus::RecoveredFromChain),
        ],
        own: Some(HashSet::from([txid(1), txid(2)])),
        ..FakeShielded::default()
    });
    assert_eq!(pending_of(&fx, &lib), [(txid(1).to_string(), true, true)]);
    quote(&lib).unwrap();
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let history = run(&fx, &lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid(6).to_string(), &grant).await
    });
    assert_eq!(code(history), "invalid_argument");
    let lib = Arc::new(FakeShielded {
        locks: vec![
            lock(1, LockStatus::RecoveredFromChain),
            lock(2, LockStatus::InstantSendLocked),
        ],
        own: Some(HashSet::from([txid(1), txid(2)])),
        ..FakeShielded::default()
    });
    assert_eq!(code(quote(&lib)), "shielded.shield_pending");

    // Before the journal is read: no pending list, the shield calls wait.
    let unread = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::Broadcast)],
        own: None,
        ..FakeShielded::default()
    });
    let status = run(&fx, &unread, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.pending_shields.is_empty());
    assert_eq!(code(quote(&unread)), "platform.unavailable");
}

/// #402: this device's own shield that Platform refused after a penalty (its
/// apply failed after the signature, and Platform's proved state shows the
/// lock partly used) is kept with what is left: no longer a pending shield,
/// so it blocks no shield and its "Check again" is refused, and the
/// unfinished transfers list it Recoverable. A lock Platform has not proved
/// partly used stays pending.
#[test]
fn an_own_shield_refused_after_a_penalty_leaves_the_pending_shields() {
    let fx = Fixture::new(0);
    let (partly, plain) = (
        lock(1, LockStatus::InstantSendLocked),
        lock(2, LockStatus::InstantSendLocked),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![partly.clone(), plain.clone()],
        own: Some(HashSet::from([partly.outpoint.txid, plain.outpoint.txid])),
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    lib.spent_reads.lock().unwrap().insert(
        partly.outpoint,
        SpentRead::Remaining {
            credits: 600_000_000,
        },
    );

    for n in [1, 2] {
        assert_eq!(
            resume_with(&fx, &lib, n, Ok(Applied::Unknown)),
            ShieldState::ConsumptionUnknown
        );
    }
    let pending: Vec<_> = pending_of(&fx, &lib).into_iter().map(|p| p.0).collect();
    assert_eq!(
        pending,
        [plain.outpoint.txid.to_string()],
        "only the unproved one pends"
    );

    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let txid = partly.outpoint.txid.to_string();
    let again = run(&fx, &lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid, &grant).await
    });
    assert_eq!(
        code(again),
        "invalid_argument",
        "no Check again of a partly used lock"
    );
    assert_eq!(lib.applied.lock().unwrap().len(), 2, "not shielded again");

    let listed = run(&fx, &lib, move |s, lib| async move {
        let kind = crate::platform::unfinished_shields::ShieldLocks::new(&s, wallet, &*lib);
        crate::platform::unfinished_locks::detect(&kind)
            .await
            .unwrap()
    });
    let state = |at: OutPoint| {
        let l = listed
            .iter()
            .find(|l| l.txid == at.txid.to_string())
            .unwrap();
        (l.state, l.remaining_credits)
    };
    use crate::platform::unfinished_locks::UnfinishedState;
    assert_eq!(
        state(partly.outpoint),
        (UnfinishedState::Recoverable, Some(600_000_000))
    );
    assert_eq!(state(plain.outpoint).0, UnfinishedState::Checking);
}

/// E1 r1 R1-2: a shield whose apply failed after its transition was
/// signed may have executed: it pends "Checking", never `NotApplied`, and
/// blocks no new shield. That shield funds a new lock of the wallet's
/// coins and applies that lock, never the pending one; "Check again"
/// applies the pending one's same outpoint (DEC-1045 (b)), and only its
/// proved apply settles it.
#[test]
fn an_unknown_shield_pends_and_resume_applies_the_same_lock() {
    let fx = Fixture::new(0);
    let pending = lock(1, LockStatus::InstantSendLocked);
    let lib = Arc::new(FakeShielded {
        locks: vec![pending.clone()],
        own: Some(HashSet::from([pending.outpoint.txid])),
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));

    // The pending lock's apply fails after its signature.
    let state = resume_with(&fx, &lib, 1, Ok(Applied::Unknown));
    assert_eq!(state, ShieldState::ConsumptionUnknown);
    assert!(lib.unknown.lock().unwrap().contains(&pending.outpoint.txid));
    assert!(lib.settles.lock().unwrap().is_empty());

    // A new shield: a new lock, applied itself; its apply is lost too.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 2_000_000));
    let shielded = run(&fx, &lib, move |s, lib| async move {
        pool::shield(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
    .unwrap();
    let new = lib.funded.lock().unwrap()[0].outpoint;
    assert_ne!(new, pending.outpoint);
    assert_eq!(shielded.txid, new.txid.to_string());
    assert_eq!(shielded.state, ShieldState::ConsumptionUnknown);
    assert_eq!(*lib.applied.lock().unwrap(), [pending.outpoint, new]);
    let mut listed = pending_of(&fx, &lib);
    listed.sort();
    let mut expected = vec![
        (pending.outpoint.txid.to_string(), true, true),
        (new.txid.to_string(), true, true),
    ];
    expected.sort();
    assert_eq!(listed, expected);

    // "Check again" on the new one: the same outpoint, proved, settled.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let (txid, grant) = (new.txid.to_string(), shield_grant(&fx, 1));
    let state = run(&fx, &lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid, &grant).await
    })
    .unwrap()
    .state;
    assert!(matches!(state, ShieldState::Applied { .. }), "{state:?}");
    assert_eq!(*lib.applied.lock().unwrap(), [pending.outpoint, new, new]);
    assert_eq!(*lib.settles.lock().unwrap(), [new.txid]);
    assert_eq!(lib.funded.lock().unwrap().len(), 1, "no other lock");
}

/// A refusal before the lock's key signed anything is `NotApplied`:
/// nothing reached Platform, the lock pends as not applied (it blocks a
/// new shield) and is not marked unknown.
#[test]
fn a_shield_refused_before_its_signature_is_not_applied() {
    let fx = Fixture::new(0);
    let pending = lock(1, LockStatus::InstantSendLocked);
    let lib = Arc::new(FakeShielded {
        locks: vec![pending.clone()],
        own: Some(HashSet::from([pending.outpoint.txid])),
        ..FakeShielded::default()
    });
    let state = resume_with(&fx, &lib, 1, Err(PlatformError::Unavailable));
    assert_eq!(
        state,
        ShieldState::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    assert!(lib.unknown.lock().unwrap().is_empty());
    assert_eq!(
        pending_of(&fx, &lib),
        [(pending.outpoint.txid.to_string(), true, false)]
    );
}

/// The library's result maps by whether the lock's key signed: an error
/// after a signature is unknown (it may follow the submission), before
/// one it is a refusal; "already consumed" is `Taken` either way.
#[test]
fn a_shield_result_is_unknown_once_its_transition_was_signed() {
    let lost = || PlatformWalletError::ShieldedBuildError("no result".into());
    assert_eq!(
        pool::applied_from(Ok(()), OutPoint::null(), true).unwrap(),
        Applied::Done
    );
    assert_eq!(
        pool::applied_from(Err(lost()), OutPoint::null(), true).unwrap(),
        Applied::Unknown
    );
    assert!(pool::applied_from(Err(lost()), OutPoint::null(), false).is_err());
    for signed in [false, true] {
        let consumed = PlatformWalletError::AssetLockAlreadyConsumed(OutPoint::null());
        assert_eq!(
            pool::applied_from(Err(consumed), OutPoint::null(), signed).unwrap(),
            Applied::Taken
        );
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        use key_wallet::Signer;
        let path = key_wallet::bip32::DerivationPath::default();
        let refusing = TestSigner(false);
        let witness = pool::Witnessed::new(&refusing);
        assert!(witness.public_key(&path).await.is_ok());
        assert!(witness.sign_ecdsa(&path, [1; 32]).await.is_err());
        assert!(!witness.signed(), "a refused signature is none");
        let signing = TestSigner(true);
        let witness = pool::Witnessed::new(&signing);
        witness.sign_ecdsa(&path, [1; 32]).await.unwrap();
        assert!(witness.signed());
    });
}

/// Signs with a fixed key when `.0`, else refuses.
struct TestSigner(bool);

fn test_key() -> dpp::secp256k1::SecretKey {
    dpp::secp256k1::SecretKey::from_secret_bytes([7; 32]).unwrap()
}

#[async_trait::async_trait]
impl key_wallet::Signer for TestSigner {
    type Error = dw_vault::SignerError;

    fn supported_methods(&self) -> &[key_wallet::SignerMethod] {
        &[key_wallet::SignerMethod::Digest]
    }

    async fn sign_ecdsa(
        &self,
        _: &key_wallet::bip32::DerivationPath,
        sighash: [u8; 32],
    ) -> Result<
        (
            dashcore::ecdsa::EcdsaSignature,
            dashcore::ecdsa::EcdsaPublicKey,
        ),
        Self::Error,
    > {
        if !self.0 {
            return Err(dw_vault::SignerError::PathNotAllowed("refused".into()));
        }
        let key = test_key();
        let message = dpp::secp256k1::Message::from_digest(sighash);
        Ok((
            dpp::secp256k1::ecdsa::sign(message, &key).into(),
            key.public_key().into(),
        ))
    }

    async fn public_key(
        &self,
        _: &key_wallet::bip32::DerivationPath,
    ) -> Result<dashcore::ecdsa::EcdsaPublicKey, Self::Error> {
        Ok(test_key().public_key().into())
    }
}

/// The unknown marks live in `app.sqlite`, per wallet, across a reopen.
#[test]
fn unknown_shield_marks_survive_a_reopen() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("app.sqlite");
    let (a, b) = (crate::WalletId([1; 32]), crate::WalletId([2; 32]));
    let txid = |n: u8| lock(n, LockStatus::Broadcast).outpoint.txid;
    {
        let db = dw_appdb::AppDb::open(&path).unwrap();
        assert!(pool::read_unknown_shields(&db, a).unwrap().is_empty());
        pool::mark_unknown_shield(&db, a, txid(1)).unwrap();
        pool::mark_unknown_shield(&db, a, txid(1)).unwrap();
        pool::mark_unknown_shield(&db, a, txid(3)).unwrap();
    }
    let db = dw_appdb::AppDb::open(&path).unwrap();
    assert_eq!(
        pool::read_unknown_shields(&db, a).unwrap(),
        HashSet::from([txid(1), txid(3)])
    );
    assert!(pool::read_unknown_shields(&db, b).unwrap().is_empty());
}

/// An incoming row (cmx `n`, `created_at_ms` `at_ms`) or an outgoing one.
fn row(n: u32, direction: Direction, at_ms: u64) -> ShieldedActivityEntry {
    let mut id = [0xEE; 32];
    id[..4].copy_from_slice(&n.to_be_bytes());
    ShieldedActivityEntry {
        id,
        direction,
        created_at_ms: at_ms,
        note_cmxs: vec![id],
        ..entry(Kind::Received, 1, Status::Confirmed)
    }
}

fn facts(fx: &Fixture, lib: &Arc<FakeShielded>) -> ShieldedFacts {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        let mut facts = ShieldedFacts::default();
        pool::registration_facts(&s, wallet, &*lib, &mut facts)
            .await
            .unwrap();
        facts
    })
}

/// A registration's quote and start read their facts here: while the
/// library cannot spend from the key set they refuse `platform.unavailable`
/// before any read, so before the start takes a fallback. Mutant: no check.
#[test]
fn registration_facts_refuse_first_while_spends_are_off() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    *lib.pool_notes.lock().unwrap() = Some(300);
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    let (refused, read) = run(&fx, &lib, move |s, lib| async move {
        let mut facts = ShieldedFacts::default();
        let refused = pool::registration_facts(&s, wallet, &*lib, &mut facts).await;
        (refused.unwrap_err().code(), facts.pool_notes)
    });
    assert_eq!(refused, "platform.unavailable");
    assert_eq!(read, None, "nothing read");
    lib.no_keyset_spends.store(false, Ordering::SeqCst);
    assert_eq!(facts(&fx, &lib).pool_notes, Some(300));
}

fn rested(facts: &ShieldedFacts, now: u64) -> (bool, Option<u64>) {
    let r = shielded_readiness(version(LATEST), facts, Some(NOTE), now);
    let rested = r.checks.iter().find(|c| c.check == ShieldedCheck::Rested);
    (rested.unwrap().ready, r.ready_at)
}

/// C-funding F16, F17: each source of an unsettled spend holds the
/// registration alone (the journal's and the markers' also with the pool
/// unbound), and the rest runs from the newest incoming row in seconds,
/// past the first history page.
#[test]
fn registration_facts_read_each_unsettled_source_and_the_newest_incoming_row() {
    let fx = Fixture::new(0);
    let at = 1_700_000_000;
    let mut activity: Vec<_> = (0..520)
        .map(|n| row(n, Direction::Out, (at + 99) * 1000))
        .collect();
    activity.push(row(520, Direction::In, at * 1000 + 999));
    let lib = Arc::new(FakeShielded {
        notes: Vec::new(),
        activity,
        rest: Mutex::new(Some(pool::RestClock {
            scanned: true,
            ..Default::default()
        })),
        ..FakeShielded::default()
    });
    let settled = facts(&fx, &lib);
    assert!(settled.synced && settled.rest_known && !settled.unsettled);
    assert_eq!(settled.latest_incoming, Some(at));
    lib.held.lock().unwrap().insert([1; 32]);
    assert!(facts(&fx, &lib).unsettled, "held");
    lib.held.lock().unwrap().clear();
    lib.unresolved.lock().unwrap().push(unresolved(1));
    assert!(facts(&fx, &lib).unsettled, "unresolved");
    lib.unresolved.lock().unwrap().clear();
    assert!(!facts(&fx, &lib).unsettled);
    let unbound = Arc::new(FakeShielded {
        bound: false,
        ..FakeShielded::default()
    });
    assert!(!facts(&fx, &unbound).unsettled);
    (fx.engine.block_on(fx.s.leases.record_unrecorded_spend(
        fx.wallet,
        [9; 32],
        [7; 32],
        vec![[8; 32]],
    )))
    .unwrap();
    assert!(facts(&fx, &lib).unsettled, "marked");
    assert!(facts(&fx, &unbound).unsettled, "marked, unbound");
    let fx = Fixture::new(0);
    fx.engine.block_on(fx.s.leases.close_journal());
    assert!(!fx.s.leases.journal_ready());
    assert!(facts(&fx, &lib).unsettled, "journal");
    assert!(facts(&fx, &unbound).unsettled, "journal, unbound");
}

/// Ruling M2a: a note first seen in the wallet's first completed scan is
/// restored and rested once synced; one first seen after it rests three
/// hours from then, and a later pass does not move that; before the first
/// scan completes the rest is unknown.
#[test]
fn a_note_seen_after_the_first_scan_rests_three_hours_and_the_first_scans_at_once() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        activity: vec![row(1, Direction::In, 0)],
        ..FakeShielded::default()
    });
    let now = crate::events::unix_now();
    let before = facts(&fx, &lib);
    assert!(!before.rest_known);
    assert_eq!(rested(&before, now), (false, None), "first scan pending");
    let pass = |lib: &Arc<FakeShielded>| {
        run(&fx, lib, |_, lib| async move {
            pool::observe_rest(&*lib, true, crate::events::unix_now())
                .await
                .unwrap()
        })
    };
    assert_eq!(pass(&lib), Some(None));
    let restored = facts(&fx, &lib);
    assert!(restored.rest_known);
    assert_eq!(rested(&restored, now), (true, None), "restored, synced");
    let unsynced = ShieldedFacts {
        synced: false,
        ..restored
    };
    assert!(!rested(&unsynced, now).0);

    lib.rows.lock().unwrap().push(row(2, Direction::In, 0));
    let first = pass(&lib).flatten().unwrap();
    assert!(first >= now);
    lib.rows.lock().unwrap().push(row(3, Direction::Out, 0));
    let later = facts(&fx, &lib);
    assert_eq!(later.latest_incoming, Some(first), "a later pass keeps it");
    let end = first + SHIELDED_REST_SECS;
    assert_eq!(rested(&later, end - 1), (false, Some(end)));
    assert_eq!(rested(&later, end), (true, None));
}

/// A payment another wallet sent, found by a pass after the wallet's first
/// scan, is dated when the engine first saw it, before and after the rest
/// clock records it; one restored in the first scan stays undated, and so
/// does a row a scan derived that is not incoming.
#[test]
fn a_shielded_payment_received_after_the_first_scan_is_dated_when_first_seen() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        activity: vec![row(1, Direction::In, 0)],
        ..FakeShielded::default()
    });
    let observe = |lib: &Arc<FakeShielded>| {
        run(&fx, lib, |_, lib| async move {
            pool::observe_rest(&*lib, true, crate::events::unix_now())
                .await
                .unwrap()
        })
    };
    let dates = |lib: &Arc<FakeShielded>| {
        run(&fx, lib, |_, lib| async move {
            (pool::history(&*lib, 0, 10).await.unwrap().into_iter())
                .map(|a| (a.id, a.created_at))
                .collect::<std::collections::HashMap<_, _>>()
        })
    };
    let id = |n: u32| hex::encode(row(n, Direction::In, 0).id);
    assert_eq!(dates(&lib)[&id(1)], None, "before the first scan");
    observe(&lib);
    assert_eq!(dates(&lib)[&id(1)], None, "restored");

    let before = crate::events::unix_now();
    lib.rows.lock().unwrap().push(row(2, Direction::In, 0));
    lib.rows.lock().unwrap().push(row(3, Direction::Out, 0));
    let unrecorded = dates(&lib)[&id(2)].expect("dated before the clock records it");
    assert!(unrecorded >= before);
    let recorded = observe(&lib).flatten().unwrap();
    let shown = dates(&lib);
    assert_eq!(shown[&id(2)], Some(recorded));
    assert_eq!(shown[&hex::encode(row(3, Direction::Out, 0).id)], None);
    assert_eq!(shown[&id(1)], None);
}

/// Ruling M9: a note the clock's write dropped (a removed wallet's insert
/// is ignored, `Ok`) is first seen now, never rested.
#[test]
fn a_note_the_write_dropped_rests_from_now() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        rest: Mutex::new(Some(pool::RestClock {
            scanned: true,
            ..Default::default()
        })),
        drops_rest: AtomicBool::new(true),
        ..FakeShielded::default()
    });
    let now = crate::events::unix_now();
    let facts = facts(&fx, &lib);
    assert!(lib.rest.lock().unwrap().as_ref().unwrap().seen.is_empty());
    let seen = facts.latest_incoming.unwrap();
    assert!(seen >= now);
    assert_eq!(
        rested(&facts, now),
        (false, Some(seen + SHIELDED_REST_SECS))
    );
}

/// Ruling M2a: a rest clock that cannot be read is not rested, with no
/// time to wait for, and the quote still reads.
#[test]
fn an_unreadable_rest_clock_is_not_rested() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        activity: vec![row(1, Direction::In, 0)],
        ..FakeShielded::default()
    });
    *lib.rest.lock().unwrap() = None;
    let facts = facts(&fx, &lib);
    assert!(!facts.rest_known && facts.synced);
    assert_eq!(rested(&facts, crate::events::unix_now()), (false, None));
}

/// The library's premise the clock answers (platform-wallet 72d0a3e7
/// `activity.rs:455-473`): a scan-derived `Received` row carries no time,
/// and its note's commitment.
#[test]
fn a_scanned_receive_has_no_time_but_its_commitment() {
    let input = platform_wallet::wallet::shielded::activity::ScanDeriveInput {
        notes: vec![ShieldedNote {
            note_data: vec![0; 115],
            ..note(0, NOTE)
        }],
        outgoing: Vec::new(),
        own_addresses: vec![vec![1; 43]],
    };
    let derived = platform_wallet::wallet::shielded::derive_activity_from_scan_data(
        &input,
        &Default::default(),
    );
    let [row] = &derived.new_entries[..] else {
        panic!("{derived:?}")
    };
    assert_eq!(
        (row.kind.clone(), row.direction),
        (Kind::Received, Direction::In)
    );
    assert_eq!(row.created_at_ms, 0);
    assert_eq!(row.note_cmxs, vec![note(0, NOTE).cmx]);
}

/// Ruling M2b: an unspent own note rests whatever row the library files it
/// under, as platform-wallet's scan derives them: a shield from another
/// device of a wallet that has spent (a `SelfTransfer`, not incoming), and a
/// payment in the same batch as an own operation (no row of its own). What
/// the incoming rows name was seen in the first scan (restored).
#[test]
fn every_unspent_own_note_rests_whatever_row_it_has() {
    use platform_wallet::wallet::shielded::{
        ScanDeriveInput, ShieldedOutgoingNote, derive_activity_from_scan_data,
    };
    let own = vec![1u8; 43];
    let at = |n, height| ShieldedNote {
        block_height: height,
        note_data: vec![0; 115],
        ..note(n, NOTE)
    };
    let (spent, shield) = (
        ShieldedNote {
            is_spent: true,
            ..at(1, 1)
        },
        at(2, 5),
    );
    let from_device = derive_activity_from_scan_data(
        &ScanDeriveInput {
            notes: vec![spent, shield.clone()],
            outgoing: vec![ShieldedOutgoingNote {
                cmx: shield.cmx,
                recipient: own.clone(),
                value: NOTE,
                memo: vec![0; 36],
                block_height: 5,
            }],
            own_addresses: vec![own.clone()],
        },
        &Default::default(),
    )
    .new_entries;
    let filed = (from_device.iter()).find(|e| e.note_cmxs.contains(&shield.cmx));
    assert_eq!(filed.unwrap().direction, Direction::SelfTransfer);
    let (change, theirs) = (at(3, 7), at(4, 7));
    let batched = derive_activity_from_scan_data(
        &ScanDeriveInput {
            notes: vec![change.clone(), theirs.clone()],
            outgoing: Vec::new(),
            own_addresses: vec![own],
        },
        &[(change.cmx, [0xAA; 32])].into_iter().collect(),
    );
    assert!(batched.new_entries.is_empty(), "{batched:?}");

    let fx = Fixture::new(0);
    for (rows, fresh) in [(from_device, shield), (batched.new_entries, theirs)] {
        let restored = (rows.iter())
            .filter(|e| e.direction == Direction::In)
            .flat_map(|e| e.note_cmxs.iter().map(|cmx| (*cmx, 0)))
            .collect();
        let lib = Arc::new(FakeShielded {
            notes: vec![fresh],
            activity: rows,
            rest: Mutex::new(Some(pool::RestClock {
                scanned: true,
                seen: restored,
            })),
            ..FakeShielded::default()
        });
        let now = crate::events::unix_now();
        let facts = facts(&fx, &lib);
        let seen = facts.latest_incoming.unwrap();
        assert!(seen >= now);
        assert_eq!(
            rested(&facts, now),
            (false, Some(seen + SHIELDED_REST_SECS))
        );
    }
}

/// The rest clock lives in the wallet's local scope of `app.sqlite`,
/// across a reopen; a row that does not read fails the read.
#[test]
fn the_rest_clock_survives_a_reopen_never_moves_and_a_bad_row_fails_it() {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("app.sqlite");
    let (a, b) = (crate::WalletId([1; 32]), crate::WalletId([2; 32]));
    {
        let db = dw_appdb::AppDb::open(&path).unwrap();
        assert_eq!(pool::read_rest_clock(&db, a).unwrap(), Default::default());
        pool::write_rest_clock(&db, a, &[[3; 32]], (0, 5), true).unwrap();
        pool::write_rest_clock(&db, a, &[[4; 32]], (17, 17), false).unwrap();
        // An observer that read the clock before the mark: it neither
        // lowers a recorded time nor records a later note as restored.
        pool::write_rest_clock(&db, a, &[[4; 32], [5; 32]], (0, 20), false).unwrap();
    }
    let db = dw_appdb::AppDb::open(&path).unwrap();
    let clock = pool::read_rest_clock(&db, a).unwrap();
    assert!(clock.scanned);
    assert_eq!(
        clock.seen,
        [([3; 32], 0), ([4; 32], 17), ([5; 32], 20)].into()
    );
    assert_eq!(pool::read_rest_clock(&db, b).unwrap(), Default::default());
    let scope = dw_appdb::local_scope(&a.to_string());
    db.set_setting(&scope, "shielded.rest.zz", Some("1"))
        .unwrap();
    assert!(pool::read_rest_clock(&db, a).is_err());
}

/// A shield's lock spends at most 50 coins (its InstantSend proof stops
/// fitting a transition from about 65 inputs): the quote refuses when the
/// fewest coins that cover it, largest first, are more.
#[test]
fn a_shield_quote_refuses_a_lock_of_more_than_50_coins() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        // 100 coins of 30,000 duffs; the fee bound is 1,000.
        coins: vec![30_000; 100],
        spendable: Spendable {
            duffs: 3_000_000,
            inputs: 100,
            protected: 0,
        },
        ..FakeShielded::default()
    });
    let quote = |duffs: u64| {
        run(&fx, &lib, move |s, lib| async move {
            pool::shield_quote(&s, &*lib, duffs).await
        })
    };
    // 50 coins cover 1,499,000 + 1,000 exactly; one duff more takes 51.
    assert_eq!(MAX_LOCK_INPUTS, 50);
    quote(1_499_000).unwrap();
    assert_eq!(
        quote(1_499_001).unwrap_err(),
        ShieldedError::TooManyInputs {
            inputs: 51,
            max: 50
        }
    );
    // Larger coins first: one big coin covers it.
    let lib = Arc::new(FakeShielded {
        coins: [vec![30_000; 99], vec![2_000_000]].concat(),
        ..FakeShielded::default()
    });
    run(&fx, &lib, |s, lib| async move {
        pool::shield_quote(&s, &*lib, 1_499_001).await
    })
    .unwrap();
}

// MARK: the real library

/// The account's address through the real library: what its bind says,
/// and the address once bound.
fn bound_address(
    engine: &crate::Engine,
    session: &Arc<NetworkSession>,
    wallet: crate::WalletId,
) -> (ShieldedAccount, Option<String>) {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            let bound = lib.bind().await?;
            let address = (lib.address().await)
                .map(|raw| OrchardAddress::from_raw_bytes(&raw).unwrap())
                .map(|a| a.to_bech32m_string(Network::Regtest));
            Ok((bound, address))
        }))
        .unwrap()
}

/// The viewing key binds from the vault while it needs no prompt, and is
/// persisted, so after a restart the account binds with the vault locked;
/// a wallet never bound stays unbound while the vault is locked.
#[test]
fn the_viewing_key_binds_once_and_from_storage_after_a_restart() {
    use super::topup_tests::{fixture, options};
    let mut f = fixture(true);
    let fvk = f.session.vault.shielded_viewing_key(&f.wallet.0).unwrap();
    let views =
        platform_wallet::wallet::shielded::AccountViewingKeys::from_fvk_bytes(&fvk).unwrap();
    let want = OrchardAddress::from_raw_bytes(&views.default_address.to_raw_address_bytes())
        .unwrap()
        .to_bech32m_string(Network::Regtest);
    assert_eq!(
        bound_address(&f.engine, &f.session, f.wallet),
        (ShieldedAccount::Bound, Some(want.clone()))
    );

    f.engine
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = (f.engine)
        .block_on(f.engine.open_network(DashNetwork::Regtest, options()))
        .unwrap();
    f.session.vault.lock();
    assert_eq!(
        f.session.vault.shielded_viewing_key(&f.wallet.0),
        Err(VaultError::Locked)
    );
    assert_eq!(
        bound_address(&f.engine, &f.session, f.wallet),
        (ShieldedAccount::Bound, Some(want))
    );
    assert!(f.session.data_dir().join(pool::SHIELDED_DB_FILE).exists());

    // Never bound and locked: the status says unlock, and no address reads.
    let fresh = fixture(true);
    fresh.session.vault.lock();
    assert_eq!(
        bound_address(&fresh.engine, &fresh.session, fresh.wallet),
        (ShieldedAccount::Locked, None)
    );
    let (account, address) = fresh
        .engine
        .block_on(on_pool(&fresh.session, fresh.wallet, |_, lib| async move {
            let status = pool::status(&lib).await?;
            Ok((
                status.account,
                pool::receive_address(&lib).await.map_err(|e| e.code()),
            ))
        }))
        .unwrap();
    assert_eq!(account, Some(ShieldedAccount::Locked));
    assert_eq!(address, Err("shielded.not_synced"));

    // An unlock for mixing only still holds it back; after a full unlock the
    // next bind binds the key and the address reads.
    let unlock = |scope| {
        let session = Arc::clone(&fresh.session);
        (fresh.engine)
            .block_on(session.vault_op(move |v| v.unlock(super::topup_tests::PASS, scope)))
            .unwrap();
    };
    unlock(dw_vault::UnlockScope::MixingOnly);
    assert_eq!(
        bound_address(&fresh.engine, &fresh.session, fresh.wallet).0,
        ShieldedAccount::MixingOnly
    );
    fresh.session.vault.lock();
    unlock(dw_vault::UnlockScope::Full);
    let (account, address) = bound_address(&fresh.engine, &fresh.session, fresh.wallet);
    assert_eq!(account, ShieldedAccount::Bound);
    assert!(address.is_some(), "the address reads once bound");
}

/// Review: a pass holds the note store for the whole of a first scan. The
/// status bar's sync read and a bound key's address answer under it; the
/// full status waits for the store.
#[test]
fn the_sync_read_and_a_bound_address_answer_while_a_pass_holds_the_store() {
    let f = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    let (account, want) = bound_address(engine, session, wallet);
    assert!(account.is_bound());
    let manager = session.manager().unwrap();
    let dp = session.dashpay(wallet);
    let s = Arc::clone(session);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &manager, None).await.unwrap();
        let _pass = coordinator.store().write().await;
        let wait = Duration::from_secs(10);
        let info = (tokio::time::timeout(wait, dp.shielded_sync_info()).await)
            .expect("the sync read waits for no store")
            .unwrap();
        assert!(info.active);
        assert_eq!(info.account, Some(ShieldedAccount::Bound));
        let address = (tokio::time::timeout(wait, dp.shielded_receive_address()).await)
            .expect("a bound key's address waits for no store")
            .unwrap();
        assert_eq!(Some(address.address), want);
        let status = tokio::time::timeout(Duration::from_millis(300), dp.shielded_status());
        assert!(status.await.is_err(), "the full status reads the store");
    });
}

/// While the library cannot spend from the key set, every quote and every
/// way into the pool refuses before the coordinator opens: a pass holding
/// the note store delays none of them. With spends on, the same quote waits
/// for the store.
#[test]
fn the_spends_off_refusals_wait_for_no_pass() {
    let f = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    assert!(bound_address(engine, session, wallet).0.is_bound());
    let manager = session.manager().unwrap();
    let dp = session.dashpay(wallet);
    let s = Arc::clone(session);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &manager, None).await.unwrap();
        let _pass = coordinator.store().write().await;
        s.keyset_spends_off.store(true, Ordering::SeqCst);
        let wait = Duration::from_secs(10);
        macro_rules! refused {
            ($call:expr) => {
                (tokio::time::timeout(wait, $call).await)
                    .expect("the refusal waits for no store")
                    .unwrap_err()
                    .code()
            };
        }
        let one = FundAmount::Duffs { duffs: 100_000 };
        let codes = [
            refused!(dp.shielded_send_quote(String::new(), 1)),
            refused!(dp.unshield_quote(1, None)),
            refused!(dp.unshield_to_platform_quote(1, None)),
            refused!(dp.shield_quote(one)),
            refused!(dp.shield_quote(FundAmount::All)),
            refused!(dp.shield_mixed_quote()),
            refused!(dp.platform_shield_quote(super::WithdrawAmount::All)),
            refused!(dp.shielded_send(String::new(), 1, None, String::new(), true)),
            refused!(dp.unshield_to_core(1, None, String::new(), true)),
            refused!(dp.unshield_to_platform(1, None, String::new(), true)),
            refused!(dp.shield_from_core(one, String::new())),
            refused!(dp.shield_mixed_coins(String::new())),
        ];
        assert_eq!(codes, ["platform.unavailable"; 12]);

        s.keyset_spends_off.store(false, Ordering::SeqCst);
        let quote = dp.shielded_send_quote(String::new(), 1);
        let held = tokio::time::timeout(Duration::from_millis(300), quote);
        assert!(
            held.await.is_err(),
            "with spends on, the quote opens the pool"
        );
    });
}

/// A Shielded-funded registration's quote, start and unsent create refuse
/// before the pool opens while spends are off; settling a create that
/// already named its identity still opens it.
#[test]
fn a_shielded_registration_refuses_before_the_pool_while_spends_are_off() {
    use super::registration::{RegistrationFunding, RegistrationRequest};
    use super::registration_flow::{PwLib, RegistrationLib};
    use super::registration_shielded::ShieldedAsk;
    let f = super::topup_tests::fixture(false);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    let manager = session.manager().unwrap();
    let dp = session.dashpay(wallet);
    let grant = || {
        let purpose = dw_vault::GrantPurpose::PlatformOp {
            max_duffs: 100_000_000,
            max_credits: 100_000_000_000_000,
        };
        let credential = dw_vault::Credential::None;
        (session
            .vault()
            .authorize(purpose, Some(&wallet.0), credential))
        .unwrap()
        .id
    };
    let (start_grant, lease_grant) = (grant(), grant());
    let s = Arc::clone(session);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &manager, None).await.unwrap();
        let _pass = coordinator.store().write().await;
        let (_, lib) = PwLib::open(&s, wallet).await.unwrap();
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[lease_grant])
            .await)
            .unwrap();
        let req = || RegistrationRequest {
            label: "spendsoff2".into(),
            temporary_label: None,
            funding: RegistrationFunding::Shielded { denomination: 0 },
            initial_profile: None,
        };
        let ask = |identity: Option<String>| ShieldedAsk {
            draft: 1,
            owner: 0,
            index: 0,
            denomination: 0,
            fallback: String::new(),
            identity,
        };
        s.keyset_spends_off.store(true, Ordering::SeqCst);
        let wait = Duration::from_secs(10);
        macro_rules! refused {
            ($call:expr) => {
                (tokio::time::timeout(wait, $call).await)
                    .expect("the refusal waits for no store")
                    .unwrap_err()
                    .code()
            };
        }
        let codes = [
            refused!(dp.registration_quote(req())),
            refused!(dp.start_registration(req(), start_grant)),
            refused!(lib.shielded_fund(ask(None), Some(&lease))),
        ];
        assert_eq!(codes, ["platform.unavailable"; 3]);

        let named = dpp::prelude::Identifier::new([7; 32])
            .to_string(dpp::platform_value::string_encoding::Encoding::Base58);
        let settle = lib.shielded_fund(ask(Some(named)), Some(&lease));
        let settle = tokio::time::timeout(Duration::from_millis(300), settle);
        assert!(
            settle.await.is_err(),
            "settling a named create opens the pool"
        );

        s.keyset_spends_off.store(false, Ordering::SeqCst);
        let quote = tokio::time::timeout(Duration::from_millis(300), dp.registration_quote(req()));
        assert!(
            quote.await.is_err(),
            "with spends on, the quote opens the pool"
        );
    });
}

/// An engine with a regtest session and no vault.
fn vaultless(no_platform: bool) -> (tempfile::TempDir, crate::Engine, Arc<NetworkSession>) {
    struct Quiet;
    impl crate::EventSink for Quiet {
        fn emit(&self, _: crate::EngineEvent) {}
    }
    let dir = dw_testutil::private_tempdir();
    let engine = crate::Engine::new(
        crate::EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: dw_vault::VaultConfig {
                kdf: dw_vault::KdfPolicy::Fixed(dw_vault::KdfParams::TEST),
                os_store: Arc::new(dw_vault::MemoryOsStore::new()),
                ..dw_vault::VaultConfig::default()
            },
        },
        Arc::new(Quiet),
    )
    .unwrap();
    let options = crate::SessionOptions {
        no_platform,
        ..super::topup_tests::options()
    };
    let session = (engine.block_on(engine.open_network(DashNetwork::Regtest, options))).unwrap();
    (dir, engine, session)
}

fn watch_only_xpub() -> String {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[7; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap()).to_string()
}

/// Review: a wallet without a seed is watch-only whatever the vault's
/// state: unlocked for mixing only it is not asked for a full unlock (a
/// seeded wallet still is), and with no vault at all its status reads.
#[test]
fn a_watch_only_wallet_is_watch_only_in_every_vault_state() {
    let f = super::topup_tests::fixture(true);
    let watch = (f.engine)
        .block_on(
            f.session
                .import_watch_only(watch_only_xpub(), crate::WatchOnlyOptions::default()),
        )
        .unwrap();
    f.session.vault.lock();
    (f.engine)
        .block_on(
            f.session.vault_op(|v| {
                v.unlock(super::topup_tests::PASS, dw_vault::UnlockScope::MixingOnly)
            }),
        )
        .unwrap();
    assert_eq!(
        bound_address(&f.engine, &f.session, watch),
        (ShieldedAccount::WatchOnly, None)
    );
    let info = (f.engine)
        .block_on(f.session.dashpay(watch).shielded_sync_info())
        .unwrap();
    assert_eq!(info.account, Some(ShieldedAccount::WatchOnly));
    assert_eq!(
        bound_address(&f.engine, &f.session, f.wallet).0,
        ShieldedAccount::MixingOnly
    );
    let info = (f.engine)
        .block_on(f.session.dashpay(f.wallet).shielded_sync_info())
        .unwrap();
    assert_eq!(info.account, Some(ShieldedAccount::MixingOnly));

    let (_dir, engine, session) = vaultless(false);
    let watch = engine
        .block_on(session.import_watch_only(watch_only_xpub(), crate::WatchOnlyOptions::default()))
        .unwrap();
    let status = engine
        .block_on(session.dashpay(watch).shielded_status())
        .unwrap();
    assert_eq!(status.account, Some(ShieldedAccount::WatchOnly));
    let info = engine
        .block_on(session.dashpay(watch).shielded_sync_info())
        .unwrap();
    assert_eq!(info.account, Some(ShieldedAccount::WatchOnly));
    assert!(info.spends_available && status.spends_available);
}

/// Review: a session without Platform (`--no-platform`) runs no pass, so
/// Shielded is not active there and nothing waits on it.
#[test]
fn shielded_is_not_active_in_a_session_without_platform() {
    let (_dir, engine, session) = vaultless(true);
    let watch = engine
        .block_on(session.import_watch_only(watch_only_xpub(), crate::WatchOnlyOptions::default()))
        .unwrap();
    let dp = session.dashpay(watch);
    let status = engine.block_on(dp.shielded_status()).unwrap();
    assert!(!status.active);
    let info = engine.block_on(dp.shielded_sync_info()).unwrap();
    assert!(!info.active);
    assert_eq!(info.account, None);
    assert!(status.spends_available, "this pin's library spends");
    assert_eq!(info.spends_available, status.spends_available, "one switch");
    let sync = dp.sync_status().unwrap();
    assert!(!sync.platform_enabled);
}

/// A spend stored under the hook is represented by its artifact, while a
/// spend that predates the hook, which has none, is refused by the hook's
/// admit before any re-send and never recorded in the schema-9 journal.
#[test]
fn a_pass_leaves_hooked_spends_to_their_artifact_and_admit_refuses_unrecorded_ones() {
    use platform_wallet::wallet::shielded::store::PendingRedrive;
    use platform_wallet::wallet::shielded::{
        DispatchedNote, ShieldedDispatch, ShieldedDispatchKind, ShieldedDispatchPayload,
        ShieldedStore, SubwalletId,
    };
    let f = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    assert!(bound_address(engine, session, wallet).0.is_bound());
    let manager = session.manager().unwrap();
    let selected_note = note(6, NOTE);
    let hooked_bytes = signed_spend(
        5,
        &ShieldedDispatchKind::Transfer,
        [0; 32],
        &[selected_note.nullifier, [0xee; 32]],
    );
    let hooked_hash: [u8; 32] = Sha256::digest(&hooked_bytes).into();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let hooked = PendingRedrive {
            activity_id: [5; 32],
            anchor: [0; 32],
            nullifiers: vec![selected_note.nullifier],
            st_bytes: hooked_bytes.clone(),
            attempts: 0,
            identity_nonce_finalized: false,
            identity_user_abandoned: false,
            anchor_height: Some(1_000),
        };
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        let payload = ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Transfer,
            wallet_id: wallet.0,
            account_index: dw_vault::SHIELDED_ACCOUNT,
            activity_id: [5; 32],
            st_hash: hooked_hash,
            st_bytes: hooked_bytes,
            anchor: [0; 32],
            selected_notes: vec![DispatchedNote {
                nullifier: selected_note.nullifier,
                cmx: selected_note.cmx,
                value: selected_note.value,
                position: selected_note.position,
            }],
            published_nullifiers: vec![selected_note.nullifier, [0xee; 32]],
        };
        (coordinator.store().write().await)
            .save_note(subwallet, &selected_note)
            .unwrap();
        (coordinator.store().write().await)
            .arm_redrive(subwallet, hooked)
            .unwrap();
        assert_eq!(
            pool::EngineDispatch::new(&s, &coordinator)
                .dispatch(payload)
                .await,
            Ok(())
        );
        s.ensure_background().await;
        assert!(s.leases.background(&wallet).is_some());
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
    assert_eq!(
        engine
            .block_on(session.leases.shielded_artifact(wallet, [5; 32]))
            .unwrap()
            .unwrap()
            .st_hash,
        Some(hooked_hash)
    );
    assert!(
        engine
            .block_on(session.leases.shielded_redrives(wallet))
            .unwrap()
            .is_empty()
    );

    // A record armed before the hook is set has no artifact: the hook's
    // admit refuses its re-send, with or without the background lease, and
    // nothing is recorded in the schema-9 journal.
    let g = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&g.engine, &g.session, g.wallet);
    let manager = session.manager().unwrap();
    let st_bytes = b"a pre-hook transition".to_vec();
    let hash: [u8; 32] = Sha256::digest(&st_bytes).into();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        m.configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let coordinator = m.shielded_coordinator().await.unwrap();
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        (coordinator.store().write().await)
            .arm_redrive(
                subwallet,
                PendingRedrive {
                    activity_id: [7; 32],
                    anchor: [1; 32],
                    nullifiers: vec![[8; 32]],
                    st_bytes,
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                    anchor_height: Some(1_000),
                },
            )
            .unwrap();
        pool::coordinator(&s, &m, None).await.unwrap();
        s.ensure_background().await;
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        let hook = pool::EngineDispatch::new(&s, &coordinator);
        let unrecorded = (hook
            .admit(wallet.0, dw_vault::SHIELDED_ACCOUNT, [7; 32], hash)
            .await)
            .unwrap_err()
            .reason;
        assert!(unrecorded.contains("no artifact"), "{unrecorded}");
        s.lock_vault().await.unwrap();
        assert!(s.leases.background(&wallet).is_none());
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        let unleased = (hook
            .admit(wallet.0, dw_vault::SHIELDED_ACCOUNT, [7; 32], hash)
            .await)
            .unwrap_err()
            .reason;
        assert!(unleased.contains("background lease"), "{unleased}");
        let stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(wallet.0)
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1.activity_id, [7; 32]);
    });
    assert!(
        engine
            .block_on(session.leases.shielded_redrives(wallet))
            .unwrap()
            .is_empty()
    );

    // A hooked-only wallet does not depend on the schema-9 journal, even
    // while its vault is locked; the hook gates its re-send per artifact.
    let g = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&g.engine, &g.session, g.wallet);
    assert!(bound_address(engine, session, wallet).0.is_bound());
    let manager = session.manager().unwrap();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        let bytes = b"journal-independent transition".to_vec();
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        (coordinator.store().write().await)
            .arm_redrive(
                subwallet,
                PendingRedrive {
                    activity_id: [9; 32],
                    anchor: [2; 32],
                    nullifiers: vec![[10; 32]],
                    st_bytes: bytes,
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                    anchor_height: Some(1_000),
                },
            )
            .unwrap();
        s.leases
            .record_shielded_artifact(
                wallet,
                NewShieldedArtifact {
                    activity: [9; 32],
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(hash),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some([2; 32]),
                    identity_id: None,
                    selected: vec![[10; 32]],
                    published: vec![[10; 32]],
                },
            )
            .await
            .unwrap();
        s.ensure_background().await;
        let journal = s.leases.journal.get();
        s.leases.journal.set(None);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        let unread = pool::EngineDispatch::new(&s, &coordinator)
            .admit(wallet.0, dw_vault::SHIELDED_ACCOUNT, [9; 32], hash)
            .await
            .unwrap_err()
            .reason;
        assert!(unread.contains("could not be read"), "{unread}");
        s.leases.journal.set(journal);
        s.lock_vault().await.unwrap();
        let journal = s.leases.journal.get();
        s.leases.journal.set(None);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        s.leases.journal.set(journal);
    });
}

/// A spend stored under the hook is represented by its artifact, while a
/// spend that predates the hook is re-recorded in the schema-9 journal.
#[test]
#[ignore = "P13b: upstream asks admit before every re-send, so the engine no longer re-records a pre-hook spend in the schema-9 journal or holds the pass for it (PassHeld::Journal, PassHeld::NoLease)"]
fn a_pass_records_spends_stored_before_the_hook_and_leaves_hooked_ones_to_their_artifact() {
    panic!("needs the hookless re-record of a pre-hook spend, which P13b replaced with admit");
}

#[cfg(any())]
#[test]
fn a_pass_records_spends_stored_before_the_hook_and_leaves_hooked_ones_to_their_artifact() {
    use platform_wallet::wallet::shielded::store::PendingRedrive;
    use platform_wallet::wallet::shielded::{
        DispatchedNote, ShieldedDispatch, ShieldedDispatchKind, ShieldedDispatchPayload,
        ShieldedStore, SubwalletId,
    };
    let f = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    assert!(bound_address(engine, session, wallet).0.is_bound());
    let manager = session.manager().unwrap();
    let selected_note = note(6, NOTE);
    let hooked_bytes = b"a hooked transition".to_vec();
    let hooked_hash: [u8; 32] = Sha256::digest(&hooked_bytes).into();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let hooked = PendingRedrive {
            activity_id: [5; 32],
            anchor: [0; 32],
            nullifiers: vec![selected_note.nullifier],
            st_bytes: hooked_bytes.clone(),
            attempts: 0,
            identity_nonce_finalized: false,
            identity_user_abandoned: false,
        };
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        let payload = ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Transfer,
            wallet_id: wallet.0,
            account_index: dw_vault::SHIELDED_ACCOUNT,
            activity_id: [5; 32],
            st_hash: hooked_hash,
            st_bytes: hooked_bytes,
            anchor: [0; 32],
            selected_notes: vec![DispatchedNote {
                nullifier: selected_note.nullifier,
                cmx: selected_note.cmx,
                value: selected_note.value,
                position: selected_note.position,
            }],
            published_nullifiers: vec![selected_note.nullifier, [0xee; 32]],
        };
        (coordinator.store().write().await)
            .save_note(subwallet, &selected_note)
            .unwrap();
        (coordinator.store().write().await)
            .arm_unsent_redrive(subwallet, hooked)
            .unwrap();
        assert_eq!(
            pool::EngineDispatch::new(&s, &coordinator)
                .dispatch(payload)
                .await,
            Ok(())
        );
        s.ensure_background().await;
        assert!(s.leases.background(&wallet).is_some());
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
    assert_eq!(
        engine
            .block_on(session.leases.shielded_artifact(wallet, [5; 32]))
            .unwrap()
            .unwrap()
            .st_hash,
        Some(hooked_hash)
    );
    assert!(
        engine
            .block_on(session.leases.shielded_redrives(wallet))
            .unwrap()
            .is_empty()
    );

    // A record armed before the hook is set remains unmarked and is
    // re-recorded at admission; the wallet-wide lease still holds it.
    let g = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&g.engine, &g.session, g.wallet);
    let manager = session.manager().unwrap();
    let st_bytes = b"a pre-hook transition".to_vec();
    let hash: [u8; 32] = Sha256::digest(&st_bytes).into();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        m.configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let coordinator = m.shielded_coordinator().await.unwrap();
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        (coordinator.store().write().await)
            .arm_redrive(
                subwallet,
                PendingRedrive {
                    activity_id: [7; 32],
                    anchor: [1; 32],
                    nullifiers: vec![[8; 32]],
                    st_bytes,
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                },
            )
            .unwrap();
        pool::coordinator(&s, &m, None).await.unwrap();
        s.ensure_background().await;
        let journal = s.leases.journal.get();
        s.leases.journal.set(None);
        assert!(matches!(
            pool::admit_pass(&s, &m).await,
            Err(pool::PassHeld::Journal(_))
        ));
        s.leases.journal.set(journal);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
    let recorded = engine
        .block_on(session.leases.shielded_redrives(wallet))
        .unwrap();
    assert_eq!(recorded, [([7; 32], hash)]);

    engine.block_on(session.lock_vault()).unwrap();
    assert!(session.leases.background(&wallet).is_none());
    let s = Arc::clone(session);
    let held = engine.block_on(async move { pool::admit_pass(&s, &manager).await.err() });
    assert_eq!(held, Some(pool::PassHeld::NoLease(wallet)));

    // A hooked-only wallet does not depend on the schema-9 journal, even
    // while its vault is locked; the hook gates its re-send per artifact.
    let g = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&g.engine, &g.session, g.wallet);
    assert!(bound_address(engine, session, wallet).0.is_bound());
    let manager = session.manager().unwrap();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        let bytes = b"journal-independent transition".to_vec();
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        (coordinator.store().write().await)
            .arm_unsent_redrive(
                subwallet,
                PendingRedrive {
                    activity_id: [9; 32],
                    anchor: [2; 32],
                    nullifiers: vec![[10; 32]],
                    st_bytes: bytes,
                    attempts: 0,
                    identity_nonce_finalized: false,
                    identity_user_abandoned: false,
                },
            )
            .unwrap();
        s.leases
            .record_shielded_artifact(
                wallet,
                NewShieldedArtifact {
                    activity: [9; 32],
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(hash),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some([2; 32]),
                    identity_id: None,
                    selected: vec![[10; 32]],
                    published: vec![[10; 32]],
                },
            )
            .await
            .unwrap();
        s.ensure_background().await;
        let journal = s.leases.journal.get();
        s.leases.journal.set(None);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        let unread = pool::EngineDispatch::new(&s, &coordinator)
            .admit(wallet.0, dw_vault::SHIELDED_ACCOUNT, [9; 32], hash)
            .await
            .unwrap_err()
            .reason;
        assert!(unread.contains("could not be read"), "{unread}");
        s.leases.journal.set(journal);
        s.lock_vault().await.unwrap();
        let journal = s.leases.journal.get();
        s.leases.journal.set(None);
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        s.leases.journal.set(journal);
    });
}

/// R1-3 (E1 review r1): the wallets the library's pass covers are the ones
/// its admission checked. A pass is paused after admitting wallet A alone;
/// meanwhile wallet B, unloaded with a stored spend of unknown result and
/// its viewing key persisted, is loaded and bound while the vault is locked.
/// The load and the bind wait for the pass to end, so the library never
/// syncs or re-sends for B in a pass that did not check it. B's spend
/// predates the hook and has no artifact, so the hook's admit refuses its
/// re-send, locked or unlocked, and it is never recorded in the journal.
#[test]
fn a_wallet_bound_during_a_pass_waits_for_it_and_its_unrecorded_spend_is_never_sent() {
    use platform_wallet::wallet::shielded::store::PendingRedrive;
    use platform_wallet::wallet::shielded::{ShieldedDispatch, ShieldedStore, SubwalletId};
    const PHRASE_B: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let mut f = super::topup_tests::fixture(true);
    let (engine, a) = (&f.engine, f.wallet);
    let b = engine
        .block_on(f.session.import_wallet(
            Zeroizing::new(PHRASE_B.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(bound_address(engine, &f.session, a).0.is_bound());
    assert!(
        bound_address(engine, &f.session, b).0.is_bound(),
        "B's viewing key persisted"
    );
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, super::topup_tests::options()))
        .unwrap();
    // The reopen loads every wallet; B joins only during the pass.
    engine.block_on(f.session.unload_wallet(b)).unwrap();
    let st_bytes = b"B's signed transfer".to_vec();
    let redrive = PendingRedrive {
        activity_id: [0xb1; 32],
        anchor: [0xb2; 32],
        nullifiers: vec![[0xb3; 32]],
        st_bytes: st_bytes.clone(),
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
        anchor_height: Some(1_000),
    };
    let manager = f.session.manager().unwrap();
    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    engine.block_on(async move {
        m.configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let coordinator = m.shielded_coordinator().await.unwrap();
        let sub = SubwalletId::new(b.0, dw_vault::SHIELDED_ACCOUNT);
        // A spend stored before the hook (a 0.1 spend): the one admission holds.
        (coordinator.store().write().await)
            .arm_redrive(sub, redrive)
            .unwrap();
        pool::coordinator(&s, &m, None).await.unwrap();
    });
    engine.block_on(f.session.lock_vault()).unwrap();
    assert!(f.session.leases.background(&a).is_none());

    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    engine.block_on(async move {
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        *s.platform.pass_pause.lock().unwrap() = Some(pool::PassPause {
            reached: reached_tx,
            resume: resume_rx,
        });
        let pass = {
            let (s, m) = (Arc::clone(&s), Arc::clone(&m));
            tokio::spawn(async move { pool::guarded_pass(&s, &m, true).await })
        };
        tokio::time::timeout(Duration::from_secs(60), reached)
            .await
            .expect("the pass was held before its rendezvous")
            .unwrap();
        let joined = {
            let s = Arc::clone(&s);
            tokio::spawn(async move {
                s.load_wallet(b).await.unwrap();
                on_pool(&s, b, |_, lib| async move { Ok(lib.bind().await?) }).await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !joined.is_finished(),
            "B joined while a pass that did not check it ran"
        );
        assert!(
            !(m.shielded_coordinator()
                .await
                .unwrap()
                .registered_subwallets()
                .await)
                .iter()
                .any(|sub| sub.wallet_id == b.0)
        );
        resume.send(()).unwrap();
        assert!(pass.await.unwrap(), "the paused pass was admitted (A only)");
        assert!(
            joined.await.unwrap().unwrap().is_bound(),
            "B bound from storage, vault locked"
        );

        // B's spend predates the hook and has no artifact: a pass runs, and
        // the hook's admit refuses the re-send, so nothing is sent.
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        assert!(pool::guarded_pass(&s, &m, true).await);
        let coordinator = m.shielded_coordinator().await.unwrap();
        let stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(b.0)
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1.attempts, 0, "never re-sent");
        let hash: [u8; 32] = Sha256::digest(&stored[0].1.st_bytes).into();
        let unleased = (pool::EngineDispatch::new(&s, &coordinator)
            .admit(b.0, dw_vault::SHIELDED_ACCOUNT, [0xb1; 32], hash)
            .await)
            .unwrap_err()
            .reason;
        assert!(unleased.contains("background lease"), "{unleased}");
    });
    assert!(
        engine
            .block_on(f.session.leases.shielded_redrives(b))
            .unwrap()
            .is_empty()
    );

    engine
        .block_on(
            f.session
                .vault_op(|v| v.unlock(super::topup_tests::PASS, dw_vault::UnlockScope::Full)),
        )
        .unwrap();
    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    let hash: [u8; 32] = Sha256::digest(&st_bytes).into();
    engine.block_on(async move {
        s.ensure_background().await;
        assert!(pool::admit_pass(&s, &m).await.is_ok());
        let coordinator = m.shielded_coordinator().await.unwrap();
        let unrecorded = (pool::EngineDispatch::new(&s, &coordinator)
            .admit(b.0, dw_vault::SHIELDED_ACCOUNT, [0xb1; 32], hash)
            .await)
            .unwrap_err()
            .reason;
        assert!(unrecorded.contains("no artifact"), "{unrecorded}");
    });
    assert!(
        engine
            .block_on(f.session.leases.shielded_redrives(b))
            .unwrap()
            .is_empty()
    );
}

/// R1-3 (E1 review r1): the wallets the library's pass covers are the ones
/// its admission checked. A pass is paused after admitting wallet A alone;
/// meanwhile wallet B, unloaded with a stored spend of unknown result and
/// its viewing key persisted, is loaded and bound while the vault is locked.
/// The load and the bind wait for the pass to end, so the library never
/// syncs or re-sends for B in a pass that did not check it. The next pass
/// is held back for B (no background lease, nothing re-sent), and once the
/// vault is unlocked B's spend is on record, by hash, before a pass runs.
#[test]
#[ignore = "P13b: upstream asks admit before every re-send, so the engine no longer holds the pass for a pre-hook spend (PassHeld::NoLease) or re-records it in the schema-9 journal"]
fn a_wallet_bound_during_a_pass_waits_for_it_and_is_admitted_before_its_spend_is_sent() {
    panic!("needs the hookless re-record of a pre-hook spend, which P13b replaced with admit");
}

#[cfg(any())]
#[test]
fn a_wallet_bound_during_a_pass_waits_for_it_and_is_admitted_before_its_spend_is_sent() {
    use platform_wallet::wallet::shielded::store::PendingRedrive;
    use platform_wallet::wallet::shielded::{ShieldedStore, SubwalletId};
    const PHRASE_B: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let mut f = super::topup_tests::fixture(true);
    let (engine, a) = (&f.engine, f.wallet);
    let b = engine
        .block_on(f.session.import_wallet(
            Zeroizing::new(PHRASE_B.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(bound_address(engine, &f.session, a).0.is_bound());
    assert!(
        bound_address(engine, &f.session, b).0.is_bound(),
        "B's viewing key persisted"
    );
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, super::topup_tests::options()))
        .unwrap();
    // The reopen loads every wallet; B joins only during the pass.
    engine.block_on(f.session.unload_wallet(b)).unwrap();
    let st_bytes = b"B's signed transfer".to_vec();
    let redrive = PendingRedrive {
        activity_id: [0xb1; 32],
        anchor: [0xb2; 32],
        nullifiers: vec![[0xb3; 32]],
        st_bytes: st_bytes.clone(),
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
    };
    let manager = f.session.manager().unwrap();
    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    engine.block_on(async move {
        m.configure_shielded(s.data_dir().join(pool::SHIELDED_DB_FILE))
            .await
            .unwrap();
        let coordinator = m.shielded_coordinator().await.unwrap();
        let sub = SubwalletId::new(b.0, dw_vault::SHIELDED_ACCOUNT);
        // A spend stored before the hook (a 0.1 spend): the one admission holds.
        (coordinator.store().write().await)
            .arm_redrive(sub, redrive)
            .unwrap();
        pool::coordinator(&s, &m, None).await.unwrap();
    });
    engine.block_on(f.session.lock_vault()).unwrap();
    assert!(f.session.leases.background(&a).is_none());

    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    engine.block_on(async move {
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        *s.platform.pass_pause.lock().unwrap() = Some(pool::PassPause {
            reached: reached_tx,
            resume: resume_rx,
        });
        let pass = {
            let (s, m) = (Arc::clone(&s), Arc::clone(&m));
            tokio::spawn(async move { pool::guarded_pass(&s, &m, true).await })
        };
        tokio::time::timeout(Duration::from_secs(60), reached)
            .await
            .expect("the pass was held before its rendezvous")
            .unwrap();
        let joined = {
            let s = Arc::clone(&s);
            tokio::spawn(async move {
                s.load_wallet(b).await.unwrap();
                on_pool(
                    &s,
                    b,
                    |_, lib| async move { Ok(lib.bind().await?.is_bound()) },
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !joined.is_finished(),
            "B joined while a pass that did not check it ran"
        );
        assert!(
            !(m.shielded_coordinator()
                .await
                .unwrap()
                .registered_subwallets()
                .await)
                .iter()
                .any(|sub| sub.wallet_id == b.0)
        );
        resume.send(()).unwrap();
        assert!(pass.await.unwrap(), "the paused pass was admitted (A only)");
        assert!(
            joined.await.unwrap().unwrap(),
            "B bound from storage, vault locked"
        );

        // B now holds a stored spend and no background lease: held back,
        // nothing re-sent, nothing recorded yet.
        assert_eq!(
            pool::admit_pass(&s, &m).await.err(),
            Some(pool::PassHeld::NoLease(b))
        );
        assert!(!pool::guarded_pass(&s, &m, true).await);
        let coordinator = m.shielded_coordinator().await.unwrap();
        let stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(b.0)
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1.attempts, 0, "never re-sent");
    });
    assert!(
        engine
            .block_on(f.session.leases.shielded_redrives(b))
            .unwrap()
            .is_empty()
    );

    engine
        .block_on(
            f.session
                .vault_op(|v| v.unlock(super::topup_tests::PASS, dw_vault::UnlockScope::Full)),
        )
        .unwrap();
    let (s, m) = (Arc::clone(&f.session), Arc::clone(&manager));
    engine.block_on(async move {
        s.ensure_background().await;
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
    let hash: [u8; 32] = Sha256::digest(&st_bytes).into();
    assert_eq!(
        engine
            .block_on(f.session.leases.shielded_redrives(b))
            .unwrap(),
        [([0xb1; 32], hash)]
    );
}

/// Lane V's reference through the facade: a mainnet session of the
/// "abandon … about" wallet shows the mobile wallets' shielded address.
#[test]
fn the_facade_shows_the_mobile_wallets_address_for_the_abandon_wallet() {
    use super::journal_tests::{ABANDON_12, Recorder, engine};
    use super::topup_tests::options;
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Mainnet, options()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let wallet = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let address = engine
        .block_on(s.dashpay(wallet).shielded_receive_address())
        .unwrap();
    assert_eq!(address.address, ABANDON_MAINNET);
    assert_eq!(address.account, 0);
}

/// The shipped switch is on for testnet only: mainnet, devnet and regtest
/// stay off until their funds review.
#[test]
fn the_shipped_switch_is_on_for_testnet_only() {
    for network in [
        Network::Mainnet,
        Network::Testnet,
        Network::Devnet,
        Network::Regtest,
    ] {
        assert_eq!(
            pool::release_keyset_spends(network),
            network == Network::Testnet,
            "{network:?}"
        );
    }
}

/// Shielded spends stay off until their funds review: the shipped switch,
/// [`pool::release_keyset_spends`], is off on every network but testnet
/// (read by `tests/shielded_spends_off.rs`). With the switch off, through
/// the real library the sync read says so and a send and an unshield to Core are
/// `platform.unavailable` before the grant is redeemed; forced on, the sync
/// read says spends are available and the same calls get past it to later
/// gates. On mainnet an unshield to a Platform address keeps its own
/// refusal, before it.
#[test]
fn shielded_spends_are_off_until_their_funds_review() {
    use super::journal_tests::{ABANDON_12, Recorder, engine};
    use super::topup_tests::options;
    const CREDITS: u64 = 1_000_000;
    for network in [DashNetwork::Mainnet, DashNetwork::Testnet] {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(dir.path(), Arc::new(Recorder::default()));
        let s = engine
            .block_on(engine.open_network(network.clone(), options()))
            .unwrap();
        engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
        let wallet = engine
            .block_on(s.import_wallet(
                Zeroizing::new(ABANDON_12.to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions::default(),
            ))
            .unwrap();
        let dp = s.dashpay(wallet);
        let to = (engine.block_on(dp.shielded_receive_address()))
            .unwrap()
            .address;
        pool::force_keyset_spends(&s, false);
        let switch = engine
            .block_on(on_pool(&s, wallet, |_, lib| async move {
                Ok(lib.keyset_spends())
            }))
            .unwrap();
        assert!(!switch, "{network:?}");
        let info = engine.block_on(dp.shielded_sync_info()).unwrap();
        assert!(!info.spends_available, "{network:?}");
        let refusals = |forced: Option<bool>| {
            if let Some(on) = forced {
                pool::force_keyset_spends(&s, on);
            }
            let grant = (s.vault())
                .authorize(
                    dw_vault::GrantPurpose::PlatformOp {
                        max_duffs: 0,
                        max_credits: 2 * CREDITS,
                    },
                    Some(&wallet.0),
                    dw_vault::Credential::None,
                )
                .unwrap()
                .id;
            let sent =
                engine.block_on(dp.shielded_send(to.clone(), CREDITS, None, grant.clone(), false));
            let unshielded =
                engine.block_on(dp.unshield_to_core(CREDITS, None, grant.clone(), false));
            let kept = (s.vault())
                .grant_purpose(&grant, dw_vault::GrantKind::PlatformOp, Some(&wallet.0))
                .is_ok();
            (
                sent.unwrap_err().code(),
                unshielded.unwrap_err().code(),
                kept,
            )
        };
        let (sent, unshielded, kept) = refusals(None);
        assert_eq!(
            (sent, unshielded, kept),
            ("platform.unavailable", "platform.unavailable", true),
            "{network:?}"
        );
        if network == DashNetwork::Mainnet {
            let to_platform =
                engine.block_on(dp.unshield_to_platform(CREDITS, None, "g".into(), false));
            assert_eq!(to_platform.unwrap_err().code(), "platform.feature_off");
        }
        let (sent, unshielded, kept) = refusals(Some(true));
        let info = engine.block_on(dp.shielded_sync_info()).unwrap();
        assert!(info.spends_available, "{network:?} forced on");
        assert_ne!(sent, "platform.unavailable", "{network:?} forced on");
        assert_ne!(unshielded, "platform.unavailable", "{network:?} forced on");
        assert!(kept, "{network:?}: refused before the grant");
    }
}

/// X1b-1 / DEC-1032 (owner, M3): a shield's lock spends at most 50 coins
/// whatever key-wallet's selection picks. Through the real build
/// (platform-wallet's `create_funded_asset_lock_proof`, key-wallet's
/// BranchAndBound over 51 coins of 0.01 DASH, every one needed), the
/// funding signer refuses the build when the builder shows it, before any
/// signature: the call is `shielded.too_many_inputs`-bound
/// (`FundError::TooManyInputs`), releases nothing and keeps no charge, the
/// fence recognises no lock of it, no lock is tracked (platform-wallet
/// tracks before it sends, so none was sent), no coin stays reserved, and
/// the step ends `Cancelled`, so the next shield is not held behind it and
/// Repair lists nothing.
#[test]
fn a_shield_lock_of_51_coins_is_refused_by_its_signer_and_leaves_nothing() {
    use super::lease::FlowOutcome;
    use platform_wallet::AssetLockFundingType;
    const COIN_DUFFS: u64 = 1_000_000;
    let f = super::topup_tests::fixture(false);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    let pw = engine.block_on(session.wallet(&wallet)).unwrap();
    let coins: Vec<OutPoint> = (0..51)
        .map(|vout| OutPoint::new(Txid::from_byte_array([0xef; 32]), vout))
        .collect();
    engine.block_on(async {
        let address = (pw.core().next_receive_address_for_account(0).await).unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        for &outpoint in &coins {
            let mut utxo = key_wallet::Utxo::new(
                outpoint,
                dashcore::TxOut {
                    value: COIN_DUFFS,
                    script_pubkey: address.script_pubkey(),
                },
                address.clone(),
                1,
                false,
            );
            utxo.is_confirmed = true;
            account.utxos.insert(outpoint, utxo);
        }
    });
    let reserved = || {
        engine.block_on(async {
            let fence = session.coin_fence(wallet).await.unwrap();
            let view = session.fenced_coins(&fence).await.unwrap();
            (view.coins.iter())
                .filter(|c| c.reserved && coins.contains(&c.utxo.outpoint))
                .count()
        })
    };
    let grant = session
        .vault()
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: 100 * COIN_DUFFS,
                max_credits: 0,
            },
            Some(&wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let s = Arc::clone(session);
    let (funded, outcome, released, budget, tracked, next) = engine.block_on(async move {
        use crate::platform::flows::BudgetPurpose;
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
        let before = lease.available(BudgetPurpose::Funding).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = pool::next_step(&s, wallet).unwrap();
        let key = FundingKey {
            funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
            identity_index: 0,
            account: 0,
        };
        // More than 50 coins' worth: all 51 are needed.
        let duffs = 50 * COIN_DUFFS + COIN_DUFFS / 2;
        let permit = (lib.funding_permit(&lease, step.clone(), key, duffs).await).unwrap();
        let funded = lib.fund(duffs, &permit).await;
        let released = permit.signatures().released();
        let outcome = permit.finish(funded.is_ok());
        let after = lease.available(BudgetPurpose::Funding).unwrap();
        let shield_locks = lib.shield_locks().await.len();
        let next = pool::next_step(&s, wallet);
        (
            funded,
            outcome,
            released,
            (before, after),
            shield_locks,
            next,
        )
    });
    let tracked = tracked + engine.block_on(pw.asset_locks().list_tracked_locks()).len();
    assert_eq!(funded, Err(FundError::TooManyInputs { max: 50 }));
    assert_eq!(released, 0, "refused before any signature");
    assert_eq!(outcome, FlowOutcome::Cancelled);
    assert_eq!(budget.0, budget.1, "its Funding charge refunded");
    assert_eq!(
        session.leases.ended_funding_steps(wallet),
        Vec::<String>::new(),
        "the fence recognises no lock of it"
    );
    assert_eq!(
        session.leases.stuck_shield_steps(wallet).unwrap(),
        vec![],
        "nothing for Repair"
    );
    assert_eq!(tracked, 0, "no lock tracked, so none sent");
    assert_eq!(reserved(), 0, "no coin stays reserved");
    assert!(next.is_ok(), "the next shield is not held: {next:?}");
}

/// A coin of a masternode collateral amount credited after a shield's coin
/// check and before the library's selection, the wallet's only coin: the
/// funding signer refuses the build before any signature, as no coin the
/// check read free, and the shield reads `CoinsLocked` with nothing
/// released, no lock tracked and the call `Cancelled`.
#[test]
fn a_shield_refuses_a_coin_credited_after_its_coin_check() {
    use super::lease::FlowOutcome;
    use platform_wallet::AssetLockFundingType;
    let f = super::topup_tests::fixture(false);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    let pw = engine.block_on(session.wallet(&wallet)).unwrap();
    let grant = session
        .vault()
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: 100_000_000,
                max_credits: 0,
            },
            Some(&wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let s = Arc::clone(session);
    let wallet_handle = Arc::clone(&pw);
    let (funded, outcome, released) = engine.block_on(async move {
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant]).await).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = pool::next_step(&s, wallet).unwrap();
        let key = FundingKey {
            funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
            identity_index: 0,
            account: 0,
        };
        let duffs = 10_000_000;
        let permit = (lib.funding_permit(&lease, step, key, duffs).await).unwrap();
        let address = (wallet_handle
            .core()
            .next_receive_address_for_account(0)
            .await)
            .unwrap();
        let coin = OutPoint::new(Txid::from_byte_array([0xe8; 32]), 0);
        let mut utxo = key_wallet::Utxo::new(
            coin,
            dashcore::TxOut {
                value: dw_protx::params::collateral_amount(false),
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        utxo.is_confirmed = true;
        (wallet_handle
            .state_mut()
            .await
            .core_wallet
            .accounts
            .standard_bip44_accounts)
            .get_mut(&0)
            .unwrap()
            .utxos
            .insert(coin, utxo);
        let funded = lib.fund(duffs, &permit).await;
        let released = permit.signatures().released();
        (funded, permit.finish(false), released)
    });
    assert_eq!(funded, Err(FundError::CoinsLocked));
    assert_eq!(released, 0, "refused before any signature");
    assert_eq!(outcome, FlowOutcome::Cancelled);
    assert!(
        engine
            .block_on(pw.asset_locks().list_tracked_locks())
            .is_empty(),
        "no lock tracked, so none sent"
    );
}

#[test]
fn ordinary_shield_funding_leaves_bip44_denominations_alone() {
    use super::lease::FlowOutcome;
    use dw_coinjoin::denoms::DENOMINATIONS;
    use platform_wallet::AssetLockFundingType;

    let f = super::topup_tests::fixture(false);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    let pw = engine.block_on(session.wallet(&wallet)).unwrap();
    let d = DENOMINATIONS[2];
    let denom_op = OutPoint::new(Txid::from_byte_array([0xd1; 32]), 0);
    engine.block_on(async {
        let address = (pw.core().next_receive_address_for_account(0).await).unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        let mut utxo = key_wallet::Utxo::new(
            denom_op,
            dashcore::TxOut {
                value: d,
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        utxo.is_confirmed = true;
        account.utxos.insert(denom_op, utxo);
    });

    let grant = || {
        session
            .vault()
            .authorize(
                dw_vault::GrantPurpose::PlatformOp {
                    max_duffs: 100_000_000,
                    max_credits: 0,
                },
                Some(&wallet.0),
                dw_vault::Credential::None,
            )
            .unwrap()
            .id
    };
    let key = FundingKey {
        funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
        identity_index: 0,
        account: 0,
    };

    let s = Arc::clone(session);
    let (values, spendable, only_denom_res, only_denom_sigs) = engine.block_on(async move {
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let values = lib.coin_values().await.unwrap();
        let spendable = lib.spendable().await.unwrap();
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant()]).await).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = pool::next_step(&s, wallet).unwrap();
        let permit = (lib.funding_permit(&lease, step, key, d - 1_000).await).unwrap();
        let funded = lib.fund(d - 1_000, &permit).await;
        let released = permit.signatures().released();
        let _ = permit.finish(false);
        (values, spendable, funded, released)
    });
    assert!(values.is_empty());
    assert_eq!(spendable.duffs, 0);
    assert!(
        matches!(only_denom_res, Err(FundError::NotSent(_))),
        "expected NotSent, got {only_denom_res:?}"
    );
    assert_eq!(only_denom_sigs, 0);
    let unlocked_after = engine.block_on(async {
        let state = pw.state().await;
        !state.core_wallet.accounts.standard_bip44_accounts[&0].utxos[&denom_op].is_locked
    });
    assert!(unlocked_after, "temporary selection lock is lifted on drop");

    // Credit an ordinary BIP44 coin beside the denomination and shield an amount
    // where BranchAndBound would prefer `denom_op` if it were selectable.
    let ordinary_op = OutPoint::new(Txid::from_byte_array([0xd2; 32]), 0);
    engine.block_on(async {
        let address = (pw.core().next_receive_address_for_account(0).await).unwrap();
        let mut state = pw.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        let mut utxo = key_wallet::Utxo::new(
            ordinary_op,
            dashcore::TxOut {
                value: 50_000_000,
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        utxo.is_confirmed = true;
        account.utxos.insert(ordinary_op, utxo);
    });
    let s = Arc::clone(session);
    let (values, sigs) = engine.block_on(async move {
        let lib = pool::PwShielded::open(&s, wallet).await.unwrap();
        let values = lib.coin_values().await.unwrap();
        let lease = (s.begin_lease(wallet, FlowKind::Shield, &[grant()]).await).unwrap();
        assert!(s.leases.refresh_funding(wallet).await);
        let step = pool::next_step(&s, wallet).unwrap();
        let target = d - 500;
        let permit = (lib.funding_permit(&lease, step, key, target).await).unwrap();
        let _ = lib.fund(target, &permit).await;
        let sigs = permit.signatures().released();
        let _ = permit.finish(false);
        (values, sigs)
    });
    assert_eq!(values, vec![50_000_000]);
    assert_eq!(
        sigs, 1,
        "selected and signed the ordinary coin, not the denomination"
    );

    // Pre-signing backstop: a denomination credited after the check is refused by the signer.
    let f2 = super::topup_tests::fixture(false);
    let (engine2, session2, wallet2) = (&f2.engine, &f2.session, f2.wallet);
    let pw2 = engine2.block_on(session2.wallet(&wallet2)).unwrap();
    let grant2 = session2
        .vault()
        .authorize(
            dw_vault::GrantPurpose::PlatformOp {
                max_duffs: 100_000_000,
                max_credits: 0,
            },
            Some(&wallet2.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let s2 = Arc::clone(session2);
    let wh2 = Arc::clone(&pw2);
    let (late_res, late_outcome, late_sigs) = engine2.block_on(async move {
        let lib = pool::PwShielded::open(&s2, wallet2).await.unwrap();
        let lease = (s2.begin_lease(wallet2, FlowKind::Shield, &[grant2]).await).unwrap();
        assert!(s2.leases.refresh_funding(wallet2).await);
        let step = pool::next_step(&s2, wallet2).unwrap();
        let permit = (lib.funding_permit(&lease, step, key, d - 1_000).await).unwrap();
        let address = (wh2.core().next_receive_address_for_account(0).await).unwrap();
        let late = OutPoint::new(Txid::from_byte_array([0xd3; 32]), 0);
        let mut utxo = key_wallet::Utxo::new(
            late,
            dashcore::TxOut {
                value: d,
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        utxo.is_confirmed = true;
        (wh2.state_mut()
            .await
            .core_wallet
            .accounts
            .standard_bip44_accounts)
            .get_mut(&0)
            .unwrap()
            .utxos
            .insert(late, utxo);
        let funded = lib.fund(d - 1_000, &permit).await;
        let released = permit.signatures().released();
        (funded, permit.finish(false), released)
    });
    assert_eq!(late_res, Err(FundError::CoinsLocked));
    assert_eq!(late_sigs, 0);
    assert_eq!(late_outcome, FlowOutcome::Cancelled);
}

/// A fake over the fixture's wallet whose shield locks are the real
/// library's tracked rows (`FakeShielded::real_rows`).
fn real_rows(fx: &Fixture) -> Arc<FakeShielded> {
    let lib = Arc::new(FakeShielded {
        real_rows: true,
        ..FakeShielded::default()
    });
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    lib
}

/// A shield of 1,000,000 duffs on `lib`, its apply answered as scripted.
fn shield_on(fx: &Fixture, lib: &Arc<FakeShielded>) -> (OutPoint, ShieldState) {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 2_000_000));
    let funded = lib.funded.lock().unwrap().len();
    let shielded = run(fx, lib, move |s, lib| async move {
        pool::shield(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
    .unwrap();
    let outpoint = lib.funded.lock().unwrap()[funded].outpoint;
    assert_eq!(shielded.txid, outpoint.txid.to_string());
    (outpoint, shielded.state)
}

/// "Check again" on the pending shield of `outpoint`, its apply (if any)
/// answered as scripted.
fn check_again(fx: &Fixture, lib: &Arc<FakeShielded>, outpoint: OutPoint) -> ShieldState {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    let txid = outpoint.txid.to_string();
    run(fx, lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid, &grant).await
    })
    .unwrap()
    .state
}

/// The real tracked row of `outpoint`.
fn row_of(fx: &Fixture, lib: &FakeShielded, outpoint: OutPoint) -> TrackedAssetLock {
    fx.engine
        .block_on(lib.row(outpoint))
        .expect("a tracked row")
}

/// The locks the dispatch journal records settled.
fn journal_settled(fx: &Fixture) -> HashSet<Txid> {
    fx.engine
        .block_on(fx.s.leases.top_ups_settled(fx.wallet))
        .unwrap()
}

/// `outpoint` is "Checking": listed `consumption_unknown`, not settled in
/// the journal, and blocking no new shield.
fn assert_checking(fx: &Fixture, lib: &Arc<FakeShielded>, outpoint: OutPoint, proved: bool) {
    let txid = outpoint.txid.to_string();
    let listed = pending_of(fx, lib);
    assert!(
        listed.contains(&(txid.clone(), proved, true)),
        "{txid} in {listed:?}"
    );
    assert!(
        !journal_settled(fx).contains(&outpoint.txid),
        "never settled"
    );
    assert!(lib.settles.lock().unwrap().is_empty(), "no settle");
    let quoted = run(fx, lib, |s, lib| async move {
        pool::shield_quote(&s, &*lib, 1_000_000).await.map(drop)
    });
    assert!(
        quoted.is_ok(),
        "a Checking lock blocks no new shield: {quoted:?}"
    );
}

/// Since `before` (coin reads, permits, locks funded), nothing funded a
/// lock or read the coins: "Check again" only applied.
fn assert_no_funding(lib: &FakeShielded, before: (u32, u32, usize)) {
    let now = (
        lib.coin_reads.load(Ordering::SeqCst),
        lib.permits.load(Ordering::SeqCst),
        lib.funded.lock().unwrap().len(),
    );
    assert_eq!(now, before, "no coin read, no funding permit, no new lock");
}

fn funding_counts(lib: &FakeShielded) -> (u32, u32, usize) {
    (
        lib.coin_reads.load(Ordering::SeqCst),
        lib.permits.load(Ordering::SeqCst),
        lib.funded.lock().unwrap().len(),
    )
}

/// DEC-1045 (b) as DEC-1051 widened it: a shield's apply hands the library
/// the tracked lock of its outpoint and nothing else to fund it.
#[test]
fn a_shields_apply_funds_from_the_tracked_lock_of_its_outpoint_only() {
    use platform_wallet::wallet::asset_lock::AssetLockFunding;
    let outpoint = OutPoint::new(Txid::from_byte_array([0x42; 32]), 3);
    match pool::shield_funding(outpoint) {
        AssetLockFunding::FromExistingAssetLock {
            out_point,
            consume_invitation_voucher,
        } => {
            assert_eq!(out_point, outpoint);
            assert!(!consume_invitation_voucher);
        }
        other => panic!("a shield's apply funded {other:?}"),
    }
}

/// DEC-1045 (c), r2 condition 2 (`orchestration.rs:478`): Platform's
/// "already consumed" never settles a shield. The library keeps the lock
/// "consumption unknown" with its ChainLock proof (`RecoveredFromChain`,
/// the real tracked row here), or, with no ChainLock proof to get, leaves
/// the row as it was; either way the lock is listed "Checking" with no
/// settle in the dispatch journal, and "Check again" stays available: the
/// pin re-submits the row's proof for the same outpoint
/// (`sync/recovery.rs:1696-1741`), and another "already consumed" leaves
/// it Checking again.
#[test]
fn already_consumed_leaves_a_shield_checking_and_check_again_available() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);

    // The library promotes the lock: RecoveredFromChain, ChainLock proof.
    lib.consumed.store(true, Ordering::SeqCst);
    *lib.row_after.lock().unwrap() = RowAfter::Recovered;
    let (promoted, state) = shield_on(&fx, &lib);
    assert_eq!(state, ShieldState::ConsumptionUnknown);
    let row = row_of(&fx, &lib, promoted);
    assert_eq!(row.status, AssetLockStatus::RecoveredFromChain);
    assert!(
        matches!(row.proof, Some(AssetLockProof::Chain(_))),
        "{:?}",
        row.proof
    );
    assert_eq!(row.proof.and_then(|p| p.out_point()), Some(promoted));
    assert_checking(&fx, &lib, promoted, true);

    // No ChainLock proof within the bound: the row stays as it was.
    lib.consumed.store(true, Ordering::SeqCst);
    let (kept, state) = shield_on(&fx, &lib);
    assert_ne!(kept, promoted);
    assert_eq!(state, ShieldState::ConsumptionUnknown);
    assert_eq!(
        row_of(&fx, &lib, kept).status,
        AssetLockStatus::InstantSendLocked
    );
    assert_checking(&fx, &lib, kept, true);

    let before = funding_counts(&lib);
    for round in 1..=2 {
        for outpoint in [promoted, kept] {
            lib.consumed.store(true, Ordering::SeqCst);
            let state = check_again(&fx, &lib, outpoint);
            assert_eq!(state, ShieldState::ConsumptionUnknown, "round {round}");
            assert_checking(&fx, &lib, outpoint, true);
        }
    }
    assert_eq!(
        row_of(&fx, &lib, promoted).status,
        AssetLockStatus::RecoveredFromChain
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [promoted, kept, promoted, kept, promoted, kept]
    );
    let before = (before.0 + 4, before.1, before.2); // the four quotes' coin reads
    assert_no_funding(&lib, before);
}

/// r2 condition 1 (DEC-1051): "Check again" reuses the identical outpoint
/// whatever proof the library derives for it, and never funds or selects
/// coins: (i) plain, the tracked InstantSend proof; (ii) the lock
/// chain-locked locally meanwhile, the library swapping to its ChainLock
/// proof inside the call; (iii) that ChainLock proof refused, the row
/// falling back to the record's InstantSend proof, or, with nothing better,
/// to no proof (`Broadcast`: "Check again" then waits for one and applies
/// nothing). The real tracked rows carry each proof; only the proved apply
/// at the end settles the lock. The seam is `ShieldedLib::apply`: each
/// proof change is simulated on the real row there, as the pin's
/// derivation would leave it (proof.rs:443-455, tracking.rs:564-620,
/// fund_from_asset_lock.rs:245-262); that derivation itself does not run.
#[test]
fn check_again_keeps_the_outpoint_whatever_proof_the_library_derives() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    let (outpoint, state) = shield_on(&fx, &lib);
    assert_eq!(state, ShieldState::ConsumptionUnknown);
    let before = funding_counts(&lib);
    let proof_of = |fx: &Fixture| row_of(fx, &lib, outpoint).proof;
    let applied = |n: usize| assert_eq!(*lib.applied.lock().unwrap(), vec![outpoint; n]);

    // (i) Plain: the tracked InstantSend proof.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    assert_eq!(
        check_again(&fx, &lib, outpoint),
        ShieldState::ConsumptionUnknown
    );
    let proof = proof_of(&fx);
    assert!(
        matches!(proof, Some(AssetLockProof::Instant(_))),
        "{proof:?}"
    );
    assert_eq!(proof.and_then(|p| p.out_point()), Some(outpoint));
    applied(2);

    // (ii) Chain-locked locally: the library carries the ChainLock proof.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    *lib.row_after.lock().unwrap() = RowAfter::ChainLocked;
    assert_eq!(
        check_again(&fx, &lib, outpoint),
        ShieldState::ConsumptionUnknown
    );
    let proof = proof_of(&fx);
    assert!(matches!(proof, Some(AssetLockProof::Chain(_))), "{proof:?}");
    assert_eq!(proof.and_then(|p| p.out_point()), Some(outpoint));
    assert_eq!(
        row_of(&fx, &lib, outpoint).status,
        AssetLockStatus::ChainLocked
    );
    applied(3);
    assert_checking(&fx, &lib, outpoint, true);

    // (iii) The ChainLock proof refused: the record's InstantSend proof.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    *lib.row_after.lock().unwrap() = RowAfter::InstantSend;
    assert_eq!(
        check_again(&fx, &lib, outpoint),
        ShieldState::ConsumptionUnknown
    );
    let proof = proof_of(&fx);
    assert!(
        matches!(proof, Some(AssetLockProof::Instant(_))),
        "{proof:?}"
    );
    assert_eq!(proof.and_then(|p| p.out_point()), Some(outpoint));
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    assert_eq!(
        check_again(&fx, &lib, outpoint),
        ShieldState::ConsumptionUnknown
    );
    applied(5);
    // ... or nothing better: no proof, so "Check again" applies nothing.
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    *lib.row_after.lock().unwrap() = RowAfter::Broadcast;
    assert_eq!(
        check_again(&fx, &lib, outpoint),
        ShieldState::ConsumptionUnknown
    );
    assert_eq!(check_again(&fx, &lib, outpoint), ShieldState::ProofWaiting);
    applied(6);
    assert_checking(&fx, &lib, outpoint, false);

    // Its ChainLock proof again, and a proved apply: settled.
    fx.engine
        .block_on(lib.change_row(outpoint, RowAfter::ChainLocked));
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let state = check_again(&fx, &lib, outpoint);
    assert!(matches!(state, ShieldState::Applied { .. }), "{state:?}");
    applied(7);
    assert!(journal_settled(&fx).contains(&outpoint.txid));
    assert!(pending_of(&fx, &lib).is_empty());
    let before = (before.0 + 2, before.1, before.2); // the two Checking quotes
    assert_no_funding(&lib, before);
}

/// A shield of 1,000,000 duffs on `lib` (no real rows), its apply answered
/// as scripted.
fn shield_with(fx: &Fixture, lib: &Arc<FakeShielded>) -> Result<ShieldState, ShieldedError> {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 2_000_000));
    run(fx, lib, move |s, lib| async move {
        pool::shield(
            &s,
            wallet,
            &*lib,
            FundAmount::Duffs { duffs: 1_000_000 },
            &grant,
        )
        .await
    })
    .map(|o| o.state)
}

/// A shield whose lock signed and then failed may be out, whatever the
/// library said of it: a stored lock is the step's until Repair's
/// self-spend shows the conflict, and a rebuild of the same lock on the
/// step would carry the ended call's signatures. Its step stays possibly
/// sent, a stored lock's inputs held, and the next shield is refused.
#[test]
fn a_shield_failing_after_its_signature_stays_possibly_sent() {
    use super::flows::DispatchState;
    let step = pool::shield_step(0);
    for (end, error) in [
        (FundEnd::BelowFloor, "platform.broadcast_unknown"),
        (FundEnd::Rejected, "platform.broadcast_unknown"),
        (FundEnd::Unknown, "platform.broadcast_unknown"),
    ] {
        let fx = Fixture::new(0);
        let lib = Arc::new(FakeShielded::default());
        *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
        *lib.fund_end.lock().unwrap() = Some(end);
        assert_eq!(code(shield_with(&fx, &lib)), error, "{end:?}");
        assert_eq!(
            fx.s.leases.funding_status(fx.wallet, &step),
            Some(DispatchState::MaybeSent),
            "{end:?}"
        );
        assert_eq!(
            fx.s.leases.unresolved_lock_inputs(fx.wallet).is_empty(),
            end == FundEnd::BelowFloor,
            "{end:?}"
        );
        assert_eq!(code(shield_with(&fx, &lib)), "platform.broadcast_unknown");
        assert!(lib.funded.lock().unwrap().is_empty());
    }
}

/// A shield whose wallet was removed or re-created after its lock was
/// signed, before the library tracked or sent it: its
/// released signature leaves the step possibly sent, so the call reads
/// `broadcast_unknown` whatever the library said, and the next shield is
/// refused on the step.
#[test]
fn a_shield_whose_wallet_left_after_its_signature_reads_broadcast_unknown() {
    shield_refused_after_the_signature(FundEnd::RemovedBeforeSend);
}

/// A shield the library refuses over the asset lock's input cap after
/// signing it reads the same.
#[test]
fn a_shield_over_the_asset_lock_input_cap_reads_broadcast_unknown() {
    shield_refused_after_the_signature(FundEnd::OverInputCap);
}

fn shield_refused_after_the_signature(end: FundEnd) {
    use super::flows::DispatchState;
    let step = pool::shield_step(0);
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    *lib.funder.lock().unwrap() = Some((Arc::downgrade(&fx.s), fx.wallet));
    *lib.fund_end.lock().unwrap() = Some(end);
    let unknown = |r: Result<ShieldState, ShieldedError>| match r {
        Err(ShieldedError::Platform(PlatformError::BroadcastUnknown { artifact })) => artifact,
        other => panic!("not broadcast_unknown: {other:?}"),
    };
    assert_eq!(unknown(shield_with(&fx, &lib)), step);
    assert_eq!(
        fx.s.leases.funding_status(fx.wallet, &step),
        Some(DispatchState::MaybeSent)
    );
    assert!(fx.s.leases.signed_lock_txids(fx.wallet, &step).is_empty());
    assert_eq!(unknown(shield_with(&fx, &lib)), step);
    assert!(lib.funded.lock().unwrap().is_empty());
}

/// A library that cannot spend from the key set: the status says so, active
/// or not, and a send, an unshield and their quotes (a contact payment's
/// too) are refused before anything else, so
/// nothing is quoted, no receive address is issued, the grant is kept and no
/// key leaves the vault. When it can again, the same send runs.
#[test]
fn a_spend_is_refused_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    let status = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(!status.spends_available);
    let below = Arc::new(FakeShielded::at(11, 11));
    below.no_keyset_spends.store(true, Ordering::SeqCst);
    let inactive = run(&fx, &below, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(!inactive.active);
    assert!(
        !inactive.spends_available,
        "read on an inactive network too"
    );
    for unshield in [false, true] {
        let grant = fx.grant(10 * NOTE);
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            code(if unshield {
                pool::unshield(&s, wallet, &*lib, NOTE, None, &g, false).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), NOTE), None, &g, false).await
            })
        });
        assert_eq!(refused, "platform.unavailable");
        assert!(fx.unused(&grant), "the refusal keeps the grant");
    }
    let quotes = run(&fx, &lib, |_, lib| async move {
        let lib = &*lib;
        vec![
            code(pool::send_quote(lib, &payee_text(), NOTE).await),
            code(pool::unshield_quote(lib, NOTE, None).await),
            code(pool::unshield_to_platform_quote(lib, NOTE, None).await),
            (super::shielded_payout::quote(lib, NOTE).await)
                .unwrap_err()
                .code(),
        ]
    });
    assert_eq!(quotes, ["platform.unavailable"; 4], "no quote either");
    assert!(lib.spent().is_empty(), "no key left the vault");
    assert_eq!(
        lib.refreshes.load(Ordering::SeqCst),
        0,
        "the refusal quoted nothing"
    );
    assert_eq!(
        lib.issued.load(Ordering::SeqCst),
        0,
        "the refusal issued no receive address"
    );

    lib.no_keyset_spends.store(false, Ordering::SeqCst);
    let status = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.spends_available);
    run(&fx, &lib, |_, lib| async move {
        pool::send_quote(&*lib, &payee_text(), NOTE).await.unwrap()
    });
    let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
    let sent = run(&fx, &lib, move |s, lib| async move {
        pool::send(
            &s,
            wallet,
            &*lib,
            (&payee_text(), NOTE),
            None,
            &lease,
            false,
        )
        .await
    });
    assert_eq!(sent.unwrap().state, ShieldedSpendState::Done);
}

/// A library that cannot spend from the key set refuses a new move into the
/// pool before anything else: no quote, no lock, the grant kept. When it can
/// again, the same shield runs.
#[test]
fn a_shield_is_refused_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let wallet = fx.wallet;
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    let quoted = run(&fx, &lib, |s, lib| async move {
        code(pool::shield_quote(&s, &*lib, 1_000_000).await)
    });
    assert_eq!(quoted, "platform.unavailable");
    let max = run(&fx, &lib, move |s, lib| async move {
        code(pool::shield_max_quote(&s, wallet, &*lib).await)
    });
    assert_eq!(max, "platform.unavailable");
    let mixed = run(&fx, &lib, move |s, lib| async move {
        code(super::shield_drain::quote(&s, wallet, &*lib).await)
    });
    assert_eq!(mixed, "platform.unavailable");
    let grant = shield_grant(&fx, 2_000_000);
    let g = grant.clone();
    let shielded = run(&fx, &lib, move |s, lib| async move {
        code(
            pool::shield(
                &s,
                wallet,
                &*lib,
                FundAmount::Duffs { duffs: 1_000_000 },
                &g,
            )
            .await,
        )
    });
    assert_eq!(shielded, "platform.unavailable");
    assert!(fx.unused(&grant), "the refusal keeps the grant");
    assert!(lib.funded.lock().unwrap().is_empty(), "no lock was funded");

    lib.no_keyset_spends.store(false, Ordering::SeqCst);
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    shield_on(&fx, &lib);
}

/// A shield already sent still finishes while the library cannot spend from
/// the key set: only a new move into the pool is refused, so what is locked
/// for Shielded is never stranded.
#[test]
fn a_shield_already_sent_finishes_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Unknown));
    let (outpoint, state) = shield_on(&fx, &lib);
    assert_eq!(state, ShieldState::ConsumptionUnknown);

    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    *lib.applies.lock().unwrap() = Some(Ok(Applied::Done));
    let state = check_again(&fx, &lib, outpoint);
    assert!(matches!(state, ShieldState::Applied { .. }), "{state:?}");
    assert_eq!(*lib.applied.lock().unwrap(), vec![outpoint; 2]);
}

/// A library that cannot spend from the key set issues no receive address
/// and quotes no shield from the Platform addresses: both refuse
/// `platform.unavailable` before any read. When it can again, both answer.
#[test]
fn no_way_in_opens_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::at(11, 12));
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    let refused = run(&fx, &lib, |_, lib| async move {
        let lib = &*lib;
        vec![
            code(pool::receive_address(lib).await),
            code(pool::platform_shield_gate(lib).await),
        ]
    });
    assert_eq!(refused, ["platform.unavailable"; 2]);
    assert_eq!(
        lib.refreshes.load(Ordering::SeqCst),
        0,
        "refused before any read"
    );

    lib.no_keyset_spends.store(false, Ordering::SeqCst);
    let (address, version) = run(&fx, &lib, |_, lib| async move {
        let address = pool::receive_address(&*lib).await.unwrap();
        let version = pool::platform_shield_gate(&*lib).await.unwrap();
        (address, version.protocol_version)
    });
    assert_eq!((address.address, version), (payee_text(), 12));
}

/// A payment to an address given out before the library lost key-set
/// spends still lands: the engine's sync runs its pass, which finds it, and
/// the status shows it in the balance, while no new address is issued.
#[test]
fn a_payment_to_an_earlier_address_shows_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::with_platform();
    let lib = Arc::new(FakeShielded::default());
    let early = run(&fx, &lib, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(early.address, payee_text());

    let (cancel, _cancelled) = tokio::sync::watch::channel(false);
    let (signals, _signalled) = tokio::sync::mpsc::unbounded_channel();
    *super::runtime::guard(&fx.s.platform.supervisor) = Some(super::runtime::Supervisor {
        cancel,
        signals,
        task: None,
        running: true,
    });
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    *lib.scan_ledger.lock().unwrap() = Some(Ledger {
        spendable: NOTE,
        synced_index: None,
    });
    let (status, again) = run(&fx, &lib, |s, lib| async move {
        let status = pool::sync(&s, &*lib).await.unwrap();
        (status, code(pool::receive_address(&*lib).await))
    });
    let synced = lib
        .scan_ledger
        .lock()
        .unwrap()
        .as_ref()
        .map(|l| l.synced_index);
    assert_eq!(synced, Some(Some(1)), "the sync ran its pass");
    assert_eq!(
        status.synced_index,
        Some(1),
        "the returned status includes the pass"
    );
    assert_eq!(status.balance, NOTE);
    assert!(!status.spends_available);
    assert_eq!(again, "platform.unavailable");
}

/// Through the real library: while it cannot spend from the key set the
/// facade issues no address, neither a bound key's (read during a pass)
/// nor a first bind's, and quotes no shield from the Platform addresses
/// (refused first, where with spends on a later gate refuses it); when it
/// can again, the same address.
#[test]
fn the_facade_issues_no_address_while_the_library_cannot_spend_from_the_key_set() {
    use super::topup_tests::fixture;
    let f = fixture(true);
    let dp = f.session.dashpay(f.wallet);
    assert!(bound_address(&f.engine, &f.session, f.wallet).0.is_bound());
    let address = f.engine.block_on(dp.shielded_receive_address()).unwrap();

    f.session.keyset_spends_off.store(true, Ordering::SeqCst);
    let refused = f.engine.block_on(dp.shielded_receive_address());
    assert_eq!(code(refused), "platform.unavailable", "the bound key's");
    let fresh = fixture(true);
    let off = &fresh.session.keyset_spends_off;
    off.store(true, Ordering::SeqCst);
    let first_dp = fresh.session.dashpay(fresh.wallet);
    let first = fresh.engine.block_on(first_dp.shielded_receive_address());
    assert_eq!(
        code(first),
        "platform.unavailable",
        "a first bind's, by either check"
    );
    // A wallet never bound, with the vault locked: with spends on, a later
    // gate refuses its Platform shield quote with another code.
    fresh.session.vault.lock();
    let shield_quote = || {
        let all = super::WithdrawAmount::All;
        let quote = fresh.engine.block_on(first_dp.platform_shield_quote(all));
        quote.unwrap_err().code()
    };
    assert_eq!(
        shield_quote(),
        "platform.unavailable",
        "no shield from Platform"
    );
    off.store(false, Ordering::SeqCst);
    assert_eq!(shield_quote(), "shielded.not_synced", "a later gate's");
    let status = (f.engine)
        .block_on(on_pool(&f.session, f.wallet, |_, lib| async move {
            pool::status(&lib).await
        }))
        .unwrap();
    assert!(!status.spends_available, "the status reads the same switch");

    f.session.keyset_spends_off.store(false, Ordering::SeqCst);
    let again = f.engine.block_on(dp.shielded_receive_address()).unwrap();
    assert_eq!(again, address);
}

/// Without the dispatch hook no spend starts: a send and an unshield are
/// refused before anything else, so nothing is quoted, no receive address
/// is issued, the grant is kept and no key leaves the vault.
#[test]
fn a_spend_is_refused_while_the_hook_is_not_set() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    lib.unhooked.store(true, Ordering::SeqCst);
    for unshield in [false, true] {
        let grant = fx.grant(10 * NOTE);
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            code(if unshield {
                pool::unshield(&s, wallet, &*lib, NOTE, None, &g, false).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), NOTE), None, &g, false).await
            })
        });
        assert_eq!(refused, "platform.unavailable");
        assert!(fx.unused(&grant), "the refusal keeps the grant");
    }
    assert!(lib.spent().is_empty(), "no key left the vault");
    assert_eq!(
        lib.refreshes.load(Ordering::SeqCst),
        0,
        "the refusal quoted nothing"
    );
    assert_eq!(
        lib.issued.load(Ordering::SeqCst),
        0,
        "the refusal issued no receive address"
    );

    // The hook is read again under the wallet's lock, after the quote:
    // one that is gone by then still refuses before the grant or a key.
    for unshield in [false, true] {
        *lib.hook_reads.lock().unwrap() = [true].into();
        let grant = fx.grant(10 * NOTE);
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            code(if unshield {
                pool::unshield(&s, wallet, &*lib, NOTE, None, &g, false).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), NOTE), None, &g, false).await
            })
        });
        assert_eq!(refused, "platform.unavailable", "read again under the lock");
        assert!(fx.unused(&grant), "the later refusal keeps the grant");
    }
    assert!(lib.spent().is_empty(), "no key left the vault");

    // With the hook the same send runs.
    lib.unhooked.store(false, Ordering::SeqCst);
    let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
    let sent = run(&fx, &lib, move |s, lib| async move {
        pool::send(
            &s,
            wallet,
            &*lib,
            (&payee_text(), NOTE),
            None,
            &lease,
            false,
        )
        .await
    });
    assert_eq!(sent.unwrap().state, ShieldedSpendState::Done);
}

/// While the library's key set spends are off a send, a withdrawal and an
/// unshield are `platform.unavailable` before anything else: nothing is
/// quoted, no receive address is issued, no key leaves the vault, nothing is
/// stored, reserved or recorded, and the grant is kept.
#[test]
fn a_spend_is_unavailable_while_key_set_spends_are_off() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    lib.no_keyset.store(true, Ordering::SeqCst);
    for route in ["send", "withdraw", "unshield"] {
        let grant = fx.grant(10 * NOTE);
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            match route {
                "send" => code(
                    pool::send(&s, wallet, &*lib, (&payee_text(), NOTE), None, &g, false).await,
                ),
                "withdraw" => code(pool::unshield(&s, wallet, &*lib, NOTE, None, &g, false).await),
                _ => {
                    code(pool::unshield_to_platform(&s, wallet, &*lib, NOTE, None, &g, false).await)
                }
            }
        });
        assert_eq!(refused, "platform.unavailable", "{route}");
        assert!(fx.unused(&grant), "{route} keeps the grant");
    }
    assert!(lib.spent().is_empty(), "no key left the vault");
    assert_eq!(lib.refreshes.load(Ordering::SeqCst), 0, "nothing quoted");
    assert_eq!(lib.issued.load(Ordering::SeqCst), 0, "no address issued");
    let (reserved, unresolved, held) = run(&fx, &lib, |_, lib| async move {
        (
            lib.reservations().await.unwrap(),
            lib.unresolved().await.unwrap(),
            lib.held_spends().await.unwrap(),
        )
    });
    assert!(reserved.is_empty() && unresolved.is_empty() && held.is_empty());
}

/// What a failed send returns follows what the library holds of it. A
/// spend refused before anything was sent (by the hook) is
/// `shielded.spend_refused`. A send that
/// failed after the library stored the spend, which then holds its notes
/// with no proof of either outcome, is pending, never failed, whatever the
/// error says, and writes no 0.1 record or marker. When what the library
/// holds cannot be read the payment may still go through:
/// `platform.broadcast_unknown`.
#[test]
fn a_failed_send_is_an_error_only_when_the_library_holds_nothing_of_it() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 4);
    let refused = || PlatformWalletError::ShieldedDispatchRefused {
        reason: "the plan check failed".into(),
    };
    let rejected = || PlatformWalletError::ShieldedBroadcastFailed("rejected".into());
    let send = |unshield: bool| {
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        run(&fx, &lib, move |s, lib| async move {
            if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &lease, false).await
            } else {
                pool::send(
                    &s,
                    wallet,
                    &*lib,
                    (&payee_text(), credits),
                    None,
                    &lease,
                    false,
                )
                .await
            }
        })
    };

    // Nothing held: errors.
    lib.fail_with(refused());
    assert_eq!(code(send(false)), "shielded.spend_refused");
    lib.fail_with(refused());
    assert_eq!(code(send(true)), "shielded.spend_refused");
    lib.fail_with(rejected());
    assert_eq!(
        code(send(false)),
        PlatformError::from(rejected()).code(),
        "a failed send the library freed is the library's error"
    );

    // Held: pending, for a failed broadcast and for a refusal alike.
    *lib.after_failure.lock().unwrap() = AfterFailure::Held;
    for (failure, unshield) in [(rejected(), false), (refused(), true)] {
        lib.fail_with(failure);
        let out = send(unshield).unwrap();
        assert_eq!(
            (out.state, out.unresolved),
            (ShieldedSpendState::Pending, None)
        );
        let kind = if unshield {
            ShieldedFeeKind::Withdrawal
        } else {
            ShieldedFeeKind::Base
        };
        assert_eq!(out.balance, 2 * NOTE - credits - fee_of(credits, kind));
    }
    let markers = fx
        .engine
        .block_on(fx.s.leases.unrecorded_spends(wallet))
        .unwrap();
    assert!(markers.is_empty());
    // A later send that fails with nothing new held is an error again.
    *lib.after_failure.lock().unwrap() = AfterFailure::Free;
    lib.fail_with(refused());
    assert_eq!(code(send(false)), "shielded.spend_refused");

    // Unreadable: may still go through.
    *lib.after_failure.lock().unwrap() = AfterFailure::Unreadable;
    lib.fail_with(rejected());
    match send(false) {
        Err(ShieldedError::Platform(PlatformError::BroadcastUnknown { artifact })) => {
            assert_eq!(artifact, "shielded transfer")
        }
        other => panic!("not broadcast_unknown: {other:?}"),
    }
}

/// A send that fails while the library still holds its signed transition,
/// armed (the write that ends the spend failed too, while reads work), is
/// pending, never an error: a later pass may send it, so the host must not
/// offer the payment again. The library keeps the transition to send. When
/// the stored transitions cannot be read the payment may still go through:
/// `platform.broadcast_unknown`.
#[test]
fn a_failed_send_the_library_still_holds_armed_is_pending() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 4);
    let failures = || {
        [
            PlatformWalletError::ShieldedStoreError("the store is busy".into()),
            PlatformWalletError::ShieldedBroadcastFailed("rejected".into()),
        ]
    };
    let send = |unshield: bool| {
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        run(&fx, &lib, move |s, lib| async move {
            if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &lease, false).await
            } else {
                pool::send(
                    &s,
                    wallet,
                    &*lib,
                    (&payee_text(), credits),
                    None,
                    &lease,
                    false,
                )
                .await
            }
        })
    };

    *lib.after_failure.lock().unwrap() = AfterFailure::Armed;
    for (n, (failure, unshield)) in (0u8..).zip(failures().into_iter().zip([false, true])) {
        lib.fail_with(failure);
        let out = send(unshield).expect("a spend the library still holds is not an error");
        assert_eq!(
            (out.state, out.unresolved),
            (ShieldedSpendState::Pending, Some(unresolved(n))),
            "pending, naming the transition the library holds"
        );
        assert!(
            lib.unresolved.lock().unwrap().contains(&unresolved(n)),
            "the library still holds it to send"
        );
    }
    assert!(lib.held.lock().unwrap().is_empty());

    // An older armed record cannot turn a new refusal into pending.
    let old_unresolved = lib.unresolved.lock().unwrap().clone();
    let old_reserved = lib.reserved.lock().unwrap().clone();
    let refusal = || PlatformWalletError::ShieldedDispatchRefused {
        reason: "the plan check failed".into(),
    };
    *lib.after_failure.lock().unwrap() = AfterFailure::Free;
    lib.fail_with(refusal());
    let error = send(false).expect_err("a new refusal is still an error");
    assert_eq!(error, pool::spend_error(refusal()));
    assert_eq!(lib.unresolved.lock().unwrap().clone(), old_unresolved);
    assert_eq!(lib.reserved.lock().unwrap().clone(), old_reserved);

    // The stored transitions cannot be read: may still go through.
    *lib.after_failure.lock().unwrap() = AfterFailure::StoredUnreadable;
    lib.fail_with(failures().into_iter().next().unwrap());
    match send(true) {
        Err(ShieldedError::Platform(PlatformError::BroadcastUnknown { artifact })) => {
            assert_eq!(artifact, "shielded withdraw")
        }
        other => panic!("not broadcast_unknown: {other:?}"),
    }
}

#[path = "shielded_dispatch_tests.rs"]
mod dispatch_tests;

#[path = "shield_drain_tests.rs"]
mod drain_tests;

#[path = "shield_drain_lib_tests.rs"]
mod drain_lib_tests;

#[path = "core_max_tests.rs"]
mod core_max_tests;

#[path = "core_max_lib_tests.rs"]
mod core_max_lib_tests;

#[path = "shielded_store_drain_tests.rs"]
mod store_drain_tests;
