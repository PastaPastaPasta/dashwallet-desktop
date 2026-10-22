//! DP5-02 §1–§2: every verdict of an invitation link, against a scripted
//! network, with links the library itself encodes.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::hashes::Hash;
use dashcore::secp256k1::SecretKey;
use dashcore::transaction::special_transaction::TransactionPayload;
use dashcore::transaction::special_transaction::asset_lock::AssetLockPayload;
use dashcore::{Network, OutPoint, PrivateKey, ScriptBuf, Transaction, TxOut, Txid};
use dpp::identity::state_transition::asset_lock_proof::InstantAssetLockProof;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::prelude::{AssetLockProof, Identifier};
use platform_wallet::wallet::identity::crypto::invitation::{InviterInfo, encode_invitation_uri};

use super::*;
use crate::platform::payments::Counterparty;

/// Platform as the checks see it, scripted.
#[derive(Default)]
struct Net {
    network: Option<Network>,
    /// Transactions by the id they are asked for.
    txs: HashMap<String, Transaction>,
    /// The fetch fails with a transport error.
    down: bool,
    identities: HashSet<Identifier>,
    inviters: HashMap<String, Counterparty>,
    /// DPNS fails.
    dpns_down: bool,
    asked: Mutex<Vec<String>>,
    pauses: AtomicU32,
    /// The transactions' blocks are not chain-locked yet.
    unlocked: bool,
}

impl InvitationNet for Net {
    fn network(&self) -> Network {
        self.network.unwrap_or(Network::Regtest)
    }

    fn transaction(&self, txid: String) -> BoxFuture<Result<Option<Fetched>, PlatformError>> {
        self.asked.lock().unwrap().push(txid.clone());
        let found = (!self.down).then(|| self.txs.get(&txid).cloned());
        let chain_locked = !self.unlocked;
        Box::pin(async move {
            let found = found.ok_or(PlatformError::Unavailable)?;
            Ok(found.map(|transaction| Fetched {
                transaction,
                chain_locked,
            }))
        })
    }

    fn identity_exists(&self, id: Identifier) -> BoxFuture<Result<bool, PlatformError>> {
        let exists = self.identities.contains(&id);
        Box::pin(async move { Ok(exists) })
    }

    fn inviter(&self, username: String) -> BoxFuture<Result<Option<Counterparty>, PlatformError>> {
        let found = (!self.dpns_down).then(|| self.inviters.get(&username).cloned());
        Box::pin(async move { found.ok_or(PlatformError::Unavailable) })
    }

    fn pause(&self) -> BoxFuture<()> {
        self.pauses.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

const DUFFS: u64 = 3_000_000;

fn voucher() -> SecretKey {
    SecretKey::from_secret_bytes([0x11; 32]).unwrap()
}

fn credit_script(key: &SecretKey) -> ScriptBuf {
    let public = PrivateKey::new(*key, Network::Regtest).public_key();
    ScriptBuf::new_p2pkh(&public.pubkey_hash())
}

/// An asset lock whose second credit output pays `key` (the first pays
/// another key: the voucher's output is selected, not taken at index 0).
fn funding_tx(key: &SecretKey) -> Transaction {
    let other = SecretKey::from_secret_bytes([0x22; 32]).unwrap();
    Transaction {
        version: 3,
        lock_time: 0,
        input: vec![],
        output: vec![],
        special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
            AssetLockPayload {
                version: 1,
                credit_outputs: vec![
                    TxOut {
                        value: 1_000,
                        script_pubkey: credit_script(&other),
                    },
                    TxOut {
                        value: DUFFS,
                        script_pubkey: credit_script(key),
                    },
                ],
            },
        )),
    }
}

/// The library's own link for `tx`: a ChainLock link, or an InstantSend
/// one whose islock locks `locked`.
fn link(tx: &Transaction, network: Network, islock_of: Option<Txid>, du: Option<&str>) -> String {
    let proof = match islock_of {
        Some(txid) => AssetLockProof::Instant(InstantAssetLockProof::new(
            InstantLock {
                txid,
                ..InstantLock::default()
            },
            tx.clone(),
            1,
        )),
        None => AssetLockProof::Chain(ChainAssetLockProof::new(
            10,
            OutPoint::new(tx.txid(), 1).into(),
        )),
    };
    let inviter = du.map(|du| InviterInfo {
        username: Some(du.into()),
        display_name: Some("Not Bob".into()),
        avatar_url: Some("https://example.com/not-bob.png".into()),
    });
    encode_invitation_uri(&voucher(), network, &proof, inviter.as_ref()).unwrap()
}

/// The id in internal byte order: the display hex reversed.
fn reversed(txid: Txid) -> String {
    hex::encode(txid.to_byte_array())
}

fn with_tx(tx: &Transaction) -> Net {
    Net {
        txs: [(tx.txid().to_string(), tx.clone())].into(),
        ..Net::default()
    }
}

fn bob() -> Counterparty {
    Counterparty {
        identity: "Bob1dentity".into(),
        username: Some("Bob".into()),
        display_name: Some("Bob B.".into()),
        avatar_url: None,
    }
}

fn run(net: &Net, text: &str) -> Result<Checked, PlatformError> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(check(net, text))
}

fn invalid(reason: InvitationInvalidReason) -> Result<Checked, PlatformError> {
    Ok(Checked::Invalid(reason))
}

#[test]
fn a_chainlock_link_is_valid_with_its_selected_output() {
    let tx = funding_tx(&voucher());
    let checked = run(&with_tx(&tx), &link(&tx, Network::Regtest, None, None));
    assert_eq!(
        checked,
        Ok(Checked::Valid {
            inviter: None,
            funding_duffs: DUFFS,
            outpoint: OutPoint::new(tx.txid(), 1),
        })
    );
}

/// A ChainLock link before its lock is `Valid` (the claim waits for it),
/// and the voucher says it is not locked yet.
#[test]
fn a_chainlock_link_before_its_lock_is_valid_and_unlocked() {
    let tx = funding_tx(&voucher());
    let text = link(&tx, Network::Regtest, None, None);
    let net = Net {
        unlocked: true,
        ..with_tx(&tx)
    };
    let checked = run(&net, &text);
    assert!(matches!(checked, Ok(Checked::Valid { .. })), "{checked:?}");
    let parsed = parse(&text).unwrap();
    let voucher = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(funding(&net, &parsed));
    assert_eq!(
        voucher,
        Ok(Ok(Voucher {
            outpoint: OutPoint::new(tx.txid(), 1),
            duffs: DUFFS,
            chain_locked: false,
        }))
    );
}

#[test]
fn an_instant_link_is_valid_when_its_islock_locks_the_transaction() {
    let tx = funding_tx(&voucher());
    let checked = run(
        &with_tx(&tx),
        &link(&tx, Network::Regtest, Some(tx.txid()), None),
    );
    assert!(
        matches!(
            checked,
            Ok(Checked::Valid {
                funding_duffs: DUFFS,
                ..
            })
        ),
        "{checked:?}"
    );
    let other = Txid::from_byte_array([9; 32]);
    let checked = run(
        &with_tx(&tx),
        &link(&tx, Network::Regtest, Some(other), None),
    );
    assert_eq!(checked, invalid(InvitationInvalidReason::Malformed));
}

/// §2: the inviter is DPNS's identity and profile, never the link's own
/// display name or avatar.
#[test]
fn the_inviter_is_what_dpns_says() {
    let tx = funding_tx(&voucher());
    let text = link(&tx, Network::Regtest, None, Some("bob"));
    let mut net = with_tx(&tx);
    net.inviters.insert("bob".into(), bob());
    let checked = run(&net, &text).unwrap();
    let Checked::Valid { inviter, .. } = checked else {
        panic!("{checked:?}");
    };
    assert_eq!(inviter, Some(bob()));
    // Unknown to DPNS: no inviter, whatever the link says.
    net.inviters.clear();
    let checked = run(&net, &text).unwrap();
    assert!(
        matches!(checked, Checked::Valid { inviter: None, .. }),
        "{checked:?}"
    );
    // DPNS down: the call fails, never a silent `None`.
    net.dpns_down = true;
    assert_eq!(run(&net, &text), Err(PlatformError::Unavailable));
}

#[test]
fn an_identity_at_the_claims_id_is_claimed() {
    let tx = funding_tx(&voucher());
    let mut net = with_tx(&tx);
    let outpoint = OutPoint::new(tx.txid(), 1);
    net.identities
        .insert(super::super::registration_flow::identity_for(outpoint));
    let checked = run(&net, &link(&tx, Network::Regtest, None, Some("bob")));
    assert_eq!(checked, Ok(Checked::Claimed));
    // Another output's identity is not this link's.
    net.identities = [super::super::registration_flow::identity_for(
        OutPoint::new(tx.txid(), 0),
    )]
    .into();
    let checked = run(&net, &link(&tx, Network::Regtest, None, None));
    assert!(matches!(checked, Ok(Checked::Valid { .. })), "{checked:?}");
}

#[test]
fn a_mainnet_link_on_regtest_is_the_wrong_network() {
    let tx = funding_tx(&voucher());
    let net = with_tx(&tx);
    let checked = run(&net, &link(&tx, Network::Mainnet, None, None));
    assert_eq!(checked, invalid(InvitationInvalidReason::WrongNetwork));
    assert!(
        net.asked.lock().unwrap().is_empty(),
        "no fetch for the wrong network"
    );
}

/// Old iOS links carry the txid little-endian: the reversed id is asked
/// within the same attempt.
#[test]
fn a_reversed_txid_is_found() {
    let tx = funding_tx(&voucher());
    let shown = tx.txid().to_string();
    let text = link(&tx, Network::Regtest, None, None);
    assert!(text.contains(&shown));
    let old = text.replace(&shown, &reversed(tx.txid()));
    let net = with_tx(&tx);
    let checked = run(&net, &old);
    assert!(matches!(checked, Ok(Checked::Valid { .. })), "{checked:?}");
    assert_eq!(*net.asked.lock().unwrap(), [reversed(tx.txid()), shown]);
    assert_eq!(net.pauses.load(Ordering::SeqCst), 0);
}

#[test]
fn a_transaction_nobody_has_is_not_found_after_every_attempt() {
    let tx = funding_tx(&voucher());
    let net = Net::default();
    let checked = run(&net, &link(&tx, Network::Regtest, None, None));
    assert_eq!(checked, invalid(InvitationInvalidReason::AssetLockNotFound));
    assert_eq!(net.asked.lock().unwrap().len(), 2 * FETCH_ATTEMPTS as usize);
    assert_eq!(net.pauses.load(Ordering::SeqCst), FETCH_ATTEMPTS - 1);
}

/// A transport error is not a miss: the call fails at once.
#[test]
fn a_transport_error_fails_the_call() {
    let tx = funding_tx(&voucher());
    let net = Net {
        down: true,
        ..with_tx(&tx)
    };
    let checked = run(&net, &link(&tx, Network::Regtest, None, None));
    assert_eq!(checked, Err(PlatformError::Unavailable));
    assert_eq!(net.asked.lock().unwrap().len(), 1);
}

#[test]
fn malformed_links_and_transactions() {
    let tx = funding_tx(&voucher());
    let text = link(&tx, Network::Regtest, None, None);
    for bad in [
        "",
        "hello",
        "dashpay://invite?du=bob",
        "dashpay://user?du=bob",
        "https://example.com/applink?assetlocktx=00&pk=00",
    ] {
        assert_eq!(
            run(&with_tx(&tx), bad),
            invalid(InvitationInvalidReason::Malformed),
            "{bad}"
        );
    }
    // DAPI answers with another transaction for the id.
    let mut net = with_tx(&tx);
    let mut other = funding_tx(&voucher());
    other.lock_time = 7;
    net.txs.insert(tx.txid().to_string(), other);
    assert_eq!(
        run(&net, &text),
        invalid(InvitationInvalidReason::Malformed)
    );
    // No credit output the voucher key controls.
    let stranger = funding_tx(&SecretKey::from_secret_bytes([0x33; 32]).unwrap());
    let text = link(&stranger, Network::Regtest, None, None);
    assert_eq!(
        run(&with_tx(&stranger), &text),
        invalid(InvitationInvalidReason::Malformed)
    );
    // Not an asset lock at all.
    let mut plain = funding_tx(&voucher());
    plain.special_transaction_payload = None;
    let text = link(&plain, Network::Regtest, None, None);
    assert_eq!(
        run(&with_tx(&plain), &text),
        invalid(InvitationInvalidReason::Malformed)
    );
}

#[test]
fn contested_names_need_the_contested_lock() {
    assert!(!Checked::contested_allowed(CONTESTED_LOCK_DUFFS - 1));
    assert!(Checked::contested_allowed(CONTESTED_LOCK_DUFFS));
}
