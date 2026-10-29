//! Dash Core's checks of the shared-masternode payloads, as pure functions
//! returning Core's reject reason (v24.0.0-rc.3):
//!
//! - `CProRegTx::IsTriviallyValid` for a version-3 payload
//!   (`src/evo/providertx.cpp:265-333`), with `IsShareListTriviallyValid`
//!   (`:63-130`) or `IsPayoutListTriviallyValid` (`:25-62`), and
//!   `CheckProRegTx` (`src/evo/specialtxman.cpp:1148-1321`):
//!   [`check_registration_trivial`], [`check_registration`];
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
//!
//! The prefix every contextual check shares, `GetValidatedPayload`
//! (`specialtxman.cpp:1098-1121`), is the decoder's (MG-04b n-r2-7): a
//! payload that doesn't decode is `bad-protx-payload`, and
//! [`SpecialTx::from_bytes`] and the payload decoders ([`ProRegTxV3`],
//! [`ProDisTx`], …) refuse trailing bytes, as `GetTxPayload` does
//! (`specialtx.h:18-28`). What depends on the caller, the transaction's type
//! (`bad-protx-type`, `:1102-1104`) and the version against `GetMax`
//! (`bad-protx-version`, `:1113-1115`: version 3 needs v24), is checked in
//! [`check_registration`]; the maintenance types exist only from v24 (each
//! check's `-too-early`).

use dashcore::bls_sig_utils::BlsScheme;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::Message;
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::{Network, OutPoint, ScriptBuf, Txid};

use super::netinfo::{
    ExtNetInfo, NetInfoEntry, PURPOSE_CORE_P2P, PURPOSE_PLATFORM_HTTPS, PURPOSE_PLATFORM_P2P,
};
use super::rawtx::SpecialTx;
use super::registration::{
    MN_TYPE_EVO, MN_TYPE_REGULAR, ProRegTxV3, SHARED_COLLATERAL_SCRIPT,
    TRANSACTION_PROVIDER_REGISTER,
};
use super::{
    CollateralShare, PRODIS_MAX_FEE, PayoutShare, ProDisTx, ProUpShareTx, ProUpSharedRegTx,
};
use crate::params::{
    MAX_EARLY_PERIOD_BLOCKS, MAX_OPERATOR_REWARD, MAX_SHARES, MIN_SHARE_AMOUNT, MIN_SHARES,
    collateral_amount,
};
use crate::rules::{
    ListedMasternode, Reject, check_net_info_unique, holds_operator_key, is_valid_operator_key,
    p2pkh_key_id,
};

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

/// `CHashSigner::VerifyHashCanonical` (rc.3 `messagesigner.cpp:58-88`), the
/// check of every share, join and consent signature (`specialtxman.cpp`):
/// 65 bytes, header 27–34, low S, then `CPubKey::RecoverCompact`
/// (`pubkey.cpp:199-217`) whose key's hash160 is `key_id`. Self-contained:
/// [`crate::rules::signed_by`] is Core's legacy `VerifyHash` for the
/// ProRegTx/ProUpRegTx owner checks and takes the aliases this refuses.
pub fn verify_hash_canonical(hash: [u8; 32], key_id: [u8; 20], sig: &[u8]) -> bool {
    const HALF_ORDER: [u8; 32] = [
        0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b,
        0x20, 0xa0,
    ];
    if sig.len() != 65 || !(27..=34).contains(&sig[0]) || sig[33..] > HALF_ORDER[..] {
        return false;
    }
    let header = sig[0] - 27;
    let Ok(recid) = RecoveryId::try_from(i32::from(header & 3)) else {
        return false;
    };
    let Ok(rsig) = RecoverableSignature::from_compact(&sig[1..], recid) else {
        return false;
    };
    let Ok(public) = rsig.recover_ecdsa(Message::from_digest(hash)) else {
        return false;
    };
    let public = dashcore::PublicKey {
        compressed: header & 4 != 0,
        inner: public,
    };
    public.pubkey_hash().to_byte_array() == key_id
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

// ---- ProRegTx, version 3 (type 1) ----

/// `MasternodePayoutShares`' size bound (`providertx.cpp:28`).
const MAX_PAYOUTS: usize = 8;
/// `MasternodePayoutShare::MIN_REWARD` / `MAX_REWARD` (`providertx.h:34-35`).
const MIN_PAYOUT_REWARD: u16 = 100;
const MAX_PAYOUT_REWARD: u16 = 10_000;

/// A `CTxDestination` Core can extract (`ExtractDestination`, rc.3
/// `script/standard.cpp:161-189`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    PubkeyHash([u8; 20]),
    ScriptHash([u8; 20]),
}

/// `ExtractDestination`: P2PKH, P2SH, or P2PK with a validly sized public
/// key (`CPubKey::ValidSize`: 33 bytes with header 2 or 3, 65 with 4, 6
/// or 7), whose destination is its key's hash.
fn extract_destination(script: &ScriptBuf) -> Option<Destination> {
    if let Some(id) = p2pkh_key_id(script) {
        return Some(Destination::PubkeyHash(id));
    }
    let b = script.as_bytes();
    if script.is_p2sh() {
        return Some(Destination::ScriptHash(b[2..22].try_into().expect("P2SH")));
    }
    let key = match b {
        [33, key @ .., 0xac] if key.len() == 33 && matches!(key[0], 2 | 3) => key,
        [65, key @ .., 0xac] if key.len() == 65 && matches!(key[0], 4 | 6 | 7) => key,
        _ => return None,
    };
    Some(Destination::PubkeyHash(
        hash160::Hash::hash(key).to_byte_array(),
    ))
}

/// `IsShareListTriviallyValid` (`providertx.cpp:63-130`), in its order.
/// A refund or reward script is P2PKH or P2SH, so `ExtractDestination`
/// succeeds (`bad-protx-shares-payee-dest`, `:104-106`, can't arise).
fn check_share_list(
    shares: &[CollateralShare],
    early_period_blocks: u32,
    early_penalty: i64,
    required_collateral: u64,
    voting_key_id: [u8; 20],
) -> Result<(), Reject> {
    if shares.len() < MIN_SHARES as usize || shares.len() > MAX_SHARES as usize {
        return Err("bad-protx-shares-count");
    }
    if early_period_blocks > MAX_EARLY_PERIOD_BLOCKS {
        return Err("bad-protx-shares-early-period");
    }
    let mut total: u64 = 0;
    let mut min_amount = u64::MAX;
    for (i, share) in shares.iter().enumerate() {
        // Bounded by the collateral first, so the sum can't overflow (`:78`).
        if share.amount < MIN_SHARE_AMOUNT || share.amount > required_collateral {
            return Err("bad-protx-shares-amount");
        }
        total += share.amount;
        min_amount = min_amount.min(share.amount);
        if share.owner_key_id == [0; 20] {
            return Err("bad-protx-shares-key-null");
        }
        let earlier = &shares[..i];
        if earlier.iter().any(|s| s.owner_key_id == share.owner_key_id) {
            return Err("bad-protx-shares-dup-key");
        }
        if earlier
            .iter()
            .any(|s| s.refund_script == share.refund_script)
        {
            return Err("bad-protx-shares-dup-refund");
        }
        // An empty reward script means "the refund script" (`:96-99`), so
        // it checks the refund again, to the same result.
        for script in [&share.refund_script, share.effective_reward_script()] {
            if !is_valid_payout_script(script) {
                return Err("bad-protx-shares-payee");
            }
            // The voting key, then every share's owner key, this one's
            // included (`:107-114`).
            if pays_key(script, voting_key_id)
                || shares.iter().any(|o| pays_key(script, o.owner_key_id))
            {
                return Err("bad-protx-shares-payee-reuse");
            }
        }
    }
    if total != required_collateral {
        return Err("bad-protx-shares-amount-sum");
    }
    if early_penalty < 0 || early_penalty as u64 >= min_amount {
        return Err("bad-protx-shares-penalty");
    }
    // A penalty without a period would still cap the unilateral bonus
    // (`:123-128`).
    if early_period_blocks == 0 && early_penalty != 0 {
        return Err("bad-protx-shares-penalty");
    }
    Ok(())
}

/// `IsPayoutListTriviallyValid` (`providertx.cpp:25-62`): a non-shared
/// version-3 payload's payouts (`GetOwnerPayouts`, `providertx.h:94-97`).
/// `bad-protx-payee-dest` (`:47-50`) can't arise after `bad-protx-payee`.
fn check_payout_list(
    payouts: &[PayoutShare],
    owner_key_id: [u8; 20],
    voting_key_id: [u8; 20],
) -> Result<(), Reject> {
    if payouts.is_empty() || payouts.len() > MAX_PAYOUTS {
        return Err("bad-protx-payouts-count");
    }
    let mut total: u32 = 0;
    for (i, payout) in payouts.iter().enumerate() {
        if !(MIN_PAYOUT_REWARD..=MAX_PAYOUT_REWARD).contains(&payout.reward) {
            return Err("bad-protx-payout-reward");
        }
        total += u32::from(payout.reward);
        if !is_valid_payout_script(&payout.script) {
            return Err("bad-protx-payee");
        }
        if payouts[..i].iter().any(|p| p.script == payout.script) {
            return Err("bad-protx-payee-dup");
        }
        if (owner_key_id != [0; 20] && pays_key(&payout.script, owner_key_id))
            || pays_key(&payout.script, voting_key_id)
        {
            return Err("bad-protx-payee-reuse");
        }
    }
    if total != u32::from(MAX_PAYOUT_REWARD) {
        return Err("bad-protx-payout-reward-sum");
    }
    Ok(())
}

/// `IsNetInfoTriviallyValid` (`providertx.cpp:166-185`): Core P2P entries
/// for every masternode, Platform entries for an EvoNode and never for a
/// regular one.
fn check_net_info_trivial(net_info: &ExtNetInfo, mn_type: u16) -> Result<(), Reject> {
    if !net_info.has_entries(PURPOSE_CORE_P2P) {
        return Err("bad-protx-netinfo-empty");
    }
    let https = net_info.has_entries(PURPOSE_PLATFORM_HTTPS);
    let p2p = net_info.has_entries(PURPOSE_PLATFORM_P2P);
    if mn_type == MN_TYPE_REGULAR && (https || p2p) {
        return Err("bad-protx-netinfo-bad");
    }
    if mn_type == MN_TYPE_EVO && !(https && p2p) {
        return Err("bad-protx-netinfo-empty");
    }
    Ok(())
}

/// `CProRegTx::IsTriviallyValid` (`providertx.cpp:265-333`) for a
/// version-3 payload, shared or not, in Core's order:
///
/// 1. `bad-protx-version` (267) and `bad-protx-evo-version` (270) can't
///    arise: [`ProRegTxV3`] is version 3 by construction.
/// 2. `bad-protx-type` (273, `IsValidMnType`: regular or Evo), `-mode`
///    (276).
/// 3. Shared: `bad-protx-shares-external` (282), `-shares-owner-key` (286),
///    `-shares-payouts` (289). Not shared: `-shares-empty-fields` (293),
///    `bad-protx-key-null` (null owner, 296).
/// 4. `bad-protx-key-null` (invalid operator key or null voting key, 300).
///    `bad-protx-operator-pubkey` (303) can't arise: a version-3 key is read
///    in the basic scheme, so a key that is only a legacy point is invalid.
/// 5. Shared: the share list ([`check_share_list`]). Not shared: the payout
///    list ([`check_payout_list`]).
/// 6. `bad-protx-netinfo-version` (315) can't arise: version 3 always has
///    an [`ExtNetInfo`]. A non-empty net info's trivial rules (318,
///    `bad-protx-netinfo-empty`, `-netinfo-bad`), then every entry trivially
///    valid (322-326, `bad-protx-netinfo-bad`).
/// 7. `bad-protx-operator-reward` (328).
pub fn check_registration_trivial(p: &ProRegTxV3) -> Result<(), Reject> {
    if !matches!(p.mn_type, MN_TYPE_REGULAR | MN_TYPE_EVO) {
        return Err("bad-protx-type");
    }
    if p.mode != 0 {
        return Err("bad-protx-mode");
    }
    if p.is_shared() {
        if p.collateral.txid != Txid::all_zeros() {
            return Err("bad-protx-shares-external");
        }
        if p.owner_key_id != [0; 20] {
            return Err("bad-protx-shares-owner-key");
        }
        if !p.payouts.is_empty() {
            return Err("bad-protx-shares-payouts");
        }
    } else {
        if p.early_period_blocks != 0 || p.early_penalty != 0 {
            return Err("bad-protx-shares-empty-fields");
        }
        if p.owner_key_id == [0; 20] {
            return Err("bad-protx-key-null");
        }
    }
    if !is_valid_operator_key(&p.operator_public_key, BlsScheme::Modern)
        || p.voting_key_id == [0; 20]
    {
        return Err("bad-protx-key-null");
    }
    if p.is_shared() {
        check_share_list(
            &p.shares,
            p.early_period_blocks,
            p.early_penalty,
            collateral_amount(p.mn_type == MN_TYPE_EVO),
            p.voting_key_id,
        )?;
    } else {
        check_payout_list(&p.payouts, p.owner_key_id, p.voting_key_id)?;
    }
    if !p.net_info.is_empty_in_core() {
        check_net_info_trivial(&p.net_info, p.mn_type)?;
    }
    if !p
        .net_info
        .all_entries()
        .all(NetInfoEntry::is_trivially_valid)
    {
        return Err("bad-protx-netinfo-bad");
    }
    if p.operator_reward > MAX_OPERATOR_REWARD {
        return Err("bad-protx-operator-reward");
    }
    Ok(())
}

/// `CheckProviderNetworkFields` (`providertx.cpp:212-263`) for a
/// version-3 payload with `allow_empty` (`specialtxman.cpp:1172-1177`): an
/// empty net info is allowed (the masternode starts PoSe-banned); otherwise
/// its trivial rules and `ExtNetInfo::Validate` with `CheckNetInfo`'s
/// reasons (MG-04b D5). An EvoNode needs a platform node id; its version-3
/// platform port fields are not serialized, so are zero as required
/// (`:233-241`).
fn check_network_fields(p: &ProRegTxV3, network: Network) -> Result<(), Reject> {
    // `bad-protx-netinfo-version` (`:216-218`): always an ExtNetInfo here.
    if !p.net_info.is_empty_in_core() {
        check_net_info_trivial(&p.net_info, p.mn_type)?;
        if let Some(reason) = p.net_info.validate(network).reject() {
            return Err(reason);
        }
    }
    if p.mn_type == MN_TYPE_EVO && p.platform_node_id.is_none_or(|id| id == [0; 20]) {
        return Err("bad-protx-platform-nodeid");
    }
    Ok(())
}

/// `IsPayoutListKeySafe` with `check_payout_collateral_reuse`
/// (`providertx.cpp:146-164`; on for version 3, `specialtxman.cpp:1229`).
fn check_payout_list_key_safe(
    payouts: &[PayoutShare],
    collateral: Destination,
    owner_key_id: [u8; 20],
    voting_key_id: [u8; 20],
) -> Result<(), Reject> {
    if collateral == Destination::PubkeyHash(owner_key_id)
        || collateral == Destination::PubkeyHash(voting_key_id)
    {
        return Err("bad-protx-collateral-reuse");
    }
    if payouts
        .iter()
        .any(|p| extract_destination(&p.script) == Some(collateral))
    {
        return Err("bad-protx-payee-reuse");
    }
    Ok(())
}

/// `CheckProRegTx` (`specialtxman.cpp:1148-1321`) for a version-3 payload
/// in `tx`, against the synced `list` at the tip, in Core's order. What an
/// SPV wallet can't see is skipped, each at its line:
///
/// - `GetValidatedPayload` (1152): `bad-protx-type` (`tx`'s type, 1102),
///   `bad-protx-version` (version 3 without v24, 1113), then
///   [`check_registration_trivial`].
/// - `bad-protx-version-disallowed` (1160) can't arise at version 3.
/// - An EvoNode (1163-1170): `bad-protx-shares-evo` for a shared one,
///   `bad-protx-payouts-evo` for more than one payout. The `evo_shares`
///   deployment is taken as inactive: the wallet registers regular shared
///   masternodes only (MG-04b D4), so this refuses an Evo shared
///   registration as Core does before `evo_shares`.
/// - The network fields (1174-1177, [`check_network_fields`]).
/// - The collateral (1183-1221): an existing coin's value and key (1185-1202)
///   are in Core's UTXO set, which SPV doesn't have, so they are skipped;
///   only a non-shared payload can name one (`-shares-external`). An
///   output of `tx`: `bad-protx-collateral-index` (1205), `-collateral`
///   (the regular or Evo amount, 1208), then for a shared one
///   `bad-protx-shares-collateral-script` ([`SHARED_COLLATERAL_SCRIPT`]
///   exactly, 1213), else `-collateral-dest` (1217).
/// - Not shared, with an output of `tx` as collateral:
///   `bad-protx-collateral-reuse` / `bad-protx-payee-reuse` (1226-1229).
/// - The list (1231-1288): `bad-protx-dup-netinfo-entry` for a service
///   (1236-1240) or domain (1241-1245) of the payload another collateral's
///   masternode holds. Core's list holds every entry of every masternode as
///   a unique property, services and domains of all purposes
///   (`deterministicmns.cpp:459-470`); the SML carries a version-3 entry's
///   whole net info (`simplifiedmns.h:81`), and [`ListedMasternode::net_info`]
///   every entry of it (MG-04b D39, [`crate::rules::listed_net_info`]).
///   `bad-protx-netinfo-entry` (1247) can't arise after the trivial checks.
///   `bad-protx-dup-key`: owner keys (each share's, 1255-1259, or
///   `keyIDOwner`, 1263) aren't in the SML, so are skipped;
///   the operator key under either scheme (1260-1263, 1272).
///   `bad-protx-dup-platformnodeid` for an EvoNode (1277-1281).
///   `bad-protx-key-not-same` (1283-1287) applies only before DIP3
///   enforcement, long past on every network the wallet syncs.
/// - `bad-protx-inputs-hash` (1290).
/// - An existing collateral's signature (`CheckStringSig`, 1295-1300) needs
///   the coin's key: skipped. Otherwise `bad-protx-sig` for a non-empty
///   `vchSig` (1303), and with `check_sigs` a shared payload's join
///   signatures, each `VerifyHashCanonical` over
///   [`ProRegTxV3::consent_hash`] by its share's owner key
///   (`bad-protx-shares-sig`, 1306-1317).
pub fn check_registration(
    tx: &SpecialTx,
    p: &ProRegTxV3,
    list: &[ListedMasternode],
    v24_active: bool,
    network: Network,
    check_sigs: bool,
) -> Result<(), Reject> {
    if tx.tx_type != TRANSACTION_PROVIDER_REGISTER {
        return Err("bad-protx-type");
    }
    if !v24_active {
        return Err("bad-protx-version");
    }
    check_registration_trivial(p)?;
    let evo = p.mn_type == MN_TYPE_EVO;
    if evo {
        if p.is_shared() {
            return Err("bad-protx-shares-evo");
        }
        if p.payouts.len() > 1 {
            return Err("bad-protx-payouts-evo");
        }
    }
    check_network_fields(p, network)?;

    let internal = p.collateral.txid == Txid::all_zeros();
    let collateral = if internal {
        let vout = p.collateral.vout;
        let out = tx
            .output
            .get(vout as usize)
            .ok_or("bad-protx-collateral-index")?;
        if out.value != collateral_amount(evo) {
            return Err("bad-protx-collateral");
        }
        if p.is_shared() {
            if out.script_pubkey.as_bytes() != SHARED_COLLATERAL_SCRIPT {
                return Err("bad-protx-shares-collateral-script");
            }
        } else {
            let dest =
                extract_destination(&out.script_pubkey).ok_or("bad-protx-collateral-dest")?;
            check_payout_list_key_safe(&p.payouts, dest, p.owner_key_id, p.voting_key_id)?;
        }
        OutPoint::new(tx.txid(), vout)
    } else {
        p.collateral
    };

    check_net_info_unique(p.net_info.all_entries(), collateral, list)?;
    if list
        .iter()
        .any(|e| holds_operator_key(e, &p.operator_public_key, BlsScheme::Modern))
    {
        return Err("bad-protx-dup-key");
    }
    if evo
        && let Some(id) = p.platform_node_id
        && list.iter().any(|e| e.platform_node_id == Some(id))
    {
        return Err("bad-protx-dup-platformnodeid");
    }

    check_inputs_hash(tx, p.inputs_hash)?;
    if internal {
        if !p.sig.is_empty() {
            return Err("bad-protx-sig");
        }
        if p.is_shared() && check_sigs {
            let consent = p.consent_hash(tx);
            let all_signed = p.shares.iter().enumerate().all(|(i, share)| {
                p.join_sigs
                    .get(i)
                    .is_some_and(|sig| verify_hash_canonical(consent, share.owner_key_id, sig))
            });
            if !all_signed {
                return Err("bad-protx-shares-sig");
            }
        }
    }
    Ok(())
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
    use super::super::registration::NULL_JOIN_SIGNATURE;
    use super::*;
    use crate::payloads::sign_hash_compact;
    use crate::rules::{p2pkh_key_id, signed_by};
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
            net_info: Vec::new(),
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

    #[test]
    fn test_QT_127_key_change_needs_a_shared_masternode() {
        let shares = shares();
        let (tx, p) = key_change(&shares);
        let not_shared = SharedMn {
            shares: &[],
            ..mn(&shares)
        };
        assert_eq!(
            check_update_shared_registrar(&tx, &p, Some(&not_shared), &[], true, true),
            Err("bad-proupsharedreg-not-shared")
        );
    }

    #[test]
    fn test_QT_127_more_than_eight_signatures_are_trivially_invalid() {
        let mut d = ProDisTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([0xAA; 32]),
            actor_index: 0,
            sigs: vec![[0; 65]; 8],
        };
        assert_eq!(check_dissolution_trivial(&d), Ok(()));
        d.sigs.push([0; 65]);
        assert_eq!(check_dissolution_trivial(&d), Err("bad-prodis-sig-count"));

        let (_, mut k) = key_change(&shares());
        k.sigs = vec![[0; 65]; 8];
        assert_eq!(check_update_shared_registrar_trivial(&k), Ok(()));
        k.sigs.push([0; 65]);
        assert_eq!(
            check_update_shared_registrar_trivial(&k),
            Err("bad-proupsharedreg-sig-count")
        );
    }

    #[test]
    fn test_QT_127_payload_prefix_is_the_decoders() {
        // `GetTxPayload` refuses trailing bytes (`specialtx.h:18-28`):
        // `bad-protx-payload`, before any rule here runs.
        let (tx, p) = registration(|_, _| {});
        let mut bytes = p.to_payload().unwrap();
        bytes.push(0);
        assert!(ProRegTxV3::from_payload(&bytes).is_err());
        let mut raw = tx.to_bytes();
        raw.push(0);
        assert!(SpecialTx::from_bytes(&raw).is_err());
        let (_, k) = key_change(&shares());
        let mut bytes = super::super::payload_bytes(&k);
        bytes.push(0);
        assert!(dashcore::consensus::deserialize::<ProUpSharedRegTx>(&bytes).is_err());
    }

    // ---- ProRegTx version 3 (type 1) ----

    fn net(services: &[&str]) -> ExtNetInfo {
        let addrs: Vec<std::net::SocketAddr> =
            services.iter().map(|s| s.parse().unwrap()).collect();
        ExtNetInfo::core_p2p(&addrs).unwrap()
    }

    fn entries(service: &str) -> Vec<NetInfoEntry> {
        net(&[service]).entries.remove(0).1
    }

    /// A shared registration laid out as `PrepareSharedRegistration` does
    /// (change, then the template collateral at index 1), after `edit`,
    /// with its inputs hash, join signatures and payload filled in.
    fn registration(edit: impl FnOnce(&mut SpecialTx, &mut ProRegTxV3)) -> (SpecialTx, ProRegTxV3) {
        let mut tx = special(
            TRANSACTION_PROVIDER_REGISTER,
            vec![input(OutPoint::new(Txid::from_byte_array([5; 32]), 0))],
            vec![
                TxOut {
                    value: 12_345,
                    script_pubkey: p2pkh([0x44; 20]),
                },
                TxOut {
                    value: 1_000 * COIN,
                    script_pubkey: ScriptBuf::from_bytes(SHARED_COLLATERAL_SCRIPT.to_vec()),
                },
            ],
        );
        let mut p = ProRegTxV3 {
            mn_type: MN_TYPE_REGULAR,
            mode: 0,
            collateral: OutPoint::new(Txid::all_zeros(), 1),
            net_info: net(&["8.8.8.8:19999"]),
            owner_key_id: [0; 20],
            operator_public_key: crate::bls::TEST_OPERATOR_KEY,
            voting_key_id: VOTING,
            operator_reward: 0,
            payouts: vec![],
            shares: shares(),
            join_sigs: vec![],
            early_period_blocks: 100,
            early_penalty: PENALTY,
            inputs_hash: [0; 32],
            platform_node_id: None,
            sig: vec![],
        };
        edit(&mut tx, &mut p);
        p.inputs_hash = tx.inputs_hash();
        let consent = p.consent_hash(&tx);
        p.join_sigs = (0..p.shares.len())
            .map(|i| sign_hash_compact(&secret(i), &consent).unwrap())
            .collect();
        tx.payload = p.to_payload().unwrap();
        (tx, p)
    }

    /// An EvoNode's net info and node id, and a 4,000 DASH collateral.
    fn make_evo(tx: &mut SpecialTx, p: &mut ProRegTxV3) {
        p.mn_type = MN_TYPE_EVO;
        p.platform_node_id = Some([0x31; 20]);
        p.net_info
            .entries
            .push((PURPOSE_PLATFORM_P2P, entries("8.8.8.8:22000")));
        p.net_info
            .entries
            .push((PURPOSE_PLATFORM_HTTPS, entries("8.8.8.8:443")));
        tx.output[1].value = 4_000 * COIN;
        for (s, dash) in p.shares.iter_mut().zip([2_000, 1_000, 1_000]) {
            s.amount = dash * COIN;
        }
    }

    type Edit = fn(&mut ProRegTxV3);

    #[test]
    fn test_QT_126_shared_registration_trivial_rules_in_core_order() {
        let (_, p) = registration(|_, _| {});
        assert_eq!(check_registration_trivial(&p), Ok(()));
        let cases: &[(Reject, Edit)] = &[
            ("bad-protx-type", |p| p.mn_type = 2),
            ("bad-protx-mode", |p| p.mode = 1),
            ("bad-protx-shares-external", |p| {
                p.collateral.txid = Txid::from_byte_array([1; 32])
            }),
            ("bad-protx-shares-owner-key", |p| p.owner_key_id = [1; 20]),
            ("bad-protx-shares-payouts", |p| {
                p.payouts = vec![PayoutShare {
                    script: p2pkh([0x45; 20]),
                    reward: 10_000,
                }]
            }),
            ("bad-protx-key-null", |p| p.voting_key_id = [0; 20]),
            ("bad-protx-key-null", |p| p.operator_public_key = [0x11; 48]),
            ("bad-protx-shares-count", |p| p.shares.truncate(1)),
            ("bad-protx-shares-count", |p| {
                let more = p.shares[2].clone();
                p.shares.extend(vec![more; 6]);
            }),
            ("bad-protx-shares-early-period", |p| {
                p.early_period_blocks = MAX_EARLY_PERIOD_BLOCKS + 1
            }),
            ("bad-protx-shares-amount", |p| {
                p.shares[0].amount = 99 * COIN
            }),
            ("bad-protx-shares-amount", |p| {
                p.shares[2].amount = 1_000 * COIN + 1
            }),
            ("bad-protx-shares-key-null", |p| {
                p.shares[1].owner_key_id = [0; 20]
            }),
            ("bad-protx-shares-dup-key", |p| {
                p.shares[2].owner_key_id = p.shares[0].owner_key_id
            }),
            ("bad-protx-shares-dup-refund", |p| {
                p.shares[1].refund_script = p.shares[0].refund_script.clone()
            }),
            ("bad-protx-shares-payee", |p| {
                p.shares[0].refund_script = ScriptBuf::from_bytes(vec![0x51])
            }),
            ("bad-protx-shares-payee", |p| {
                p.shares[1].reward_script = ScriptBuf::from_bytes(vec![0x51])
            }),
            ("bad-protx-shares-payee-reuse", |p| {
                p.shares[0].refund_script = p2pkh(VOTING)
            }),
            // A later share's owner key, and the share's own.
            ("bad-protx-shares-payee-reuse", |p| {
                p.shares[0].reward_script = p2pkh(owner_id(2))
            }),
            ("bad-protx-shares-payee-reuse", |p| {
                p.shares[1].reward_script = p2pkh(owner_id(1))
            }),
            ("bad-protx-shares-amount-sum", |p| p.shares[0].amount -= 1),
            ("bad-protx-shares-penalty", |p| p.early_penalty = -1),
            // The smallest share is 300 DASH.
            ("bad-protx-shares-penalty", |p| {
                p.early_penalty = 300 * COIN as i64
            }),
            ("bad-protx-shares-penalty", |p| p.early_period_blocks = 0),
            // An unknown version isn't empty and has no Core P2P entry.
            ("bad-protx-netinfo-empty", |p| {
                p.net_info = ExtNetInfo {
                    version: 2,
                    entries: vec![],
                }
            }),
            // A purpose with no entry isn't empty either.
            ("bad-protx-netinfo-empty", |p| {
                p.net_info = ExtNetInfo {
                    version: 1,
                    entries: vec![(PURPOSE_CORE_P2P, vec![])],
                }
            }),
            ("bad-protx-netinfo-bad", |p| {
                p.net_info
                    .entries
                    .push((PURPOSE_PLATFORM_P2P, entries("8.8.8.8:22000")))
            }),
            ("bad-protx-netinfo-bad", |p| {
                p.net_info = net(&["0.0.0.0:19999"])
            }),
            ("bad-protx-operator-reward", |p| p.operator_reward = 10_001),
        ];
        for (reason, edit) in cases {
            let mut q = p.clone();
            edit(&mut q);
            assert_eq!(check_registration_trivial(&q), Err(*reason), "{reason}");
        }

        // Core's order where two rules meet.
        let mut q = p.clone();
        q.mode = 1;
        q.shares.clear();
        q.owner_key_id = [0; 20];
        assert_eq!(check_registration_trivial(&q), Err("bad-protx-mode"));
        let mut q = p.clone();
        q.shares[0].amount -= 1;
        q.shares[2].owner_key_id = [0; 20];
        assert_eq!(
            check_registration_trivial(&q),
            Err("bad-protx-shares-key-null")
        );
        let mut q = p.clone();
        q.early_penalty = -1;
        q.net_info = net(&["0.0.0.0:19999"]);
        q.operator_reward = 10_001;
        assert_eq!(
            check_registration_trivial(&q),
            Err("bad-protx-shares-penalty")
        );

        // The limits themselves pass; an empty net info is the net info's
        // trivial rules' to skip.
        let limits: &[Edit] = &[
            |p| p.early_penalty = 300 * COIN as i64 - 1,
            |p| p.early_penalty = 0,
            |p| {
                p.early_period_blocks = 0;
                p.early_penalty = 0
            },
            |p| p.early_period_blocks = MAX_EARLY_PERIOD_BLOCKS,
            |p| p.net_info = ExtNetInfo::default(),
            |p| p.operator_reward = 10_000,
            |p| {
                let mut eight = Vec::new();
                for i in 0..8u8 {
                    eight.push(CollateralShare {
                        amount: 125 * COIN,
                        refund_script: p2pkh([0x60 + i; 20]),
                        reward_script: ScriptBuf::new(),
                        owner_key_id: [0x80 + i; 20],
                    });
                }
                p.shares = eight;
                p.early_penalty = 0;
            },
        ];
        for edit in limits {
            let mut q = p.clone();
            edit(&mut q);
            assert_eq!(check_registration_trivial(&q), Ok(()));
        }
    }

    #[test]
    fn test_QT_126_non_shared_v3_registration_trivial_rules() {
        let (_, shared) = registration(|_, _| {});
        let p = ProRegTxV3 {
            shares: vec![],
            join_sigs: vec![],
            early_period_blocks: 0,
            early_penalty: 0,
            owner_key_id: [0x21; 20],
            payouts: vec![PayoutShare {
                script: p2pkh([0x45; 20]),
                reward: 10_000,
            }],
            ..shared
        };
        assert_eq!(check_registration_trivial(&p), Ok(()));
        let payout = |id: u8, reward| PayoutShare {
            script: p2pkh([id; 20]),
            reward,
        };
        let cases: &[(Reject, Edit)] = &[
            ("bad-protx-shares-empty-fields", |p| {
                p.early_period_blocks = 1
            }),
            ("bad-protx-shares-empty-fields", |p| p.early_penalty = 1),
            ("bad-protx-key-null", |p| p.owner_key_id = [0; 20]),
            ("bad-protx-payouts-count", |p| p.payouts.clear()),
            ("bad-protx-payouts-count", |p| {
                p.payouts = (0..9)
                    .map(|i| PayoutShare {
                        script: p2pkh([0x50 + i; 20]),
                        reward: 1_000,
                    })
                    .collect()
            }),
            ("bad-protx-payout-reward", |p| p.payouts[0].reward = 99),
            ("bad-protx-payee", |p| {
                p.payouts[0].script = ScriptBuf::from_bytes(vec![0x51])
            }),
            ("bad-protx-payee-dup", |p| {
                p.payouts[0].reward = 5_000;
                p.payouts.push(p.payouts[0].clone());
            }),
            ("bad-protx-payee-reuse", |p| {
                p.payouts[0].script = p2pkh([0x21; 20])
            }),
            ("bad-protx-payee-reuse", |p| {
                p.payouts[0].script = p2pkh(VOTING)
            }),
            ("bad-protx-payout-reward-sum", |p| {
                p.payouts[0].reward = 9_999
            }),
        ];
        for (reason, edit) in cases {
            let mut q = p.clone();
            edit(&mut q);
            assert_eq!(check_registration_trivial(&q), Err(*reason), "{reason}");
        }
        let mut two = p.clone();
        two.payouts = vec![payout(0x46, 2_500), payout(0x47, 7_500)];
        assert_eq!(check_registration_trivial(&two), Ok(()));
    }

    #[test]
    fn test_QT_126_evo_net_info_needs_its_platform_entries() {
        let (_, p) = registration(make_evo);
        assert_eq!(check_registration_trivial(&p), Ok(()));
        let mut q = p.clone();
        q.net_info.entries.truncate(2);
        assert_eq!(
            check_registration_trivial(&q),
            Err("bad-protx-netinfo-empty")
        );
        // A domain for Platform HTTPS is trivially fine.
        let mut q = p.clone();
        q.net_info.entries[2].1 = vec![NetInfoEntry::Domain {
            name: "node.dash.org".into(),
            port: 443,
        }];
        assert_eq!(check_registration_trivial(&q), Ok(()));
    }

    fn other_mn(
        service: Option<&str>,
        operator: [u8; 48],
        collateral: Option<OutPoint>,
    ) -> ListedMasternode {
        ListedMasternode {
            pro_tx_hash: [0xBB; 32],
            service: service.map(|s| s.parse().unwrap()),
            net_info: service
                .map(|s| {
                    crate::rules::listed_net_info(
                        &dashcore::sml::masternode_list_entry::MasternodeNetInfo::Legacy(
                            s.parse().unwrap(),
                        ),
                    )
                })
                .unwrap_or_default(),
            operator_public_key: operator,
            voting_key_id: [0; 20],
            platform_node_id: None,
            collateral,
        }
    }

    const UNRELATED: [u8; 48] = [0x88; 48];

    #[test]
    fn test_QT_126_shared_registration_contextual_rules_in_core_order() {
        let net_id = Network::Testnet;
        let (tx, p) = registration(|_, _| {});
        let check = |tx: &SpecialTx, p: &ProRegTxV3, list: &[ListedMasternode]| {
            check_registration(tx, p, list, true, net_id, true)
        };
        assert_eq!(check(&tx, &p, &[]), Ok(()));

        // GetValidatedPayload's type and version, then the trivial rules.
        let mut t = tx.clone();
        t.tx_type = 2;
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-type"));
        assert_eq!(
            check_registration(&tx, &p, &[], false, net_id, true),
            Err("bad-protx-version")
        );
        let mut q = p.clone();
        q.shares[0].amount -= 1;
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-shares-amount-sum"));

        // An Evo shared registration, trivially valid.
        let (t, q) = registration(make_evo);
        assert_eq!(check_registration_trivial(&q), Ok(()));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-shares-evo"));

        // The network fields: Core's v3 rules on the network (D5).
        let (t, q) = registration(|_, p| p.net_info = net(&["10.0.0.1:19999"]));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-addr-unroutable"));
        assert_eq!(
            check_registration(&t, &q, &[], true, Network::Regtest, true),
            Ok(())
        );
        let (t, q) = registration(|_, p| p.net_info = net(&["8.8.8.8:8333"]));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-port"));
        let (t, q) = registration(|_, p| p.net_info = net(&["8.8.8.8:19999", "8.8.8.8:19998"]));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-dup-netinfo-entry"));
        // Five Core P2P entries, which the payload's bytes carry to the
        // rules as Core's reader does (D67), and a domain outside Platform
        // HTTPS.
        let (t, q) = registration(|_, p| {
            p.net_info = net(&[
                "8.8.8.1:19999",
                "8.8.8.2:19999",
                "8.8.8.3:19999",
                "8.8.8.4:19999",
            ]);
            p.net_info.entries[0].1.extend(entries("8.8.8.5:19999"));
        });
        assert_eq!(check_registration_trivial(&q), Ok(()));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-maxlimit"));
        let q = ProRegTxV3::from_payload(&q.to_payload().unwrap()).unwrap();
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-maxlimit"));
        let (t, q) = registration(|_, p| {
            p.net_info.entries[0].1.push(NetInfoEntry::Domain {
                name: "node.dash.org".into(),
                port: 19999,
            })
        });
        assert_eq!(check_registration_trivial(&q), Ok(()));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-entry"));
        // An invalid address is an invalid entry to Core's reader, refused
        // trivially before `CheckNetInfo`'s `bad-protx-netinfo-addr`.
        let (t, q) = registration(|_, p| p.net_info = net(&["[2001:db8::1]:19999"]));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-netinfo-bad"));
        // An empty service is allowed (the masternode starts PoSe-banned).
        let (t, q) = registration(|_, p| p.net_info = ExtNetInfo::default());
        assert_eq!(check(&t, &q, &[]), Ok(()));

        // The collateral: index, amount, the template script exactly.
        let mut q = p.clone();
        q.collateral.vout = 2;
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-collateral-index"));
        let mut t = tx.clone();
        t.output[1].value -= 1;
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-collateral"));
        let mut t = tx.clone();
        t.output[1].value = 4_000 * COIN;
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-collateral"));
        let mut t = tx.clone();
        t.output[1].script_pubkey = p2pkh([0x46; 20]);
        assert_eq!(
            check(&t, &p, &[]),
            Err("bad-protx-shares-collateral-script")
        );
        let mut t = tx.clone();
        let mut longer = SHARED_COLLATERAL_SCRIPT.to_vec();
        longer.push(0x51);
        t.output[1].script_pubkey = ScriptBuf::from_bytes(longer);
        assert_eq!(
            check(&t, &p, &[]),
            Err("bad-protx-shares-collateral-script")
        );

        // The list: a service another collateral's masternode holds, in
        // either form; this collateral's own entry may keep it.
        for held in ["8.8.8.8:19999", "[::ffff:8.8.8.8]:19999"] {
            assert_eq!(
                check(&tx, &p, &[other_mn(Some(held), UNRELATED, None)]),
                Err("bad-protx-dup-netinfo-entry"),
                "{held}"
            );
        }
        let ours = OutPoint::new(tx.txid(), 1);
        assert_eq!(
            check(
                &tx,
                &p,
                &[other_mn(Some("8.8.8.8:19999"), UNRELATED, Some(ours))]
            ),
            Ok(())
        );
        assert_eq!(
            check(&tx, &p, &[other_mn(Some("8.8.8.8:19998"), UNRELATED, None)]),
            Ok(())
        );
        // Any entry of another masternode, whatever its purpose (MG-04b
        // D39); another network's address or a domain isn't the service.
        let holding = |net_info: Vec<NetInfoEntry>| ListedMasternode {
            net_info,
            ..other_mn(Some("8.8.8.7:19999"), UNRELATED, None)
        };
        let second = [entries("8.8.8.7:19999"), entries("8.8.8.8:19999")].concat();
        assert_eq!(
            check(&tx, &p, &[holding(second)]),
            Err("bad-protx-dup-netinfo-entry")
        );
        let tor = NetInfoEntry::Service(super::super::netinfo::ServiceAddr {
            network: super::super::netinfo::BIP155_TORV3,
            address: vec![0x11; 32],
            port: 19999,
        });
        let domain = NetInfoEntry::Domain {
            name: "8.8.8.8".into(),
            port: 19999,
        };
        assert_eq!(check(&tx, &p, &[holding(vec![tor, domain])]), Ok(()));
        // The operator key under either scheme, even for the same collateral.
        let legacy = crate::bls::legacy_of(&crate::bls::TEST_OPERATOR_KEY).unwrap();
        for key in [crate::bls::TEST_OPERATOR_KEY, legacy] {
            assert_eq!(
                check(&tx, &p, &[other_mn(Some("8.8.8.8:19999"), key, Some(ours))]),
                Err("bad-protx-dup-key")
            );
        }
        assert_eq!(
            check(
                &tx,
                &p,
                &[other_mn(
                    Some("8.8.8.8:19999"),
                    crate::bls::TEST_OPERATOR_KEY,
                    None
                )]
            ),
            Err("bad-protx-dup-netinfo-entry")
        );

        // The inputs hash, an empty vchSig, then every join signature.
        let mut q = p.clone();
        q.inputs_hash = [0; 32];
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-inputs-hash"));
        let mut q = p.clone();
        q.sig = vec![0; 65];
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-sig"));
        let mut q = p.clone();
        q.join_sigs.swap(0, 1);
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-shares-sig"));
        assert_eq!(
            check_registration(&tx, &q, &[], true, net_id, false),
            Ok(())
        );
        let mut q = p.clone();
        q.join_sigs[2] = NULL_JOIN_SIGNATURE;
        assert_eq!(check(&tx, &q, &[]), Err("bad-protx-shares-sig"));
        // A join signature commits to every output.
        let mut t = tx.clone();
        t.output[0].value += 1;
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-shares-sig"));
    }

    /// A non-shared version-3 registration funding its own collateral at
    /// index 1, after `edit`.
    fn plain_registration(
        edit: impl FnOnce(&mut SpecialTx, &mut ProRegTxV3),
    ) -> (SpecialTx, ProRegTxV3) {
        registration(|t, p| {
            p.shares.clear();
            p.early_period_blocks = 0;
            p.early_penalty = 0;
            p.owner_key_id = [0x21; 20];
            p.payouts = vec![PayoutShare {
                script: p2pkh([0x45; 20]),
                reward: 10_000,
            }];
            t.output[1].script_pubkey = p2pkh([0x52; 20]);
            edit(t, p);
        })
    }

    #[test]
    fn test_QT_126_non_shared_v3_registration_contextual_rules() {
        let net_id = Network::Testnet;
        let check = |tx: &SpecialTx, p: &ProRegTxV3, list: &[ListedMasternode]| {
            check_registration(tx, p, list, true, net_id, true)
        };
        let (tx, p) = plain_registration(|_, _| {});
        assert_eq!(check(&tx, &p, &[]), Ok(()));

        let mut t = tx.clone();
        t.output[1].script_pubkey = ScriptBuf::from_bytes(vec![0x51]);
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-collateral-dest"));
        for reused in [[0x21; 20], VOTING] {
            let mut t = tx.clone();
            t.output[1].script_pubkey = p2pkh(reused);
            assert_eq!(check(&t, &p, &[]), Err("bad-protx-collateral-reuse"));
        }
        let mut t = tx.clone();
        t.output[1].script_pubkey = p2pkh([0x45; 20]);
        assert_eq!(check(&t, &p, &[]), Err("bad-protx-payee-reuse"));
        // A P2PK collateral's destination is its key's hash.
        let sk = SecretKey::from_secret_bytes(secret(0)).unwrap();
        let key = dashcore::PublicKey::new(sk.public_key()).to_bytes();
        let mut p2pk = vec![33];
        p2pk.extend_from_slice(&key);
        p2pk.push(0xac);
        let mut t = tx.clone();
        t.output[1].script_pubkey = ScriptBuf::from_bytes(p2pk);
        assert_eq!(check(&t, &p, &[]), Ok(()));
        let mut q = p.clone();
        q.payouts[0].script = p2pkh(owner_id(0));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-payee-reuse"));
        // An existing collateral: the coin and its signature are Core's to see.
        let mut q = p.clone();
        q.collateral = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
        q.sig = vec![1; 65];
        assert_eq!(check(&tx, &q, &[]), Ok(()));

        // EvoNodes: one payout before `evo_shares`, a node id, unique.
        let (t, q) = plain_registration(|t, p| {
            make_evo(t, p);
            p.payouts = vec![
                PayoutShare {
                    script: p2pkh([0x46; 20]),
                    reward: 5_000,
                },
                PayoutShare {
                    script: p2pkh([0x47; 20]),
                    reward: 5_000,
                },
            ];
        });
        assert_eq!(check_registration_trivial(&q), Ok(()));
        assert_eq!(check(&t, &q, &[]), Err("bad-protx-payouts-evo"));
        let (t, q) = plain_registration(make_evo);
        assert_eq!(check(&t, &q, &[]), Ok(()));
        let mut z = q.clone();
        z.platform_node_id = Some([0; 20]);
        assert_eq!(check(&t, &z, &[]), Err("bad-protx-platform-nodeid"));
        let holder = ListedMasternode {
            platform_node_id: Some([0x31; 20]),
            ..other_mn(None, UNRELATED, None)
        };
        assert_eq!(
            check(&t, &q, &[holder]),
            Err("bad-protx-dup-platformnodeid")
        );
    }
}
