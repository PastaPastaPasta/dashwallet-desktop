//! The credits cost table (DP1-06, DASHPAY F8): what each DashPay action
//! costs at the network's protocol version, and the low-credit levels it
//! sets.
//!
//! Every price is the network's. The protocol version is the one the SDK
//! last proved (each verified response's metadata ratchets it;
//! `refresh_protocol_version` asks for it), and its `FeeVersion` prices:
//!
//! - a document action at Drive's own estimate of creating the document
//!   (`drive::drive::document::cost`, the estimator wasm-sdk exposes): every
//!   element the insert writes at the version's per-byte rates, the
//!   batch's processing and the contract's action fees, for the document at
//!   the largest sizes the DashPay contract allows, every index value new.
//!   The estimator follows protocol version 14's insert methods; below that
//!   it prices version 14's layout at the network's `FeeVersion` (the
//!   DashPay documents use no index feature version 14 added, so the
//!   elements are the same);
//! - the key upgrade at the version's `identity_update` minimum plus its
//!   two keys' stored bytes at the version's per-byte rates (no estimator
//!   prices an identity update; the byte bound is the engine's);
//! - the top-up fee at dpp's own minimum for an `IdentityTopUp`.
//!
//! The write paths' grants (`flows.rs`) stay bounds above these figures;
//! a test holds them above the table at every known protocol version.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, PoisonError};

use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::data_contract::DataContract;
use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::state_transition::StateTransitionEstimatedFeeValidation;
use dpp::state_transition::identity_topup_transition::IdentityTopUpTransition;
use dpp::state_transition::identity_topup_transition::v0::IdentityTopUpTransitionV0;
use dpp::system_data_contracts::{SystemDataContract, load_system_data_contract};
use dpp::version::PlatformVersion;
use drive::drive::document::cost::{CostAssumptions, FieldChoice, document_type_create_cost};

use super::credits::{CostTable, CreditLevel};
use super::errors::PlatformError;
use crate::NetworkSession;

/// The least a top-up funds, in duffs: iOS's custom minimum, 0.01 DASH
/// (DASHPAY F8, IOS-131), unless the network's own floor is higher.
const TOP_UP_MIN_DUFFS: u64 = 1_000_000;
/// One identity key's stored bytes, bounded generously: its serialization,
/// its entries in the identity's key trees and its key-hash reference
/// (E0-04 §4.2's 3.5 KiB for a two-key update).
const KEY_STORED_BYTES: u64 = 1_792;
/// The keys `enable_dashpay_keys` adds: DIP-15's encryption and
/// decryption keys.
const DASHPAY_KEYS: u64 = 2;
/// Android's ratio (`CreditBalanceInfo`): the balance is low at ten of the
/// dearest action.
const LOW_ACTIONS: u64 = 10;

/// The cost table at `protocol_version`, computed once per version.
pub(crate) fn cost_table(protocol_version: u32) -> Result<CostTable, PlatformError> {
    static TABLES: LazyLock<Mutex<HashMap<u32, CostTable>>> = LazyLock::new(Mutex::default);
    let mut tables = TABLES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(table) = tables.get(&protocol_version) {
        return Ok(*table);
    }
    let table = compute(PlatformVersion::get(protocol_version).map_err(internal)?)?;
    tables.insert(protocol_version, table);
    Ok(table)
}

/// Where `balance` stands against `table` (Android `CreditBalanceInfo`):
/// empty below one of the dearest action, low at or below ten of them.
pub(crate) fn credit_level(balance: u64, table: &CostTable) -> CreditLevel {
    if balance < table.empty_credits {
        CreditLevel::Empty
    } else if balance <= table.low_credits {
        CreditLevel::Low
    } else {
        CreditLevel::Ok
    }
}

impl NetworkSession {
    /// The cost table at the network's protocol version, as the SDK last
    /// proved it.
    pub(crate) fn cost_table(&self) -> Result<CostTable, PlatformError> {
        let _op = self.try_enter()?;
        cost_table(self.manager()?.sdk_arc().protocol_version_number())
    }
}

/// The table at `network`, the network's version.
fn compute(network: &PlatformVersion) -> Result<CostTable, PlatformError> {
    let contract =
        load_system_data_contract(SystemDataContract::Dashpay, network).map_err(internal)?;
    let pricing = pricing_version(&contract, network)?;
    let document = |name| document_cost(&contract, name, &pricing);
    let (contact_request, profile_update, contact_info) = (
        document("contactRequest")?,
        document("profile")?,
        document("contactInfo")?,
    );
    let fees = &network.fee_version;
    let per_byte = fees.storage.storage_disk_usage_credit_per_byte
        + fees.storage.storage_processing_credit_per_byte;
    let enable_dashpay_keys =
        fees.state_transition_min_fees.identity_update + DASHPAY_KEYS * KEY_STORED_BYTES * per_byte;
    let top_up_fee = IdentityTopUpTransition::V0(IdentityTopUpTransitionV0::default())
        .calculate_min_required_fee(network)
        .map_err(internal)?;
    let dearest = contact_request.max(profile_update);
    Ok(CostTable {
        protocol_version: network.protocol_version,
        contact_request,
        profile_update,
        contact_info,
        enable_dashpay_keys,
        credits_per_duff: CREDITS_PER_DUFF,
        top_up_fee,
        // A lock always pays its own fee.
        top_up_min_duffs: TOP_UP_MIN_DUFFS.max(top_up_fee.div_ceil(CREDITS_PER_DUFF)),
        low_credits: LOW_ACTIONS * dearest,
        empty_credits: dearest,
    })
}

/// The version the estimator prices with: `network` when it follows its
/// insert methods, else the first later version that does, carrying
/// `network`'s `FeeVersion` (module doc).
fn pricing_version(
    contract: &DataContract,
    network: &PlatformVersion,
) -> Result<PlatformVersion, PlatformError> {
    let probe = contract
        .document_type_for_name("profile")
        .map_err(internal)?;
    let accepts = |v: &PlatformVersion| {
        document_type_create_cost(
            contract,
            probe,
            &BTreeMap::new(),
            &CostAssumptions::new(v),
            v,
        )
        .is_ok()
    };
    if accepts(network) {
        return Ok(network.clone());
    }
    let latest = PlatformVersion::latest().protocol_version;
    for later in network.protocol_version + 1..=latest {
        let layout = PlatformVersion::get(later).map_err(internal)?;
        if accepts(layout) {
            let mut pricing = layout.clone();
            pricing.fee_version = network.fee_version.clone();
            return Ok(pricing);
        }
    }
    Err(PlatformError::Internal {
        detail: format!(
            "no protocol version prices documents at protocol version {}",
            network.protocol_version
        ),
    })
}

/// Creating a `name` document at the largest sizes the contract allows,
/// every optional field present and every index value new.
fn document_cost(
    contract: &DataContract,
    name: &str,
    version: &PlatformVersion,
) -> Result<u64, PlatformError> {
    let document_type = contract.document_type_for_name(name).map_err(internal)?;
    let assumptions = CostAssumptions::new(version);
    let estimate = |choices: &BTreeMap<String, FieldChoice>| {
        document_type_create_cost(contract, document_type, choices, &assumptions, version)
            .map_err(internal)
    };
    let largest = estimate(&BTreeMap::new())?
        .fields
        .into_iter()
        .map(|f| {
            let choice = FieldChoice {
                present: Some(true),
                length: f.max_length,
            };
            (f.path, choice)
        })
        .collect();
    Ok(estimate(&largest)?.total_credits().new_values)
}

fn internal(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Internal {
        detail: format!("the cost table: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::flows::{CONTACT_REQUEST_CREDITS, DOCUMENT_10K_CREDITS, KEY_UPDATE_CREDITS};
    use super::*;

    fn known_versions() -> impl Iterator<Item = u32> {
        PlatformVersion::first().protocol_version..=PlatformVersion::latest().protocol_version
    }

    #[test]
    fn every_known_protocol_version_has_a_table() {
        for v in known_versions() {
            let table = cost_table(v).unwrap_or_else(|e| panic!("protocol version {v}: {e}"));
            assert_eq!(table.protocol_version, v);
            assert!(table.contact_request > 0 && table.contact_info > 0);
            assert_eq!(
                table.empty_credits,
                table.contact_request.max(table.profile_update)
            );
            assert_eq!(table.low_credits, 10 * table.empty_credits);
        }
    }

    /// The grants a write asks for (`flows.rs`) cover what the network
    /// charges for it, at every version the SDK may meet.
    #[test]
    fn the_grant_bounds_cover_the_table() {
        for v in known_versions() {
            let t = cost_table(v).unwrap();
            assert!(t.contact_request <= CONTACT_REQUEST_CREDITS, "{v}: {t:?}");
            assert!(t.profile_update <= DOCUMENT_10K_CREDITS, "{v}: {t:?}");
            assert!(t.contact_info <= DOCUMENT_10K_CREDITS, "{v}: {t:?}");
            assert!(t.enable_dashpay_keys <= KEY_UPDATE_CREDITS, "{v}: {t:?}");
        }
    }

    /// A version below the estimator's own is priced at its own fees: the
    /// same as the estimator's version where the fee versions agree, and
    /// following the fee version where they differ.
    #[test]
    fn a_version_below_the_estimator_is_priced_at_its_own_fees() {
        let (v13, v14) = (
            PlatformVersion::get(13).unwrap(),
            PlatformVersion::get(14).unwrap(),
        );
        let contract = load_system_data_contract(SystemDataContract::Dashpay, v13).unwrap();
        let priced = pricing_version(&contract, v13).unwrap();
        assert_eq!(priced.fee_version, v13.fee_version);
        let cost = |v: &PlatformVersion| document_cost(&contract, "contactRequest", v).unwrap();
        if v13.fee_version.storage == v14.fee_version.storage
            && v13.fee_version.processing == v14.fee_version.processing
            && v13.fee_version.signature == v14.fee_version.signature
            && v13.fee_version.hashing == v14.fee_version.hashing
        {
            assert_eq!(cost(&priced), cost(v14));
        }
        let mut dearer = priced.clone();
        dearer
            .fee_version
            .storage
            .storage_disk_usage_credit_per_byte *= 2;
        assert!(cost(&dearer) > cost(&priced));
    }

    /// Every price follows the fee version: none is a constant in disguise.
    #[test]
    fn the_table_follows_the_fee_version() {
        let v = PlatformVersion::latest();
        let contract = load_system_data_contract(SystemDataContract::Dashpay, v).unwrap();
        let base = document_cost(&contract, "profile", v).unwrap();
        let mut processing = v.clone();
        processing
            .fee_version
            .storage
            .storage_processing_credit_per_byte += 1;
        let mut storage = v.clone();
        storage
            .fee_version
            .storage
            .storage_disk_usage_credit_per_byte += 1;
        let mut signature = v.clone();
        signature
            .fee_version
            .signature
            .verify_signature_ecdsa_secp256k1 += 1_000_000;
        for changed in [processing, storage, signature] {
            assert!(document_cost(&contract, "profile", &changed).unwrap() > base);
        }
        let mut key = v.clone();
        key.fee_version.state_transition_min_fees.identity_update += 1;
        let table = |v: &PlatformVersion| compute(v).unwrap();
        assert!(table(&key).enable_dashpay_keys > table(v).enable_dashpay_keys);
    }

    /// The table prices documents at their largest: a profile with a full
    /// avatar URL costs more than the estimator's middle sizes.
    #[test]
    fn documents_are_priced_at_their_largest() {
        let v = PlatformVersion::latest();
        let contract = load_system_data_contract(SystemDataContract::Dashpay, v).unwrap();
        let document_type = contract.document_type_for_name("profile").unwrap();
        let middle = document_type_create_cost(
            &contract,
            document_type,
            &BTreeMap::new(),
            &CostAssumptions::new(v),
            v,
        )
        .unwrap()
        .total_credits()
        .new_values;
        assert!(document_cost(&contract, "profile", v).unwrap() > middle);
    }

    #[test]
    fn levels_follow_android() {
        let t = cost_table(PlatformVersion::latest().protocol_version).unwrap();
        assert_eq!(credit_level(t.empty_credits - 1, &t), CreditLevel::Empty);
        assert_eq!(credit_level(t.empty_credits, &t), CreditLevel::Low);
        assert_eq!(credit_level(t.low_credits, &t), CreditLevel::Low);
        assert_eq!(credit_level(t.low_credits + 1, &t), CreditLevel::Ok);
    }

    #[test]
    fn the_top_up_minimum_covers_the_network_floor() {
        for v in known_versions() {
            let t = cost_table(v).unwrap();
            assert!(t.top_up_min_duffs >= TOP_UP_MIN_DUFFS);
            assert!(t.top_up_min_duffs * t.credits_per_duff >= t.top_up_fee);
        }
    }
}
