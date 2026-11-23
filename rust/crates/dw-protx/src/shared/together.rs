//! Dissolve together (MG-04b §3.3, §4, part 2): the unanimous ProDisTx
//! every owner signs. `protx shared_dissolve_prepare` builds it
//! (`PrepareSharedDissolution`, rc.3 `providertx_service.cpp:1281-1296`),
//! each owner signs `MakeSignHash(tx, n)` over it, and any party combines
//! the signatures in share order (`InsertUnanimousSignatures`, `:705-729`).
//! dash-qt's reader (`ComputeSharedSignHash`, `sharedmndialogs.cpp:253-291`,
//! and `SharedMnReadDissolution`, `sharedmnwidgets.cpp:272-352`) takes a
//! request only when it returns every share its principal; the wallet takes
//! one only when it is the builder's output byte for byte (MG-04b D92).

use std::collections::BTreeMap;

use super::dissolve::{BuildError, Dissolution, build_dissolution};
use super::rawtx::SpecialTx;
use super::rules::{SharedMn, verify_hash_canonical};
use super::{CompactSignature, ProDisTx, TRANSACTION_PROVIDER_DISSOLVE};

/// `PrepareSharedDissolution`: `BuildDissolution(dmn, actor, 0, fee)`, the
/// payload without signatures.
pub fn prepare(mn: &SharedMn, actor: u16, fee: u64) -> Result<Dissolution, BuildError> {
    build_dissolution(mn, actor, 0, fee)
}

/// The signature count a unanimous dissolution commits to: the share count.
pub fn sig_count(mn: &SharedMn) -> u8 {
    u8::try_from(mn.shares.len()).unwrap_or(u8::MAX)
}

/// Why a received request isn't one every owner may sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a type-10 transaction whose payload decodes ("The transaction is
    /// not a dissolution transaction.", "… payload is not deserializable.").
    NotDissolution,
    /// It dissolves another masternode (`:276-280`).
    OtherMasternode,
    /// It carries signatures: not the prepared transaction (`:281-285`).
    Signed,
    /// It isn't `BuildDissolution`'s output at penalty 0 for its actor and
    /// the fee its outputs leave: it doesn't return every share its
    /// principal (dash-qt's one wording), or it differs from the builder
    /// in a field dash-qt doesn't read (MG-04b D92).
    NotBuilt,
}

/// A request that passes [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub dissolution: Dissolution,
    pub actor: u16,
    pub fee: u64,
    /// What every owner signs: `MakeSignHash(tx, n)`.
    pub sign_hash: [u8; 32],
}

/// The checks before any owner signs `tx` or a signature over it is
/// taken: a ProDisTx of `mn` without signatures, equal to
/// `BuildDissolution(mn, actor, 0, fee)` for its actor and the fee its
/// outputs leave. Equality covers `SharedMnReadDissolution`'s rules (one
/// input, every non-actor its principal at its refund script in share
/// order, at most the actor's refund after them, a fee in (0, 1,000,000])
/// and the collateral input, its empty scriptSig, the final sequence and
/// lock time 0 (D16).
pub fn check(tx: &SpecialTx, mn: &SharedMn) -> Result<Checked, Refusal> {
    if tx.tx_type != TRANSACTION_PROVIDER_DISSOLVE {
        return Err(Refusal::NotDissolution);
    }
    let p: ProDisTx =
        dashcore::consensus::deserialize(&tx.payload).map_err(|_| Refusal::NotDissolution)?;
    if p.pro_tx_hash != mn.pro_tx_hash {
        return Err(Refusal::OtherMasternode);
    }
    if !p.sigs.is_empty() {
        return Err(Refusal::Signed);
    }
    // Received outputs may sum past `u64`: no builder writes those.
    let total = checked_sum(mn.shares.iter().map(|s| s.amount));
    let paid = checked_sum(tx.output.iter().map(|o| o.value));
    let fee = total
        .zip(paid)
        .and_then(|(total, paid)| total.checked_sub(paid))
        .ok_or(Refusal::NotBuilt)?;
    let dissolution = prepare(mn, p.actor_index, fee).map_err(|_| Refusal::NotBuilt)?;
    if dissolution.tx != *tx {
        return Err(Refusal::NotBuilt);
    }
    let sign_hash = dissolution.sign_hash(sig_count(mn));
    Ok(Checked {
        actor: p.actor_index,
        fee,
        sign_hash,
        dissolution,
    })
}

/// Whether `sig` is share `index`'s owner signature over `sign_hash`
/// (`VerifyHashCanonical`, `sharedmndialogs.cpp:604-609`).
pub fn verifies(mn: &SharedMn, sign_hash: [u8; 32], index: u16, sig: &CompactSignature) -> bool {
    mn.shares
        .get(usize::from(index))
        .is_some_and(|s| verify_hash_canonical(sign_hash, s.owner_key_id, sig))
}

/// Why [`combine`] refuses, with Core's texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CombineError {
    #[error(
        "a dissolution combined through shared_combine requires a signature from every share; a \
         unilateral dissolution is created fully signed by \"protx shared_dissolve\""
    )]
    Count,
    #[error("missing signature for share {0}")]
    Missing(u16),
}

/// `InsertUnanimousSignatures`: one signature per share, in share order,
/// into the prepared transaction.
pub fn combine(
    checked: &Checked,
    mn: &SharedMn,
    sigs: &BTreeMap<u16, CompactSignature>,
) -> Result<SpecialTx, CombineError> {
    if sigs.len() != mn.shares.len() {
        return Err(CombineError::Count);
    }
    let ordered = (0..mn.shares.len())
        .map(|i| {
            let i = u16::try_from(i).unwrap_or(u16::MAX);
            sigs.get(&i).copied().ok_or(CombineError::Missing(i))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(checked.dissolution.signed(ordered))
}

fn checked_sum(mut values: impl Iterator<Item = u64>) -> Option<u64> {
    values.try_fold(0, u64::checked_add)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::secp256k1::SecretKey;
    use dashcore::{OutPoint, ScriptBuf, Txid};

    use super::super::rules::check_dissolution;
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

    /// Every share's signature: share `i`'s key is `[i + 1; 32]`.
    fn sign_all(c: &Checked, n: u8) -> BTreeMap<u16, CompactSignature> {
        (0..n)
            .map(|i| {
                let sig = sign_hash_compact(&[i + 1; 32], &c.sign_hash).unwrap();
                (u16::from(i), sig)
            })
            .collect()
    }

    #[test]
    fn test_QT_127_a_prepared_request_checks_and_combines_in_share_order() {
        let s = shares();
        let m = mn(&s);
        let d = prepare(&m, 2, 50_000).unwrap();
        assert_eq!(d, build_dissolution(&m, 2, 0, 50_000).unwrap());
        let c = check(&d.tx, &m).unwrap();
        assert_eq!((c.actor, c.fee), (2, 50_000));
        assert_eq!(c.sign_hash, d.sign_hash(3));
        assert_ne!(c.sign_hash, d.sign_hash(1));
        let sigs = sign_all(&c, 3);
        for (i, sig) in &sigs {
            assert!(verifies(&m, c.sign_hash, *i, sig));
            assert!(!verifies(&m, c.sign_hash, (*i + 1) % 3, sig));
        }
        assert!(!verifies(&m, c.sign_hash, 3, &sigs[&0]));
        let tx = combine(&c, &m, &sigs).unwrap();
        let p: ProDisTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
        assert_eq!(p.sigs, sigs.values().copied().collect::<Vec<_>>());
        // Unanimous: valid in the early period at penalty 0, signatures
        // checked as Core checks them.
        check_dissolution(&tx, &p, Some(&m), 101, true, true).unwrap();
        let mut swapped = p.clone();
        swapped.sigs.swap(0, 1);
        let bad = SpecialTx {
            payload: payload_bytes(&swapped),
            ..tx.clone()
        };
        assert_eq!(
            check_dissolution(&bad, &swapped, Some(&m), 101, true, true),
            Err("bad-prodis-sig")
        );
    }

    #[test]
    fn test_QT_127_combine_needs_every_share_as_core() {
        let s = shares();
        let m = mn(&s);
        let c = check(&prepare(&m, 0, 1_000).unwrap().tx, &m).unwrap();
        let mut sigs = sign_all(&c, 3);
        sigs.remove(&1);
        assert_eq!(combine(&c, &m, &sigs), Err(CombineError::Count));
        sigs.insert(5, [1; 65]);
        assert_eq!(combine(&c, &m, &sigs), Err(CombineError::Missing(1)));
    }

    #[test]
    fn test_QT_127_a_request_is_refused_unless_it_is_the_builders() {
        let s = shares();
        let m = mn(&s);
        let d = prepare(&m, 1, 1_000).unwrap();
        // Another type, an undecodable payload, another masternode, signed.
        let other = SpecialTx {
            tx_type: 11,
            ..d.tx.clone()
        };
        assert_eq!(check(&other, &m), Err(Refusal::NotDissolution));
        let junk = SpecialTx {
            payload: vec![1, 2],
            ..d.tx.clone()
        };
        assert_eq!(check(&junk, &m), Err(Refusal::NotDissolution));
        let elsewhere = SharedMn {
            pro_tx_hash: Txid::from_byte_array([8; 32]),
            ..m.clone()
        };
        assert_eq!(check(&d.tx, &elsewhere), Err(Refusal::OtherMasternode));
        assert_eq!(check(&d.signed(vec![[1; 65]]), &m), Err(Refusal::Signed));
        // A non-actor underpaid, a penalty taken, an output to another
        // script, a fee above Core's ceiling, a fee of 0.
        let mut under = d.tx.clone();
        under.output[0].value -= 1;
        assert_eq!(check(&under, &m), Err(Refusal::NotBuilt));
        let penalty = build_dissolution(&m, 1, 5, 1_000).unwrap();
        assert_eq!(check(&penalty.tx, &m), Err(Refusal::NotBuilt));
        let mut elsewhere = d.tx.clone();
        elsewhere.output[2].script_pubkey = ScriptBuf::new();
        assert_eq!(check(&elsewhere, &m), Err(Refusal::NotBuilt));
        let mut greedy = d.tx.clone();
        greedy.output[2].value -= 1_000_000;
        assert_eq!(check(&greedy, &m), Err(Refusal::NotBuilt));
        let mut free = d.tx.clone();
        free.output[2].value += 1_000;
        assert_eq!(check(&free, &m), Err(Refusal::NotBuilt));
        // Fields dash-qt doesn't read: a lock time, a sequence, a version,
        // a scriptSig, another input.
        let edits: [fn(&mut SpecialTx); 5] = [
            |t: &mut SpecialTx| t.lock_time = 1,
            |t: &mut SpecialTx| t.input[0].sequence = 0,
            |t: &mut SpecialTx| t.version = 2,
            |t: &mut SpecialTx| t.input[0].script_sig = ScriptBuf::from_bytes(vec![0x51]),
            |t: &mut SpecialTx| t.input[0].previous_output.vout = 3,
        ];
        for edit in edits {
            let mut tx = d.tx.clone();
            edit(&mut tx);
            assert_eq!(check(&tx, &m), Err(Refusal::NotBuilt));
        }
        // Outputs summing past `u64`.
        let mut huge = d.tx.clone();
        huge.output[2].value = u64::MAX;
        assert_eq!(check(&huge, &m), Err(Refusal::NotBuilt));
        // The actor's whole share as fee is valid if within the ceiling:
        // the actor's output is then omitted, as the builder omits it.
        let tiny = [share(400 * COIN, 1), share(500_000, 2)];
        let small = mn(&tiny);
        let all = prepare(&small, 1, 500_000).unwrap();
        assert_eq!(all.tx.output.len(), 1);
        assert_eq!(check(&all.tx, &small).unwrap().fee, 500_000);
    }
}
