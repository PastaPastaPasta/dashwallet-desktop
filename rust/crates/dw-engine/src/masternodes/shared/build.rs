//! The pure steps of a shared registration (MG-04b §2.4, §4, §5): the
//! service policy (D5), the coin selection (§5's funding), the placeholder
//! Core's rules run on before any key is spent, the frozen registration
//! (`shared_register_prepare`'s), the combine (join signatures into the
//! payload, D24: no coordinator scriptSig) and the funding signatures over
//! this wallet's own inputs only (`signedOnlyOwnInputs`).

use std::net::SocketAddr;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, PubkeyHash, ScriptBuf, TxOut, Txid};
use dw_protx::params::MASTERNODE_COLLATERAL;
use dw_protx::shared::envelope::decode_base64;
use dw_protx::shared::netinfo::{
    EXT_NET_INFO_VERSION, NetInfoEntry, PURPOSE_CORE_P2P, ServiceAddr, parse_core_service,
};
use dw_protx::shared::rawtx::{SIGHASH_ALL, SPECIAL_VERSION};
use dw_protx::shared::registration::{
    MN_TYPE_REGULAR, NULL_JOIN_SIGNATURE, SHARED_COLLATERAL_SCRIPT, TRANSACTION_PROVIDER_REGISTER,
};
use dw_protx::shared::session::consent::P2PKH_DUST;
use dw_protx::shared::session::funding::{
    check_frozen, decode_frozen, funding_parts, key_id_of, script_for, service_entries,
};
use dw_protx::shared::session::merge::check_join_signatures;
use dw_protx::shared::session::{MAX_CONTRIBUTION_INPUTS, Session, Stage};
use dw_protx::shared::{CollateralShare, ExtNetInfo, ProRegTxV3, SpecialTx};
use dw_vault::VaultSigner;
use key_wallet::{DerivationPath, Signer, Utxo};

use super::super::MasternodesError;
use super::super::protx::build::invalid;
use super::store::{envelope_refusal, funding_refusal, merge_refusal};
use crate::EngineError;
use crate::send::SendFailure;

/// How a service text fails the wallet's policy (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceProblem {
    /// Not an address at all: `invalid_argument`.
    Invalid,
    /// A kind the wallet doesn't register or join with (a domain, Tor, I2P,
    /// CJDNS; IPv6 as coordinator): `unsupported{field: "service"}`.
    Unsupported,
}

fn parse_entry(entry: &str) -> Result<SocketAddr, ServiceProblem> {
    entry.parse().map_err(|_| {
        // A host name, a bracketed or bare IPv6, an onion: a kind, not a typo.
        if entry.bytes().any(|b| b.is_ascii_alphabetic() || b == b'[')
            || entry.matches(':').count() > 1
        {
            ServiceProblem::Unsupported
        } else {
            ServiceProblem::Invalid
        }
    })
}

/// D5 for the coordinator's request: required, numeric IPv4 `a.b.c.d:port`
/// entries only.
pub(crate) fn coordinator_services(text: &str) -> Result<Vec<SocketAddr>, ServiceProblem> {
    let parsed = service_entries(text)
        .map(parse_entry)
        .collect::<Result<Vec<_>, _>>()?;
    if parsed.is_empty() {
        return Err(ServiceProblem::Invalid);
    }
    if parsed.iter().any(|a| !a.is_ipv4()) {
        return Err(ServiceProblem::Unsupported);
    }
    Ok(parsed)
}

/// D5 for a participant: a dash-qt coordinator's empty, IPv4 or IPv6
/// service, in any form Core reads (`parse_core_service`: `127.1`,
/// `[::ffff:127.0.0.1]`, …), is accepted; anything else (an fc00::/8
/// address flipped to CJDNS first, as Core does) is refused.
pub(crate) fn participant_services_ok(text: &str) -> bool {
    let Some(services) = service_entries(text)
        .map(|e| {
            parse_core_service(e)
                .and_then(ServiceAddr::from_socket_addr)
                .map(NetInfoEntry::Service)
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if services.is_empty() {
        return true;
    }
    ExtNetInfo {
        version: EXT_NET_INFO_VERSION,
        entries: vec![(PURPOSE_CORE_P2P, services)],
    }
    .is_ip_core_p2p_only()
}

/// The payload's net info for `coreP2PAddrs`, as `shared_register_prepare`
/// builds it with `AddEntry` (the empty info for an empty service). `None`:
/// not one Core can carry (more than four entries, an address without an IP
/// form).
pub(crate) fn net_info_of(text: &str) -> Option<ExtNetInfo> {
    let parsed = service_entries(text)
        .map(parse_core_service)
        .collect::<Option<Vec<_>>>()?;
    ExtNetInfo::core_p2p(&parsed)
}

/// A selection of this wallet's coins for one contribution.
#[derive(Debug, Clone)]
pub(crate) struct Selected {
    pub inputs: Vec<(Utxo, DerivationPath)>,
    pub total: u64,
    /// `None` when the remainder is below dust (it goes to the fee).
    pub change: Option<u64>,
}

/// §5's funding (`sharedmncreatedialog.cpp:3017-3072`): largest first over
/// the confirmed `candidates` until `target` is covered; change at or above
/// dust is kept, below it goes to the fee. At most 64 inputs
/// (`MAX_CONTRIBUTION_INPUTS`).
pub(crate) fn select(
    mut candidates: Vec<(Utxo, DerivationPath)>,
    target: u64,
) -> Result<Selected, SendFailure> {
    candidates.retain(|(u, _)| u.is_confirmed);
    candidates
        .sort_by(|(a, _), (b, _)| b.value().cmp(&a.value()).then(a.outpoint.cmp(&b.outpoint)));
    let available: u64 = candidates.iter().map(|(u, _)| u.value()).sum();
    let mut inputs = Vec::new();
    let mut total = 0u64;
    for coin in candidates {
        if total >= target {
            break;
        }
        if inputs.len() == MAX_CONTRIBUTION_INPUTS {
            return Err(SendFailure::TxTooLarge);
        }
        total = total.saturating_add(coin.0.value());
        inputs.push(coin);
    }
    if total < target {
        return Err(SendFailure::AmountExceedsBalance { available });
    }
    let change = Some(total - target).filter(|c| *c >= P2PKH_DUST);
    Ok(Selected {
        inputs,
        total,
        change,
    })
}

fn p2pkh(id: [u8; 20]) -> ScriptBuf {
    ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(id))
}

/// The registration payload the session's rows and terms give, collateral
/// output `collateral_vout`. `placeholder`: a row without an owner yet
/// gets a stand-in owner key and refund (distinct, never a wallet's), so
/// Core's trivial and list rules can run before every row is filled.
fn payload_of(session: &Session, collateral_vout: u32, placeholder: bool) -> ProRegTxV3 {
    let (network, t) = (session.network, &session.terms);
    let shares = session
        .shares
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let stand_in = placeholder && row.owner_address.is_empty();
            let n = i as u8;
            CollateralShare {
                amount: u64::try_from(row.amount).unwrap_or(0),
                refund_script: if stand_in {
                    p2pkh([0xb0 | n; 20])
                } else {
                    script_for(&row.refund_address, network)
                },
                reward_script: if row.reward_address.is_empty() {
                    ScriptBuf::new()
                } else {
                    script_for(&row.reward_address, network)
                },
                owner_key_id: if stand_in {
                    [0xa0 | n; 20]
                } else {
                    key_id_of(&row.owner_address, network).unwrap_or_default()
                },
            }
        })
        .collect::<Vec<_>>();
    let mut operator_public_key = [0u8; 48];
    // An invalid key stays null: Core's `bad-protx-key-null`.
    let _ = hex::decode_to_slice(&t.operator_pub_key, &mut operator_public_key);
    ProRegTxV3 {
        mn_type: MN_TYPE_REGULAR,
        mode: 0,
        collateral: OutPoint::new(Txid::all_zeros(), collateral_vout),
        net_info: net_info_of(&t.core_p2p_addrs).unwrap_or_default(),
        owner_key_id: [0; 20],
        operator_public_key,
        voting_key_id: key_id_of(&t.voting_address, network).unwrap_or_default(),
        // Out of range: Core's `bad-protx-operator-reward`.
        operator_reward: u16::try_from(t.operator_reward).unwrap_or(u16::MAX),
        payouts: Vec::new(),
        join_sigs: vec![NULL_JOIN_SIGNATURE; shares.len()],
        shares,
        early_period_blocks: t.early_period_blocks,
        early_penalty: t.early_penalty,
        inputs_hash: [0; 32],
        platform_node_id: None,
        sig: Vec::new(),
    }
}

fn collateral_output() -> TxOut {
    TxOut {
        value: MASTERNODE_COLLATERAL,
        script_pubkey: ScriptBuf::from_bytes(SHARED_COLLATERAL_SCRIPT.to_vec()),
    }
}

fn encoded(p: &ProRegTxV3) -> Result<Vec<u8>, MasternodesError> {
    p.to_payload()
        .map_err(|e| EngineError::Internal(format!("encoding a shared ProRegTx: {e}")).into())
}

/// The registration of `session` with stand-ins for its empty rows and no
/// funding inputs: what Core's rules see of the terms before anything is
/// locked (`start_shared`, `join_shared`).
pub(crate) fn placeholder(session: &Session) -> Result<(SpecialTx, ProRegTxV3), MasternodesError> {
    let mut p = payload_of(session, 0, true);
    let mut tx = SpecialTx {
        version: SPECIAL_VERSION,
        tx_type: TRANSACTION_PROVIDER_REGISTER,
        input: Vec::new(),
        output: vec![collateral_output()],
        lock_time: 0,
        payload: Vec::new(),
    };
    p.inputs_hash = tx.inputs_hash();
    tx.payload = encoded(&p)?;
    Ok((tx, p))
}

/// `freeze` (§3.1 step 3, `shared_register_prepare`): the funding template
/// with the collateral appended last, `nVersion` 3, type 1, lock time 0;
/// the payload of the rows and terms with null join signatures and the
/// inputs hash; `consentHash`, `collateralIndex`; `frozen`, approvals
/// cleared, the revision kept.
pub(crate) fn freeze(session: &mut Session) -> Result<(), MasternodesError> {
    let (input, mut output) = funding_parts(&session.contributions, session.network)
        .map_err(|_| funding_refusal("template"))?;
    let collateral_index = output.len();
    output.push(collateral_output());
    let mut tx = SpecialTx {
        version: SPECIAL_VERSION,
        tx_type: TRANSACTION_PROVIDER_REGISTER,
        input,
        output,
        lock_time: 0,
        payload: Vec::new(),
    };
    let mut p = payload_of(session, collateral_index as u32, false);
    p.inputs_hash = tx.inputs_hash();
    tx.payload = encoded(&p)?;
    session.protx = hex::encode(tx.to_bytes());
    session.consent_hash = ProRegTxV3::display_hash(&p.consent_hash(&tx));
    session.collateral_index = collateral_index as i32;
    session.sigs.clear();
    session.stage = Stage::Frozen;
    Ok(())
}

/// Back to `draft` (`unfreeze`, `mnsharesession.cpp:925-935`): the frozen
/// registration and every approval dropped, `revision` + 1.
pub(crate) fn unfreeze(session: &mut Session) {
    session.protx.clear();
    session.consent_hash.clear();
    session.collateral_index = -1;
    session.sigs.clear();
    session.stage = Stage::Draft;
    session.revision += 1;
}

/// The coordinator's combine once every share approved (§3.1 step 4,
/// `shared_combine`): each approval into the payload's join signatures,
/// `combined`, no scriptSig added (D24); then D12's checks on the result.
pub(crate) fn combine(session: &mut Session) -> Result<(), MasternodesError> {
    let mut frozen = decode_frozen(session).map_err(envelope_refusal)?;
    let sigs = (0..session.shares.len())
        .map(|i| {
            session
                .signature_for(i as i32)
                .and_then(|s| decode_base64(&s.signature))
                .and_then(|b| <[u8; 65]>::try_from(b).ok())
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| funding_refusal("join_signature"))?;
    frozen.payload.join_sigs = sigs;
    frozen.tx.payload = encoded(&frozen.payload)?;
    let mut combined = session.clone();
    combined.protx = hex::encode(frozen.tx.to_bytes());
    combined.stage = Stage::Combined;
    let checked = check_frozen(&combined).map_err(envelope_refusal)?;
    check_join_signatures(&combined, &checked).map_err(merge_refusal)?;
    *session = combined;
    Ok(())
}

/// One of this wallet's funding inputs: its outpoint, the script it pays
/// (the sighash's script code) and its key path.
pub(crate) type OwnInput = (OutPoint, ScriptBuf, DerivationPath);

/// Signs this wallet's inputs of `tx` (SIGHASH_ALL over the special
/// transaction, payload included) and checks that nothing else changed
/// (`signedOnlyOwnInputs`, `sharedmncreatedialog.cpp:2448-2466`).
pub(crate) async fn sign_own_inputs(
    tx: &SpecialTx,
    own: &[OwnInput],
    signer: &VaultSigner,
) -> Result<SpecialTx, MasternodesError> {
    let mut signed = tx.clone();
    for (index, input) in tx.input.iter().enumerate() {
        let Some((_, script, path)) = own.iter().find(|(o, _, _)| *o == input.previous_output)
        else {
            continue;
        };
        let hash = tx
            .signature_hash_all(index, script)
            .ok_or_else(|| EngineError::Internal("no such input".into()))?;
        let (sig, pubkey) = signer
            .sign_ecdsa(path, hash)
            .await
            .map_err(|e| EngineError::Internal(format!("signing a funding input: {e}")))?;
        signed.input[index].script_sig =
            crate::coinjoin::p2pkh_script_sig(&sig, SIGHASH_ALL, &pubkey);
    }
    if !signed_only_own_inputs(tx, &signed, own) {
        return Err(funding_refusal("not_own_inputs"));
    }
    Ok(signed)
}

/// `signedOnlyOwnInputs`: the copies equal but for scriptSigs, and only
/// this wallet's inputs gained one, each of them.
pub(crate) fn signed_only_own_inputs(
    before: &SpecialTx,
    after: &SpecialTx,
    own: &[OwnInput],
) -> bool {
    let bare = |tx: &SpecialTx| {
        let mut tx = tx.clone();
        tx.input
            .iter_mut()
            .for_each(|i| i.script_sig = ScriptBuf::new());
        tx
    };
    bare(before) == bare(after)
        && before.input.iter().zip(&after.input).all(|(b, a)| {
            if own.iter().any(|(o, _, _)| *o == b.previous_output) {
                b.script_sig.is_empty() && !a.script_sig.is_empty()
            } else {
                b.script_sig == a.script_sig
            }
        })
}

/// Checks a request's share table before anything is picked (§5's shape).
pub(crate) fn check_slots(count: usize) -> Result<(), MasternodesError> {
    if count < dw_protx::params::MIN_SHARES as usize
        || count > dw_protx::params::MAX_SHARES as usize
    {
        return Err(invalid("a shared masternode has 2 to 8 shares"));
    }
    Ok(())
}

/// Checks the coordinator's registration fee (D9) before anything is picked.
pub(crate) fn check_fee(fee: u64) -> Result<(), MasternodesError> {
    if fee == 0 || fee > crate::send::MAX_TX_FEE {
        return Err(invalid("the fee is out of range"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_126_services_follow_d5() {
        assert_eq!(
            coordinator_services("1.2.3.4:9999, 5.6.7.8:9999")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(coordinator_services(""), Err(ServiceProblem::Invalid));
        assert_eq!(coordinator_services("1.2.3"), Err(ServiceProblem::Invalid));
        for kind in ["[::1]:9999", "example.com:9999", "abc.onion:9999"] {
            assert_eq!(
                coordinator_services(kind),
                Err(ServiceProblem::Unsupported),
                "{kind}"
            );
        }
        assert!(participant_services_ok(""));
        assert!(participant_services_ok("1.2.3.4:9999"));
        assert!(participant_services_ok("[2001:db8::1]:9999"));
        assert!(!participant_services_ok("[fc00::1]:9999"));
        assert!(!participant_services_ok("example.com:9999"));
        // Core's forms of 1.2.3.4 (M9): read as dash-qt reads them.
        for form in ["1.2.772:9999", "[::ffff:1.2.3.4]:9999", "[1.2.3.4]:+9999"] {
            assert!(participant_services_ok(form), "{form}");
            assert_eq!(net_info_of(form), net_info_of("1.2.3.4:9999"), "{form}");
            assert!(coordinator_services(form).is_err(), "{form}");
        }
    }

    /// fixes-1a M8: entries split as `NetInfoMatches`' `[,\s]+` without
    /// Unicode properties: a no-break space joins two entries into one
    /// that doesn't parse; an ASCII vertical tab separates them.
    #[test]
    fn test_QT_126_services_split_on_ascii_space_only() {
        let joined = "1.2.3.4:9999\u{a0}5.6.7.8:9999";
        assert!(coordinator_services(joined).is_err());
        assert!(!participant_services_ok(joined));
        assert_eq!(net_info_of(joined), None);
        let split = "1.2.3.4:9999\x0b5.6.7.8:9999";
        assert_eq!(coordinator_services(split).unwrap().len(), 2);
        assert!(participant_services_ok(split));
        assert!(net_info_of(split).is_some());
    }
}
