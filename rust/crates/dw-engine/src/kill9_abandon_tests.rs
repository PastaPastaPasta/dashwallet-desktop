//! H-03 row R6 (a): an Abandon of a pending Core payment killed with
//! SIGKILL in a child process once its abandoned mark and its rescan record
//! are stored, before its sweep is stored and the rescan starts. The next
//! launch shows the payment abandoned, refuses Resend, offers its coins and
//! keeps the rescan of their block; once the scan passes it again, and on a
//! launch after that, the same, with the rescan done.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dashcore::{Address, OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose};

use super::own_spend_disk_tests::{
    Accept, COIN, COINS, FOREIGN, FUNDED_AT, coins, fresh_address, funding, synced, wait_stored,
};
use super::{BIRTH, PHRASE, balances, mempool, mined};
use crate::kill9;
use crate::send::{CoinSource, Recipient};
use crate::{Engine, NetworkSession, WalletId};

const ROW: &str = "abandon";
const CHILD: &str = "mempool_sync_tests::kill9_abandon_tests::abandon_child";
const PASS: &[u8] = b"kill9 abandon passphrase";
const AMOUNT: u64 = COIN * 5 / 2;

/// A payment of [`AMOUNT`] to [`FOREIGN`], handed off and returned.
fn pay(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> Transaction {
    let accept = Arc::new(Accept::default());
    let draft = session.new_tx_draft(id).unwrap();
    *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&accept) as _);
    draft
        .set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount: AMOUNT,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    draft.set_source(CoinSource::Any).unwrap();
    let grant = (session.vault())
        .authorize(
            GrantPurpose::Spend {
                max_duffs: AMOUNT + COIN,
            },
            Some(&id.0),
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id;
    let prepared = engine.block_on(draft.prepare(grant)).unwrap();
    engine.block_on(draft.broadcast(prepared)).unwrap();
    let sent = accept.0.lock().unwrap();
    assert_eq!(sent.len(), 1);
    sent[0].clone()
}

/// Run by [`kill9_r6a_abandon_killed_before_its_rescan_frees_the_coins_after_it`]
/// in a child process: a wallet funded in a mined block, a payment from it
/// stored unmined, and its Abandon held once its marks are stored.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn abandon_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    assert_eq!(child.point, "marked");
    let (engine, session) = kill9::engine_at(&child.root, PASS, false);
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    let payment = pay(&engine, &session, id);
    mempool(&engine, &session, id, &payment);
    wait_stored(&session, id, &[payment.txid()]);

    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.pause_abandon_sweep.lock().unwrap() = Some(Arc::clone(&pause));
    let (s, txid) = (Arc::clone(&session), payment.txid());
    let abandoning = session.rt.spawn(async move {
        let manager = s.manager()?;
        s.abandon_inner(&manager, id, txid).await
    });
    kill9::eventually("the abandon stored its marks", || {
        assert!(
            !abandoning.is_finished(),
            "the abandon ended before its pause"
        );
        pause.reached.load(Ordering::SeqCst)
    });
    let inputs: Vec<String> = (payment.input.iter())
        .map(|i| i.previous_output.to_string())
        .collect();
    kill9::reached(
        &child,
        &format!("{id}\n{txid}\n{}\n{}\n", inputs.join(","), funded.txid()),
    );
}

/// What the relaunched wallet shows of the payment and its coins.
#[derive(Debug)]
struct Seen {
    abandoned: bool,
    can_resend: bool,
    free: bool,
    synced: u32,
    total: u64,
}

fn seen(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
    inputs: &BTreeSet<OutPoint>,
) -> Seen {
    let free: BTreeSet<OutPoint> = coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    let resent = engine.block_on(session.resend_transaction(id, txid));
    Seen {
        abandoned: (session.hub.history.get(&id, &txid)).is_some_and(|e| e.is_abandoned()),
        can_resend: extras.can_resend
            || !matches!(
                resent,
                Err(crate::EngineError::TxActionRefused(
                    crate::tx_actions::TxActionRefusal::AlreadyAbandoned
                ))
            ),
        free: inputs.is_subset(&free),
        synced: synced(engine, session, id),
        total: balances(session, id).total,
    }
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r6a_abandon_killed_before_its_rescan_frees_the_coins_after_it() {
    let (dir, report) = kill9::killed_at(CHILD, ROW, "marked");
    let lines: Vec<&str> = report.lines().collect();
    let id: WalletId = lines[0].parse().unwrap();
    let txid = Txid::from_str(lines[1]).unwrap();
    let inputs: BTreeSet<OutPoint> = (lines[2].split(','))
        .map(|o| OutPoint::from_str(o).unwrap())
        .collect();
    let funding_txid = Txid::from_str(lines[3]).unwrap();
    let whole = COINS as u64 * COIN;
    let settled = |seen: &Seen, when: &str| {
        assert!(seen.abandoned, "{when}: not shown abandoned: {seen:?}");
        assert!(!seen.can_resend, "{when}: Resend offered: {seen:?}");
        assert!(seen.free, "{when}: its coins are held: {seen:?}");
        assert_eq!(seen.total, whole, "{when}: {seen:?}");
    };

    let (engine, session) = kill9::engine_at(dir.path(), PASS, false);
    let first = seen(&engine, &session, id, txid, &inputs);
    settled(&first, "relaunched");
    assert!(
        first.synced < FUNDED_AT,
        "the rescan of the coins' block is lost: {first:?}"
    );
    let funding_tx = (session.hub.history.get(&id, &funding_txid)).unwrap().tx;
    mined(&engine, &session, id, FUNDED_AT, vec![funding_tx]);
    let rescanned = seen(&engine, &session, id, txid, &inputs);
    settled(&rescanned, "rescanned");
    assert!(rescanned.synced >= FUNDED_AT, "{rescanned:?}");
    kill9::close(engine);

    let (engine, session) = kill9::engine_at(dir.path(), PASS, false);
    let again = seen(&engine, &session, id, txid, &inputs);
    settled(&again, "relaunched again");
    assert!(
        again.synced >= FUNDED_AT,
        "the rescan runs again: {again:?}"
    );
    kill9::close(engine);
}
