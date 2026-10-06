//! M3 masternode keychain (IOS-083), tracked masternodes with attached keys
//! (IOS-082) and evonode tools (IOS-081). Owner: R3 (platform-wallet's
//! tracked masternodes and withdrawal, `dw-protx` keychain). Contract:
//! docs/contracts/m3-engine.md §2.5.
//!
//! Private keys leave the engine only through `Vault.reveal_masternode_key`
//! (a `RevealSecret` grant), as the mnemonic does through
//! `Vault.reveal_mnemonic`. Keys attached to tracked masternodes are kept
//! in the vault, encrypted under the data key (DESIGN-opus §1.8).
//!
//! Evonode credit status and withdrawal need Platform queries and state
//! transitions (iOS: `fetchClaimableBalance`, `masternodeWithdraw`). They
//! are part of this contract but may land with M4's Platform work; until
//! then they return `NotImplemented` with a `.platform` call name.

use zeroize::Zeroizing;

use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
use crate::api::masternode::parse_pro_tx_hash;
use crate::{MasternodeError, MasternodeKeyRole, MasternodeRow, NetworkSession, Vault};

fn tracked(t: dw_engine::masternodes::TrackedMasternodeInfo) -> TrackedMasternode {
    TrackedMasternode {
        row: t.row.into(),
        label: t.label,
        attached_roles: t.attached_roles.into_iter().map(Into::into).collect(),
        capabilities: TrackedCapabilities {
            can_withdraw: t.capabilities.can_withdraw,
            can_update_service: t.capabilities.can_update_service,
            can_update_registrar: t.capabilities.can_update_registrar,
            can_vote: t.capabilities.can_vote,
        },
    }
}

/// Where a keychain key is used (IOS-083 "Used at ip:port" / revoked).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeKeyUsage {
    pub pro_tx_hash: String,
    pub service: Option<String>,
    pub revoked: bool,
}

/// One derived provider key (public data only).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeKeyInfo {
    pub role: MasternodeKeyRole,
    pub index: u32,
    /// e.g. `m/9'/5'/3'/1'/0'` (DIP3/DIP9 provider key paths).
    pub derivation_path: String,
    /// P2PKH address for secp256k1 roles.
    pub address: Option<String>,
    pub public_key_hex: String,
    /// Legacy BLS serialization of an operator key.
    pub legacy_public_key_hex: Option<String>,
    /// Tenderdash node id of a platform node key (40 hex).
    pub platform_node_id: Option<String>,
    pub used_by: Vec<MasternodeKeyUsage>,
}

/// A revealed provider private key. ASCII bytes, held by the host in a
/// zeroing buffer.
#[derive(uniffi::Record)]
pub struct RevealedMasternodeKey {
    pub private_key_hex: Vec<u8>,
    /// WIF for secp256k1 roles.
    pub wif: Option<Vec<u8>>,
    /// The Tenderdash `priv_validator_key`/node key form for a platform
    /// node key (base64).
    pub tenderdash_key: Option<Vec<u8>>,
}

/// What a tracked masternode's attached keys allow (platform-wallet
/// `MasternodeCapabilities`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TrackedCapabilities {
    /// Owner or payout key: evonode credit withdrawal.
    pub can_withdraw: bool,
    /// Operator key: Update Service / unban, revoke.
    pub can_update_service: bool,
    /// Owner key: Update Registrar.
    pub can_update_registrar: bool,
    /// Voting key: governance and contested-name votes.
    pub can_vote: bool,
}

/// A tracked masternode (IOS-082).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TrackedMasternode {
    pub row: MasternodeRow,
    pub label: Option<String>,
    pub attached_roles: Vec<MasternodeKeyRole>,
    pub capabilities: TrackedCapabilities,
}

/// Evonode Platform status (IOS-080 claimable balance, epoch blocks;
/// IOS-081 status request).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EvonodePlatformStatus {
    pub pro_tx_hash: String,
    /// Owner identity's claimable credits.
    pub claimable_credits: Option<u64>,
    /// Blocks the node proposed in the current epoch.
    pub epoch_proposed_blocks: Option<u32>,
    pub epoch_index: Option<u32>,
}

/// Where withdrawn evonode credits go (iOS withdrawal sheet).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CreditWithdrawalDestination {
    /// The registered payout address (no choice with only the payout key).
    PayoutAddress,
    /// Any Core address (needs the owner key).
    Address { address: String },
}

#[uniffi::export]
impl NetworkSession {
    /// Derived provider keys of `role` for indexes `start..start+count`
    /// (count ≤ 100), with where each is used. Public data, no grant.
    pub async fn masternode_keys(
        &self,
        wallet_id: String,
        role: MasternodeKeyRole,
        start: u32,
        count: u32,
    ) -> Result<Vec<MasternodeKeyInfo>, MasternodeError> {
        let id = parse_wallet_id(&wallet_id)?;
        if count > 100 {
            return Err(MasternodeError::InvalidArgument {
                detail: "count must be at most 100".to_string(),
            });
        }
        ensure_open(&self.inner)?;
        let keys = self
            .inner
            .masternode_keys(id, role.into(), start, count)
            .await?;
        Ok(keys
            .into_iter()
            .map(|k| MasternodeKeyInfo {
                role: k.role.into(),
                index: k.index,
                derivation_path: k.derivation_path,
                address: k.address,
                public_key_hex: k.public_key_hex,
                legacy_public_key_hex: k.legacy_public_key_hex,
                platform_node_id: k.platform_node_id,
                used_by: k
                    .used_by
                    .into_iter()
                    .map(|u| MasternodeKeyUsage {
                        pro_tx_hash: u.pro_tx_hash,
                        service: u.service,
                        revoked: u.revoked,
                    })
                    .collect(),
            })
            .collect())
    }

    /// Finds masternodes in the list by IP, `IP:port`, proTxHash, owner /
    /// voting / payout address or operator key (IOS-082 "Track any
    /// masternode").
    pub async fn locate_masternodes(
        &self,
        query: String,
    ) -> Result<Vec<MasternodeRow>, MasternodeError> {
        ensure_open(&self.inner)?;
        let rows = self.inner.locate_masternodes(query).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn tracked_masternodes(&self) -> Result<Vec<TrackedMasternode>, MasternodeError> {
        ensure_open(&self.inner)?;
        let rows = self.inner.tracked_masternodes().await?;
        Ok(rows.into_iter().map(tracked).collect())
    }

    pub async fn track_masternode(
        &self,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<TrackedMasternode, MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(tracked(
            self.inner.track_masternode(pro_tx_hash, label).await?,
        ))
    }

    /// Stops tracking and deletes its attached keys from the vault.
    /// `false` when it was not tracked.
    pub async fn untrack_masternode(&self, pro_tx_hash: String) -> Result<bool, MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(self.inner.untrack_masternode(pro_tx_hash).await?)
    }

    pub async fn set_tracked_masternode_label(
        &self,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<(), MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(self
            .inner
            .set_tracked_masternode_label(pro_tx_hash, label)
            .await?)
    }

    /// Attaches a private key to a tracked masternode: WIF or hex for
    /// secp256k1 roles, hex for BLS, hex/base64 for ed25519. The key must
    /// match the masternode's registered key for `role`
    /// (`masternode.invalid_key`). Stored in the vault; `MasternodeOp`
    /// grant. The bytes are wiped when the call returns.
    pub async fn attach_masternode_key(
        &self,
        pro_tx_hash: String,
        role: MasternodeKeyRole,
        key: Vec<u8>,
        grant_id: String,
    ) -> Result<(), MasternodeError> {
        let key = Zeroizing::new(key);
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(self
            .inner
            .attach_masternode_key(pro_tx_hash, role.into(), key, grant_id)
            .await?)
    }

    pub async fn detach_masternode_key(
        &self,
        pro_tx_hash: String,
        role: MasternodeKeyRole,
    ) -> Result<(), MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        Ok(self
            .inner
            .detach_masternode_key(pro_tx_hash, role.into())
            .await?)
    }

    /// Evonode Platform status (IOS-080/081). Platform query: may land in
    /// M4 (`NotImplemented { call: "NetworkSession.evonode_status.platform" }`
    /// until then).
    pub async fn evonode_status(
        &self,
        pro_tx_hash: String,
    ) -> Result<EvonodePlatformStatus, MasternodeError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.evonode_status.platform")
    }

    /// Withdraws evonode credits to Core (IOS-081, tracked withdraw
    /// IOS-082). Signs with the owner or payout key a wallet derives or a
    /// tracked masternode has attached; `MasternodeOp` grant. Platform state
    /// transition: may land in M4 (`.platform` call name until then).
    /// Returns the withdrawal's state-transition id.
    pub async fn withdraw_evonode_credits(
        &self,
        pro_tx_hash: String,
        amount: u64,
        destination: CreditWithdrawalDestination,
        grant_id: String,
    ) -> Result<String, MasternodeError> {
        let _ = (amount, destination, grant_id);
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.withdraw_evonode_credits.platform")
    }
}

#[uniffi::export]
impl Vault {
    /// Reveals one provider private key of a wallet (IOS-083) or attached
    /// to a tracked masternode (`wallet_id = None`, `index` ignored).
    /// `RevealSecret` grant.
    pub async fn reveal_masternode_key(
        &self,
        wallet_id: Option<String>,
        pro_tx_hash: Option<String>,
        role: MasternodeKeyRole,
        index: u32,
        grant_id: String,
    ) -> Result<RevealedMasternodeKey, MasternodeError> {
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        if let Some(h) = &pro_tx_hash {
            parse_pro_tx_hash(h)?;
        }
        if wallet_id.is_some() == pro_tx_hash.is_some() {
            return Err(MasternodeError::InvalidArgument {
                detail: "pass exactly one of wallet_id and pro_tx_hash".to_string(),
            });
        }
        ensure_open(&self.session)?;
        let revealed = self
            .session
            .reveal_masternode_key(wallet, pro_tx_hash, role.into(), index, grant_id)
            .await?;
        Ok(RevealedMasternodeKey {
            private_key_hex: revealed.private_key_hex.to_vec(),
            wif: revealed.wif.map(|w| w.to_vec()),
            tenderdash_key: revealed.tenderdash_key.map(|t| t.to_vec()),
        })
    }
}
