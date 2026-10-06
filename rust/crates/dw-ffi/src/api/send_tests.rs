//! The send, coin and label calls through the FFI on an empty wallet:
//! typed errors with their codes, no fake success. Funded flows are in
//! dw-engine's `send::flow_tests` and the regtest l1-send suite.

use std::sync::Arc;

use crate::{
    AddressPurpose, ChangePolicy, CoinSource, CoinsError, DashNetwork, Engine, EngineConfig,
    EngineEvent, EngineObserver, FeeMode, ImportOptions, LabelsError, OutPoint, Recipient,
    SendError, SessionOptions, UtxoFilter,
};

const ABANDON_12: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

struct Null;
impl EngineObserver for Null {
    fn on_event(&self, _: EngineEvent) {}
}

fn pay(address: &str, amount: u64) -> Recipient {
    Recipient {
        address: address.into(),
        amount,
        subtract_fee_from_amount: false,
        label: None,
        message: None,
    }
}

#[test]
fn send_coins_and_labels_map_engine_results() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().to_string_lossy().into_owned(),
            worker_threads: Some(2),
        },
        Arc::new(Null),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let session = rt
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
            },
        ))
        .unwrap();
    rt.block_on(session.vault().create(Some(b"ffi send test".to_vec())))
        .unwrap();
    let id = rt
        .block_on(session.import_wallet(
            ABANDON_12.into(),
            Vec::new(),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();

    let bad = session.new_tx_draft("XYZ".into());
    assert!(matches!(bad, Err(SendError::InvalidArgument { .. })));
    let missing = session.new_tx_draft("00".repeat(32));
    assert!(matches!(missing, Err(SendError::WalletNotFound { .. })));

    let draft = session.new_tx_draft(id.clone()).unwrap();
    assert_eq!(draft.wallet_id(), id);
    let e = draft
        .set_recipients(vec![pay(FOREIGN, 10_000), pay("nonsense", 10_000)])
        .unwrap_err();
    assert!(matches!(e, SendError::InvalidAddress { index: 1 }), "{e:?}");
    assert_eq!(e.code(), "send.invalid_address");
    let e = draft.set_recipients(vec![pay(FOREIGN, 100)]).unwrap_err();
    assert_eq!(e.code(), "send.dust_amount");
    draft.set_recipients(vec![pay(FOREIGN, 10_000)]).unwrap();
    let e = rt.block_on(draft.estimate()).unwrap_err();
    assert!(
        matches!(e, SendError::AmountExceedsBalance { available: 0 }),
        "{e:?}"
    );
    // The plan fails before the grant is looked at.
    let e = rt.block_on(draft.prepare("unknown".into())).unwrap_err();
    assert!(
        matches!(e, SendError::AmountExceedsBalance { available: 0 }),
        "{e:?}"
    );
    let e = draft.set_source(CoinSource::FullyMixedOnly).unwrap_err();
    assert_eq!(e.code(), "not_implemented");
    let e = draft
        .set_source(CoinSource::Outpoints {
            outpoints: vec![OutPoint {
                txid: "AB".repeat(32),
                vout: 0,
            }],
        })
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let e = draft
        .set_fee(FeeMode::PerKb { duffs_per_kb: 999 })
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let e = draft
        .set_change(ChangePolicy::Address {
            address: "bogus".into(),
        })
        .unwrap_err();
    assert_eq!(e.code(), "send.invalid_change_address");
    let max = rt
        .block_on(session.max_spendable(
            id.clone(),
            CoinSource::Any,
            FeeMode::Recommended { target_blocks: 6 },
        ))
        .unwrap();
    assert_eq!(max, 0);

    // Coins.
    let filter = UtxoFilter {
        include_locked: true,
        fully_mixed_only: false,
        min_confirmations: None,
    };
    assert!(
        rt.block_on(session.utxos(id.clone(), filter.clone()))
            .unwrap()
            .is_empty()
    );
    let r = rt.block_on(session.utxos(
        id.clone(),
        UtxoFilter {
            fully_mixed_only: true,
            ..filter
        },
    ));
    assert!(matches!(r, Err(CoinsError::NotImplemented { .. })), "{r:?}");
    let unknown = OutPoint {
        txid: "ab".repeat(32),
        vout: 3,
    };
    let r = rt.block_on(session.lock_outpoints(id.clone(), vec![unknown.clone()]));
    match r {
        Err(CoinsError::OutpointNotFound { outpoint }) => assert_eq!(outpoint, unknown),
        other => panic!("{other:?}"),
    }
    assert!(
        rt.block_on(session.locked_outpoints(id.clone()))
            .unwrap()
            .is_empty()
    );
    assert_eq!(rt.block_on(session.dust_protection()).unwrap(), None);
    rt.block_on(session.set_dust_protection(Some(10_000)))
        .unwrap();
    assert_eq!(
        rt.block_on(session.dust_protection()).unwrap(),
        Some(10_000)
    );
    let r = rt.block_on(session.set_dust_protection(Some(2_000_000)));
    assert!(matches!(r, Err(CoinsError::InvalidArgument { .. })), "{r:?}");

    // Labels.
    let entry = rt
        .block_on(session.save_address_book_entry(
            id.clone(),
            FOREIGN.into(),
            "Alice".into(),
            AddressPurpose::Send,
            false,
        ))
        .unwrap();
    assert_eq!(entry.label, "Alice");
    assert!(entry.created_at.is_some());
    let r = rt.block_on(session.save_address_book_entry(
        id.clone(),
        FOREIGN.into(),
        "Bob".into(),
        AddressPurpose::Send,
        false,
    ));
    assert!(matches!(r, Err(LabelsError::DuplicateAddress)), "{r:?}");
    let book = rt
        .block_on(session.address_book(
            id.clone(),
            Some(AddressPurpose::Send),
            Some("ali".into()),
        ))
        .unwrap();
    assert_eq!(book.len(), 1);
    rt.block_on(session.delete_address_book_entry(id.clone(), FOREIGN.into()))
        .unwrap();
    let r = rt.block_on(session.delete_address_book_entry(id.clone(), FOREIGN.into()));
    assert!(matches!(r, Err(LabelsError::EntryNotFound)), "{r:?}");
    let r = rt.block_on(session.set_tx_label(id.clone(), "xyz".into(), Some("x".into())));
    assert!(matches!(r, Err(LabelsError::InvalidArgument { .. })), "{r:?}");
    rt.block_on(session.set_tx_label(id, "ab".repeat(32), Some("rent".into())))
        .unwrap();

    rt.block_on(engine.shutdown()).unwrap();
    let r = session.new_tx_draft("00".repeat(32));
    assert!(matches!(r, Err(SendError::NetworkNotOpen { .. })), "{r:?}");
}
