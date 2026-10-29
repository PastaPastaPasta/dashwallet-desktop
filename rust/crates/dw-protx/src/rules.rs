//! Dash Core's checks of the owner-side provider payloads, as pure
//! functions returning Core's reject reason (v24.0.0-rc.3):
//!
//! - `CProRegTx::IsTriviallyValid` (`src/evo/providertx.cpp:265-333`) with
//!   `IsPayoutListTriviallyValid` (`:25-62`) for the one payout of a
//!   version-2 payload;
//! - `CheckProviderNetworkFields` (`:212-263`) and `CheckNetInfo`
//!   (`:187-210`) for the version-2 service and Platform fields;
//! - `CheckProRegTx` (`src/evo/specialtxman.cpp:1148-1310`): the collateral,
//!   `IsPayoutListKeySafe` (`providertx.cpp:146-164`), the duplicates
//!   against the masternode list, the inputs hash and the signature;
//! - `CProUpRegTx::IsTriviallyValid` (`providertx.cpp:459-477`) and
//!   `CheckProUpRegTx` (`specialtxman.cpp:1428-1528`).
//!
//! The engine runs them before it signs (MG-04 D5): Core v24 sends no
//! `reject` message, so a refusal on the network is silent. A check needs
//! only what the wallet can know: the payload, the transaction, the
//! collateral's script when the wallet has it, and the SPV list's entries
//! ([`ListedMasternode`]). Core's checks against its whole UTXO set and the
//! owner keys of other masternodes (which the SPV list doesn't carry) stay
//! Core's.

use std::net::{IpAddr, SocketAddr};

use dashcore::blockdata::transaction::special_transaction::SpecialTransactionBasePayloadEncodable;
use dashcore::blockdata::transaction::special_transaction::provider_registration::{
    ProviderMasternodeType, ProviderRegistrationPayload,
};
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::bls_sig_utils::{BlsPkBytes, BlsScheme};
use dashcore::hashes::Hash;
use dashcore::secp256k1::Message;
use dashcore::sign_message::{MessageSignature, signed_msg_hash};
use dashcore::sml::masternode_list_entry::MasternodeNetInfo;
use dashcore::sml::masternode_list_entry::net_info::{self as list_net_info, Bip155Network};
use dashcore::{Network, OutPoint, PubkeyHash, ScriptBuf, Transaction, Txid};

use crate::params::{MAX_OPERATOR_REWARD, collateral_amount};
use crate::service::{self, ServiceError};
use crate::shared::netinfo::{
    BIP155_CJDNS, BIP155_I2P, BIP155_IPV4, BIP155_IPV6, BIP155_TORV3, NetInfoEntry, ServiceAddr,
};

/// A reject reason, verbatim from Core (`bad-protx-…`).
pub type Reject = &'static str;

/// The provider-payload version the builders write: `ProTxVersion::BasicBLS`.
pub const BASIC_BLS: u16 = 2;
/// `ProTxVersion::ExtAddr`, the highest Core knows (`src/evo/types.h`).
pub const EXT_ADDR: u16 = 3;

/// What the SPV masternode list says about one entry: the fields Core's
/// uniqueness checks compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMasternode {
    /// Wire order.
    pub pro_tx_hash: [u8; 32],
    /// Every network-info entry the list holds for it, of every purpose
    /// ([`listed_net_info`]): each is a unique property of Core's list
    /// (`bad-protx-dup-netinfo-entry`, MG-04b D39).
    pub net_info: Vec<NetInfoEntry>,
    /// As the list serializes it (basic for version 2 and later, legacy
    /// for version 1).
    pub operator_public_key: [u8; 48],
    pub voting_key_id: [u8; 20],
    pub platform_node_id: Option<[u8; 20]>,
    /// The collateral, when the wallet knows it (only the wallet's own
    /// masternodes: the SPV list carries none).
    pub collateral: Option<OutPoint>,
}

/// A listed masternode's network-info entries as Core's list holds them,
/// each a unique property (rc.3 `deterministicmns.cpp:459-470`): a legacy
/// entry's one address, or a version-3 entry's every entry of every
/// purpose, services and domains, in wire order. A service is keyed as
/// `GetUniquePropertyHash` keys a `CService` (`deterministicmns.h:526-547`):
/// by network, address and port, an IPv4-mapped IPv6 address as its IPv4
/// one and an IPv6 address never flipped to CJDNS (a legacy entry's
/// `CService` is read as written). No entries for a legacy entry without a
/// service, or an entry dashcore couldn't read.
pub fn listed_net_info(info: &MasternodeNetInfo) -> Vec<NetInfoEntry> {
    match info {
        MasternodeNetInfo::Legacy(addr) => legacy_entry(*addr).into_iter().collect(),
        MasternodeNetInfo::Extended(ext) => ext
            .purposes
            .iter()
            .flat_map(|(_, entries)| entries)
            .filter_map(|e| match e {
                list_net_info::NetInfoEntry::Service {
                    network,
                    addr,
                    port,
                } => Some(NetInfoEntry::Service(ServiceAddr {
                    network: match network {
                        Bip155Network::Ipv4 => BIP155_IPV4,
                        Bip155Network::Ipv6 => BIP155_IPV6,
                        Bip155Network::TorV3 => BIP155_TORV3,
                        Bip155Network::I2p => BIP155_I2P,
                        Bip155Network::Cjdns => BIP155_CJDNS,
                    },
                    address: addr.clone(),
                    port: *port,
                })),
                list_net_info::NetInfoEntry::Domain { host, port } => Some(NetInfoEntry::Domain {
                    name: host.clone(),
                    port: *port,
                }),
                list_net_info::NetInfoEntry::Invalid => None,
            })
            .collect(),
    }
}

/// A legacy (`MnNetInfo`) address as the entry Core keys it by, `None` for
/// no service.
fn legacy_entry(addr: SocketAddr) -> Option<NetInfoEntry> {
    if service::is_no_service(&addr) {
        return None;
    }
    let (network, address) = match addr.ip().to_canonical() {
        IpAddr::V4(ip) => (BIP155_IPV4, ip.octets().to_vec()),
        IpAddr::V6(ip) => (BIP155_IPV6, ip.octets().to_vec()),
    };
    Some(NetInfoEntry::Service(ServiceAddr {
        network,
        address,
        port: addr.port(),
    }))
}

/// `bad-protx-dup-netinfo-entry` (rc.3 `specialtxman.cpp:1235-1245`): one
/// of `entries` is a unique property of a listed masternode on another
/// collateral, whichever purpose either side holds it under.
pub(crate) fn check_net_info_unique<'a>(
    entries: impl IntoIterator<Item = &'a NetInfoEntry>,
    collateral: OutPoint,
    list: &[ListedMasternode],
) -> Result<(), Reject> {
    for entry in entries {
        if list
            .iter()
            .any(|e| e.collateral != Some(collateral) && e.net_info.contains(entry))
        {
            return Err("bad-protx-dup-netinfo-entry");
        }
    }
    Ok(())
}

/// The hash160 a P2PKH script pays, `None` for any other script.
pub fn p2pkh_key_id(script: &ScriptBuf) -> Option<[u8; 20]> {
    let b = script.as_bytes();
    (b.len() == 25 && b[..3] == [0x76, 0xa9, 0x14] && b[23..] == [0x88, 0xac]).then(|| {
        let mut id = [0u8; 20];
        id.copy_from_slice(&b[3..23]);
        id
    })
}

/// `IsValidPayoutScript`: P2PKH or P2SH.
fn is_valid_payout_script(script: &ScriptBuf) -> bool {
    script.is_p2pkh() || script.is_p2sh()
}

/// `CBLSLazyPublicKey::Get().IsValid()` under `scheme`: a G1 point that
/// serializes back to the same bytes (`CheckMalleable`, rc.3
/// `bls/bls.h:211-219,492-511`).
pub(crate) fn is_valid_operator_key(key: &[u8; 48], scheme: BlsScheme) -> bool {
    BlsPkBytes::from_bytes(*key)
        .as_scheme(scheme)
        .canonicalize()
        .is_ok_and(|pk| pk.as_bytes() == key)
}

/// The scheme a payload of `version` serializes its operator key in.
fn operator_scheme(version: u16) -> BlsScheme {
    if version == 1 {
        BlsScheme::Legacy
    } else {
        BlsScheme::Modern
    }
}

/// The key in the other serialization, for Core's
/// `HasOperatorKeyUnderAnyScheme` (`deterministicmns.cpp`).
fn other_scheme(key: &[u8; 48], scheme: BlsScheme) -> Option<[u8; 48]> {
    let other = match scheme {
        BlsScheme::Legacy => BlsScheme::Modern,
        _ => BlsScheme::Legacy,
    };
    BlsPkBytes::from_bytes(*key)
        .as_scheme(scheme)
        .reencode(other)
        .ok()
        .map(|pk| *pk.as_bytes())
}

/// Whether `entry` holds `key` (in `scheme`) under either serialization.
pub(crate) fn holds_operator_key(
    entry: &ListedMasternode,
    key: &[u8; 48],
    scheme: BlsScheme,
) -> bool {
    entry.operator_public_key == *key
        || other_scheme(key, scheme).is_some_and(|k| entry.operator_public_key == k)
}

/// `IsPayoutListTriviallyValid` for a version-2 payload's one payout (all
/// of the reward, so the reward checks hold).
fn check_payout(script: &ScriptBuf, owner: [u8; 20], voting: [u8; 20]) -> Result<(), Reject> {
    if !is_valid_payout_script(script) {
        return Err("bad-protx-payee");
    }
    if let Some(id) = p2pkh_key_id(script)
        && (id == owner || id == voting)
    {
        return Err("bad-protx-payee-reuse");
    }
    Ok(())
}

/// `CProRegTx::IsTriviallyValid` for a version-1 or -2 payload: the
/// builders' payload, so no shares and one payout.
pub fn check_registration_trivial(p: &ProviderRegistrationPayload) -> Result<(), Reject> {
    if p.version == 0 || p.version > EXT_ADDR {
        return Err("bad-protx-version");
    }
    let evo = p.masternode_type == ProviderMasternodeType::HighPerformance;
    if p.version < BASIC_BLS && evo {
        return Err("bad-protx-evo-version");
    }
    if p.masternode_mode != 0 {
        return Err("bad-protx-mode");
    }
    if p.owner_key_hash == PubkeyHash::all_zeros() {
        return Err("bad-protx-key-null");
    }
    let operator: [u8; 48] = *p.operator_public_key.as_ref();
    if !is_valid_operator_key(&operator, operator_scheme(p.version))
        || p.voting_key_hash == PubkeyHash::all_zeros()
    {
        return Err("bad-protx-key-null");
    }
    check_payout(
        &p.script_payout,
        p.owner_key_hash.to_byte_array(),
        p.voting_key_hash.to_byte_array(),
    )?;
    if p.operator_reward > MAX_OPERATOR_REWARD {
        return Err("bad-protx-operator-reward");
    }
    Ok(())
}

/// `CheckProviderNetworkFields` with `CheckNetInfo` for a version-2
/// payload on `network`: an empty service is allowed (`allow_empty`, the
/// masternode registers PoSe-banned); otherwise the address must pass
/// `MnNetInfo::ValidateService` (`netinfo.cpp:286`). An evonode's node id
/// must be non-null and its ports pass Core's rules.
pub fn check_network_fields(
    p: &ProviderRegistrationPayload,
    network: Network,
) -> Result<(), Reject> {
    let service = legacy_service(&p.service_address)?;
    if !service::is_no_service(&service) {
        let v4 = match service {
            SocketAddr::V4(a) => a,
            SocketAddr::V6(a) => match a.ip().to_ipv4_mapped() {
                Some(ip) => std::net::SocketAddrV4::new(ip, a.port()),
                None => return Err("bad-protx-netinfo-addr-type"),
            },
        };
        service::validate_service(v4, network).map_err(|e| match e {
            ServiceError::BadPort(_) => "bad-protx-netinfo-port",
            ServiceError::NotRoutable(_) => "bad-protx-netinfo-addr-unroutable",
            _ => "bad-protx-netinfo-addr",
        })?;
    }
    if p.masternode_type != ProviderMasternodeType::HighPerformance {
        return Ok(());
    }
    if p.platform_node_id
        .is_none_or(|id| id.to_byte_array() == [0; 20])
    {
        return Err("bad-protx-platform-nodeid");
    }
    let (Some(p2p), Some(http)) = (p.platform_p2p_port, p.platform_http_port) else {
        return Err("bad-protx-platform-p2p-port");
    };
    service::check_platform_ports(network, service.port(), p2p, http).map_err(|e| match e {
        ServiceError::BadPlatformPort("P2P", _) => "bad-protx-platform-p2p-port",
        ServiceError::BadPlatformPort(_, _) => "bad-protx-platform-http-port",
        _ => "bad-protx-platform-dup-ports",
    })
}

/// `IsPayoutListKeySafe` at version 2: the collateral's key is neither the
/// owner nor the voting key (`bad-protx-collateral-reuse`). Version 2 does
/// not compare the payee with the collateral (`check_payout_collateral_reuse`
/// is for version 3).
pub fn check_collateral_key_safe(
    collateral_script: &ScriptBuf,
    owner: [u8; 20],
    voting: [u8; 20],
) -> Result<(), Reject> {
    match p2pkh_key_id(collateral_script) {
        Some(id) if id == owner || id == voting => Err("bad-protx-collateral-reuse"),
        _ => Ok(()),
    }
}

/// `CheckProRegTx`'s collateral for an output the transaction itself
/// creates (a null payload txid): its index, amount and destination, and
/// an empty signature.
pub fn check_internal_collateral(
    tx: &Transaction,
    p: &ProviderRegistrationPayload,
) -> Result<(), Reject> {
    let evo = p.masternode_type == ProviderMasternodeType::HighPerformance;
    let out = tx
        .output
        .get(p.collateral_outpoint.vout as usize)
        .ok_or("bad-protx-collateral-index")?;
    if out.value != collateral_amount(evo) {
        return Err("bad-protx-collateral");
    }
    if !is_valid_payout_script(&out.script_pubkey) {
        return Err("bad-protx-collateral-dest");
    }
    check_collateral_key_safe(
        &out.script_pubkey,
        p.owner_key_hash.to_byte_array(),
        p.voting_key_hash.to_byte_array(),
    )?;
    if !p.signature.is_empty() {
        return Err("bad-protx-sig");
    }
    Ok(())
}

/// `CheckProRegTx`'s collateral for an existing output: its amount and
/// script, when the wallet has the coin (`bad-protx-collateral`,
/// `bad-protx-collateral-pkh`), the key reuse, and the signature by its key
/// over `MakeSignString` (`CheckStringSig`, `specialtxman.cpp:1079-1094`).
pub fn check_external_collateral(
    p: &ProviderRegistrationPayload,
    collateral_script: &ScriptBuf,
    collateral_value: Option<u64>,
    network: Network,
) -> Result<(), Reject> {
    let evo = p.masternode_type == ProviderMasternodeType::HighPerformance;
    if collateral_value.is_some_and(|v| v != collateral_amount(evo)) {
        return Err("bad-protx-collateral");
    }
    let key_id = p2pkh_key_id(collateral_script).ok_or("bad-protx-collateral-pkh")?;
    check_collateral_key_safe(
        collateral_script,
        p.owner_key_hash.to_byte_array(),
        p.voting_key_hash.to_byte_array(),
    )?;
    let message = p
        .payload_collateral_string(network)
        .map_err(|_| "bad-protx-payee-dest")?;
    if !signed_by(
        &p.signature,
        signed_msg_hash(&message).to_byte_array(),
        key_id,
    ) {
        return Err("bad-protx-sig");
    }
    Ok(())
}

/// Whether `sig` is a 65-byte compact signature over `hash` by the key
/// with hash160 `key_id`, compressed or not: what Core's legacy
/// `CHashSigner::VerifyHash` accepts. Shared-masternode signatures use
/// Core's canonical check instead (MG-04b).
pub fn signed_by(sig: &[u8], hash: [u8; 32], key_id: [u8; 20]) -> bool {
    let Ok(mut sig) = <[u8; 65]>::try_from(sig) else {
        return false;
    };
    // As `VerifyHash` reads the header; `from_slice` alone is narrower.
    sig[0] = 27 + (sig[0].wrapping_sub(27) & 7);
    let Ok(sig) = MessageSignature::from_slice(&sig) else {
        return false;
    };
    let Ok(public) = sig.signature.recover_ecdsa(Message::from_digest(hash)) else {
        return false;
    };
    let public = dashcore::PublicKey {
        compressed: sig.compressed,
        inner: public,
    };
    public.pubkey_hash().to_byte_array() == key_id
}

/// `CheckInputsHash`: the payload commits to the transaction's inputs.
pub fn check_inputs_hash(tx: &Transaction, inputs_hash: [u8; 32]) -> Result<(), Reject> {
    if tx.hash_inputs().to_byte_array() != inputs_hash {
        return Err("bad-protx-inputs-hash");
    }
    Ok(())
}

/// `CheckProRegTx`'s list checks of a version-2 payload: a service another
/// collateral's masternode holds under any purpose, an operator key any
/// masternode holds under either serialization, or an evonode's platform
/// node id already listed. Core v24 compares the operator key under both schemes
/// (`specialtxman.cpp:1266-1274`); before v24 only the payload's own, so
/// this is stricter there (MG-04 D5).
pub fn check_registration_list(
    p: &ProviderRegistrationPayload,
    collateral: OutPoint,
    list: &[ListedMasternode],
) -> Result<(), Reject> {
    let service = legacy_entry(legacy_service(&p.service_address)?);
    check_net_info_unique(&service, collateral, list)?;
    let operator: [u8; 48] = *p.operator_public_key.as_ref();
    let scheme = operator_scheme(p.version);
    if list
        .iter()
        .any(|e| holds_operator_key(e, &operator, scheme))
    {
        return Err("bad-protx-dup-key");
    }
    if p.masternode_type == ProviderMasternodeType::HighPerformance
        && let Some(id) = p.platform_node_id
        && list
            .iter()
            .any(|e| e.platform_node_id == Some(id.to_byte_array()))
    {
        return Err("bad-protx-dup-platformnodeid");
    }
    Ok(())
}

/// A version-2 payload's service address. Core refuses extended net info
/// below version 3 (`providertx.cpp:216-218`).
fn legacy_service(service: &MasternodeNetInfo) -> Result<SocketAddr, Reject> {
    match service {
        MasternodeNetInfo::Legacy(addr) => Ok(*addr),
        MasternodeNetInfo::Extended(_) => Err("bad-protx-netinfo-version"),
    }
}

/// Every check of a finished ProRegTx the wallet can run: trivial
/// validity, the network fields, the collateral (internal, or existing
/// with its script and, when the wallet holds it, its value), the inputs
/// hash, and the list.
pub fn check_registration(
    tx: &Transaction,
    p: &ProviderRegistrationPayload,
    existing: Option<(&ScriptBuf, Option<u64>)>,
    list: &[ListedMasternode],
    network: Network,
) -> Result<(), Reject> {
    check_registration_trivial(p)?;
    check_network_fields(p, network)?;
    let collateral = if p.collateral_outpoint.txid == Txid::all_zeros() {
        check_internal_collateral(tx, p)?;
        OutPoint::new(tx.txid(), p.collateral_outpoint.vout)
    } else {
        let (script, value) = existing.ok_or("bad-protx-collateral")?;
        check_external_collateral(p, script, value, network)?;
        p.collateral_outpoint
    };
    check_inputs_hash(tx, p.inputs_hash.to_byte_array())?;
    check_registration_list(p, collateral, list)
}

/// `CProUpRegTx::IsTriviallyValid` plus `CheckProUpRegTx`'s checks the
/// wallet can run: the payee against the masternode's owner key and the new
/// voting key, the collateral key reuse (`collateral_script`, when the
/// wallet knows it), an operator key another listed masternode holds, the
/// inputs hash, and the owner's signature over the payload hash
/// (`CheckHashSig`).
pub fn check_update_registrar(
    tx: &Transaction,
    p: &ProviderUpdateRegistrarPayload,
    owner_key_id: [u8; 20],
    collateral_script: Option<&ScriptBuf>,
    list: &[ListedMasternode],
) -> Result<(), Reject> {
    check_update_registrar_terms(p, owner_key_id, collateral_script, list)?;
    check_inputs_hash(tx, p.inputs_hash.to_byte_array())?;
    if !signed_by(
        &p.payload_sig,
        p.base_payload_hash().to_byte_array(),
        owner_key_id,
    ) {
        return Err("bad-protx-sig");
    }
    Ok(())
}

/// [`check_update_registrar`] without the inputs hash and the signature:
/// what a placeholder can be held to before anything is signed.
pub fn check_update_registrar_terms(
    p: &ProviderUpdateRegistrarPayload,
    owner_key_id: [u8; 20],
    collateral_script: Option<&ScriptBuf>,
    list: &[ListedMasternode],
) -> Result<(), Reject> {
    if p.version == 0 || p.version > EXT_ADDR {
        return Err("bad-protx-version");
    }
    if p.provider_mode != 0 {
        return Err("bad-protx-mode");
    }
    let operator: [u8; 48] = *p.operator_public_key.as_ref();
    let scheme = operator_scheme(p.version);
    let voting = p.voting_key_hash.to_byte_array();
    if !is_valid_operator_key(&operator, scheme) || voting == [0; 20] {
        return Err("bad-protx-key-null");
    }
    check_payout(&p.script_payout, owner_key_id, voting)?;
    if let Some(script) = collateral_script {
        check_collateral_key_safe(script, owner_key_id, voting)?;
    }
    let own = p.pro_tx_hash.to_byte_array();
    if list
        .iter()
        .any(|e| e.pro_tx_hash != own && holds_operator_key(e, &operator, scheme))
    {
        return Err("bad-protx-dup-key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payloads::{CollateralSource, RegistrationTerms, registration_placeholder};
    use dashcore::{Address, TxIn, TxOut};

    fn script(seed: u8) -> ScriptBuf {
        Address::new(
            Network::Regtest,
            dashcore::address::Payload::PubkeyHash(PubkeyHash::from_byte_array([seed; 20])),
        )
        .script_pubkey()
    }

    fn payload(evo: bool) -> ProviderRegistrationPayload {
        registration_placeholder(&RegistrationTerms {
            evo,
            collateral: CollateralSource::FundNew {
                script: script(5),
                amount: collateral_amount(evo),
            },
            service: "127.0.0.1:19899".parse().unwrap(),
            owner_key_hash: [1; 20],
            voting_key_hash: [2; 20],
            operator_public_key: crate::bls::TEST_OPERATOR_KEY,
            operator_reward: 0,
            payout_script: script(3),
            platform: evo.then_some(crate::payloads::PlatformPorts {
                node_id: [7; 20],
                p2p_port: 22_200,
                http_port: 22_201,
            }),
        })
    }

    #[test]
    fn trivial_validity_returns_cores_reasons() {
        assert_eq!(check_registration_trivial(&payload(false)), Ok(()));
        let mut p = payload(false);
        p.version = 4;
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-version"));
        let mut p = payload(true);
        p.version = 1;
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-evo-version"));
        let mut p = payload(false);
        p.masternode_mode = 1;
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-mode"));
        let mut p = payload(false);
        p.voting_key_hash = PubkeyHash::all_zeros();
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-key-null"));
        let mut p = payload(false);
        p.operator_public_key = [0x11; 48].into();
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-key-null"));
        let mut p = payload(false);
        p.script_payout = script(1);
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-payee-reuse"));
        p.script_payout = script(2);
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-payee-reuse"));
        p.script_payout = ScriptBuf::from_bytes(vec![0x51]);
        assert_eq!(check_registration_trivial(&p), Err("bad-protx-payee"));
        let mut p = payload(false);
        p.operator_reward = 10_001;
        assert_eq!(
            check_registration_trivial(&p),
            Err("bad-protx-operator-reward")
        );
    }

    #[test]
    fn network_fields_follow_core() {
        let r = Network::Regtest;
        assert_eq!(check_network_fields(&payload(true), r), Ok(()));
        let mut p = payload(true);
        p.platform_node_id = Some(dashcore::platform_node_id::PlatformNodeId::from_byte_array(
            [0; 20],
        ));
        assert_eq!(
            check_network_fields(&p, r),
            Err("bad-protx-platform-nodeid")
        );
        let mut p = payload(true);
        p.platform_p2p_port = Some(9_999);
        assert_eq!(
            check_network_fields(&p, r),
            Err("bad-protx-platform-p2p-port")
        );
        let mut p = payload(true);
        p.platform_http_port = Some(22_200);
        assert_eq!(
            check_network_fields(&p, r),
            Err("bad-protx-platform-dup-ports")
        );
        let mut p = payload(false);
        p.service_address = MasternodeNetInfo::Legacy("127.0.0.1:9999".parse().unwrap());
        assert_eq!(check_network_fields(&p, r), Err("bad-protx-netinfo-port"));
        assert_eq!(
            check_network_fields(&payload(false), Network::Testnet),
            Err("bad-protx-netinfo-addr-unroutable")
        );
        let mut p = payload(false);
        p.service_address = MasternodeNetInfo::Legacy(service::NO_SERVICE);
        assert_eq!(check_network_fields(&p, Network::Testnet), Ok(()));
    }

    #[test]
    fn internal_collateral_is_checked_by_index_amount_and_key() {
        let p = payload(false);
        let mut tx = Transaction {
            version: 3,
            lock_time: 0,
            input: vec![TxIn::default()],
            output: vec![TxOut {
                value: collateral_amount(false),
                script_pubkey: script(5),
            }],
            special_transaction_payload: None,
        };
        assert_eq!(check_internal_collateral(&tx, &p), Ok(()));
        let mut q = p.clone();
        q.collateral_outpoint.vout = 1;
        assert_eq!(
            check_internal_collateral(&tx, &q),
            Err("bad-protx-collateral-index")
        );
        tx.output[0].value -= 1;
        assert_eq!(
            check_internal_collateral(&tx, &p),
            Err("bad-protx-collateral")
        );
        tx.output[0].value += 1;
        tx.output[0].script_pubkey = script(1);
        assert_eq!(
            check_internal_collateral(&tx, &p),
            Err("bad-protx-collateral-reuse")
        );
        tx.output[0].script_pubkey = script(5);
        let mut q = p.clone();
        q.signature = vec![0; 65];
        assert_eq!(check_internal_collateral(&tx, &q), Err("bad-protx-sig"));
    }

    #[test]
    fn list_duplicates_are_refused() {
        let p = payload(true);
        let collateral = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
        let entry = |service: &str, op: [u8; 48], node: [u8; 20]| ListedMasternode {
            pro_tx_hash: [4; 32],
            net_info: listed_net_info(&MasternodeNetInfo::Legacy(service.parse().unwrap())),
            operator_public_key: op,
            voting_key_id: [8; 20],
            platform_node_id: Some(node),
            collateral: None,
        };
        let other_key = crate::bls::legacy_of(&crate::bls::TEST_OPERATOR_KEY).unwrap();
        // Legacy form of our key: a duplicate under either scheme.
        let list = [entry("127.0.0.2:19899", other_key, [6; 20])];
        assert_eq!(
            check_registration_list(&p, collateral, &list),
            Err("bad-protx-dup-key")
        );
        let unrelated = [0x88u8; 48];
        let list = [entry("127.0.0.1:19899", unrelated, [6; 20])];
        assert_eq!(
            check_registration_list(&p, collateral, &list),
            Err("bad-protx-dup-netinfo-entry")
        );
        // The same collateral may reuse the address (it replaces the entry).
        let mut same = list.clone();
        same[0].collateral = Some(collateral);
        assert_eq!(check_registration_list(&p, collateral, &same), Ok(()));
        let list = [entry("127.0.0.2:19899", unrelated, [7; 20])];
        assert_eq!(
            check_registration_list(&p, collateral, &list),
            Err("bad-protx-dup-platformnodeid")
        );
    }

    /// MG-04b D39: every entry of a listed masternode is Core's unique
    /// property, whatever its purpose; an IPv4-mapped legacy address is its
    /// IPv4 one, and a domain or another network's address isn't a service.
    #[test]
    fn every_listed_net_info_entry_is_unique() {
        use dashcore::sml::masternode_list_entry::net_info::{
            ExtNetInfo, NetInfoEntry as ListEntry, NetInfoPurpose,
        };
        let p = payload(false);
        let collateral = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
        let ipv4 = |a: [u8; 4], port| ListEntry::Service {
            network: Bip155Network::Ipv4,
            addr: a.to_vec(),
            port,
        };
        let listed = |info: MasternodeNetInfo| ListedMasternode {
            pro_tx_hash: [4; 32],
            net_info: listed_net_info(&info),
            operator_public_key: [0x88; 48],
            voting_key_id: [8; 20],
            platform_node_id: None,
            collateral: None,
        };
        let ext = |purposes| {
            MasternodeNetInfo::Extended(ExtNetInfo {
                version: 1,
                purposes,
            })
        };
        // The payload's service, 127.0.0.1:19899, as another masternode's
        // second Core P2P entry, and as its Platform P2P entry.
        for purposes in [
            vec![(
                NetInfoPurpose::CoreP2P,
                vec![ipv4([127, 0, 0, 2], 19899), ipv4([127, 0, 0, 1], 19899)],
            )],
            vec![
                (NetInfoPurpose::CoreP2P, vec![ipv4([127, 0, 0, 2], 19899)]),
                (
                    NetInfoPurpose::PlatformP2P,
                    vec![ipv4([127, 0, 0, 1], 19899)],
                ),
            ],
        ] {
            let list = [listed(ext(purposes))];
            assert_eq!(
                check_registration_list(&p, collateral, &list),
                Err("bad-protx-dup-netinfo-entry")
            );
        }
        let mapped = MasternodeNetInfo::Legacy("[::ffff:127.0.0.1]:19899".parse().unwrap());
        assert_eq!(
            check_registration_list(&p, collateral, &[listed(mapped)]),
            Err("bad-protx-dup-netinfo-entry")
        );
        let other = ext(vec![
            (NetInfoPurpose::CoreP2P, vec![ipv4([127, 0, 0, 1], 19900)]),
            (
                NetInfoPurpose::PlatformHttps,
                vec![ListEntry::Domain {
                    host: "127.0.0.1".into(),
                    port: 19899,
                }],
            ),
        ]);
        assert_eq!(
            check_registration_list(&p, collateral, &[listed(other)]),
            Ok(())
        );
        assert!(listed_net_info(&MasternodeNetInfo::Legacy(service::NO_SERVICE)).is_empty());
        // A domain is listed too, keyed by host and port.
        let domain = ListEntry::Domain {
            host: "node.example".into(),
            port: 19999,
        };
        let holder = listed(ext(vec![(NetInfoPurpose::PlatformHttps, vec![domain])]));
        let ours = NetInfoEntry::Domain {
            name: "node.example".into(),
            port: 19999,
        };
        assert_eq!(
            check_net_info_unique([&ours], collateral, &[holder]),
            Err("bad-protx-dup-netinfo-entry")
        );
    }

    /// `CheckMalleable` (MG-04b D66): an operator key that decodes to a G1
    /// point but serializes back to other bytes is invalid. The legacy
    /// reader ignores the unused flag bits of the first byte; no such
    /// encoding of the generator was found in the basic serialization.
    #[test]
    fn an_operator_key_must_serialize_back_to_its_bytes() {
        let basic = crate::bls::parse_public_hex(
            "97f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb",
        )
        .unwrap();
        let legacy = crate::bls::legacy_of(&basic).unwrap();
        assert!(is_valid_operator_key(&basic, BlsScheme::Modern));
        assert!(is_valid_operator_key(&legacy, BlsScheme::Legacy));
        for flag in [0x20, 0x40] {
            let mut malleated = legacy;
            malleated[0] ^= flag;
            let decoded = BlsPkBytes::from_bytes(malleated)
                .as_scheme(BlsScheme::Legacy)
                .canonicalize();
            assert_eq!(decoded.unwrap().as_bytes(), &legacy, "the same point");
            assert!(!is_valid_operator_key(&malleated, BlsScheme::Legacy));
        }
    }

    #[test]
    fn signed_by_accepts_what_core_verifyhash_accepts() {
        let secret = [0x11u8; 32];
        let hash = [0x22u8; 32];
        let public = dashcore::PublicKey {
            compressed: true,
            inner: dashcore::secp256k1::PublicKey::from_secret_key(
                &dashcore::secp256k1::SecretKey::from_secret_bytes(secret).unwrap(),
            ),
        };
        let key_id = public.pubkey_hash().to_byte_array();
        let sig = crate::payloads::sign_hash_compact(&secret, &hash).unwrap();
        assert!(signed_by(&sig, hash, key_id));
        for header in [sig[0].wrapping_add(8), sig[0].wrapping_sub(8)] {
            let mut moved = sig;
            moved[0] = header;
            assert!(signed_by(&moved, hash, key_id), "header {header}");
        }
        let mut uncompressed = sig;
        uncompressed[0] -= 4;
        assert!(!signed_by(&uncompressed, hash, key_id));
        assert!(!signed_by(&sig[..64], hash, key_id));
    }

    #[test]
    fn p2pkh_key_ids_come_from_p2pkh_scripts_only() {
        assert_eq!(p2pkh_key_id(&script(3)), Some([3; 20]));
        assert_eq!(p2pkh_key_id(&ScriptBuf::new()), None);
        let p2sh = Address::new(
            Network::Regtest,
            dashcore::address::Payload::ScriptHash(dashcore::ScriptHash::from_byte_array([3; 20])),
        )
        .script_pubkey();
        assert_eq!(p2pkh_key_id(&p2sh), None);
    }
}
