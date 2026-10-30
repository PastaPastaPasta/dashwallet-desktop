//! Core's dissolution builder (MG-04b §5, part 2): `BuildDissolution`
//! (`evo/providertx_service.cpp:639-681`, rc.3) and `SplitAmountByShares`
//! (`evo/providertx_util.cpp:97-115`), output for output, so a dissolution
//! the wallet signs is the one dashd's `protx shared_dissolve` would.

use dashcore::{ScriptBuf, TxIn, TxOut};

use super::rawtx::{SPECIAL_VERSION, SpecialTx};
use super::rules::{SharedMn, pro_rata_floor};
use super::{
    CollateralShare, CompactSignature, PRODIS_MAX_FEE, ProDisTx, TRANSACTION_PROVIDER_DISSOLVE,
    payload_bytes,
};

/// `SplitAmountByShares`: `total` split over `shares` by amount, each but
/// the last `ProRataFloor`, the last the remainder, so the parts sum to
/// `total` exactly.
pub fn split_amount_by_shares(total: u64, shares: &[&CollateralShare]) -> Vec<u64> {
    let weight_total: u64 = shares.iter().map(|s| s.amount).sum();
    let mut paid = 0;
    let mut parts = Vec::with_capacity(shares.len());
    for (i, share) in shares.iter().enumerate() {
        let part = if i + 1 == shares.len() {
            total - paid
        } else {
            pro_rata_floor(total, share.amount, weight_total)
        };
        paid += part;
        parts.push(part);
    }
    parts
}

/// Why `BuildDissolution` refuses, with Core's texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("actorIndex out of range")]
    ActorOutOfRange,
    #[error("fee must be positive")]
    FeeNotPositive,
    #[error("fee exceeds the consensus ceiling of 1000000 duffs")]
    FeeAboveCeiling,
    #[error("penalty and fee exceed the actor's share")]
    ActorOverdrawn,
}

/// An unsigned dissolution: the transaction (its payload without
/// signatures) and the payload to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dissolution {
    pub tx: SpecialTx,
    pub payload: ProDisTx,
    /// The actor's `amount − penalty − fee`, 0 when its output is omitted.
    pub actor_output: u64,
}

impl Dissolution {
    /// `MakeSignHash(tx, sig_count)`: 1 unilateral, the share count
    /// unanimous.
    pub fn sign_hash(&self, sig_count: u8) -> [u8; 32] {
        self.payload.sign_hash(
            self.tx.version,
            self.tx.lock_time,
            &self.tx.prevouts(),
            &self.tx.output,
            sig_count,
        )
    }

    /// The transaction carrying `sigs` (`SetTxPayload`).
    pub fn signed(&self, sigs: Vec<CompactSignature>) -> SpecialTx {
        let payload = ProDisTx {
            sigs,
            ..self.payload.clone()
        };
        SpecialTx {
            payload: payload_bytes(&payload),
            ..self.tx.clone()
        }
    }
}

/// `BuildDissolution`'s fee checks, which come before the actor's balance.
pub fn check_fee(fee: u64) -> Result<(), BuildError> {
    match fee {
        0 => Err(BuildError::FeeNotPositive),
        f if f > PRODIS_MAX_FEE => Err(BuildError::FeeAboveCeiling),
        _ => Ok(()),
    }
}

/// `BuildDissolution(dmn, actor_index, penalty, fee)`: the collateral as
/// the one input (empty scriptSig, final sequence), lock time 0; each
/// non-actor share in order `amount + bonus` to its refund script, the
/// bonuses `SplitAmountByShares(penalty, non-actors)`; then the actor's
/// `amount − penalty − fee` to its refund script, omitted when 0.
pub fn build_dissolution(
    mn: &SharedMn,
    actor_index: u16,
    penalty: u64,
    fee: u64,
) -> Result<Dissolution, BuildError> {
    let shares = mn.shares;
    let actor = shares
        .get(usize::from(actor_index))
        .ok_or(BuildError::ActorOutOfRange)?;
    check_fee(fee)?;
    let actor_output = actor
        .amount
        .checked_sub(penalty)
        .and_then(|left| left.checked_sub(fee))
        .ok_or(BuildError::ActorOverdrawn)?;
    let non_actors: Vec<&CollateralShare> = shares
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != usize::from(actor_index))
        .map(|(_, s)| s)
        .collect();
    let bonuses = split_amount_by_shares(penalty, &non_actors);
    let pay = |value: u64, script: &ScriptBuf| TxOut {
        value,
        script_pubkey: script.clone(),
    };
    let mut output: Vec<TxOut> = non_actors
        .iter()
        .zip(&bonuses)
        .map(|(s, bonus)| pay(s.amount + bonus, &s.refund_script))
        .collect();
    if actor_output > 0 {
        output.push(pay(actor_output, &actor.refund_script));
    }
    let payload = ProDisTx {
        version: ProDisTx::CURRENT_VERSION,
        pro_tx_hash: mn.pro_tx_hash,
        actor_index,
        sigs: Vec::new(),
    };
    let tx = SpecialTx {
        version: SPECIAL_VERSION,
        tx_type: TRANSACTION_PROVIDER_DISSOLVE,
        input: vec![TxIn {
            previous_output: mn.collateral,
            script_sig: ScriptBuf::new(),
            sequence: u32::MAX,
            witness: Default::default(),
        }],
        output,
        lock_time: 0,
        payload: payload_bytes(&payload),
    };
    Ok(Dissolution {
        tx,
        payload,
        actor_output,
    })
}

/// `DissolveShared`'s test (`providertx_service.cpp:1249-1251`): a
/// dissolution mined at `spend_height` is early while fewer than
/// `nEarlyPeriodBlocks` blocks separate it from the registration.
pub fn is_early(mn: &SharedMn, spend_height: u32) -> bool {
    i64::from(spend_height) - i64::from(mn.registered_height) < i64::from(mn.early_period_blocks)
}

/// The first height at which a unilateral dissolution pays no penalty.
pub fn penalty_free_from(mn: &SharedMn) -> u32 {
    mn.registered_height.saturating_add(mn.early_period_blocks)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::{OutPoint, Txid};

    use super::super::rules::check_dissolution;
    use super::*;

    const COIN: u64 = 100_000_000;

    fn share(amount: u64, refund: u8) -> CollateralShare {
        let mut script = vec![0x76, 0xa9, 0x14];
        script.extend_from_slice(&[refund; 20]);
        script.extend_from_slice(&[0x88, 0xac]);
        CollateralShare {
            amount,
            refund_script: ScriptBuf::from_bytes(script),
            reward_script: ScriptBuf::new(),
            owner_key_id: [refund; 20],
        }
    }

    fn mn(shares: &[CollateralShare]) -> SharedMn<'_> {
        let hash = Txid::from_byte_array([7; 32]);
        SharedMn {
            pro_tx_hash: hash,
            collateral: OutPoint::new(hash, 2),
            shares,
            registered_height: 100,
            early_period_blocks: 50,
            early_penalty: (7 * COIN) as i64,
            voting_key_id: [9; 20],
        }
    }

    #[test]
    fn test_QT_127_split_gives_the_remainder_to_the_last_share() {
        let shares = [
            share(300 * COIN, 1),
            share(300 * COIN, 2),
            share(400 * COIN, 3),
        ];
        let refs: Vec<_> = shares.iter().collect();
        // 10 split 3:3:4 is 3, 3, then the remainder 4; 1 duff is 0, 0, 1.
        assert_eq!(split_amount_by_shares(10, &refs), [3, 3, 4]);
        assert_eq!(split_amount_by_shares(1, &refs), [0, 0, 1]);
        assert_eq!(split_amount_by_shares(0, &refs), [0, 0, 0]);
        let total = 7 * COIN + 1;
        assert_eq!(
            split_amount_by_shares(total, &refs).iter().sum::<u64>(),
            total
        );
    }

    #[test]
    fn test_QT_127_build_dissolution_is_cores_and_passes_its_rules() {
        let shares = [
            share(400 * COIN, 1),
            share(300 * COIN, 2),
            share(300 * COIN, 3),
        ];
        let m = mn(&shares);
        let penalty = 7 * COIN;
        let d = build_dissolution(&m, 1, penalty, 1_000).unwrap();
        assert_eq!((d.tx.version, d.tx.tx_type, d.tx.lock_time), (3, 10, 0));
        assert_eq!(d.tx.input.len(), 1);
        assert_eq!(
            (d.tx.input[0].previous_output, d.tx.input[0].sequence),
            (m.collateral, u32::MAX)
        );
        assert!(d.tx.input[0].script_sig.is_empty());
        // Non-actors 0 and 2 in order, the penalty 4:3, the actor last.
        let bonus0 = pro_rata_floor(penalty, 400 * COIN, 700 * COIN);
        let got: Vec<_> =
            d.tx.output
                .iter()
                .map(|o| (o.value, o.script_pubkey.clone()))
                .collect();
        assert_eq!(
            got,
            [
                (400 * COIN + bonus0, shares[0].refund_script.clone()),
                (
                    300 * COIN + penalty - bonus0,
                    shares[2].refund_script.clone()
                ),
                (
                    300 * COIN - penalty - 1_000,
                    shares[1].refund_script.clone()
                ),
            ]
        );
        let p = ProDisTx {
            sigs: vec![[0; 65]],
            ..d.payload.clone()
        };
        // Early (spent at 120 < 150): the penalty is due and is paid.
        check_dissolution(&d.tx, &p, Some(&m), 120, true, false).unwrap();
        assert!(is_early(&m, 149) && !is_early(&m, 150));
        assert_eq!(penalty_free_from(&m), 150);

        // Without the penalty it is refused early, valid from 150 on.
        let free = build_dissolution(&m, 1, 0, 1_000).unwrap();
        let p = ProDisTx {
            sigs: vec![[0; 65]],
            ..free.payload.clone()
        };
        assert_eq!(
            check_dissolution(&free.tx, &p, Some(&m), 149, true, false),
            Err("bad-prodis-penalty-floor")
        );
        check_dissolution(&free.tx, &p, Some(&m), 150, true, false).unwrap();
    }

    #[test]
    fn test_QT_127_build_dissolution_refuses_as_core() {
        let shares = [share(100 * COIN, 1), share(900 * COIN, 2)];
        let m = mn(&shares);
        assert_eq!(
            build_dissolution(&m, 2, 0, 1_000),
            Err(BuildError::ActorOutOfRange)
        );
        assert_eq!(
            build_dissolution(&m, 0, 0, 0),
            Err(BuildError::FeeNotPositive)
        );
        assert_eq!(
            build_dissolution(&m, 0, 0, PRODIS_MAX_FEE + 1),
            Err(BuildError::FeeAboveCeiling)
        );
        assert!(build_dissolution(&m, 0, 0, PRODIS_MAX_FEE).is_ok());
        assert_eq!(
            build_dissolution(&m, 0, 100 * COIN - 999, 1_000),
            Err(BuildError::ActorOverdrawn)
        );
        // Exactly the actor's share: its output is omitted.
        let d = build_dissolution(&m, 0, 100 * COIN - 1_000, 1_000).unwrap();
        assert_eq!(d.tx.output.len(), 1);
        assert_eq!(d.tx.output[0].value, 1_000 * COIN - 1_000);
        assert_eq!(d.actor_output, 0);
    }

    #[test]
    fn test_QT_127_the_signed_copy_carries_the_signatures_only() {
        let shares = [share(500 * COIN, 1), share(500 * COIN, 2)];
        let m = mn(&shares);
        let d = build_dissolution(&m, 0, 0, 1_000).unwrap();
        let hash = d.sign_hash(1);
        let signed = d.signed(vec![[5; 65]]);
        assert_eq!((&signed.input, &signed.output), (&d.tx.input, &d.tx.output));
        let p: ProDisTx = dashcore::consensus::deserialize(&signed.payload).unwrap();
        assert_eq!(p.sigs, vec![[5u8; 65]]);
        // The digest commits to the outputs and the count, not to the sigs.
        assert_eq!(
            p.sign_hash(
                signed.version,
                signed.lock_time,
                &signed.prevouts(),
                &signed.output,
                1
            ),
            hash
        );
        assert_ne!(d.sign_hash(2), hash);
    }
}
