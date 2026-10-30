//! The standby (MG-04b §9.4, part 2): dash-qt's pair of unilateral
//! dissolutions kept for later (`DissolveDialog::createStandby`, rc.3
//! `qt/sharedmndialogs.cpp:1634-1668`), and D32's check of a pasted one.

use super::dissolve::{BuildError, Dissolution, build_dissolution, is_early, penalty_free_from};
use super::rawtx::SpecialTx;
use super::rules::{SharedMn, check_dissolution, verify_hash_canonical};
use super::{ProDisTx, TRANSACTION_PROVIDER_DISSOLVE};
use crate::rules::Reject;

/// A standby's two variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Penalty 0: valid once the early period is over.
    Full,
    /// `nEarlyPenalty` paid: valid now.
    Immediate,
}

/// `createStandby`'s pair by `actor` paying `fee`: `generateStandby` with
/// `pay_penalty` false, then true, unconditionally (`:1645-1647`), so the
/// immediate variant pays `nEarlyPenalty` whatever the height
/// (`providertx_service.cpp:1252-1262`). With a zero penalty the two are
/// the same transaction and `immediate` is `None` (m-20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub full: Dissolution,
    pub immediate: Option<Dissolution>,
}

/// The penalty the immediate variant pays.
pub fn early_penalty(mn: &SharedMn) -> u64 {
    mn.early_penalty.max(0) as u64
}

/// What `variant` pays as penalty.
pub fn variant_penalty(mn: &SharedMn, variant: Variant) -> u64 {
    match variant {
        Variant::Full => 0,
        Variant::Immediate => early_penalty(mn),
    }
}

/// Both of `actor`'s variants paying `fee`, built as Core builds them; a
/// refusal of either refuses the pair, as `createStandby` stores nothing
/// then.
pub fn pair(mn: &SharedMn, actor: u16, fee: u64) -> Result<Pair, BuildError> {
    let full = build_dissolution(mn, actor, 0, fee)?;
    let immediate = match early_penalty(mn) {
        0 => None,
        penalty => Some(build_dissolution(mn, actor, penalty, fee)?),
    };
    Ok(Pair { full, immediate })
}

/// Whether the full-principal variant is valid mined at `spend_height`:
/// the early period is over, or there is no penalty to pay
/// (`specialtxman.cpp:1704-1708`).
pub fn full_valid(mn: &SharedMn, spend_height: u32) -> bool {
    !is_early(mn, spend_height) || early_penalty(mn) == 0
}

/// Why a pasted transaction isn't a standby (D32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a ProDisTx with one signature: not unilateral, or not decodable.
    NotStandby,
    /// Core's rule refuses it, its signature's included (`bad-prodis-*`).
    Rule(Reject),
    /// Valid, but not what `BuildDissolution` builds for its actor and fee
    /// with either penalty (m-19).
    NotBuilt,
}

/// A pasted standby that passes D32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub variant: Variant,
    pub actor: u16,
    pub fee: u64,
    pub actor_output: u64,
}

/// D32 on `tx`: a type-10 transaction with one signature; Core's rules
/// (§5.1) mined at `spend_height` or, when that is in the early period, at
/// its end, the first height every standby is valid at (a ProDisTx valid
/// once stays valid, §5); its signature by the actor share's owner key; and
/// the transaction `BuildDissolution` builds for its actor and fee with the
/// penalty 0 or `nEarlyPenalty`, signed (not `SharedMnReadDissolution`,
/// which takes an overpaid immediate variant, `masternodelist.cpp:346-353`).
pub fn check(tx: &SpecialTx, mn: &SharedMn, spend_height: u32) -> Result<Checked, Refusal> {
    if tx.tx_type != TRANSACTION_PROVIDER_DISSOLVE {
        return Err(Refusal::NotStandby);
    }
    let p: ProDisTx =
        dashcore::consensus::deserialize(&tx.payload).map_err(|_| Refusal::NotStandby)?;
    if p.sigs.len() != 1 {
        return Err(Refusal::NotStandby);
    }
    let height = spend_height.max(penalty_free_from(mn));
    check_dissolution(tx, &p, Some(mn), height, true, false).map_err(Refusal::Rule)?;
    // The rules checked the actor index against the share table.
    let actor = &mn.shares[usize::from(p.actor_index)];
    let unsigned = ProDisTx {
        sigs: Vec::new(),
        ..p.clone()
    };
    let hash = unsigned.sign_hash(tx.version, tx.lock_time, &tx.prevouts(), &tx.output, 1);
    if !verify_hash_canonical(hash, actor.owner_key_id, &p.sigs[0]) {
        return Err(Refusal::Rule("bad-prodis-sig"));
    }
    // Pasted outputs may sum past `u64`: no builder writes those.
    let total = checked_sum(mn.shares.iter().map(|s| s.amount));
    let paid = checked_sum(tx.output.iter().map(|o| o.value));
    let fee = total
        .zip(paid)
        .and_then(|(total, paid)| total.checked_sub(paid))
        .ok_or(Refusal::NotBuilt)?;
    for variant in [Variant::Full, Variant::Immediate] {
        let penalty = variant_penalty(mn, variant);
        let Ok(built) = build_dissolution(mn, p.actor_index, penalty, fee) else {
            continue;
        };
        if built.signed(p.sigs.clone()) == *tx {
            return Ok(Checked {
                variant,
                actor: p.actor_index,
                fee,
                actor_output: built.actor_output,
            });
        }
    }
    Err(Refusal::NotBuilt)
}

fn checked_sum(values: impl Iterator<Item = u64>) -> Option<u64> {
    values.into_iter().try_fold(0, u64::checked_add)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::secp256k1::SecretKey;
    use dashcore::{OutPoint, ScriptBuf, TxOut, Txid};

    use super::super::{CollateralShare, payload_bytes};
    use super::*;
    use crate::payloads::sign_hash_compact;

    const COIN: u64 = 100_000_000;

    fn key_id(i: u8) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes([i; 32]).unwrap();
        dashcore::PublicKey::new(sk.public_key())
            .pubkey_hash()
            .to_byte_array()
    }

    fn share(amount: u64, i: u8) -> CollateralShare {
        let mut script = vec![0x76, 0xa9, 0x14];
        script.extend_from_slice(&[i; 20]);
        script.extend_from_slice(&[0x88, 0xac]);
        CollateralShare {
            amount,
            refund_script: ScriptBuf::from_bytes(script),
            reward_script: ScriptBuf::new(),
            owner_key_id: key_id(i),
        }
    }

    fn shares() -> [CollateralShare; 3] {
        [
            share(400 * COIN, 1),
            share(300 * COIN, 2),
            share(300 * COIN, 3),
        ]
    }

    fn mn(shares: &[CollateralShare], penalty: i64) -> SharedMn<'_> {
        let hash = Txid::from_byte_array([7; 32]);
        SharedMn {
            pro_tx_hash: hash,
            collateral: OutPoint::new(hash, 2),
            shares,
            registered_height: 100,
            early_period_blocks: 50,
            early_penalty: penalty,
            voting_key_id: [9; 20],
        }
    }

    /// `d` signed by share `i`'s key over `MakeSignHash(tx, 1)`.
    fn sign(d: &Dissolution, i: u8) -> SpecialTx {
        d.signed(vec![sign_hash_compact(&[i; 32], &d.sign_hash(1)).unwrap()])
    }

    #[test]
    fn test_QT_127_the_pair_is_full_and_immediate_or_one_copy_at_penalty_0() {
        let s = shares();
        let m = mn(&s, (7 * COIN) as i64);
        let p = pair(&m, 1, 1_000).unwrap();
        assert_eq!(p.full, build_dissolution(&m, 1, 0, 1_000).unwrap());
        assert_eq!(
            p.immediate,
            Some(build_dissolution(&m, 1, 7 * COIN, 1_000).unwrap())
        );
        let free = mn(&s, 0);
        let one = pair(&free, 1, 1_000).unwrap();
        assert_eq!(
            (one.full, one.immediate),
            (build_dissolution(&free, 1, 0, 1_000).unwrap(), None)
        );
        // The immediate one overdraws the actor: no pair.
        let big = mn(&s, (300 * COIN) as i64);
        assert_eq!(pair(&big, 1, 1_000), Err(BuildError::ActorOverdrawn));
        assert_eq!(pair(&m, 1, 0), Err(BuildError::FeeNotPositive));
    }

    #[test]
    fn test_QT_127_the_full_variant_is_valid_from_the_early_periods_end_or_at_penalty_0() {
        let s = shares();
        let m = mn(&s, (7 * COIN) as i64);
        assert!(!full_valid(&m, 149) && full_valid(&m, 150));
        assert!(full_valid(&mn(&s, 0), 101));
        assert_eq!(variant_penalty(&m, Variant::Full), 0);
        assert_eq!(variant_penalty(&m, Variant::Immediate), 7 * COIN);
        assert_eq!(early_penalty(&mn(&s, -5)), 0);
    }

    #[test]
    fn test_QT_127_a_standby_is_checked_as_d32_lists() {
        let s = shares();
        let m = mn(&s, (7 * COIN) as i64);
        let p = pair(&m, 1, 1_000).unwrap();
        let full = sign(&p.full, 2);
        let immediate = sign(p.immediate.as_ref().unwrap(), 2);
        // Both pass early: the full one is checked at the period's end.
        let got = check(&full, &m, 120).unwrap();
        assert_eq!(
            got,
            Checked {
                variant: Variant::Full,
                actor: 1,
                fee: 1_000,
                actor_output: 300 * COIN - 1_000,
            }
        );
        let got = check(&immediate, &m, 120).unwrap();
        assert_eq!(
            (got.variant, got.fee, got.actor_output),
            (Variant::Immediate, 1_000, 300 * COIN - 7 * COIN - 1_000)
        );
        // At penalty 0 the one copy is the full one.
        let free = mn(&s, 0);
        let one = sign(&pair(&free, 1, 1_000).unwrap().full, 2);
        assert_eq!(check(&one, &free, 120).unwrap().variant, Variant::Full);

        // Another type, two signatures, an undecodable payload.
        let other = SpecialTx {
            tx_type: 11,
            ..full.clone()
        };
        assert_eq!(check(&other, &m, 120), Err(Refusal::NotStandby));
        let mut two = full.clone();
        let mut payload: ProDisTx = dashcore::consensus::deserialize(&two.payload).unwrap();
        payload.sigs.push(payload.sigs[0]);
        two.payload = payload_bytes(&payload);
        assert_eq!(check(&two, &m, 120), Err(Refusal::NotStandby));
        let junk = SpecialTx {
            payload: vec![1, 2],
            ..full.clone()
        };
        assert_eq!(check(&junk, &m, 120), Err(Refusal::NotStandby));

        // Core's rules: another input, another masternode.
        let mut input = p.full.clone();
        input.tx.input[0].previous_output.vout = 3;
        assert_eq!(
            check(&sign(&input, 2), &m, 120),
            Err(Refusal::Rule("bad-prodis-input"))
        );
        let elsewhere = SharedMn {
            pro_tx_hash: Txid::from_byte_array([8; 32]),
            ..m.clone()
        };
        assert_eq!(
            check(&full, &elsewhere, 120),
            Err(Refusal::Rule("bad-prodis-hash"))
        );
        // Signed by another share's key.
        assert_eq!(
            check(&sign(&p.full, 1), &m, 120),
            Err(Refusal::Rule("bad-prodis-sig"))
        );
        // Valid but not built so: half the penalty, which Core's floor takes
        // once the period is over; a lock time; an extra output paying a
        // non-actor more.
        let half = sign(&build_dissolution(&m, 1, 7 * COIN / 2, 1_000).unwrap(), 2);
        assert_eq!(check(&half, &m, 120), Err(Refusal::NotBuilt));
        let mut locked = p.full.clone();
        locked.tx.lock_time = 5;
        assert_eq!(check(&sign(&locked, 2), &m, 120), Err(Refusal::NotBuilt));
        let mut version = p.full.clone();
        version.tx.version = 4;
        assert_eq!(check(&sign(&version, 2), &m, 120), Err(Refusal::NotBuilt));
        let mut more = p.full.clone();
        more.tx.output[0].value += 500;
        more.tx.output[2].value -= 500;
        assert_eq!(check(&sign(&more, 2), &m, 120), Err(Refusal::NotBuilt));
        // Paying more than the collateral.
        let mut over = p.full.clone();
        over.tx.output.push(TxOut {
            value: 1,
            script_pubkey: ScriptBuf::new(),
        });
        assert!(check(&sign(&over, 2), &m, 120).is_err());
        // Outputs summing past `u64`, signed by the actor (review n-5).
        let mut huge = p.full.clone();
        huge.tx.output[2].value = u64::MAX;
        assert!(check(&sign(&huge, 2), &m, 120).is_err());
    }
}
