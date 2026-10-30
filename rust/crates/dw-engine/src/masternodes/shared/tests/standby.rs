//! The standby (MG-04b §9.4, slice 2b) on [`listed`]'s three-party
//! masternode: A (wallet 0) holds share 0 (400), B and C 300 each;
//! registered at 50, early period 1,000 blocks (the full variant is valid
//! from 1,050), penalty 1 DASH; the list at 100.

use dw_protx::shared::rules::SharedMn;
use dw_protx::shared::standby::{self, Variant};
use dw_protx::shared::{CollateralShare, ProDisTx, SpecialTx};
use dw_vault::GrantKind;

use super::dissolve::{assert_update_pending, dissolve_now, listed, listed_with};
use super::*;
use crate::masternodes::{
    ProTxOperation, ProTxOperationKind, ProTxOperationState, Standby, StandbyDescription,
    StandbyFile, StandbySource, StandbyVariant,
};

/// The masternode as Core's rules read it, from the frozen registration.
struct Reg {
    txid: Txid,
    vout: u32,
    shares: Vec<CollateralShare>,
    period: u32,
    penalty: i64,
}

impl Reg {
    fn of(f: &Fx, id: &str, txid: Txid) -> Self {
        let p = decode_frozen(&parse(&f.message(0, id))).unwrap().payload;
        Reg {
            txid,
            vout: p.collateral.vout,
            shares: p.shares,
            period: p.early_period_blocks,
            penalty: p.early_penalty,
        }
    }

    fn mn(&self) -> SharedMn<'_> {
        SharedMn {
            pro_tx_hash: self.txid,
            collateral: OutPoint::new(self.txid, self.vout),
            shares: &self.shares,
            registered_height: 50,
            early_period_blocks: self.period,
            early_penalty: self.penalty,
            voting_key_id: [1; 20],
        }
    }
}

fn create(f: &Fx, w: usize, txid: Txid, max: u64) -> Result<Vec<Standby>, MasternodesError> {
    f.run(
        f.mn()
            .create_standby(f.wallets[w], txid.to_string(), 1_000, f.grant(w, max)),
    )
}

fn standbys(f: &Fx, w: usize, txid: Txid) -> Vec<Standby> {
    f.run(f.mn().standby(f.wallets[w], txid.to_string()))
        .unwrap()
}

fn export(f: &Fx, w: usize, txid: Txid, share: u16) -> Result<StandbyFile, MasternodesError> {
    f.run(
        f.mn()
            .export_standby(f.wallets[w], txid.to_string(), share, 1_000, f.grant(w, 0)),
    )
}

/// [`export`] with the file's date estimated from `now`.
fn export_at(
    f: &Fx,
    w: usize,
    txid: Txid,
    share: u16,
    now: u64,
) -> Result<StandbyFile, MasternodesError> {
    f.run(f.mn().export_standby_at(
        f.wallets[w],
        txid.to_string(),
        share,
        1_000,
        f.grant(w, 0),
        now,
    ))
}

/// The hex lines under the file's two labels.
fn hex_lines(file: &StandbyFile) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = file.text.lines().collect();
    let under = |label: &str| {
        let at = lines.iter().position(|l| l.starts_with(label))?;
        Some(lines[at + 1].to_string())
    };
    (under("FULL PRINCIPAL"), under("IMMEDIATE"))
}

fn decode(hex_line: &str) -> SpecialTx {
    SpecialTx::from_bytes(&hex::decode(hex_line).unwrap()).unwrap()
}

fn send(
    f: &Fx,
    w: usize,
    txid: Txid,
    share: u16,
    variant: StandbyVariant,
    max: u64,
) -> Result<ProTxOperation, MasternodesError> {
    f.run(f.mn().send_standby(
        f.wallets[w],
        txid.to_string(),
        share,
        1_000,
        variant,
        f.grant(w, max),
    ))
}

fn import(f: &Fx, w: usize, text: &str) -> Result<SharedImport, MasternodesError> {
    f.run(
        f.mn()
            .import_shared_message(f.wallets[w], text.to_string(), None),
    )
}

fn imported_standby(f: &Fx, w: usize, text: &str) -> (StandbyDescription, bool) {
    match import(f, w, text).unwrap() {
        SharedImport::Standby {
            description,
            stored,
        } => (description, stored),
        other => panic!("not a standby: {other:?}"),
    }
}

fn list_at(f: &Fx, height: u32) {
    *f.seam.list_height.lock().unwrap() = height;
}

#[test]
fn test_QT_127_create_standby_signs_cores_pair_and_stores_it() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    let reg = Reg::of(&f, &id, txid);
    let off = f.run(f.sending_off().create_standby(
        f.wallets[0],
        txid.to_string(),
        1_000,
        f.grant(0, 2 * COIN),
    ));
    assert!(
        matches!(off, Err(MasternodesError::Unsupported { .. })),
        "{off:?}"
    );
    // Charged what the immediate variant gives up: the penalty and the
    // fee (§6.3).
    let short = create(&f, 0, txid, COIN + 999).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    assert!(standbys(&f, 0, txid).is_empty());

    let made = create(&f, 0, txid, COIN + 1_000).unwrap();
    let expected = Standby {
        pro_tx_hash: txid.to_string(),
        share_index: 0,
        fee: 1_000,
        full_valid_from: 1_050,
        full: true,
        immediate: true,
        source: StandbySource::Created,
        created_at: made[0].created_at,
    };
    assert_eq!(made, vec![expected.clone()]);
    // Signing again gives the same bytes: nothing new is stored. Both files
    // are dated from one clock reading, so a minute turning between them
    // changes nothing.
    let now = crate::coins::now_secs();
    let file = export_at(&f, 0, txid, 0, now).unwrap();
    assert_eq!(create(&f, 0, txid, COIN + 1_000).unwrap(), vec![expected]);
    assert_eq!(export_at(&f, 0, txid, 0, now).unwrap().text, file.text);
    // Nothing went out, nothing is an operation, nothing is locked.
    assert!(f.net.sent().is_empty());
    assert!(f.run(f.mn().operations(f.wallets[0])).unwrap().is_empty());
    assert!(f.locked(0).is_empty());

    // Core's file, its variants Core's builder's output signed by share
    // 0's owner key.
    let short_hash: String = txid.to_string().chars().take(8).collect();
    assert_eq!(
        file.file_name,
        format!("standby-dissolution-{short_hash}-share1.txt")
    );
    assert!(file.text.contains("\nShare 1 of 3\n"), "{}", file.text);
    assert!(
        file.text
            .contains("FULL PRINCIPAL — valid from block 1050 (~")
    );
    assert!(
        file.text
            .contains("IMMEDIATE — pays the 1\u{a0}tDASH early-exit penalty, valid right away")
    );
    assert!(file.text.ends_with('\n'));
    let (full, immediate) = hex_lines(&file);
    let (full, immediate) = (decode(&full.unwrap()), decode(&immediate.unwrap()));
    let mn = reg.mn();
    let pair = standby::pair(&mn, 0, 1_000).unwrap();
    let p: ProDisTx = dashcore::consensus::deserialize(&full.payload).unwrap();
    assert_eq!(full, pair.full.signed(p.sigs.clone()));
    let p: ProDisTx = dashcore::consensus::deserialize(&immediate.payload).unwrap();
    assert_eq!(immediate, pair.immediate.unwrap().signed(p.sigs.clone()));
    assert_eq!(
        standby::check(&full, &mn, 101).unwrap().variant,
        Variant::Full
    );
    assert_eq!(
        standby::check(&immediate, &mn, 101).unwrap().variant,
        Variant::Immediate
    );
    // The description holds no bytes (m-22).
    let described = serde_json::to_string(&standbys(&f, 0, txid)).unwrap();
    assert!(!described.contains(&hex::encode(full.to_bytes())[..40]));

    // The export is gated (D21): sending off, an unknown standby.
    let off = f.run(f.sending_off().export_standby(
        f.wallets[0],
        txid.to_string(),
        0,
        1_000,
        f.grant(0, 0),
    ));
    assert!(
        matches!(off, Err(MasternodesError::Unsupported { .. })),
        "{off:?}"
    );
    assert_eq!(
        code(export(&f, 0, txid, 1).unwrap_err()),
        "invalid_argument"
    );
    let no_grant = f.run(f.mn().export_standby(
        f.wallets[0],
        txid.to_string(),
        0,
        1_000,
        "not-a-grant".into(),
    ));
    assert!(no_grant.is_err());

    // B signs its own: share 1.
    let b = create(&f, 1, txid, COIN + 1_000).unwrap();
    assert_eq!((b.len(), b[0].share_index), (1, 1));
    assert_eq!(standbys(&f, 0, txid).len(), 1, "per wallet");
}

/// m-16: one block deep and listed; D29: the registration in the history;
/// a share whose owner key isn't the wallet's signs nothing.
#[test]
fn test_QT_127_create_standby_needs_the_listed_registration_and_a_share() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    list_at(&f, 50);
    let e = create(&f, 0, txid, 2 * COIN).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    list_at(&f, 100);
    let entries = f.seam.list.lock().unwrap().take();
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    let e = create(&f, 0, txid, 2 * COIN).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    // A list holding another masternode only.
    let mut others = entries.clone().unwrap();
    others[0].pro_tx_hash = [0x55; 32];
    *f.seam.list.lock().unwrap() = Some(others);
    let e = create(&f, 0, txid, 2 * COIN).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    *f.seam.list.lock().unwrap() = entries;
    let mined = std::mem::take(&mut *f.seam.shared_mined.lock().unwrap());
    let e = create(&f, 0, txid, 2 * COIN).unwrap_err();
    assert!(
        matches!(e, MasternodesError::SharedRegistrationUnknown),
        "{e:?}"
    );
    *f.seam.shared_mined.lock().unwrap() = mined;

    let other = outsider(&f);
    let e = f
        .run(f.mn().create_standby(
            other,
            txid.to_string(),
            1_000,
            outsider_grant(&f, other, 2 * COIN),
        ))
        .unwrap_err();
    assert!(matches!(e, MasternodesError::ShareNotMine), "{e:?}");
    // Below the relay minimum, or Core's builder refusing it.
    assert_eq!(
        code(
            f.run(
                f.mn()
                    .create_standby(f.wallets[0], txid.to_string(), 100, f.grant(0, 2 * COIN))
            )
            .unwrap_err()
        ),
        "invalid_argument"
    );
    assert_eq!(
        code(
            f.run(
                f.mn()
                    .create_standby(f.wallets[0], txid.to_string(), 0, f.grant(0, 2 * COIN))
            )
            .unwrap_err()
        ),
        "invalid_argument"
    );
    assert!(standbys(&f, 0, txid).is_empty());
}

/// D21: the file's signed bytes leave only under a `MasternodeOp` grant of
/// the wallet, charged nothing.
#[test]
fn test_QT_127_export_standby_needs_the_wallets_masternode_grant() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let export_with = |grant: String| {
        f.run(
            f.mn()
                .export_standby(f.wallets[0], txid.to_string(), 0, 1_000, grant),
        )
    };
    let spend = f
        .session
        .vault()
        .authorize(
            GrantPurpose::Spend { max_duffs: COIN },
            Some(&f.wallets[0].0),
            Credential::None,
        )
        .unwrap()
        .id;
    let revoked = f.grant(0, 0);
    f.session.vault().revoke_grant(&revoked);
    for grant in [spend, f.grant(1, 0), revoked, "nonsense".into()] {
        let e = export_with(grant.clone()).unwrap_err();
        assert!(code(e).starts_with("platform."), "{grant}");
    }
    let file = export_with(f.grant(0, 0)).unwrap();
    assert!(file.text.contains("FULL PRINCIPAL"));
}

/// A wallet of another seed: no share, no session.
fn outsider(f: &Fx) -> WalletId {
    f.run(f.session.import_wallet(
        Zeroizing::new(PHRASES[0].as_bytes().to_vec()),
        Zeroizing::new(b"another".to_vec()),
        ImportOptions {
            birth_height: Some(0),
            core_compat: false,
            ..Default::default()
        },
    ))
    .unwrap()
}

fn outsider_grant(f: &Fx, w: WalletId, max_duffs: u64) -> String {
    f.session
        .vault()
        .authorize(
            GrantPurpose::MasternodeOp { max_duffs },
            Some(&w.0),
            Credential::None,
        )
        .unwrap()
        .id
}

#[test]
fn test_QT_127_send_standby_sends_the_immediate_variant_early() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, immediate) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    // The full one isn't valid before 1,050: the network would refuse it
    // `bad-prodis-penalty-floor` (D85).
    let e = send(&f, 0, txid, 0, StandbyVariant::Full, 2 * COIN).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    let off = f.run(f.sending_off().send_standby(
        f.wallets[0],
        txid.to_string(),
        0,
        1_000,
        StandbyVariant::Immediate,
        f.grant(0, 2 * COIN),
    ));
    assert!(
        matches!(off, Err(MasternodesError::Unsupported { .. })),
        "{off:?}"
    );
    // Charged the penalty and the fee.
    let short = send(&f, 0, txid, 0, StandbyVariant::Immediate, COIN + 999).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    let sent_op = send(&f, 0, txid, 0, StandbyVariant::Immediate, COIN + 1_000).unwrap();
    let ops = f.run(f.mn().operations(f.wallets[0])).unwrap();
    assert_eq!(ops, vec![sent_op]);
    let op = &ops[0];
    assert_eq!(
        (op.kind, op.state, op.fee),
        (
            ProTxOperationKind::Dissolve,
            ProTxOperationState::Sent,
            1_000
        )
    );
    let sent = f.net.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        hex::encode(dashcore::consensus::serialize(&sent[0])),
        immediate.unwrap()
    );

    // D23 both ways: the standby holds `dissolve`, and the full variant.
    assert_update_pending(dissolve_now(&f, txid, 0).unwrap_err(), &op.id);
    list_at(&f, 1_049);
    let e = send(&f, 0, txid, 0, StandbyVariant::Full, 2 * COIN).unwrap_err();
    assert_update_pending(e, &op.id);
    // The resend charges the penalty and the fee again.
    let short = f.run(
        f.mn()
            .resend_operation(f.wallets[0], op.id.clone(), f.grant(0, COIN + 999)),
    );
    assert_eq!(send_code(&short.unwrap_err()), "send.grant_exceeded");
    assert!(full.is_some());
}

/// m-20: once the full variant is valid, the immediate one would pay the
/// penalty for nothing.
#[test]
fn test_QT_127_send_standby_refuses_the_immediate_once_the_full_is_valid() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, _) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    // At 1,048 a block mined next is still early.
    list_at(&f, 1_048);
    let e = send(&f, 0, txid, 0, StandbyVariant::Full, 2 * COIN).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    // The list lags the headers: the tip judges (review r1 m-2), as dash-qt
    // judges at the chain's tip.
    list_at(&f, 1_047);
    f.tip(1_049);
    let e = send(&f, 0, txid, 0, StandbyVariant::Immediate, 2 * COIN).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "full_valid"),
        "{e:?}"
    );
    assert!(f.net.sent().is_empty());
    // The full one is charged the fee alone.
    let short = send(&f, 0, txid, 0, StandbyVariant::Full, 999).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    let op = send(&f, 0, txid, 0, StandbyVariant::Full, 1_000).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(
        hex::encode(dashcore::consensus::serialize(&f.net.sent()[0])),
        full.unwrap()
    );
    // Its export now says it is valid.
    let file = export(&f, 0, txid, 0).unwrap();
    assert!(
        file.text
            .contains("FULL PRINCIPAL — valid now, the early period has ended\n")
    );
}

/// m-20 at penalty 0: one copy, valid at once, sent as the full one.
#[test]
fn test_QT_127_a_zero_penalty_standby_is_one_copy() {
    let f = fx(3, true);
    let (_, txid) = listed_with(&f, 0);
    // Charged the fee alone.
    let short = create(&f, 0, txid, 999).unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    let made = create(&f, 0, txid, 1_000).unwrap();
    assert_eq!((made[0].full, made[0].immediate), (true, false));
    let file = export(&f, 0, txid, 0).unwrap();
    let (full, immediate) = hex_lines(&file);
    assert_eq!(full, immediate, "Core's file prints the one copy twice");
    assert!(
        file.text
            .contains("pays the 0\u{a0}tDASH early-exit penalty")
    );
    let e = send(&f, 0, txid, 0, StandbyVariant::Immediate, COIN).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "full_valid"),
        "{e:?}"
    );
    let op = send(&f, 0, txid, 0, StandbyVariant::Full, 1_000).unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
}

/// m-19, D32: a pasted standby is checked, stored and sendable, by any
/// wallet whose history holds the registration.
#[test]
fn test_QT_127_an_imported_standby_is_checked_stored_and_sent() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, immediate) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    let (full, immediate) = (full.unwrap(), immediate.unwrap());

    // B imports A's file line by line.
    let (d, stored) = imported_standby(&f, 1, &format!("  {full}\n"));
    assert!(stored);
    assert_eq!(
        (d.variant, d.penalty, d.returns_principal),
        (StandbyVariant::Full, 0, true)
    );
    assert_eq!(d.txid, decode(&full).txid().to_string());
    assert_eq!(
        (
            d.standby.share_index,
            d.standby.fee,
            d.standby.full,
            d.standby.immediate,
            d.standby.source,
            d.standby.full_valid_from
        ),
        (0, 1_000, true, false, StandbySource::Imported, 1_050)
    );
    let amounts: Vec<_> = d.returns.iter().map(|r| (r.amount, r.mine)).collect();
    assert_eq!(
        amounts,
        [
            (400 * COIN - 1_000, false),
            (300 * COIN, true),
            (300 * COIN, false)
        ]
    );
    assert!(!imported_standby(&f, 1, &full).1, "held already");
    let (d, stored) = imported_standby(&f, 1, &immediate);
    assert!(stored);
    assert_eq!(
        (d.variant, d.penalty, d.returns_principal),
        (StandbyVariant::Immediate, COIN, false)
    );
    assert_eq!(
        (d.standby.full, d.standby.immediate),
        (true, true),
        "the second line completes the row"
    );
    assert_eq!(
        d.returns.iter().map(|r| r.amount).collect::<Vec<_>>(),
        [
            399 * COIN - 1_000,
            300 * COIN + COIN / 2,
            300 * COIN + COIN / 2
        ]
    );
    // A's own: held already, its source kept.
    let (d, stored) = imported_standby(&f, 0, &full);
    assert!(!stored);
    assert_eq!(d.standby.source, StandbySource::Created);

    // A wallet of another seed (no share, no session) sends it: it gives
    // up nothing of its own, so a grant of 0 covers it (§6.3).
    let other = outsider(&f);
    let text = immediate.clone();
    let imported = f
        .run(f.mn().import_shared_message(other, text, None))
        .unwrap();
    let SharedImport::Standby { description, .. } = imported else {
        panic!("not a standby");
    };
    assert!(description.returns.iter().all(|r| !r.mine));
    let op = f
        .run(f.mn().send_standby(
            other,
            txid.to_string(),
            0,
            1_000,
            StandbyVariant::Immediate,
            outsider_grant(&f, other, 0),
        ))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(
        hex::encode(dashcore::consensus::serialize(&f.net.sent()[0])),
        immediate
    );
    // Its resend is charged nothing either.
    f.run(
        f.mn()
            .resend_operation(other, op.id.clone(), outsider_grant(&f, other, 0)),
    )
    .unwrap();
    // It pays this wallet nothing, so it settles by the list as an unseen
    // dissolution does (D81, D87), without a session: once the masternode
    // is out of it and the wallet has scanned to its height.
    let state = || {
        let ops = f.run(f.mn().operations(other)).unwrap();
        ops.into_iter().find(|o| o.id == op.id).unwrap().state
    };
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    *f.seam.list_height.lock().unwrap() = 1_100;
    f.tip(1_100);
    assert_eq!(state(), ProTxOperationState::Sent, "the scan is behind");
    let mut scan = f.session.hub.wallet_state(&other).unwrap_or_default();
    (scan.synced_height, scan.processed_height) = (1_100, 1_100);
    f.session.hub.set_wallet_state(other, scan);
    assert_eq!(state(), ProTxOperationState::Confirmed { height: 1_100 });
}

#[test]
fn test_QT_127_an_imported_standby_is_refused_with_the_failing_check() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, _) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    let tx = decode(&full.unwrap());
    let hex_of = |tx: &SpecialTx| hex::encode(tx.to_bytes());

    // Two signatures: not unilateral.
    let mut two = tx.clone();
    let mut p: ProDisTx = dashcore::consensus::deserialize(&two.payload).unwrap();
    p.sigs.push(p.sigs[0]);
    two.payload = dw_protx::shared::payload_bytes(&p);
    assert_message(import(&f, 1, &hex_of(&two)).unwrap_err(), "not_standby");
    // Another payee: Core's rule.
    let mut payee = tx.clone();
    payee.output.swap(0, 1);
    let e = import(&f, 1, &hex_of(&payee)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-prodis-payee"),
        "{e:?}"
    );
    // A changed amount: the signature no longer verifies.
    let mut amount = tx.clone();
    amount.output[0].value += 1;
    amount.output[2].value -= 1;
    let e = import(&f, 1, &hex_of(&amount)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-prodis-sig"),
        "{e:?}"
    );
    // Not listed, not in the history.
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    let e = import(&f, 1, &hex_of(&tx)).unwrap_err();
    assert!(matches!(e, MasternodesError::SharedTooEarly), "{e:?}");
    let mined = std::mem::take(&mut *f.seam.shared_mined.lock().unwrap());
    let e = import(&f, 1, &hex_of(&tx)).unwrap_err();
    assert!(
        matches!(e, MasternodesError::SharedRegistrationUnknown),
        "{e:?}"
    );
    *f.seam.shared_mined.lock().unwrap() = mined;
    assert!(standbys(&f, 1, txid).is_empty(), "nothing stored");
}

/// Review r1 m-1: dashd's RPC builds a standby at any fee. One below the
/// relay minimum is refused on import, as `create_standby` refuses it, and
/// one stored before is refused on send, nothing saved or charged: sent, it
/// would stay `sent` and hold the masternode's updates (D23).
#[test]
fn test_QT_127_a_standby_below_the_relay_minimum_is_refused() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, _) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    // `protx shared_dissolve <hash> 0 100 false false`: the actor's output,
    // last, keeps 900 more; signed by its owner key.
    let mut cheap = decode(&full.unwrap());
    cheap.output.last_mut().unwrap().value += 900;
    let mut p: ProDisTx = dashcore::consensus::deserialize(&cheap.payload).unwrap();
    p.sigs.clear();
    cheap.payload = dw_protx::shared::payload_bytes(&p);
    let registration = {
        let mined = f.seam.shared_mined.lock().unwrap();
        let (_, typed, _) = mined.iter().find(|(t, _, _)| *t == txid).unwrap();
        dashcore::consensus::serialize(typed)
    };
    let (_, reg) =
        dw_protx::shared::session::funding::decode_shared_protx(&hex::encode(&registration))
            .unwrap();
    let index = f
        .run(
            f.session
                .keychain_owner(f.wallets[0], reg.shares[0].owner_key_id),
        )
        .unwrap()
        .unwrap();
    let vault = f.session.vault();
    let token = vault
        .redeem_grant(
            &f.grant(0, 0),
            GrantKind::MasternodeOp,
            Some(&f.wallets[0].0),
        )
        .unwrap();
    let sig = vault
        .masternode_signer(&f.wallets[0].0, &token)
        .unwrap()
        .sign_dissolution(index, &cheap.to_bytes(), &registration, 1)
        .unwrap();
    p.sigs = vec![sig];
    cheap.payload = dw_protx::shared::payload_bytes(&p);
    let cheap_hex = hex::encode(cheap.to_bytes());

    let e = import(&f, 1, &cheap_hex).unwrap_err();
    assert!(e.to_string().contains("relay minimum"), "{e}");
    assert_eq!(code(e), "invalid_argument");
    assert!(standbys(&f, 1, txid).is_empty(), "nothing stored");

    // Stored before the import refused it.
    f.db()
        .store_standby_durable(
            &f.wallets[0].to_string(),
            &dw_appdb::StandbyRow {
                pro_tx_hash: txid.to_string(),
                share_index: 0,
                fee: 100,
                full_tx: Some(cheap.to_bytes()),
                immediate_tx: None,
                full_valid_from: 1_050,
                source: "imported".into(),
                created_at: 0,
            },
        )
        .unwrap();
    list_at(&f, 1_049);
    let grant = f.grant(0, 2 * COIN);
    let e = f
        .run(f.mn().send_standby(
            f.wallets[0],
            txid.to_string(),
            0,
            100,
            StandbyVariant::Full,
            grant.clone(),
        ))
        .unwrap_err();
    assert!(e.to_string().contains("relay minimum"), "{e}");
    assert_eq!(code(e), "invalid_argument");
    assert!(f.net.sent().is_empty());
    assert!(f.run(f.mn().operations(f.wallets[0])).unwrap().is_empty());
    // Nothing charged: the grant, untouched, sends the relayable one.
    f.run(f.mn().send_standby(
        f.wallets[0],
        txid.to_string(),
        0,
        1_000,
        StandbyVariant::Full,
        grant,
    ))
    .unwrap();
}

/// Review r1 n-2: the send checks that the bytes it read are the variant
/// asked for, whatever the store holds.
#[test]
fn test_QT_127_send_standby_refuses_a_variant_in_the_wrong_column() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    let (full, _) = hex_lines(&export(&f, 0, txid, 0).unwrap());
    f.db()
        .store_standby_durable(
            &f.wallets[0].to_string(),
            &dw_appdb::StandbyRow {
                pro_tx_hash: txid.to_string(),
                share_index: 0,
                fee: 2_000,
                full_tx: None,
                immediate_tx: Some(decode(&full.unwrap()).to_bytes()),
                full_valid_from: 1_050,
                source: "imported".into(),
                created_at: 0,
            },
        )
        .unwrap();
    let e = f
        .run(f.mn().send_standby(
            f.wallets[0],
            txid.to_string(),
            0,
            2_000,
            StandbyVariant::Immediate,
            f.grant(0, 2 * COIN),
        ))
        .unwrap_err();
    assert_eq!(code(e), "internal");
    assert!(f.net.sent().is_empty());
}

/// Review r1 n-1: a standby shows the period's end from the registration's
/// current block, not the one stored.
#[test]
fn test_QT_127_a_standby_shows_the_registrations_current_block() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let created = create(&f, 0, txid, COIN + 1_000).unwrap();
    assert_eq!(created[0].full_valid_from, 1_050);
    // A reorg moves the registration three blocks later.
    for (t, _, at) in f.seam.shared_mined.lock().unwrap().iter_mut() {
        if *t == txid {
            *at = 53;
        }
    }
    assert_eq!(standbys(&f, 0, txid)[0].full_valid_from, 1_053);
    let file = export(&f, 0, txid, 0).unwrap();
    assert!(file.text.contains("valid from block 1053"), "{}", file.text);
}

/// D83: a standby whose actor output is dust is signed as Core signs it,
/// but not sent: no node would relay it.
#[test]
fn test_QT_127_send_standby_refuses_a_dust_actor_output() {
    let f = fx(3, true);
    let (_, txid) = listed_with(&f, 300 * COIN - 1_100);
    create(&f, 1, txid, 300 * COIN).unwrap();
    let e = send(&f, 1, txid, 1, StandbyVariant::Immediate, 300 * COIN).unwrap_err();
    assert!(e.to_string().contains("dust"), "{e}");
    assert_eq!(code(e), "invalid_argument");
    assert!(f.net.sent().is_empty());
    assert!(f.run(f.mn().operations(f.wallets[1])).unwrap().is_empty());
}

/// D21: a confirmed registration prompts `create_standby` until each of
/// the wallet's shares has one.
#[test]
fn test_QT_127_a_confirmed_session_prompts_create_standby() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    let s = f.session_of(0, &id);
    assert!(matches!(s.stage, SharedStage::Confirmed { .. }));
    assert_eq!(s.next, SharedNext::CreateStandby);
    let all = f.run(f.mn().shared_sessions(f.wallets[0])).unwrap();
    assert_eq!(all[0].next, SharedNext::CreateStandby);
    // Out of the list (dissolved), or no synced list: nothing to sign.
    let listed = f.seam.list.lock().unwrap().replace(Vec::new());
    assert_eq!(f.session_of(0, &id).next, SharedNext::Nothing);
    *f.seam.list.lock().unwrap() = None;
    assert_eq!(f.session_of(0, &id).next, SharedNext::Nothing);
    *f.seam.list.lock().unwrap() = listed;
    assert_eq!(f.session_of(0, &id).next, SharedNext::CreateStandby);
    // Another wallet's standby of another share doesn't count.
    create(&f, 1, txid, COIN + 1_000).unwrap();
    assert_eq!(f.session_of(0, &id).next, SharedNext::CreateStandby);
    create(&f, 0, txid, COIN + 1_000).unwrap();
    assert_eq!(f.session_of(0, &id).next, SharedNext::Nothing);
    let all = f.run(f.mn().shared_sessions(f.wallets[0])).unwrap();
    assert_eq!(all[0].next, SharedNext::Nothing);
}
