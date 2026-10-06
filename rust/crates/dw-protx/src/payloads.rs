//! The four classic provider payloads (ProRegTx, ProUpServTx, ProUpRegTx,
//! ProUpRevTx) at version 2 (basic BLS), as Dash Core v24 builds them before
//! the v24 extended-address fork (`src/evo/providertx.h:129-434`,
//! `src/rpc/evo.cpp` `protx_register_common_wrapper`, `protx_update_*`,
//! `protx_revoke`).
//!
//! Every payload commits to the funding inputs (`inputsHash`) and most are
//! signed over that commitment, so a build has two steps: a *placeholder*
//! with the selection-dependent fields zeroed (coin selection prices its
//! size into the fee), then a *finalizer* that receives the assembled
//! unsigned transaction and fills `inputsHash` and the signature. The
//! funding inputs are signed afterwards, since their sighash covers the
//! finished payload. key-wallet's `TransactionBuilder::set_payload_finalizer`
//! runs the finalizer between selection and input signing.
//!
//! Signatures (Dash Core `src/evo/specialtx.h`, `src/rpc/evo.cpp`):
//! - ProRegTx with an existing collateral: the collateral key signs the
//!   *message* `payout|reward|owner|voting|payloadHash` (`MakeSignString`,
//!   `providertx.cpp:375`) with `signmessage`'s format; with a collateral the
//!   transaction itself creates, the signature stays empty.
//! - ProUpRegTx: the owner key signs the payload hash
//!   (`CHashSigner::SignHash`, 65-byte compact).
//! - ProUpServTx, ProUpRevTx: the operator key signs the payload hash
//!   (basic BLS scheme).

use std::net::SocketAddr;

use dashcore::blockdata::transaction::special_transaction::provider_registration::{
    ProviderMasternodeType, ProviderRegistrationPayload,
};
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::blockdata::transaction::special_transaction::provider_update_revocation::ProviderUpdateRevocationPayload;
use dashcore::blockdata::transaction::special_transaction::provider_update_service::ProviderUpdateServicePayload;
use dashcore::blockdata::transaction::special_transaction::{
    SpecialTransactionBasePayloadEncodable, TransactionPayload,
};
use dashcore::bls_sig_utils::{BLSPublicKey, BLSSignature};
use dashcore::hash_types::InputsHash;
use dashcore::hashes::Hash;
use dashcore::platform_node_id::PlatformNodeId;
use dashcore::secp256k1::{Message, Secp256k1, SecretKey};
use dashcore::{Address, Network, OutPoint, PubkeyHash, ScriptBuf, Transaction, Txid};
use zeroize::Zeroizing;

use crate::bls;

/// What the collateral of a registration is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollateralSource {
    /// The registration pays the collateral to `script` itself
    /// (`protx register_fund`): the payload names the output by index.
    FundNew { script: ScriptBuf, amount: u64 },
    /// An existing unspent output (`protx register`).
    Existing(OutPoint),
}

/// Platform fields of a version-2 EvoNode payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformPorts {
    pub node_id: [u8; 20],
    pub p2p_port: u16,
    pub http_port: u16,
}

/// The answers a registration is built from (all checked by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationTerms {
    pub evo: bool,
    pub collateral: CollateralSource,
    pub service: SocketAddr,
    pub owner_key_hash: [u8; 20],
    pub voting_key_hash: [u8; 20],
    pub operator_public_key: [u8; 48],
    /// Hundredths of a percent.
    pub operator_reward: u16,
    pub payout_script: ScriptBuf,
    pub platform: Option<PlatformPorts>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    #[error("the assembled transaction has no {0} payload")]
    MissingPlaceholder(&'static str),
    #[error("the assembled transaction has no outputs")]
    NoOutputs,
    #[error("the collateral output of {amount} duffs is missing")]
    CollateralOutputMissing { amount: u64 },
    #[error("not a valid secp256k1 secret")]
    BadSecret,
    #[error(transparent)]
    Bls(#[from] bls::BlsError),
    #[error("payout script cannot be shown as an address")]
    PayoutNotAddress,
}

/// A zero 32-byte hash (`uint256()`), the txid of a collateral the
/// registration itself creates.
fn null_txid() -> Txid {
    Txid::all_zeros()
}

/// The ProRegTx placeholder: final except `inputsHash`, the signature and,
/// for a fund-new collateral, the output index.
pub fn registration_placeholder(terms: &RegistrationTerms) -> ProviderRegistrationPayload {
    let collateral_outpoint = match &terms.collateral {
        CollateralSource::FundNew { .. } => OutPoint {
            txid: null_txid(),
            vout: 0,
        },
        CollateralSource::Existing(outpoint) => *outpoint,
    };
    let (node_id, p2p, http) = match terms.platform {
        Some(p) if terms.evo => (
            Some(PlatformNodeId::from_byte_array(p.node_id)),
            Some(p.p2p_port),
            Some(p.http_port),
        ),
        _ => (None, None, None),
    };
    ProviderRegistrationPayload {
        version: ProviderRegistrationPayload::CURRENT_VERSION,
        masternode_type: if terms.evo {
            ProviderMasternodeType::HighPerformance
        } else {
            ProviderMasternodeType::Regular
        },
        masternode_mode: 0,
        collateral_outpoint,
        service_address: terms.service,
        owner_key_hash: PubkeyHash::from_byte_array(terms.owner_key_hash),
        operator_public_key: BLSPublicKey::from(terms.operator_public_key),
        voting_key_hash: PubkeyHash::from_byte_array(terms.voting_key_hash),
        operator_reward: terms.operator_reward,
        script_payout: terms.payout_script.clone(),
        inputs_hash: InputsHash::all_zeros(),
        signature: Vec::new(),
        platform_node_id: node_id,
        platform_p2p_port: p2p,
        platform_http_port: http,
    }
}

/// How the finalizer signs a registration.
pub enum RegistrationSigner {
    /// Fund-new collateral: find the output, leave the signature empty.
    FundNew { script: ScriptBuf, amount: u64 },
    /// Existing collateral: sign the message with its key.
    Collateral {
        secret: Zeroizing<[u8; 32]>,
        network: Network,
    },
}

/// The ProRegTx finalizer: commits to the inputs, names a fund-new
/// collateral output by its index after BIP69 sorting, and signs an
/// existing collateral's message.
pub fn finalize_registration(
    unsigned: &Transaction,
    signer: &RegistrationSigner,
) -> Result<TransactionPayload, PayloadError> {
    if unsigned.output.is_empty() {
        return Err(PayloadError::NoOutputs);
    }
    let Some(TransactionPayload::ProviderRegistrationPayloadType(placeholder)) =
        &unsigned.special_transaction_payload
    else {
        return Err(PayloadError::MissingPlaceholder("ProRegTx"));
    };
    let mut payload = placeholder.clone();
    payload.inputs_hash = unsigned.hash_inputs();
    match signer {
        RegistrationSigner::FundNew { script, amount } => {
            let index = unsigned
                .output
                .iter()
                .position(|o| &o.script_pubkey == script && o.value == *amount)
                .ok_or(PayloadError::CollateralOutputMissing { amount: *amount })?;
            payload.collateral_outpoint = OutPoint {
                txid: null_txid(),
                vout: index as u32,
            };
            payload.signature = Vec::new();
        }
        RegistrationSigner::Collateral { secret, network } => {
            let message = registration_sign_message(&payload, *network)?;
            payload.signature = sign_message_compact(secret, &message)?.to_vec();
        }
    }
    Ok(TransactionPayload::ProviderRegistrationPayloadType(payload))
}

/// `CProRegTx::MakeSignString`: what the collateral key signs
/// (`providertx.cpp:375-395`).
pub fn registration_sign_message(
    payload: &ProviderRegistrationPayload,
    network: Network,
) -> Result<String, PayloadError> {
    payload
        .payload_collateral_string(network)
        .map_err(|_| PayloadError::PayoutNotAddress)
}

/// `signmessage` with a raw secp256k1 secret: the 65-byte compact
/// signature over the `DarkCoin Signed Message:\n` hash, compressed key.
pub fn sign_message_compact(secret: &[u8; 32], message: &str) -> Result<[u8; 65], PayloadError> {
    let hash = dashcore::sign_message::signed_msg_hash(message);
    sign_hash_compact(secret, hash.as_byte_array())
}

/// `CKey::SignCompact` over a 32-byte hash (internal byte order), for a
/// compressed public key: `27 + 4 + recid ‖ r ‖ s`.
pub fn sign_hash_compact(secret: &[u8; 32], hash: &[u8; 32]) -> Result<[u8; 65], PayloadError> {
    let secp = Secp256k1::signing_only();
    let key = SecretKey::from_slice(secret).map_err(|_| PayloadError::BadSecret)?;
    let sig = secp.sign_ecdsa_recoverable(&Message::from_digest(*hash), &key);
    Ok(dashcore::sign_message::MessageSignature::new(sig, true).serialize())
}

/// The ProUpServTx placeholder (version 2) for a new service.
pub fn update_service_placeholder(
    pro_tx_hash: Txid,
    service: SocketAddr,
    operator_payout: ScriptBuf,
    platform: Option<PlatformPorts>,
) -> ProviderUpdateServicePayload {
    let (mn_type, node_id, p2p, http) = match platform {
        Some(p) => (
            ProviderMasternodeType::HighPerformance as u16,
            Some(PlatformNodeId::from_byte_array(p.node_id)),
            Some(p.p2p_port),
            Some(p.http_port),
        ),
        None => (ProviderMasternodeType::Regular as u16, None, None, None),
    };
    let (ip_address, port) = service_payload_fields(service);
    ProviderUpdateServicePayload::new(
        Some(mn_type),
        pro_tx_hash,
        ip_address,
        port,
        operator_payout,
        InputsHash::all_zeros(),
        node_id,
        p2p,
        http,
        BLSSignature::from([0u8; 96]),
    )
}

/// A service as the ProUpServTx payload encodes it: the IPv6 (or
/// IPv4-mapped) octets as a little-endian `u128` and the port in host order
/// (platform-wallet `service_payload_fields`, checked against dashcore's
/// testnet vector).
pub fn service_payload_fields(service: SocketAddr) -> (u128, u16) {
    let octets = match service.ip() {
        std::net::IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        std::net::IpAddr::V6(v6) => v6.octets(),
    };
    (u128::from_le_bytes(octets), service.port())
}

/// The ProUpServTx finalizer: inputs hash, then the operator signature.
pub fn finalize_update_service(
    unsigned: &Transaction,
    operator_secret: &[u8; 32],
) -> Result<TransactionPayload, PayloadError> {
    if unsigned.output.is_empty() {
        return Err(PayloadError::NoOutputs);
    }
    let Some(TransactionPayload::ProviderUpdateServicePayloadType(placeholder)) =
        &unsigned.special_transaction_payload
    else {
        return Err(PayloadError::MissingPlaceholder("ProUpServTx"));
    };
    let mut payload = placeholder.clone();
    payload.inputs_hash = unsigned.hash_inputs();
    let sig = bls::sign_hash(operator_secret, payload.base_payload_hash().as_byte_array())?;
    payload.payload_sig = BLSSignature::from(sig);
    Ok(TransactionPayload::ProviderUpdateServicePayloadType(
        payload,
    ))
}

/// The ProUpRegTx placeholder (version 2). Unchanged fields carry the
/// masternode's current values (Core copies them from the list entry).
pub fn update_registrar_placeholder(
    pro_tx_hash: Txid,
    operator_public_key: [u8; 48],
    voting_key_hash: [u8; 20],
    payout_script: ScriptBuf,
) -> ProviderUpdateRegistrarPayload {
    ProviderUpdateRegistrarPayload {
        version: ProviderUpdateRegistrarPayload::CURRENT_VERSION,
        pro_tx_hash,
        provider_mode: 0,
        operator_public_key: BLSPublicKey::from(operator_public_key),
        voting_key_hash: PubkeyHash::from_byte_array(voting_key_hash),
        script_payout: payout_script,
        inputs_hash: InputsHash::all_zeros(),
        payload_sig: Vec::new(),
    }
}

/// The ProUpRegTx finalizer: inputs hash, then the owner key's compact
/// signature over the payload hash.
pub fn finalize_update_registrar(
    unsigned: &Transaction,
    owner_secret: &[u8; 32],
) -> Result<TransactionPayload, PayloadError> {
    if unsigned.output.is_empty() {
        return Err(PayloadError::NoOutputs);
    }
    let Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(placeholder)) =
        &unsigned.special_transaction_payload
    else {
        return Err(PayloadError::MissingPlaceholder("ProUpRegTx"));
    };
    let mut payload = placeholder.clone();
    payload.inputs_hash = unsigned.hash_inputs();
    let hash = payload.base_payload_hash();
    payload.payload_sig = sign_hash_compact(owner_secret, hash.as_byte_array())?.to_vec();
    Ok(TransactionPayload::ProviderUpdateRegistrarPayloadType(
        payload,
    ))
}

/// The ProUpRevTx placeholder (version 2), reason 0–3.
pub fn revoke_placeholder(pro_tx_hash: Txid, reason: u16) -> ProviderUpdateRevocationPayload {
    ProviderUpdateRevocationPayload {
        version: ProviderUpdateRevocationPayload::CURRENT_VERSION,
        pro_tx_hash,
        reason,
        inputs_hash: InputsHash::all_zeros(),
        payload_sig: BLSSignature::from([0u8; 96]),
    }
}

/// The ProUpRevTx finalizer: inputs hash, then the operator signature.
pub fn finalize_revoke(
    unsigned: &Transaction,
    operator_secret: &[u8; 32],
) -> Result<TransactionPayload, PayloadError> {
    if unsigned.output.is_empty() {
        return Err(PayloadError::NoOutputs);
    }
    let Some(TransactionPayload::ProviderUpdateRevocationPayloadType(placeholder)) =
        &unsigned.special_transaction_payload
    else {
        return Err(PayloadError::MissingPlaceholder("ProUpRevTx"));
    };
    let mut payload = placeholder.clone();
    payload.inputs_hash = unsigned.hash_inputs();
    let sig = bls::sign_hash(operator_secret, payload.base_payload_hash().as_byte_array())?;
    payload.payload_sig = BLSSignature::from(sig);
    Ok(TransactionPayload::ProviderUpdateRevocationPayloadType(
        payload,
    ))
}

/// The payout script of an address the payout field accepts: P2PKH or P2SH
/// (`IsValidPayoutScript`).
pub fn payout_script(address: &Address) -> ScriptBuf {
    address.script_pubkey()
}

/// The ProTx hash of a fund-new registration's collateral: the
/// registration's own txid (`COutPoint(tx.GetHash(), n)`).
pub fn resolve_collateral(pro_tx_hash: Txid, payload_outpoint: OutPoint) -> OutPoint {
    if payload_outpoint.txid == null_txid() {
        OutPoint {
            txid: pro_tx_hash,
            vout: payload_outpoint.vout,
        }
    } else {
        payload_outpoint
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::blockdata::transaction::special_transaction::TransactionType;
    use dashcore::secp256k1::PublicKey;
    use dashcore::sign_message::MessageSignature;
    use dashcore::{TxIn, TxOut};

    fn addr(seed: u8) -> Address {
        Address::new(
            Network::Testnet,
            dashcore::address::Payload::PubkeyHash(PubkeyHash::from_byte_array([seed; 20])),
        )
    }

    fn tx_with(payload: TransactionPayload, outputs: Vec<TxOut>) -> Transaction {
        Transaction {
            version: 3,
            lock_time: 0,
            input: vec![
                TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([9; 32]),
                        vout: 1,
                    },
                    ..Default::default()
                },
                TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([3; 32]),
                        vout: 0,
                    },
                    ..Default::default()
                },
            ],
            output: outputs,
            special_transaction_payload: Some(payload),
        }
    }

    fn terms(collateral: CollateralSource) -> RegistrationTerms {
        let (basic, _) = bls::public_keys(&[7u8; 32]).unwrap();
        RegistrationTerms {
            evo: false,
            collateral,
            service: "1.2.3.4:19999".parse().unwrap(),
            owner_key_hash: [1; 20],
            voting_key_hash: [2; 20],
            operator_public_key: basic,
            operator_reward: 0,
            payout_script: addr(3).script_pubkey(),
            platform: None,
        }
    }

    #[test]
    fn test_QT_123_fund_new_names_the_collateral_output_after_sorting() {
        let collateral = addr(5).script_pubkey();
        let amount = crate::params::MASTERNODE_COLLATERAL;
        let t = terms(CollateralSource::FundNew {
            script: collateral.clone(),
            amount,
        });
        let unsigned = tx_with(
            TransactionPayload::ProviderRegistrationPayloadType(registration_placeholder(&t)),
            vec![
                TxOut {
                    value: 5_000,
                    script_pubkey: addr(6).script_pubkey(),
                },
                TxOut {
                    value: amount,
                    script_pubkey: collateral.clone(),
                },
            ],
        );
        let TransactionPayload::ProviderRegistrationPayloadType(p) = finalize_registration(
            &unsigned,
            &RegistrationSigner::FundNew {
                script: collateral,
                amount,
            },
        )
        .unwrap() else {
            panic!("ProRegTx payload expected");
        };
        assert_eq!(p.collateral_outpoint.vout, 1);
        assert_eq!(p.collateral_outpoint.txid, Txid::all_zeros());
        assert_eq!(p.inputs_hash, unsigned.hash_inputs());
        assert!(p.signature.is_empty());
        assert_eq!(p.version, 2);
        let mut tx = unsigned.clone();
        tx.special_transaction_payload = Some(TransactionPayload::ProviderRegistrationPayloadType(
            p.clone(),
        ));
        assert_eq!(tx.tx_type(), TransactionType::ProviderRegistration);
        assert_eq!(
            resolve_collateral(tx.txid(), p.collateral_outpoint),
            OutPoint {
                txid: tx.txid(),
                vout: 1
            }
        );
    }

    #[test]
    fn test_QT_123_existing_collateral_signs_the_core_sign_string() {
        let secret = Zeroizing::new([0x11u8; 32]);
        let secp = Secp256k1::new();
        let pk = PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&secret[..]).unwrap());
        let collateral_address = Address::p2pkh(&dashcore::PublicKey::new(pk), Network::Testnet);
        let t = terms(CollateralSource::Existing(OutPoint {
            txid: Txid::from_byte_array([5; 32]),
            vout: 2,
        }));
        let unsigned = tx_with(
            TransactionPayload::ProviderRegistrationPayloadType(registration_placeholder(&t)),
            vec![TxOut {
                value: 1_000,
                script_pubkey: addr(6).script_pubkey(),
            }],
        );
        let TransactionPayload::ProviderRegistrationPayloadType(p) = finalize_registration(
            &unsigned,
            &RegistrationSigner::Collateral {
                secret: secret.clone(),
                network: Network::Testnet,
            },
        )
        .unwrap() else {
            panic!("ProRegTx payload expected");
        };
        let message = registration_sign_message(&p, Network::Testnet).unwrap();
        let parts: Vec<&str> = message.split('|').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[1], "0");
        assert_eq!(parts[0], addr(3).to_string());
        let sig = MessageSignature::from_slice(&p.signature).unwrap();
        let hash = dashcore::sign_message::signed_msg_hash(&message);
        assert!(
            sig.is_signed_by_address(&secp, &collateral_address, hash)
                .unwrap()
        );
    }

    #[test]
    fn test_QT_125_update_registrar_is_signed_by_the_owner_key() {
        let secret = [0x22u8; 32];
        let (basic, _) = bls::public_keys(&[7u8; 32]).unwrap();
        let placeholder = update_registrar_placeholder(
            Txid::from_byte_array([4; 32]),
            basic,
            [2; 20],
            ScriptBuf::new(),
        );
        let unsigned = tx_with(
            TransactionPayload::ProviderUpdateRegistrarPayloadType(placeholder),
            vec![TxOut::default()],
        );
        let TransactionPayload::ProviderUpdateRegistrarPayloadType(p) =
            finalize_update_registrar(&unsigned, &secret).unwrap()
        else {
            panic!("ProUpRegTx payload expected");
        };
        assert_eq!(p.payload_sig.len(), 65);
        let sig = MessageSignature::from_slice(&p.payload_sig).unwrap();
        assert!(sig.compressed);
        let secp = Secp256k1::new();
        let pubkey = secp
            .recover_ecdsa(
                &Message::from_digest(*p.base_payload_hash().as_byte_array()),
                &sig.signature,
            )
            .unwrap();
        assert_eq!(
            pubkey,
            PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&secret).unwrap())
        );
    }

    #[test]
    fn test_QT_125_service_and_revoke_payloads_carry_operator_signatures() {
        let secret = [7u8; 32];
        let serv = update_service_placeholder(
            Txid::from_byte_array([4; 32]),
            "1.2.3.4:19999".parse().unwrap(),
            ScriptBuf::new(),
            None,
        );
        let unsigned = tx_with(
            TransactionPayload::ProviderUpdateServicePayloadType(serv),
            vec![TxOut::default()],
        );
        let TransactionPayload::ProviderUpdateServicePayloadType(p) =
            finalize_update_service(&unsigned, &secret).unwrap()
        else {
            panic!("ProUpServTx payload expected");
        };
        assert_eq!(p.inputs_hash, unsigned.hash_inputs());
        assert_ne!(p.payload_sig, BLSSignature::from([0u8; 96]));

        let rev = revoke_placeholder(Txid::from_byte_array([4; 32]), 3);
        let unsigned = tx_with(
            TransactionPayload::ProviderUpdateRevocationPayloadType(rev),
            vec![TxOut::default()],
        );
        let TransactionPayload::ProviderUpdateRevocationPayloadType(p) =
            finalize_revoke(&unsigned, &secret).unwrap()
        else {
            panic!("ProUpRevTx payload expected");
        };
        assert_eq!(p.reason, 3);
        assert_eq!(p.inputs_hash, unsigned.hash_inputs());
        assert!(matches!(
            finalize_revoke(
                &tx_with(
                    TransactionPayload::ProviderUpdateRevocationPayloadType(revoke_placeholder(
                        Txid::all_zeros(),
                        0
                    )),
                    vec![]
                ),
                &secret
            ),
            Err(PayloadError::NoOutputs)
        ));
    }
}
