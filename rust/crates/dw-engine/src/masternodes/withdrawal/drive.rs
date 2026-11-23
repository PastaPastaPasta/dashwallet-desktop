//! Drive's side of a masternode withdrawal, mirrored (MG-13 F3, P2, P7):
//! which identity key signs, where an owner-key withdrawal pays, and which
//! documents a record's execution could have written.

use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyType, Purpose};
use dpp::platform_value::btreemap_extensions::BTreeValueMapHelper;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::withdrawals_contract::v1::document_types::withdrawal;
use dw_appdb::WithdrawalRow;

use super::WithdrawalError;
use crate::platform::withdrawal::amount_of;

/// Where Drive pays an owner-key withdrawal, which carries no output script
/// (F3), or why it refuses one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Window {
    /// The script, and the id of the TRANSFER key it comes from.
    Pays { script: CoreScript, key: u32 },
    /// No TRANSFER key: `MissingTransferKeyError`.
    NoTransferKey,
    /// The highest and the next 5 all disabled:
    /// `IdentityPublicKeyIsDisabledError`.
    Disabled,
    /// The chosen key is neither ECDSA_HASH160 nor BIP13_SCRIPT_HASH:
    /// `NoTransferKeyForCoreWithdrawalAvailableError`.
    OtherType,
}

impl Window {
    /// The window's script, or P2's `withdrawal_no_payout_key`.
    pub(crate) fn script(&self) -> Result<&CoreScript, WithdrawalError> {
        match self {
            Self::Pays { script, .. } => Ok(script),
            Self::NoTransferKey => Err(WithdrawalError::NoPayoutKey("no transfer key")),
            Self::Disabled => Err(WithdrawalError::NoPayoutKey("every transfer key disabled")),
            Self::OtherType => Err(WithdrawalError::NoPayoutKey(
                "a transfer key of a type Core can't be paid to",
            )),
        }
    }
}

/// The identity's TRANSFER keys in descending key id, Drive's
/// `RecentWithdrawalKeys` (`rs-drive/src/drive/identity/key/fetch/mod.rs:1006-1017`:
/// the transfer-keys tree, all of it, right to left). Disabled keys stay
/// in it: Drive disables keys, never deletes them (F3).
fn transfer_keys_descending(identity: &Identity) -> Vec<&IdentityPublicKey> {
    let mut keys: Vec<_> = (identity.public_keys().values())
        .filter(|k| k.purpose() == Purpose::TRANSFER)
        .collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.id()));
    keys
}

/// Drive's owner-key destination, exactly as
/// `rs-drive/src/state_transition_action/identity/identity_credit_withdrawal/v0/transformer.rs:84-175`
/// picks it: the highest-id TRANSFER key; if it is disabled, the highest
/// enabled one among the next 5 (`limit 5, offset 1`, read back highest
/// first); P2PKH of an ECDSA_HASH160 key's data, P2SH of a
/// BIP13_SCRIPT_HASH key's, and its refusal for any other type.
pub(crate) fn window(identity: &Identity) -> Window {
    let keys = transfer_keys_descending(identity);
    let Some(first) = keys.first() else {
        return Window::NoTransferKey;
    };
    let key = if first.is_disabled() {
        match keys.iter().skip(1).take(5).find(|k| !k.is_disabled()) {
            Some(k) => *k,
            None => return Window::Disabled,
        }
    } else {
        first
    };
    match script_of(key) {
        Some(script) => Window::Pays {
            script,
            key: key.id(),
        },
        None => Window::OtherType,
    }
}

/// The script Drive pays for TRANSFER key `key` (`transformer.rs:139-158`):
/// P2PKH for ECDSA_HASH160, P2SH for BIP13_SCRIPT_HASH, none for any other
/// type (or data that is no hash160, where `public_key_hash()` fails).
pub(crate) fn script_of(key: &IdentityPublicKey) -> Option<CoreScript> {
    let hash = <[u8; 20]>::try_from(key.data().as_slice()).ok()?;
    match key.key_type() {
        KeyType::ECDSA_HASH160 => Some(CoreScript::new_p2pkh(hash)),
        KeyType::BIP13_SCRIPT_HASH => Some(CoreScript::new_p2sh(hash)),
        _ => None,
    }
}

/// The hash160s of the identity's ECDSA_HASH160 keys of `purpose`, enabled
/// or not: what the wallet's keys are looked up by (P2).
pub(crate) fn key_hashes(identity: &Identity, purpose: Purpose) -> Vec<[u8; 20]> {
    (identity.public_keys().values())
        .filter(|k| k.purpose() == purpose && k.key_type() == KeyType::ECDSA_HASH160)
        .filter_map(|k| <[u8; 20]>::try_from(k.data().as_slice()).ok())
        .collect()
}

/// The identity key a held key signs with (P2's local selector): purpose
/// `purpose` (OWNER or TRANSFER), ECDSA_HASH160, data equal to the key's
/// hash160, and **not disabled**. A match that is disabled, or none, is
/// `withdrawal_key_missing`.
pub(crate) fn identity_key<'a>(
    identity: &'a Identity,
    purpose: Purpose,
    hash: &[u8; 20],
) -> Result<&'a IdentityPublicKey, WithdrawalError> {
    (identity.public_keys().values())
        .filter(|k| {
            k.purpose() == purpose
                && k.key_type() == KeyType::ECDSA_HASH160
                && k.data().as_slice() == hash.as_slice()
        })
        .find(|k| !k.is_disabled())
        .ok_or(WithdrawalError::KeyMissing)
}

/// The scripts a record's execution could have paid (P7 candidates): the
/// payout key's signed script; for an owner-key record, every TRANSFER key
/// of `identity` (proved at or above the floor), disabled ones included,
/// each mapped as Drive maps it, and the destination the quote saw. Keys
/// and documents are never deleted (F3), so these cover every script Drive
/// could have paid by then.
pub(crate) fn candidates(row: &WithdrawalRow, identity: Option<&Identity>) -> Vec<CoreScript> {
    let mut scripts = Vec::new();
    let mut add = |s: CoreScript| {
        if !scripts.contains(&s) {
            scripts.push(s);
        }
    };
    if let Some(script) = &row.signed.script {
        add(CoreScript::from_bytes(script.clone()));
        return scripts;
    }
    if let Some(identity) = identity {
        for key in transfer_keys_descending(identity) {
            if let Some(s) = script_of(key) {
                add(s);
            }
        }
    }
    if let Some(expected) = &row.signed.expected_script {
        add(CoreScript::from_bytes(expected.clone()));
    }
    scripts
}

/// Whether owner-key candidates need an identity read.
pub(crate) fn needs_identity(row: &WithdrawalRow) -> bool {
    row.signed.script.is_none()
}

/// P7's match: the document at the record's id for `script` holds exactly
/// the record's effect, amount, `coreFeePerByte` and pooling, paid to
/// `script` by the record's identity. A present document with other
/// effect fields is a proved non-match (P8 (iv)).
pub(crate) fn matches(
    doc: &Document,
    row: &WithdrawalRow,
    owner: &Identifier,
    script: &CoreScript,
) -> bool {
    let p = doc.properties();
    let fee = p
        .get_integer::<u32>(withdrawal::properties::CORE_FEE_PER_BYTE)
        .ok();
    let pooling = p.get_integer::<u8>(withdrawal::properties::POOLING).ok();
    let paid = p.get_bytes(withdrawal::properties::OUTPUT_SCRIPT).ok();
    doc.owner_id() == *owner
        && amount_of(doc) == Some(row.signed.amount)
        && fee == Some(row.signed.core_fee_per_byte)
        && pooling == Some(row.signed.pooling)
        && paid.as_deref() == Some(script.as_bytes())
}

/// The document's `status` (Drive's integer), if it has one.
pub(crate) fn document_status(doc: &Document) -> Option<u8> {
    doc.properties()
        .get_integer::<u8>(withdrawal::properties::STATUS)
        .ok()
}
