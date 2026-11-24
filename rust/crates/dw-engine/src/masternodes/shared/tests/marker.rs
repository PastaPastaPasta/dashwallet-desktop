//! #170 case 17: a shared registration's sent mark names only the inputs
//! this wallet signed. A mark restores a claim on each coin it names at
//! load; a co-participant's input that pays this wallet, unseen when the
//! wallet signed, must not be held by it after a restart. The registration
//! reaches the wallet as dash-spv relays it, through the production wallet
//! manager, and the funding blocks are scanned after the restart.

use std::collections::BTreeSet;

use dashcore::{Address, Block, BlockHash, ScriptBuf, TxIn};
use dw_vault::{Credential, GrantPurpose};
use platform_wallet::PlatformWalletInfo;
use platform_wallet::key_wallet_manager::{WalletEvent, WalletInterface, WalletManager};

use super::*;
use crate::send::{CoinSource, Recipient};

/// Someone else's regtest address.
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

/// Runs `op` on wallet 0's manager as dash-spv does, then hands the hub
/// its events and flushes the store.
fn deliver<F>(f: &Fx, op: F)
where
    F: for<'a> FnOnce(
            &'a mut WalletManager<PlatformWalletInfo>,
        ) -> futures_util::future::BoxFuture<'a, ()>
        + Send
        + 'static,
{
    use dash_spv::EventHandler;
    use platform_wallet::changeset::PlatformWalletPersistence;
    let manager = f.session.manager().unwrap();
    let events: Vec<WalletEvent> = f.run(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut rx = wm.subscribe_events();
        op(&mut wm).await;
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    });
    for event in &events {
        f.session.hub.on_wallet_event(event);
    }
    (f.session.live().unwrap().persister)
        .flush(f.wallets[0].0)
        .unwrap();
}

fn mempool(f: &Fx, tx: &Transaction) {
    let tx = tx.clone();
    deliver(f, move |wm| {
        Box::pin(async move {
            assert!(wm.process_mempool_transaction(&tx, None).await.is_relevant);
        })
    });
}

/// The scan reaches `height`, whose block holds `txs`.
fn mined(f: &Fx, height: u32, txs: Vec<Transaction>) {
    let id = f.wallets[0];
    deliver(f, move |wm| {
        Box::pin(async move {
            let mut block = Block {
                header: dashcore::blockdata::block::Header {
                    version: dashcore::blockdata::block::Version::from_consensus(1),
                    prev_blockhash: BlockHash::all_zeros(),
                    merkle_root: dashcore::TxMerkleNode::all_zeros(),
                    time: 1_793_598_712,
                    bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
                    nonce: height,
                },
                txdata: txs,
            };
            block.header.merkle_root = block.compute_merkle_root().unwrap();
            let hash = block.block_hash();
            wm.process_block_for_wallets(&block, hash, height, &BTreeSet::from([id.0]))
                .await;
            wm.update_wallet_synced_height(&id.0, height);
        })
    });
    f.tip(height);
}

fn receive(f: &Fx) -> Address {
    f.run(f.session.next_receive_address(f.wallets[0], None))
        .unwrap()
        .address
        .parse::<Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked()
}

/// `value` to `to` from a coin no wallet here owns.
fn paying(seed: u8, to: &Address, value: u64) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value,
            script_pubkey: to.script_pubkey(),
        }],
        special_transaction_payload: None,
    }
}

/// Wallet 0's coins, as key-wallet holds them.
fn held(f: &Fx) -> BTreeSet<OutPoint> {
    let (manager, id) = (f.session.manager().unwrap(), f.wallets[0]);
    f.run(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0).unwrap();
        (info.core_wallet.accounts.all_funding_accounts().into_iter())
            .flat_map(|a| a.utxos.keys().copied())
            .collect()
    })
}

fn total(f: &Fx) -> u64 {
    f.session.balances(&f.wallets[0]).unwrap().unwrap().total
}

/// Whether a payment from `coin` alone can be prepared.
fn offered(f: &Fx, coin: OutPoint) -> bool {
    let id = f.wallets[0];
    let draft = f.session.new_tx_draft(id).unwrap();
    draft
        .set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount: COIN,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    draft.set_source(CoinSource::Outpoints(vec![coin])).unwrap();
    let grant = (f.session.vault())
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 2 * COIN,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    f.run(draft.prepare(grant))
        .is_ok_and(|p| (p.summary().inputs.iter()).any(|i| i.outpoint == coin))
}

/// The coins wallet 0's mark of `txid` names.
fn marked(f: &Fx, txid: Txid) -> BTreeSet<OutPoint> {
    let (sent, read) = crate::tx_actions::load_sent(&f.db(), f.wallets[0]);
    assert!(read);
    sent.get(&txid).map(|m| m.coins.clone()).unwrap_or_default()
}

struct Shared {
    id: String,
    /// Wallet 0's input, from `paid_a`, mined.
    a: OutPoint,
    paid_a: Transaction,
    /// The dash-qt participant's input: an output of `paid_x` paying
    /// wallet 0, which the wallet hasn't seen.
    x: OutPoint,
    paid_x: Transaction,
    registration: Transaction,
}

/// Wallet 0 coordinates a registration with a dash-qt participant Q whose
/// contribution spends `x`, under a scriptSig that is no signature (the
/// other parties' scriptSigs aren't verified). The hand-off's outcome is
/// unknown; the network relays the registration back to the wallet.
fn shared_with_a_coin_paying_us(f: &Fx) -> Shared {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../../testdata/shared_registration.json"
    ))
    .unwrap();
    let party = &vectors["cases"][0]["parties"][1];
    let secret = *dashcore::PrivateKey::from_wif(party["owner_wif"].as_str().unwrap())
        .unwrap()
        .inner
        .to_bytes();
    let paid_a = paying(41, &receive(f), 701 * COIN);
    mined(f, 10, vec![paid_a.clone()]);
    let paid_x = paying(42, &receive(f), 300 * COIN);
    let (a, x) = (
        OutPoint::new(paid_a.txid(), 0),
        OutPoint::new(paid_x.txid(), 0),
    );

    let inv = start(f, 0, request(&[("A", 700), ("Q", 300)], FEE)).unwrap();
    let id = inv.session.id.clone();
    let mut q = parse(&inv.message.text);
    q.shares[1].owner_address = party["share"]["ownerAddress"].as_str().unwrap().into();
    q.shares[1].refund_address = party["share"]["refundAddress"].as_str().unwrap().into();
    q.contributions.push(Contribution {
        label: "Q".into(),
        inputs: vec![Input {
            txid: x.txid.to_string(),
            vout: x.vout,
            sequence: u32::MAX,
        }],
        change: None,
        change_index: -1,
    });
    dw_protx::shared::session::funding::rebuild_funding_tx(&mut q).unwrap();
    q.revision += 1;
    f.imported(0, &q.to_text());
    let locked = f.lock(0, &id).unwrap();
    let mut q = parse(&locked.message.text);
    let frozen = decode_frozen(&q).unwrap();
    let sig = dw_protx::payloads::sign_hash_compact(&secret, &frozen.consent_hash).unwrap();
    assert!(add_signature(&mut q, &frozen, 1, &encode_base64(&sig)).unwrap());
    assert_eq!(f.imported(0, &q.to_text()).0.stage, SharedStage::Combined);

    let mut q = parse(&f.message(0, &id));
    let mut tx = decode_frozen(&q).unwrap().tx;
    for input in tx.input.iter_mut().filter(|i| i.previous_output == x) {
        input.script_sig = ScriptBuf::from_bytes(vec![0x51]);
    }
    q.protx = hex::encode(tx.to_bytes());
    assert_eq!(f.imported(0, &q.to_text()).0.next, SharedNext::Send);

    f.net.unconfirm_next();
    f.send(0, &id, 700 * COIN + FEE).unwrap();
    assert!(f.row(0, &id).last_error.is_some(), "the outcome is unknown");
    let registration: Transaction =
        dashcore::consensus::deserialize(&f.row(0, &id).signed_tx.unwrap()).unwrap();
    let spent: BTreeSet<OutPoint> = (registration.input.iter())
        .map(|i| i.previous_output)
        .collect();
    assert_eq!(spent, BTreeSet::from([a, x]));
    mempool(f, &registration);
    Shared {
        id,
        a,
        paid_a,
        x,
        paid_x,
        registration,
    }
}

/// The mark of a shared registration whose hand-off's outcome is unknown
/// names this wallet's input, never the participant's.
#[test]
fn a_shared_registrations_mark_names_only_the_inputs_this_wallet_signed() {
    let f = fx(1, true);
    let s = shared_with_a_coin_paying_us(&f);
    assert_eq!(marked(&f, s.registration.txid()), BTreeSet::from([s.a]));
}

/// After a restart the participant's input is no claim: its funding block,
/// scanned then, credits the coin to the balance. (A send still doesn't
/// take it while the registration spending it is pending: the history's
/// `spent_outpoints`, as Core's `IsSpent`.)
#[test]
fn a_participants_input_paying_this_wallet_is_credited_after_a_restart() {
    let mut f = fx(1, true);
    let s = shared_with_a_coin_paying_us(&f);
    f.restart();
    let before = total(&f);
    mined(&f, 11, vec![s.paid_x.clone()]);
    assert!(held(&f).contains(&s.x), "{:?}", held(&f));
    assert_eq!(total(&f), before + 300 * COIN);
}

/// Guard: this wallet's own input stays claimed across the restart: its
/// funding block scanned again doesn't credit it, and no send takes it
/// while the registration's outcome is unknown.
#[test]
fn this_wallets_input_stays_claimed_across_a_restart() {
    let mut f = fx(1, true);
    let s = shared_with_a_coin_paying_us(&f);
    f.restart();
    assert_eq!(marked(&f, s.registration.txid()), BTreeSet::from([s.a]));
    let before = total(&f);
    mined(&f, 11, vec![s.paid_a.clone()]);
    assert!(!held(&f).contains(&s.a), "{:?}", held(&f));
    assert_eq!(total(&f), before);
    assert!(!offered(&f, s.a));
}

/// A resend after a restart writes the mark again from the session's
/// recorded contribution: a mark naming every input, as a version before
/// #170 case 17 wrote it, is replaced by one naming this wallet's input.
#[test]
fn a_resend_after_a_restart_marks_only_the_recorded_contribution() {
    let mut f = fx(1, true);
    let s = shared_with_a_coin_paying_us(&f);
    f.restart();
    let txid = s.registration.txid();
    let legacy = format!("{},{}", s.a, s.x);
    let key = format!("{}{txid}", crate::tx_actions::SENT_PREFIX);
    (f.db())
        .set_setting_durable(&f.wallets[0].to_string(), &key, Some(&legacy))
        .unwrap();
    assert_eq!(marked(&f, txid), BTreeSet::from([s.a, s.x]));
    f.send(0, &s.id, 700 * COIN + FEE).unwrap();
    assert_eq!(f.net.sent().last().map(Transaction::txid), Some(txid));
    assert_eq!(marked(&f, txid), BTreeSet::from([s.a]));
}
