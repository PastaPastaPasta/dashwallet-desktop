//! The shared registrar key change (MG-04b §3.3, slice 2d): the
//! ProUpSharedRegTx every owner signs. `protx
//! shared_update_registrar_prepare` builds it (`PrepareSharedRegistrarUpdate`,
//! rc.3 `providertx_service.cpp:1336-1371`): the payload with the keys set or
//! kept, one empty signature per share for the fee's size, the funding
//! wallet's fee inputs and `inputsHash`. Each owner signs
//! `SerializeHash(payload)` (`:1150-1158`), which leaves the signatures out;
//! the funding wallet inserts them in share order and signs its fee inputs
//! again (`CombineShared`, `:1224-1236`), since the payload they were signed
//! over changed. The fee inputs' signature hash covers the payload whatever
//! the hash type (`script/interpreter.cpp:1437-1438`), so no signature made
//! before the insert spends them after it: only the funder can finish.

use std::collections::BTreeMap;

use dashcore::{OutPoint, ScriptBuf, TxIn, TxOut};

use super::rawtx::{SPECIAL_VERSION, SpecialTx};
use super::rules::{self, SharedMn, verify_hash_canonical};
use super::{
    CompactSignature, ProUpSharedRegTx, TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR, payload_bytes,
};
use crate::rules::Reject;

/// The sequence the wallet writes on its own inputs (MG-04b D16).
pub const SEQUENCE: u32 = 0xffff_ffff;

/// The keys a key change writes: absolute, a kept one copied from the list
/// (`:1345-1353`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keys {
    /// Basic serialization (`Set(key, false)`, `:1348`).
    pub operator_public_key: [u8; 48],
    pub voting_key_id: [u8; 20],
}

/// The payload `PrepareSharedRegistrarUpdate` writes before funding: one
/// empty signature per share (`vchSigs.assign(n, CompactSignature{})`,
/// `:1358`), so the fee is sized for the signed transaction.
pub fn placeholder(mn: &SharedMn, keys: Keys) -> ProUpSharedRegTx {
    ProUpSharedRegTx {
        version: ProUpSharedRegTx::CURRENT_VERSION,
        pro_tx_hash: mn.pro_tx_hash,
        operator_public_key: keys.operator_public_key,
        voting_key_id: keys.voting_key_id,
        inputs_hash: [0; 32],
        sigs: vec![[0; 65]; mn.shares.len()],
    }
}

/// The prepared request: the placeholder payload on `inputs`, its change
/// output if any, `inputsHash` over the inputs (`UpdateInputsHash`), the
/// inputs unsigned, lock time 0 and final sequences (D16).
pub fn prepare(mn: &SharedMn, keys: Keys, inputs: &[OutPoint], change: Option<TxOut>) -> SpecialTx {
    let mut p = placeholder(mn, keys);
    let mut tx = SpecialTx {
        version: SPECIAL_VERSION,
        tx_type: TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR,
        input: inputs
            .iter()
            .map(|o| TxIn {
                previous_output: *o,
                script_sig: ScriptBuf::new(),
                sequence: SEQUENCE,
                witness: Default::default(),
            })
            .collect(),
        output: change.into_iter().collect(),
        lock_time: 0,
        payload: Vec::new(),
    };
    p.inputs_hash = tx.inputs_hash();
    tx.payload = payload_bytes(&p);
    tx
}

/// Why a received request isn't one an owner may sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a version-3, type-12 transaction whose payload decodes.
    NotKeyChange,
    /// It updates another masternode (`ComputeSharedSignHash`,
    /// `sharedmndialogs.cpp:292-312`).
    OtherMasternode,
    /// Core's trivial rules (`CheckProUpSharedRegTx`'s first checks), or a
    /// signature count other than the share count, which no combine can
    /// fill (`InsertUnanimousSignatures`).
    Rules(Reject),
    /// `inputsHash` isn't the transaction's inputs: the owners' signatures
    /// wouldn't bind the fee inputs that make the request single-use
    /// (`providertx.h:527`).
    InputsHash,
}

/// A request that passes [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The payload as received; its signatures (placeholders or filled)
    /// aren't read.
    pub payload: ProUpSharedRegTx,
    pub keys: Keys,
    /// What every owner signs: `SerializeHash(payload)`.
    pub sign_hash: [u8; 32],
}

/// The checks before any owner signs `tx` or a signature over it is taken:
/// a version-3 ProUpSharedRegTx of `mn` (dash-qt's reader checks the type
/// and the ProTx hash only, `sharedmndialogs.cpp:292-312`), Core's trivial
/// rules, one signature slot per share, and `inputsHash` equal to its
/// inputs. The inputs' scriptSigs, sequences and lock time are the
/// funder's: an owner's signature doesn't cover them (MG-04b D101).
pub fn check(tx: &SpecialTx, mn: &SharedMn) -> Result<Checked, Refusal> {
    if tx.version != SPECIAL_VERSION || tx.tx_type != TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR {
        return Err(Refusal::NotKeyChange);
    }
    let payload: ProUpSharedRegTx =
        dashcore::consensus::deserialize(&tx.payload).map_err(|_| Refusal::NotKeyChange)?;
    if payload.pro_tx_hash != mn.pro_tx_hash {
        return Err(Refusal::OtherMasternode);
    }
    rules::check_update_shared_registrar_trivial(&payload).map_err(Refusal::Rules)?;
    if payload.sigs.len() != mn.shares.len() {
        return Err(Refusal::Rules("bad-proupsharedreg-sig-count"));
    }
    if payload.inputs_hash != tx.inputs_hash() {
        return Err(Refusal::InputsHash);
    }
    Ok(Checked {
        keys: Keys {
            operator_public_key: payload.operator_public_key,
            voting_key_id: payload.voting_key_id,
        },
        sign_hash: payload.sign_hash(),
        payload,
    })
}

/// Whether `sig` is share `index`'s owner signature over `sign_hash`.
pub fn verifies(mn: &SharedMn, sign_hash: [u8; 32], index: u16, sig: &CompactSignature) -> bool {
    mn.shares
        .get(usize::from(index))
        .is_some_and(|s| verify_hash_canonical(sign_hash, s.owner_key_id, sig))
}

/// Why [`combine`] refuses, with Core's texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CombineError {
    #[error("a shared registrar update requires a signature from every share")]
    Count,
    #[error("missing signature for share {0}")]
    Missing(u16),
}

/// `InsertUnanimousSignatures`: one signature per share, in share order,
/// into the request. Every scriptSig is emptied: the fee inputs were signed
/// (if at all) over the payload before the insert (`:1231-1232`), so the
/// funder signs them again on the result.
pub fn combine(
    request: &SpecialTx,
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
    let payload = ProUpSharedRegTx {
        sigs: ordered,
        ..checked.payload.clone()
    };
    let mut tx = request.clone();
    for input in &mut tx.input {
        input.script_sig = ScriptBuf::new();
    }
    tx.payload = payload_bytes(&payload);
    Ok(tx)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::secp256k1::{Message, SecretKey, ecdsa};
    use dashcore::{Txid, script::Builder};

    use super::super::CollateralShare;
    use super::super::rules::check_update_shared_registrar;
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

    fn keys() -> Keys {
        Keys {
            operator_public_key: crate::bls::TEST_OPERATOR_KEY,
            voting_key_id: [4; 20],
        }
    }

    fn fee_inputs() -> Vec<OutPoint> {
        vec![
            OutPoint::new(Txid::from_byte_array([1; 32]), 0),
            OutPoint::new(Txid::from_byte_array([2; 32]), 1),
        ]
    }

    fn change() -> TxOut {
        TxOut {
            value: 5 * COIN,
            script_pubkey: share(0, 8).refund_script,
        }
    }

    fn sign_all(c: &Checked) -> BTreeMap<u16, CompactSignature> {
        (0..3u8)
            .map(|i| {
                (
                    u16::from(i),
                    sign_hash_compact(&[i + 1; 32], &c.sign_hash).unwrap(),
                )
            })
            .collect()
    }

    /// `script` spending P2PKH `key`'s coin: `<sig||SIGHASH_ALL> <pubkey>`
    /// over input `index` of `tx`.
    fn script_sig(tx: &SpecialTx, index: usize, key: &SecretKey) -> ScriptBuf {
        let pk = dashcore::PublicKey::new(key.public_key());
        let code = ScriptBuf::new_p2pkh(&pk.pubkey_hash());
        let hash = tx.signature_hash_all(index, &code).unwrap();
        let sig = ecdsa::sign(Message::from_digest(hash), key);
        let mut bytes = sig.serialize_der().to_vec();
        bytes.push(1);
        Builder::new()
            .push_slice(<&dashcore::script::PushBytes>::try_from(bytes.as_slice()).unwrap())
            .push_key(&pk)
            .into_script()
    }

    /// Whether input `index`'s scriptSig (from [`script_sig`]) is a valid
    /// signature of `tx` as it stands.
    fn spends(tx: &SpecialTx, index: usize) -> bool {
        let mut pushes = tx.input[index].script_sig.instructions();
        let (Some(Ok(sig)), Some(Ok(key))) = (pushes.next(), pushes.next()) else {
            return false;
        };
        let (sig, key) = (sig.push_bytes().unwrap(), key.push_bytes().unwrap());
        let pk = dashcore::PublicKey::from_slice(key.as_bytes()).unwrap();
        let code = ScriptBuf::new_p2pkh(&pk.pubkey_hash());
        let hash = tx.signature_hash_all(index, &code).unwrap();
        let der = &sig.as_bytes()[..sig.len() - 1];
        ecdsa::Signature::from_der(der)
            .and_then(|s| ecdsa::verify(&s, Message::from_digest(hash), &pk.inner))
            .is_ok()
    }

    #[test]
    fn test_QT_127_a_prepared_key_change_checks_and_combines_in_share_order() {
        let s = shares();
        let m = mn(&s);
        let tx = prepare(&m, keys(), &fee_inputs(), Some(change()));
        assert_eq!((tx.version, tx.tx_type, tx.lock_time), (3, 12, 0));
        assert!(
            tx.input
                .iter()
                .all(|i| i.sequence == SEQUENCE && i.script_sig.is_empty())
        );
        let c = check(&tx, &m).unwrap();
        assert_eq!(c.keys, keys());
        assert_eq!(c.payload.sigs, vec![[0; 65]; 3]);
        assert_eq!(c.payload.inputs_hash, tx.inputs_hash());
        assert_eq!(c.sign_hash, c.payload.sign_hash());
        let sigs = sign_all(&c);
        for (i, sig) in &sigs {
            assert!(verifies(&m, c.sign_hash, *i, sig));
            assert!(!verifies(&m, c.sign_hash, (*i + 1) % 3, sig));
        }
        let done = combine(&tx, &c, &m, &sigs).unwrap();
        let p: ProUpSharedRegTx = dashcore::consensus::deserialize(&done.payload).unwrap();
        assert_eq!(p.sigs, sigs.values().copied().collect::<Vec<_>>());
        // The signatures change the payload, not what the owners signed.
        assert_eq!(p.sign_hash(), c.sign_hash);
        assert_eq!((done.input.len(), &done.output), (2, &tx.output));
        check_update_shared_registrar(&done, &p, Some(&m), &[], true, true).unwrap();
        let mut swapped = p.clone();
        swapped.sigs.swap(0, 1);
        let bad = SpecialTx {
            payload: payload_bytes(&swapped),
            ..done.clone()
        };
        assert_eq!(
            check_update_shared_registrar(&bad, &swapped, Some(&m), &[], true, true),
            Err("bad-proupsharedreg-sig")
        );
        let mut short = sigs.clone();
        short.remove(&2);
        assert_eq!(combine(&tx, &c, &m, &short), Err(CombineError::Count));
        short.insert(7, [1; 65]);
        assert_eq!(combine(&tx, &c, &m, &short), Err(CombineError::Missing(2)));
    }

    /// R3 (DEC-965): an envelope that leaves the funder before its send
    /// can't be finished by anyone else. A fee input signed over the
    /// request (as Core's prepare may sign it) no longer spends once the
    /// owners' signatures are in: the signature hash covers the payload.
    /// The funder's signature over the combined transaction does.
    #[test]
    fn test_QT_127_a_fee_input_signed_before_the_insert_does_not_spend_after_it() {
        let s = shares();
        let m = mn(&s);
        let funder = SecretKey::from_secret_bytes([42; 32]).unwrap();
        let mut request = prepare(&m, keys(), &fee_inputs(), Some(change()));
        for i in 0..request.input.len() {
            request.input[i].script_sig = script_sig(&request, i, &funder);
        }
        assert!((0..2).all(|i| spends(&request, i)));
        let c = check(&request, &m).unwrap();
        let sigs = sign_all(&c);
        // A non-funder holding every share's signature: the insert alone.
        let mut kept = request.clone();
        let p: ProUpSharedRegTx = dashcore::consensus::deserialize(&request.payload).unwrap();
        kept.payload = payload_bytes(&ProUpSharedRegTx {
            sigs: sigs.values().copied().collect(),
            ..p
        });
        assert!((0..2).all(|i| !spends(&kept, i)));
        // `combine` drops them; the funder signs the result.
        let mut done = combine(&request, &c, &m, &sigs).unwrap();
        assert!(done.input.iter().all(|i| i.script_sig.is_empty()));
        assert_eq!(done.payload, kept.payload);
        for i in 0..done.input.len() {
            done.input[i].script_sig = script_sig(&done, i, &funder);
        }
        assert!((0..2).all(|i| spends(&done, i)));
    }

    #[test]
    fn test_QT_127_a_key_change_request_is_refused_unless_it_is_one_of_this_masternode() {
        let s = shares();
        let m = mn(&s);
        let tx = prepare(&m, keys(), &fee_inputs(), None);
        let typed = |tx_type| SpecialTx {
            tx_type,
            ..tx.clone()
        };
        assert_eq!(check(&typed(10), &m), Err(Refusal::NotKeyChange));
        let v2 = SpecialTx {
            version: 2,
            ..tx.clone()
        };
        assert_eq!(check(&v2, &m), Err(Refusal::NotKeyChange));
        let junk = SpecialTx {
            payload: vec![1, 2],
            ..tx.clone()
        };
        assert_eq!(check(&junk, &m), Err(Refusal::NotKeyChange));
        let elsewhere = SharedMn {
            pro_tx_hash: Txid::from_byte_array([8; 32]),
            ..m.clone()
        };
        assert_eq!(check(&tx, &elsewhere), Err(Refusal::OtherMasternode));
        let with = |edit: &dyn Fn(&mut ProUpSharedRegTx)| {
            let mut p: ProUpSharedRegTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
            edit(&mut p);
            SpecialTx {
                payload: payload_bytes(&p),
                ..tx.clone()
            }
        };
        assert_eq!(
            check(&with(&|p| p.version = 2), &m),
            Err(Refusal::Rules("bad-proupsharedreg-version"))
        );
        assert_eq!(
            check(&with(&|p| p.voting_key_id = [0; 20]), &m),
            Err(Refusal::Rules("bad-protx-key-null"))
        );
        assert_eq!(
            check(&with(&|p| p.sigs.pop().map(drop).unwrap()), &m),
            Err(Refusal::Rules("bad-proupsharedreg-sig-count"))
        );
        assert_eq!(
            check(&with(&|p| p.inputs_hash[0] ^= 1), &m),
            Err(Refusal::InputsHash)
        );
        let mut more = tx.clone();
        more.input.pop();
        assert_eq!(check(&more, &m), Err(Refusal::InputsHash));
        // The funder's scriptSigs, sequences and lock time aren't read.
        let mut theirs = tx.clone();
        theirs.input[0].sequence = 0xffff_fffe;
        theirs.input[0].script_sig = ScriptBuf::from_bytes(vec![0x51]);
        theirs.lock_time = 1_000;
        assert!(check(&theirs, &m).is_ok());
    }
}
