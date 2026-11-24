//! C-funding D-C5's shared rows (C5-0) over the contact fixture: a
//! `dp_contact_payout` row is the pair's lock across every source, a
//! restart keeps it while its source record exists and clears it as not
//! sent when none does, and `contact_pay` refuses in P2's order, every
//! source `platform.feature_off` until its slice lands.

use dw_appdb::DpContactPayoutRow;
use dw_appdb::dispatch::{ArtifactKind, ArtifactOrigin, NewShieldedArtifact};

use super::*;
use crate::platform::payment_locks::move_key;
use crate::platform::{ActivityFilter, PaymentSource};

const MOVE: &str = "move-7-0";

impl Fixture {
    fn payout(
        &self,
        to: u8,
        source: &str,
        reference: &str,
        st_hash: Option<[u8; 32]>,
    ) -> DpContactPayoutRow {
        DpContactPayoutRow {
            reference: reference.into(),
            identity: b58(ME),
            contact: b58(to),
            source: source.into(),
            st_hash: st_hash.map(hex::encode),
            address: self.contact_address(to, 0).to_string(),
            credits: 5_000_000,
            expected_duffs: 4_000,
            note: Some("lunch".into()),
            state: "pending".into(),
            since: 100,
        }
    }

    /// Writes `row` as a payout's row; `None` when written, else the
    /// holder's txid or reference.
    fn write_payout(&self, row: DpContactPayoutRow) -> Option<String> {
        let w = self.wallet.to_string();
        (self.engine)
            .block_on(
                (self.session).appdb_op(move |db| db.insert_dp_contact_payout_durable(&w, &row)),
            )
            .unwrap()
            .map(|h| h.id().to_owned())
    }

    /// The wallet's rows through a `.dwbackup`'s export, the wallet's
    /// removal and the import.
    fn restore_rows(&self) {
        let w = self.wallet.to_string();
        let imported = (self.engine)
            .block_on(self.session.appdb_op(move |db| {
                let rows = db.export_wallet_rows(&w)?;
                db.delete_wallet(&w)?;
                db.readmit_wallet(&w)?;
                db.import_wallet_rows(&w, &rows)
            }))
            .unwrap();
        assert!(imported > 0);
    }

    /// Makes app.sqlite's settings unreadable, or readable again.
    fn settings_readable(&self, readable: bool) {
        let path = self.session.data_dir().join(dw_appdb::APP_DB_FILE);
        let db = rusqlite::Connection::open(path).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        db.execute_batch(if readable {
            "ALTER TABLE settings_kv_away RENAME TO settings_kv;"
        } else {
            "ALTER TABLE settings_kv RENAME TO settings_kv_away;"
        })
        .unwrap();
    }

    fn activity(&self, to: u8) -> Vec<String> {
        let dp = self.session.dashpay(self.wallet);
        let page = (self.engine)
            .block_on(dp.contact_activity(b58(ME), b58(to), None, ActivityFilter::All))
            .unwrap();
        page.items.into_iter().map(|i| i.reference).collect()
    }

    /// The move record a Platform payout writes before it signs.
    fn write_move(&self, id: &str) {
        let scope = dw_appdb::local_scope(&self.wallet.to_string());
        let key = move_key(id);
        (self.engine)
            .block_on(
                (self.session).appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("{}"))),
            )
            .unwrap();
    }

    fn write_artifact(&self, st_hash: [u8; 32]) {
        let artifact = NewShieldedArtifact {
            activity: [9; 32],
            origin: ArtifactOrigin::Dispatched,
            st_hash: Some(st_hash),
            kind: Some(ArtifactKind::Unshield),
            anchor: Some([0xa1; 32]),
            identity_id: None,
            selected: vec![[102; 32]],
            published: vec![[102; 32]],
        };
        let (session, wallet) = (Arc::clone(&self.session), self.wallet);
        let recorded = (self.engine).block_on(async move {
            session
                .leases
                .record_shielded_artifact(wallet, artifact)
                .await
        });
        assert_eq!(recorded, Ok(true));
    }

    /// The wallet's payout rows, as (reference, state).
    fn payouts(&self) -> Vec<(String, String)> {
        let w = self.wallet.to_string();
        let rows = (self.engine)
            .block_on(self.session.appdb_op(move |db| db.dp_contact_payouts(&w)))
            .unwrap();
        rows.into_iter().map(|r| (r.reference, r.state)).collect()
    }

    fn restart(&self) {
        self.session.platform.payments.restart();
        self.engine
            .block_on(self.session.load_payment_locks(self.wallet));
    }

    fn contact_pay(
        &self,
        to: u8,
        credits: u64,
        source: PaymentSource,
        grant: &str,
    ) -> ContactError {
        let dp = self.session.dashpay(self.wallet);
        let paid = self.engine.block_on(dp.contact_pay(
            b58(ME),
            b58(to),
            credits,
            source,
            Some("lunch".into()),
            grant.into(),
            false,
        ));
        paid.expect_err("no source pays before its slice")
    }

    fn quote(&self, to: u8, source: PaymentSource) -> ContactError {
        let dp = self.session.dashpay(self.wallet);
        let quote = (self.engine).block_on(dp.contact_pay_quote(b58(ME), b58(to), 1_000, source));
        quote.expect_err("no source quotes before its slice")
    }
}

fn locked(e: ContactError) -> String {
    match e {
        ContactError::PaymentLocked { txid } => txid,
        other => panic!("expected contact.payment_locked, got {other:?}"),
    }
}

fn code(e: ContactError) -> &'static str {
    match e {
        ContactError::Platform(e) => e.code(),
        other => other.code(),
    }
}

/// P6: a restart reads a `pending` payout with its source record back as
/// the pair's lock, against a payment from every source; only its source's
/// evidence settles it, and resolving with none keeps it.
#[test]
fn a_restart_keeps_a_recorded_payout_as_the_lock_for_every_source() {
    let f = fixture(false);
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    f.write_move(MOVE);
    f.restart();
    let lock = f.lock(BOB).expect("the payout locks Bob");
    assert_eq!(
        (lock.txid.as_str(), lock.reference.as_str(), lock.source),
        (MOVE, MOVE, PaymentSource::Platform)
    );
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    let core = d.set_contact_recipient(f.pay(BOB, AMOUNT));
    assert_eq!(locked(contact_error(core)), MOVE);
    let grant = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10_000_000,
    });
    for source in [PaymentSource::Shielded, PaymentSource::Platform] {
        assert_eq!(locked(f.contact_pay(BOB, 1_000, source, &grant)), MOVE);
        assert_eq!(locked(f.quote(BOB, source)), MOVE);
    }
    assert_eq!(
        code(f.contact_pay(BOB, 1_000, PaymentSource::Core, &grant)),
        "invalid_argument"
    );
    assert_eq!(code(f.quote(BOB, PaymentSource::Core)), "invalid_argument");
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some(), "no evidence, no clear");
    assert_eq!(f.payouts(), [(MOVE.to_string(), "pending".to_string())]);
    assert!(f.reserved(BOB).is_none(), "the address stays consumed");

    // A shielded payout whose `st_hash` names an artifact holds too.
    let hash = [0x5a; 32];
    assert_eq!(
        f.write_payout(f.payout(CAROL, "shielded", "sh-1", Some(hash))),
        None
    );
    f.write_artifact(hash);
    f.restart();
    assert_eq!(
        f.lock(CAROL).map(|l| l.source),
        Some(PaymentSource::Shielded)
    );
}

/// P6: a `pending` payout with no source record was never signed: the
/// restart settles it not sent, unlocks the contact and gives its address
/// back to the pair, which the next Core payment pays.
#[test]
fn a_restart_clears_a_payout_with_no_source_record_and_returns_its_address() {
    let f = fixture(false);
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    f.restart();
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payouts(), [(MOVE.to_string(), "not_sent".to_string())]);
    assert_eq!(f.reserved(BOB), Some(f.contact_address(BOB, 0)));
    let d = f.draft(BOB);
    Seam::install(&d, Net::Accept);
    let p = f.prepare(&d).unwrap();
    assert_eq!(paid(&p), f.contact_address(BOB, 0).to_string());

    // Shielded: no `st_hash`, or one with no artifact.
    let hash = [0x5b; 32];
    assert_eq!(
        f.write_payout(f.payout(CAROL, "shielded", "sh-1", None)),
        None
    );
    f.restart();
    assert!(f.lock(CAROL).is_none());
    assert_eq!(
        f.write_payout(f.payout(CAROL, "shielded", "sh-2", Some(hash))),
        None
    );
    f.restart();
    assert!(f.lock(CAROL).is_none());
    let states: Vec<String> = f.payouts().into_iter().map(|(_, s)| s).collect();
    assert_eq!(states, ["not_sent"; 3]);
}

/// A restored `.dwbackup` carries the payout rows but not their source
/// records (the move record is `local:`, the artifact in dispatch.sqlite):
/// a row this install did not write stays the lock, its address consumed,
/// and still shows.
#[test]
fn a_restored_payout_stays_the_lock_without_its_source_record() {
    let f = fixture(false);
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    f.write_move(MOVE);
    let hash = [0x5c; 32];
    assert_eq!(
        f.write_payout(f.payout(CAROL, "shielded", "sh-1", Some(hash))),
        None
    );
    f.restore_rows();
    f.restart();
    assert_eq!(
        f.lock(BOB).map(|l| (l.reference, l.source)),
        Some((MOVE.to_string(), PaymentSource::Platform))
    );
    assert_eq!(
        f.lock(CAROL).map(|l| l.source),
        Some(PaymentSource::Shielded)
    );
    let states: Vec<String> = f.payouts().into_iter().map(|(_, s)| s).collect();
    assert_eq!(states, ["pending"; 2]);
    assert!(f.reserved(BOB).is_none(), "the address is not given back");
    assert!(f.reserved(CAROL).is_none());
    assert_eq!(f.activity(BOB), [MOVE]);
}

/// P6: a source record that cannot be read is not an absent one; the row
/// stays the lock and its address stays consumed.
#[test]
fn a_restart_keeps_a_payout_whose_source_record_cannot_be_read() {
    let f = fixture(false);
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    f.write_move(MOVE);
    f.settings_readable(false);
    f.restart();
    f.settings_readable(true);
    assert_eq!(f.lock(BOB).map(|l| l.reference), Some(MOVE.to_string()));
    assert_eq!(f.payouts(), [(MOVE.to_string(), "pending".to_string())]);
    assert!(f.reserved(BOB).is_none());

    let hash = [0x5d; 32];
    assert_eq!(
        f.write_payout(f.payout(CAROL, "shielded", "sh-1", Some(hash))),
        None
    );
    let journal = f.session.leases.journal.get();
    f.session.leases.journal.set(None);
    f.restart();
    f.session.leases.journal.set(journal);
    assert_eq!(
        f.lock(CAROL).map(|l| l.source),
        Some(PaymentSource::Shielded)
    );
    assert!(f.reserved(CAROL).is_none());
    assert!(
        f.payouts()
            .contains(&("sh-1".to_string(), "pending".to_string()))
    );
}

/// A payout whose call is under way here is left to it by a load.
#[test]
fn a_load_leaves_a_payout_under_way_alone() {
    let f = fixture(false);
    let pair = (id(ME), id(BOB));
    let address = f.contact_address(BOB, 0);
    (f.session.platform.payments).keep(f.wallet, pair, address.clone(), true);
    // P2 claims the pair before the payout's reference exists.
    let claimed = f.session.claim_payment(f.wallet, pair, "claim", &address);
    let Ok(_under_way) = claimed else {
        panic!("the claim");
    };
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    f.reload();
    assert_eq!(f.payouts(), [(MOVE.to_string(), "pending".to_string())]);
}

/// One lock per pair on disk: a Core lock row refuses a payout row, and a
/// pending payout row refuses a Core payment's hand-off even when memory
/// lost its lock.
#[test]
fn a_core_lock_and_a_payout_row_refuse_each_other() {
    let f = fixture(false);
    assert_eq!(f.write_payout(f.payout(BOB, "platform", MOVE, None)), None);
    let refused = f.engine.block_on(f.session.record_payment(
        f.wallet,
        (id(ME), id(BOB)),
        &Txid::from_byte_array([3; 32]).to_string(),
        &[],
    ));
    assert_eq!(locked(contact_error(refused)), MOVE);
    assert_eq!(f.lock(BOB).map(|l| l.source), Some(PaymentSource::Platform));
    assert!(f.rows().is_empty(), "no Core row written");

    f.establish(DAVE, false);
    let txid = Txid::from_byte_array([4; 32]);
    f.engine.block_on(
        f.session
            .lock_payment(f.wallet, (id(ME), id(DAVE)), txid, vec![]),
    );
    assert_eq!(
        f.write_payout(f.payout(DAVE, "shielded", "sh-1", None)),
        Some(txid.to_string())
    );
    let grant = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10_000_000,
    });
    assert_eq!(
        locked(f.contact_pay(DAVE, 1_000, PaymentSource::Shielded, &grant)),
        txid.to_string()
    );
}

/// P2's order: the arguments, the live payee, the pair's lock and rule 5
/// before the source's gates. Nothing before takes an address or the
/// grant.
#[test]
fn contact_pay_refuses_in_order_before_the_source() {
    let f = fixture(false);
    let grant = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10_000_000,
    });
    let pay = |to, credits, source| code(f.contact_pay(to, credits, source, &grant));
    assert_eq!(pay(BOB, 1_000, PaymentSource::Core), "invalid_argument");
    assert_eq!(pay(BOB, 0, PaymentSource::Shielded), "invalid_argument");
    assert_eq!(
        pay(DAVE, 1_000, PaymentSource::Shielded),
        "invalid_argument"
    );
    assert_eq!(
        pay(CAROL, 1_000, PaymentSource::Shielded),
        "contact.ineligible"
    );
    // C5b: Platform pays; this session has no Platform to reach.
    assert_eq!(
        pay(BOB, 1_000, PaymentSource::Platform),
        "platform.unavailable"
    );
    assert_eq!(
        code(f.quote(BOB, PaymentSource::Platform)),
        "platform.unavailable"
    );

    let platform = f.gated(TrustMode::Enforcing, Proof::Unverified);
    assert_eq!(
        pay(BOB, 1_000, PaymentSource::Shielded),
        "platform.context_unavailable"
    );
    assert!(platform.fetches() > 0);
    assert!(f.reserved(BOB).is_none(), "no address taken");
    assert!(f.payouts().is_empty());
    assert!(
        (f.session.vault())
            .check_grant(&grant, dw_vault::GrantKind::PlatformOp, Some(&f.wallet.0))
            .is_ok(),
        "the grant is kept"
    );
}

/// P15: "Accept and pay" takes its source; a non-Core one is off until
/// its slice, with every grant kept; a source on another flow is refused.
/// A payout's flow from Shielded is C5a's (`a_flow_lease_pays_one_payout`).
#[test]
fn begin_flow_takes_the_payment_source() {
    let f = fixture(false);
    let op = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10_000_000,
    });
    let begin = |flow, source| {
        let s = Arc::clone(&f.session);
        let grants = vec![op.clone()];
        f.engine
            .block_on(async move { s.begin_flow(f.wallet, flow, grants, source).await })
    };
    for (flow, source, code) in [
        (
            FlowKind::AcceptAndPay,
            Some(PaymentSource::Shielded),
            "platform.feature_off",
        ),
        (
            FlowKind::AcceptAndPay,
            Some(PaymentSource::Platform),
            "platform.feature_off",
        ),
        (
            FlowKind::ContactPayFromBalance,
            Some(PaymentSource::Core),
            "invalid_argument",
        ),
        (FlowKind::ContactPayFromBalance, None, "invalid_argument"),
        (
            FlowKind::ContactRequest,
            Some(PaymentSource::Core),
            "invalid_argument",
        ),
    ] {
        assert_eq!(
            begin(flow, source).unwrap_err().code(),
            code,
            "{flow:?} {source:?}"
        );
    }
    let lease = begin(FlowKind::AcceptAndPay, Some(PaymentSource::Core)).expect("Core as today");
    f.session.end_flow(lease).unwrap();
    // C5b: a payout from the Platform balance has its flow, from a grant of
    // its own (the one above went into the lease that just ended).
    let fresh = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10_000_000,
    });
    let s = Arc::clone(&f.session);
    let lease = (f.engine)
        .block_on(async move {
            s.begin_flow(
                f.wallet,
                FlowKind::ContactPayFromBalance,
                vec![fresh],
                Some(PaymentSource::Platform),
            )
            .await
        })
        .expect("the Platform source's flow");
    f.session.end_flow(lease).unwrap();
}
