//! The tracking calls (mn-engine §4): track and untrack any listed
//! masternode, labels, and the owner, voting and payout keys attached to a
//! tracked one (MG-03 §4, IOS-082). No operator or platform-node key is
//! taken: the vault has no such role (MG-01 D7).

use std::sync::Arc;

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::SecretKey;
use dashcore::{PrivateKey, PubkeyHash, ScriptBuf};
use zeroize::Zeroizing;

use super::follow::{all_owned, read_list, set_label};
use super::reads::list_facts;
use super::records::{AttachedKeyRole, KeyCheck, ListedMasternode, UntrackOutcome};
use super::{Masternodes, MasternodesError, display_hex, parse_pro_tx_hash};
use crate::{EngineError, NetworkSession};

/// Longest label, in characters.
pub const MAX_MASTERNODE_LABEL: usize = 64;

/// A trimmed label; blank clears it.
fn normalize_label(label: Option<String>) -> Result<Option<String>, EngineError> {
    let Some(label) = label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_MASTERNODE_LABEL {
        return Err(EngineError::InvalidArgument(format!(
            "a masternode label has at most {MAX_MASTERNODE_LABEL} characters"
        )));
    }
    Ok(Some(label))
}

/// The secret of a WIF (of the session's network) or of 64 hex characters,
/// and the hash160 of its public key (compressed unless the WIF says
/// otherwise).
fn parse_secret(
    key: &str,
    network: dashcore::Network,
) -> Result<(Zeroizing<[u8; 32]>, [u8; 20]), EngineError> {
    let invalid = || EngineError::InvalidArgument("not a WIF or 64-hex private key".into());
    let key = key.trim();
    let private = if key.len() == 64 {
        let mut bytes = Zeroizing::new([0u8; 32]);
        hex::decode_to_slice(key, &mut bytes[..]).map_err(|_| invalid())?;
        PrivateKey::new(
            SecretKey::from_secret_bytes(*bytes).map_err(|_| invalid())?,
            network,
        )
    } else {
        let private = PrivateKey::from_wif(key).map_err(|_| invalid())?;
        // A WIF's prefix tells mainnet from the test networks only.
        let mainnet = |n| n == dashcore::Network::Mainnet;
        if mainnet(private.network) != mainnet(network) {
            return Err(EngineError::InvalidArgument(format!(
                "the key is for {}, not this network",
                private.network
            )));
        }
        private
    };
    let id = hash160::Hash::hash(&private.public_key().to_bytes()).to_byte_array();
    Ok((Zeroizing::new(private.inner.to_secret_bytes()), id))
}

/// What a role's key is checked against (MG-03 §4, MG-00 D13).
enum Reference {
    /// A key id the key must hash to.
    KeyId([u8; 20]),
    /// A P2PKH payout script the key may pay. The wallet's payout script can
    /// be stale (a ProUpRegTx it doesn't hold, D5), so a key that doesn't
    /// pay it is `unchecked`, not refused (D19).
    Payout(ScriptBuf),
    Nothing,
}

impl Reference {
    fn check(&self, id: [u8; 20], role: AttachedKeyRole) -> Result<KeyCheck, MasternodesError> {
        match self {
            Self::KeyId(want) if *want == id => Ok(KeyCheck::Matched),
            Self::KeyId(_) => Err(MasternodesError::KeyMismatch { role }),
            Self::Payout(script)
                if *script == ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id)) =>
            {
                Ok(KeyCheck::Matched)
            }
            Self::Payout(_) | Self::Nothing => Ok(KeyCheck::Unchecked),
        }
    }
}

/// The reference for `role` of `hash`: the list's voting key, else (owner,
/// payout) what a loaded wallet's ProRegTx/ProUpRegTx says. Caller holds
/// the guard.
async fn reference(
    session: &NetworkSession,
    hash: [u8; 32],
    role: AttachedKeyRole,
) -> Result<Reference, MasternodesError> {
    let manager = session.manager()?;
    if role == AttachedKeyRole::Voting {
        let list = read_list(session, &manager).await;
        if !list.synced {
            return Err(MasternodesError::ListUnavailable);
        }
        return Ok(match list.entry(&hash) {
            Some(e) => Reference::KeyId(e.voting_key_id),
            None => Reference::Nothing,
        });
    }
    let hub = Arc::clone(&session.hub);
    Ok(tokio::task::spawn_blocking(move || {
        let record = all_owned(&manager, &hub.history)
            .into_iter()
            .find(|o| o.record.pro_tx_hash == hash)
            .map(|o| o.record);
        if role == AttachedKeyRole::Owner {
            record
                .and_then(|r| r.owner_key_hash)
                .map_or(Reference::Nothing, Reference::KeyId)
        } else {
            record
                .and_then(|r| r.payout_script)
                .map(ScriptBuf::from)
                .filter(|s| s.is_p2pkh())
                .map_or(Reference::Nothing, Reference::Payout)
        }
    })
    .await?)
}

impl Masternodes {
    fn require_tracked(&self, hash: &[u8; 32]) -> Result<(), MasternodesError> {
        match self.session.manager()?.tracked_masternode(hash) {
            Some(_) => Ok(()),
            None => Err(MasternodesError::NotTracked {
                pro_tx_hash: display_hex(hash),
            }),
        }
    }

    /// Tracks a masternode of the synced list (MG-03 D7); every wallet's
    /// overview lists it.
    pub async fn track(
        &self,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<ListedMasternode, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let label = normalize_label(label)?;
        let _mutation = self.session.masternodes.mutation().await;
        let listed = self
            .op(move |session| async move {
                let manager = session.manager()?;
                let list = read_list(&session, &manager).await;
                if !list.synced {
                    return Err(MasternodesError::ListUnavailable);
                }
                let entry = list
                    .entry(&hash)
                    .ok_or_else(|| MasternodesError::NotFound {
                        query: display_hex(&hash),
                    })?;
                let facts = list_facts(entry, session.network.core_network());
                if manager.tracked_masternode(&hash).is_some() {
                    return Err(MasternodesError::AlreadyTracked {
                        pro_tx_hash: display_hex(&hash),
                    });
                }
                let service = manager.tracked_masternodes_service();
                tokio::task::spawn_blocking(move || {
                    service
                        .track_blocking(hash, label)
                        .map_err(EngineError::from)
                })
                .await??;
                Ok(ListedMasternode {
                    pro_tx_hash: display_hex(&hash),
                    facts,
                    tracked: true,
                })
            })
            .await?;
        self.session.masternodes.local_change();
        Ok(listed)
    }

    /// Stops tracking: the attached keys are deleted first (MG-00 D22), so
    /// a vault that can't delete them (locked) leaves it tracked.
    pub async fn untrack(&self, pro_tx_hash: String) -> Result<UntrackOutcome, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let _mutation = self.session.masternodes.mutation().await;
        self.require_tracked(&hash)?;
        let keys_deleted = self
            .session
            .vault_op(move |v| v.delete_masternode_keys(&hash, None))
            .await?;
        let untracked = self
            .op(move |session| async move {
                let service = session.manager()?.tracked_masternodes_service();
                tokio::task::spawn_blocking(move || {
                    service.untrack_blocking(&hash).map_err(EngineError::from)
                })
                .await??;
                Ok(())
            })
            .await;
        // the keys are gone even when the registry kept the row
        self.session.masternodes.local_change();
        untracked?;
        Ok(UntrackOutcome {
            keys_deleted: u32::try_from(keys_deleted).unwrap_or(u32::MAX),
        })
    }

    /// Names a masternode (`None` or blank: back to the default name). A
    /// tracked one's label is the registry's; a wallet masternode's is kept
    /// in app.sqlite (MG-03 D12).
    pub async fn set_label(
        &self,
        pro_tx_hash: String,
        label: Option<String>,
    ) -> Result<(), MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let label = normalize_label(label)?;
        let _mutation = self.session.masternodes.mutation().await;
        self.op(move |session| async move {
            let manager = session.manager()?;
            if manager.tracked_masternode(&hash).is_some() {
                let service = manager.tracked_masternodes_service();
                tokio::task::spawn_blocking(move || {
                    service
                        .set_label_blocking(&hash, label)
                        .map_err(EngineError::from)
                })
                .await??;
            } else {
                let key = display_hex(&hash);
                session
                    .appdb_op(move |db| set_label(db, &key, label.as_deref()))
                    .await?;
            }
            Ok(())
        })
        .await?;
        self.session.masternodes.local_change();
        Ok(())
    }

    /// Attaches the private key of `role` to a tracked masternode, checked
    /// as far as the wallet can (MG-03 §4): a voting key against the list,
    /// an owner key or payout key against a ProRegTx a loaded wallet holds.
    /// Needs the vault's full-scope data key.
    pub async fn attach_key(
        &self,
        pro_tx_hash: String,
        role: AttachedKeyRole,
        key: Zeroizing<String>,
    ) -> Result<KeyCheck, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let (secret, id) = parse_secret(&key, self.session.network.core_network())?;
        drop(key);
        let _mutation = self.session.masternodes.mutation().await;
        self.require_tracked(&hash)?;
        let check =
            self.op(move |session| async move {
                reference(&session, hash, role).await?.check(id, role)
            })
            .await?;
        self.session
            .vault_op(move |v| v.store_masternode_key(&hash, role.vault_name(), &secret[..]))
            .await?;
        self.session.masternodes.local_change();
        Ok(check)
    }

    /// Deletes the key attached for `role`; `false` when none was.
    pub async fn detach_key(
        &self,
        pro_tx_hash: String,
        role: AttachedKeyRole,
    ) -> Result<bool, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let _mutation = self.session.masternodes.mutation().await;
        self.require_tracked(&hash)?;
        let deleted = self
            .session
            .vault_op(move |v| v.delete_masternode_keys(&hash, Some(role.vault_name())))
            .await?;
        self.session.masternodes.local_change();
        Ok(deleted > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: dashcore::Network = dashcore::Network::Regtest;

    #[test]
    fn secrets_parse_from_hex_and_wif_of_the_network() {
        let hex_key = "01".repeat(32);
        let (secret, id) = parse_secret(&hex_key, NET).unwrap();
        assert_eq!(&secret[..], &[1u8; 32]);
        let wif = PrivateKey::new(SecretKey::from_secret_bytes([1; 32]).unwrap(), NET).to_wif();
        assert_eq!(
            parse_secret(&wif, NET).unwrap().1,
            id,
            "the same key, compressed"
        );
        let mainnet = PrivateKey::new(
            SecretKey::from_secret_bytes([1; 32]).unwrap(),
            dashcore::Network::Mainnet,
        );
        assert_eq!(
            parse_secret(&mainnet.to_wif(), NET).unwrap_err().code(),
            "invalid_argument"
        );
        for bad in ["", "00".repeat(32).as_str(), "not a key"] {
            assert_eq!(
                parse_secret(bad, NET).unwrap_err().code(),
                "invalid_argument"
            );
        }
    }

    #[test]
    fn references_match_key_ids_and_p2pkh_payouts_never_refuse() {
        let id = [7u8; 20];
        let role = AttachedKeyRole::Owner;
        assert_eq!(
            Reference::KeyId(id).check(id, role).unwrap(),
            KeyCheck::Matched
        );
        assert_eq!(
            Reference::KeyId([8; 20])
                .check(id, role)
                .unwrap_err()
                .code(),
            "masternodes.key_mismatch"
        );
        let script = ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id));
        assert_eq!(
            Reference::Payout(script)
                .check(id, AttachedKeyRole::Payout)
                .unwrap(),
            KeyCheck::Matched
        );
        let other = ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([8; 20]));
        assert_eq!(
            Reference::Payout(other)
                .check(id, AttachedKeyRole::Payout)
                .unwrap(),
            KeyCheck::Unchecked,
            "a payout script can be stale"
        );
        assert_eq!(
            Reference::Nothing.check(id, role).unwrap(),
            KeyCheck::Unchecked
        );
    }

    #[test]
    fn labels_are_trimmed_and_bounded() {
        assert_eq!(normalize_label(Some("  ".into())).unwrap(), None);
        assert_eq!(
            normalize_label(Some(" a ".into())).unwrap().as_deref(),
            Some("a")
        );
        let long = "é".repeat(MAX_MASTERNODE_LABEL + 1);
        assert_eq!(
            normalize_label(Some(long)).unwrap_err().code(),
            "invalid_argument"
        );
    }
}
