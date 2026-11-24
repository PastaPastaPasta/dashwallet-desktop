//! C-funding D-C5's Platform source (C5b) over the contact fixture and a
//! scripted Platform: the payout pays the pair's reserved address as a
//! `ToContact` withdrawal, its row is on disk before anything is signed, it
//! is the pair's lock until the withdrawal's document or proof that it can
//! never execute settles it, and a held payout's way out is a "Move to Dash
//! wallet" at its inputs and nonces.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering as AtomicOrdering;

use dpp::address_funds::PlatformAddress;
use dpp::version::PlatformVersion;
use dw_vault::GrantKind;

use super::*;
use crate::platform::WithdrawAmount;
use crate::platform::contact_payout;
use crate::platform::payment_locks::PayCall;
use crate::platform::platform_moves::tests::{Act, FakeMoves};
use crate::platform::platform_moves::{self, Limits, MoveKind, MoveRecord};
use crate::platform::{
    ActivityFilter, ContactPayOutcome, PaymentSource, PayoutState, PlatformActivity,
    PlatformActivityKind, PlatformActivityState,
};

/// One Dash in credits.
const DASH: u64 = 100_000_000_000;
/// What a payout here pays.
const PAY: u64 = DASH / 10;

fn estimate() -> u64 {
    Limits::at(PlatformVersion::latest()).unwrap().fee(1)
}

/// What a one-input payout of [`PAY`] may take: the amount and three fee
/// estimates kept on its fee source.
fn debit() -> u64 {
    PAY + 3 * estimate()
}

fn code(e: &ContactError) -> &'static str {
    e.code()
}

impl Fixture {
    /// The scripted Platform, watching this wallet's moves.
    fn platform(&self) -> Arc<FakeMoves> {
        let net = FakeMoves::new();
        net.watch(&self.session, self.wallet);
        net
    }

    /// The payment account's addresses, in the wallet's order.
    fn pool(&self) -> Vec<PlatformAddress> {
        self.with_wallet(|w| async move {
            let pool = platform_moves::pool(&w).await.unwrap();
            pool.into_keys().collect()
        })
    }

    fn op_grant(&self, credits: u64) -> String {
        self.grant(GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits: credits,
        })
    }

    fn unused_grant(&self, grant: &str) -> bool {
        (self.session.vault())
            .check_grant(grant, GrantKind::PlatformOp, Some(&self.wallet.0))
            .is_ok()
    }

    /// Runs `body` on the session's runtime with the wallet; a panic in it
    /// (the process stopping) is `None`.
    fn on_platform<T: Send + 'static, Fut>(
        &self,
        body: impl FnOnce(Arc<NetworkSession>, Arc<PlatformWallet>) -> Fut + Send + 'static,
    ) -> Option<T>
    where
        Fut: Future<Output = T> + Send + 'static,
    {
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let task = self.session.rt.spawn(async move {
            let wallet = session.wallet(&id).await.unwrap();
            body(session, wallet).await
        });
        self.engine.block_on(task).ok()
    }

    fn platform_pay(
        &self,
        net: &Arc<FakeMoves>,
        to: u8,
        credits: u64,
        grant: &str,
    ) -> Option<Result<ContactPayOutcome, ContactError>> {
        let call = PayCall {
            identity: b58(ME),
            contact: b58(to),
            credits,
            source: PaymentSource::Platform,
            note: Some("lunch".into()),
            grant: grant.into(),
            confirm_unrecorded: false,
        };
        let (net, id) = (Arc::clone(net), self.wallet);
        self.on_platform(
            move |s, w| async move { contact_payout::pay(&*net, &s, &w, id, call).await },
        )
    }

    fn paid(
        &self,
        net: &Arc<FakeMoves>,
        to: u8,
        credits: u64,
        grant: &str,
    ) -> Result<ContactPayOutcome, ContactError> {
        self.platform_pay(net, to, credits, grant)
            .expect("the process runs on")
    }

    fn platform_quote(
        &self,
        net: &Arc<FakeMoves>,
        credits: u64,
    ) -> Result<crate::platform::ContactPayQuote, ContactError> {
        let (net, id) = (Arc::clone(net), self.wallet);
        self.on_platform(move |s, w| async move {
            contact_payout::quote(&*net, &s, &w, id, credits).await
        })
        .unwrap()
    }

    fn platform_activity(&self, net: &Arc<FakeMoves>) -> Vec<PlatformActivity> {
        let (net, id) = (Arc::clone(net), self.wallet);
        self.on_platform(move |s, w| async move {
            platform_moves::activity(&*net, &s, &w, id).await.unwrap()
        })
        .unwrap()
    }

    /// "Move to Dash wallet" of everything it may move, to the wallet's own
    /// receiving address.
    fn move_home(
        &self,
        net: &Arc<FakeMoves>,
        grant: &str,
    ) -> Result<crate::platform::MoveToCoreOutcome, crate::platform::CreditsError> {
        let (net, id, grant) = (Arc::clone(net), self.wallet, grant.to_owned());
        self.on_platform(move |s, w| async move {
            let to = s.current_receiving_address(id).await.unwrap().to_string();
            let ask = (WithdrawAmount::All, to.as_str(), None);
            platform_moves::move_to_core(&*net, &s, &w, id, ask, &grant).await
        })
        .unwrap()
    }

    fn move_to(
        &self,
        net: &Arc<FakeMoves>,
        to: String,
        grant: &str,
    ) -> Result<crate::platform::MoveToCoreOutcome, crate::platform::CreditsError> {
        let (net, id, grant) = (Arc::clone(net), self.wallet, grant.to_owned());
        self.on_platform(move |s, w| async move {
            let ask = (WithdrawAmount::All, to.as_str(), None);
            platform_moves::move_to_core(&*net, &s, &w, id, ask, &grant).await
        })
        .unwrap()
    }

    fn check_again(&self, net: &Arc<FakeMoves>, to: u8) -> LockResolution {
        let reference = self.lock(to).expect("a lock to check").reference;
        let (net, id) = (Arc::clone(net), self.wallet);
        self.on_platform(move |s, w| async move {
            let pair = (super::id(ME), super::id(to));
            contact_payout::resolve_with(&*net, &s, &w, id, pair, &reference)
                .await
                .unwrap()
        })
        .unwrap()
    }

    fn move_records(&self) -> Vec<MoveRecord> {
        let id = self.wallet;
        self.on_platform(move |s, _| async move { platform_moves::records(&s, id).await.unwrap() })
            .unwrap()
    }

    /// The wallet's payout rows, as (reference, state).
    fn payout_states(&self) -> Vec<(String, String)> {
        let w = self.wallet.to_string();
        let rows = (self.engine)
            .block_on(self.session.appdb_op(move |db| db.dp_contact_payouts(&w)))
            .unwrap();
        rows.into_iter().map(|r| (r.reference, r.state)).collect()
    }

    /// A restart: memory goes, the locks and payouts are read back.
    fn reload_locks(&self) {
        self.session.platform.payments.restart();
        self.engine
            .block_on(self.session.load_payment_locks(self.wallet));
    }

    fn contact_refs(&self, to: u8) -> Vec<String> {
        let dp = self.session.dashpay(self.wallet);
        let page = (self.engine)
            .block_on(dp.contact_activity(b58(ME), b58(to), None, ActivityFilter::All))
            .unwrap();
        page.items.into_iter().map(|i| i.reference).collect()
    }

    /// Which address a Core payment to `to` prepares now.
    fn next_core_address(&self, to: u8) -> String {
        let d = self.draft(to);
        Seam::install(&d, Net::Accept);
        let p = self.prepare(&d).unwrap();
        let address = paid(&p);
        self.engine.block_on(d.abandon(p)).unwrap();
        address
    }

    /// A funded payment account with `credits` on its first address.
    /// Contact `n`'s data checked, as [`Self::verify_contacts`] does for Bob.
    fn verify(&self, n: u8) {
        let subject = Subject::Payee(self.wallet, (id(ME), id(n)));
        let session = Arc::clone(&self.session);
        (self.engine)
            .block_on(session.on_runtime({
                let session = Arc::clone(&self.session);
                async move {
                    session.record_verified_for_tests(subject).await;
                    Ok(())
                }
            }))
            .unwrap();
    }

    fn funded(&self, credits: u64) -> (Arc<FakeMoves>, Vec<PlatformAddress>) {
        let net = self.platform();
        let pool = self.pool();
        net.fund(pool[0], credits);
        (net, pool)
    }
}

fn state_of(moves: &[MoveRecord], kind: MoveKind) -> (PlatformActivityState, bool) {
    let m = moves.iter().find(|m| m.kind == kind).unwrap();
    (m.state, m.group.is_some())
}

/// P2, P4, P10, P11: the quote names the amount, the fee and what the
/// contact receives, the grant the amount and the fee band; the payout's
/// row is on disk before its record, both before the signature; it pays
/// the pair's reserved address, settles sent on its document and consumes
/// the address; history and contact activity name the contact.
/// Mainnet is off ([`contact_payout::FEATURE`]): the quote and the payment,
/// directly and through the facade, and the flow's lease refuse
/// `platform.feature_off` before the grant, the pair's address, a row or a
/// record.
#[test]
fn mainnet_refuses_a_payment_from_the_platform_balance() {
    let dir = dw_testutil::private_tempdir();
    let mut f = fixture_on(dir.path(), false, DashNetwork::Mainnet);
    f._dir = Some(dir);
    let (net, _) = f.funded(DASH);
    let op = f.op_grant(4 * debit());
    let call = PayCall {
        identity: b58(ME),
        contact: b58(BOB),
        credits: PAY,
        source: PaymentSource::Platform,
        note: None,
        grant: op.clone(),
        confirm_unrecorded: false,
    };
    let facade = f.engine.block_on(f.session.contact_pay_of(f.wallet, call));
    let lease = f.engine.block_on(f.session.begin_flow(
        f.wallet,
        FlowKind::ContactPayFromBalance,
        vec![op.clone()],
        Some(PaymentSource::Platform),
    ));
    for e in [
        f.platform_quote(&net, PAY).unwrap_err(),
        f.paid(&net, BOB, PAY, &op).unwrap_err(),
        facade.unwrap_err(),
        lease.unwrap_err().into(),
    ] {
        assert!(
            matches!(&e, ContactError::Platform(PlatformError::FeatureOff { feature }) if feature == contact_payout::FEATURE),
            "{e:?}"
        );
    }
    assert!(f.unused_grant(&op));
    assert!(f.payout_states().is_empty());
    assert!(f.move_records().is_empty());
    assert!(f.contact_refs(BOB).is_empty());
    assert!(net.sent().is_empty());
}

#[test]
fn a_payout_pays_the_reserved_address_and_settles_sent() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    let quote = f.platform_quote(&net, PAY).unwrap();
    assert_eq!(quote.source, PaymentSource::Platform);
    assert_eq!((quote.credits, quote.fee_credits), (PAY, estimate()));
    assert_eq!(quote.grant.max_credits, debit());
    assert!(quote.expected_duffs > 0 && quote.expected_duffs < PAY / 1_000);

    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Sent);
    assert_eq!(*net.payout_rows_at_signing.lock().unwrap(), [1]);
    assert_eq!(*net.recorded_before_signing.lock().unwrap(), [true]);
    assert_eq!(f.payout_states(), [(out.reference.clone(), "sent".into())]);
    assert!(f.lock(BOB).is_none());
    let records = f.move_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, MoveKind::ToContact);
    assert_eq!(records[0].to, f.contact_address(BOB, 0).to_string());
    assert_eq!(records[0].id, out.reference);
    assert_eq!(net.documents(), 1);
    // Consumed: the next payment pays the contact's next address.
    assert!(f.reserved(BOB).is_none());
    assert_eq!(
        f.next_core_address(BOB),
        f.contact_address(BOB, 1).to_string()
    );

    assert_eq!(f.contact_refs(BOB), [out.reference.as_str()]);
    let activity = f.platform_activity(&net);
    let row = (activity.iter())
        .find(|r| r.kind == PlatformActivityKind::ContactPayment)
        .unwrap();
    assert_eq!(row.state, PlatformActivityState::Done);
    assert_eq!(
        row.contact.as_ref().map(|c| c.identity.clone()),
        Some(b58(BOB))
    );
    assert_eq!(row.to, Some(f.contact_address(BOB, 0).to_string()));
    assert!(
        !(activity.iter()).any(|r| r.kind == PlatformActivityKind::Outgoing),
        "{activity:?}"
    );
}

/// #533: a payout planned on a read behind a done payout would sign the
/// nonce that payout took. Refused before the grant is redeemed or an
/// address taken; once the read is current, the same grant pays.
#[test]
fn a_payout_on_a_read_behind_a_done_payout_is_refused() {
    let f = fixture(false);
    let (net, pool) = f.funded(DASH);
    let first = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(first.state, PayoutState::Sent);
    assert_eq!(net.sent()[0][&pool[0]].0, 1);

    net.lag(pool[0], true);
    let grant = f.op_grant(debit());
    let stale = f.paid(&net, BOB, PAY, &grant).unwrap_err();
    assert_eq!(code(&stale), "platform.unavailable");
    assert_eq!(net.sent().len(), 1, "nothing signed");
    assert!(f.unused_grant(&grant));
    assert!(f.reserved(BOB).is_none(), "no address taken");
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states().len(), 1);
    assert_eq!(f.move_records().len(), 1);

    net.lag(pool[0], false);
    let second = f.paid(&net, BOB, PAY, &grant).unwrap();
    assert_eq!(second.state, PayoutState::Sent);
    let sent = net.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1][&pool[0]].0, 2, "the nonce after the done payout's");
    let records = f.move_records();
    assert_eq!(records[1].to, f.contact_address(BOB, 1).to_string());
}

/// P2, P14: the withdrawal range and the grant's cap refuse before the
/// grant is redeemed and before an address is taken; nothing is signed.
#[test]
fn refusals_before_the_lease_keep_the_grant_and_the_address() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    let limits = Limits::at(PlatformVersion::latest()).unwrap();
    let grant = f.op_grant(DASH);
    let below = f.paid(&net, BOB, limits.min - 1, &grant).unwrap_err();
    assert_eq!(code(&below), "credits.withdrawal_out_of_range");
    let above = f.paid(&net, BOB, limits.max + 1, &grant).unwrap_err();
    assert_eq!(code(&above), "credits.withdrawal_out_of_range");
    assert_eq!(
        code(&f.platform_quote(&net, limits.min - 1).unwrap_err()),
        "credits.withdrawal_out_of_range"
    );
    assert!(f.unused_grant(&grant));
    let small = f.op_grant(PAY);
    let short = f.paid(&net, BOB, PAY, &small).unwrap_err();
    assert_eq!(code(&short), "platform.grant_exceeded");
    assert!(f.unused_grant(&small));
    let lease = f.engine.block_on(f.session.begin_flow(
        f.wallet,
        FlowKind::ContactPayment,
        vec![f.spend(AMOUNT)],
        None,
    ));
    let wrong = f.paid(&net, BOB, PAY, &lease.unwrap()).unwrap_err();
    assert_eq!(code(&wrong), "platform.grant_invalid");
    assert!(net.sent().is_empty());
    assert!(f.reserved(BOB).is_none(), "no address taken");
    assert!(f.payout_states().is_empty());
}

/// P6, P7 (C5-0's shared rows for this source): an unknown outcome locks
/// the contact against a payment from every source and a second payout
/// (a double submit signs once); a check with no evidence keeps it; the
/// withdrawal's document, once proved, settles it sent.
#[test]
fn an_unknown_payout_locks_the_contact_until_its_document_is_proved() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    net.then(Act::Lost);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    let lock = f.lock(BOB).unwrap();
    assert_eq!(
        (lock.source, lock.reference.as_str()),
        (PaymentSource::Platform, out.reference.as_str())
    );
    let again = f.op_grant(debit());
    let second = f.paid(&net, BOB, PAY, &again).unwrap_err();
    assert_eq!(
        second,
        ContactError::PaymentLocked {
            txid: out.reference.clone()
        }
    );
    assert!(f.unused_grant(&again));
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    let core = d.set_contact_recipient(f.pay(BOB, AMOUNT));
    assert!(matches!(
        contact_error(core),
        ContactError::PaymentLocked { .. }
    ));
    let dp = f.session.dashpay(f.wallet);
    let shielded = f.engine.block_on(dp.contact_pay(
        b58(ME),
        b58(BOB),
        PAY,
        PaymentSource::Shielded,
        None,
        again.clone(),
        false,
    ));
    assert!(matches!(shielded, Err(ContactError::PaymentLocked { .. })));
    assert_eq!(net.sent().len(), 1, "signed once");

    // No evidence: the lock stays, however often it is checked.
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    net.tick();
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some());
    // It executes after all: its document settles it.
    assert!(net.execute_sent(0));
    assert_eq!(f.check_again(&net, BOB), LockResolution::Sent);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states(), [(out.reference, "sent".into())]);
    assert!(f.reserved(BOB).is_none());
}

/// Mutant guard: the library's `Ok` with no execution behind it is no
/// evidence; the lock stays until a proof shows the payout's document.
#[test]
fn an_answer_without_proof_keeps_the_lock() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    net.then(Act::AnswersOnly);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    assert!(f.lock(BOB).is_some());
    f.platform_activity(&net);
    assert!(f.lock(BOB).is_some());
    assert!(net.execute_sent(0));
    f.platform_activity(&net);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "sent");
}

/// P5: refused with nothing signed, the payout can never execute: not
/// sent, the lock clear and the address back with the pair, which the next
/// payment pays.
#[test]
fn nothing_signed_returns_the_address_to_the_pair() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    net.then(Act::Unsigned);
    let refused = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap_err();
    assert_ne!(code(&refused), "contact.payment_locked");
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "not_sent");
    assert_eq!(f.reserved(BOB), Some(f.contact_address(BOB, 0)));
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Sent);
    let records = f.move_records();
    assert_eq!(
        records.last().unwrap().to,
        f.contact_address(BOB, 0).to_string()
    );
}

/// A submit that fails after signing: a consensus refusal may be a node's
/// answer, not proof, so the signed payout holds, the contact stays locked
/// and its address is consumed; its way out ("Move to Dash wallet" at its
/// inputs and nonces) settles it not sent. Mutant: read the refusal as
/// definitive (`never_valid`): the address goes back while the signed
/// bytes may still execute.
#[test]
fn a_refusal_after_signing_holds_until_evidence() {
    let f = fixture(false);
    let (net, pool) = f.funded(DASH);
    net.then(Act::Invalid);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    assert!(f.lock(BOB).is_some());
    assert!(f.reserved(BOB).is_none(), "the address is consumed");
    net.fund(pool[1], DASH);
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    let exit = f.move_home(&net, &f.op_grant(2 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "not_sent");
    assert!(f.reserved(BOB).is_none());
}

/// P8: another device takes one input's signed nonce of a held two-input
/// payout and leaves the other: the payout can never execute, and a fresh
/// proof without its document settles it not sent. Mutant: wait for every
/// input's nonce (`reached`): the contact stays locked for good.
#[test]
fn one_taken_nonce_settles_a_multi_input_payout() {
    let f = fixture(false);
    let (net, pool) = f.funded(PAY * 6 / 10);
    net.fund(pool[1], PAY * 6 / 10);
    net.then(Act::Invalid);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(2 * DASH)).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    let record = f.move_records().pop().unwrap();
    assert_eq!(record.inputs.len(), 2, "a two-input payout");
    net.take_nonce(pool[0]);
    assert_eq!(f.check_again(&net, BOB), LockResolution::NotSent);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "not_sent");
    assert!(f.reserved(BOB).is_none(), "the address stays consumed");
}

/// P15: a `ContactPayFromBalance` lease is one payment: once its payout
/// claimed an address it ends, so a resubmit under it signs nothing.
#[test]
fn a_contact_pay_from_balance_lease_pays_once() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    let lease = f
        .engine
        .block_on(f.session.begin_flow(
            f.wallet,
            FlowKind::ContactPayFromBalance,
            vec![f.op_grant(4 * debit())],
            Some(PaymentSource::Platform),
        ))
        .unwrap();
    let out = f.paid(&net, BOB, PAY, &lease).unwrap();
    assert_eq!(out.state, PayoutState::Sent);
    let again = f.paid(&net, BOB, PAY, &lease).unwrap_err();
    assert_eq!(code(&again), "platform.lease_expired", "{again:?}");
    assert_eq!(net.sent().len(), 1, "signed once");
}

/// A wallet restored from a `.dwbackup` has the payout row but not its
/// move record: the row's document id lets Check again prove it executed.
#[test]
fn a_restored_payout_settles_sent_on_its_document() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    net.then(Act::Lost);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    let w = f.wallet.to_string();
    (f.engine)
        .block_on(f.session.appdb_op(move |db| {
            let rows = db.export_wallet_rows(&w)?;
            db.delete_wallet(&w)?;
            db.readmit_wallet(&w)?;
            db.import_wallet_rows(&w, &rows)
        }))
        .unwrap();
    f.reload_locks();
    assert!(f.move_records().is_empty(), "the record stayed behind");
    assert!(f.lock(BOB).is_some());
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    // The lost broadcast reached a node after all.
    assert!(net.execute_sent(0));
    assert_eq!(f.check_again(&net, BOB), LockResolution::Sent);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "sent");
}

/// C5b: `to_contact_fee_shortfall_holds_and_to_core_exits`. The payout's
/// fee input falls short of Platform's metered fee, so it executes
/// nothing and holds, the contact locked; beside it a send, a payout to
/// another contact and a second payment to this one are refused; a "Move
/// to Dash wallet" at its inputs and nonces runs and executes; the proof
/// shows the move's document and not the payout's, so the row settles not
/// sent, the lock clears, and the address is not returned: the next
/// payment pays the next one. Mutants: return the address; settle on the
/// nonces alone (main's group rule: `replaced` and a group outcome).
#[test]
fn to_contact_fee_shortfall_holds_and_to_core_exits() {
    let f = fixture(false);
    f.establish(DAVE, false);
    f.verify_contacts();
    f.verify(DAVE);
    let (net, pool) = f.funded(debit() + estimate());
    let normal = net.fee();
    net.set_fee(10 * estimate());
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Pending);
    // A proof above the first read with its nonce unused: failed by
    // height, and still holding.
    net.fund(pool[1], DASH);
    f.platform_activity(&net);
    let records = f.move_records();
    assert_eq!(
        (records[0].state, records[0].holds()),
        (PlatformActivityState::Failed, true)
    );
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    let other = f.op_grant(debit());
    let dave = f.paid(&net, DAVE, PAY, &other).unwrap_err();
    assert_eq!(code(&dave), "credits.move_pending");
    assert!(f.unused_grant(&other));
    assert!(matches!(
        f.paid(&net, BOB, PAY, &other).unwrap_err(),
        ContactError::PaymentLocked { .. }
    ));

    // The way out, at the payout's inputs and nonces.
    net.set_fee(normal);
    let exit = f.move_home(&net, &f.op_grant(2 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    let sent = net.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&pool[0]]);
    assert_eq!(sent[1][&pool[0]].0, sent[0][&pool[0]].0, "the same nonce");
    let records = f.move_records();
    assert_eq!(
        state_of(&records, MoveKind::ToContact),
        (PlatformActivityState::Failed, false)
    );
    assert_eq!(
        state_of(&records, MoveKind::ToCore),
        (PlatformActivityState::Done, false)
    );
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states(), [(out.reference, "not_sent".into())]);
    assert!(f.reserved(BOB).is_none(), "the address stays consumed");
    assert_eq!(
        f.next_core_address(BOB),
        f.contact_address(BOB, 1).to_string()
    );
    let activity = f.platform_activity(&net);
    let paid_row = (activity.iter())
        .find(|r| r.kind == PlatformActivityKind::ContactPayment)
        .unwrap();
    assert_eq!(
        (paid_row.state, paid_row.may_land),
        (PlatformActivityState::Failed, false)
    );
}

/// The opposite branch: both signed at one nonce and the payout's
/// withdrawal is the one that executed: its document proved, the row is
/// sent, the `ToContact` record `done`, the move's `failed`, and no "Moved
/// to Dash wallet" row is done. Mutant: main's group rule.
#[test]
fn a_proved_payout_document_settles_sent_and_its_exit_failed() {
    let f = fixture(false);
    let (net, pool) = f.funded(DASH);
    net.then(Act::Lost);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    net.fund(pool[1], DASH);
    f.platform_activity(&net);
    net.then(Act::EarlierExecutes(0));
    let exit = f.move_home(&net, &f.op_grant(2 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Failed);
    let records = f.move_records();
    assert_eq!(
        state_of(&records, MoveKind::ToContact),
        (PlatformActivityState::Done, false)
    );
    assert_eq!(
        state_of(&records, MoveKind::ToCore),
        (PlatformActivityState::Failed, false)
    );
    assert_eq!(f.payout_states(), [(out.reference, "sent".into())]);
    assert!(f.lock(BOB).is_none());
    let activity = f.platform_activity(&net);
    assert!(
        !(activity.iter()).any(|r| r.kind == PlatformActivityKind::MoveToCore
            && r.state == PlatformActivityState::Done),
        "{activity:?}"
    );
}

/// P8: the nonce taken (another device) with neither document present
/// settles not sent, but only on a document proof at or above the nonce
/// proof's height: an older one decides nothing. Mutant: drop the height
/// check.
#[test]
fn a_taken_nonce_with_no_document_is_not_sent_only_on_a_fresh_proof() {
    let f = fixture(false);
    let (net, pool) = f.funded(DASH);
    net.then(Act::Lost);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    net.take_nonce(pool[0]);
    net.document_lag.store(3, AtomicOrdering::SeqCst);
    assert_eq!(f.check_again(&net, BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some());
    net.document_lag.store(0, AtomicOrdering::SeqCst);
    assert_eq!(f.check_again(&net, BOB), LockResolution::NotSent);
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states(), [(out.reference, "not_sent".into())]);
    assert!(f.reserved(BOB).is_none(), "the address stays consumed");
}

/// P8: the payout's fee input keeps three fee estimates. One credit short
/// of that is refused; at it the payout signs. Mutant: main's withdrawal
/// planner, which keeps one.
#[test]
fn the_payout_fee_input_keeps_three_fee_estimates() {
    let f = fixture(false);
    let (net, pool) = f.funded(debit() - 1);
    assert_eq!(
        code(&f.platform_quote(&net, PAY).unwrap_err()),
        "platform.insufficient_credits"
    );
    let grant = f.op_grant(debit());
    assert_eq!(
        code(&f.paid(&net, BOB, PAY, &grant).unwrap_err()),
        "platform.insufficient_credits"
    );
    assert!(f.unused_grant(&grant));
    net.fund(pool[0], 1);
    let out = f.paid(&net, BOB, PAY, &grant).unwrap();
    assert_eq!(out.state, PayoutState::Sent);
    let record = &f.move_records()[0];
    assert_eq!(record.credits, debit());
    assert_eq!(net.credits(pool[0]), 3 * estimate() - net.fee());
}

/// P4: "Move to Dash wallet" still refuses an address that is not the
/// wallet's own, a contact's included.
#[test]
fn move_to_core_still_refuses_a_contact_address() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    let grant = f.op_grant(2 * DASH);
    let theirs = f.contact_address(BOB, 0).to_string();
    let refused = f.move_to(&net, theirs, &grant).unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    assert!(net.sent().is_empty());
}

/// P6: a process stopped between the payout row and the move record never
/// signed: the relaunch settles the row not sent, unlocks the contact and
/// gives the address back to the pair. Mutant: keep such a row as the lock.
#[test]
fn a_kill_between_the_payout_row_and_the_move_record_clears_on_relaunch() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    (f.session.platform.payments.stop_after_row).store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        f.platform_pay(&net, BOB, PAY, &f.op_grant(debit()))
            .is_none()
    );
    assert_eq!(f.payout_states()[0].1, "pending");
    assert!(f.move_records().is_empty());
    assert!(net.sent().is_empty());
    f.reload_locks();
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states()[0].1, "not_sent");
    assert_eq!(f.reserved(BOB), Some(f.contact_address(BOB, 0)));
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    assert_eq!(out.state, PayoutState::Sent);
}

/// A resume after a crash at each step past the record: the relaunch
/// reads the row and its record back as the contact's lock, and the move's
/// evidence settles it: sent once the payout executed, else held until
/// its nonce is proved unused at a higher height, and then its way out
/// ("Move to Dash wallet") settles it not sent.
#[test]
fn a_payout_stopped_anywhere_after_its_record_is_settled_after_a_relaunch() {
    for (stop, executed) in [
        (Act::StopsBeforeSigning, false),
        (Act::StopsAfterSigning, false),
        (Act::StopsAfterBroadcast, true),
        (Act::StopsBeforeOutcome, true),
    ] {
        let f = fixture(false);
        let (net, pool) = f.funded(DASH);
        net.then(stop);
        assert!(
            f.platform_pay(&net, BOB, PAY, &f.op_grant(debit()))
                .is_none(),
            "{stop:?}"
        );
        f.reload_locks();
        let lock = f.lock(BOB).unwrap_or_else(|| panic!("{stop:?}: locked"));
        assert_eq!(lock.source, PaymentSource::Platform, "{stop:?}");
        if executed {
            assert_eq!(f.check_again(&net, BOB), LockResolution::Sent, "{stop:?}");
            assert_eq!(f.payout_states()[0].1, "sent", "{stop:?}");
            continue;
        }
        assert_eq!(
            f.check_again(&net, BOB),
            LockResolution::Unknown,
            "{stop:?}"
        );
        net.fund(pool[1], DASH);
        assert_eq!(
            f.check_again(&net, BOB),
            LockResolution::Unknown,
            "{stop:?}"
        );
        assert!(f.lock(BOB).is_some(), "{stop:?}");
        let exit = f.move_home(&net, &f.op_grant(2 * DASH)).unwrap();
        assert_eq!(exit.state, PlatformActivityState::Done, "{stop:?}");
        assert!(f.lock(BOB).is_none(), "{stop:?}");
        assert_eq!(f.payout_states()[0].1, "not_sent", "{stop:?}");
        assert!(f.reserved(BOB).is_none(), "{stop:?}");
    }
}

/// A restart after the record settled but before the row did (the outcome
/// write lost): the load settles the row from the record.
#[test]
fn a_relaunch_settles_a_row_its_record_already_settled() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    let out = f.paid(&net, BOB, PAY, &f.op_grant(debit())).unwrap();
    let w = f.wallet.to_string();
    let reference = out.reference.clone();
    let db = f.sqlite();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.execute(
        "UPDATE dp_contact_payout SET state = 'pending' WHERE wallet_id = ?1 AND reference = ?2",
        rusqlite::params![w, reference],
    )
    .unwrap();
    drop(db);
    f.reload_locks();
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_states(), [(out.reference, "sent".into())]);
}

/// `lock_order_contact_pay`: the payout waits out a holder of the move lock
/// (an activity read in flight) and both finish; no deadlock.
#[test]
fn lock_order_contact_pay() {
    let f = fixture(false);
    let (net, _) = f.funded(DASH);
    net.block_next_read();
    let (n, id) = (Arc::clone(&net), f.wallet);
    let s = Arc::clone(&f.session);
    let reading = f.session.rt.spawn(async move {
        let w = s.wallet(&id).await.unwrap();
        platform_moves::activity(&*n, &s, &w, id).await.map(drop)
    });
    f.engine.block_on(net.read_entered.notified());
    let call = PayCall {
        identity: b58(ME),
        contact: b58(BOB),
        credits: PAY,
        source: PaymentSource::Platform,
        note: None,
        grant: f.op_grant(debit()),
        confirm_unrecorded: false,
    };
    let (n, s) = (Arc::clone(&net), Arc::clone(&f.session));
    let paying = f.session.rt.spawn(async move {
        let w = s.wallet(&id).await.unwrap();
        contact_payout::pay(&*n, &s, &w, id, call).await
    });
    net.read_release.notify_one();
    let done = f.engine.block_on(async move {
        tokio::time::timeout(Duration::from_secs(60), async move {
            (reading.await, paying.await)
        })
        .await
    });
    let (reading, paying) = done.expect("no deadlock");
    reading.unwrap().unwrap();
    assert_eq!(paying.unwrap().unwrap().state, PayoutState::Sent);
}

/// A double submit: two payouts to one contact at once sign once while
/// the first's outcome is unknown; the other is refused with its grant
/// kept.
#[test]
fn two_payouts_at_once_sign_once() {
    let f = fixture(false);
    let (net, _) = f.funded(2 * DASH);
    net.then(Act::Lost);
    let grants = [f.op_grant(debit()), f.op_grant(debit())];
    let spawn = |grant: &String| {
        let call = PayCall {
            identity: b58(ME),
            contact: b58(BOB),
            credits: PAY,
            source: PaymentSource::Platform,
            note: None,
            grant: grant.clone(),
            confirm_unrecorded: false,
        };
        let (n, s, id) = (Arc::clone(&net), Arc::clone(&f.session), f.wallet);
        f.session.rt.spawn(async move {
            let w = s.wallet(&id).await.unwrap();
            contact_payout::pay(&*n, &s, &w, id, call).await
        })
    };
    let (a, b) = (spawn(&grants[0]), spawn(&grants[1]));
    let results = f
        .engine
        .block_on(async move { (a.await.unwrap(), b.await.unwrap()) });
    let results = [results.0, results.1];
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
    assert!(
        matches!(
            code(refused),
            "contact.payment_locked" | "credits.move_pending"
        ),
        "{refused:?}"
    );
    assert_eq!(net.sent().len(), 1);
    assert_eq!(grants.iter().filter(|g| f.unused_grant(g)).count(), 1);
}

/// The plan's band is the payout's own: a "Move to Dash wallet" keeps one
/// estimate, a payout three.
#[test]
fn the_contact_limits_keep_three_estimates() {
    let version = PlatformVersion::latest();
    let (to_core, to_contact) = (
        Limits::at(version).unwrap(),
        Limits::to_contact_at(version).unwrap(),
    );
    assert_eq!(to_core.keep(1), estimate());
    assert_eq!(to_contact.keep(1), 3 * estimate());
    let balances: BTreeMap<PlatformAddress, u64> =
        [(PlatformAddress::P2pkh([1; 20]), debit() - 1)].into();
    let amount = WithdrawAmount::Credits { credits: PAY };
    assert!(platform_moves::plan(&balances, amount, &to_core).is_ok());
    assert!(platform_moves::plan(&balances, amount, &to_contact).is_err());
}
