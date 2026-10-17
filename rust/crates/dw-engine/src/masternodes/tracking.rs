//! The tracking calls (mn-engine §4): track and untrack any listed
//! masternode, labels, and the owner, voting and payout keys attached to a
//! tracked one (MG-03 §4, IOS-082). No operator or platform-node key is
//! taken: the vault has no such role (MG-01 D7).

use std::sync::Arc;

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::SecretKey;
use dashcore::{PrivateKey, PubkeyHash, ScriptBuf};
use zeroize::Zeroizing;

use platform_wallet::masternode::MasternodeListSummary;

use super::aggregate::OwnedMasternode;
use super::follow::{all_owned, key_id, read_list, set_key_id, set_label};
use super::reads::{list_facts, not_found};
use super::records::{AttachedKeyRole, KeyCheck, ListedMasternode, UntrackOutcome};
use super::{Masternodes, MasternodesError, display_hex, parse_pro_tx_hash};
use crate::{EngineError, NetworkSession};

const ROLES: [AttachedKeyRole; 3] = [
    AttachedKeyRole::Owner,
    AttachedKeyRole::Voting,
    AttachedKeyRole::Payout,
];

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

/// What a key of each role is checked against (MG-03 §4, MG-00 D13, D24):
/// at attach, and again on every read.
#[derive(Debug, Default)]
pub(super) struct KeyReferences {
    /// The synced list's voting key id.
    voting: Option<[u8; 20]>,
    /// The owner key id of a ProRegTx a loaded wallet holds (the owner key
    /// never changes).
    owner: Option<[u8; 20]>,
    /// The P2PKH payout script of its latest ProRegTx/ProUpRegTx. It can be
    /// stale (a ProUpRegTx the wallet doesn't hold, D5), so a key that
    /// doesn't pay it is `unchecked`, not a mismatch (D19).
    payout: Option<ScriptBuf>,
}

impl KeyReferences {
    /// From the list entry and a loaded wallet's record; a record built
    /// from a version-3 payload is misread, so not used (D25).
    pub(super) fn new(
        entry: Option<&MasternodeListSummary>,
        owned: Option<&OwnedMasternode>,
    ) -> Self {
        let record = owned.filter(|o| !o.shared_unsupported).map(|o| &o.record);
        Self {
            voting: entry.map(|e| e.voting_key_id),
            owner: record.and_then(|r| r.owner_key_hash),
            payout: record
                .and_then(|r| r.payout_script.clone())
                .map(ScriptBuf::from)
                .filter(|s| s.is_p2pkh()),
        }
    }

    pub(super) fn check(&self, role: AttachedKeyRole, id: [u8; 20]) -> KeyCheck {
        let key_id = |want: Option<[u8; 20]>| match want {
            Some(want) if want == id => KeyCheck::Matched,
            Some(_) => KeyCheck::KeyMismatch,
            None => KeyCheck::Unchecked,
        };
        match role {
            AttachedKeyRole::Owner => key_id(self.owner),
            AttachedKeyRole::Voting => key_id(self.voting),
            AttachedKeyRole::Payout => {
                let pays = ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id));
                if self.payout.as_ref() == Some(&pays) {
                    KeyCheck::Matched
                } else {
                    KeyCheck::Unchecked
                }
            }
        }
    }
}

/// The references for `role` of `hash`: a voting key needs the synced
/// list. Caller holds the guard.
async fn references(
    session: &NetworkSession,
    hash: [u8; 32],
    role: AttachedKeyRole,
) -> Result<KeyReferences, MasternodesError> {
    let manager = session.manager()?;
    let list = read_list(session, &manager).await;
    if role == AttachedKeyRole::Voting && !list.synced {
        return Err(MasternodesError::ListUnavailable);
    }
    let hub = Arc::clone(&session.hub);
    let owned = tokio::task::spawn_blocking(move || {
        all_owned(&manager, &hub.history)
            .into_iter()
            .find(|o| o.record.pro_tx_hash == hash)
    })
    .await?;
    Ok(KeyReferences::new(list.entry(&hash), owned.as_ref()))
}

/// Forgets the recorded ids of `hash`'s keys of `roles`. The keys are gone
/// from the vault already, so a failure is only logged: a stale id without
/// a key is never read (D24).
async fn forget_key_ids(session: &NetworkSession, hash: &[u8; 32], roles: &[AttachedKeyRole]) {
    let key = display_hex(hash);
    let roles = roles.to_vec();
    let forgotten = session
        .appdb_op(move |db| {
            roles
                .iter()
                .try_for_each(|role| set_key_id(db, &key, *role, None))
        })
        .await;
    if let Err(e) = forgotten {
        tracing::warn!(error = %e, "could not forget an attached key's id");
    }
}

impl Masternodes {
    fn require_tracked(&self, hash: &[u8; 32]) -> Result<(), MasternodesError> {
        match self.session.manager()?.tracked_masternode(hash) {
            Some(_) => Ok(()),
            None => Err(MasternodesError::NotTracked),
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
                let entry = list.entry(&hash).ok_or(MasternodesError::NotFound)?;
                let facts = list_facts(entry, session.network.core_network());
                if manager.tracked_masternode(&hash).is_some() {
                    return Err(MasternodesError::AlreadyTracked);
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
                forget_key_ids(&session, &hash, &ROLES).await;
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
    /// tracked one's label is the registry's; a loaded wallet's or a listed
    /// one's is kept in app.sqlite (MG-03 D12). Any other hash is refused,
    /// so app.sqlite holds no label of a masternode nobody knows (D27).
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
                let list = read_list(&session, &manager).await;
                if list.entry(&hash).is_none() {
                    let hub = Arc::clone(&session.hub);
                    let owned = tokio::task::spawn_blocking(move || {
                        all_owned(&manager, &hub.history)
                            .iter()
                            .any(|o| o.record.pro_tx_hash == hash)
                    })
                    .await?;
                    if !owned {
                        return Err(not_found(&list));
                    }
                }
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
    /// Its public key's hash160 is recorded in app.sqlite first, so every
    /// read compares it again (D24). Needs the vault's full-scope data key.
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
        let key = display_hex(&hash);
        let (check, previous) = self
            .op(move |session| async move {
                let check = references(&session, hash, role).await?.check(role, id);
                if check == KeyCheck::KeyMismatch {
                    return Err(MasternodesError::KeyMismatch { role });
                }
                let previous = session
                    .appdb_op(move |db| {
                        let previous = key_id(db, &key, role)?;
                        set_key_id(db, &key, role, Some(id))?;
                        Ok(previous)
                    })
                    .await?;
                Ok((check, previous))
            })
            .await?;
        let stored = self
            .session
            .vault_op(move |v| v.store_masternode_key(&hash, role.vault_name(), &secret[..]))
            .await;
        if let Err(e) = stored {
            // The vault still holds the key it had, if any.
            let key = display_hex(&hash);
            let restored = self
                .session
                .appdb_op(move |db| set_key_id(db, &key, role, previous))
                .await;
            if let Err(e) = restored {
                tracing::warn!(error = %e, "could not restore an attached key's id");
            }
            return Err(e.into());
        }
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
        self.op(move |session| async move {
            forget_key_ids(&session, &hash, &[role]).await;
            Ok(())
        })
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
    fn references_compare_key_ids_and_p2pkh_payouts_are_never_a_mismatch() {
        let id = [7u8; 20];
        let refs = KeyReferences {
            voting: Some(id),
            owner: Some([8; 20]),
            payout: Some(ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id))),
        };
        assert_eq!(refs.check(AttachedKeyRole::Voting, id), KeyCheck::Matched);
        assert_eq!(
            refs.check(AttachedKeyRole::Owner, id),
            KeyCheck::KeyMismatch
        );
        assert_eq!(refs.check(AttachedKeyRole::Payout, id), KeyCheck::Matched);
        assert_eq!(
            refs.check(AttachedKeyRole::Payout, [8; 20]),
            KeyCheck::Unchecked,
            "a payout script can be stale"
        );
        let none = KeyReferences::default();
        for role in ROLES {
            assert_eq!(none.check(role, id), KeyCheck::Unchecked);
        }
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
