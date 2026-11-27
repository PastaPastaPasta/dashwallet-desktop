//! App Connect v2 logins (DashPay Connect, DASHPAY F24; dips#181): a
//! `dashpay://connect` request read against Platform, the sheet the user
//! decides on, and the approval that adds the app's keys to the identity
//! and publishes the sealed response the app polls for.
//!
//! Nothing is derived or signed before the approval: the sheet and the
//! quote read Platform only. An approval runs under an `AppConnect` grant
//! bound to the request and to the digest of everything the quote showed;
//! it reads Platform again and refuses when anything differs. The vault
//! derives and seals the application keys (dw-vault `app_connect`); this
//! side sees their public keys and the sealed payload.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::identity_nonce::IDENTITY_NONCE_VALUE_FILTER;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::accessors::v1::IdentityPublicKeyGettersV1;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::BinaryData;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use dpp::util::hash::hash_double;
use dpp::version::PlatformVersion;
use dpp::version::fee::FeeVersion;
use dw_uri::connect::{AppConnectRequest, Bounds, ConnectUri, EncPurpose, Level};
use dw_vault::app_connect::AppKey;
use dw_vault::{GrantKind, GrantPurpose, SignerError, SignerScope, VaultSigner};
use platform_wallet::PlatformWallet;
use serde::{Deserialize, Serialize};

use super::app_connect_net::{ConnectNet, Response, SdkConnect};
use super::app_connect_plan::{
    self as plan, Commitment, ConnectRefusal, ContractFacts, EncPlan, GroupFacts, GroupInfoFacts,
    MAX_FUNDING, MIN_PROTOCOL_VERSION, PROFILE_TYPE,
};
use super::dashpay::DashPay;
use super::errors::{ConnectError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind};
use super::identity_writes::identity_locks;
use super::keys_policy::{key_path, missing_dashpay_purposes};
use super::lease::session::HeldKey;
use super::lease::{Lease, SignatureCount};
use super::requests::{leased, write_permit};
use super::signers::VaultIdentitySigner;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

/// What the IdentityUpdate adding the keys may cost, in credits.
pub(crate) const CONNECT_UPDATE_CREDITS: u64 = 300_000_000;
/// What the response document may cost, in credits.
pub(crate) const CONNECT_RESPONSE_CREDITS: u64 = 150_000_000;
/// What one login costs at most: the update and the response.
const CONNECT_FEE_CREDITS: u64 = CONNECT_UPDATE_CREDITS + CONNECT_RESPONSE_CREDITS;
/// Responses one grant pays for: the first and two "Retry sending" (a send
/// that may have gone out keeps its reservation).
const CONNECT_SENDS: u64 = 3;
/// The grant's credit bound.
const CONNECT_MAX_CREDITS: u64 = CONNECT_UPDATE_CREDITS + CONNECT_SENDS * CONNECT_RESPONSE_CREDITS;
/// Platform's most keys added, and disabled, in one IdentityUpdate.
const MAX_ADDED: usize = 6;
const MAX_DISABLED: usize = 10;
/// The confirmation reads after the update: three, 2 s then 4 s apart.
const CONFIRM_WAITS_MS: [u64; 3] = [0, 2_000, 4_000];
/// Inactive (`Disconnected`) App Connect records kept per wallet; active
/// records whose keys may still be enabled are never evicted.
const MAX_RECORDS: usize = 64;
const RECORDS_SETTING: &str = "appconnect.grants";
/// Responses kept for "Retry sending" in one session.
const MAX_PENDING: usize = 8;
/// Added storage bytes when Drive sets `disabled_at` on an existing key
/// (`DISABLE_KEY_TIME_BYTE_COST` in `rs-drive`).
const DISABLE_KEY_ADDED_BYTES: u32 = 9;
/// Conservative GroveDB Merk path depth for populated top-level trees (covers
/// root descent plus an AVL tree of > 2^22 entries at 1.44 * log2(n)).
const DISCONNECT_POPULATED_WALK_LEVELS: u32 = 36;
/// Upper bound on bytes loaded per interior Merk tree node during a walk.
const DISCONNECT_NODE_BYTES_PER_LEVEL: u64 = 192;
/// Upper bound on a serialized `IdentityPublicKey` (with bounds + limits).
const DISCONNECT_MAX_IDENTITY_KEY_BYTES: u32 = 512;
/// Upper bound on a serialized GroveDB key-reference element (primary or
/// untrusted `SiblingReference`).
const DISCONNECT_KEY_REF_BYTES: u32 = 128;
/// Explicit safety margin (1.25x, strictly <= 2x) on metered Drive processing
/// costs for tree-shape variance between quote and execution.
const DISCONNECT_FEE_MARGIN_NUM: u64 = 5;
const DISCONNECT_FEE_MARGIN_DEN: u64 = 4;
/// Framing bytes for `ContractGroupInfoV0` (owner 32 B + bincode/varint headers)
/// before counting `max_contract_group_admins`, UTF-8 name and UTF-8 description.
const CONTRACT_GROUP_INFO_FRAMING_BYTES: usize = 128;
/// The highest Platform credit fee a single Disconnect transition may spend
/// (0.001 DASH). Multi-key disconnects exceeding this bound are split into
/// smaller transitions; a single key above this bound is refused, never clamped.
pub(crate) const DISCONNECT_FEE_BOUND: u64 = 100_000_000;
/// Maximum retries to obtain an identity read at proved height >= nonce read height.
const CONSISTENT_READ_RETRIES: usize = 3;

/// The concrete bound-root lookup Drive meters when disabling one key
/// (`disable_identity_keys_operations_v1` -> `new_from_single_key` +
/// `refresh_potential_contract_info_key_references_v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisconnectKeyFeeSpec {
    /// `SingleContract` or `SingleContractDocumentType`: Drive loads the bound
    /// `DataContract` twice (once in `new_from_single_key`, once in
    /// `fetch_bound_root_with_fee`). If `!readonly`, the contract may grow up
    /// to `SystemLimits::max_state_transition_size` before execution.
    Contract {
        serialized_bytes: usize,
        readonly: bool,
    },
    /// `ContractGroup`: Drive loads `ContractGroupInfo` once in
    /// `fetch_bound_root_with_fee` (`new_from_single_key` does not read it).
    /// Sized to at least the protocol worst-case group metadata (64-char UTF-8
    /// name, 256-char UTF-8 description, and `max_contract_group_admins`).
    Group { serialized_bytes: usize },
}

/// One GroveDB operation's metered counters, matching `grovedb_costs::OperationCost`
/// and priced by the exact formula in pinned Drive `9d61fee` (`rs-drive/src/fees/op.rs`
/// `DriveCost::ephemeral_cost`).
#[derive(Debug, Clone, Copy, Default)]
struct MeteredGroveOp {
    seek_count: u32,
    added_bytes: u32,
    replaced_bytes: u32,
    storage_loaded_bytes: u64,
    hash_node_calls: u32,
}

impl MeteredGroveOp {
    fn ephemeral_cost(&self, fee_version: &FeeVersion) -> Result<u64, PlatformError> {
        let overflow = |label| PlatformError::Internal {
            detail: format!("drive ephemeral cost overflow ({label})"),
        };
        let written_rate = fee_version.storage.storage_processing_credit_per_byte;
        let seek_cost = (self.seek_count as u64)
            .checked_mul(fee_version.storage.storage_seek_cost)
            .ok_or_else(|| overflow("seek"))?;
        let added_cost = (self.added_bytes as u64)
            .checked_mul(written_rate)
            .ok_or_else(|| overflow("added"))?;
        let replaced_cost = (self.replaced_bytes as u64)
            .checked_mul(written_rate)
            .ok_or_else(|| overflow("replaced"))?;
        let loaded_cost = self
            .storage_loaded_bytes
            .checked_mul(fee_version.storage.storage_load_credit_per_byte)
            .ok_or_else(|| overflow("loaded"))?;
        let blake3_total = fee_version
            .hashing
            .blake3_base
            .checked_add(fee_version.hashing.blake3_per_block)
            .ok_or_else(|| overflow("blake3"))?;
        let hash_node_cost = blake3_total
            .checked_mul(self.hash_node_calls as u64)
            .ok_or_else(|| overflow("hash_node"))?;
        seek_cost
            .checked_add(added_cost)
            .and_then(|c| c.checked_add(replaced_cost))
            .and_then(|c| c.checked_add(loaded_cost))
            .and_then(|c| c.checked_add(hash_node_cost))
            .ok_or_else(|| overflow("sum"))
    }
}

/// Builds a worst-case `MeteredGroveOp` for one populated GroveDB tree walk
/// (`write = true` doubles `hash_node_calls` for AVL root-hash recomputation).
fn populated_walk_cost(
    write: bool,
    payload_loaded: u64,
    added_bytes: u32,
    replaced_bytes: u32,
) -> MeteredGroveOp {
    let interior_loaded =
        (DISCONNECT_POPULATED_WALK_LEVELS as u64).saturating_mul(DISCONNECT_NODE_BYTES_PER_LEVEL);
    MeteredGroveOp {
        seek_count: DISCONNECT_POPULATED_WALK_LEVELS,
        added_bytes,
        replaced_bytes,
        storage_loaded_bytes: interior_loaded.saturating_add(payload_loaded),
        hash_node_calls: if write {
            DISCONNECT_POPULATED_WALK_LEVELS.saturating_mul(2)
        } else {
            DISCONNECT_POPULATED_WALK_LEVELS
        },
    }
}

/// Exact Drive `ephemeral_cost` upper bound for the metered operations in
/// `disable_identity_keys_operations_v1` and
/// `refresh_potential_contract_info_key_references_v1` at `version`.
pub(crate) fn disconnect_metered_operations(
    version: &PlatformVersion,
    specs: &[DisconnectKeyFeeSpec],
) -> Result<(u64, u64), PlatformError> {
    // Refuse to establish a fee bound for unaudited future Drive versions.
    if version.drive.methods.identity.update.disable_identity_keys > 1
        || version
            .drive
            .methods
            .identity
            .contract_info
            .refresh_potential_contract_info_key_references
            > 1
    {
        return Err(PlatformError::Unavailable);
    }
    let fees = &version.fee_version;
    let limits = &version.system_limits;
    let max_group_info_bytes = CONTRACT_GROUP_INFO_FRAMING_BYTES
        .saturating_add((limits.max_contract_group_admins as usize).saturating_mul(32))
        .saturating_add((limits.max_contract_group_name_length as usize).saturating_mul(4))
        .saturating_add((limits.max_contract_group_description_length as usize).saturating_mul(4));

    let eval = |op: MeteredGroveOp| -> Result<u64, PlatformError> { op.ephemeral_cost(fees) };

    // 1. Transition base: min fee, signature verification, identity/nonce/balance
    //    processing, and 4 populated identity-tree operations (revision update,
    //    nonce update, balance update, MASTER key fetch).
    let mut processing = fees
        .state_transition_min_fees
        .identity_update
        .saturating_add(fees.signature.verify_signature_ecdsa_secp256k1)
        .saturating_add(
            fees.processing
                .fetch_identity_balance_and_revision_processing_cost,
        )
        .saturating_add(fees.processing.fetch_identity_balance_processing_cost)
        .saturating_add(fees.processing.fetch_key_with_type_nonce_and_balance_cost)
        .saturating_add(fees.processing.fetch_single_identity_key_processing_cost);
    // Revision, nonce, and balance updates (3 write walks) + MASTER key read (1 read walk).
    for _ in 0..3 {
        processing = processing.saturating_add(eval(populated_walk_cost(true, 16, 0, 16))?);
    }
    processing = processing.saturating_add(eval(populated_walk_cost(
        false,
        DISCONNECT_MAX_IDENTITY_KEY_BYTES as u64,
        0,
        0,
    ))?);

    let mut disk = 0u64;

    // 2. Per disabled key (exact Drive v1 operation list):
    //    - fetch_identity_keys_operations (1 read walk + key bytes)
    //    - replace_key_in_storage_operations (9 disk bytes + 1 write walk + key loaded & replaced)
    //    - refresh_identity_key_reference_operations_v0 (1 write walk on auth security-level tree)
    //    - bound-root fetch(es):
    //        Contract: 2 cold loads (new_from_single_key + fetch_bound_root_with_fee)
    //        Group:    1 cold load  (fetch_bound_root_with_fee only)
    //    - refresh_contract_info_operations_v1:
    //        primary reference write walk + untrusted SiblingReference write walk + target read walk
    for spec in specs {
        disk = disk.saturating_add(
            (DISABLE_KEY_ADDED_BYTES as u64)
                .saturating_mul(fees.storage.storage_disk_usage_credit_per_byte),
        );
        processing = processing
            .saturating_add(fees.processing.fetch_single_identity_key_processing_cost)
            .saturating_add(fees.processing.validate_key_structure)
            // fetch_identity_keys_operations
            .saturating_add(eval(populated_walk_cost(
                false,
                DISCONNECT_MAX_IDENTITY_KEY_BYTES as u64,
                0,
                0,
            ))?)
            // replace_key_in_storage_operations
            .saturating_add(eval(populated_walk_cost(
                true,
                DISCONNECT_MAX_IDENTITY_KEY_BYTES as u64,
                DISABLE_KEY_ADDED_BYTES,
                DISCONNECT_MAX_IDENTITY_KEY_BYTES,
            ))?)
            // refresh_identity_key_reference_operations_v0 (AUTHENTICATION/<level>)
            .saturating_add(eval(populated_walk_cost(
                true,
                DISCONNECT_KEY_REF_BYTES as u64,
                0,
                DISCONNECT_KEY_REF_BYTES,
            ))?);

        match *spec {
            DisconnectKeyFeeSpec::Contract {
                serialized_bytes,
                readonly,
            } => {
                // Mutable contracts can be updated up to `max_state_transition_size`
                // between quote and execution; readonly contracts cannot grow.
                let exec_bytes = if readonly {
                    serialized_bytes
                } else {
                    serialized_bytes.max(limits.max_state_transition_size as usize)
                } as u64;
                // Two cold contract loads: `new_from_single_key` and `fetch_bound_root_with_fee`.
                let one_load = eval(populated_walk_cost(false, exec_bytes, 0, 0))?;
                processing = processing.saturating_add(one_load.saturating_mul(2));
            }
            DisconnectKeyFeeSpec::Group { serialized_bytes } => {
                // One cold group metadata load in `fetch_bound_root_with_fee`,
                // sized at least to full protocol metadata (64-char UTF-8 name,
                // 256-char UTF-8 description, max_contract_group_admins).
                let exec_bytes = serialized_bytes.max(max_group_info_bytes) as u64;
                processing =
                    processing.saturating_add(eval(populated_walk_cost(false, exec_bytes, 0, 0))?);
            }
        }

        // refresh_contract_info_operations_v1: primary reference refresh +
        // untrusted latest-key SiblingReference refresh (write + target read).
        processing = processing
            .saturating_add(eval(populated_walk_cost(
                true,
                DISCONNECT_KEY_REF_BYTES as u64,
                0,
                DISCONNECT_KEY_REF_BYTES,
            ))?)
            .saturating_add(eval(populated_walk_cost(
                true,
                DISCONNECT_KEY_REF_BYTES as u64,
                0,
                DISCONNECT_KEY_REF_BYTES,
            ))?)
            .saturating_add(eval(populated_walk_cost(
                false,
                DISCONNECT_KEY_REF_BYTES as u64,
                0,
                0,
            ))?);
    }

    Ok((processing, disk))
}

/// Platform-grounded upper bound on the credit fee Drive meters to disable the
/// concrete bound keys in `specs` at `protocol_version`.
///
/// Prices every metered Drive operation (`OperationCost::ephemeral_cost` at
/// `PlatformVersion`'s real rates: 20 credits/loaded byte, 400 credits/written
/// byte, 2000/seek, 400/hash_node_call, 27000/disk byte) with an explicit
/// 1.25x (`5/4`) processing margin and exact disk cost. Refuses unaudited
/// future Drive method versions rather than guessing.
pub(crate) fn disconnect_fee_at(
    protocol_version: u32,
    specs: &[DisconnectKeyFeeSpec],
) -> Result<u64, PlatformError> {
    if specs.is_empty() {
        return Ok(0);
    }
    let version = PlatformVersion::get(protocol_version).map_err(|e| PlatformError::Internal {
        detail: format!("protocol version {protocol_version}: {e}"),
    })?;
    let (processing, disk) = disconnect_metered_operations(version, specs)?;
    let margined_processing = processing
        .saturating_mul(DISCONNECT_FEE_MARGIN_NUM)
        .div_ceil(DISCONNECT_FEE_MARGIN_DEN);
    Ok(margined_processing.saturating_add(disk))
}

/// Whether App Connect runs on `network`: not on mainnet in this build,
/// until its keys ruling; there every call is `connect.mainnet_off`.
pub(crate) fn enabled_on(network: &DashNetwork) -> bool {
    *network != DashNetwork::Mainnet
}

/// Why an App Connect login stopped after it was approved, from a fixed
/// list: the host shows its own words for each, never Platform's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectFailure {
    /// A key this login derives is on the identity with other limits.
    KeyConflict,
    /// Platform did not show every added key in time. The request stays
    /// open: approving it again re-reads the identity, reusing what landed.
    NotConfirmed,
    /// The identity has no key the wallet holds that may sign the response.
    NoResponseKey,
    /// No unsent response of this request is kept (a lock ended it).
    NothingToSend,
    /// A group or encryption key changed after the keys were added: the
    /// response is not sent.
    ScopeChanged,
    /// A key recorded for this app could not be verified on Platform as an
    /// app-added session key of this grant (for example an older grant with
    /// no recorded key digest, or a key whose public data or role does not
    /// match).
    UnverifiedKey,
    /// The live Platform credit fee to disable this app's keys exceeds the
    /// wallet's fee bound.
    FeeTooHigh,
    /// A prior unconfirmed Disconnect transition was superseded or rejected on
    /// Platform (for example a revision conflict or spent nonce) and retired;
    /// the app's keys remain enabled, and a new user-initiated, re-quoted
    /// Disconnect is required.
    DisconnectConflict,
    /// An earlier Disconnect transition is still pending confirmation on
    /// Platform and the live fee to resend it now exceeds the wallet's fee
    /// bound; the pending attempt is preserved so confirmation-only recovery
    /// still works.
    PendingFeeTooHigh,
    /// Another unresolved write on this identity (such as a credit withdrawal,
    /// credit transfer, or another app's pending disconnect) currently owns
    /// the identity's next nonce, so Approve or Disconnect refused to sign a
    /// colliding transition.
    PendingWrite,
}

/// What the user sets: one funding limit for the connection, one expiry
/// for its keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectTerms {
    /// Credits.
    pub funding_limit: u64,
    pub expires_at_ms: u64,
}

/// What the user picked on the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectChoice {
    /// Base58.
    pub identity: String,
    /// The auth bindings granted, by their index in the request; the
    /// primary (0) always.
    pub selection: Vec<u8>,
    pub terms: ConnectTerms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectLevel {
    Critical,
    High,
}

/// What a key of one binding may sign for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectScope {
    /// One contract of the app's owner. `tokens`: it has tokens, or an
    /// update could add some.
    Contract { contract: String, tokens: bool },
    /// One document type of a contract of the app's owner.
    DocumentType {
        contract: String,
        document_type: String,
    },
    /// The user's DashPay profile.
    Profile,
    /// A contract group of the app's owner, as Platform proved it now: it
    /// may grow, and the key covers what joins it.
    Group(ConnectGroup),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectGroup {
    pub group: String,
    pub owner: String,
    pub admins: Vec<String>,
    pub contracts: Vec<String>,
    pub document_types: Vec<ConnectDocumentType>,
    pub tokens: Vec<ConnectToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectDocumentType {
    pub contract: String,
    pub document_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectToken {
    pub contract: String,
    pub position: u16,
}

/// One auth binding of the request, as the sheet lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectBinding {
    pub index: u8,
    /// The primary binding, which cannot be turned off.
    pub primary: bool,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
}

/// One of the wallet's identities on the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectIdentity {
    pub identity: String,
    pub username: String,
    /// Credits, as the wallet last read them.
    pub balance: Option<u64>,
}

/// What the sheet shows before the user decides. The label is the app's
/// claim: nothing verifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectSheet {
    /// `hash160(appEphPub)`, hex.
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub app_owner: String,
    /// The identities with a username; the hinted one alone when the
    /// request names one.
    pub identities: Vec<ConnectIdentity>,
    pub bindings: Vec<ConnectBinding>,
    /// An encryption key for the app's contract is asked for.
    pub encryption: bool,
    pub terms: ConnectTerms,
    /// The highest funding limit an approval may set, in credits.
    pub max_funding_limit: u64,
    /// An approval of this request went to Platform from this wallet and
    /// no response to it is published: its keys may be on the identity.
    /// Approving it again with this choice reuses them.
    pub earlier_approval: Option<ConnectChoice>,
    pub block_time_ms: u64,
    /// The digest of the group facts the bindings show.
    pub groups: [u8; 32],
}

/// What an approval of `choice` costs and the grant it needs: an
/// `AppConnect` grant naming `request` and `plan`, with `max_credits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectQuote {
    pub request: [u8; 20],
    /// The digest of what this quote read.
    pub plan: [u8; 32],
    /// The digest of the group facts this quote read: a sheet showing
    /// others reads the request again.
    pub groups: [u8; 32],
    /// The grant's credit bound: the update and up to three sends.
    pub max_credits: u64,
    /// What the login costs at most: the update and one send.
    pub fee: u64,
    /// Each selected binding's budget, in selection order.
    pub budgets: Vec<u64>,
    pub balance: u64,
}

/// A finished login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectDone {
    pub identity: String,
    /// The key ids added to the identity.
    pub keys_added: Vec<KeyID>,
}

/// One still-enabled AUTHENTICATION session key of a connected app, as
/// Platform proves it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAppKey {
    pub key_id: KeyID,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
    /// Credits, as Platform proves it.
    pub budget: Option<u64>,
    pub expires_at_ms: Option<u64>,
}

/// One app this wallet approved for an identity that still has enabled
/// session keys on Platform. `label` is the app's own claim: nothing
/// verifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedApp {
    /// `hash160(appEphPub)`, hex: identifies this approval for Disconnect.
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub returns_to: Option<String>,
    pub identity: String,
    pub keys: Vec<ConnectedAppKey>,
    /// Credits: sum of the proved on-chain budgets of the still-enabled keys.
    pub funding_limit: u64,
    /// Latest expiry across all still-enabled keys, or `None` if any enabled
    /// key has no expiry on Platform.
    pub expires_at_ms: Option<u64>,
    pub approved_at_ms: u64,
}

/// The connected apps of one identity, with the proved Platform block time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedApps {
    pub identity: String,
    pub apps: Vec<ConnectedApp>,
    pub block_time_ms: u64,
}

/// What disconnecting one connected app costs and the `AppDisconnect` grant
/// it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectQuote {
    pub request: [u8; 20],
    /// Raw 32-byte identity id bound into `GrantPurpose::AppDisconnect`.
    pub identity_id: [u8; 32],
    pub identity: String,
    /// The verified session key ids that will be disabled.
    pub keys: Vec<KeyID>,
    /// The grant's credit bound, equal to `fee` (never clamped).
    pub max_credits: u64,
    /// Live Platform credit fee upper bound to disable `keys`.
    pub fee: u64,
    pub balance: u64,
    /// `true` when an earlier Disconnect attempt for this app was retired after
    /// a revision or nonce conflict and may already have incurred a Dash
    /// Platform fee.
    pub prior_attempt_conflict: bool,
    /// Observed identity balance debit (in credits) from that retired attempt,
    /// when Platform's balance proved a drop; `None` when no drop was observed
    /// or concurrent activity masked it.
    pub prior_attempt_debit: Option<u64>,
}

/// A finished disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectDone {
    pub identity: String,
    /// The key ids disabled on Platform (`empty` when already disabled).
    pub keys_disabled: Vec<KeyID>,
    /// `true` when an earlier sign-in response document was broadcast with an
    /// unknown outcome (or committed on Platform after the disable), so a
    /// response carrying a shared key may already have reached the app even
    /// though its session keys are now disabled.
    pub response_may_have_landed: bool,
}

impl From<ConnectRefusal> for ConnectError {
    fn from(r: ConnectRefusal) -> Self {
        match r {
            ConnectRefusal::Unsupported => ConnectError::Unsupported,
            ConnectRefusal::Invalid => ConnectError::Invalid,
            ConnectRefusal::Owner => ConnectError::Owner,
        }
    }
}

fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

fn signer_error(e: &SignerError) -> PlatformError {
    match e {
        SignerError::Locked => PlatformError::SignerUnavailable,
        _ => PlatformError::Internal {
            detail: "app connect key".into(),
        },
    }
}

/// The request's application keys under one call permit: public keys
/// freely, each proof of possession and the seal counted as one released
/// signature.
pub(crate) struct AppKeys {
    signer: VaultSigner,
    count: Arc<SignatureCount>,
}

impl AppKeys {
    pub(crate) fn new(
        signer: VaultSigner,
        count: Arc<SignatureCount>,
    ) -> Result<Self, EngineError> {
        if !matches!(signer.scope(), SignerScope::AppConnect { .. }) {
            return Err(EngineError::InvalidArgument(
                "an AppConnect signer is required".into(),
            ));
        }
        Ok(Self { signer, count })
    }

    fn public(
        &self,
        identity: [u8; 32],
        app: &[u8; 33],
        key: &AppKey,
    ) -> Result<[u8; 33], PlatformError> {
        self.signer
            .app_public_key(identity, app, key)
            .map_err(|e| signer_error(&e))
    }

    fn counted<T>(
        &self,
        op: impl FnOnce(&VaultSigner) -> Result<T, SignerError>,
    ) -> Result<T, PlatformError> {
        let signing = self.count.begin().ok_or(PlatformError::SignerUnavailable)?;
        match op(&self.signer) {
            Ok(out) => {
                signing.released();
                Ok(out)
            }
            Err(e) => {
                self.count.observe_err(&e);
                Err(signer_error(&e))
            }
        }
    }

    fn sign(
        &self,
        identity: [u8; 32],
        app: &[u8; 33],
        key: &AppKey,
        key_data: &[u8],
        data: &[u8],
    ) -> Result<[u8; 65], PlatformError> {
        self.counted(|s| s.sign_app_key(identity, app, key, key_data, data))
    }

    fn seal(
        &self,
        owner: [u8; 32],
        app: &[u8; 33],
        keys: &[AppKey],
    ) -> Result<Response, PlatformError> {
        let sealed = self.counted(|s| s.app_connect_seal(owner, app, keys))?;
        Ok(Response {
            request_hash: dw_uri::connect::request_hash(app),
            wallet_ephemeral_pub_key: sealed.wallet_ephemeral_pub_key,
            payload: sealed.payload,
        })
    }
}

/// The IdentityUpdate's signer: the MASTER key for the transition, and
/// each new encryption key for its own proof of possession. Session keys
/// are `ECDSA_HASH160` and sign nothing.
pub(crate) struct ConnectUpdateSigner {
    master: VaultIdentitySigner,
    app: AppKeys,
    identity: [u8; 32],
    app_eph_pub: [u8; 33],
    added: BTreeMap<KeyID, (AppKey, Vec<u8>)>,
}

impl std::fmt::Debug for ConnectUpdateSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectUpdateSigner")
            .field("added", &self.added.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Signer<IdentityPublicKey> for ConnectUpdateSigner {
    async fn sign(
        &self,
        key: &IdentityPublicKey,
        data: &[u8],
    ) -> Result<BinaryData, ProtocolError> {
        let Some((app_key, key_data)) = self.added.get(&key.id()) else {
            return self.master.sign(key, data).await;
        };
        if key.data().as_slice() != key_data.as_slice() {
            return Err(ProtocolError::ExternalSignerError(format!(
                "key {} is not the key this login adds",
                key.id()
            )));
        }
        let signature = (self.app)
            .sign(self.identity, &self.app_eph_pub, app_key, key_data, data)
            .map_err(|e| ProtocolError::ExternalSignerError(e.to_string()))?;
        Ok(signature.to_vec().into())
    }

    async fn sign_create_witness(
        &self,
        key: &IdentityPublicKey,
        _data: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        Err(ProtocolError::InvalidIdentityPublicKeyTypeError(
            dpp::state_transition::errors::InvalidIdentityPublicKeyTypeError::new(key.key_type()),
        ))
    }

    fn can_sign_with(&self, key: &IdentityPublicKey) -> bool {
        match self.added.get(&key.id()) {
            Some((_, data)) => {
                key.key_type() == KeyType::ECDSA_SECP256K1
                    && key.data().as_slice() == data.as_slice()
            }
            None => self.master.can_sign_with(key),
        }
    }
}

/// One login this wallet approved, kept in the app database: what a
/// disconnect, a later login of the same connection and "Retry sending"
/// read. Public keys and ids only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConnectRecord {
    pub(crate) request: String,
    pub(crate) app_id: String,
    pub(crate) label: String,
    pub(crate) identity: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) session_keys: Vec<KeyID>,
    pub(crate) encryption_keys: Vec<KeyID>,
    /// The bindings approved, by index.
    #[serde(default)]
    pub(crate) selection: Vec<u8>,
    pub(crate) funding_limit: u64,
    pub(crate) expires_at_ms: u64,
    pub(crate) state: RecordState,
    pub(crate) created_ms: u64,
    /// The callback host from the sign-in link, if any.
    #[serde(default)]
    pub(crate) returns_to: Option<String>,
    /// Hex-encoded on-chain `data` (20-byte `ECDSA_HASH160`) of each key in
    /// `session_keys`, recorded at approval so Disconnect verifies both key id
    /// and public key data before disabling.
    #[serde(default)]
    pub(crate) session_key_data: BTreeMap<KeyID, String>,
    /// An unconfirmed disable-only `IdentityUpdateTransition` persisted before
    /// broadcast so a retry resolves or re-broadcasts the exact same signed
    /// transition (same revision and nonce) instead of minting a second paid
    /// transition.
    #[serde(default)]
    pub(crate) pending_disconnect: Option<PendingDisconnect>,
    /// Delivery state of the sign-in response document so Disconnect can tell
    /// an unsent response apart from an in-flight broadcast whose outcome is
    /// unknown and may still commit with a shared key.
    #[serde(default)]
    pub(crate) response_delivery: ResponseDelivery,
    /// Summary of the most recent retired conflicting Disconnect attempt, so
    /// the next user-initiated, re-quoted Disconnect discloses any known or
    /// possible earlier fee charge before signing anew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_conflict: Option<DisconnectConflictInfo>,
}

/// Recorded outcome of a retired conflicting Disconnect attempt, used to
/// disclose any known or possible earlier fee charge on the next re-quoted
/// Disconnect and to keep its nonce reserved until proven spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DisconnectConflictInfo {
    pub(crate) transition_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_debit: Option<u64>,
    /// Nonce `k` of the retired revision-conflicted transition while `k` is
    /// not yet proven spent (`k <= tip && !takes` at `id_height >= nonce_height`).
    /// While `Some(k)`, the dead transition bytes can still be included by
    /// Drive as a paid `BumpIdentityNonceAction` consuming `k`, so transfers,
    /// withdrawals, and other apps treat `k` as reserved and the next
    /// re-quoted Disconnect for this record signs at `k` while admissible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nonce: Option<u64>,
    /// Proved identity nonce tip observed when `nonce: Some(k)` was persisted.
    /// Every skip past `k` (transfers, withdrawals, other apps' Disconnects,
    /// and Approve G1's `k <= tip` allowance) refuses `Unavailable` when the
    /// current read's tip is below `observed_tip` or when `observed_tip` is
    /// `None` (fail closed), so a lagging read cannot allocate a nonce already
    /// consumed by the conflicting update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_tip: Option<u64>,
    /// Proved Platform block height (`max(id_height, nonce_height)`) of the
    /// read that proved the conflict when `nonce: Some(k)` was persisted.
    /// Because `read_consistent_state` reads the nonce before the identity,
    /// `observed_tip` can come from a nonce read older than the identity read
    /// that observed the conflicting revision bump. Every skip past `k` must
    /// also require the current nonce read's height to be `>= observed_height`
    /// (failing closed when `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_height: Option<u64>,
}

/// Whether the sign-in `loginKeyResponse` document was provably never sent,
/// was broadcast with an unknown outcome, or was confirmed delivered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResponseDelivery {
    /// Provably not broadcast (or not attempted yet).
    #[default]
    Unsent,
    /// Broadcast was attempted and did not fail locally before dispatch
    /// (for example timed out), so it may commit on Platform even after
    /// Disconnect cancels local retries.
    BroadcastUnknown,
    /// Confirmed written on Platform.
    Delivered,
}

/// A signed disable-only `IdentityUpdateTransition` awaiting confirmation.
/// Public transition bytes only (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingDisconnect {
    pub(crate) keys: Vec<KeyID>,
    pub(crate) revision: u64,
    pub(crate) nonce: u64,
    pub(crate) transition_hex: String,
    /// Double-SHA256 hex digest of the signed transition bytes, for outcome
    /// reconciliation and diagnostics.
    #[serde(default)]
    pub(crate) transition_hash: String,
    /// Identity credit balance observed at sign time, to identify any debit
    /// incurred on a terminal conflict.
    #[serde(default)]
    pub(crate) balance_before: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordState {
    /// The update may be out.
    Submitted,
    /// Every key was seen on Platform.
    Confirmed,
    /// The response is out.
    Published,
    /// Every session key of this grant was disabled on Platform.
    Disconnected,
}

type PendingSendKey = (WalletId, Identifier, [u8; 20]);

/// The responses an approval sealed and could not publish, for "Retry
/// sending": the sealed payload (nothing secret) and the lease that signs
/// the document, which a lock ends. Keyed by `(wallet, identity, request)` so
/// two identities approving the same request never overwrite or cancel each
/// other's parked response.
#[derive(Default)]
pub(crate) struct PendingSends(Mutex<HashMap<PendingSendKey, PendingSend>>);

struct PendingSend {
    lease: Lease,
    identity: Identifier,
    /// The groups the user approved, compared again before each send.
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
    index: u32,
    doc_key: KeyID,
    response: Response,
    /// Set once the user asks to Disconnect this app so any automatic retry
    /// cannot deliver a decryption key while a refused Disconnect is being
    /// resolved. Cleared only if the user explicitly initiates sign-in retry;
    /// the entry is removed once Disconnect is submitted or completes.
    held_by_disconnect: bool,
}

impl PendingSends {
    fn put(&self, wallet: WalletId, send: PendingSend) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= MAX_PENDING {
            map.clear();
        }
        map.insert((wallet, send.identity, send.response.request_hash), send);
    }

    fn identity_for(
        &self,
        wallet: WalletId,
        request: [u8; 20],
        hint: Option<Identifier>,
    ) -> Option<Identifier> {
        let map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(id) = hint {
            return map.contains_key(&(wallet, id, request)).then_some(id);
        }
        let mut matches = map
            .keys()
            .filter(|(w, _, r)| *w == wallet && *r == request)
            .map(|(_, id, _)| *id);
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }

    fn take_for(
        &self,
        wallet: WalletId,
        identity: Identifier,
        request: [u8; 20],
    ) -> Option<PendingSend> {
        (self.0.lock().unwrap_or_else(PoisonError::into_inner)).remove(&(wallet, identity, request))
    }

    /// Marks any parked sign-in response for `(wallet, identity, request)` as
    /// held by Disconnect without deleting it, preventing automatic retries
    /// while preserving it for explicit user sign-in retry if Disconnect is
    /// refused before submission.
    fn hold_for(&self, wallet: WalletId, identity: Identifier, request: [u8; 20]) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(send) = map.get_mut(&(wallet, identity, request)) {
            send.held_by_disconnect = true;
        }
    }

    /// Takes a parked response for an automatic retry only when it is not held
    /// by Disconnect.
    #[cfg_attr(not(test), allow(dead_code))]
    fn take_auto_for(
        &self,
        wallet: WalletId,
        identity: Identifier,
        request: [u8; 20],
    ) -> Option<PendingSend> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (wallet, identity, request);
        if map.get(&key).is_some_and(|s| s.held_by_disconnect) {
            return None;
        }
        map.remove(&key)
    }

    #[cfg(test)]
    fn is_held(&self, wallet: WalletId, identity: Identifier, request: [u8; 20]) -> Option<bool> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(wallet, identity, request))
            .map(|s| s.held_by_disconnect)
    }
}

/// The identity of the wallet at its DIP-13 index, with a username.
struct Candidate {
    id: Identifier,
    index: u32,
    username: String,
    balance: Option<u64>,
}

fn candidates(session: &NetworkSession, wallet: WalletId) -> Result<Vec<Candidate>, PlatformError> {
    let summaries = session.identity_summaries(wallet)?;
    Ok(summaries
        .into_iter()
        .filter_map(|s| {
            let username = s.main_name.clone().or_else(|| s.names.first().cloned())?;
            Some(Candidate {
                id: Identifier::from_string(&s.identity, Encoding::Base58).ok()?,
                index: s.index,
                username,
                balance: s.balance,
            })
        })
        .collect())
}

/// The checks that need no read: the network, the flag, what this wallet
/// supports.
fn check_link(network: &DashNetwork, link: &ConnectUri) -> Result<(), ConnectError> {
    if !link.network.matches(network.core_network()) {
        return Err(ConnectError::Network);
    }
    if !enabled_on(network) {
        return Err(ConnectError::MainnetOff);
    }
    link.request
        .check_supported()
        .map_err(|_| ConnectError::Unsupported)?;
    Ok(())
}

/// What Platform says of the request as a whole: protocol, contracts and
/// groups, and the owners the DIP requires.
struct RequestFacts {
    app_owner: Identifier,
    contracts: BTreeMap<Identifier, Option<ContractFacts>>,
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
}

async fn request_facts(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
) -> Result<RequestFacts, ConnectError> {
    if net.protocol_version().await? < MIN_PROTOCOL_VERSION {
        return Err(ConnectError::Protocol);
    }
    let mut contracts = BTreeMap::new();
    for c in plan::named_contracts(request)
        .into_iter()
        .chain(request.enc_bindings.iter().map(|b| plan::id(&b.contract)))
    {
        if let std::collections::btree_map::Entry::Vacant(e) = contracts.entry(c) {
            e.insert(net.contract(c).await?);
        }
    }
    let mut groups = BTreeMap::new();
    for g in plan::named_groups(request) {
        groups.insert(g, net.group(g).await?);
    }
    plan::check_ownership(request, &contracts, &groups)?;
    let app_owner = (contracts.get(&plan::id(&request.app_id)).cloned().flatten())
        .ok_or(ConnectError::Invalid)?
        .owner;
    Ok(RequestFacts {
        app_owner,
        contracts,
        groups,
    })
}

fn scope(bounds: &Bounds, facts: &RequestFacts) -> ConnectScope {
    match bounds {
        b if plan::is_profile(b) => ConnectScope::Profile,
        Bounds::Contract(c) => ConnectScope::Contract {
            contract: b58(&plan::id(c)),
            tokens: (facts.contracts.get(&plan::id(c)).cloned().flatten())
                .is_none_or(|f| f.has_tokens || !f.readonly),
        },
        Bounds::DocumentType(c, name) => ConnectScope::DocumentType {
            contract: b58(&plan::id(c)),
            document_type: name.clone(),
        },
        Bounds::Group(g) => {
            let facts = (facts.groups)
                .get(&plan::id(g))
                .cloned()
                .flatten()
                .unwrap_or_default();
            ConnectScope::Group(ConnectGroup {
                group: b58(&plan::id(g)),
                owner: b58(&facts.owner),
                admins: facts.admins.iter().map(b58).collect(),
                contracts: facts.contracts.iter().map(b58).collect(),
                document_types: (facts.document_types.iter())
                    .map(|(c, t)| ConnectDocumentType {
                        contract: b58(c),
                        document_type: t.clone(),
                    })
                    .collect(),
                tokens: (facts.tokens.iter())
                    .map(|(c, p)| ConnectToken {
                        contract: b58(c),
                        position: *p,
                    })
                    .collect(),
            })
        }
    }
}

/// The request's age against the proved block time.
fn check_age(request: &AppConnectRequest, block_time_ms: u64) -> Result<(), ConnectError> {
    match request.age(block_time_ms) {
        dw_uri::connect::Age::Fresh => Ok(()),
        _ => Err(ConnectError::Stale),
    }
}

pub(crate) async fn sheet(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
) -> Result<ConnectSheet, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let mut identities = candidates(session, wallet)?;
    if let Some(hint) = request.identity_hint {
        if identities.is_empty() {
            return Err(ConnectError::NoIdentity);
        }
        identities.retain(|c| c.id == plan::id(&hint));
        if identities.is_empty() {
            return Err(ConnectError::Hint);
        }
    }
    let first = identities.first().ok_or(ConnectError::NoIdentity)?;
    let earlier_approval = (earlier_approval(net, session, wallet, request).await?).filter(|c| {
        parse_identity(&c.identity).is_ok_and(|id| identities.iter().any(|i| i.id == id))
    });
    let facts = request_facts(net, request).await?;
    let groups = plan::groups_digest(&facts.groups).ok_or(PlatformError::Internal {
        detail: "app connect groups".into(),
    })?;
    let (_, block_time_ms, _) = net.identity(first.id).await?;
    check_age(request, block_time_ms)?;
    let bindings = (request.auth_bindings.iter().enumerate())
        .map(|(i, b)| ConnectBinding {
            index: i as u8,
            primary: i == 0,
            level: match b.level {
                Level::Critical => ConnectLevel::Critical,
                Level::High => ConnectLevel::High,
            },
            scope: scope(&b.bounds, &facts),
        })
        .collect();
    Ok(ConnectSheet {
        request: hex::encode(request.request_hash()),
        app_id: b58(&plan::id(&request.app_id)),
        label: request.label.clone(),
        app_owner: b58(&facts.app_owner),
        identities: (identities.into_iter())
            .map(|c| ConnectIdentity {
                identity: b58(&c.id),
                username: c.username,
                balance: c.balance,
            })
            .collect(),
        bindings,
        encryption: !request.enc_bindings.is_empty(),
        terms: plan::default_terms(request, block_time_ms),
        max_funding_limit: MAX_FUNDING,
        earlier_approval,
        block_time_ms,
        groups,
    })
}

/// Everything an approval of `choice` rests on, read now.
struct Prepared {
    identity: Identity,
    index: u32,
    selection: Vec<u8>,
    terms: ConnectTerms,
    budgets: Vec<u64>,
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
    groups_digest: [u8; 32],
    encryption: Vec<EncPlan>,
    digest: [u8; 32],
}

/// The choice of this wallet's last submitted approval of `request`; one
/// whose response Platform holds answered it (`connect.answered`).
async fn earlier_approval(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    request: &AppConnectRequest,
) -> Result<Option<ConnectChoice>, ConnectError> {
    let hash = request.request_hash();
    let this = hex::encode(hash);
    let mut found = None;
    for r in records(session, wallet).await? {
        if r.request != this {
            continue;
        }
        if net
            .response_exists(hash, parse_identity(&r.identity)?)
            .await?
        {
            return Err(ConnectError::Answered);
        }
        if r.selection.is_empty() {
            continue;
        }
        found = Some(ConnectChoice {
            identity: r.identity,
            selection: r.selection,
            terms: ConnectTerms {
                funding_limit: r.funding_limit,
                expires_at_ms: r.expires_at_ms,
            },
        });
    }
    Ok(found)
}

/// An update error that proves nothing was sent. Any other, the library's
/// flattened broadcast failures included, may follow an accepted update,
/// so its keys may still land.
fn refused_unsent(e: &PlatformError) -> bool {
    matches!(
        e,
        PlatformError::SignerUnavailable
            | PlatformError::SeedMismatch
            | PlatformError::InsufficientCredits { .. }
            | PlatformError::InvalidArgument { .. }
            | PlatformError::GrantInvalid
            | PlatformError::GrantExceeded { .. }
            | PlatformError::NeedsGrant { .. }
            | PlatformError::Cancelled
            | PlatformError::LeaseRevoked { .. }
            | PlatformError::LeaseExpired
            | PlatformError::WalletNotFound
            | PlatformError::Identity(_)
    )
}

fn parse_identity(text: &str) -> Result<Identifier, ConnectError> {
    Identifier::from_string(text, Encoding::Base58).map_err(|_| {
        PlatformError::InvalidArgument {
            detail: "identity".into(),
        }
        .into()
    })
}

/// The identity's keys of `purpose` bound to `contract` alone, in any
/// state.
fn contract_keys(
    identity: &Identity,
    contract: Identifier,
    purpose: Purpose,
) -> impl Iterator<Item = &IdentityPublicKey> {
    use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
    identity.public_keys().values().filter(move |k| {
        k.purpose() == purpose
            && matches!(k.contract_bounds(), Some(ContractBounds::SingleContract { id }) if *id == contract)
    })
}

/// What each encryption binding gets. No app key goes on the DashPay contract,
/// and none is added or shared unless a lower-id key of its purpose comes
/// first: platform-wallet picks contact-request keys by purpose and id, not
/// by bounds.
async fn encryption_plans(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
    identity: &Identity,
) -> Result<Vec<EncPlan>, ConnectError> {
    let mut out = Vec::with_capacity(request.enc_bindings.len());
    for binding in &request.enc_bindings {
        let contract = plan::id(&binding.contract);
        let (purpose, _) = plan::enc_purpose(binding.purpose);
        let current = net
            .current_contract_key(identity.id(), contract, purpose)
            .await?;
        let current =
            current.and_then(|id| identity.public_keys().get(&id).filter(|k| !k.is_disabled()));
        out.push(match current {
            _ if contract == dashpay_contract::ID => EncPlan::Omit,
            Some(key)
                if key.key_type() == KeyType::ECDSA_SECP256K1 && picked_first(identity, key) =>
            {
                EncPlan::Share {
                    key_id: key.id(),
                    rotation: None,
                }
            }
            Some(_) => EncPlan::Omit,
            None if contract_keys(identity, contract, purpose).next().is_some()
                || missing_dashpay_purposes(identity.public_keys().values()).contains(&purpose) =>
            {
                EncPlan::Omit
            }
            None => EncPlan::Register,
        });
    }
    Ok(out)
}

/// Whether an enabled ECDSA key of `key`'s purpose has a lower id than `key`,
/// so platform-wallet's contact-request selectors take that one, not `key`.
fn picked_first(identity: &Identity, key: &IdentityPublicKey) -> bool {
    identity.public_keys().values().any(|k| {
        k.id() < key.id()
            && k.purpose() == key.purpose()
            && k.key_type() == KeyType::ECDSA_SECP256K1
            && !k.is_disabled()
    })
}

/// The doc key a response is signed with: an enabled ECDSA AUTHENTICATION
/// key at HIGH or CRITICAL with no bounds, budget or expiry, that `usable`.
fn response_key(identity: &Identity, usable: impl Fn(&IdentityPublicKey) -> bool) -> Option<KeyID> {
    identity
        .public_keys()
        .values()
        .find(|k| {
            k.purpose() == Purpose::AUTHENTICATION
                && matches!(
                    k.security_level(),
                    SecurityLevel::HIGH | SecurityLevel::CRITICAL
                )
                && k.key_type() == KeyType::ECDSA_SECP256K1
                && k.contract_bounds().is_none()
                && !k.has_limits()
                && !k.is_disabled()
                && usable(k)
        })
        .map(|k| k.id())
}

fn master_key(identity: &Identity) -> Option<&IdentityPublicKey> {
    identity.public_keys().values().find(|k| {
        k.purpose() == Purpose::AUTHENTICATION
            && k.security_level() == SecurityLevel::MASTER
            && k.key_type() == KeyType::ECDSA_SECP256K1
            && !k.is_disabled()
    })
}

async fn prepare(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
) -> Result<Prepared, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let id = parse_identity(&choice.identity)?;
    let candidate = (candidates(session, wallet)?.into_iter())
        .find(|c| c.id == id)
        .ok_or(ConnectError::NoIdentity)?;
    if request.identity_hint.is_some_and(|h| plan::id(&h) != id) {
        return Err(ConnectError::Hint);
    }
    let mut selection = choice.selection.clone();
    selection.sort_unstable();
    selection.dedup();
    if selection.first() != Some(&0)
        || selection
            .iter()
            .any(|&i| usize::from(i) >= request.auth_bindings.len())
    {
        return Err(PlatformError::InvalidArgument {
            detail: "selection".into(),
        }
        .into());
    }
    let facts = request_facts(net, request).await?;
    let (identity, block_time_ms, _) = net.identity(id).await?;
    let identity = identity.ok_or(ConnectError::NoIdentity)?;
    check_age(request, block_time_ms)?;
    if !plan::terms_allowed(&choice.terms, block_time_ms) {
        return Err(PlatformError::InvalidArgument {
            detail: "terms".into(),
        }
        .into());
    }
    if net.response_exists(request.request_hash(), id).await? {
        return Err(ConnectError::Answered);
    }
    let budgets = plan::key_budgets(request, &selection, choice.terms.funding_limit).ok_or(
        PlatformError::InvalidArgument {
            detail: "funding limit".into(),
        },
    )?;
    let encryption = encryption_plans(net, request, &identity).await?;
    if identity.balance() < CONNECT_FEE_CREDITS {
        return Err(ConnectError::Credits {
            needed: CONNECT_FEE_CREDITS,
            available: identity.balance(),
        });
    }
    let body = request.encode();
    let digest = Commitment {
        request: &body,
        identity: id,
        selection: &selection,
        terms: choice.terms,
        budgets: &budgets,
        app_owner: facts.app_owner,
        groups: &facts.groups,
        encryption: &encryption,
    }
    .digest()
    .ok_or(PlatformError::Internal {
        detail: "app connect plan".into(),
    })?;
    let groups_digest = plan::groups_digest(&facts.groups).ok_or(PlatformError::Internal {
        detail: "app connect groups".into(),
    })?;
    Ok(Prepared {
        identity,
        index: candidate.index,
        selection,
        terms: choice.terms,
        budgets,
        groups: facts.groups,
        groups_digest,
        encryption,
        digest,
    })
}

pub(crate) async fn quote(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
) -> Result<ConnectQuote, ConnectError> {
    let p = prepare(net, session, wallet, link, choice).await?;
    Ok(ConnectQuote {
        request: link.request.request_hash(),
        plan: p.digest,
        groups: p.groups_digest,
        max_credits: CONNECT_MAX_CREDITS,
        fee: CONNECT_FEE_CREDITS,
        budgets: p.budgets,
        balance: p.identity.balance(),
    })
}

/// Refuses a grant that is not this wallet's `AppConnect` grant for this
/// request and plan, or whose cap is below the quote, before it is
/// redeemed: a refusal leaves it for another try.
async fn check_grant(
    session: &NetworkSession,
    wallet: WalletId,
    grant: &str,
    request: [u8; 20],
    digest: [u8; 32],
) -> Result<(), ConnectError> {
    let (vault, g) = (session.vault.clone(), grant.to_owned());
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&g, GrantKind::AppConnect, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::from(EngineError::from(e)))?
    .map_err(PlatformError::from)?;
    let GrantPurpose::AppConnect {
        max_credits,
        request: granted,
        plan,
    } = purpose
    else {
        return Err(PlatformError::GrantInvalid.into());
    };
    if granted != request {
        return Err(PlatformError::GrantInvalid.into());
    }
    if plan != digest {
        return Err(ConnectError::PlanChanged);
    }
    if max_credits < CONNECT_MAX_CREDITS {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: CONNECT_MAX_CREDITS,
            remaining: max_credits,
        }
        .into());
    }
    Ok(())
}

/// The keys an approval adds and the slots it seals, in the DIP's order.
#[derive(Default)]
struct KeyPlan {
    add: Vec<IdentityPublicKey>,
    /// New encryption keys by id, for their proofs of possession.
    proofs: BTreeMap<KeyID, (AppKey, Vec<u8>)>,
    slots: Vec<AppKey>,
    session_ids: Vec<KeyID>,
    /// Hex-encoded `ECDSA_HASH160` data for every id in `session_ids`.
    session_data: BTreeMap<KeyID, String>,
    encryption_ids: Vec<KeyID>,
}

fn app_encryption(contract: &[u8; 32], purpose: EncPurpose, rotation: Option<u32>) -> AppKey {
    AppKey::Encryption {
        contract: *contract,
        purpose: plan::enc_purpose(purpose).1,
        rotation,
    }
}

/// A session key derived earlier that is on the identity: reused only when
/// it is enabled and equal to what this approval grants (DIP "Session
/// Keys"); else `connect.failed{key_conflict}`.
fn key_plan(
    keys: &AppKeys,
    p: &Prepared,
    request: &AppConnectRequest,
) -> Result<KeyPlan, ConnectError> {
    let owner = p.identity.id().to_buffer();
    let app = &request.app_eph_pub;
    let mut next = p.identity.public_keys().keys().max().map_or(0, |m| m + 1);
    let mut out = KeyPlan::default();
    for (&index, &budget) in p.selection.iter().zip(&p.budgets) {
        let key = AppKey::Session {
            binding_index: index,
        };
        let public = keys.public(owner, app, &key)?;
        let wanted =
            plan::session_key(request, index, next, &public, budget, p.terms.expires_at_ms)
                .ok_or(ConnectError::Invalid)?;
        let found = (p.identity.public_keys().values()).find(|k| k.data() == wanted.data());
        let data_hex = hex::encode(wanted.data().as_slice());
        match found {
            Some(k) => {
                let same = !k.is_disabled()
                    && k.purpose() == wanted.purpose()
                    && k.security_level() == wanted.security_level()
                    && k.key_type() == wanted.key_type()
                    && k.contract_bounds() == wanted.contract_bounds()
                    && k.total_budget() == wanted.total_budget()
                    && k.expires_at() == wanted.expires_at();
                if !same {
                    return Err(ConnectError::Failed {
                        failure: ConnectFailure::KeyConflict,
                    });
                }
                out.session_ids.push(k.id());
                out.session_data.insert(k.id(), data_hex);
            }
            None => {
                out.session_ids.push(next);
                out.session_data.insert(next, data_hex);
                out.add.push(wanted);
                next += 1;
            }
        }
        out.slots.push(key);
    }
    for (binding, enc) in request.enc_bindings.iter().zip(&p.encryption) {
        match *enc {
            EncPlan::Omit => {}
            EncPlan::Share { key_id, .. } => {
                let on_chain = p
                    .identity
                    .public_keys()
                    .get(&key_id)
                    .map(|k| k.data().to_vec());
                for rotation in [Some(0), None] {
                    let key = app_encryption(&binding.contract, binding.purpose, rotation);
                    let Ok(public) = keys.public(owner, app, &key) else {
                        continue;
                    };
                    if on_chain.as_deref() == Some(&public[..]) {
                        out.slots.push(key);
                        out.encryption_ids.push(key_id);
                        break;
                    }
                }
            }
            EncPlan::Register => {
                let key = app_encryption(&binding.contract, binding.purpose, Some(0));
                let public = keys.public(owner, app, &key)?;
                out.add.push(plan::encryption_key(
                    &binding.contract,
                    binding.purpose,
                    next,
                    &public,
                ));
                out.proofs.insert(next, (key, public.to_vec()));
                out.encryption_ids.push(next);
                out.slots.push(key);
                next += 1;
            }
        }
    }
    if out.add.len() > MAX_ADDED {
        return Err(ConnectError::Unsupported);
    }
    Ok(out)
}

/// The previous grant's session keys for the same identity, app and
/// connection id that are still enabled: disabled in the same update. Only
/// a confirmed record's ids are known to hold its keys; a submitted one's
/// may since belong to another key. The records are local, so each id must
/// also pass Disconnect's check against its own record
/// ([`check_disconnect_key`]): a record with no key data disables nothing.
fn superseded(
    records: &[ConnectRecord],
    identity: &Identity,
    request: &AppConnectRequest,
) -> Vec<KeyID> {
    let Some(connection) = request.connection_id.map(hex::encode) else {
        return Vec::new();
    };
    let (app, owner) = (b58(&plan::id(&request.app_id)), b58(&identity.id()));
    let this = hex::encode(request.request_hash());
    (records.iter())
        .filter(|r| {
            r.app_id == app
                && r.identity == owner
                && r.connection_id.as_deref() == Some(&connection)
                && r.request != this
                && r.state != RecordState::Submitted
        })
        .flat_map(|r| {
            (r.session_keys.iter())
                .filter_map(|id| identity.public_keys().get(id))
                .filter(move |k| !k.is_disabled() && check_disconnect_key(r, k).is_ok())
                .map(|k| k.id())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_DISABLED)
        .collect()
}

async fn records(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<Vec<ConnectRecord>, PlatformError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let raw = session
        .appdb_op(move |db| db.setting(&scope, RECORDS_SETTING))
        .await?;
    raw.map_or(Ok(Vec::new()), |r| {
        serde_json::from_str(&r).map_err(|_| PlatformError::Internal {
            detail: "app connect records".into(),
        })
    })
}

static RECORD_WRITE: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(Default::default);

/// Returns `Some(k)` when retired transition nonce `k` is NOT proven spent on
/// this `(nonce_state, id_height, nonce_height)` read, or `None` once
/// `id_height >= nonce_height && k <= tip && !takes(nonce_state, k, id)`.
fn reserved_nonce_on_retire(
    k: u64,
    nonce_state: u64,
    id_height: u64,
    nonce_height: u64,
    id: Identifier,
) -> Option<u64> {
    let tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
    let proven_spent =
        id_height >= nonce_height && k <= tip && !super::withdrawal::takes(nonce_state, k, id);
    if proven_spent { None } else { Some(k) }
}

/// Whether an unresolved Disconnect nonce on an identity belongs to a live
/// `pending_disconnect` (still able to modify keys/revision) or a retired
/// conflict reservation (stale-revision dead bytes that can only consume
/// their own nonce `k` via a paid bump).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisconnectNonceKind {
    Pending,
    Reserved {
        observed_tip: Option<u64>,
        observed_height: Option<u64>,
    },
}

/// All unresolved Disconnect nonces (both active `pending_disconnect` and
/// retired-conflict `last_conflict.nonce` reservations) and transition hashes
/// persisted for `(wallet, identity)`, used by cross-flow nonce ownership
/// checks in withdrawals and credit transfers so another flow never collides
/// with an unresolved or still-admissible dead Disconnect nonce.
pub(crate) async fn pending_disconnects_for_identity(
    session: &NetworkSession,
    wallet: WalletId,
    identity: Identifier,
) -> Result<Vec<(u64, String, DisconnectNonceKind)>, PlatformError> {
    let owner = b58(&identity);
    Ok(records(session, wallet)
        .await?
        .into_iter()
        .filter(|r| r.identity == owner)
        .flat_map(|r| {
            let pending_nonce = r.pending_disconnect.as_ref().map(|p| p.nonce);
            let pending = r
                .pending_disconnect
                .map(|p| (p.nonce, p.transition_hash, DisconnectNonceKind::Pending));
            let reserved = r.last_conflict.and_then(|c| {
                let observed_tip = c.observed_tip;
                let observed_height = c.observed_height;
                c.nonce.filter(|&n| pending_nonce != Some(n)).map(|n| {
                    (
                        n,
                        c.transition_hash,
                        DisconnectNonceKind::Reserved {
                            observed_tip,
                            observed_height,
                        },
                    )
                })
            });
            pending.into_iter().chain(reserved)
        })
        .collect())
}

/// Drops any persisted `last_conflict.nonce` reservation on `(wallet, identity)`
/// once a proved `nonce_state` shows `k <= tip && !takes(nonce_state, k, identity)`
/// (consumed or permanently outside the missing-nonce window), while keeping
/// `last_conflict` itself for earlier-charge disclosure. Raises the session
/// floor past each proven-spent `k` (`k <= tip`) so a subsequent lagging node
/// can never hand `k` back out to a transfer or withdrawal.
pub(crate) async fn clear_spent_conflict_nonces(
    session: &NetworkSession,
    wallet: WalletId,
    identity: Identifier,
    nonce_state: u64,
) {
    let tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
    let owner = b58(&identity);
    let _write = RECORD_WRITE.lock().await;
    let mut all = match records(session, wallet).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(
                error = %e.code(),
                "app connect read records before clearing conflict nonce"
            );
            return;
        }
    };
    let mut changed = false;
    let mut cleared = Vec::new();
    for r in &mut all {
        if r.identity != owner {
            continue;
        }
        if let Some(c) = &mut r.last_conflict
            && let Some(k) = c.nonce
            && k <= tip
            && !super::withdrawal::takes(nonce_state, k, identity)
        {
            c.nonce = None;
            c.observed_tip = None;
            c.observed_height = None;
            changed = true;
            cleared.push(k);
        }
    }
    if changed {
        for k in cleared {
            session.withdrawal_nonces.advance_floor(identity, k);
        }
        match serde_json::to_string(&all) {
            Ok(value) => {
                let scope = dw_appdb::local_scope(&wallet.to_string());
                if let Err(e) = session
                    .appdb_op(move |db| {
                        db.set_setting_durable(&scope, RECORDS_SETTING, Some(&value))
                    })
                    .await
                {
                    tracing::warn!(
                        error = ?e,
                        "app connect persist cleared conflict nonce"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "app connect serialize cleared conflict nonce"
                );
            }
        }
    }
}

async fn put_record(
    session: &NetworkSession,
    wallet: WalletId,
    record: ConnectRecord,
) -> Result<(), PlatformError> {
    let _write = RECORD_WRITE.lock().await;
    let mut all = records(session, wallet).await?;
    all.retain(|r| r.request != record.request || r.identity != record.identity);
    all.push(record);
    // Never evict a record whose keys may still be enabled or whose Disconnect
    // nonce is still unresolved; bound only settled Disconnected history to
    // MAX_RECORDS so an active app stays disconnectable and nonce reservations
    // are never lost.
    let disconnected = all
        .iter()
        .filter(|r| {
            r.state == RecordState::Disconnected
                && r.pending_disconnect.is_none()
                && r.last_conflict.as_ref().and_then(|c| c.nonce).is_none()
        })
        .count();
    let mut drop_disconnected = disconnected.saturating_sub(MAX_RECORDS);
    if drop_disconnected > 0 {
        all.retain(|r| {
            if r.state == RecordState::Disconnected
                && r.pending_disconnect.is_none()
                && r.last_conflict.as_ref().and_then(|c| c.nonce).is_none()
                && drop_disconnected > 0
            {
                drop_disconnected -= 1;
                false
            } else {
                true
            }
        });
    }
    let value = serde_json::to_string(&all).map_err(|e| PlatformError::Internal {
        detail: format!("app connect record: {e}"),
    })?;
    let scope = dw_appdb::local_scope(&wallet.to_string());
    session
        .appdb_op(move |db| db.set_setting_durable(&scope, RECORDS_SETTING, Some(&value)))
        .await?;
    Ok(())
}

/// Whether every key of `add` is on `identity` enabled, and every key of
/// `disable` is present on `identity` with `disabled_at` set (never treating
/// an absent key from a lagging read as disabled).
fn landed(identity: &Identity, add: &[IdentityPublicKey], disable: &[KeyID]) -> bool {
    let keys = identity.public_keys();
    add.iter().all(|k| {
        keys.get(&k.id())
            .is_some_and(|on| on.data() == k.data() && !on.is_disabled())
    }) && disable
        .iter()
        .all(|id| keys.get(id).is_some_and(|on| on.is_disabled()))
}

/// Polls Platform until every key in `disable` is present on `identity`,
/// matches `record.session_key_data`, and has `is_disabled() == true`.
async fn confirm_disabled(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    id: Identifier,
    record: &ConnectRecord,
    disable: &[KeyID],
) -> bool {
    for wait in CONFIRM_WAITS_MS {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        if net.refresh(wallet, id).await.is_ok_and(|(identity, _)| {
            disable.iter().all(|key_id| {
                identity
                    .public_keys()
                    .get(key_id)
                    .is_some_and(|on| on.is_disabled() && check_disconnect_key(record, on).is_ok())
            })
        }) {
            return true;
        }
    }
    false
}

/// The groups as Platform has them now, against the approval's.
async fn same_groups(
    net: &impl ConnectNet,
    approved: &BTreeMap<Identifier, Option<GroupFacts>>,
) -> Result<(), ConnectError> {
    for (id, facts) in approved {
        if net.group(*id).await? != *facts {
            return Err(ConnectError::PlanChanged);
        }
    }
    Ok(())
}

/// The encryption keys sealed are still the identity's current keys.
async fn same_encryption(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
    identity: Identifier,
    sealed: &[KeyID],
) -> Result<(), ConnectError> {
    let mut sealed = sealed.iter().peekable();
    for binding in &request.enc_bindings {
        let (purpose, _) = plan::enc_purpose(binding.purpose);
        let current = net
            .current_contract_key(identity, plan::id(&binding.contract), purpose)
            .await?;
        if let Some(current) = current {
            sealed.next_if_eq(&&current);
        }
    }
    if sealed.next().is_some() {
        return Err(ConnectError::PlanChanged);
    }
    Ok(())
}

pub(crate) async fn approve(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
    grant: String,
) -> Result<ConnectDone, ConnectError> {
    let request = &link.request;
    let request_hash = request.request_hash();
    check_link(&session.network, link)?;
    let id = parse_identity(&choice.identity)?;
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    // Never overwrite or drop an unconfirmed Disconnect for this (identity, request).
    let req_hex = hex::encode(request_hash);
    let owner_b58 = b58(&id);
    let all_records = records(session, wallet_id).await?;
    if all_records.iter().any(|r| {
        r.identity == owner_b58
            && r.request == req_hex
            && (r.pending_disconnect.is_some()
                || r.last_conflict.as_ref().and_then(|c| c.nonce).is_some())
    }) {
        return Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        });
    }
    // The update signs at `tip + 1` of this one read, so every check below
    // decides the nonce it signs at. It never takes a nonce an unresolved
    // transfer or withdrawal holds, a live pending Disconnect holds, or an
    // ahead-of-tip conflict reservation holds (r4712173000 / G1).
    let (nonce_state, nonce_height) = net.identity_nonce(id).await?;
    let tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
    match session.withdrawal_nonces.ownership(id, nonce_state) {
        super::withdrawal::NonceOwner::Free => {}
        super::withdrawal::NonceOwner::Pending => {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::PendingWrite,
            });
        }
        super::withdrawal::NonceOwner::Behind => {
            return Err(PlatformError::Unavailable.into());
        }
    }
    let nonce = tip + 1;
    let mut other_live_pendings: Vec<u64> = Vec::new();
    let mut other_reserved_nonces: Vec<(u64, Option<u64>, Option<u64>)> = Vec::new();
    for r in all_records
        .iter()
        .filter(|r| r.identity == owner_b58 && r.request != req_hex)
    {
        let pending_nonce = r.pending_disconnect.as_ref().map(|p| p.nonce);
        if let Some(n) = pending_nonce {
            other_live_pendings.push(n);
        }
        if let Some(c) = r.last_conflict.as_ref()
            && let Some(n) = c.nonce
            && pending_nonce != Some(n)
        {
            other_reserved_nonces.push((n, c.observed_tip, c.observed_height));
        }
    }
    if !other_live_pendings.is_empty() || !other_reserved_nonces.is_empty() {
        for d_nonce in other_live_pendings {
            if super::withdrawal::takes(nonce_state, d_nonce, id) {
                return Err(ConnectError::Failed {
                    failure: ConnectFailure::PendingWrite,
                });
            }
            if d_nonce > tip {
                return Err(PlatformError::Unavailable.into());
            }
        }
        let mut had_spent_reservation = false;
        for (reserved_k, observed_tip, observed_height) in other_reserved_nonces {
            if reserved_k > tip {
                if super::withdrawal::takes(nonce_state, reserved_k, id) {
                    return Err(ConnectError::Failed {
                        failure: ConnectFailure::PendingWrite,
                    });
                }
                return Err(PlatformError::Unavailable.into());
            }
            if super::withdrawal::takes(nonce_state, reserved_k, id) {
                if observed_tip.is_none_or(|obs| tip < obs)
                    || observed_height.is_none_or(|obs_h| nonce_height < obs_h)
                {
                    return Err(PlatformError::Unavailable.into());
                }
            } else {
                had_spent_reservation = true;
            }
        }
        if had_spent_reservation {
            clear_spent_conflict_nonces(session, wallet_id, id, nonce_state).await;
        }
    }
    let p = prepare(net, session, wallet_id, link, choice).await?;
    check_grant(session, wallet_id, &grant, request_hash, p.digest).await?;
    // Mandatory refresh before redeeming the grant (r4712172979): the update
    // signs at the refreshed identity's next revision, and a failed refresh
    // refuses with nothing signed and the grant unused.
    let current = net.refresh_cache(wallet, id).await?;
    let master = master_key(&p.identity).ok_or(PlatformError::from(
        super::errors::IdentityError::KeysMissing {
            purpose: super::identity::KeyPurpose::Authentication,
        },
    ))?;
    let held: Vec<HeldKey> = key_path(session.vault.network(), p.index, master.id())
        .map(|path| vec![(path, master.data().to_vec())])
        .unwrap_or_default();
    let lease = session
        .begin_lease_holding(wallet_id, FlowKind::AppConnect, &[grant], held)
        .await
        .map_err(PlatformError::from)?;
    if response_signer_key(&lease, &p.identity, p.index)?.is_none() {
        return Err(ConnectError::Failed {
            failure: ConnectFailure::NoResponseKey,
        });
    }
    let owner = p.identity.id().to_buffer();
    let update = write_permit(&lease, "app_connect_update", CONNECT_UPDATE_CREDITS)?;
    let keys = update.app_connect_keys().map_err(PlatformError::from)?;
    let plan = match key_plan(&keys, &p, request) {
        Ok(plan) => plan,
        Err(e) => {
            update.finish(false);
            return Err(e);
        }
    };
    let disable: Vec<KeyID> = superseded(&records(session, wallet_id).await?, &p.identity, request)
        .into_iter()
        .filter(|k| !plan.session_ids.contains(k))
        .collect();
    let mut record = ConnectRecord {
        request: hex::encode(request_hash),
        app_id: b58(&plan::id(&request.app_id)),
        label: request.label.clone(),
        identity: b58(&id),
        connection_id: request.connection_id.map(hex::encode),
        session_keys: plan.session_ids.clone(),
        encryption_keys: plan.encryption_ids.clone(),
        selection: p.selection.clone(),
        funding_limit: p.terms.funding_limit,
        expires_at_ms: p.terms.expires_at_ms,
        state: RecordState::Submitted,
        created_ms: super::balances::now_ms(),
        returns_to: link.callback.as_ref().map(|c| c.host().to_owned()),
        session_key_data: plan.session_data.clone(),
        pending_disconnect: None,
        response_delivery: ResponseDelivery::Unsent,
        last_conflict: None,
    };
    if plan.add.is_empty() && disable.is_empty() {
        update.finish(false);
    } else {
        if let Err(e) = same_groups(net, &p.groups).await {
            update.finish(false);
            return Err(e);
        }
        put_record(session, wallet_id, record.clone()).await?;
        let signer = ConnectUpdateSigner {
            master: update
                .identity_signer(&[p.index])
                .map_err(PlatformError::from)?
                .only_keys([master.id()]),
            app: keys,
            identity: owner,
            app_eph_pub: request.app_eph_pub,
            added: plan.proofs.clone(),
        };
        let written = leased(
            &lease,
            net.update_identity(
                wallet,
                &current,
                master.id(),
                plan.add.clone(),
                disable.clone(),
                nonce,
                &signer,
            ),
        )
        .await;
        drop(signer);
        update.finish(written.is_ok());
        if let Err(e) = &written {
            tracing::info!("app connect update: {}", e.code());
        }
        if !confirm(net, wallet, id, &plan.add, &disable).await {
            return Err(match written {
                Err(e) if refused_unsent(&e) => e.into(),
                _ => ConnectError::Failed {
                    failure: ConnectFailure::NotConfirmed,
                },
            });
        }
    }
    record.state = RecordState::Confirmed;
    put_record(session, wallet_id, record.clone()).await?;
    let response = {
        let seal = write_permit(&lease, "app_connect_seal", 0)?;
        let keys = seal.app_connect_keys().map_err(PlatformError::from)?;
        let sealed = keys.seal(owner, &request.app_eph_pub, &plan.slots);
        seal.finish(sealed.is_ok());
        sealed?
    };
    let fresh = net.refresh_cache(wallet, id).await?;
    let doc_key = response_signer_key(&lease, &fresh, p.index)?.ok_or(ConnectError::Failed {
        failure: ConnectFailure::NoResponseKey,
    })?;
    let pending = PendingSend {
        lease,
        identity: id,
        groups: p.groups.clone(),
        index: p.index,
        doc_key,
        response,
        held_by_disconnect: false,
    };
    // Persist BroadcastUnknown before broadcasting the response document so a
    // local timeout or crash leaves Disconnect aware that a response carrying
    // shared keys may still commit on Platform.
    record.response_delivery = ResponseDelivery::BroadcastUnknown;
    put_record(session, wallet_id, record.clone()).await?;
    let published = publish(net, wallet, &pending, request, &plan.encryption_ids).await;
    match published {
        Ok(()) => {
            record.state = RecordState::Published;
            record.response_delivery = ResponseDelivery::Delivered;
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after publish: {}", e.code());
            }
            Ok(ConnectDone {
                identity: b58(&id),
                keys_added: plan.add.iter().map(|k| k.id()).collect(),
            })
        }
        Err(ConnectError::Platform(e)) => {
            if refused_unsent(&e) {
                record.response_delivery = ResponseDelivery::Unsent;
                let _ = put_record(session, wallet_id, record).await;
            }
            Err(park(session, wallet_id, pending, &e))
        }
        Err(e) => {
            // ScopeChanged or another pre-broadcast check failed before publish.
            record.response_delivery = ResponseDelivery::Unsent;
            let _ = put_record(session, wallet_id, record).await;
            Err(e)
        }
    }
}

/// The key of `identity` the lease can sign the response with, if any.
fn response_signer_key(
    lease: &Lease,
    identity: &Identity,
    index: u32,
) -> Result<Option<KeyID>, PlatformError> {
    let probe = lease.call_permit("app_connect_probe", None)?;
    let found = probe
        .identity_signer(&[index])
        .map(|signer| response_key(identity, |k| signer.can_sign_with(k)));
    probe.finish(false);
    Ok(found?)
}

/// Keeps a response Platform did not take for "Retry sending".
fn park(
    session: &NetworkSession,
    wallet_id: WalletId,
    pending: PendingSend,
    e: &PlatformError,
) -> ConnectError {
    tracing::info!("app connect response: {}", e.code());
    session.platform.app_connect.put(wallet_id, pending);
    ConnectError::SendFailed
}

/// Reads the identity until every key of `add` is on it (three reads, 2 s
/// then 4 s apart).
async fn confirm(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    id: Identifier,
    add: &[IdentityPublicKey],
    disable: &[KeyID],
) -> bool {
    for wait in CONFIRM_WAITS_MS {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        if net
            .refresh_cache(wallet, id)
            .await
            .is_ok_and(|i| landed(&i, add, disable))
        {
            return true;
        }
    }
    false
}

/// Sends the sealed response once the approved groups and the sealed
/// encryption keys are still Platform's: a change after the keys were added
/// is `connect.failed{scope_changed}`.
async fn publish(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    pending: &PendingSend,
    request: &AppConnectRequest,
    encryption: &[KeyID],
) -> Result<(), ConnectError> {
    let scope_changed = |e| match e {
        ConnectError::PlanChanged => ConnectError::Failed {
            failure: ConnectFailure::ScopeChanged,
        },
        e => e,
    };
    same_groups(net, &pending.groups)
        .await
        .map_err(scope_changed)?;
    same_encryption(net, request, pending.identity, encryption)
        .await
        .map_err(scope_changed)?;
    let permit = write_permit(
        &pending.lease,
        "app_connect_response",
        CONNECT_RESPONSE_CREDITS,
    )?;
    let signer = permit
        .identity_signer(&[pending.index])
        .map_err(PlatformError::from)?
        .only_keys([pending.doc_key]);
    let written = leased(
        &pending.lease,
        net.publish(wallet, pending.identity, &pending.response, &signer),
    )
    .await;
    drop(signer);
    permit.finish(written.is_ok());
    Ok(written?)
}

pub(crate) async fn retry_send(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    link: &ConnectUri,
) -> Result<ConnectDone, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let nothing = || ConnectError::Failed {
        failure: ConnectFailure::NothingToSend,
    };
    let hint = request.identity_hint.map(Identifier::from);
    let identity = (session.platform.app_connect)
        .identity_for(wallet_id, request.request_hash(), hint)
        .ok_or_else(nothing)?;
    let _refresh = session.after_spend(wallet_id, identity);
    let locks = identity_locks(wallet_id, identity);
    let _write = locks.write.lock().await;
    // Take ownership of the parked response ONLY for this locked identity so
    // a concurrent `disconnect` for `identity` serializes atomically and
    // another identity's parked response for the same request is never touched.
    // Because this entry point is an explicit user-initiated sign-in retry, it
    // clears any Disconnect hold (while still refusing below if Disconnect was
    // submitted or keys are already disabled).
    let mut pending = (session.platform.app_connect)
        .take_for(wallet_id, identity, request.request_hash())
        .ok_or_else(nothing)?;
    pending.held_by_disconnect = false;
    let sent = async {
        let mut record = (records(session, wallet_id).await?.into_iter())
            .find(|r| {
                r.request == hex::encode(request.request_hash())
                    && r.identity == b58(&pending.identity)
            })
            .ok_or_else(nothing)?;
        if record.state == RecordState::Disconnected || record.pending_disconnect.is_some() {
            return Err(nothing());
        }
        // Re-check Platform under the identity write lock before publishing or
        // marking Published: drop the parked response only when a session key
        // is definitively disabled or its public data mismatches; if a lagging
        // node omits a key, return Unavailable so the parked response is kept.
        let (on_chain, _, _) = net.identity(pending.identity).await?;
        let on_chain = on_chain.ok_or(PlatformError::Unavailable)?;
        if record.session_keys.is_empty() {
            return Err(nothing());
        }
        for key_id in &record.session_keys {
            let Some(k) = on_chain.public_keys().get(key_id) else {
                return Err(PlatformError::Unavailable.into());
            };
            if k.is_disabled()
                || record
                    .session_key_data
                    .get(key_id)
                    .is_some_and(|h| hex::encode(k.data().as_slice()) != *h)
            {
                return Err(nothing());
            }
        }
        if !net
            .response_exists(request.request_hash(), pending.identity)
            .await?
        {
            let prev_delivery = record.response_delivery;
            record.response_delivery = ResponseDelivery::BroadcastUnknown;
            put_record(session, wallet_id, record.clone()).await?;
            if let Err(e) = publish(net, wallet, &pending, request, &record.encryption_keys).await {
                if matches!(&e, ConnectError::Platform(pe) if refused_unsent(pe))
                    || !matches!(&e, ConnectError::Platform(_))
                {
                    record.response_delivery = prev_delivery;
                    let _ = put_record(session, wallet_id, record).await;
                }
                return Err(e);
            }
        }
        Ok(record)
    }
    .await;
    match sent {
        Ok(mut record) => {
            record.state = RecordState::Published;
            record.response_delivery = ResponseDelivery::Delivered;
            if let Err(e) = put_record(session, wallet_id, record.clone()).await {
                tracing::warn!("app connect record after publish: {}", e.code());
            }
            Ok(ConnectDone {
                identity: record.identity,
                keys_added: Vec::new(),
            })
        }
        Err(ConnectError::Platform(
            PlatformError::LeaseRevoked { .. }
            | PlatformError::LeaseExpired
            | PlatformError::Cancelled
            | PlatformError::NeedsGrant { .. }
            | PlatformError::GrantExceeded { .. }
            | PlatformError::GrantInvalid,
        )) => Err(nothing()),
        Err(ConnectError::Platform(e)) => Err(park(session, wallet_id, pending, &e)),
        Err(e) => Err(e),
    }
}

fn check_enabled(network: &DashNetwork) -> Result<(), ConnectError> {
    if !enabled_on(network) {
        return Err(ConnectError::MainnetOff);
    }
    Ok(())
}

fn parse_request_hash(text: &str) -> Result<[u8; 20], ConnectError> {
    let invalid = || {
        ConnectError::from(PlatformError::InvalidArgument {
            detail: "request".into(),
        })
    };
    let bytes = hex::decode(text).map_err(|_| invalid())?;
    if bytes.len() != 20 || text != hex::encode(&bytes) {
        return Err(invalid());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn wallet_identity_index(
    session: &NetworkSession,
    wallet: WalletId,
    id: Identifier,
) -> Result<u32, ConnectError> {
    let target = b58(&id);
    let summaries = session
        .identity_summaries(wallet)
        .map_err(PlatformError::from)?;
    summaries
        .into_iter()
        .find(|s| s.identity == target)
        .map(|s| s.index)
        .ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound).into())
}

fn is_forbidden_dashpay_bounds(bounds: &ContractBounds) -> bool {
    match bounds {
        ContractBounds::SingleContract { id } => *id == dashpay_contract::ID,
        ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        } => *id == dashpay_contract::ID && document_type_name != PROFILE_TYPE,
        ContractBounds::ContractGroup { .. } => false,
    }
}

/// Verifies that `key` on Platform is an app-added AUTHENTICATION session
/// key belonging to `record` (whether it is still enabled is the caller's
/// check):
/// - its key id is in `record.session_keys`;
/// - `record.session_key_data` holds the expected key digest AND matches
///   `key.data()` (older grants without key data are refused, never matched
///   by purpose/level alone);
/// - `key.purpose() == Purpose::AUTHENTICATION`;
/// - `key.security_level()` is `HIGH` or `CRITICAL` (never `MASTER`);
/// - `key.key_type() == KeyType::ECDSA_HASH160`;
/// - `key.contract_bounds()` is `Some(...)` (never unbounded, including
///   unbounded `CRITICAL`);
/// - `key.has_limits()`;
/// - not a whole-contract DashPay key (only `dashpay/profile` doc-type is
///   allowed on the DashPay contract).
fn check_disconnect_key(
    record: &ConnectRecord,
    key: &IdentityPublicKey,
) -> Result<(), ConnectError> {
    let unverified = || ConnectError::Failed {
        failure: ConnectFailure::UnverifiedKey,
    };
    if !record.session_keys.contains(&key.id()) {
        return Err(unverified());
    }
    let Some(expected_hex) = record.session_key_data.get(&key.id()) else {
        return Err(unverified());
    };
    let Ok(expected_bytes) = hex::decode(expected_hex) else {
        return Err(unverified());
    };
    if expected_bytes.is_empty() || key.data().as_slice() != expected_bytes.as_slice() {
        return Err(unverified());
    }
    if key.purpose() != Purpose::AUTHENTICATION {
        return Err(unverified());
    }
    if !matches!(
        key.security_level(),
        SecurityLevel::HIGH | SecurityLevel::CRITICAL
    ) {
        return Err(unverified());
    }
    if key.key_type() != KeyType::ECDSA_HASH160 {
        return Err(unverified());
    }
    let Some(bounds) = key.contract_bounds() else {
        return Err(unverified());
    };
    if !key.has_limits() {
        return Err(unverified());
    }
    if is_forbidden_dashpay_bounds(bounds) {
        return Err(unverified());
    }
    Ok(())
}

/// The verified still-enabled session key ids of `record` on `identity`.
/// Every key id in `record.session_keys` must be present on `identity` and
/// match `record.session_key_data` and app-key criteria:
/// - an absent key id (e.g., from a lagging pre-approval snapshot) is treated
///   as inconclusive (`PlatformError::Unavailable`), never as disabled;
/// - only a present, matching key with `is_disabled() == true` is counted as
///   already revoked;
/// - any key failing verification refuses the whole disconnect rather than
///   disabling partially or guessing.
struct VerifiedDisconnectKeys {
    /// Present, verified, still-enabled session keys eligible to disable.
    to_disable: Vec<KeyID>,
    /// `true` when at least one recorded key id was absent from the identity
    /// snapshot (for example a lagging pre-approval read or an unlanded
    /// re-approval's predicted key id).
    has_absent: bool,
}

/// Returns the still-enabled session keys of `record` on `identity` after
/// verifying every present key belongs to this app's grant.
///
/// - Any present key that fails `check_disconnect_key` refuses the whole
///   Disconnect with `UnverifiedKey`.
/// - If NO key is present & enabled AND at least one key is absent (for
///   example a lagging pre-approval snapshot where newly added keys are not
///   visible yet), returns `PlatformError::Unavailable` so Disconnect never
///   treats absent keys as disabled.
/// - If some keys are present & verified while another predicted id (e.g. from
///   an unlanded re-approval) is absent, returns the present verified keys so
///   the user can still disable them, with `has_absent = true` so the record
///   is NOT marked `Disconnected` until every id is present and disabled.
fn verified_disconnect_keys(
    record: &ConnectRecord,
    identity: &Identity,
) -> Result<VerifiedDisconnectKeys, ConnectError> {
    if record.session_keys.is_empty() {
        return Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        });
    }
    let mut to_disable = Vec::new();
    let mut has_absent = false;
    let mut seen = BTreeSet::new();
    for &key_id in &record.session_keys {
        if !seen.insert(key_id) {
            continue;
        }
        let Some(key) = identity.public_keys().get(&key_id) else {
            has_absent = true;
            continue;
        };
        check_disconnect_key(record, key)?;
        if key.is_disabled() {
            continue;
        }
        to_disable.push(key_id);
    }
    if to_disable.is_empty() && has_absent {
        return Err(PlatformError::Unavailable.into());
    }
    if to_disable.len() > MAX_DISABLED {
        return Err(ConnectError::Unsupported);
    }
    Ok(VerifiedDisconnectKeys {
        to_disable,
        has_absent,
    })
}

/// Resolves each concrete key's bound contract/group metadata size (fetching
/// ONLY `ContractGroupInfo` metadata for group-bound keys, never the full
/// member list) and returns the raw Platform-grounded fee upper bound without
/// clamping or capping against `DISCONNECT_FEE_BOUND`.
async fn live_disconnect_fee_raw(
    net: &impl ConnectNet,
    identity: &Identity,
    key_ids: &[KeyID],
) -> Result<u64, ConnectError> {
    if key_ids.is_empty() {
        return Ok(0);
    }
    let mut contracts: BTreeMap<Identifier, Option<ContractFacts>> = BTreeMap::new();
    let mut group_infos: BTreeMap<Identifier, Option<GroupInfoFacts>> = BTreeMap::new();
    let mut specs = Vec::with_capacity(key_ids.len());
    for &key_id in key_ids {
        let key = identity
            .public_keys()
            .get(&key_id)
            .ok_or(PlatformError::Unavailable)?;
        let bounds = key.contract_bounds().ok_or(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })?;
        match bounds {
            ContractBounds::SingleContract { id }
            | ContractBounds::SingleContractDocumentType { id, .. } => {
                if let std::collections::btree_map::Entry::Vacant(e) = contracts.entry(*id) {
                    e.insert(net.contract(*id).await?);
                }
                let facts = contracts
                    .get(id)
                    .and_then(|c| c.as_ref())
                    .ok_or(PlatformError::Unavailable)?;
                specs.push(DisconnectKeyFeeSpec::Contract {
                    serialized_bytes: facts.serialized_bytes,
                    readonly: facts.readonly,
                });
            }
            ContractBounds::ContractGroup { id } => {
                if let std::collections::btree_map::Entry::Vacant(e) = group_infos.entry(*id) {
                    e.insert(net.group_info(*id).await?);
                }
                let facts = group_infos
                    .get(id)
                    .and_then(|g| g.as_ref())
                    .ok_or(PlatformError::Unavailable)?;
                specs.push(DisconnectKeyFeeSpec::Group {
                    serialized_bytes: facts.serialized_bytes,
                });
            }
        }
    }
    Ok(net.disconnect_fee(&specs).await?)
}

/// Selects the largest non-empty prefix of `key_ids` whose live fee fits within
/// `DISCONNECT_FEE_BOUND`:
/// - when all `key_ids` fit, returns all of them in a single transition;
/// - when multiple keys together exceed `DISCONNECT_FEE_BOUND`, splits into a
///   sub-bound batch so a large app cannot make Disconnect impossible;
/// - when even a single key exceeds `DISCONNECT_FEE_BOUND`, refuses with
///   `FeeTooHigh` (never clamps).
async fn select_disconnect_batch(
    net: &impl ConnectNet,
    identity: &Identity,
    key_ids: &[KeyID],
) -> Result<(Vec<KeyID>, u64), ConnectError> {
    if key_ids.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let full_fee = live_disconnect_fee_raw(net, identity, key_ids).await?;
    if full_fee <= DISCONNECT_FEE_BOUND {
        return Ok((key_ids.to_vec(), full_fee));
    }
    if key_ids.len() > 1 {
        for count in (1..key_ids.len()).rev() {
            let batch = &key_ids[..count];
            let fee = live_disconnect_fee_raw(net, identity, batch).await?;
            if fee <= DISCONNECT_FEE_BOUND {
                return Ok((batch.to_vec(), fee));
            }
        }
    }
    Err(ConnectError::Failed {
        failure: ConnectFailure::FeeTooHigh,
    })
}

/// Reads the identity nonce (with proved height) and refreshes the identity
/// (with proved height), retrying the identity read up to
/// `CONSISTENT_READ_RETRIES` times until `id_height >= nonce_height`.
async fn read_consistent_state(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    id: Identifier,
) -> Result<(Identity, u64, u64, u64), PlatformError> {
    let (nonce_state, nonce_height) = net.identity_nonce(id).await?;
    let (mut identity, mut id_height) = net.refresh(wallet, id).await?;
    for _ in 0..CONSISTENT_READ_RETRIES {
        if id_height >= nonce_height {
            break;
        }
        let (next_id, next_height) = net.refresh(wallet, id).await?;
        identity = next_id;
        id_height = next_height;
    }
    Ok((identity, id_height, nonce_state, nonce_height))
}

/// Reconciles any in-flight sign-in response document before Disconnect claims
/// completion. Returns `true` when an earlier sign-in response had an unknown
/// broadcast outcome (whether it has now been observed on Platform or may still
/// commit later), so the UI can warn the user that a key may already have
/// reached the app.
async fn reconcile_response_delivery(
    net: &impl ConnectNet,
    record: &mut ConnectRecord,
    request_hash: [u8; 20],
    id: Identifier,
) -> bool {
    if record.response_delivery != ResponseDelivery::BroadcastUnknown {
        return false;
    }
    if net.response_exists(request_hash, id).await.unwrap_or(false) {
        record.response_delivery = ResponseDelivery::Delivered;
    }
    true
}

/// Returns `true` when every key in `keys` is present on `identity`, passes
/// `check_disconnect_key`, and is disabled.
fn all_present_and_disabled(identity: &Identity, record: &ConnectRecord, keys: &[KeyID]) -> bool {
    !keys.is_empty()
        && keys.iter().all(|key_id| {
            identity
                .public_keys()
                .get(key_id)
                .is_some_and(|on| on.is_disabled() && check_disconnect_key(record, on).is_ok())
        })
}

/// Refuses a grant that is not this wallet's `AppDisconnect` grant for
/// `(identity, request)`, returning its `max_credits` cap before redemption.
async fn check_disconnect_grant(
    session: &NetworkSession,
    wallet: WalletId,
    grant: &str,
    identity: Identifier,
    request: [u8; 20],
) -> Result<u64, ConnectError> {
    let (vault, g) = (session.vault.clone(), grant.to_owned());
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&g, GrantKind::AppDisconnect, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::from(EngineError::from(e)))?
    .map_err(PlatformError::from)?;
    let GrantPurpose::AppDisconnect {
        max_credits,
        identity: granted_identity,
        request: granted_request,
    } = purpose
    else {
        return Err(PlatformError::GrantInvalid.into());
    };
    if granted_identity != identity.to_buffer() || granted_request != request {
        return Err(PlatformError::GrantInvalid.into());
    }
    Ok(max_credits)
}

async fn resolve_key_scope(
    net: &impl ConnectNet,
    bounds: Option<&ContractBounds>,
    fallback_app_id: &str,
    contracts: &mut BTreeMap<Identifier, Option<ContractFacts>>,
    groups: &mut BTreeMap<Identifier, Option<GroupFacts>>,
) -> Result<ConnectScope, ConnectError> {
    match bounds {
        Some(ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        }) if *id == dashpay_contract::ID && document_type_name == PROFILE_TYPE => {
            Ok(ConnectScope::Profile)
        }
        Some(ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        }) => Ok(ConnectScope::DocumentType {
            contract: b58(id),
            document_type: document_type_name.clone(),
        }),
        Some(ContractBounds::SingleContract { id }) => {
            if let std::collections::btree_map::Entry::Vacant(e) = contracts.entry(*id) {
                match net.contract(*id).await {
                    Ok(facts) => {
                        e.insert(facts);
                    }
                    Err(err) => {
                        tracing::warn!(
                            contract = %b58(id),
                            code = %err.code(),
                            "app connect list: contract display lookup failed; continuing so Disconnect stays reachable"
                        );
                        e.insert(None);
                    }
                }
            }
            let tokens = contracts
                .get(id)
                .cloned()
                .flatten()
                .is_none_or(|f| f.has_tokens || !f.readonly);
            Ok(ConnectScope::Contract {
                contract: b58(id),
                tokens,
            })
        }
        Some(ContractBounds::ContractGroup { id }) => {
            if let std::collections::btree_map::Entry::Vacant(e) = groups.entry(*id) {
                match net.group_all(*id).await {
                    Ok(facts) => {
                        e.insert(facts);
                    }
                    Err(err) => {
                        tracing::warn!(
                            group = %b58(id),
                            code = %err.code(),
                            "app connect list: group member lookup failed; falling back to group metadata so Disconnect stays reachable"
                        );
                        let fallback =
                            net.group_info(*id)
                                .await
                                .ok()
                                .flatten()
                                .map(|info| GroupFacts {
                                    owner: info.owner,
                                    admins: info.admins,
                                    ..GroupFacts::default()
                                });
                        e.insert(fallback);
                    }
                }
            }
            let facts = groups.get(id).cloned().flatten().unwrap_or_default();
            Ok(ConnectScope::Group(ConnectGroup {
                group: b58(id),
                owner: b58(&facts.owner),
                admins: facts.admins.iter().map(b58).collect(),
                contracts: facts.contracts.iter().map(b58).collect(),
                document_types: facts
                    .document_types
                    .iter()
                    .map(|(c, t)| ConnectDocumentType {
                        contract: b58(c),
                        document_type: t.clone(),
                    })
                    .collect(),
                tokens: facts
                    .tokens
                    .iter()
                    .map(|(c, p)| ConnectToken {
                        contract: b58(c),
                        position: *p,
                    })
                    .collect(),
            }))
        }
        None => Ok(ConnectScope::Contract {
            contract: fallback_app_id.to_owned(),
            tokens: true,
        }),
    }
}

pub(crate) async fn list(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    identity: &str,
) -> Result<ConnectedApps, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let _index = wallet_identity_index(session, wallet, id)?;
    let owner = b58(&id);
    let mut recs: Vec<ConnectRecord> = records(session, wallet)
        .await?
        .into_iter()
        .filter(|r| r.identity == owner && r.state != RecordState::Disconnected)
        .collect();
    recs.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
    if recs.is_empty() {
        return Ok(ConnectedApps {
            identity: owner,
            apps: Vec::new(),
            block_time_ms: 0,
        });
    }
    let (on_chain, block_time_ms, _) = net.identity(id).await?;
    let on_chain =
        on_chain.ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound))?;
    let mut contracts = BTreeMap::new();
    let mut groups = BTreeMap::new();
    let mut apps = Vec::new();
    for r in recs {
        // An older record with no key digests whose update never confirmed may
        // name ids another key later took: skip it.
        if r.state == RecordState::Submitted && r.session_key_data.is_empty() {
            continue;
        }
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        for &key_id in &r.session_keys {
            if !seen.insert(key_id) {
                continue;
            }
            let Some(k) = on_chain.public_keys().get(&key_id) else {
                continue;
            };
            if k.is_disabled() {
                continue;
            }
            if let Some(expected_hex) = r.session_key_data.get(&key_id)
                && hex::encode(k.data().as_slice()) != *expected_hex
            {
                continue;
            }
            let level = match k.security_level() {
                SecurityLevel::CRITICAL => ConnectLevel::Critical,
                _ => ConnectLevel::High,
            };
            let scope = resolve_key_scope(
                net,
                k.contract_bounds(),
                &r.app_id,
                &mut contracts,
                &mut groups,
            )
            .await?;
            keys.push(ConnectedAppKey {
                key_id,
                level,
                scope,
                budget: k.total_budget(),
                expires_at_ms: k.expires_at(),
            });
        }
        if keys.is_empty() {
            continue;
        }
        // Use the proved on-chain budgets of the still-enabled keys (matching
        // what the sign-in quote wrote to Platform, e.g. PROFILE_SHARE for a
        // profile-only approval) rather than the originally requested limit.
        let funding_limit = if keys.iter().any(|k| k.budget.is_some()) {
            keys.iter()
                .filter_map(|k| k.budget)
                .fold(0u64, u64::saturating_add)
        } else {
            r.funding_limit
        };
        // Access ends only once every enabled key has expired; if any enabled
        // key has no expiry on Platform, the connection has no end date.
        let expires_at_ms = if keys.iter().all(|k| k.expires_at_ms.is_some()) {
            keys.iter().filter_map(|k| k.expires_at_ms).max()
        } else {
            None
        };
        apps.push(ConnectedApp {
            request: r.request,
            app_id: r.app_id,
            label: r.label,
            returns_to: r.returns_to,
            identity: r.identity,
            keys,
            funding_limit,
            expires_at_ms,
            approved_at_ms: r.created_ms,
        });
    }
    Ok(ConnectedApps {
        identity: owner,
        apps,
        block_time_ms,
    })
}

pub(crate) async fn disconnect_quote(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    identity: &str,
    request: &str,
) -> Result<DisconnectQuote, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let request_hash = parse_request_hash(request)?;
    let _index = wallet_identity_index(session, wallet, id)?;
    let owner = b58(&id);
    let record = records(session, wallet)
        .await?
        .into_iter()
        .find(|r| r.identity == owner && r.request == request)
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "unknown app connection".into(),
        })?;
    let (on_chain, _, _) = net.identity(id).await?;
    let on_chain =
        on_chain.ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound))?;
    let verified = verified_disconnect_keys(&record, &on_chain)?;
    let (keys, fee) = if let Some(pending) = &record.pending_disconnect
        && !pending.keys.is_empty()
        && pending.keys.iter().all(|k| verified.to_disable.contains(k))
    {
        let fee = live_disconnect_fee_raw(net, &on_chain, &pending.keys).await?;
        if fee > DISCONNECT_FEE_BOUND {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::PendingFeeTooHigh,
            });
        }
        (pending.keys.clone(), fee)
    } else {
        select_disconnect_batch(net, &on_chain, &verified.to_disable).await?
    };
    if on_chain.balance() < fee {
        return Err(ConnectError::Credits {
            needed: fee,
            available: on_chain.balance(),
        });
    }
    Ok(DisconnectQuote {
        request: request_hash,
        identity_id: id.to_buffer(),
        identity: owner,
        keys,
        max_credits: fee,
        fee,
        balance: on_chain.balance(),
        prior_attempt_conflict: record.last_conflict.is_some(),
        prior_attempt_debit: record.last_conflict.as_ref().and_then(|c| c.observed_debit),
    })
}

pub(crate) async fn disconnect(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    identity: &str,
    request: &str,
    grant: String,
) -> Result<DisconnectDone, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let request_hash = parse_request_hash(request)?;
    let index = wallet_identity_index(session, wallet_id, id)?;
    let owner = b58(&id);
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    let all_records = records(session, wallet_id).await?;
    let mut record = all_records
        .iter()
        .find(|r| r.identity == owner && r.request == request)
        .cloned()
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "unknown app connection".into(),
        })?;
    let mut other_live_pendings: Vec<u64> = Vec::new();
    let mut other_reserved_nonces: Vec<(u64, Option<u64>, Option<u64>)> = Vec::new();
    for r in all_records
        .iter()
        .filter(|r| r.identity == owner && r.request != request)
    {
        let pending_nonce = r.pending_disconnect.as_ref().map(|p| p.nonce);
        if let Some(n) = pending_nonce {
            other_live_pendings.push(n);
        }
        if let Some(c) = r.last_conflict.as_ref()
            && let Some(n) = c.nonce
            && pending_nonce != Some(n)
        {
            other_reserved_nonces.push((n, c.observed_tip, c.observed_height));
        }
    }
    let granted_max_credits =
        check_disconnect_grant(session, wallet_id, &grant, id, request_hash).await?;
    // Hold (do not cancel yet) any parked login response for THIS identity
    // under its write lock once Disconnect is authorized: any automatic retry
    // is blocked immediately, while a refused Disconnect keeps the response
    // intact for explicit user sign-in retry. It is removed only once
    // Disconnect is actually submitted or confirmed disabled.
    session
        .platform
        .app_connect
        .hold_for(wallet_id, id, request_hash);

    let (fresh, id_height, nonce_state, nonce_height) =
        read_consistent_state(net, wallet, id).await?;
    if id_height >= nonce_height {
        clear_spent_conflict_nonces(session, wallet_id, id, nonce_state).await;
        let tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
        if let Some(c) = &mut record.last_conflict
            && let Some(k) = c.nonce
            && k <= tip
            && !super::withdrawal::takes(nonce_state, k, id)
        {
            c.nonce = None;
            c.observed_tip = None;
            c.observed_height = None;
            session.withdrawal_nonces.advance_floor(id, k);
        }
    }
    let verified = verified_disconnect_keys(&record, &fresh)?;
    if verified.to_disable.is_empty() {
        let _ = session
            .platform
            .app_connect
            .take_for(wallet_id, id, request_hash);
        if let Some(p) = &record.pending_disconnect {
            session.withdrawal_nonces.advance_floor(id, p.nonce);
        }
        let response_may_have_landed =
            reconcile_response_delivery(net, &mut record, request_hash, id).await;
        let has_unresolved_conflict_nonce = record
            .last_conflict
            .as_ref()
            .and_then(|c| c.nonce)
            .is_some();
        if record.state != RecordState::Disconnected
            || record.pending_disconnect.is_some()
            || (record.last_conflict.is_some() && !has_unresolved_conflict_nonce)
            || response_may_have_landed
        {
            if !verified.has_absent {
                record.state = RecordState::Disconnected;
            }
            record.pending_disconnect = None;
            if !has_unresolved_conflict_nonce {
                record.last_conflict = None;
            }
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after disconnect: {}", e.code());
            }
        }
        return Ok(DisconnectDone {
            identity: owner,
            keys_disabled: Vec::new(),
            response_may_have_landed,
        });
    }

    // If a prior unconfirmed disconnect signed a transition, reconcile or
    // re-broadcast that exact signed transition (same revision & nonce) —
    // never automatically minting a second paid transition.
    if let Some(pending) = record.pending_disconnect.clone() {
        // A Disconnect was already submitted for this app: cancel any parked
        // local sign-in retry immediately under the identity write lock.
        let _ = session
            .platform
            .app_connect
            .take_for(wallet_id, id, request_hash);
        // 1. Confirmation-only recovery always works without a fee check.
        if all_present_and_disabled(&fresh, &record, &pending.keys) {
            session.withdrawal_nonces.advance_floor(id, pending.nonce);
            let response_may_have_landed =
                reconcile_response_delivery(net, &mut record, request_hash, id).await;
            record.pending_disconnect = None;
            record.last_conflict = None;
            if !verified.has_absent && verified.to_disable.is_empty() {
                record.state = RecordState::Disconnected;
            }
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after disconnect: {}", e.code());
            }
            return Ok(DisconnectDone {
                identity: owner,
                keys_disabled: pending.keys,
                response_may_have_landed,
            });
        }

        // 2. Revision advanced while keys stayed enabled: the pending
        //    transition was superseded or suffered a paid revision conflict.
        //    Retire it and require a new user-initiated, re-quoted Disconnect
        //    instead of automatically signing a second paid transition.
        if fresh.revision() >= pending.revision {
            let observed_debit = pending
                .balance_before
                .and_then(|before| before.checked_sub(fresh.balance()))
                .filter(|&delta| delta > 0);
            tracing::warn!(
                transition_hash = %pending.transition_hash,
                pending_revision = pending.revision,
                current_revision = fresh.revision(),
                observed_debit = ?observed_debit,
                "app connect disconnect retired after revision conflict"
            );
            let reserved =
                reserved_nonce_on_retire(pending.nonce, nonce_state, id_height, nonce_height, id);
            let observed_tip = reserved.map(|_| nonce_state & IDENTITY_NONCE_VALUE_FILTER);
            let observed_height = reserved.map(|_| id_height.max(nonce_height));
            if reserved.is_none() {
                session.withdrawal_nonces.advance_floor(id, pending.nonce);
            }
            record.pending_disconnect = None;
            record.last_conflict = Some(DisconnectConflictInfo {
                transition_hash: pending.transition_hash,
                observed_debit,
                nonce: reserved,
                observed_tip,
                observed_height,
            });
            let _ = put_record(session, wallet_id, record).await;
            session.vault.revoke_grant(&grant);
            return Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            });
        }

        // 3. Revision < pending.revision: check whether the signed nonce is
        //    still admissible.
        let nonce_available = super::withdrawal::takes(nonce_state, pending.nonce, id);
        if !nonce_available {
            // Only retire the pending transition when the proved nonce tip is
            // already at or past `pending.nonce` (proving `pending.nonce` was
            // consumed or fell past the window, rather than `takes()` returning
            // false on a stale nonce read > 24 behind) AND a proved identity
            // read at height >= nonce_height confirms revision < pending.revision
            // and the keys are still enabled; a lagging or stale read never
            // retires or re-signs.
            let tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
            let nonce_spent = pending.nonce <= tip;
            if nonce_spent && id_height >= nonce_height {
                let observed_debit = pending
                    .balance_before
                    .and_then(|before| before.checked_sub(fresh.balance()))
                    .filter(|&delta| delta > 0);
                tracing::warn!(
                    transition_hash = %pending.transition_hash,
                    pending_nonce = pending.nonce,
                    id_height,
                    nonce_height,
                    observed_debit = ?observed_debit,
                    "app connect disconnect retired after spent nonce at consistent height"
                );
                session.withdrawal_nonces.advance_floor(id, pending.nonce);
                record.pending_disconnect = None;
                record.last_conflict = Some(DisconnectConflictInfo {
                    transition_hash: pending.transition_hash,
                    observed_debit,
                    nonce: None,
                    observed_tip: None,
                    observed_height: None,
                });
                let _ = put_record(session, wallet_id, record).await;
                session.vault.revoke_grant(&grant);
                return Err(ConnectError::Failed {
                    failure: ConnectFailure::DisconnectConflict,
                });
            }
            return Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            });
        }

        // 4. Nonce is still admissible and revision < pending.revision:
        //    enforce live fee limits before rebroadcasting. On refusal, keep
        //    `pending_disconnect`, still run confirmation-only recovery, and
        //    never report "nothing was sent".
        if pending.keys.iter().all(|k| verified.to_disable.contains(k)) {
            let fee = live_disconnect_fee_raw(net, &fresh, &pending.keys).await?;
            let fee_refusal: Option<ConnectError> = if fee > DISCONNECT_FEE_BOUND {
                Some(ConnectError::Failed {
                    failure: ConnectFailure::PendingFeeTooHigh,
                })
            } else if fee > granted_max_credits {
                Some(
                    PlatformError::GrantExceeded {
                        purpose: BudgetPurpose::Credits,
                        needed: fee,
                        remaining: granted_max_credits,
                    }
                    .into(),
                )
            } else if fresh.balance() < fee {
                Some(ConnectError::Credits {
                    needed: fee,
                    available: fresh.balance(),
                })
            } else {
                None
            };

            if let Some(refusal) = fee_refusal {
                if confirm_disabled(net, wallet, id, &record, &pending.keys).await {
                    session.withdrawal_nonces.advance_floor(id, pending.nonce);
                    let response_may_have_landed =
                        reconcile_response_delivery(net, &mut record, request_hash, id).await;
                    record.pending_disconnect = None;
                    record.last_conflict = None;
                    if !verified.has_absent && pending.keys.len() == verified.to_disable.len() {
                        record.state = RecordState::Disconnected;
                    }
                    if let Err(e) = put_record(session, wallet_id, record).await {
                        tracing::warn!("app connect record after disconnect: {}", e.code());
                    }
                    return Ok(DisconnectDone {
                        identity: owner,
                        keys_disabled: pending.keys,
                        response_may_have_landed,
                    });
                }
                return Err(refusal);
            }

            if let Err(e) = session.check_funds_gate(wallet_id, FlowKind::AppDisconnect) {
                if confirm_disabled(net, wallet, id, &record, &pending.keys).await {
                    session.withdrawal_nonces.advance_floor(id, pending.nonce);
                    let response_may_have_landed =
                        reconcile_response_delivery(net, &mut record, request_hash, id).await;
                    record.pending_disconnect = None;
                    record.last_conflict = None;
                    if !verified.has_absent && pending.keys.len() == verified.to_disable.len() {
                        record.state = RecordState::Disconnected;
                    }
                    if let Err(e) = put_record(session, wallet_id, record).await {
                        tracing::warn!("app connect record after disconnect: {}", e.code());
                    }
                    return Ok(DisconnectDone {
                        identity: owner,
                        keys_disabled: pending.keys,
                        response_may_have_landed,
                    });
                }
                return Err(PlatformError::from(e).into());
            }

            if let Ok(bytes) = hex::decode(&pending.transition_hex)
                && let Err(e) = net.broadcast_disable_keys(wallet, id, &bytes).await
            {
                tracing::info!("app connect disconnect re-broadcast: {}", e.code());
            }
        }

        if confirm_disabled(net, wallet, id, &record, &pending.keys).await {
            session.withdrawal_nonces.advance_floor(id, pending.nonce);
            let response_may_have_landed =
                reconcile_response_delivery(net, &mut record, request_hash, id).await;
            record.pending_disconnect = None;
            record.last_conflict = None;
            if !verified.has_absent && pending.keys.len() == verified.to_disable.len() {
                record.state = RecordState::Disconnected;
            }
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after disconnect: {}", e.code());
            }
            return Ok(DisconnectDone {
                identity: owner,
                keys_disabled: pending.keys,
                response_may_have_landed,
            });
        }
        // Check whether the rebroadcast confirmed right after confirm_disabled or
        // hit a terminal conflict (revision advanced or nonce spent at
        // consistent height while keys stayed enabled).
        if let Ok((fresh2, id_height2, nonce_state2, nonce_height2)) =
            read_consistent_state(net, wallet, id).await
        {
            if all_present_and_disabled(&fresh2, &record, &pending.keys) {
                session.withdrawal_nonces.advance_floor(id, pending.nonce);
                let response_may_have_landed =
                    reconcile_response_delivery(net, &mut record, request_hash, id).await;
                record.pending_disconnect = None;
                record.last_conflict = None;
                if !verified.has_absent && pending.keys.len() == verified.to_disable.len() {
                    record.state = RecordState::Disconnected;
                }
                if let Err(e) = put_record(session, wallet_id, record).await {
                    tracing::warn!("app connect record after disconnect: {}", e.code());
                }
                return Ok(DisconnectDone {
                    identity: owner,
                    keys_disabled: pending.keys,
                    response_may_have_landed,
                });
            }
            let tip2 = nonce_state2 & IDENTITY_NONCE_VALUE_FILTER;
            let nonce_spent2 =
                pending.nonce <= tip2 && !super::withdrawal::takes(nonce_state2, pending.nonce, id);
            if fresh2.revision() >= pending.revision
                || (id_height2 >= nonce_height2 && nonce_spent2)
            {
                let observed_debit = pending
                    .balance_before
                    .and_then(|before| before.checked_sub(fresh2.balance()))
                    .filter(|&delta| delta > 0);
                tracing::warn!(
                    transition_hash = %pending.transition_hash,
                    pending_revision = pending.revision,
                    current_revision = fresh2.revision(),
                    observed_debit = ?observed_debit,
                    "app connect disconnect retired after conflict on rebroadcast"
                );
                let reserved = reserved_nonce_on_retire(
                    pending.nonce,
                    nonce_state2,
                    id_height2,
                    nonce_height2,
                    id,
                );
                let observed_tip = reserved.map(|_| tip2);
                let observed_height = reserved.map(|_| id_height2.max(nonce_height2));
                if reserved.is_none() {
                    session.withdrawal_nonces.advance_floor(id, pending.nonce);
                }
                record.pending_disconnect = None;
                record.last_conflict = Some(DisconnectConflictInfo {
                    transition_hash: pending.transition_hash,
                    observed_debit,
                    nonce: reserved,
                    observed_tip,
                    observed_height,
                });
                let _ = put_record(session, wallet_id, record).await;
                session.vault.revoke_grant(&grant);
                return Err(ConnectError::Failed {
                    failure: ConnectFailure::DisconnectConflict,
                });
            }
        }
        return Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        });
    }

    // Fresh sign path: require identity read height >= nonce read height so we
    // never sign at a stale revision that would incur a paid consensus error.
    if id_height < nonce_height {
        return Err(PlatformError::Unavailable.into());
    }
    // One nonce owner per identity across Disconnect, transfers, withdrawals,
    // and any other app's live pending Disconnect: never sign a nonce held
    // unresolved by another flow.
    match session.withdrawal_nonces.ownership(id, nonce_state) {
        super::withdrawal::NonceOwner::Free => {}
        super::withdrawal::NonceOwner::Pending => {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::PendingWrite,
            });
        }
        super::withdrawal::NonceOwner::Behind => {
            return Err(PlatformError::Unavailable.into());
        }
    }
    let nonce_tip = nonce_state & IDENTITY_NONCE_VALUE_FILTER;
    for other_nonce in other_live_pendings {
        if super::withdrawal::takes(nonce_state, other_nonce, id) {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::PendingWrite,
            });
        }
        if other_nonce > nonce_tip {
            return Err(PlatformError::Unavailable.into());
        }
    }
    // Retired conflict reservations from other apps forbid signing AT `other_k`
    // only (their dead bytes carry a stale revision and can only consume
    // `other_k` via a paid bump, never changing keys or revision). Skip past
    // admissible reservations by computing a local floor (`max(tip+1, k+1)`)
    // without raising `withdrawal_nonces.floor` (which would wedge the owning
    // record's own re-quote at `other_k`). Still refuse `Unavailable` on a
    // stale read (`other_k > tip`).
    let mut other_reserved_floor = 0u64;
    for (other_k, observed_tip, observed_height) in other_reserved_nonces {
        if super::withdrawal::takes(nonce_state, other_k, id) {
            if observed_tip.is_none_or(|obs| nonce_tip < obs)
                || observed_height.is_none_or(|obs_h| nonce_height < obs_h)
            {
                return Err(PlatformError::Unavailable.into());
            }
            other_reserved_floor =
                other_reserved_floor.max(other_k.checked_add(1).ok_or_else(|| {
                    PlatformError::Internal {
                        detail: "identity nonce overflow".into(),
                    }
                })?);
        } else if other_k > nonce_tip {
            return Err(PlatformError::Unavailable.into());
        }
    }
    let session_floor = session.withdrawal_nonces.floor(id);
    let fallback_nonce = nonce_tip
        .checked_add(1)
        .ok_or_else(|| PlatformError::Internal {
            detail: "identity nonce overflow".into(),
        })?
        .max(session_floor)
        .max(other_reserved_floor);
    // If this record retired an earlier revision-conflicted transition whose
    // nonce `k` is not proven spent yet, sign at `k` while `takes(nonce_state, k)`
    // so the only competitor for `k` is our own dead bytes (never a transfer or
    // withdrawal). Refuse `Unavailable` on a stale read (`k > tip`). Clear the
    // reservation only when `k` itself is proved spent (`k <= tip && !takes`).
    let next_nonce = if let Some(reserved_k) = record.last_conflict.as_ref().and_then(|c| c.nonce) {
        if super::withdrawal::takes(nonce_state, reserved_k, id) {
            reserved_k
        } else if reserved_k > nonce_tip {
            return Err(PlatformError::Unavailable.into());
        } else {
            if let Some(c) = &mut record.last_conflict {
                c.nonce = None;
                c.observed_tip = None;
                c.observed_height = None;
            }
            fallback_nonce
        }
    } else {
        fallback_nonce
    };
    if !super::withdrawal::takes(nonce_state, next_nonce, id) {
        return Err(PlatformError::Unavailable.into());
    }
    let (disable, fee) = select_disconnect_batch(net, &fresh, &verified.to_disable).await?;
    if fee > granted_max_credits {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: fee,
            remaining: granted_max_credits,
        }
        .into());
    }
    if fresh.balance() < fee {
        return Err(ConnectError::Credits {
            needed: fee,
            available: fresh.balance(),
        });
    }
    let master = master_key(&fresh).ok_or(PlatformError::from(
        super::errors::IdentityError::KeysMissing {
            purpose: super::identity::KeyPurpose::Authentication,
        },
    ))?;
    // Pre-sign re-check on every key ID going into the IdentityUpdate.
    for &key_id in &disable {
        if key_id == master.id() {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::UnverifiedKey,
            });
        }
        let key = fresh
            .public_keys()
            .get(&key_id)
            .ok_or(ConnectError::Failed {
                failure: ConnectFailure::UnverifiedKey,
            })?;
        check_disconnect_key(&record, key)?;
    }
    let target_revision = fresh.revision() + 1;
    let held: Vec<HeldKey> = key_path(session.vault.network(), index, master.id())
        .map(|path| vec![(path, master.data().to_vec())])
        .unwrap_or_default();
    let lease = session
        .begin_lease_holding(
            wallet_id,
            FlowKind::AppDisconnect,
            std::slice::from_ref(&grant),
            held,
        )
        .await
        .map_err(PlatformError::from)?;
    let permit = write_permit(&lease, "app_connect_disconnect", fee)?;
    let signer = permit
        .identity_signer(&[index])
        .map_err(PlatformError::from)?
        .only_keys([master.id()]);
    let signed = leased(
        &lease,
        net.sign_disable_keys(&fresh, master.id(), disable.clone(), next_nonce, &signer),
    )
    .await;
    drop(signer);
    let transition_bytes = match signed {
        Ok(bytes) => bytes,
        Err(e) => {
            permit.finish(false);
            drop(lease);
            return Err(e.into());
        }
    };
    let transition_hash = hex::encode(hash_double(&transition_bytes));
    record.pending_disconnect = Some(PendingDisconnect {
        keys: disable.clone(),
        revision: target_revision,
        nonce: next_nonce,
        transition_hex: hex::encode(&transition_bytes),
        transition_hash: transition_hash.clone(),
        balance_before: Some(fresh.balance()),
    });
    if let Err(e) = put_record(session, wallet_id, record.clone()).await {
        permit.finish(false);
        drop(lease);
        return Err(e.into());
    }
    // Cancel any parked sign-in response now that Disconnect is committed and
    // about to be broadcast; if Platform definitively refuses before accepting
    // the broadcast, restore it under the Disconnect hold (never auto-retrying).
    let parked = session
        .platform
        .app_connect
        .take_for(wallet_id, id, request_hash);
    let written = leased(
        &lease,
        net.broadcast_disable_keys(wallet, id, &transition_bytes),
    )
    .await;
    permit.finish(written.is_ok());
    drop(lease);
    if let Err(e) = &written {
        tracing::info!("app connect disconnect: {}", e.code());
    }
    if !confirm_disabled(net, wallet, id, &record, &disable).await {
        if let Err(e) = written
            && refused_unsent(&e)
        {
            record.pending_disconnect = None;
            let _ = put_record(session, wallet_id, record).await;
            if let Some(mut send) = parked {
                send.held_by_disconnect = true;
                session.platform.app_connect.put(wallet_id, send);
            }
            return Err(e.into());
        }
        if let Ok((fresh2, id_height2, nonce_state2, nonce_height2)) =
            read_consistent_state(net, wallet, id).await
        {
            if all_present_and_disabled(&fresh2, &record, &disable) {
                session.withdrawal_nonces.advance_floor(id, next_nonce);
                let response_may_have_landed =
                    reconcile_response_delivery(net, &mut record, request_hash, id).await;
                record.pending_disconnect = None;
                record.last_conflict = None;
                if !verified.has_absent && disable.len() == verified.to_disable.len() {
                    record.state = RecordState::Disconnected;
                }
                if let Err(e) = put_record(session, wallet_id, record).await {
                    tracing::warn!("app connect record after disconnect: {}", e.code());
                }
                return Ok(DisconnectDone {
                    identity: owner,
                    keys_disabled: disable,
                    response_may_have_landed,
                });
            }
            let tip2 = nonce_state2 & IDENTITY_NONCE_VALUE_FILTER;
            let nonce_spent2 =
                next_nonce <= tip2 && !super::withdrawal::takes(nonce_state2, next_nonce, id);
            if fresh2.revision() >= target_revision || (id_height2 >= nonce_height2 && nonce_spent2)
            {
                let observed_debit = fresh
                    .balance()
                    .checked_sub(fresh2.balance())
                    .filter(|&delta| delta > 0);
                tracing::warn!(
                    transition_hash = %transition_hash,
                    target_revision,
                    current_revision = fresh2.revision(),
                    observed_debit = ?observed_debit,
                    "app connect disconnect retired after conflict"
                );
                let reserved = reserved_nonce_on_retire(
                    next_nonce,
                    nonce_state2,
                    id_height2,
                    nonce_height2,
                    id,
                );
                let observed_tip = reserved.map(|_| tip2);
                let observed_height = reserved.map(|_| id_height2.max(nonce_height2));
                if reserved.is_none() {
                    session.withdrawal_nonces.advance_floor(id, next_nonce);
                }
                record.pending_disconnect = None;
                record.last_conflict = Some(DisconnectConflictInfo {
                    transition_hash,
                    observed_debit,
                    nonce: reserved,
                    observed_tip,
                    observed_height,
                });
                let _ = put_record(session, wallet_id, record).await;
                session.vault.revoke_grant(&grant);
                return Err(ConnectError::Failed {
                    failure: ConnectFailure::DisconnectConflict,
                });
            }
        }
        return Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        });
    }
    session.withdrawal_nonces.advance_floor(id, next_nonce);
    let response_may_have_landed =
        reconcile_response_delivery(net, &mut record, request_hash, id).await;
    record.pending_disconnect = None;
    record.last_conflict = None;
    if !verified.has_absent && disable.len() == verified.to_disable.len() {
        record.state = RecordState::Disconnected;
    }
    if let Err(e) = put_record(session, wallet_id, record).await {
        tracing::warn!("app connect record after disconnect: {}", e.code());
    }
    Ok(DisconnectDone {
        identity: owner,
        keys_disabled: disable,
        response_may_have_landed,
    })
}

impl DashPay {
    /// The approval sheet of an App Connect request (DASHPAY F24): the
    /// app, the wallet's identities that may sign in, each binding with a
    /// group's owner, admins and members as Platform proves them now, and
    /// the default terms. Refused, with nothing read, for another network
    /// (`connect.network`), on mainnet in this build
    /// (`connect.mainnet_off`) and for bindings this wallet does not grant
    /// (`connect.unsupported`); after the reads for an old protocol
    /// (`connect.protocol`), a wrong owner (`connect.owner`), a missing
    /// contract or group (`connect.invalid`), no identity with a username
    /// (`connect.no_identity`), a hint this wallet does not hold
    /// (`connect.hint`) and an old request (`connect.stale`). Derives and
    /// signs nothing.
    pub async fn app_connect_sheet(&self, link: ConnectUri) -> Result<ConnectSheet, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            sheet(&SdkConnect(manager.sdk_arc()), &session, wallet_id, &link).await
        })
        .await
    }

    /// The cost of approving `choice` and the grant it needs: an
    /// `AppConnect` grant naming the quote's `request` and `plan`, the
    /// digest of everything read for it. The sheet's refusals, and
    /// `connect.answered` for a request this identity answered,
    /// `connect.credits` for a balance below the quote. Derives and signs
    /// nothing.
    pub async fn app_connect_quote(
        &self,
        link: ConnectUri,
        choice: ConnectChoice,
    ) -> Result<ConnectQuote, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            quote(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &link,
                &choice,
            )
            .await
        })
        .await
    }

    /// Approves `choice` under `grant`: reads everything the quote read
    /// again and refuses `connect.plan_changed` when its digest is not the
    /// grant's; adds the session keys (and an encryption key the identity
    /// lacks) in one MASTER-signed IdentityUpdate, disabling the keys of
    /// the same connection's previous grant; waits for Platform to show
    /// every key; seals the keys to the app and publishes the response,
    /// checking the groups again before the update and before the
    /// response. A response that could not be published is
    /// `connect.send_failed`, kept for [`Self::app_connect_retry_send`].
    pub async fn app_connect_approve(
        &self,
        link: ConnectUri,
        choice: ConnectChoice,
        grant: String,
    ) -> Result<ConnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            approve(&net, &session, &wallet, wallet_id, &link, &choice, grant).await
        })
        .await
    }

    /// Publishes the response an approval of `link` sealed and could not
    /// send, under that approval's lease (a lock ends it:
    /// `connect.failed{nothing_to_send}`).
    pub async fn app_connect_retry_send(
        &self,
        link: ConnectUri,
    ) -> Result<ConnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            retry_send(&net, &session, &wallet, wallet_id, &link).await
        })
        .await
    }

    /// Lists each app this wallet approved for `identity` that still has
    /// enabled session keys on Platform, combining local approval records
    /// with Platform's proved identity keys and live contract/group scopes.
    /// Refused on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_list(&self, identity: String) -> Result<ConnectedApps, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            list(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &identity,
            )
            .await
        })
        .await
    }

    /// The live Platform credit fee and `AppDisconnect` grant to disconnect
    /// one approved app (`request` hex) on `identity`. Verifies every key id
    /// and its public key data against Platform and refuses when a key is
    /// unverified (`connect.failed{unverified_key}`), the live fee exceeds the
    /// fee bound (`connect.failed{fee_too_high}`), credits are short
    /// (`connect.credits`), or on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_disconnect_quote(
        &self,
        identity: String,
        request: String,
    ) -> Result<DisconnectQuote, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            disconnect_quote(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &identity,
                &request,
            )
            .await
        })
        .await
    }

    /// Disables one approved app's verified AUTHENTICATION session keys on
    /// `identity` in a MASTER-signed IdentityUpdate under `grant` (an
    /// `AppDisconnect` grant bound to `request`). Safe to call twice: refreshes
    /// Platform first under the identity write lock and succeeds without
    /// spending credits or sending an update if the app's keys are already
    /// disabled. Refused on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_disconnect(
        &self,
        identity: String,
        request: String,
        grant: String,
    ) -> Result<DisconnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            disconnect(
                &net, &session, &wallet, wallet_id, &identity, &request, grant,
            )
            .await
        })
        .await
    }
}

#[cfg(test)]
#[path = "app_connect_tests.rs"]
mod tests;
