//! Offline tests of the masternode keychain (IOS-083,
//! docs/contracts/m3-engine.md §2.5): provider keys on their DIP3 paths,
//! checked against an independent BIP32 derivation, and the reveal of one
//! private key under a `RevealSecret` grant.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    MasternodeFailure, MasternodeKeyRole, NetworkSession, SessionOptions, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

struct Quiet;

impl EventSink for Quiet {
    fn emit(&self, _event: EngineEvent) {}
}

fn engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Quiet),
    )
    .unwrap()
}

fn open(e: &Engine) -> Arc<NetworkSession> {
    let s = e
        .block_on(e.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    e.block_on(s.vault_op(|v| v.create(None))).unwrap();
    s
}

fn import(e: &Engine, s: &Arc<NetworkSession>) -> WalletId {
    e.block_on(s.import_wallet(
        Zeroizing::new(PHRASE.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(0),
            ..ImportOptions::default()
        },
    ))
    .unwrap()
}

/// The secp256k1 key at `path` of `PHRASE` on regtest: P2PKH address and WIF.
fn secp_at(path: &str) -> (String, String) {
    let secret = dw_vault::mnemonic::derive_secret(PHRASE.as_bytes(), b"", false).unwrap();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let child = master
        .derive_priv(&DerivationPath::from_str(path).unwrap())
        .unwrap();
    let pubkey = dashcore::PublicKey::new(child.private_key.public_key());
    let address = dashcore::Address::p2pkh(&pubkey, dashcore::Network::Regtest).to_string();
    let wif = dashcore::PrivateKey {
        compressed: true,
        network: dashcore::Network::Regtest,
        inner: child.private_key,
    }
    .to_wif();
    (address, wif)
}

fn reveal_grant(e: &Engine, s: &Arc<NetworkSession>, id: WalletId) -> String {
    e.block_on(
        s.vault_op(move |v| v.authorize(GrantPurpose::RevealSecret, Some(&id.0), Credential::None)),
    )
    .unwrap()
    .id
}

#[test]
fn test_ios_083_secp256k1_keys_match_their_dip3_paths() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e);
    let id = import(&e, &s);

    for (role, sub) in [
        (MasternodeKeyRole::Owner, 2),
        (MasternodeKeyRole::Voting, 1),
    ] {
        let keys = e.block_on(s.masternode_keys(id, role, 3, 2)).unwrap();
        assert_eq!(keys.iter().map(|k| k.index).collect::<Vec<_>>(), [3, 4]);
        for k in &keys {
            let path = format!("m/9'/1'/3'/{sub}'/{}", k.index);
            assert_eq!(k.derivation_path, path);
            assert_eq!(
                k.address.as_deref(),
                Some(secp_at(&path).0.as_str()),
                "{role:?}"
            );
            assert_eq!(k.public_key_hex.len(), 66, "compressed secp256k1");
            assert!(k.legacy_public_key_hex.is_none() && k.platform_node_id.is_none());
        }
    }

    let grant = reveal_grant(&e, &s, id);
    let revealed = e
        .block_on(s.reveal_masternode_key(id, MasternodeKeyRole::Owner, 4, grant.clone()))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(revealed.wif.as_deref().unwrap()).unwrap(),
        secp_at("m/9'/1'/3'/2'/4").1
    );
    assert_eq!(revealed.private_key_hex.len(), 64);
    assert!(revealed.tenderdash_key.is_none());

    // The grant is consumed.
    let again = e.block_on(s.reveal_masternode_key(id, MasternodeKeyRole::Owner, 4, grant));
    assert!(
        matches!(
            again,
            Err(EngineError::Masternode(MasternodeFailure::GrantInvalid))
        ),
        "{:?}",
        again.err()
    );
}

#[test]
fn test_ios_083_operator_and_platform_node_keys() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e);
    let id = import(&e, &s);

    let operator = e
        .block_on(s.masternode_keys(id, MasternodeKeyRole::Operator, 0, 2))
        .unwrap();
    assert_eq!(operator.len(), 2);
    assert_eq!(operator[1].derivation_path, "m/9'/1'/3'/3'/1");
    assert_eq!(operator[0].public_key_hex.len(), 96, "48-byte BLS key");
    let legacy = operator[0].legacy_public_key_hex.as_deref().unwrap();
    assert_eq!(legacy.len(), 96);
    assert_ne!(operator[0].public_key_hex, operator[1].public_key_hex);
    assert!(operator[0].address.is_none());

    // The pre-derived pool holds 20 platform node keys; later indexes are
    // left out.
    let nodes = e
        .block_on(s.masternode_keys(id, MasternodeKeyRole::PlatformNode, 18, 5))
        .unwrap();
    assert_eq!(nodes.iter().map(|k| k.index).collect::<Vec<_>>(), [18, 19]);
    assert_eq!(nodes[0].derivation_path, "m/9'/1'/3'/4'/18'");
    assert_eq!(nodes[0].public_key_hex.len(), 64);
    assert_eq!(
        nodes[0].platform_node_id.as_ref().map(String::len),
        Some(40)
    );

    let grant = reveal_grant(&e, &s, id);
    let revealed = e
        .block_on(s.reveal_masternode_key(id, MasternodeKeyRole::PlatformNode, 18, grant))
        .unwrap();
    assert!(revealed.wif.is_none());
    let tenderdash = revealed.tenderdash_key.as_deref().unwrap();
    assert_eq!(tenderdash.len(), 88, "base64 of 64 bytes");

    let grant = reveal_grant(&e, &s, id);
    let revealed = e
        .block_on(s.reveal_masternode_key(id, MasternodeKeyRole::Operator, 0, grant))
        .unwrap();
    assert_eq!(revealed.private_key_hex.len(), 64);
    assert!(revealed.wif.is_none() && revealed.tenderdash_key.is_none());

    assert!(matches!(
        e.block_on(s.masternode_keys(id, MasternodeKeyRole::Owner, 0, 101)),
        Err(EngineError::InvalidArgument(_))
    ));
}
