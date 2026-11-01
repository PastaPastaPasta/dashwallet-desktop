use super::*;

fn snap(entries: &[(&str, u64)]) -> Snapshot {
    entries.iter().map(|(a, c)| ((*a).to_owned(), *c)).collect()
}

fn kinds(rows: &[PlatformActivity]) -> Vec<(PlatformActivityKind, String, u64)> {
    rows.iter()
        .map(|r| (r.kind, r.address.clone().unwrap_or_default(), r.credits))
        .collect()
}

/// The first read reports every funded address as incoming; later reads
/// report each rise and fall once, and an emptied address leaves the
/// snapshot.
#[test]
fn balance_changes_become_rows_once() {
    let mut unseen = BTreeMap::new();
    let held = BTreeSet::new();
    let first = snap(&[("a", 500), ("b", 70)]);
    let (rows, next) = changes(None, &first, &held, &mut unseen, 10);
    assert_eq!(
        kinds(&rows),
        vec![
            (PlatformActivityKind::Incoming, "a".into(), 500),
            (PlatformActivityKind::Incoming, "b".into(), 70),
        ]
    );
    assert_eq!(next, first);
    let (rows, again) = changes(Some(&next), &first, &held, &mut unseen, 11);
    assert!(rows.is_empty());
    assert_eq!(again, first);
    let later = snap(&[("a", 650)]);
    let (rows, next) = changes(Some(&first), &later, &held, &mut unseen, 12);
    assert_eq!(
        kinds(&rows),
        vec![
            (PlatformActivityKind::Incoming, "a".into(), 150),
            (PlatformActivityKind::Outgoing, "b".into(), 70),
        ]
    );
    assert_eq!(next, later);
    assert!(
        rows.iter()
            .all(|r| r.state == PlatformActivityState::Done && r.at == 12)
    );
}

/// A pending move's inputs are not compared (no outgoing row, the stored
/// value kept); a done move's fall is the move's, read once, and only what
/// exceeds it is outgoing.
#[test]
fn a_moves_own_spend_is_no_outgoing_row() {
    let before = snap(&[("a", 1_000), ("b", 400)]);
    let now = snap(&[("a", 100), ("b", 400)]);
    let held: BTreeSet<String> = ["a".to_owned()].into();
    let mut unseen = BTreeMap::new();
    let (rows, next) = changes(Some(&before), &now, &held, &mut unseen, 5);
    assert!(rows.is_empty());
    assert_eq!(
        next, before,
        "a pending move's input keeps its stored value"
    );

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 905)].into();
    let (rows, next) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, 6);
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(next, now);
    assert_eq!(unseen["a"], 0, "spent once read");

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 800)].into();
    let (rows, _) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, 7);
    assert_eq!(
        kinds(&rows),
        vec![(PlatformActivityKind::Outgoing, "a".into(), 100)]
    );
    // Not read yet (the cache did not move): kept for the next read.
    let mut unseen: BTreeMap<String, u64> = [("b".to_owned(), 50)].into();
    let (_, _) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, 8);
    assert_eq!(unseen["b"], 50);
}

fn record(at: u64, inputs: &[(&str, u32)]) -> MoveRecord {
    MoveRecord {
        id: "move-x".into(),
        inputs: inputs
            .iter()
            .map(|(a, n)| MoveInput {
                address: (*a).to_owned(),
                nonce: *n,
                credits: 1,
                unseen: 0,
            })
            .collect(),
        credits: 2,
        fee_credits: 1,
        expected_duffs: 0,
        to: "y".into(),
        at,
        state: PlatformActivityState::Pending,
    }
}

/// Done once every input's proved nonce reached the signed one; failed only
/// after ten minutes without; an unknown address or a failed read is no
/// evidence.
#[test]
fn a_pending_move_settles_on_its_nonces() {
    let r = record(1_000, &[("a", 4), ("b", 9)]);
    let proved = |a: Option<u32>, b: Option<u32>| -> BTreeMap<String, Option<u32>> {
        [("a".to_owned(), a), ("b".to_owned(), b)].into()
    };
    assert_eq!(
        settled(&r, &proved(Some(4), Some(9)), 1_001),
        PlatformActivityState::Done
    );
    assert_eq!(
        settled(&r, &proved(Some(5), Some(10)), 5_000),
        PlatformActivityState::Done
    );
    assert_eq!(
        settled(&r, &proved(Some(4), Some(8)), 1_599),
        PlatformActivityState::Pending
    );
    assert_eq!(
        settled(&r, &proved(Some(4), None), 1_100),
        PlatformActivityState::Pending
    );
    assert_eq!(
        settled(&r, &proved(Some(3), Some(8)), 1_600),
        PlatformActivityState::Failed
    );
    assert_eq!(
        settled(&r, &BTreeMap::new(), 1_600),
        PlatformActivityState::Failed
    );
}

#[test]
fn a_done_move_owns_each_inputs_fall() {
    let mut r = record(1, &[("a", 1), ("b", 1)]);
    r.done();
    assert_eq!(r.state, PlatformActivityState::Done);
    assert!(
        r.inputs
            .iter()
            .all(|i| i.unseen == i.credits + r.fee_credits)
    );
    let row = r.activity();
    assert_eq!(row.kind, PlatformActivityKind::MoveToCore);
    assert_eq!(row.to.as_deref(), Some("y"));
}

fn address(byte: u8) -> PlatformAddress {
    PlatformAddress::P2pkh([byte; 20])
}

/// An "all" plan over `balances`, the fee on the largest, as platform-wallet
/// builds it.
fn plan_of(balances: &[(u8, u64)], fee: u64) -> WithdrawalPlan {
    let mut inputs: BTreeMap<PlatformAddress, u64> =
        balances.iter().map(|(b, c)| (address(*b), *c)).collect();
    let (largest, _) = balances.iter().max_by_key(|(_, c)| *c).copied().unwrap();
    *inputs.get_mut(&address(largest)).unwrap() -= fee;
    let index = inputs.keys().position(|a| *a == address(largest)).unwrap();
    WithdrawalPlan {
        net_withdrawable: inputs.values().sum(),
        inputs,
        fee_strategy: vec![AddressFundsFeeStrategyStep::DeductFromInput(index as u16)],
        estimated_fee: fee,
    }
}

fn fee_source(
    inputs: &BTreeMap<PlatformAddress, u64>,
    strategy: &AddressFundsFeeStrategy,
) -> PlatformAddress {
    let [AddressFundsFeeStrategyStep::DeductFromInput(i)] = strategy.as_slice() else {
        panic!("{strategy:?}");
    };
    *inputs.keys().nth(usize::from(*i)).unwrap()
}

/// A partial move takes from the plan's fee source first (so the fee stays
/// on an input with room), then the largest, and points the strategy at the
/// fee source in the new input order.
#[test]
fn a_partial_move_keeps_the_fee_on_the_fee_source() {
    let plan = plan_of(&[(1, 300), (2, 1_000), (3, 500)], 10);
    let (inputs, strategy) = select(&plan, 400, 5).unwrap();
    assert_eq!(inputs, [(address(2), 400)].into());
    assert_eq!(fee_source(&inputs, &strategy), address(2));

    let (inputs, strategy) = select(&plan, 1_200, 5).unwrap();
    assert_eq!(inputs, [(address(2), 990), (address(3), 210)].into());
    assert_eq!(inputs.values().sum::<u64>(), 1_200);
    assert_eq!(fee_source(&inputs, &strategy), address(2));

    // Everything the plan withdraws: the plan's own inputs.
    let (inputs, _) = select(&plan, plan.net_withdrawable, 5).unwrap();
    assert_eq!(inputs, plan.inputs);
    assert!(select(&plan, plan.net_withdrawable + 1, 5).is_none());
}

/// A last input below the per-input minimum borrows from the one before;
/// when that cannot stay above the minimum, there is no selection.
#[test]
fn a_small_last_input_borrows_or_refuses() {
    let plan = plan_of(&[(1, 300), (2, 1_000)], 10);
    let (inputs, strategy) = select(&plan, 1_000, 50).unwrap();
    assert_eq!(inputs, [(address(2), 950), (address(1), 50)].into());
    assert_eq!(fee_source(&inputs, &strategy), address(2));
    assert!(select(&plan, 1_000, 960).is_none());
    assert!(select(&plan, 20, 50).is_none());
}
