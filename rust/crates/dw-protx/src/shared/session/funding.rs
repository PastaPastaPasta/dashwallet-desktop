//! The funding template and the frozen registration (MG-04b §2.4): what
//! `contributions` builds (`BuildFundingParts`, `rebuildFundingTx`,
//! rc.3 `mnsharesession.cpp:129-158,802-813`), the shared ProRegTx a session
//! carries past `draft` (`DecodeSharedProTx`, `decodeFrozen`), and the check
//! that it registers exactly what the envelope displays
//! (`payloadMatchesEnvelope`, `:982-1086`).

use std::net::SocketAddr;
use std::str::FromStr;

use dashcore::address::{NetworkUnchecked, Payload};
use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint, ScriptBuf, TxIn, TxOut, Txid};

use super::super::envelope::{EnvelopeError, decode_base64, decode_tx_hex, is_hex};
use super::super::netinfo::{NetInfoStatus, PURPOSE_CORE_P2P, ServiceAddr, validate_service};
use super::super::registration::{
    MN_TYPE_REGULAR, SHARED_COLLATERAL_SCRIPT, TRANSACTION_PROVIDER_REGISTER,
};
use super::super::rules::verify_hash_canonical;
use super::super::{ExtNetInfo, ProRegTxV3, SpecialTx};
use super::{Contribution, Session, Stage};
use crate::bls::is_valid_basic_public;
use crate::params::MASTERNODE_COLLATERAL;

/// `IsTxidHex`: 64 hex digits, either case.
pub fn is_txid_hex(txid: &str) -> bool {
    txid.len() == 64 && is_hex(txid)
}

/// `DecodeDestination` on `network`: a base58 P2PKH or P2SH address.
pub fn decode_destination(address: &str, network: Network) -> Option<Address> {
    let address = Address::<NetworkUnchecked>::from_str(address)
        .ok()?
        .require_network(network)
        .ok()?;
    matches!(
        address.payload(),
        Payload::PubkeyHash(_) | Payload::ScriptHash(_)
    )
    .then_some(address)
}

/// `GetScriptForDestination(DecodeDestination(address))`: empty for an
/// address that doesn't decode (`CNoDestination`).
pub fn script_for(address: &str, network: Network) -> ScriptBuf {
    decode_destination(address, network).map_or_else(ScriptBuf::new, |a| a.script_pubkey())
}

/// The key id of a P2PKH address (`std::get_if<PKHash>`).
pub fn key_id_of(address: &str, network: Network) -> Option<[u8; 20]> {
    match decode_destination(address, network)?.payload() {
        Payload::PubkeyHash(h) => Some(h.to_byte_array()),
        _ => None,
    }
}

/// Why `contributions` doesn't build (`BuildFundingParts`' errors).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FundingPartsError {
    #[error("contribution {0} has a malformed input transaction id")]
    Txid(usize),
    #[error("contribution {0} has an invalid change address")]
    ChangeAddress(usize),
}

/// `BuildFundingParts`: every contribution's inputs, in order, then each
/// contribution's change output, in the same order.
pub fn funding_parts(
    contributions: &[Contribution],
    network: Network,
) -> Result<(Vec<TxIn>, Vec<TxOut>), FundingPartsError> {
    let mut vin = Vec::new();
    for (i, c) in contributions.iter().enumerate() {
        for input in &c.inputs {
            let txid = is_txid_hex(&input.txid)
                .then(|| Txid::from_str(&input.txid).ok())
                .flatten()
                .ok_or(FundingPartsError::Txid(i))?;
            vin.push(TxIn {
                previous_output: OutPoint::new(txid, input.vout),
                script_sig: ScriptBuf::new(),
                sequence: input.sequence,
                witness: Default::default(),
            });
        }
    }
    let mut vout = Vec::new();
    for (i, c) in contributions.iter().enumerate() {
        let Some(change) = &c.change else { continue };
        let dest = decode_destination(&change.address, network)
            .ok_or(FundingPartsError::ChangeAddress(i))?;
        vout.push(TxOut {
            value: change.amount as u64,
            script_pubkey: dest.script_pubkey(),
        });
    }
    Ok((vin, vout))
}

/// `rebuildFundingTx`: `nVersion` 2, type 0, lock time 0, the parts of
/// [`funding_parts`]; refreshes every `changeIndex`.
pub fn rebuild_funding_tx(session: &mut Session) -> Result<(), FundingPartsError> {
    let (input, output) = funding_parts(&session.contributions, session.network)?;
    let mut next = 0;
    for c in &mut session.contributions {
        c.change_index = if c.change.is_some() {
            next += 1;
            next - 1
        } else {
            -1
        };
    }
    let tx = SpecialTx {
        version: 2,
        tx_type: 0,
        input,
        output,
        lock_time: 0,
        payload: Vec::new(),
    };
    session.funding_tx = hex::encode(tx.to_bytes());
    Ok(())
}

/// `DecodeSharedProTx`: hex of a ProRegTx whose payload decodes and has a
/// share table. The pin can't decode a version-3 payload, so anything else
/// (a version ≤ 2 ProRegTx, which is never shared) is refused here too.
pub fn decode_shared_protx(protx_hex: &str) -> Option<(SpecialTx, ProRegTxV3)> {
    let tx = decode_tx_hex(protx_hex)?;
    if tx.tx_type != TRANSACTION_PROVIDER_REGISTER {
        return None;
    }
    let payload = ProRegTxV3::from_payload(&tx.payload).ok()?;
    payload.is_shared().then_some((tx, payload))
}

/// A session's frozen registration, decoded and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub tx: SpecialTx,
    pub payload: ProRegTxV3,
    /// `MakeSharedRegConsentHash`, internal byte order.
    pub consent_hash: [u8; 32],
}

/// `decodeFrozen`: [`decode_shared_protx`] whose consent digest is the
/// session's `consentHash`.
pub fn decode_frozen(session: &Session) -> Result<Frozen, EnvelopeError> {
    let (tx, payload) = decode_shared_protx(&session.protx).ok_or(EnvelopeError::Parse("protx"))?;
    let consent_hash = payload.consent_hash(&tx);
    if ProRegTxV3::display_hash(&consent_hash) != session.consent_hash {
        return Err(EnvelopeError::Parse("consentHash"));
    }
    Ok(Frozen {
        tx,
        payload,
        consent_hash,
    })
}

/// A separator of `NetInfoMatches`' `[,\s]+` (`mnsharesession.cpp:166`): a
/// comma, or what PCRE2's `\s` matches without Unicode properties (the
/// expression sets no `UseUnicodePropertiesOption`): HT, LF, VT, FF, CR and
/// space. A no-break space or any other non-ASCII space is part of an entry.
pub fn is_service_separator(c: char) -> bool {
    matches!(c, ',' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

/// The entries of a `coreP2PAddrs` as `NetInfoMatches` splits them, empty
/// ones skipped (`Qt::SkipEmptyParts`).
pub fn service_entries(addrs: &str) -> impl Iterator<Item = &str> {
    addrs.split(is_service_separator).filter(|e| !e.is_empty())
}

/// `NetInfoMatches` (`mnsharesession.cpp:160-170`) on `network`: the
/// entries of `coreP2PAddrs` ([`service_entries`]), each added as a Core
/// P2P service, give exactly the payload's net info. An entry Core's
/// `ExtNetInfo::AddEntry` (rc.3 `evo/netinfo.cpp:500-551`) would refuse
/// never matches: its `ValidateService` ([`validate_service`]: a valid,
/// known-network address, routable where `network` requires it, a port
/// that isn't zero or bad), a repeated address (`IsAddrDuplicate`), or more
/// than `MAX_ENTRIES_EXTNETINFO` (`ProcessCandidate`); and one that isn't a
/// numeric `ip:port` (MG-04b D5: no domains or privacy networks).
pub fn net_info_matches(net_info: &ExtNetInfo, addrs: &str, network: Network) -> bool {
    let entries: Option<Vec<SocketAddr>> = service_entries(addrs).map(|e| e.parse().ok()).collect();
    let Some(entries) = entries else {
        return false;
    };
    let services_valid = entries.iter().all(|a| {
        ServiceAddr::from_socket_addr(*a).is_some_and(|s| {
            validate_service(PURPOSE_CORE_P2P, &s, network) == NetInfoStatus::Success
        })
    });
    if !services_valid {
        return false;
    }
    // `IsAddrDuplicate`: one address once per purpose, whatever the port.
    if entries
        .iter()
        .enumerate()
        .any(|(i, a)| entries[..i].iter().any(|b| b.ip() == a.ip()))
    {
        return false;
    }
    ExtNetInfo::core_p2p(&entries).is_some_and(|expected| &expected == net_info)
}

/// `pubKeyOperator.Get().ToString(false)` (`mnsharesession.cpp:1023`): the
/// key's hex when Core reads it as valid ([`is_valid_basic_public`]), else
/// the hex of Core's invalid object, 48 zero bytes (`ToBytes`, rc.3
/// `bls/bls.h:125-131`). A payload key that isn't a point never shows as
/// its own bytes.
fn operator_key_string(key: &[u8; 48]) -> String {
    if is_valid_basic_public(key) {
        hex::encode(key)
    } else {
        hex::encode([0u8; 48])
    }
}

/// `payloadMatchesEnvelope`: the frozen registration's share table, terms
/// and funding are exactly the envelope's.
pub fn payload_matches_envelope(session: &Session, frozen: &Frozen) -> bool {
    let (tx, payload, network) = (&frozen.tx, &frozen.payload, session.network);
    if payload.shares.len() != session.shares.len() {
        return false;
    }
    for (share, signed) in session.shares.iter().zip(&payload.shares) {
        let reward = if share.reward_address.is_empty() {
            ScriptBuf::new()
        } else {
            script_for(&share.reward_address, network)
        };
        if key_id_of(&share.owner_address, network) != Some(signed.owner_key_id)
            || u64::try_from(share.amount) != Ok(signed.amount)
            || script_for(&share.refund_address, network) != signed.refund_script
            || reward != signed.reward_script
        {
            return false;
        }
    }

    let t = &session.terms;
    if key_id_of(&t.voting_address, network) != Some(payload.voting_key_id)
        || !operator_key_string(&payload.operator_public_key)
            .eq_ignore_ascii_case(&t.operator_pub_key)
        || t.operator_reward != i32::from(payload.operator_reward)
        || t.early_period_blocks != payload.early_period_blocks
        || t.early_penalty != payload.early_penalty
        || !net_info_matches(&payload.net_info, &t.core_p2p_addrs, network)
    {
        return false;
    }

    let Ok((expected_vin, mut expected_vout)) = funding_parts(&session.contributions, network)
    else {
        return false;
    };
    if usize::try_from(session.collateral_index) != Ok(expected_vout.len())
        || u32::try_from(session.collateral_index) != Ok(payload.collateral.vout)
        || payload.mn_type != MN_TYPE_REGULAR
    {
        return false;
    }
    expected_vout.push(TxOut {
        value: MASTERNODE_COLLATERAL,
        script_pubkey: ScriptBuf::from_bytes(SHARED_COLLATERAL_SCRIPT.to_vec()),
    });
    tx.input.len() == expected_vin.len()
        && tx.output.len() == expected_vout.len()
        && tx.lock_time == 0
        && tx
            .input
            .iter()
            .zip(&expected_vin)
            .all(|(a, b)| a.previous_output == b.previous_output && a.sequence == b.sequence)
        && tx.output.iter().zip(&expected_vout).all(|(a, b)| a == b)
}

/// `verifySignature` once the frozen registration has been checked: base64
/// of a canonical compact signature over the consent digest by the share's
/// owner key.
pub fn verify_consent_signature(frozen: &Frozen, share_index: i32, signature: &str) -> bool {
    let Some(share) = usize::try_from(share_index)
        .ok()
        .and_then(|i| frozen.payload.shares.get(i))
    else {
        return false;
    };
    decode_base64(signature)
        .is_some_and(|sig| verify_hash_canonical(frozen.consent_hash, share.owner_key_id, &sig))
}

/// `fromJson`'s checks past `draft` (`mnsharesession.cpp:612-655`), in its
/// order: the transaction and consent hash present; [`decode_frozen`];
/// [`payload_matches_envelope`]; every signature's share in range; from
/// `combined`, a signature per share, each verifying; from
/// `fundingSigned`, every input signed.
pub fn check_frozen(session: &Session) -> Result<Frozen, EnvelopeError> {
    use EnvelopeError::Parse;
    if session.protx.is_empty() || session.consent_hash.is_empty() {
        return Err(Parse("frozen"));
    }
    let frozen = decode_frozen(session)?;
    if !payload_matches_envelope(session, &frozen) {
        return Err(Parse("protx"));
    }
    if session.signed_count() != session.sigs.len() {
        return Err(Parse("sigs"));
    }
    if session.stage >= Stage::Combined {
        if session.signed_count() != session.shares.len() {
            return Err(Parse("sigs"));
        }
        for sig in &session.sigs {
            if !verify_consent_signature(&frozen, sig.share_index, &sig.signature) {
                return Err(EnvelopeError::BadSignature {
                    share: sig.share_index as u16,
                });
            }
        }
    }
    if session.stage >= Stage::FundingSigned
        && frozen.tx.input.iter().any(|i| i.script_sig.is_empty())
    {
        return Err(Parse("scriptSig"));
    }
    Ok(frozen)
}
