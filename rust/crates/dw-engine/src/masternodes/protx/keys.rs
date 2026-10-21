//! The keys a ProTx names (MG-04 D13, D14, D15): the keychain's public
//! keys and addresses, or what the user gave. Never a secret: the operator
//! key is the keychain's BLS public key, derived from the xpub, or a given
//! one; no step here reads an operator secret.

use std::collections::HashSet;
use std::str::FromStr;

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint};
use dw_appdb::MasternodeOpRow;

use super::super::MasternodesError;
use super::super::protx_records::{
    KeyPick, OperatorChoice, PlatformNodeChoice, ProTxKeys, ProTxOperationKind, ProTxTerms,
    VotingChoice,
};
use super::ops::kind_of;
use crate::masternode_keys::{MasternodeKeyInfo, MasternodeKeyRole};
use crate::{EngineError, NetworkSession, WalletId};

/// Keychain owner and voting indexes a registration may use: the indexes
/// MG-03 scans for roles, so a masternode registered with any of them is
/// one the wallet sees as its own (D13).
pub(crate) const KEY_INDEXES: u32 = 20;
/// Operator and platform-node indexes `NextUnused` scans.
const SCAN_INDEXES: u32 = 100;

/// The keys the wallet's masternodes, the synced list and the wallet's
/// operations already name (D13), and the collaterals in use (D23).
#[derive(Debug, Default)]
pub(crate) struct InUse {
    pub owner: HashSet<[u8; 20]>,
    pub voting: HashSet<[u8; 20]>,
    /// Operator keys as listed or registered, in whatever serialization;
    /// a candidate is compared under both (D5).
    pub operator: HashSet<[u8; 48]>,
    pub node_ids: HashSet<[u8; 20]>,
    pub collateral: HashSet<OutPoint>,
}

impl InUse {
    /// What `terms` names: an operation's keys count as used until it is
    /// discarded.
    pub fn add_terms(&mut self, terms: &ProTxTerms, network: Network) {
        if let Some(id) = p2pkh_id(&terms.owner_address, network) {
            self.owner.insert(id);
        }
        if let Some(id) = p2pkh_id(&terms.voting_address, network) {
            self.voting.insert(id);
        }
        if let Ok(key) = dw_protx::bls::parse_public_hex(&terms.operator_public_key) {
            self.operator.insert(key);
        }
        if let Some(node) = terms.platform.as_ref().and_then(|p| hex20(&p.node_id).ok()) {
            self.node_ids.insert(node);
        }
        if let Some(o) = terms
            .collateral_outpoint
            .as_deref()
            .and_then(|o| parse_outpoint(o).ok())
        {
            self.collateral.insert(o);
        }
    }

    fn holds_operator(&self, basic: &[u8; 48]) -> bool {
        self.operator.contains(basic)
            || dw_protx::bls::legacy_of(basic).is_some_and(|l| self.operator.contains(&l))
    }
}

/// What an operation claims that Core's mempool holds unique (D51): one of
/// the wallet's pending rows ([`super::build::Context::pending`]), or a new
/// operation's terms.
#[derive(Debug, Default)]
pub(crate) struct Claim {
    /// The row's id; a row never clashes with itself.
    pub id: Option<String>,
    pub operator: Option<[u8; 48]>,
    pub node_id: Option<[u8; 20]>,
    pub service: Option<String>,
}

impl Claim {
    /// What `row` claims, from its `terms`: a registration its operator
    /// key, platform node id and service; an update only its operator key,
    /// the one key Core's mempool indexes for a ProUpRegTx (rc.3
    /// `txmempool.cpp:656-659`).
    pub fn of_row(row: &MasternodeOpRow, terms: &ProTxTerms) -> Self {
        let operator = dw_protx::bls::parse_public_hex(&terms.operator_public_key).ok();
        let claim = Claim {
            id: Some(row.id.clone()),
            operator,
            ..Claim::default()
        };
        match kind_of(&row.kind) {
            ProTxOperationKind::UpdateRegistrar => claim,
            ProTxOperationKind::Registration => Claim {
                node_id: terms.platform.as_ref().and_then(|p| hex20(&p.node_id).ok()),
                service: Some(terms.service.clone()).filter(|s| !s.is_empty()),
                ..claim
            },
        }
    }

    /// Whether `other` claims what this one holds: the operator key, the
    /// platform node id or the service. Both sides are the wallet's terms,
    /// whose operator keys are in the basic serialization
    /// (`parse_public_hex`), so one comparison covers it.
    pub fn clashes(&self, other: &Claim) -> bool {
        self.operator.is_some() && self.operator == other.operator
            || self.node_id.is_some() && self.node_id == other.node_id
            || self.service.is_some() && self.service == other.service
    }
}

/// The key id of a P2PKH address on `network`.
pub(crate) fn p2pkh_id(text: &str, network: Network) -> Option<[u8; 20]> {
    let address = Address::from_str(text)
        .ok()?
        .require_network(network)
        .ok()?;
    match address.payload() {
        Payload::PubkeyHash(h) => Some(h.to_byte_array()),
        _ => None,
    }
}

/// `"<txid>-<n>"` (Core's `COutPoint::ToStringShort`).
pub(crate) fn parse_outpoint(text: &str) -> Result<OutPoint, EngineError> {
    let bad = || EngineError::InvalidArgument("not an outpoint (\"<txid>-<n>\")".into());
    let (txid, vout) = text.trim().rsplit_once('-').ok_or_else(bad)?;
    Ok(OutPoint {
        txid: dashcore::Txid::from_str(txid).map_err(|_| bad())?,
        vout: vout.parse().map_err(|_| bad())?,
    })
}

/// 64 hex characters: the shape of a secp256k1 or BLS secret where a
/// public key or node id is expected (D15). The refusal echoes nothing.
fn looks_secret(text: &str) -> bool {
    let t = text.trim();
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn secret_refusal() -> MasternodesError {
    EngineError::InvalidArgument("looks like a secret key, not a public key".into()).into()
}

/// An operator BLS public key: 96 hex characters, basic serialization.
pub(crate) fn parse_operator_key(text: &str) -> Result<[u8; 48], MasternodesError> {
    if looks_secret(text) {
        return Err(secret_refusal());
    }
    dw_protx::bls::parse_public_hex(text).map_err(|_| {
        EngineError::InvalidArgument("not an operator BLS public key (96 hex)".into()).into()
    })
}

/// A platform node id: 40 hex characters.
pub(crate) fn parse_node_id(text: &str) -> Result<[u8; 20], MasternodesError> {
    if looks_secret(text) {
        return Err(secret_refusal());
    }
    hex20(text)
        .map_err(|_| EngineError::InvalidArgument("not a platform node id (40 hex)".into()).into())
}

fn hex20(text: &str) -> Result<[u8; 20], hex::FromHexError> {
    let mut out = [0u8; 20];
    hex::decode_to_slice(text.trim(), &mut out)?;
    Ok(out)
}

/// A secp256k1 key of the keychain: its index, address and key id.
#[derive(Debug, Clone)]
pub(crate) struct Picked {
    pub index: Option<u32>,
    pub address: String,
    pub key_id: [u8; 20],
}

/// An operator key: given, or the keychain's at `index`.
#[derive(Debug, Clone)]
pub(crate) struct PickedOperator {
    pub index: Option<u32>,
    pub public_key: [u8; 48],
}

#[derive(Debug, Clone)]
pub(crate) struct PickedNode {
    pub index: Option<u32>,
    pub node_id: [u8; 20],
}

fn exhausted(role: &str) -> MasternodesError {
    MasternodesError::KeysExhausted { role: role.into() }
}

impl NetworkSession {
    async fn keys_of(
        &self,
        wallet: WalletId,
        role: MasternodeKeyRole,
        start: u32,
        count: u32,
    ) -> Result<Vec<MasternodeKeyInfo>, MasternodesError> {
        Ok(self
            .derive_masternode_keys(wallet, role, start, count)
            .await?)
    }

    /// The owner key: the keychain's at `index`, or its lowest unused one.
    /// One the wallet already uses is refused as Core would (`bad-protx-dup-key`).
    pub(crate) async fn pick_owner(
        &self,
        wallet: WalletId,
        index: Option<u32>,
        used: &InUse,
    ) -> Result<Picked, MasternodesError> {
        let network = self.network.core_network();
        let range = match index {
            Some(i) if i >= KEY_INDEXES => {
                return Err(EngineError::InvalidArgument(format!(
                    "owner index must be below {KEY_INDEXES}"
                ))
                .into());
            }
            Some(i) => (i, 1),
            None => (0, KEY_INDEXES),
        };
        let keys = self
            .keys_of(wallet, MasternodeKeyRole::Owner, range.0, range.1)
            .await?;
        for k in keys {
            let Some(picked) = secp_pick(&k, network) else {
                continue;
            };
            if !used.owner.contains(&picked.key_id) {
                return Ok(picked);
            }
            if index.is_some() {
                return Err(MasternodesError::ProtxRefused {
                    reason: "bad-protx-dup-key".into(),
                    operation: None,
                });
            }
        }
        Err(exhausted("owner"))
    }

    /// The keychain owner index 0–19 whose key id is `key_id` (D12).
    pub(crate) async fn keychain_owner(
        &self,
        wallet: WalletId,
        key_id: [u8; 20],
    ) -> Result<Option<u32>, MasternodesError> {
        let network = self.network.core_network();
        let keys = self
            .keys_of(wallet, MasternodeKeyRole::Owner, 0, KEY_INDEXES)
            .await?;
        Ok(keys
            .iter()
            .filter_map(|k| secp_pick(k, network))
            .find(|p| p.key_id == key_id)
            .and_then(|p| p.index))
    }

    pub(crate) async fn pick_voting(
        &self,
        wallet: WalletId,
        choice: &VotingChoice,
        used: &InUse,
    ) -> Result<Picked, MasternodesError> {
        let network = self.network.core_network();
        let (start, count) = match choice {
            VotingChoice::Address { address } => {
                let key_id = p2pkh_id(address, network).ok_or_else(|| {
                    EngineError::InvalidArgument("the voting key is not a P2PKH address".into())
                })?;
                return Ok(Picked {
                    index: None,
                    address: address.trim().to_string(),
                    key_id,
                });
            }
            VotingChoice::Keychain { index } if *index >= KEY_INDEXES => {
                return Err(EngineError::InvalidArgument(format!(
                    "voting index must be below {KEY_INDEXES}"
                ))
                .into());
            }
            VotingChoice::Keychain { index } => (*index, 1),
            VotingChoice::NextUnused => (0, KEY_INDEXES),
        };
        let keys = self
            .keys_of(wallet, MasternodeKeyRole::Voting, start, count)
            .await?;
        let next_unused = *choice == VotingChoice::NextUnused;
        keys.iter()
            .filter_map(|k| secp_pick(k, network))
            .find(|p| {
                !next_unused || !(used.voting.contains(&p.key_id) || used.owner.contains(&p.key_id))
            })
            .ok_or_else(|| exhausted("voting"))
    }

    pub(crate) async fn pick_operator(
        &self,
        wallet: WalletId,
        choice: &OperatorChoice,
        used: &InUse,
    ) -> Result<PickedOperator, MasternodesError> {
        let (start, count) = match choice {
            OperatorChoice::Given { public_key } => {
                return Ok(PickedOperator {
                    index: None,
                    public_key: parse_operator_key(public_key)?,
                });
            }
            OperatorChoice::Keychain { index } => (*index, 1),
            OperatorChoice::NextUnused => (0, SCAN_INDEXES),
        };
        let keys = self
            .keys_of(wallet, MasternodeKeyRole::Operator, start, count)
            .await?;
        let next_unused = *choice == OperatorChoice::NextUnused;
        keys.iter()
            .filter_map(|k| {
                let mut key = [0u8; 48];
                hex::decode_to_slice(&k.public_key_hex, &mut key).ok()?;
                Some(PickedOperator {
                    index: Some(k.index),
                    public_key: key,
                })
            })
            .find(|p| !next_unused || !used.holds_operator(&p.public_key))
            .ok_or_else(|| exhausted("operator"))
    }

    pub(crate) async fn pick_platform_node(
        &self,
        wallet: WalletId,
        choice: &PlatformNodeChoice,
        used: &InUse,
    ) -> Result<PickedNode, MasternodesError> {
        let (start, count) = match choice {
            PlatformNodeChoice::Given { node_id } => {
                return Ok(PickedNode {
                    index: None,
                    node_id: parse_node_id(node_id)?,
                });
            }
            PlatformNodeChoice::Keychain { index } => (*index, 1),
            PlatformNodeChoice::NextUnused => (0, SCAN_INDEXES),
        };
        let keys = self
            .keys_of(wallet, MasternodeKeyRole::PlatformNode, start, count)
            .await?;
        let next_unused = *choice == PlatformNodeChoice::NextUnused;
        keys.iter()
            .filter_map(|k| {
                Some(PickedNode {
                    index: Some(k.index),
                    node_id: hex20(k.platform_node_id.as_deref()?).ok()?,
                })
            })
            .find(|p| !next_unused || !used.node_ids.contains(&p.node_id))
            .ok_or_else(|| exhausted("platform_node"))
    }

    /// The defaults a registration of `evonode` would pick (D13).
    pub(crate) async fn default_keys(
        &self,
        wallet: WalletId,
        evonode: bool,
        used: &InUse,
    ) -> Result<ProTxKeys, MasternodesError> {
        let owner = self.pick_owner(wallet, None, used).await?;
        let voting = self
            .pick_voting(wallet, &VotingChoice::NextUnused, used)
            .await?;
        let operator = self
            .pick_operator(wallet, &OperatorChoice::NextUnused, used)
            .await?;
        let platform_node = if evonode {
            let node = self
                .pick_platform_node(wallet, &PlatformNodeChoice::NextUnused, used)
                .await?;
            Some(KeyPick {
                index: node.index.unwrap_or_default(),
                address: None,
                public_key: None,
                node_id: Some(hex::encode(node.node_id)),
            })
        } else {
            None
        };
        let secp = |p: Picked| KeyPick {
            index: p.index.unwrap_or_default(),
            address: Some(p.address),
            public_key: None,
            node_id: None,
        };
        Ok(ProTxKeys {
            owner: secp(owner),
            voting: secp(voting),
            operator: KeyPick {
                index: operator.index.unwrap_or_default(),
                address: None,
                public_key: Some(hex::encode(operator.public_key)),
                node_id: None,
            },
            platform_node,
        })
    }
}

fn secp_pick(k: &MasternodeKeyInfo, network: Network) -> Option<Picked> {
    let address = k.address.clone()?;
    Some(Picked {
        index: Some(k.index),
        key_id: p2pkh_id(&address, network)?,
        address,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_shaped_keys_are_refused_without_echo() {
        let secret = "ab".repeat(32);
        for e in [
            parse_operator_key(&secret).unwrap_err(),
            parse_node_id(&secret).unwrap_err(),
        ] {
            assert_eq!(e.code(), "invalid_argument");
            assert!(!e.to_string().contains(&secret));
            assert!(e.to_string().contains("secret"));
        }
        assert!(parse_operator_key(&"ab".repeat(48)).is_err());
    }

    #[test]
    fn outpoints_parse_as_core_prints_them() {
        let txid = "aa".repeat(32);
        let o = parse_outpoint(&format!("{txid}-3")).unwrap();
        assert_eq!((o.txid.to_string(), o.vout), (txid.clone(), 3));
        for bad in ["", "x-1", &txid.to_string(), &format!("{txid}-x")] {
            assert_eq!(parse_outpoint(bad).unwrap_err().code(), "invalid_argument");
        }
    }
}
