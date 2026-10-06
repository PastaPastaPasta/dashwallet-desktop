//! The masternode keychain (IOS-083), tracked masternodes with attached
//! keys (IOS-082) and the list search (`locate_masternodes`).
//!
//! Provider keys derive on their DIP3 paths (`dw_protx::keychain`) through
//! platform-wallet's `derive_provider_key_at_index`; public keys need no
//! seed except ed25519 platform node keys, which are read from the wallet's
//! pre-derived platform-node pool. A private key leaves the engine only
//! through [`NetworkSession::reveal_masternode_key`] under a `RevealSecret`
//! grant. Keys attached to tracked masternodes live in the vault
//! (`mnkey/<proTxHash>/<role>` records) and are checked against the
//! masternode's registered key before they are stored.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dw_protx::keychain::{self, ProviderRole};
use dw_vault::GrantKind;
use key_wallet::managed_account::address_pool::PublicKeyType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use platform_wallet::masternode::locator::p2pkh_script_hash;
use platform_wallet::masternode::{
    KeyVerification, LocatorSecret, MasternodeKeyReference, MasternodeKeyRole as PwRole,
    parse_secret_for_role, verify_masternode_key,
};
use platform_wallet::wallet::provider_key_at_index::ProviderKeyKind;
use zeroize::{Zeroize, Zeroizing};

use super::list::{Known, MasternodeListStatus, MasternodeRow, display_hex, parse_pro_tx_hash};
use super::protx::vault_failure;
use crate::send::l1_address;
use crate::{EngineError, MasternodeFailure, MasternodeKeyRole, NetworkSession, WalletId};

/// Where a keychain key is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeKeyUsage {
    pub pro_tx_hash: String,
    pub service: Option<String>,
    pub revoked: bool,
}

/// One derived provider key (public data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeKeyInfo {
    pub role: MasternodeKeyRole,
    pub index: u32,
    pub derivation_path: String,
    pub address: Option<String>,
    pub public_key_hex: String,
    pub legacy_public_key_hex: Option<String>,
    pub platform_node_id: Option<String>,
    pub used_by: Vec<MasternodeKeyUsage>,
}

/// A revealed private key, ASCII in zeroing buffers.
pub struct RevealedMasternodeKey {
    pub private_key_hex: Zeroizing<Vec<u8>>,
    pub wif: Option<Zeroizing<Vec<u8>>>,
    pub tenderdash_key: Option<Zeroizing<Vec<u8>>>,
}

/// What the attached keys of a tracked masternode allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackedCapabilities {
    pub can_withdraw: bool,
    pub can_update_service: bool,
    pub can_update_registrar: bool,
    pub can_vote: bool,
}

/// A tracked masternode (IOS-082).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedMasternodeInfo {
    pub row: MasternodeRow,
    pub label: Option<String>,
    pub attached_roles: Vec<MasternodeKeyRole>,
    pub capabilities: TrackedCapabilities,
}

const ROLES: [MasternodeKeyRole; 6] = [
    MasternodeKeyRole::Owner,
    MasternodeKeyRole::Voting,
    MasternodeKeyRole::Operator,
    MasternodeKeyRole::PlatformNode,
    MasternodeKeyRole::OwnerPayout,
    MasternodeKeyRole::OperatorPayout,
];

/// The vault record name of an attached key's role.
pub(crate) fn role_name(role: MasternodeKeyRole) -> &'static str {
    match role {
        MasternodeKeyRole::Owner => "owner",
        MasternodeKeyRole::Voting => "voting",
        MasternodeKeyRole::Operator => "operator",
        MasternodeKeyRole::PlatformNode => "platform_node",
        MasternodeKeyRole::OwnerPayout => "owner_payout",
        MasternodeKeyRole::OperatorPayout => "operator_payout",
    }
}

fn role_from_name(name: &str) -> Option<MasternodeKeyRole> {
    ROLES.into_iter().find(|r| role_name(*r) == name)
}

fn pw_role(role: MasternodeKeyRole) -> PwRole {
    match role {
        MasternodeKeyRole::Owner => PwRole::Owner,
        MasternodeKeyRole::Voting => PwRole::Voting,
        MasternodeKeyRole::Operator => PwRole::Operator,
        MasternodeKeyRole::PlatformNode => PwRole::PlatformNode,
        MasternodeKeyRole::OwnerPayout => PwRole::OwnerPayout,
        MasternodeKeyRole::OperatorPayout => PwRole::OperatorPayout,
    }
}

/// The DIP3 family of a keychain role; payout keys are BIP44 addresses.
fn provider_role(role: MasternodeKeyRole) -> Result<(ProviderRole, ProviderKeyKind), EngineError> {
    match role {
        MasternodeKeyRole::Owner => Ok((ProviderRole::Owner, ProviderKeyKind::Owner)),
        MasternodeKeyRole::Voting => Ok((ProviderRole::Voting, ProviderKeyKind::Voting)),
        MasternodeKeyRole::Operator => Ok((ProviderRole::Operator, ProviderKeyKind::Operator)),
        MasternodeKeyRole::PlatformNode => {
            Ok((ProviderRole::PlatformNode, ProviderKeyKind::PlatformNode))
        }
        MasternodeKeyRole::OwnerPayout | MasternodeKeyRole::OperatorPayout => {
            Err(EngineError::InvalidArgument(
                "payout keys are BIP44 addresses of the wallet, not provider keys".into(),
            ))
        }
    }
}

/// Capabilities of a set of attached roles (platform-wallet
/// `capabilities_for_roles`, plus Update Registrar with the owner key).
fn capabilities(roles: &[MasternodeKeyRole]) -> TrackedCapabilities {
    let caps =
        platform_wallet::masternode::capabilities_for_roles(roles.iter().map(|r| pw_role(*r)));
    TrackedCapabilities {
        can_withdraw: caps.can_withdraw,
        can_update_service: caps.can_update_service,
        can_update_registrar: roles.contains(&MasternodeKeyRole::Owner),
        can_vote: caps.can_vote,
    }
}

/// What a key can be checked against for `known`.
fn key_reference(known: &Known) -> MasternodeKeyReference {
    MasternodeKeyReference {
        owner_key_hash: known.owner_key_hash(),
        voting_key_id: known.voting_key_hash(),
        operator_public_key: known.operator_public_key(),
        platform_node_id: known.platform_node_id(),
        payout_key_hash: known
            .payout_script()
            .and_then(|s| p2pkh_script_hash(s.as_bytes())),
        operator_payout_key_hash: known
            .wallet
            .as_ref()
            .and_then(|w| w.operator_payout.as_ref())
            .and_then(|s| p2pkh_script_hash(s.as_bytes())),
    }
}

fn hash160(bytes: &[u8]) -> [u8; 20] {
    dashcore::hashes::hash160::Hash::hash(bytes).to_byte_array()
}

/// Revealed forms of a parsed secret.
fn revealed(secret: &LocatorSecret, network: dashcore::Network) -> RevealedMasternodeKey {
    match secret {
        LocatorSecret::Ecdsa { secret, compressed } => {
            let wif = dashcore::PrivateKey {
                compressed: *compressed,
                network,
                inner: dashcore::secp256k1::SecretKey::from_slice(&secret[..])
                    .expect("parsed secp256k1 secrets are valid"),
            }
            .to_wif();
            RevealedMasternodeKey {
                private_key_hex: Zeroizing::new(hex::encode(&secret[..]).into_bytes()),
                wif: Some(Zeroizing::new(wif.into_bytes())),
                tenderdash_key: None,
            }
        }
        LocatorSecret::Bls(secret) => RevealedMasternodeKey {
            private_key_hex: Zeroizing::new(hex::encode(&secret[..]).into_bytes()),
            wif: None,
            tenderdash_key: None,
        },
        LocatorSecret::Ed25519(seed) => RevealedMasternodeKey {
            private_key_hex: Zeroizing::new(hex::encode(&seed[..]).into_bytes()),
            wif: None,
            tenderdash_key: Some(Zeroizing::new(
                keychain::tenderdash_private_key(seed).as_bytes().to_vec(),
            )),
        },
    }
}

impl NetworkSession {
    /// Derived provider keys of `role` for `start..start+count` (≤ 100),
    /// with the masternodes that use them (IOS-083). Platform node keys come
    /// from the wallet's pre-derived pool; indexes past it are left out.
    pub async fn masternode_keys(
        self: &Arc<Self>,
        wallet_id: WalletId,
        role: MasternodeKeyRole,
        start: u32,
        count: u32,
    ) -> Result<Vec<MasternodeKeyInfo>, EngineError> {
        if count > 100 {
            return Err(EngineError::InvalidArgument(
                "count must be at most 100".into(),
            ));
        }
        let (family, kind) = provider_role(role)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let network = this.network.core_network();
            let wallet = this.wallet(&wallet_id).await?;
            let end = start.saturating_add(count);
            let mut keys: Vec<MasternodeKeyInfo> = if kind == ProviderKeyKind::PlatformNode {
                let state = wallet.state().await;
                let mut found = Vec::new();
                if let Some(acct) = state.core_wallet.accounts.provider_platform_keys.as_ref() {
                    for pool in acct.managed_account_type().address_pools() {
                        for entry in pool.addresses.values() {
                            if !(start..end).contains(&entry.index) {
                                continue;
                            }
                            if let Some(PublicKeyType::EdDSA(pk)) = &entry.public_key
                                && let Ok(pk32) = <[u8; 32]>::try_from(pk.as_slice())
                            {
                                found.push(MasternodeKeyInfo {
                                    role,
                                    index: entry.index,
                                    derivation_path: keychain::path(family, network, entry.index),
                                    address: None,
                                    public_key_hex: hex::encode(pk32),
                                    legacy_public_key_hex: None,
                                    platform_node_id: Some(keychain::platform_node_id_hex(&pk32)),
                                    used_by: Vec::new(),
                                });
                            }
                        }
                    }
                }
                found.sort_by_key(|k| k.index);
                found
            } else {
                let wallet = Arc::clone(&wallet);
                tokio::task::spawn_blocking(move || {
                    (start..end)
                        .map(|index| {
                            let key =
                                wallet.derive_provider_key_at_index(kind, index, None, false)?;
                            Ok(MasternodeKeyInfo {
                                role,
                                index,
                                derivation_path: keychain::path(family, network, index),
                                address: key.address.clone(),
                                public_key_hex: hex::encode(&key.public_key_bytes),
                                legacy_public_key_hex: key
                                    .legacy_public_key_bytes
                                    .as_ref()
                                    .map(hex::encode),
                                platform_node_id: None,
                                used_by: Vec::new(),
                            })
                        })
                        .collect::<Result<Vec<_>, platform_wallet::PlatformWalletError>>()
                })
                .await??
            };
            let snap = this.masternode_snapshot().await?;
            for key in &mut keys {
                let public = hex::decode(&key.public_key_hex).unwrap_or_default();
                for k in snap.known.values() {
                    let uses = match role {
                        MasternodeKeyRole::Owner => k.owner_key_hash() == Some(hash160(&public)),
                        MasternodeKeyRole::Voting => k.voting_key_hash() == Some(hash160(&public)),
                        MasternodeKeyRole::Operator => k.operator_public_key().is_some_and(|op| {
                            op[..] == public[..]
                                || key
                                    .legacy_public_key_hex
                                    .as_deref()
                                    .is_some_and(|l| hex::encode(op) == l)
                        }),
                        MasternodeKeyRole::PlatformNode => {
                            k.platform_node_id().map(hex::encode) == key.platform_node_id
                        }
                        _ => false,
                    };
                    if uses {
                        let row = k.row(snap.network, snap.list_available);
                        key.used_by.push(MasternodeKeyUsage {
                            pro_tx_hash: row.pro_tx_hash,
                            service: row.service,
                            revoked: matches!(row.status, MasternodeListStatus::Retired)
                                || k.wallet.as_ref().is_some_and(|w| w.record.revoked),
                        });
                    }
                }
            }
            Ok(keys)
        })
        .await
    }

    /// One provider private key (IOS-083 / IOS-082) under a `RevealSecret`
    /// grant: a wallet's derived key at `index`, or the key attached to a
    /// tracked masternode.
    pub async fn reveal_masternode_key(
        self: &Arc<Self>,
        wallet_id: Option<WalletId>,
        pro_tx_hash: Option<String>,
        role: MasternodeKeyRole,
        index: u32,
        grant_id: String,
    ) -> Result<RevealedMasternodeKey, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let network = this.network.core_network();
            match (wallet_id, pro_tx_hash) {
                (Some(wallet_id), None) => {
                    let (_, kind) = provider_role(role)?;
                    let wallet = this.wallet(&wallet_id).await?;
                    let vault = this.vault.clone();
                    let derived = tokio::task::spawn_blocking(move || {
                        vault.with_revealed_seed(&wallet_id.0, &grant_id, |seed| {
                            wallet.derive_provider_key_at_index(kind, index, Some(&seed[..]), true)
                        })
                    })
                    .await?
                    .map_err(vault_failure)??;
                    let private = derived.private_key.as_ref().ok_or_else(|| {
                        EngineError::Internal("the derivation returned no private key".into())
                    })?;
                    let mut seed32 = Zeroizing::new([0u8; 32]);
                    let tenderdash = (kind == ProviderKeyKind::PlatformNode && private.len() >= 32)
                        .then(|| {
                            seed32.copy_from_slice(&private[..32]);
                            Zeroizing::new(
                                keychain::tenderdash_private_key(&seed32)
                                    .as_bytes()
                                    .to_vec(),
                            )
                        });
                    Ok(RevealedMasternodeKey {
                        private_key_hex: Zeroizing::new(hex::encode(&private[..]).into_bytes()),
                        wif: derived
                            .private_key_wif
                            .as_ref()
                            .map(|w| Zeroizing::new(w.as_bytes().to_vec())),
                        tenderdash_key: tenderdash,
                    })
                }
                (None, Some(hash_text)) => {
                    let hash = parse_pro_tx_hash(&hash_text)?;
                    let wallet = this
                        .vault
                        .grant_wallet(&grant_id)
                        .ok_or(MasternodeFailure::GrantInvalid)?;
                    let token = this
                        .vault
                        .redeem_grant(&grant_id, GrantKind::RevealSecret, Some(&wallet))
                        .map_err(vault_failure)?;
                    let stored = this
                        .vault
                        .masternode_key(&token, &hash, role_name(role))
                        .map_err(vault_failure)?
                        .ok_or(MasternodeFailure::KeyNotInWallet(role))?;
                    let text = std::str::from_utf8(&stored)
                        .map_err(|_| EngineError::Internal("an attached key is not text".into()))?;
                    let secret = parse_secret_for_role(text, pw_role(role), network)
                        .map_err(|e| EngineError::Internal(format!("attached key: {e}")))?;
                    Ok(revealed(&secret, network))
                }
                _ => Err(EngineError::InvalidArgument(
                    "pass exactly one of wallet_id and pro_tx_hash".into(),
                )),
            }
        })
        .await
    }

    /// IOS-082 "Track any masternode": list search by IP, `IP:port`,
    /// proTxHash, owner/voting/payout address or operator key.
    pub async fn locate_masternodes(
        self: &Arc<Self>,
        query: String,
    ) -> Result<Vec<MasternodeRow>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let snap = this.masternode_snapshot().await?;
            if !snap.list_available {
                return Err(MasternodeFailure::ListUnavailable.into());
            }
            let q = query.trim();
            let socket = q.parse::<SocketAddr>().ok();
            let ip = q.parse::<IpAddr>().ok();
            let hash = (q.len() == 64).then(|| parse_pro_tx_hash(q).ok()).flatten();
            let operator = (q.len() == 96)
                .then(|| hex::decode(q).ok())
                .flatten()
                .filter(|b| b.len() == 48);
            let key_id = l1_address(q, snap.network)
                .ok()
                .and_then(|a| match a.payload() {
                    Payload::PubkeyHash(h) => Some(h.to_byte_array()),
                    _ => None,
                });
            let payout_script = l1_address(q, snap.network).ok().map(|a| a.script_pubkey());
            let mut rows: Vec<MasternodeRow> = snap
                .known
                .values()
                .filter(|k| {
                    let service = k.service();
                    socket.is_some_and(|s| service == Some(s))
                        || ip.is_some_and(|ip| service.is_some_and(|s| s.ip() == ip))
                        || hash.is_some_and(|h| h == k.hash)
                        || operator.as_ref().is_some_and(|op| {
                            k.operator_public_key().is_some_and(|mine| {
                                mine[..] == op[..]
                                    || <[u8; 48]>::try_from(op.as_slice())
                                        .ok()
                                        .and_then(|b| dw_protx::bls::legacy_of(&b))
                                        .is_some_and(|l| l == mine)
                            })
                        })
                        || key_id.is_some_and(|id| {
                            k.owner_key_hash() == Some(id) || k.voting_key_hash() == Some(id)
                        })
                        || payout_script
                            .as_ref()
                            .is_some_and(|s| k.payout_script().as_ref() == Some(s))
                })
                .map(|k| k.row(snap.network, snap.list_available))
                .collect();
            rows.sort_by(|a, b| a.pro_tx_hash.cmp(&b.pro_tx_hash));
            Ok(rows)
        })
        .await
    }

    /// Tracked masternodes with their attached roles.
    pub async fn tracked_masternodes(
        self: &Arc<Self>,
    ) -> Result<Vec<TrackedMasternodeInfo>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let snap = this.masternode_snapshot().await?;
            let mut out: Vec<(u64, TrackedMasternodeInfo)> = snap
                .known
                .values()
                .filter_map(|k| {
                    let tracked = k.tracked.as_ref()?;
                    Some((
                        tracked.added_at,
                        this.tracked_info(k, snap.network, snap.list_available),
                    ))
                })
                .collect();
            out.sort_by_key(|(added, info)| (*added, info.row.pro_tx_hash.clone()));
            Ok(out.into_iter().map(|(_, i)| i).collect())
        })
        .await
    }

    fn tracked_info(
        &self,
        known: &Known,
        network: dashcore::Network,
        list_available: bool,
    ) -> TrackedMasternodeInfo {
        let attached: Vec<MasternodeKeyRole> = self
            .vault
            .masternode_key_roles(&known.hash)
            .iter()
            .filter_map(|name| role_from_name(name))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let row = known.row(network, list_available);
        TrackedMasternodeInfo {
            label: row.label.clone(),
            capabilities: capabilities(&attached),
            attached_roles: attached,
            row,
        }
    }

    async fn tracked_known(
        &self,
        hash: &[u8; 32],
    ) -> Result<(Known, super::list::Snapshot), EngineError> {
        let snap = self.masternode_snapshot().await?;
        let known = snap
            .known
            .get(hash)
            .filter(|k| k.tracked.is_some())
            .cloned()
            .ok_or_else(|| MasternodeFailure::NotFound(display_hex(hash)))?;
        Ok((known, snap))
    }

    /// Tracks a masternode (IOS-082). A proTxHash the synced list and the
    /// wallets do not know is `masternode.not_found`.
    pub async fn track_masternode(
        self: &Arc<Self>,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<TrackedMasternodeInfo, EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let service = manager.tracked_masternodes_service();
            if service.get(&hash).is_some() {
                return Err(MasternodeFailure::AlreadyTracked(pro_tx_hash).into());
            }
            let snap = this.masternode_snapshot().await?;
            if snap.list_available && !snap.known.contains_key(&hash) {
                return Err(MasternodeFailure::NotFound(pro_tx_hash).into());
            }
            tokio::task::spawn_blocking(move || service.track_blocking(hash, label)).await??;
            this.announce_masternodes();
            let (known, snap) = this.tracked_known(&hash).await?;
            Ok(this.tracked_info(&known, snap.network, snap.list_available))
        })
        .await
    }

    /// Stops tracking and deletes the attached keys. `false` when it was not
    /// tracked.
    pub async fn untrack_masternode(
        self: &Arc<Self>,
        pro_tx_hash: String,
    ) -> Result<bool, EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let service = manager.tracked_masternodes_service();
            if service.get(&hash).is_none() {
                return Ok(false);
            }
            // Keys first: a locked vault refuses before the row is gone.
            this.vault
                .delete_masternode_keys(&hash, None)
                .map_err(vault_failure)?;
            let removed =
                tokio::task::spawn_blocking(move || service.untrack_blocking(&hash)).await??;
            this.announce_masternodes();
            Ok(removed)
        })
        .await
    }

    pub async fn set_tracked_masternode_label(
        self: &Arc<Self>,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<(), EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let service = this.manager()?.tracked_masternodes_service();
            if service.get(&hash).is_none() {
                return Err(MasternodeFailure::NotFound(pro_tx_hash).into());
            }
            tokio::task::spawn_blocking(move || service.set_label_blocking(&hash, label)).await??;
            this.announce_masternodes();
            Ok(())
        })
        .await
    }

    /// Attaches a private key to a tracked masternode after checking it is
    /// the masternode's registered key for `role`. The grant is a
    /// `MasternodeOp` grant bound to any wallet. `key` is wiped.
    pub async fn attach_masternode_key(
        self: &Arc<Self>,
        pro_tx_hash: String,
        role: MasternodeKeyRole,
        key: Zeroizing<Vec<u8>>,
        grant_id: String,
    ) -> Result<(), EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let network = this.network.core_network();
            let (known, _) = this.tracked_known(&hash).await?;
            let mut text = std::str::from_utf8(&key)
                .map_err(|_| MasternodeFailure::InvalidKey {
                    role,
                    detail: "the key is not text".into(),
                })?
                .trim()
                .to_string();
            let parsed = parse_secret_for_role(&text, pw_role(role), network).map_err(|e| {
                MasternodeFailure::InvalidKey {
                    role,
                    detail: e.to_string(),
                }
            });
            let parsed = match parsed {
                Ok(p) => p,
                Err(e) => {
                    text.zeroize();
                    return Err(e.into());
                }
            };
            match verify_masternode_key(&key_reference(&known), pw_role(role), &parsed) {
                KeyVerification::Matches => {}
                KeyVerification::DoesNotMatch => {
                    text.zeroize();
                    return Err(MasternodeFailure::InvalidKey {
                        role,
                        detail: "the key does not match the masternode's registered key".into(),
                    }
                    .into());
                }
                KeyVerification::Unverifiable => {
                    text.zeroize();
                    return Err(MasternodeFailure::InvalidKey {
                        role,
                        detail: "the masternode's key for this role is not known, so the key \
                                 cannot be checked"
                            .into(),
                    }
                    .into());
                }
            }
            let stored = (|| {
                let wallet = this
                    .vault
                    .grant_wallet(&grant_id)
                    .ok_or(dw_vault::VaultError::GrantInvalid)?;
                let token =
                    this.vault
                        .redeem_grant(&grant_id, GrantKind::MasternodeOp, Some(&wallet))?;
                this.vault
                    .store_masternode_key(&token, &hash, role_name(role), text.as_bytes())
            })();
            text.zeroize();
            stored.map_err(vault_failure)?;
            this.announce_masternodes();
            Ok(())
        })
        .await
    }

    pub async fn detach_masternode_key(
        self: &Arc<Self>,
        pro_tx_hash: String,
        role: MasternodeKeyRole,
    ) -> Result<(), EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.vault
                .delete_masternode_keys(&hash, Some(role_name(role)))
                .map_err(vault_failure)?;
            this.announce_masternodes();
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_IOS_082_role_names_round_trip_and_capabilities() {
        for r in ROLES {
            assert_eq!(role_from_name(role_name(r)), Some(r));
        }
        let caps = capabilities(&[MasternodeKeyRole::Owner, MasternodeKeyRole::Voting]);
        assert!(caps.can_withdraw && caps.can_update_registrar && caps.can_vote);
        assert!(!caps.can_update_service);
        assert!(capabilities(&[MasternodeKeyRole::Operator]).can_update_service);
    }

    #[test]
    fn test_IOS_083_payout_roles_are_not_provider_keys() {
        assert!(provider_role(MasternodeKeyRole::OwnerPayout).is_err());
        assert!(provider_role(MasternodeKeyRole::Operator).is_ok());
    }
}
