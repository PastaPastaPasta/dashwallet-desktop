//! The tracking calls (mn-engine §4): track and untrack any listed
//! masternode, labels, and the owner, voting and payout keys attached to a
//! tracked one (MG-03 §4, IOS-082). No operator or platform-node key is
//! taken: the vault has no such role (MG-01 D7).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::SecretKey;
use dashcore::{PrivateKey, PubkeyHash, ScriptBuf};
use zeroize::Zeroizing;

use platform_wallet::masternode::MasternodeListSummary;

use super::aggregate::OwnedMasternode;
use super::follow::{
    Notes, all_owned, clear_key_id_durably, load_notes, read_list, set_key_id, set_label,
};
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

/// The secret of a compressed WIF (of the session's network) or of 64 hex
/// characters, and the hash160 of its compressed public key. An
/// uncompressed WIF is refused: the vault keeps the bare secret, whose id
/// is always read back compressed (D36).
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
        if !private.compressed {
            return Err(EngineError::InvalidArgument(
                "an uncompressed WIF is not accepted; use its compressed WIF or 64 hex".into(),
            ));
        }
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
    /// from a version-3 payload is misread, so not used (D25), and a
    /// `shared` masternode's payout isn't one script (D29).
    pub(super) fn new(
        entry: Option<&MasternodeListSummary>,
        owned: Option<&OwnedMasternode>,
        shared: bool,
    ) -> Self {
        let record = owned.filter(|o| !o.shared_unsupported).map(|o| &o.record);
        Self {
            voting: entry.map(|e| e.voting_key_id),
            owner: record.and_then(|r| r.owner_key_hash),
            payout: record
                .filter(|_| !shared)
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
    let key = display_hex(&hash);
    let seen = session
        .appdb_op(move |db| Ok(load_notes(db)?.shared_seen.contains(&key)))
        .await?;
    let entry = list.entry(&hash);
    let shared = seen
        || entry.is_some_and(|e| e.has_extended_net_info)
        || owned.as_ref().is_some_and(|o| o.shared_unsupported);
    Ok(KeyReferences::new(entry, owned.as_ref(), shared))
}

/// Brings the recorded ids in line with the keys the vault holds, under the
/// tracking lock so no `attach_key` interleaves. Once a session (the first
/// read with the vault unlocked) every recorded id is compared with the
/// vault's key: one naming another key (a power loss kept an id the attach
/// had cleared) is dropped, so the key holds nothing until the next session
/// records it (D34). On every read a key with no id gets its vault key's
/// (D31), unless its vault write failed in this session: this session's
/// vault may hold another key in memory than on disk. A locked vault
/// changes nothing; a key the vault can't read is logged and skipped, and
/// the comparison runs again on the next read, as it does after a failed
/// write. Any id this read couldn't compare is left out, so that key holds
/// nothing on this read: a change holding the lock, the vault locking
/// meanwhile, or a key it can't read. A busy lock or a written change makes
/// the host read again (`local_change`).
pub(super) async fn reconcile_key_ids(
    session: &NetworkSession,
    attached: &HashMap<[u8; 32], Vec<AttachedKeyRole>>,
    notes: Notes,
) -> Result<Notes, MasternodesError> {
    let runtime = &session.masternodes;
    let mut notes = reconcile(session, attached, notes).await?;
    // An unsettled key has no id stored, but this read's notes may predate
    // the write that dropped it.
    for (hash, roles) in attached {
        for role in roles {
            if runtime.is_unsettled(*hash, *role) {
                notes.key_ids.remove(&(display_hex(hash), *role));
            }
        }
    }
    Ok(notes)
}

fn leave_out(notes: &mut Notes, pairs: impl IntoIterator<Item = ([u8; 32], AttachedKeyRole)>) {
    for (hash, role) in pairs {
        notes.key_ids.remove(&(display_hex(&hash), role));
    }
}

async fn reconcile(
    session: &NetworkSession,
    attached: &HashMap<[u8; 32], Vec<AttachedKeyRole>>,
    mut notes: Notes,
) -> Result<Notes, MasternodesError> {
    let runtime = &session.masternodes;
    let pending = |notes: &Notes| -> Vec<([u8; 32], AttachedKeyRole)> {
        let verify = !runtime.ids_verified.load(Ordering::SeqCst);
        attached
            .iter()
            .flat_map(|(hash, roles)| roles.iter().map(move |role| (*hash, *role)))
            .filter(|(hash, role)| {
                !runtime.is_unsettled(*hash, *role)
                    && (verify || !notes.key_ids.contains_key(&(display_hex(hash), *role)))
            })
            .collect()
    };
    if pending(&notes).is_empty() {
        return Ok(notes);
    }
    let Some(_mutation) = runtime.try_mutation() else {
        let unchecked = pending(&notes);
        leave_out(&mut notes, unchecked);
        runtime.local_change();
        return Ok(notes);
    };
    let mut notes = session.appdb_op(load_notes).await?;
    let pending = pending(&notes);
    let vault = session.vault.clone();
    let read = tokio::task::spawn_blocking(move || {
        pending
            .into_iter()
            .map(|(hash, role)| {
                (
                    hash,
                    role,
                    vault.masternode_key_id(&hash, role.vault_name()),
                )
            })
            .collect::<Vec<_>>()
    })
    .await?;
    if read
        .iter()
        .any(|(_, _, held)| matches!(held, Err(dw_vault::VaultError::Locked)))
    {
        leave_out(
            &mut notes,
            read.into_iter().map(|(hash, role, _)| (hash, role)),
        );
        return Ok(notes);
    }
    let mut changes = Vec::new();
    let mut dropped = Vec::new();
    let mut verified = true;
    for (hash, role, held) in read {
        let held = match held {
            Ok(held) => held,
            Err(e) => {
                tracing::warn!(error = %e, "could not read an attached key's id");
                leave_out(&mut notes, [(hash, role)]);
                verified = false;
                continue;
            }
        };
        match notes.key_ids.get(&(display_hex(&hash), role)) {
            None => changes.extend(held.map(|id| (hash, role, Some(id)))),
            Some(recorded) if Some(*recorded) == held => {}
            Some(_) => {
                tracing::warn!("an attached key's recorded id names another key; dropped");
                dropped.push((hash, role));
                changes.push((hash, role, None));
            }
        }
    }
    let notes = if changes.is_empty() {
        notes
    } else {
        let written = session
            .appdb_op(move |db| {
                for (hash, role, id) in changes {
                    set_key_id(db, &display_hex(&hash), role, id)?;
                }
                load_notes(db)
            })
            .await?;
        runtime.local_change();
        written
    };
    // Only once the drops are written: a failed write leaves them, and the
    // comparison, to the next read.
    for (hash, role) in dropped {
        runtime.unsettle(hash, role);
    }
    if verified {
        runtime.ids_verified.store(true, Ordering::SeqCst);
    }
    Ok(notes)
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
        // The id is cleared, durably (D34), before the vault write and
        // recorded only after it succeeded, so an interruption leaves a key
        // without an id, which holds no role, never an id that names a key
        // the vault doesn't hold (D24). A failed write may have stored the key all the same (a
        // read-back or directory fsync failing after the rename), so no id
        // is put back: the next session records the id of what the vault
        // then holds (D31).
        let key = display_hex(&hash);
        let check = self
            .op(move |session| async move {
                let check = references(&session, hash, role).await?.check(role, id);
                if check == KeyCheck::KeyMismatch {
                    return Err(MasternodesError::KeyMismatch { role });
                }
                session
                    .appdb_op(move |db| clear_key_id_durably(db, &key, role))
                    .await?;
                Ok(check)
            })
            .await?;
        let runtime = &self.session.masternodes;
        let stored = self
            .session
            .vault_op(move |v| v.store_masternode_key(&hash, role.vault_name(), &secret[..]))
            .await;
        if let Err(e) = stored {
            runtime.unsettle(hash, role);
            runtime.local_change();
            return Err(e.into());
        }
        runtime.settle(hash, role);
        let key = display_hex(&hash);
        let written = self
            .session
            .appdb_op(move |db| set_key_id(db, &key, role, Some(id)))
            .await;
        runtime.local_change();
        written?;
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
        // Refused with fixed text: the vault's bare secret reads back
        // compressed, another id (D36).
        let uncompressed =
            PrivateKey::new_uncompressed(SecretKey::from_secret_bytes([1; 32]).unwrap(), NET)
                .to_wif();
        let refused = parse_secret(&uncompressed, NET).unwrap_err();
        assert_eq!(refused.code(), "invalid_argument");
        assert!(!refused.to_string().contains(&uncompressed));
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

    /// A version-3 masternode's payout isn't one script; its owner key, from
    /// a version-2 record, still reads right (D29).
    #[test]
    fn a_shared_masternode_has_no_payout_reference() {
        use super::super::aggregate::aggregate;
        use super::super::aggregate::fixtures::{coins, null, registration};
        use dw_protx::params::MASTERNODE_COLLATERAL;
        let tx = registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let owned = aggregate(&[(10, 0, tx)]);
        let (plain, shared) = (
            KeyReferences::new(None, owned.first(), false),
            KeyReferences::new(None, owned.first(), true),
        );
        assert!(plain.payout.is_some() && shared.payout.is_none());
        assert_eq!(shared.owner, Some([1; 20]));
    }

    /// An attached voting key holds only while it matches the synced list:
    /// rotated, or not checked yet (the list not synced), it doesn't (D29).
    #[test]
    fn only_a_matched_voting_key_holds() {
        use super::super::records::AttachedKey;
        let key = |role, check| AttachedKey {
            role,
            key_id: Some("07".repeat(20)),
            check,
        };
        use AttachedKeyRole::{Owner, Payout, Voting};
        assert!(key(Voting, KeyCheck::Matched).holds());
        assert!(!key(Voting, KeyCheck::KeyMismatch).holds());
        assert!(!key(Voting, KeyCheck::Unchecked).holds());
        assert!(key(Owner, KeyCheck::Unchecked).holds());
        assert!(key(Payout, KeyCheck::Unchecked).holds());
        assert!(!key(Owner, KeyCheck::KeyMismatch).holds());
        let unrecorded = AttachedKey {
            key_id: None,
            ..key(Owner, KeyCheck::Unchecked)
        };
        assert!(!unrecorded.holds());
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

    /// Debug builds only: dw-vault's fault injection and dw-appdb's power
    /// loss exist in no release build (D38).
    #[cfg(all(test, debug_assertions))]
    mod ambiguous_writes {
        use std::path::Path;

        use dw_vault::fault::{Fault, arm};
        use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VAULT_DIR, VaultConfig};

        use super::super::super::records::AttachedKey;
        use super::*;
        use crate::{
            DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions, WalletId,
        };

        struct NoEvents;

        impl EventSink for NoEvents {
            fn emit(&self, _: EngineEvent) {}
        }

        const HASH: [u8; 32] = [0x5a; 32];

        /// An engine on `dir`; `store` holds an unencrypted vault's data key
        /// across a restart.
        fn new_engine(dir: &Path, store: &Arc<MemoryOsStore>) -> Engine {
            Engine::new(
                EngineConfig {
                    data_root: dir.join("data"),
                    worker_threads: Some(4),
                    vault: VaultConfig {
                        kdf: KdfPolicy::Fixed(KdfParams::TEST),
                        os_store: store.clone(),
                        ..VaultConfig::default()
                    },
                },
                Arc::new(NoEvents),
            )
            .unwrap()
        }

        fn open(engine: &Engine) -> Arc<NetworkSession> {
            engine
                .block_on(engine.open_network(
                    DashNetwork::Regtest,
                    SessionOptions {
                        dapi_addresses: vec!["http://127.0.0.1:1".into()],
                        quorum_url: Some("http://127.0.0.1:1".into()),
                        spv_peers: vec!["127.0.0.1:1".into()],
                        ..Default::default()
                    },
                ))
                .unwrap()
        }

        fn id_of(secret: [u8; 32]) -> String {
            let (_, id) = parse_secret(&hex::encode(secret), NET).unwrap();
            hex::encode(id)
        }

        fn owner_key(engine: &Engine, s: &Arc<NetworkSession>, wallet: WalletId) -> AttachedKey {
            let overview = engine.block_on(s.masternodes().list(wallet)).unwrap();
            let row = overview
                .masternodes
                .iter()
                .find(|r| r.pro_tx_hash == display_hex(&HASH))
                .expect("the tracked row");
            let attached = row.roles.attached.clone().expect("an unlocked vault");
            assert_eq!(attached.len(), 1, "{attached:?}");
            attached.into_iter().next().unwrap()
        }

        fn attach(
            engine: &Engine,
            s: &Arc<NetworkSession>,
            secret: [u8; 32],
        ) -> Result<KeyCheck, MasternodesError> {
            engine.block_on(s.masternodes().attach_key(
                display_hex(&HASH),
                AttachedKeyRole::Owner,
                Zeroizing::new(hex::encode(secret)),
            ))
        }

        /// MG-03 D31: a vault write that fails after its rename (B stored on
        /// disk, the vault's memory holding A or B) records no id, so the
        /// key holds no role in this session; the next session records the
        /// id of the key the vault holds, B.
        fn an_ambiguous_write_records_the_vaults_key_next_session(fault: Fault) {
            let (a, b) = ([7u8; 32], [9u8; 32]);
            let dir = dw_testutil::private_tempdir();
            let store = Arc::new(MemoryOsStore::new());
            let engine = new_engine(dir.path(), &store);
            let s = open(&engine);
            engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
            let wallet = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
            let service = s.manager().unwrap().tracked_masternodes_service();
            service.track_blocking(HASH, None).unwrap();
            drop(service);

            assert_eq!(attach(&engine, &s, a).unwrap(), KeyCheck::Unchecked);
            let key = owner_key(&engine, &s, wallet);
            assert_eq!(key.key_id, Some(id_of(a)));
            assert!(key.holds());

            arm(&s.data_dir().join(VAULT_DIR), fault);
            assert!(attach(&engine, &s, b).is_err(), "the injected failure");
            for _ in 0..2 {
                let key = owner_key(&engine, &s, wallet);
                assert_eq!(key.key_id, None, "no id after an ambiguous write");
                assert_eq!(key.check, KeyCheck::Unchecked);
                assert!(!key.holds());
            }

            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
            drop(engine);
            let engine = new_engine(dir.path(), &store);
            let s = open(&engine);
            let key = owner_key(&engine, &s, wallet);
            assert_eq!(key.key_id, Some(id_of(b)), "the id of the key on disk");
            assert!(key.holds());
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
        }

        /// The owner key's id and whether it holds after attaching B over A
        /// and a power loss once the vault made B durable, before app.sqlite
        /// synced (MG-03 D34). `skip_barrier` skips the sync of the attach's
        /// clearing of A's id; `!verify` skips the session's comparison of
        /// the recorded ids with the vault's keys.
        fn after_power_loss(skip_barrier: bool, verify: bool) -> (Option<String>, bool) {
            use dw_appdb::durability_sim::{arm, power_loss};
            let dir = dw_testutil::private_tempdir();
            let store = Arc::new(MemoryOsStore::new());
            let engine = new_engine(dir.path(), &store);
            let s = open(&engine);
            let appdb = s.data_dir().join(dw_appdb::APP_DB_FILE);
            engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
            let wallet = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
            let service = s.manager().unwrap().tracked_masternodes_service();
            service.track_blocking(HASH, None).unwrap();
            drop(service);
            attach(&engine, &s, [7; 32]).unwrap();
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
            drop(engine);

            let session = |engine: &Engine| {
                let s = open(engine);
                if !verify {
                    s.masternodes.ids_verified.store(true, Ordering::SeqCst);
                }
                s
            };
            arm(&appdb, skip_barrier);
            let engine = new_engine(dir.path(), &store);
            let s = session(&engine);
            attach(&engine, &s, [9; 32]).unwrap();
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
            drop(engine);
            power_loss(&appdb);

            let engine = new_engine(dir.path(), &store);
            let s = session(&engine);
            let key = owner_key(&engine, &s, wallet);
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
            (key.key_id.clone(), key.holds())
        }

        /// A read while a tracking change holds the lock doesn't wait for
        /// it (a close could hang behind both) and leaves out the ids it
        /// hasn't compared yet; the next read compares them (D34).
        #[test]
        fn a_read_during_a_change_holds_nothing_unverified() {
            let dir = dw_testutil::private_tempdir();
            let store = Arc::new(MemoryOsStore::new());
            let engine = new_engine(dir.path(), &store);
            let s = open(&engine);
            engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
            let wallet = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
            let service = s.manager().unwrap().tracked_masternodes_service();
            service.track_blocking(HASH, None).unwrap();
            drop(service);
            attach(&engine, &s, [7; 32]).unwrap();
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
            drop(engine);

            let engine = new_engine(dir.path(), &store);
            let s = open(&engine);
            let change = engine.block_on(s.masternodes.mutation());
            let key = owner_key(&engine, &s, wallet);
            assert_eq!((key.key_id.clone(), key.holds()), (None, false));
            drop(change);
            let key = owner_key(&engine, &s, wallet);
            assert_eq!(
                (key.key_id.clone(), key.holds()),
                (Some(id_of([7; 32])), true)
            );
            drop(s);
            engine.block_on(engine.shutdown()).unwrap();
        }

        #[test]
        fn a_power_loss_never_leaves_the_old_keys_id_beside_the_new_key() {
            // Neither: A's id is back beside B, a held role (`matched` once
            // the list names A's key, the false role D34 closes).
            assert_eq!(after_power_loss(true, false), (Some(id_of([7; 32])), true));
            // The durable clear alone: no id survives, and the read records
            // the key the vault holds.
            assert_eq!(after_power_loss(false, false), (Some(id_of([9; 32])), true));
            // The comparison alone: A's id is dropped, holding nothing.
            assert_eq!(after_power_loss(true, true), (None, false));
        }

        #[test]
        fn a_failed_read_back_records_the_vaults_key_next_session() {
            an_ambiguous_write_records_the_vaults_key_next_session(Fault::ReadBack);
        }

        #[test]
        fn a_failed_directory_fsync_records_the_vaults_key_next_session() {
            an_ambiguous_write_records_the_vaults_key_next_session(Fault::DirSync);
        }
    }
}
