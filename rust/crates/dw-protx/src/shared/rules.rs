//! Dash Core's checks of the shared-masternode maintenance payloads, as pure
//! functions returning Core's reject reason (v24.0.0-rc.3):
//!
//! - `CProDisTx`, `CProUpShareTx` and `CProUpSharedRegTx::IsTriviallyValid`
//!   (`src/evo/providertx.cpp:535-589`);
//! - `CheckProUpShareTx`, `CheckProUpSharedRegTx`, `CheckProDisTxForList` and
//!   `CheckProDisTx` (`src/evo/specialtxman.cpp:1563-1812`), in Core's order.
//!
//! The engine runs them before it signs or sends (MG-04b §5.1): a refusal on
//! the network is silent. Core validates against `pindexPrev`'s list; the
//! caller passes the masternode as the wallet knows it ([`SharedMn`]: the
//! share table and terms from the registration in its history, the voting
//! key from the synced list) and the height the transaction would be mined
//! at. Signature checks are `CHashSigner::VerifyHashCanonical`
//! (`src/messagesigner.cpp:58-90`).

use dashcore::hashes::Hash;
use dashcore::{OutPoint, ScriptBuf, Txid};

use super::rawtx::SpecialTx;
use super::{CollateralShare, PRODIS_MAX_FEE, ProDisTx, ProUpShareTx, ProUpSharedRegTx};
use crate::params::MAX_SHARES;
use crate::rules::{
    ListedMasternode, Reject, holds_operator_key, is_valid_operator_key, p2pkh_key_id, signed_by,
};
use dashcore::bls_sig_utils::BlsScheme;

/// A registered shared masternode, as the wallet knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedMn<'a> {
    pub pro_tx_hash: Txid,
    pub collateral: OutPoint,
    /// From the registration; empty for a masternode that isn't shared.
    pub shares: &'a [CollateralShare],
    pub registered_height: u32,
    pub early_period_blocks: u32,
    pub early_penalty: i64,
    /// The list's current voting key.
    pub voting_key_id: [u8; 20],
}

impl SharedMn<'_> {
    /// `CDeterministicMNState::IsShared` (`evo/dmnstate.h:157`).
    fn is_shared(&self) -> bool {
        !self.shares.is_empty()
    }
}

/// A share's new reward in a ProUpShareTx (MG-04b D31).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareReward {
    /// Rewards back to the share's refund address.
    Refund,
    Script(ScriptBuf),
}

impl ShareReward {
    /// The `scriptReward` to write. `Refund` is the refund script itself:
    /// a ProUpShareTx's reward must be P2PKH or P2SH, so an empty one (which
    /// means "the refund" only at registration) is `bad-proupshare-payee`
    /// (`providertx.cpp:560-562`; "use the refund script to reset rewards",
    /// `providertx.h:493`).
    pub fn script(&self, share: &CollateralShare) -> ScriptBuf {
        match self {
            Self::Refund => share.refund_script.clone(),
            Self::Script(script) => script.clone(),
        }
    }
}

/// `CHashSigner::VerifyHashCanonical`: 65 bytes, header 27–34, low S, and
/// the recovered key's hash160 is `key_id`.
pub fn verify_hash_canonical(hash: [u8; 32], key_id: [u8; 20], sig: &[u8]) -> bool {
    const HALF_ORDER: [u8; 32] = [
        0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b,
        0x20, 0xa0,
    ];
    sig.len() == 65
        && (27..=34).contains(&sig[0])
        && sig[33..] <= HALF_ORDER[..]
        && signed_by(sig, hash, key_id)
}

/// `ProRataFloor` (`evo/providertx_util.cpp:88-95`): `floor(total × weight /
/// weight_total)` with a wide intermediate.
pub fn pro_rata_floor(total: u64, weight: u64, weight_total: u64) -> u64 {
    (u128::from(total) * u128::from(weight) / u128::from(weight_total)) as u64
}

/// `ExtractDestination(script) == PKHash(key_id)`: only a P2PKH script pays
/// a key id.
fn pays_key(script: &ScriptBuf, key_id: [u8; 20]) -> bool {
    p2pkh_key_id(script) == Some(key_id)
}

/// `IsValidPayoutScript`: P2PKH or P2SH.
fn is_valid_payout_script(script: &ScriptBuf) -> bool {
    script.is_p2pkh() || script.is_p2sh()
}

/// `IsShareListVotingKeySafe` (`providertx.cpp:132-144`): no share's refund
/// or reward pays the voting key.
pub fn share_list_voting_key_safe(shares: &[CollateralShare], voting_key_id: [u8; 20]) -> bool {
    shares.iter().all(|s| {
        !pays_key(&s.refund_script, voting_key_id)
            && !pays_key(s.effective_reward_script(), voting_key_id)
    })
}

fn check_sig_count(count: usize, reason: Reject) -> Result<(), Reject> {
    if count == 0 || count > MAX_SHARES as usize {
        return Err(reason);
    }
    Ok(())
}

/// `CProDisTx::IsTriviallyValid` (`providertx.cpp:535-544`).
pub fn check_dissolution_trivial(p: &ProDisTx) -> Result<(), Reject> {
    if p.version == 0 || p.version > ProDisTx::CURRENT_VERSION {
        return Err("bad-prodis-version");
    }
    check_sig_count(p.sigs.len(), "bad-prodis-sig-count")
}

/// `CProUpShareTx::IsTriviallyValid` (`providertx.cpp:552-564`).
pub fn check_update_share_trivial(p: &ProUpShareTx) -> Result<(), Reject> {
    if p.version == 0 || p.version > ProUpShareTx::CURRENT_VERSION {
        return Err("bad-proupshare-version");
    }
    if p.sig.len() != 65 {
        return Err("bad-proupshare-sig-size");
    }
    if !is_valid_payout_script(&p.reward_script) {
        return Err("bad-proupshare-payee");
    }
    Ok(())
}

/// `CProUpSharedRegTx::IsTriviallyValid` (`providertx.cpp:574-589`). The
/// operator key is serialized basic, so a key that is only a valid legacy
/// point fails as invalid; Core's separate `IsLegacy` refusal can't arise
/// from the wire.
pub fn check_update_shared_registrar_trivial(p: &ProUpSharedRegTx) -> Result<(), Reject> {
    if p.version == 0 || p.version > ProUpSharedRegTx::CURRENT_VERSION {
        return Err("bad-proupsharedreg-version");
    }
    if !is_valid_operator_key(&p.operator_public_key, BlsScheme::Modern)
        || p.voting_key_id == [0; 20]
    {
        return Err("bad-protx-key-null");
    }
    check_sig_count(p.sigs.len(), "bad-proupsharedreg-sig-count")
}

/// The masternode the payload names, listed and shared.
fn listed<'m, 'a>(
    mn: Option<&'m SharedMn<'a>>,
    pro_tx_hash: Txid,
    hash: Reject,
    not_shared: Reject,
) -> Result<&'m SharedMn<'a>, Reject> {
    let mn = mn.filter(|m| m.pro_tx_hash == pro_tx_hash).ok_or(hash)?;
    if !mn.is_shared() {
        return Err(not_shared);
    }
    Ok(mn)
}

fn check_inputs_hash(tx: &SpecialTx, inputs_hash: [u8; 32]) -> Result<(), Reject> {
    if tx.inputs_hash() != inputs_hash {
        return Err("bad-protx-inputs-hash");
    }
    Ok(())
}

/// `CheckProUpShareTx` (`specialtxman.cpp:1563-1617`), after the trivial
/// checks.
pub fn check_update_share(
    tx: &SpecialTx,
    p: &ProUpShareTx,
    mn: Option<&SharedMn>,
    v24_active: bool,
    check_sigs: bool,
) -> Result<(), Reject> {
    check_update_share_trivial(p)?;
    if !v24_active {
        return Err("bad-proupshare-too-early");
    }
    let mn = listed(
        mn,
        p.pro_tx_hash,
        "bad-proupshare-hash",
        "bad-proupshare-not-shared",
    )?;
    let share = mn
        .shares
        .get(usize::from(p.share_index))
        .ok_or("bad-proupshare-index")?;
    // `ExtractDestination` succeeds for every P2PKH and P2SH script, which
    // the trivial check already required.
    if !is_valid_payout_script(&p.reward_script) {
        return Err("bad-proupshare-payee-dest");
    }
    if pays_key(&p.reward_script, mn.voting_key_id)
        || mn
            .shares
            .iter()
            .any(|s| pays_key(&p.reward_script, s.owner_key_id))
    {
        return Err("bad-proupshare-payee-reuse");
    }
    check_inputs_hash(tx, p.inputs_hash)?;
    if check_sigs && !verify_hash_canonical(p.sign_hash(), share.owner_key_id, &p.sig) {
        return Err("bad-proupshare-sig");
    }
    Ok(())
}

/// `CheckProUpSharedRegTx` (`specialtxman.cpp:1619-1674`), after the
/// trivial checks. `list` is the synced list, for the operator key's
/// uniqueness under either scheme (this masternode excluded).
pub fn check_update_shared_registrar(
    tx: &SpecialTx,
    p: &ProUpSharedRegTx,
    mn: Option<&SharedMn>,
    list: &[ListedMasternode],
    v24_active: bool,
    check_sigs: bool,
) -> Result<(), Reject> {
    check_update_shared_registrar_trivial(p)?;
    if !v24_active {
        return Err("bad-proupsharedreg-too-early");
    }
    let mn = listed(
        mn,
        p.pro_tx_hash,
        "bad-proupsharedreg-hash",
        "bad-proupsharedreg-not-shared",
    )?;
    if p.sigs.len() != mn.shares.len() {
        return Err("bad-proupsharedreg-sig-count");
    }
    let own = p.pro_tx_hash.to_byte_array();
    if list.iter().any(|e| {
        e.pro_tx_hash != own && holds_operator_key(e, &p.operator_public_key, BlsScheme::Modern)
    }) {
        return Err("bad-protx-dup-key");
    }
    if !share_list_voting_key_safe(mn.shares, p.voting_key_id) {
        return Err("bad-proupsharedreg-payee-reuse");
    }
    check_inputs_hash(tx, p.inputs_hash)?;
    if check_sigs {
        let hash = p.sign_hash();
        if !mn
            .shares
            .iter()
            .zip(&p.sigs)
            .all(|(s, sig)| verify_hash_canonical(hash, s.owner_key_id, sig))
        {
            return Err("bad-proupsharedreg-sig");
        }
    }
    Ok(())
}

/// `CheckProDisTx` (`specialtxman.cpp:1789-1812`) with
/// `CheckProDisTxForList` (`:1676-1787`), after the trivial checks.
/// `spend_height` is the height the transaction would be mined at.
pub fn check_dissolution(
    tx: &SpecialTx,
    p: &ProDisTx,
    mn: Option<&SharedMn>,
    spend_height: u32,
    v24_active: bool,
    check_sigs: bool,
) -> Result<(), Reject> {
    check_dissolution_trivial(p)?;
    if !v24_active {
        return Err("bad-prodis-too-early");
    }
    let mn = listed(
        mn,
        p.pro_tx_hash,
        "bad-prodis-hash",
        "bad-prodis-not-shared",
    )?;
    let shares = mn.shares;
    if tx.input.len() != 1
        || tx.input[0].previous_output != mn.collateral
        || !tx.input[0].script_sig.is_empty()
    {
        return Err("bad-prodis-input");
    }
    let actor = usize::from(p.actor_index);
    if actor >= shares.len() {
        return Err("bad-prodis-actor");
    }
    if p.sigs.len() != 1 && p.sigs.len() != shares.len() {
        return Err("bad-prodis-sig-count");
    }
    let unanimous = p.sigs.len() == shares.len();
    let early = i64::from(spend_height) - i64::from(mn.registered_height)
        < i64::from(mn.early_period_blocks);
    let required_penalty = if unanimous || !early {
        0
    } else {
        mn.early_penalty.max(0) as u64
    };

    let non_actor_count = shares.len() - 1;
    if tx.output.len() != non_actor_count && tx.output.len() != non_actor_count + 1 {
        return Err("bad-prodis-payee-count");
    }
    let non_actor_total: u64 = shares
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != actor)
        .map(|(_, s)| s.amount)
        .sum();
    let mut outputs = tx.output.iter();
    let mut bonus_total: i128 = 0;
    for (_, share) in shares.iter().enumerate().filter(|(i, _)| *i != actor) {
        let out = outputs.next().expect("counted above");
        if out.script_pubkey != share.refund_script {
            return Err("bad-prodis-payee");
        }
        let bonus = i128::from(out.value) - i128::from(share.amount);
        if bonus
            < i128::from(pro_rata_floor(
                required_penalty,
                share.amount,
                non_actor_total,
            ))
        {
            return Err("bad-prodis-penalty-floor");
        }
        bonus_total += bonus;
    }
    if let Some(actor_out) = outputs.next() {
        if actor_out.script_pubkey != shares[actor].refund_script {
            return Err("bad-prodis-payee");
        }
        if actor_out.value == 0 {
            return Err("bad-prodis-actor-output-zero");
        }
    }
    if bonus_total < i128::from(required_penalty) {
        return Err("bad-prodis-penalty-sum");
    }
    let out_total: i128 = tx.output.iter().map(|o| i128::from(o.value)).sum();
    let fee = i128::from(non_actor_total) + i128::from(shares[actor].amount) - out_total;
    if fee > i128::from(PRODIS_MAX_FEE) {
        return Err("bad-prodis-fee");
    }
    if !unanimous && bonus_total > i128::from(mn.early_penalty) {
        return Err("bad-prodis-bonus");
    }
    if check_sigs {
        let hash = p.sign_hash(
            tx.version,
            tx.lock_time,
            &tx.prevouts(),
            &tx.output,
            p.sigs.len() as u8,
        );
        let ok = if unanimous {
            shares
                .iter()
                .zip(&p.sigs)
                .all(|(s, sig)| verify_hash_canonical(hash, s.owner_key_id, sig))
        } else {
            verify_hash_canonical(hash, shares[actor].owner_key_id, &p.sigs[0])
        };
        if !ok {
            return Err("bad-prodis-sig");
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::payloads::sign_hash_compact;
    use crate::rules::p2pkh_key_id;
    use dashcore::secp256k1::SecretKey;
    use dashcore::{TxIn, TxOut, Witness};

    const COIN: u64 = 100_000_000;
    const PENALTY: i64 = 10 * COIN as i64;

    fn p2pkh(id: [u8; 20]) -> ScriptBuf {
        let mut b = vec![0x76, 0xa9, 0x14];
        b.extend_from_slice(&id);
        b.extend_from_slice(&[0x88, 0xac]);
        ScriptBuf::from_bytes(b)
    }

    fn secret(i: usize) -> [u8; 32] {
        [i as u8 + 1; 32]
    }

    fn owner_id(i: usize) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes(secret(i)).unwrap();
        let pk = dashcore::PublicKey::new(sk.public_key());
        pk.pubkey_hash().to_byte_array()
    }

    fn shares() -> Vec<CollateralShare> {
        [400, 300, 300]
            .iter()
            .enumerate()
            .map(|(i, dash)| CollateralShare {
                amount: dash * COIN,
                refund_script: p2pkh([0x10 + i as u8; 20]),
                reward_script: ScriptBuf::new(),
                owner_key_id: owner_id(i),
            })
            .collect()
    }

    const VOTING: [u8; 20] = [0x77; 20];

    fn mn(shares: &[CollateralShare]) -> SharedMn<'_> {
        SharedMn {
            pro_tx_hash: Txid::from_byte_array([0xAA; 32]),
            collateral: OutPoint::new(Txid::from_byte_array([0xAA; 32]), 3),
            shares,
            registered_height: 1_000,
            early_period_blocks: 100,
            early_penalty: PENALTY,
            voting_key_id: VOTING,
        }
    }

    fn input(prevout: OutPoint) -> TxIn {
        TxIn {
            previous_output: prevout,
            script_sig: ScriptBuf::new(),
            sequence: u32::MAX,
            witness: Witness::default(),
        }
    }

    fn special(tx_type: u16, input: Vec<TxIn>, output: Vec<TxOut>) -> SpecialTx {
        SpecialTx {
            version: 3,
            tx_type,
            input,
            output,
            lock_time: 0,
            payload: vec![],
        }
    }

    // ---- ProUpShareTx (type 11) ----

    fn signed_update(
        shares: &[CollateralShare],
        index: u16,
        reward: ShareReward,
    ) -> (SpecialTx, ProUpShareTx) {
        let tx = special(
            11,
            vec![input(OutPoint::new(Txid::from_byte_array([9; 32]), 0))],
            vec![],
        );
        let mut p = ProUpShareTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([0xAA; 32]),
            share_index: index,
            reward_script: reward.script(&shares[usize::from(index)]),
            inputs_hash: tx.inputs_hash(),
            sig: vec![],
        };
        p.sig = sign_hash_compact(&secret(usize::from(index)), &p.sign_hash())
            .unwrap()
            .to_vec();
        (tx, p)
    }

    #[test]
    fn test_QT_127_reward_reset_is_the_refund_script_never_empty() {
        let shares = shares();
        let (tx, p) = signed_update(&shares, 1, ShareReward::Refund);
        assert_eq!(p.reward_script, shares[1].refund_script);
        assert_eq!(
            check_update_share(&tx, &p, Some(&mn(&shares)), true, true),
            Ok(())
        );
        // The registration's "empty = the refund" is invalid in a ProUpShareTx.
        let mut empty = p.clone();
        empty.reward_script = ScriptBuf::new();
        assert_eq!(
            check_update_share_trivial(&empty),
            Err("bad-proupshare-payee")
        );
    }

    #[test]
    fn test_QT_127_update_share_rules_in_core_order() {
        let shares = shares();
        let m = mn(&shares);
        let new_reward = ShareReward::Script(p2pkh([0x55; 20]));
        let (tx, p) = signed_update(&shares, 2, new_reward.clone());
        assert_eq!(check_update_share(&tx, &p, Some(&m), true, true), Ok(()));

        let refuse = |p: &ProUpShareTx, m: Option<&SharedMn>, v24: bool| {
            check_update_share(&tx, p, m, v24, true)
        };
        let mut v = p.clone();
        v.version = 2;
        assert_eq!(refuse(&v, Some(&m), true), Err("bad-proupshare-version"));
        let mut v = p.clone();
        v.sig.pop();
        assert_eq!(refuse(&v, Some(&m), true), Err("bad-proupshare-sig-size"));
        assert_eq!(refuse(&p, Some(&m), false), Err("bad-proupshare-too-early"));
        assert_eq!(refuse(&p, None, true), Err("bad-proupshare-hash"));
        let mut other = m.clone();
        other.pro_tx_hash = Txid::from_byte_array([0xBB; 32]);
        assert_eq!(refuse(&p, Some(&other), true), Err("bad-proupshare-hash"));
        let not_shared = SharedMn {
            shares: &[],
            ..m.clone()
        };
        assert_eq!(
            refuse(&p, Some(&not_shared), true),
            Err("bad-proupshare-not-shared")
        );
        let mut v = p.clone();
        v.share_index = 3;
        assert_eq!(refuse(&v, Some(&m), true), Err("bad-proupshare-index"));
        for reuse in [VOTING, owner_id(0), owner_id(2)] {
            let mut v = p.clone();
            v.reward_script = p2pkh(reuse);
            assert_eq!(
                refuse(&v, Some(&m), true),
                Err("bad-proupshare-payee-reuse")
            );
        }
        let mut v = p.clone();
        v.inputs_hash = [0; 32];
        assert_eq!(refuse(&v, Some(&m), true), Err("bad-protx-inputs-hash"));
        // Signed by another share's owner.
        let mut v = p.clone();
        v.sig = sign_hash_compact(&secret(0), &v.sign_hash())
            .unwrap()
            .to_vec();
        assert_eq!(refuse(&v, Some(&m), true), Err("bad-proupshare-sig"));
        assert_eq!(check_update_share(&tx, &v, Some(&m), true, false), Ok(()));
    }

    #[test]
    fn test_QT_127_canonical_signatures_only() {
        let hash = [0x42; 32];
        let sig = sign_hash_compact(&secret(0), &hash).unwrap();
        assert!(verify_hash_canonical(hash, owner_id(0), &sig));
        assert!(!verify_hash_canonical(hash, owner_id(1), &sig));
        // A header alias (27 + 8) recovers the same key in Core's RecoverCompact.
        let mut alias = sig;
        alias[0] += 8;
        assert!(!verify_hash_canonical(hash, owner_id(0), &alias));
        // High S: (r, n − s) with the other parity recovers the same key.
        let mut high = sig;
        let n: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c,
            0xd0, 0x36, 0x41, 0x41,
        ];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let d = i16::from(n[i]) - i16::from(sig[33 + i]) - borrow;
            high[33 + i] = d.rem_euclid(256) as u8;
            borrow = i16::from(d < 0);
        }
        // The other parity, still compressed: 27 + 4 + (recid ^ 1).
        high[0] = 31 + ((sig[0] - 31) ^ 1);
        assert!(
            signed_by(&high, hash, owner_id(0)),
            "the malleated form still recovers"
        );
        assert!(!verify_hash_canonical(hash, owner_id(0), &high));
        assert!(!verify_hash_canonical(hash, owner_id(0), &sig[..64]));
    }

    // ---- ProDisTx (type 10) ----

    /// Core's `BuildDissolution` (`providertx_service.cpp:639-681`).
    fn build(shares: &[CollateralShare], actor: usize, penalty: u64, fee: u64) -> Vec<TxOut> {
        let non_actors: Vec<_> = shares
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != actor)
            .collect();
        let weight: u64 = non_actors.iter().map(|(_, s)| s.amount).sum();
        let mut paid = 0;
        let mut out = vec![];
        for (k, (_, s)) in non_actors.iter().enumerate() {
            let bonus = if k + 1 == non_actors.len() {
                penalty - paid
            } else {
                pro_rata_floor(penalty, s.amount, weight)
            };
            paid += bonus;
            out.push(TxOut {
                value: s.amount + bonus,
                script_pubkey: s.refund_script.clone(),
            });
        }
        let actor_out = shares[actor].amount - penalty - fee;
        if actor_out > 0 {
            out.push(TxOut {
                value: actor_out,
                script_pubkey: shares[actor].refund_script.clone(),
            });
        }
        out
    }

    fn dissolution(
        m: &SharedMn,
        actor: usize,
        penalty: u64,
        fee: u64,
        signers: &[usize],
    ) -> (SpecialTx, ProDisTx) {
        let tx = special(
            10,
            vec![input(m.collateral)],
            build(m.shares, actor, penalty, fee),
        );
        let mut p = ProDisTx {
            version: 1,
            pro_tx_hash: m.pro_tx_hash,
            actor_index: actor as u16,
            sigs: vec![],
        };
        let count = signers.len() as u8;
        let hash = p.sign_hash(tx.version, tx.lock_time, &tx.prevouts(), &tx.output, count);
        p.sigs = signers
            .iter()
            .map(|&i| sign_hash_compact(&secret(i), &hash).unwrap())
            .collect();
        (tx, p)
    }

    const FEE: u64 = 1_000;
    const EARLY: u32 = 1_050;
    const LATE: u32 = 1_100;

    #[test]
    fn test_QT_127_unilateral_dissolution_pays_the_penalty_while_early() {
        let shares = shares();
        let m = mn(&shares);
        let (tx, p) = dissolution(&m, 0, PENALTY as u64, FEE, &[0]);
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), EARLY, true, true),
            Ok(())
        );
        // Valid at every later height too (monotone, specialtxman.cpp:1705-1708).
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), LATE, true, true),
            Ok(())
        );
        // Without the penalty it is valid only once the period is over.
        let (tx, p) = dissolution(&m, 0, 0, FEE, &[0]);
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), EARLY, true, true),
            Err("bad-prodis-penalty-floor")
        );
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), LATE, true, true),
            Ok(())
        );
        // Unanimous: no penalty at any height.
        let (tx, p) = dissolution(&m, 1, 0, FEE, &[0, 1, 2]);
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), EARLY, true, true),
            Ok(())
        );
    }

    #[test]
    fn test_QT_127_dissolution_rules_in_core_order() {
        let shares = shares();
        let m = mn(&shares);
        let (tx, p) = dissolution(&m, 2, PENALTY as u64, FEE, &[2]);
        let check =
            |tx: &SpecialTx, p: &ProDisTx| check_dissolution(tx, p, Some(&m), EARLY, true, true);
        assert_eq!(check(&tx, &p), Ok(()));

        let mut v = p.clone();
        v.version = 0;
        assert_eq!(check(&tx, &v), Err("bad-prodis-version"));
        let mut v = p.clone();
        v.sigs = vec![[0; 65]; 9];
        assert_eq!(check(&tx, &v), Err("bad-prodis-sig-count"));
        assert_eq!(
            check_dissolution(&tx, &p, Some(&m), EARLY, false, true),
            Err("bad-prodis-too-early")
        );
        assert_eq!(
            check_dissolution(&tx, &p, None, EARLY, true, true),
            Err("bad-prodis-hash")
        );
        let not_shared = SharedMn {
            shares: &[],
            ..m.clone()
        };
        assert_eq!(
            check_dissolution(&tx, &p, Some(&not_shared), EARLY, true, true),
            Err("bad-prodis-not-shared")
        );
        let mut t = tx.clone();
        t.input
            .push(input(OutPoint::new(Txid::from_byte_array([1; 32]), 0)));
        assert_eq!(check(&t, &p), Err("bad-prodis-input"));
        let mut t = tx.clone();
        t.input[0].script_sig = ScriptBuf::from_bytes(vec![0x51]);
        assert_eq!(check(&t, &p), Err("bad-prodis-input"));
        let mut v = p.clone();
        v.actor_index = 3;
        assert_eq!(check(&tx, &v), Err("bad-prodis-actor"));
        let mut v = p.clone();
        v.sigs.push(v.sigs[0]);
        assert_eq!(check(&tx, &v), Err("bad-prodis-sig-count"));
        let mut t = tx.clone();
        t.output.pop();
        t.output.pop();
        assert_eq!(check(&t, &p), Err("bad-prodis-payee-count"));
        let mut t = tx.clone();
        t.output[0].script_pubkey = p2pkh([0x99; 20]);
        assert_eq!(check(&t, &p), Err("bad-prodis-payee"));
        let mut t = tx.clone();
        t.output[0].value -= 1;
        assert_eq!(check(&t, &p), Err("bad-prodis-penalty-floor"));
        let mut t = tx.clone();
        t.output[2].script_pubkey = shares[0].refund_script.clone();
        assert_eq!(check(&t, &p), Err("bad-prodis-payee"));
        let mut t = tx.clone();
        t.output[2].value = 0;
        assert_eq!(check(&t, &p), Err("bad-prodis-actor-output-zero"));
        // The per-share floors pass but the remainder duff is missing.
        let m3 = SharedMn {
            early_penalty: 3,
            ..m.clone()
        };
        let (mut t, v) = dissolution(&m3, 0, 3, FEE, &[0]);
        t.output[1].value -= 1;
        assert_eq!(
            check_dissolution(&t, &v, Some(&m3), EARLY, true, false),
            Err("bad-prodis-penalty-sum")
        );
        let (t, v) = dissolution(&m, 2, PENALTY as u64, PRODIS_MAX_FEE + 1, &[2]);
        assert_eq!(check(&t, &v), Err("bad-prodis-fee"));
        // A unilateral exit may not pay the others more than the penalty.
        let (t, v) = dissolution(&m, 2, PENALTY as u64 + 1, FEE, &[2]);
        assert_eq!(check(&t, &v), Err("bad-prodis-bonus"));
        // Signed by a non-actor, or by too few for a unanimous one.
        let (t, v) = dissolution(&m, 2, PENALTY as u64, FEE, &[1]);
        assert_eq!(check(&t, &v), Err("bad-prodis-sig"));
        let (t, mut v) = dissolution(&m, 2, 0, FEE, &[0, 1, 2]);
        v.sigs.swap(0, 1);
        assert_eq!(check(&t, &v), Err("bad-prodis-sig"));
    }

    // ---- ProUpSharedRegTx (type 12) ----

    fn key_change(shares: &[CollateralShare]) -> (SpecialTx, ProUpSharedRegTx) {
        let tx = special(
            12,
            vec![input(OutPoint::new(Txid::from_byte_array([8; 32]), 1))],
            vec![],
        );
        let mut p = ProUpSharedRegTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([0xAA; 32]),
            operator_public_key: crate::bls::TEST_OPERATOR_KEY,
            voting_key_id: [0x66; 20],
            inputs_hash: tx.inputs_hash(),
            sigs: vec![],
        };
        let hash = p.sign_hash();
        p.sigs = (0..shares.len())
            .map(|i| sign_hash_compact(&secret(i), &hash).unwrap())
            .collect();
        (tx, p)
    }

    fn listed(hash: u8, operator: [u8; 48]) -> ListedMasternode {
        ListedMasternode {
            pro_tx_hash: [hash; 32],
            service: None,
            operator_public_key: operator,
            voting_key_id: [0; 20],
            platform_node_id: None,
            collateral: None,
        }
    }

    #[test]
    fn test_QT_127_key_change_rules_in_core_order() {
        let shares = shares();
        let m = mn(&shares);
        let (tx, p) = key_change(&shares);
        let check = |p: &ProUpSharedRegTx, list: &[ListedMasternode]| {
            check_update_shared_registrar(&tx, p, Some(&m), list, true, true)
        };
        // Its own entry holding the key is no duplicate.
        assert_eq!(
            check(&p, &[listed(0xAA, crate::bls::TEST_OPERATOR_KEY)]),
            Ok(())
        );

        let mut v = p.clone();
        v.version = 2;
        assert_eq!(check(&v, &[]), Err("bad-proupsharedreg-version"));
        let mut v = p.clone();
        v.voting_key_id = [0; 20];
        assert_eq!(check(&v, &[]), Err("bad-protx-key-null"));
        let mut v = p.clone();
        v.operator_public_key = [0; 48];
        assert_eq!(check(&v, &[]), Err("bad-protx-key-null"));
        let mut v = p.clone();
        v.sigs.clear();
        assert_eq!(check(&v, &[]), Err("bad-proupsharedreg-sig-count"));
        assert_eq!(
            check_update_shared_registrar(&tx, &p, Some(&m), &[], false, true),
            Err("bad-proupsharedreg-too-early")
        );
        assert_eq!(
            check_update_shared_registrar(&tx, &p, None, &[], true, true),
            Err("bad-proupsharedreg-hash")
        );
        let mut v = p.clone();
        v.sigs.pop();
        assert_eq!(check(&v, &[]), Err("bad-proupsharedreg-sig-count"));
        // Another masternode holds the key, in either serialization.
        assert_eq!(
            check(&p, &[listed(0xBB, crate::bls::TEST_OPERATOR_KEY)]),
            Err("bad-protx-dup-key")
        );
        let legacy = crate::bls::legacy_of(&crate::bls::TEST_OPERATOR_KEY).unwrap();
        assert_eq!(check(&p, &[listed(0xBB, legacy)]), Err("bad-protx-dup-key"));
        // The new voting key may not be paid by any share's refund or reward.
        let mut v = p.clone();
        v.voting_key_id = p2pkh_key_id(&shares[1].refund_script).unwrap();
        assert_eq!(check(&v, &[]), Err("bad-proupsharedreg-payee-reuse"));
        let mut rewarded = shares.clone();
        rewarded[2].reward_script = p2pkh([0x66; 20]);
        assert_eq!(
            check_update_shared_registrar(&tx, &p, Some(&mn(&rewarded)), &[], true, false),
            Err("bad-proupsharedreg-payee-reuse")
        );
        let mut v = p.clone();
        v.inputs_hash = [0; 32];
        assert_eq!(check(&v, &[]), Err("bad-protx-inputs-hash"));
        // Every share signs, in share order.
        let mut v = p.clone();
        v.sigs.swap(1, 2);
        assert_eq!(check(&v, &[]), Err("bad-proupsharedreg-sig"));
    }
}
