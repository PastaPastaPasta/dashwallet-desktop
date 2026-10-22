//! The records the masternodes facade returns and takes
//! (`docs/contracts/mn-engine.md` §5). Hashes are 64-character lower-case
//! hex in Core's display order; outpoints are `"<txid>-<n>"` (Core's
//! `ToStringShort`); amounts are duffs unless named credits; times are Unix
//! seconds. Each field says where it comes from (MG-03 §1): the SPV list
//! (`ListFacts`), a provider transaction the wallet holds
//! (`RegistrationFacts`), the wallet's history, or Platform.

use serde::{Deserialize, Serialize};

/// Core's `MnType` (`dmn_types.h`): `Regular` and `Evo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeType {
    Regular,
    Evonode,
}

/// The status of a masternode (MG-03 §2, D2). `enabled` and `pose_banned`
/// are Core's `ENABLED` / `POSE_BANNED`; the rest are the wallet's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeStatus {
    /// In the synced list and valid.
    Enabled,
    /// In the synced list and not valid (PoSe-banned).
    PoseBanned,
    /// Not in the synced list, though it was listed or registered at or
    /// below the list height: its collateral was spent.
    Removed,
    /// The wallet holds its ProRegTx, not yet in a block.
    Unconfirmed,
    /// The list isn't synced, or a tracked masternode was never listed.
    Unknown,
}

/// A key role the vault keeps for a tracked masternode (MG-01 D7). No
/// operator or platform-node role exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachedKeyRole {
    Owner,
    Voting,
    Payout,
}

impl AttachedKeyRole {
    /// The vault's role name (`MASTERNODE_KEY_ROLES`).
    pub fn vault_name(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Voting => "voting",
            Self::Payout => "payout",
        }
    }

    pub(crate) fn from_vault_name(name: &str) -> Option<Self> {
        match name {
            "owner" => Some(Self::Owner),
            "voting" => Some(Self::Voting),
            "payout" => Some(Self::Payout),
            _ => None,
        }
    }
}

/// How an attached key compares with what the wallet knows of the
/// masternode now (MG-00 D13, MG-03 D24): the list's voting key, the owner
/// key and P2PKH payout of a ProRegTx/ProUpRegTx a loaded wallet holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCheck {
    /// It is the masternode's key for the role.
    Matched,
    /// It is not (a ProUpRegTx rotated the voting key, or the masternode
    /// was listed after the key was attached). `attach_key` refuses one,
    /// so only reads report it.
    KeyMismatch,
    /// Nothing known to check it against (the list not synced, not listed,
    /// no ProRegTx held, or a payout script that may be stale, D19). An
    /// unchecked voting key holds no role (D29).
    Unchecked,
}

/// A key attached to a tracked masternode (MG-03 D24).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedKey {
    pub role: AttachedKeyRole,
    /// hash160 of its public key, 40 hex, recorded when it was attached;
    /// `None` for a key attached before the id was kept (attach it again).
    pub key_id: Option<String>,
    /// Compared on this read; `Unchecked` without `key_id`.
    pub check: KeyCheck,
}

impl AttachedKey {
    /// Whether the key gives the wallet its role: recorded, and not a
    /// mismatch. A ProUpRegTx can rotate the voting key, so only a match
    /// with the synced list holds it (D29).
    pub fn holds(&self) -> bool {
        self.key_id.is_some()
            && match self.check {
                KeyCheck::Matched => true,
                KeyCheck::KeyMismatch => false,
                KeyCheck::Unchecked => self.role != AttachedKeyRole::Voting,
            }
    }
}

/// The collateral outpoint, as far as the wallet can derive it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralState {
    /// Resolved per Core from a ProRegTx the wallet holds: a null payload
    /// hash names an output of the ProRegTx, whose amount was checked.
    Known { outpoint: String },
    /// No ProRegTx in the wallet (a tracked masternode, MG-00 D43 needs an
    /// upstream API, MG-03 D11), or its internal output didn't check out.
    Unresolved,
}

/// What "in this wallet" holds for one masternode (MG-03 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeRoles {
    /// The collateral outpoint is an unspent output of the wallet.
    pub collateral: bool,
    /// A derived owner key (indexes 0–19), or an attached owner key that
    /// holds ([`AttachedKey::holds`]).
    pub owner: bool,
    /// The list's voting key is derived by the wallet (0–19), or an
    /// attached voting key holds.
    pub voting: bool,
    /// The payout script is the wallet's.
    pub payout: bool,
    /// The operator key is the wallet's (a self-operator). Never signs.
    pub operator: bool,
    /// The keys attached in the vault (tracked masternodes), by role;
    /// `None` when the vault is locked (the record ids need the data key).
    pub attached: Option<Vec<AttachedKey>>,
}

/// The SPV list's facts about a masternode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFacts {
    pub kind: MasternodeType,
    /// Core: not PoSe-banned.
    pub valid: bool,
    /// `"ip:port"`.
    pub service: Option<String>,
    /// 96 hex characters, as the list carries it.
    pub operator_public_key: String,
    /// The voting key's P2PKH address.
    pub voting_address: String,
    /// Evonodes: the Tenderdash node id, 40 hex.
    pub platform_node_id: Option<String>,
    pub platform_http_port: Option<u16>,
}

/// Facts from the provider transactions the wallet holds (MG-03 §1). The
/// keys and payout follow the latest ProRegTx/ProUpRegTx the wallet saw,
/// which may miss an update that names no wallet key (D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationFacts {
    /// The ProRegTx's block height; `None` until it is in a block, or when
    /// the wallet holds only updates.
    pub registered_height: Option<u32>,
    pub collateral: CollateralState,
    /// The owner key's P2PKH address (from the ProRegTx).
    pub owner_address: Option<String>,
    pub voting_address: Option<String>,
    /// The payout as an address, when the script is a standard one.
    pub payout_address: Option<String>,
    /// The payout script, hex.
    pub payout_script: Option<String>,
    /// The operator's share in basis points (ProRegTx `operatorReward`).
    pub operator_reward: Option<u16>,
    /// A ProUpRevTx was seen; the reason it gave.
    pub revoked: bool,
    pub revocation_reason: Option<u16>,
    /// Provider transactions of this masternode in the wallet.
    pub transactions: u32,
}

/// One payout: a wallet history record of a coinbase output to the
/// masternode's payout address (MG-00 D10, MG-03 D9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodePayout {
    pub txid: String,
    /// The history record's index within the transaction.
    pub record_index: u32,
    pub amount: u64,
    pub address: String,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
    /// Past coinbase maturity (spendable).
    pub matured: bool,
    /// It may be a share of a version-3 masternode paid to the same
    /// script, so no masternode's `payouts_30d` counts it; the overview's
    /// `unattributed_30d` does (D30).
    pub unattributed: bool,
}

/// One masternode in a wallet's overview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeRow {
    pub pro_tx_hash: String,
    /// From the list, else the ProRegTx; `None` when neither is known.
    pub kind: Option<MasternodeType>,
    /// The per-type number among this wallet's own masternodes
    /// ("Masternode 1", "Evonode 1"); 0 for a tracked-only one.
    pub type_index: u32,
    pub label: Option<String>,
    pub status: MasternodeStatus,
    /// The wallet's first sighting of the current ban (MG-00 D9).
    pub banned_seen_at: Option<u64>,
    /// From the list, else the wallet's latest provider transaction.
    pub service: Option<String>,
    /// The wallet has a role through its own provider transactions.
    pub mine: bool,
    pub tracked: bool,
    /// A version-3 ProRegTx/ProUpRegTx, or a list entry seen at version 3
    /// (remembered, D29), whose payouts and shares (Core v24) aren't read
    /// yet: its payouts may be shared, so the keys and payout aren't
    /// reported and its payouts are left out of every total (MG-03 D25;
    /// shares are MG-03b).
    pub shared_unsupported: bool,
    pub roles: MasternodeRoles,
    pub collateral: CollateralState,
    pub payout_address: Option<String>,
    /// Payouts to this wallet in the last 30 days (duffs), without the
    /// `unattributed` ones.
    pub payouts_30d: u64,
    /// `shared_unsupported`, or a payout in the window is `unattributed`:
    /// `payouts_30d` leaves some of its income out (D30).
    pub payouts_incomplete: bool,
    pub last_payout: Option<MasternodePayout>,
}

/// The SPV list as the overview read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeListState {
    /// The headers and masternode phases are done.
    pub synced: bool,
    /// The masternode phase's height (D3).
    pub height: Option<u32>,
}

/// An unspent output of exactly a collateral amount that is no known
/// masternode's collateral (MG-00 D1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralCoin {
    pub outpoint: String,
    pub address: String,
    pub amount: u64,
    pub kind: MasternodeType,
    pub confirmations: u32,
    pub user_locked: bool,
}

/// A wallet's My masternodes page (UX-SPEC §4.25.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeOverview {
    pub list: MasternodeListState,
    /// The wallet's own masternodes in registration order, then the
    /// tracked ones not already listed, in the order they were tracked.
    pub masternodes: Vec<MasternodeRow>,
    pub collateral_coins: Vec<CollateralCoin>,
    /// Payouts to this wallet in the last 30 days, all masternodes,
    /// without the `unattributed` ones.
    pub payouts_30d: u64,
    /// Coinbase outputs to the wallet in the last 30 days that no row's
    /// `payouts_30d` counts: possible shares, and income paying no row.
    pub unattributed_30d: u64,
    /// A row's `payouts_incomplete`; or, once version 3 was seen (D30),
    /// any coinbase output to the wallet in the window, all of it then
    /// `unattributed`.
    pub payouts_incomplete: bool,
}

/// One masternode in full, each source apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeDetail {
    pub row: MasternodeRow,
    /// `None` when the list isn't synced or doesn't hold it.
    pub list: Option<ListFacts>,
    /// `None` when the wallet holds none of its provider transactions.
    pub registration: Option<RegistrationFacts>,
    /// The height up to which every update of the ProTx hash was applied
    /// (MG-00 D42); `None` ("checking") until that watch exists (D5).
    pub payout_checked_height: Option<u32>,
    pub tracked_at: Option<u64>,
}

/// A list entry `find` returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedMasternode {
    pub pro_tx_hash: String,
    pub facts: ListFacts,
    pub tracked: bool,
}

/// What `untrack` removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrackOutcome {
    /// Attached keys deleted from the vault (MG-00 D22).
    pub keys_deleted: u32,
}

/// An evonode's claimable credits, read with a Platform proof (MG-03 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvonodeCredits {
    pub pro_tx_hash: String,
    pub credits: u64,
    /// `credits` / 1,000 (dpp's `CREDITS_PER_DUFF`), rounded down.
    pub duffs: u64,
    pub fetched_at: u64,
}
