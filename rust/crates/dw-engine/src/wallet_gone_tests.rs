//! #263: a read whose wallet left the manager between its handle lookup and
//! its read (the lock hand-over of a removal) fails or skips, never panics.
//! [`detached`] makes that state: the handle stays, its info is gone.

use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use crate::{DashNetwork, Engine, NetworkSession, WalletId};

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

struct Silent;

impl crate::EventSink for Silent {
    fn emit(&self, _: crate::EngineEvent) {}
}

pub(crate) struct Detached {
    pub(crate) engine: Engine,
    pub(crate) session: Arc<NetworkSession>,
    pub(crate) id: WalletId,
    pub(crate) wallet: Arc<PlatformWallet>,
    _dir: tempfile::TempDir,
}

/// An offline regtest session with one imported wallet whose info was
/// taken out of the manager, as a removal does under its write lock, while
/// its handle is still found.
pub(crate) fn detached() -> Detached {
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        crate::EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap();
    let options = crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    };
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, options))
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    let wallet = detach(&engine, &session, id);
    Detached {
        engine,
        session,
        id,
        wallet,
        _dir: dir,
    }
}

/// Takes wallet `id`'s info out of the manager, leaving its handle.
pub(crate) fn detach(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> Arc<PlatformWallet> {
    let wallet = engine.block_on(session.wallet(&id)).unwrap();
    engine.block_on(async {
        let mut wm = wallet.wallet_manager().write().await;
        wm.remove_wallet(&id.0).unwrap();
    });
    wallet
}

/// `r` failed as `wallet_not_found` (a panicked task's is `internal`).
#[track_caller]
pub(crate) fn assert_gone<T: std::fmt::Debug>(r: Result<T, &str>) {
    assert!(
        matches!(r, Err("wallet_not_found")),
        "expected wallet_not_found: {r:?}"
    );
}

fn outpoint() -> OutPoint {
    OutPoint::new(Txid::from_byte_array([7; 32]), 0)
}

#[test]
fn a_detached_wallet_is_still_found_by_its_handle() {
    let d = detached();
    assert!(d.engine.block_on(d.session.wallet(&d.id)).is_ok());
    assert_gone(d.engine.block_on(info_code(&d.wallet)));
}

async fn info_code(wallet: &PlatformWallet) -> Result<(), &'static str> {
    super::info_of(wallet)
        .await
        .map(drop)
        .map_err(|e| e.code())?;
    super::info_mut_of(wallet)
        .await
        .map(drop)
        .map_err(|e| e.code())
}

#[test]
fn locking_coins_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let s = &d.session;
    let r = d.engine.block_on(s.lock_outpoints(d.id, vec![outpoint()]));
    assert_gone(r.map_err(|e| e.code()));
    let r = d
        .engine
        .block_on(s.unlock_outpoints(d.id, vec![outpoint()]));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn the_funding_coin_count_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = d
        .engine
        .block_on(crate::coins::funding_coin_count(&d.wallet));
    assert_gone(r.map_err(|e| e.code()));
    let r = d.engine.block_on(d.session.fee_bound_worst(d.id));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn the_replay_guard_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = d.engine.block_on(d.session.replay_guard(d.id, 0));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn platform_node_keys_of_a_wallet_gone_from_the_manager_do_not_panic() {
    let d = detached();
    let r = d.engine.block_on(d.session.masternode_keys(
        d.id,
        crate::MasternodeKeyRole::PlatformNode,
        0,
        1,
    ));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn owner_keys_of_a_wallet_gone_from_the_manager_do_not_panic() {
    let d = detached();
    let r =
        d.engine.block_on(
            d.session
                .masternode_keys(d.id, crate::MasternodeKeyRole::Owner, 0, 1),
        );
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn the_masternode_view_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = d.engine.block_on(d.session.masternodes().list(d.id));
    assert_gone(r.map_err(|e| e.code()));
}

/// A derivation error still consumes the `RevealSecret` grant:
/// `with_revealed_seed` redeems it before the derivation runs.
#[test]
fn revealing_a_key_of_a_wallet_gone_from_the_manager_consumes_the_grant() {
    use dw_vault::{Credential, GrantPurpose};
    let d = detached();
    let id = d.id;
    let grant = d
        .engine
        .block_on(d.session.vault_op(move |v| {
            v.authorize(GrantPurpose::RevealSecret, Some(&id.0), Credential::None)
        }))
        .unwrap()
        .id;
    let r = d.engine.block_on(d.session.reveal_masternode_key(
        d.id,
        crate::MasternodeKeyRole::Owner,
        0,
        grant.clone(),
    ));
    assert_gone(r.map(|_| ()).map_err(|e| e.code()));
    let again = d.engine.block_on(d.session.reveal_masternode_key(
        d.id,
        crate::MasternodeKeyRole::Owner,
        0,
        grant,
    ));
    match again {
        Err(crate::EngineError::Masternode(crate::MasternodeFailure::GrantInvalid)) => {}
        Err(e) => panic!("expected grant invalid, got {e}"),
        Ok(_) => panic!("expected the consumed grant to be refused"),
    }
}

#[test]
fn the_lookahead_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = (d.engine).block_on(crate::keys::apply_lookahead(&d.wallet, 30, false));
    assert_gone(r.map_err(|e| e.code()));
}

/// A regtest P2PKH address: the reads come before its ownership check.
const OWN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

#[test]
fn signing_a_message_for_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = d.engine.block_on(d.session.sign_message(
        d.id,
        OWN.into(),
        b"m".to_vec(),
        "no-grant".into(),
    ));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn an_address_book_entry_for_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let r = d.engine.block_on(d.session.save_address_book_entry(
        d.id,
        OWN.into(),
        "a".into(),
        crate::BookPurpose::Send,
        false,
    ));
    assert_gone(r.map_err(|e| e.code()));
}

#[test]
fn signing_mixing_inputs_of_a_wallet_gone_from_the_manager_does_not_panic() {
    use std::str::FromStr;
    let d = detached();
    let script = dashcore::Address::from_str(OWN)
        .unwrap()
        .assume_checked()
        .script_pubkey();
    let tx = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: outpoint(),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    };
    let r = d
        .engine
        .block_on(d.session.sign_mixing_inputs(d.id, &tx, &[(0, script)]));
    let e = r.expect_err("no wallet to sign with");
    assert!(e.0.starts_with("wallet not found"), "{e:?}");
}

#[test]
fn signing_a_fully_mixed_payment_of_a_wallet_gone_from_the_manager_does_not_panic() {
    let d = detached();
    let signer = d.session.vault.mixing_signer(&d.id.0).unwrap();
    let r = d.engine.block_on(async {
        (d.session)
            .sign_coinjoin_payment(d.id, &[], Vec::new(), &signer)
            .await
    });
    assert_gone(r.map_err(|e| e.code()));
}
