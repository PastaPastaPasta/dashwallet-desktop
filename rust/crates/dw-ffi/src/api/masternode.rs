//! M3 Masternodes tab: the list model over the SPV masternode list, owned
//! detection and the details dialog (QT-118…122, IOS-080), plus the error
//! domain shared by every masternode and ProTx call (`protx.rs`,
//! `masternode_keys.rs`). Owner: R3 (`dw-protx`,
//! `dw-engine/src/masternodes.rs`). Contract: docs/contracts/m3-engine.md
//! §2.3.
//!
//! What an SPV wallet knows (research 02 §10.2): service, type, valid or
//! banned, voting key, operator key and platform node id come from the
//! list; owner/payout/collateral, shares and registration height come from
//! provider transactions the wallets hold (or tracked masternodes'). PoSe
//! score, ban/revive heights, last paid and next payment need a full node:
//! they are `None` here, never estimated.

use crate::api::common::{ensure_open, parse_txid};
use crate::{DashNetwork, NetworkSession, OutPoint};
use dw_protx::params;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MasternodeType {
    Regular,
    /// EvoNode (high-performance masternode).
    Evo,
}

/// The list's type combo (QT-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MasternodeTypeFilter {
    All,
    Regular,
    Evo,
    Shared,
}

/// A key's job on a masternode (platform-wallet `MasternodeKeyRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MasternodeKeyRole {
    Owner,
    Voting,
    /// BLS operator key.
    Operator,
    /// ed25519 Tenderdash node key (EvoNodes).
    PlatformNode,
    OwnerPayout,
    OperatorPayout,
}

/// Why the wallets count a masternode as theirs (QT-120 "Owned").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OwnedRole {
    Collateral,
    Owner,
    Voting,
    Operator,
    Payout,
    OperatorPayout,
    PlatformNode,
    ShareOwner,
    ShareRefund,
    /// Tracked by the user (IOS-082), with or without keys.
    Tracked,
}

/// Status from the list (QT-119 status icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MasternodeListStatus {
    /// Valid in the list. `since_height` is unknown on SPV.
    Active { since_height: Option<u32> },
    /// PoSe-banned in the list.
    Banned { since_height: Option<u32> },
    /// Not in the list (revoked, collateral spent); only for wallet and
    /// tracked masternodes.
    Retired,
    /// The list has not synced.
    Unknown,
}

/// "Operator Reward" column: "NONE" when `percent_x100 == 0`, "x.xx% to
/// <addr>", or "…but not claimed" without a payout address.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OperatorReward {
    /// Hundredths of a percent (0–10000).
    pub percent_x100: u16,
    pub payout_address: Option<String>,
}

/// "Shared (you hold %1 of %2)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SharedHolding {
    pub held_shares: u32,
    pub total_shares: u32,
}

/// One list row (QT-119). `None` = not known to an SPV wallet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeRow {
    /// Display order.
    pub pro_tx_hash: String,
    pub service: Option<String>,
    pub node_type: MasternodeType,
    /// `Some` for a v24 shared masternode the wallets know of.
    pub shared: Option<SharedHolding>,
    pub status: MasternodeListStatus,
    pub pose_score: Option<u32>,
    pub registered_height: Option<u32>,
    pub last_paid_height: Option<u32>,
    pub next_payment_height: Option<u32>,
    pub operator_reward: Option<OperatorReward>,
    pub collateral: Option<OutPoint>,
    pub collateral_address: Option<String>,
    pub owner_address: Option<String>,
    pub voting_address: String,
    pub payout_addresses: Vec<String>,
    /// 96 hex characters.
    pub operator_public_key: String,
    pub platform_node_id: Option<String>,
    /// Empty = not owned.
    pub owned_roles: Vec<OwnedRole>,
    /// Tracked masternode label (IOS-082).
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeQuery {
    pub type_filter: MasternodeTypeFilter,
    /// Literal, case-insensitive substring over the fields dash-qt searches
    /// (service, type, PoSe, heights, payout addresses, operator reward
    /// text, collateral/owner/voting addresses, proTxHash, share
    /// addresses); not the operator key or platform node id.
    pub text: Option<String>,
    pub owned_only: bool,
    pub hide_banned: bool,
}

/// Masternode list availability (QT-118 "Node Count", status line).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeListState {
    /// The list synced at least once this session.
    pub available: bool,
    pub height: Option<u32>,
    pub total: u32,
    pub enabled: u32,
    pub evo_total: u32,
    pub evo_enabled: u32,
    /// SPV is not caught up (refresh every 30 s instead of 3 s).
    pub syncing: bool,
}

/// One share of a shared masternode (details "shares table").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeShare {
    pub amount: u64,
    pub owner_address: String,
    pub payout_address: String,
    pub refund_address: String,
    pub mine: bool,
}

/// The details dialog (QT-122, IOS-080).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeDetail {
    pub row: MasternodeRow,
    pub consecutive_payments: Option<u32>,
    pub pose_ban_height: Option<u32>,
    pub pose_revived_height: Option<u32>,
    /// Every Core P2P address (v24 network info).
    pub network_addresses: Vec<String>,
    pub platform_p2p_addresses: Vec<String>,
    pub platform_https_addresses: Vec<String>,
    pub shares: Vec<MasternodeShare>,
    /// Height the early-exit period ends.
    pub early_period_end: Option<u32>,
    pub early_exit_penalty: Option<u64>,
    /// A standby dissolution was saved for it (QT-127).
    pub has_standby_dissolution: bool,
    /// A ProUpRevTx was seen; the reason (0–3) when known.
    pub revocation_reason: Option<u16>,
    /// Provider transactions of the wallets for it (the source of the
    /// owner/payout/collateral fields).
    pub wallet_transactions: u32,
}

/// Default ports of a network (Register wizard, QT-123).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeNetworkDefaults {
    pub core_p2p_port: u16,
    pub platform_p2p_port: u16,
    pub platform_https_port: u16,
    pub masternode_collateral: u64,
    pub evonode_collateral: u64,
    pub min_shares: u32,
    pub max_shares: u32,
    pub min_share_amount: u64,
    pub max_early_period_blocks: u32,
    pub max_envelope_bytes: u64,
    pub max_operator_reward_x100: u16,
}

/// The masternode constants of `network`. **Works** (constants of
/// `dw-protx`).
#[uniffi::export]
pub fn masternode_network_defaults(network: DashNetwork) -> MasternodeNetworkDefaults {
    let core = dw_engine::DashNetwork::from(network).core_network();
    let ports = params::default_ports(core);
    MasternodeNetworkDefaults {
        core_p2p_port: ports.core_p2p,
        platform_p2p_port: ports.platform_p2p,
        platform_https_port: ports.platform_https,
        masternode_collateral: params::MASTERNODE_COLLATERAL,
        evonode_collateral: params::EVONODE_COLLATERAL,
        min_shares: params::MIN_SHARES,
        max_shares: params::MAX_SHARES,
        min_share_amount: params::MIN_SHARE_AMOUNT,
        max_early_period_blocks: params::MAX_EARLY_PERIOD_BLOCKS,
        max_envelope_bytes: params::MAX_ENVELOPE_BYTES as u64,
        max_operator_reward_x100: params::MAX_OPERATOR_REWARD,
    }
}

/// Why an outpoint cannot be a collateral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CollateralRefusal {
    WrongAmount,
    Unconfirmed,
    NotP2pkh,
    Locked,
    AlreadyCollateral,
    NotFound,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MasternodeError {
    /// Code `masternode.list_unavailable`.
    #[error("masternode list unavailable")]
    ListUnavailable,
    /// Code `masternode.not_found`.
    #[error("masternode {pro_tx_hash} not found")]
    NotFound { pro_tx_hash: String },
    /// Code `masternode.key_not_in_wallet`.
    #[error("{role:?} key not in wallet")]
    KeyNotInWallet { role: MasternodeKeyRole },
    /// Code `masternode.invalid_service`.
    #[error("invalid service: {detail}")]
    InvalidService { detail: String },
    /// Code `masternode.invalid_key`.
    #[error("invalid {role:?} key: {detail}")]
    InvalidKey {
        role: MasternodeKeyRole,
        detail: String,
    },
    /// Code `masternode.invalid_payout`.
    #[error("invalid payout: {detail}")]
    InvalidPayout { detail: String },
    /// Code `masternode.duplicate_address`.
    #[error("duplicate address: {detail}")]
    DuplicateAddress { detail: String },
    /// Code `masternode.collateral_unavailable`.
    #[error("collateral unavailable: {refusal:?}")]
    CollateralUnavailable { refusal: CollateralRefusal },
    /// Code `masternode.insufficient_funds`.
    #[error("needs {needed} duffs, {available} available")]
    InsufficientFunds { needed: u64, available: u64 },
    /// Code `masternode.operator_secret_mismatch`.
    #[error("operator secret does not match")]
    OperatorSecretMismatch,
    /// Code `masternode.operator_secret_unconfirmed`.
    #[error("operator secret not confirmed")]
    OperatorSecretUnconfirmed,
    /// Code `masternode.collateral_signature_invalid`.
    #[error("collateral signature invalid")]
    CollateralSignatureInvalid,
    /// Code `masternode.unsupported_entry`.
    #[error("unsupported entry: {detail}")]
    UnsupportedEntry { detail: String },
    /// Code `masternode.watch_only`.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `masternode.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `masternode.grant_invalid`.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `masternode.no_peers`.
    #[error("no connected peers")]
    NoPeers,
    /// Code `masternode.broadcast_rejected`.
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
    /// Code `masternode.shared_envelope_invalid`.
    #[error("shared message invalid: {detail}")]
    SharedEnvelopeInvalid { detail: String },
    /// Code `masternode.shared_envelope_too_large`.
    #[error("shared message of {size_bytes} bytes is too large")]
    SharedEnvelopeTooLarge { size_bytes: u64 },
    /// Code `masternode.shared_network_mismatch`.
    #[error("shared message is for another network")]
    SharedNetworkMismatch,
    /// Code `masternode.shared_session_not_found`.
    #[error("shared session {session_id} not found")]
    SharedSessionNotFound { session_id: String },
    /// Code `masternode.shared_inputs_refused`.
    #[error("shared inputs refused: {detail}")]
    SharedInputsRefused { detail: String },
    /// Code `masternode.shared_coin_spent`.
    #[error("reserved coin was spent")]
    SharedCoinSpent { outpoint: OutPoint },
    /// Code `masternode.already_tracked`.
    #[error("masternode {pro_tx_hash} already tracked")]
    AlreadyTracked { pro_tx_hash: String },
    /// Code `masternode.platform_unavailable`.
    #[error("platform unavailable")]
    PlatformUnavailable,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

crate::api::common::domain_error_common!(@not_implemented MasternodeError);
crate::api::common::export_error_code!(MasternodeError);

impl MasternodeError {
    /// Stable code (docs/contracts/m3-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::ListUnavailable => "masternode.list_unavailable",
            Self::NotFound { .. } => "masternode.not_found",
            Self::KeyNotInWallet { .. } => "masternode.key_not_in_wallet",
            Self::InvalidService { .. } => "masternode.invalid_service",
            Self::InvalidKey { .. } => "masternode.invalid_key",
            Self::InvalidPayout { .. } => "masternode.invalid_payout",
            Self::DuplicateAddress { .. } => "masternode.duplicate_address",
            Self::CollateralUnavailable { .. } => "masternode.collateral_unavailable",
            Self::InsufficientFunds { .. } => "masternode.insufficient_funds",
            Self::OperatorSecretMismatch => "masternode.operator_secret_mismatch",
            Self::OperatorSecretUnconfirmed => "masternode.operator_secret_unconfirmed",
            Self::CollateralSignatureInvalid => "masternode.collateral_signature_invalid",
            Self::UnsupportedEntry { .. } => "masternode.unsupported_entry",
            Self::WatchOnly => "masternode.watch_only",
            Self::VaultLocked => "masternode.vault_locked",
            Self::GrantInvalid => "masternode.grant_invalid",
            Self::NoPeers => "masternode.no_peers",
            Self::BroadcastRejected { .. } => "masternode.broadcast_rejected",
            Self::SharedEnvelopeInvalid { .. } => "masternode.shared_envelope_invalid",
            Self::SharedEnvelopeTooLarge { .. } => "masternode.shared_envelope_too_large",
            Self::SharedNetworkMismatch => "masternode.shared_network_mismatch",
            Self::SharedSessionNotFound { .. } => "masternode.shared_session_not_found",
            Self::SharedInputsRefused { .. } => "masternode.shared_inputs_refused",
            Self::SharedCoinSpent { .. } => "masternode.shared_coin_spent",
            Self::AlreadyTracked { .. } => "masternode.already_tracked",
            Self::PlatformUnavailable => "masternode.platform_unavailable",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<dw_engine::MasternodeKeyRole> for MasternodeKeyRole {
    fn from(r: dw_engine::MasternodeKeyRole) -> Self {
        use dw_engine::MasternodeKeyRole as R;
        match r {
            R::Owner => Self::Owner,
            R::Voting => Self::Voting,
            R::Operator => Self::Operator,
            R::PlatformNode => Self::PlatformNode,
            R::OwnerPayout => Self::OwnerPayout,
            R::OperatorPayout => Self::OperatorPayout,
        }
    }
}

impl From<dw_engine::CollateralRefusal> for CollateralRefusal {
    fn from(r: dw_engine::CollateralRefusal) -> Self {
        use dw_engine::CollateralRefusal as R;
        match r {
            R::WrongAmount => Self::WrongAmount,
            R::Unconfirmed => Self::Unconfirmed,
            R::NotP2pkh => Self::NotP2pkh,
            R::Locked => Self::Locked,
            R::AlreadyCollateral => Self::AlreadyCollateral,
            R::NotFound => Self::NotFound,
        }
    }
}

impl From<dw_engine::EngineError> for MasternodeError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        use dw_engine::MasternodeFailure as F;
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::Masternode(f) => match f {
                F::ListUnavailable => Self::ListUnavailable,
                F::NotFound(pro_tx_hash) => Self::NotFound { pro_tx_hash },
                F::KeyNotInWallet(role) => Self::KeyNotInWallet { role: role.into() },
                F::InvalidService(detail) => Self::InvalidService { detail },
                F::InvalidKey { role, detail } => Self::InvalidKey {
                    role: role.into(),
                    detail,
                },
                F::InvalidPayout(detail) => Self::InvalidPayout { detail },
                F::DuplicateAddress(detail) => Self::DuplicateAddress { detail },
                F::CollateralUnavailable(r) => Self::CollateralUnavailable { refusal: r.into() },
                F::InsufficientFunds { needed, available } => {
                    Self::InsufficientFunds { needed, available }
                }
                F::OperatorSecretMismatch => Self::OperatorSecretMismatch,
                F::OperatorSecretUnconfirmed => Self::OperatorSecretUnconfirmed,
                F::CollateralSignatureInvalid => Self::CollateralSignatureInvalid,
                F::UnsupportedEntry(detail) => Self::UnsupportedEntry { detail },
                F::WatchOnly => Self::WatchOnly,
                F::VaultLocked => Self::VaultLocked,
                F::GrantInvalid => Self::GrantInvalid,
                F::NoPeers => Self::NoPeers,
                F::BroadcastRejected(reason) => Self::BroadcastRejected { reason },
                F::SharedEnvelopeInvalid(detail) => Self::SharedEnvelopeInvalid { detail },
                F::SharedEnvelopeTooLarge { size_bytes } => {
                    Self::SharedEnvelopeTooLarge { size_bytes }
                }
                F::SharedNetworkMismatch => Self::SharedNetworkMismatch,
                F::SharedSessionNotFound(session_id) => Self::SharedSessionNotFound { session_id },
                F::SharedInputsRefused(detail) => Self::SharedInputsRefused { detail },
                F::SharedCoinSpent(o) => Self::SharedCoinSpent { outpoint: o.into() },
                F::AlreadyTracked(pro_tx_hash) => Self::AlreadyTracked { pro_tx_hash },
                F::PlatformUnavailable => Self::PlatformUnavailable,
            },
            E::Vault(V::NoVault | V::Locked | V::MixingOnly) => Self::VaultLocked,
            E::Vault(V::GrantInvalid | V::GrantPurposeMismatch) => Self::GrantInvalid,
            E::NoPeers => Self::NoPeers,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

/// Parses a proTxHash argument (64 lowercase hex, display order).
pub(crate) fn parse_pro_tx_hash(hash: &str) -> Result<(), MasternodeError> {
    parse_txid(hash).map(|_| ()).map_err(Into::into)
}

// Engine ↔ FFI conversions of the list model.

impl From<dw_engine::masternodes::MasternodeType> for MasternodeType {
    fn from(t: dw_engine::masternodes::MasternodeType) -> Self {
        match t {
            dw_engine::masternodes::MasternodeType::Regular => Self::Regular,
            dw_engine::masternodes::MasternodeType::Evo => Self::Evo,
        }
    }
}

impl From<MasternodeType> for dw_engine::masternodes::MasternodeType {
    fn from(t: MasternodeType) -> Self {
        match t {
            MasternodeType::Regular => Self::Regular,
            MasternodeType::Evo => Self::Evo,
        }
    }
}

impl From<MasternodeKeyRole> for dw_engine::MasternodeKeyRole {
    fn from(r: MasternodeKeyRole) -> Self {
        match r {
            MasternodeKeyRole::Owner => Self::Owner,
            MasternodeKeyRole::Voting => Self::Voting,
            MasternodeKeyRole::Operator => Self::Operator,
            MasternodeKeyRole::PlatformNode => Self::PlatformNode,
            MasternodeKeyRole::OwnerPayout => Self::OwnerPayout,
            MasternodeKeyRole::OperatorPayout => Self::OperatorPayout,
        }
    }
}

impl From<dw_engine::masternodes::OwnedRole> for OwnedRole {
    fn from(r: dw_engine::masternodes::OwnedRole) -> Self {
        use dw_engine::masternodes::OwnedRole as R;
        match r {
            R::Collateral => Self::Collateral,
            R::Owner => Self::Owner,
            R::Voting => Self::Voting,
            R::Operator => Self::Operator,
            R::Payout => Self::Payout,
            R::OperatorPayout => Self::OperatorPayout,
            R::PlatformNode => Self::PlatformNode,
            R::ShareOwner => Self::ShareOwner,
            R::ShareRefund => Self::ShareRefund,
            R::Tracked => Self::Tracked,
        }
    }
}

impl From<dw_engine::masternodes::MasternodeListStatus> for MasternodeListStatus {
    fn from(s: dw_engine::masternodes::MasternodeListStatus) -> Self {
        use dw_engine::masternodes::MasternodeListStatus as S;
        match s {
            S::Active { since_height } => Self::Active { since_height },
            S::Banned { since_height } => Self::Banned { since_height },
            S::Retired => Self::Retired,
            S::Unknown => Self::Unknown,
        }
    }
}

impl From<dw_engine::masternodes::MasternodeRow> for MasternodeRow {
    fn from(r: dw_engine::masternodes::MasternodeRow) -> Self {
        Self {
            pro_tx_hash: r.pro_tx_hash,
            service: r.service,
            node_type: r.node_type.into(),
            shared: r.shared.map(|(held_shares, total_shares)| SharedHolding {
                held_shares,
                total_shares,
            }),
            status: r.status.into(),
            pose_score: r.pose_score,
            registered_height: r.registered_height,
            last_paid_height: r.last_paid_height,
            next_payment_height: r.next_payment_height,
            operator_reward: r.operator_reward.map(|o| OperatorReward {
                percent_x100: o.percent_x100,
                payout_address: o.payout_address,
            }),
            collateral: r.collateral.map(Into::into),
            collateral_address: r.collateral_address,
            owner_address: r.owner_address,
            voting_address: r.voting_address,
            payout_addresses: r.payout_addresses,
            operator_public_key: r.operator_public_key,
            platform_node_id: r.platform_node_id,
            owned_roles: r.owned_roles.into_iter().map(Into::into).collect(),
            label: r.label,
        }
    }
}

impl From<MasternodeQuery> for dw_engine::masternodes::MasternodeQuery {
    fn from(q: MasternodeQuery) -> Self {
        use dw_engine::masternodes::MasternodeTypeFilter as F;
        Self {
            type_filter: match q.type_filter {
                MasternodeTypeFilter::All => F::All,
                MasternodeTypeFilter::Regular => F::Regular,
                MasternodeTypeFilter::Evo => F::Evo,
                MasternodeTypeFilter::Shared => F::Shared,
            },
            text: q.text,
            owned_only: q.owned_only,
            hide_banned: q.hide_banned,
        }
    }
}

impl From<dw_engine::masternodes::MasternodeDetail> for MasternodeDetail {
    fn from(d: dw_engine::masternodes::MasternodeDetail) -> Self {
        Self {
            row: d.row.into(),
            consecutive_payments: d.consecutive_payments,
            pose_ban_height: d.pose_ban_height,
            pose_revived_height: d.pose_revived_height,
            network_addresses: d.network_addresses,
            platform_p2p_addresses: d.platform_p2p_addresses,
            platform_https_addresses: d.platform_https_addresses,
            shares: d
                .shares
                .into_iter()
                .map(|s| MasternodeShare {
                    amount: s.amount,
                    owner_address: s.owner_address,
                    payout_address: s.payout_address,
                    refund_address: s.refund_address,
                    mine: s.mine,
                })
                .collect(),
            early_period_end: d.early_period_end,
            early_exit_penalty: d.early_exit_penalty,
            has_standby_dissolution: d.has_standby_dissolution,
            revocation_reason: d.revocation_reason,
            wallet_transactions: d.wallet_transactions,
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// In-memory read; re-query on `Masternodes` events.
    pub fn masternode_list_state(&self) -> Result<MasternodeListState, MasternodeError> {
        ensure_open(&self.inner)?;
        let s = self.inner.masternode_list_state()?;
        Ok(MasternodeListState {
            available: s.available,
            height: s.height,
            total: s.total,
            enabled: s.enabled,
            evo_total: s.evo_total,
            evo_enabled: s.evo_enabled,
            syncing: s.syncing,
        })
    }

    /// The filtered list (QT-118/119), list order. Works without a wallet
    /// (owned detection then finds nothing). Before the list synced: only
    /// wallet and tracked masternodes, with `Unknown` status.
    pub async fn masternodes(
        &self,
        query: MasternodeQuery,
    ) -> Result<Vec<MasternodeRow>, MasternodeError> {
        ensure_open(&self.inner)?;
        let rows = self.inner.masternodes(query.into()).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// The details dialog (QT-122) and IOS-080 detail.
    pub async fn masternode_detail(
        &self,
        pro_tx_hash: String,
    ) -> Result<MasternodeDetail, MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(self.inner.masternode_detail(pro_tx_hash).await?.into())
    }
}
