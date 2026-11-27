//! App Connect's rules over proved facts, without a network or a key: what
//! a request may ask for, who must own what it names, the terms a user may
//! set, how the funding limit splits into key budgets, the keys a grant
//! registers and the digest a grant is bound to. Not part of the facade.

use std::collections::{BTreeMap, BTreeSet};

use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::identity_public_key::v1::IdentityPublicKeyV1;
use dpp::identity::{IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::BinaryData;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use dw_uri::connect::{AppConnectRequest, Bounds, EncPurpose, Level};
use key_wallet::dip9::ApplicationKeyPurpose;

use super::app_connect::ConnectTerms;
use sha2::{Digest, Sha256};

/// Platform's credits per DASH.
pub(crate) const CREDITS_PER_DASH: u64 = 100_000_000_000;
/// The most a connection may spend (iOS and Android: 5 DASH).
pub(crate) const MAX_FUNDING: u64 = 5 * CREDITS_PER_DASH;
/// A DashPay profile key's share of the limit: about five profile updates.
pub(crate) const PROFILE_SHARE: u64 = CREDITS_PER_DASH / 500;
/// The limit offered when the app asks for no budget.
pub(crate) const DEFAULT_FUNDING: u64 = CREDITS_PER_DASH / 2;
pub(crate) const DAY_MS: u64 = 24 * 60 * 60 * 1000;
pub(crate) const HOUR_MS: u64 = 60 * 60 * 1000;
pub(crate) const MAX_LIFETIME_MS: u64 = 365 * DAY_MS;
pub(crate) const DEFAULT_LIFETIME_MS: u64 = 90 * DAY_MS;
/// App Connect's system contract and its response's lowest protocol.
pub(crate) const MIN_PROTOCOL_VERSION: u32 = 14;
pub(crate) const RESPONSE_TYPE: &str = "loginKeyResponse";
/// The one foreign document type an app may ask a key for.
pub(crate) const PROFILE_TYPE: &str = "profile";
/// The most group members a consent lists; a larger group is refused.
pub(crate) const MAX_GROUP_MEMBERS: usize = 200;

/// What the ownership rules refuse a request for (DIP "Owner
/// consistency"), before anything is signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectRefusal {
    /// A binding this wallet does not grant (a group too large to list).
    Unsupported,
    /// A contract or document type that does not exist, a system contract
    /// as the app's, a contract that did not opt in to encryption.
    Invalid,
    /// A key would cover a contract or group the app's owner does not own.
    Owner,
}

/// A contract as the proved read found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractFacts {
    pub(crate) owner: Identifier,
    pub(crate) document_types: BTreeSet<String>,
    /// `requiresIdentityEncryptionBoundedKey` / `…DecryptionBoundedKey`.
    pub(crate) encryption_requirement: Option<u8>,
    pub(crate) decryption_requirement: Option<u8>,
    pub(crate) has_tokens: bool,
    /// No update can change it, so it can never gain tokens.
    pub(crate) readonly: bool,
    /// Serialized contract size in bytes at the network's protocol version,
    /// used to bound the real contract lookup Drive meters when disabling a
    /// contract-bound key.
    pub(crate) serialized_bytes: usize,
}

/// A contract group's metadata only (`ContractGroupInfo`), fetched without
/// enumerating its members (used to price group-bound key revocation).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GroupInfoFacts {
    pub(crate) owner: Identifier,
    pub(crate) admins: Vec<Identifier>,
    /// Exact serialized byte length of `ContractGroupInfo` (owner, admins,
    /// name and description) as stored by Drive.
    pub(crate) serialized_bytes: usize,
}

/// A contract group as the proved reads found it: owner, admins and every
/// member.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct GroupFacts {
    pub(crate) owner: Identifier,
    pub(crate) admins: Vec<Identifier>,
    pub(crate) contracts: Vec<Identifier>,
    pub(crate) document_types: Vec<(Identifier, String)>,
    pub(crate) tokens: Vec<(Identifier, u16)>,
}

impl GroupFacts {
    pub(crate) fn member_count(&self) -> usize {
        self.contracts.len() + self.document_types.len() + self.tokens.len()
    }
}

/// What the wallet does for an encryption binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) enum EncPlan {
    /// No key of that purpose for the contract: register rotation 0.
    Register,
    /// The identity's current key for the contract, which this wallet
    /// derives at `rotation` (`None`: the legacy path): shared, not
    /// registered again.
    Share {
        key_id: KeyID,
        rotation: Option<u32>,
    },
    /// Keys exist but none is current, or another wallet's: no slot.
    Omit,
}

pub(crate) fn id(bytes: &[u8; 32]) -> Identifier {
    Identifier::from(*bytes)
}

pub(crate) fn is_profile(bounds: &Bounds) -> bool {
    matches!(bounds, Bounds::DocumentType(c, name) if id(c) == dashpay_contract::ID && name == PROFILE_TYPE)
}

/// System contracts an app may not claim as its own.
pub(crate) fn is_system_contract(contract: &Identifier) -> bool {
    use dpp::system_data_contracts::SystemDataContract as S;
    [
        S::Withdrawals,
        S::MasternodeRewards,
        S::FeatureFlags,
        S::DPNS,
        S::Dashpay,
        S::WalletUtils,
        S::TokenHistory,
        S::KeywordSearch,
        S::DocumentHistory,
        S::AppConnect,
        S::ModerationCharters,
    ]
    .into_iter()
    .any(|s| s.id() == *contract)
}

/// The contracts a request's bindings name, the app's included.
pub(crate) fn named_contracts(request: &AppConnectRequest) -> BTreeSet<Identifier> {
    let mut out: BTreeSet<_> = [id(&request.app_id)].into();
    for b in &request.auth_bindings {
        match &b.bounds {
            Bounds::Contract(c) | Bounds::DocumentType(c, _) => {
                out.insert(id(c));
            }
            Bounds::Group(_) => {}
        }
    }
    out
}

pub(crate) fn named_groups(request: &AppConnectRequest) -> BTreeSet<Identifier> {
    (request.auth_bindings.iter())
        .filter_map(|b| match &b.bounds {
            Bounds::Group(g) => Some(id(g)),
            _ => None,
        })
        .collect()
}

/// DIP §"Owner consistency": the app's contract exists and is no system
/// contract; every group and single contract is the app owner's; a
/// document type is the app owner's or DashPay's `profile`, and exists;
/// every encryption binding's contract opted in (0 or 2).
pub(crate) fn check_ownership(
    request: &AppConnectRequest,
    contracts: &BTreeMap<Identifier, Option<ContractFacts>>,
    groups: &BTreeMap<Identifier, Option<GroupFacts>>,
) -> Result<(), ConnectRefusal> {
    let app_id = id(&request.app_id);
    if is_system_contract(&app_id) {
        return Err(ConnectRefusal::Invalid);
    }
    let fact = |c: &Identifier| {
        contracts
            .get(c)
            .cloned()
            .flatten()
            .ok_or(ConnectRefusal::Invalid)
    };
    let app = fact(&app_id)?;
    if app.owner == Identifier::default() {
        return Err(ConnectRefusal::Invalid);
    }
    for binding in &request.auth_bindings {
        match &binding.bounds {
            Bounds::Group(g) => {
                let group = groups.get(&id(g)).cloned().flatten();
                let group = group.ok_or(ConnectRefusal::Invalid)?;
                if group.owner != app.owner {
                    return Err(ConnectRefusal::Owner);
                }
                if group.member_count() > MAX_GROUP_MEMBERS {
                    return Err(ConnectRefusal::Unsupported);
                }
            }
            Bounds::Contract(c) => {
                if fact(&id(c))?.owner != app.owner {
                    return Err(ConnectRefusal::Owner);
                }
            }
            bounds @ Bounds::DocumentType(c, name) => {
                let contract = fact(&id(c))?;
                if !contract.document_types.contains(name) {
                    return Err(ConnectRefusal::Invalid);
                }
                if !is_profile(bounds) && contract.owner != app.owner {
                    return Err(ConnectRefusal::Owner);
                }
            }
        }
    }
    for binding in &request.enc_bindings {
        let contract = fact(&id(&binding.contract))?;
        let requirement = match binding.purpose {
            EncPurpose::Encryption => contract.encryption_requirement,
            EncPurpose::Decryption => contract.decryption_requirement,
        };
        if !matches!(requirement, Some(0 | 2)) {
            return Err(ConnectRefusal::Invalid);
        }
    }
    Ok(())
}

/// The terms the sheet opens with: the app's total ask capped at 5 DASH
/// (half a DASH when it asks for none), and its earliest expiry capped at a
/// year (90 days when it asks for none).
pub(crate) fn default_terms(request: &AppConnectRequest, block_time_ms: u64) -> ConnectTerms {
    let asked: u64 = (request.auth_bindings.iter())
        .map(|b| b.budget.unwrap_or(0).min(MAX_FUNDING))
        .fold(0u64, u64::saturating_add);
    let funding_limit = if asked == 0 {
        DEFAULT_FUNDING
    } else {
        asked.min(MAX_FUNDING)
    };
    let cap = block_time_ms.saturating_add(MAX_LIFETIME_MS);
    let expires_at_ms = (request.auth_bindings.iter())
        .filter_map(|b| b.expires_at_ms)
        .min()
        .unwrap_or(block_time_ms.saturating_add(DEFAULT_LIFETIME_MS))
        .clamp(block_time_ms.saturating_add(DAY_MS), cap);
    ConnectTerms {
        funding_limit,
        expires_at_ms,
    }
}

/// Terms a user may set at `block_time_ms`: a limit above 0 and at most
/// 5 DASH; an expiry more than an hour after block time and at most a year.
pub(crate) fn terms_allowed(terms: &ConnectTerms, block_time_ms: u64) -> bool {
    terms.funding_limit > 0
        && terms.funding_limit <= MAX_FUNDING
        && terms.expires_at_ms > block_time_ms.saturating_add(HOUR_MS)
        && terms.expires_at_ms <= block_time_ms.saturating_add(MAX_LIFETIME_MS)
}

/// Each selected binding's budget, in selection order, for `limit`: the
/// DashPay profile keys share [`PROFILE_SHARE`] (half the limit below
/// twice that, all of it up to the share with no app key); the app's keys
/// share the rest in proportion to their asks, one credit each first, the
/// remainder to the first. `None` when a key would get nothing.
pub(crate) fn key_budgets(
    request: &AppConnectRequest,
    selection: &[u8],
    limit: u64,
) -> Option<Vec<u64>> {
    let bindings: Vec<_> = (selection.iter())
        .map(|&i| request.auth_bindings.get(usize::from(i)))
        .collect::<Option<_>>()?;
    let profile: Vec<usize> = (0..bindings.len())
        .filter(|&i| is_profile(&bindings[i].bounds))
        .collect();
    let app: Vec<usize> = (0..bindings.len())
        .filter(|&i| !is_profile(&bindings[i].bounds))
        .collect();
    let profile_total = match (profile.is_empty(), app.is_empty()) {
        (true, _) => 0,
        (false, true) => limit.min(PROFILE_SHARE),
        (false, false) if limit < 2 * PROFILE_SHARE => limit / 2,
        (false, false) => PROFILE_SHARE,
    };
    let mut budgets = vec![0u64; bindings.len()];
    for (slot, share) in profile
        .iter()
        .zip(even_shares(profile_total, profile.len()))
    {
        budgets[*slot] = share;
    }
    let asks: Vec<u64> = (app.iter())
        .map(|&i| bindings[i].budget.unwrap_or(0).min(MAX_FUNDING))
        .collect();
    for (slot, share) in app
        .iter()
        .zip(weighted_shares(limit - profile_total, &asks))
    {
        budgets[*slot] = share;
    }
    (!budgets.contains(&0)).then_some(budgets)
}

fn even_shares(total: u64, count: usize) -> Vec<u64> {
    if count == 0 {
        return Vec::new();
    }
    let n = count as u64;
    let mut out = vec![total / n; count];
    out[0] += total % n;
    out
}

fn weighted_shares(total: u64, asks: &[u64]) -> Vec<u64> {
    let n = asks.len() as u64;
    if n == 0 || total < n {
        return vec![0; asks.len()];
    }
    let floor = asks.iter().copied().filter(|&a| a > 0).min().unwrap_or(1);
    let weights: Vec<u128> = asks
        .iter()
        .map(|&a| u128::from(if a == 0 { floor } else { a }))
        .collect();
    let sum: u128 = weights.iter().sum();
    let rest = u128::from(total - n);
    let mut out: Vec<u64> = (weights.iter())
        .map(|w| 1 + u64::try_from(rest * w / sum).unwrap_or(0))
        .collect();
    let given: u64 = out.iter().sum();
    out[0] += total - given;
    out
}

/// The session key of the selected binding `index` at key id `key_id`:
/// AUTHENTICATION at the binding's level, `ECDSA_HASH160` of
/// `public_key`, bound as the binding is, with its budget and the expiry.
pub(crate) fn session_key(
    request: &AppConnectRequest,
    index: u8,
    key_id: KeyID,
    public_key: &[u8; 33],
    budget: u64,
    expires_at_ms: u64,
) -> Option<IdentityPublicKey> {
    use dashcore::hashes::hash160;
    let binding = request.auth_bindings.get(usize::from(index))?;
    let security_level = match binding.level {
        Level::Critical => SecurityLevel::CRITICAL,
        Level::High => SecurityLevel::HIGH,
    };
    let contract_bounds = Some(match &binding.bounds {
        Bounds::Contract(c) => ContractBounds::SingleContract { id: id(c) },
        Bounds::DocumentType(c, name) => ContractBounds::SingleContractDocumentType {
            id: id(c),
            document_type_name: name.clone(),
        },
        Bounds::Group(g) => ContractBounds::ContractGroup { id: id(g) },
    });
    let v0 = IdentityPublicKeyV0 {
        id: key_id,
        purpose: Purpose::AUTHENTICATION,
        security_level,
        contract_bounds,
        key_type: KeyType::ECDSA_HASH160,
        read_only: false,
        data: BinaryData::new(hash160::Hash::hash(public_key).to_byte_array().to_vec()),
        disabled_at: None,
    };
    Some(IdentityPublicKey::V1(
        IdentityPublicKeyV1::from_v0_with_limits(v0, Some(budget), Some(expires_at_ms)),
    ))
}

pub(crate) fn enc_purpose(p: EncPurpose) -> (Purpose, ApplicationKeyPurpose) {
    match p {
        EncPurpose::Encryption => (Purpose::ENCRYPTION, ApplicationKeyPurpose::Encryption),
        EncPurpose::Decryption => (Purpose::DECRYPTION, ApplicationKeyPurpose::Decryption),
    }
}

/// A new shared encryption key: MEDIUM, `ECDSA_SECP256K1`, bound to the
/// app's contract, no limits.
pub(crate) fn encryption_key(
    contract: &[u8; 32],
    purpose: EncPurpose,
    key_id: KeyID,
    public_key: &[u8; 33],
) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: key_id,
        purpose: enc_purpose(purpose).0,
        security_level: SecurityLevel::MEDIUM,
        contract_bounds: Some(ContractBounds::SingleContract { id: id(contract) }),
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: BinaryData::new(public_key.to_vec()),
        disabled_at: None,
    })
}

/// What one approval commits to, and so what its grant names: the request,
/// the identity, the selection and terms, every fact consent showed, and
/// what is done for each encryption binding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Commitment<'a> {
    pub(crate) request: &'a [u8],
    pub(crate) identity: Identifier,
    pub(crate) selection: &'a [u8],
    pub(crate) terms: ConnectTerms,
    pub(crate) budgets: &'a [u64],
    pub(crate) app_owner: Identifier,
    pub(crate) groups: &'a BTreeMap<Identifier, Option<GroupFacts>>,
    pub(crate) encryption: &'a [EncPlan],
}

impl Commitment<'_> {
    pub(crate) fn digest(&self) -> Option<[u8; 32]> {
        let bytes = serde_json::to_vec(self).ok()?;
        let mut h = Sha256::new();
        h.update(b"dashwallet-desktop app-connect plan v1\0");
        h.update(&bytes);
        Some(h.finalize().into())
    }
}

/// The digest of the group facts one read found, so the sheet that shows
/// them can tell a quote that read others.
pub(crate) fn groups_digest(groups: &BTreeMap<Identifier, Option<GroupFacts>>) -> Option<[u8; 32]> {
    let bytes = serde_json::to_vec(groups).ok()?;
    let mut h = Sha256::new();
    h.update(b"dashwallet-desktop app-connect groups v1\0");
    h.update(&bytes);
    Some(h.finalize().into())
}
