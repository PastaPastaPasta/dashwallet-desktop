//! Sign / verify message (dw-message; QT-099/100). Owner: E2 (signing uses
//! B's VaultSigner). Contract: docs/contracts/m1-engine.md §message.

use dw_message::VerifyError;

use crate::api::common::{domain_error_common, not_implemented, parse_wallet_id};
use crate::{DashNetwork, NetworkSession};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MessageError {
    /// Code `message.invalid_address`: does not decode on the network.
    #[error("invalid address")]
    InvalidAddress,
    /// Code `message.address_no_key`: valid but not P2PKH.
    #[error("address does not refer to a key")]
    AddressNoKey,
    /// Code `message.malformed_signature`: not strict base64.
    #[error("malformed signature")]
    MalformedSignature,
    /// Code `message.pubkey_not_recovered`: the signature does not match the
    /// message digest.
    #[error("public key not recovered")]
    PubkeyNotRecovered,
    /// Code `message.not_signed`: recovered key does not own the address.
    #[error("message verification failed")]
    NotSigned,
    /// Code `message.address_not_mine`: signing with an address the wallet
    /// does not own.
    #[error("address not in wallet")]
    AddressNotMine,
    /// Code `message.watch_only`.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `message.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `message.grant_invalid`: missing, expired or not a SignMessage grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(MessageError);

impl MessageError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidAddress => "message.invalid_address",
            Self::AddressNoKey => "message.address_no_key",
            Self::MalformedSignature => "message.malformed_signature",
            Self::PubkeyNotRecovered => "message.pubkey_not_recovered",
            Self::NotSigned => "message.not_signed",
            Self::AddressNotMine => "message.address_not_mine",
            Self::WatchOnly => "message.watch_only",
            Self::VaultLocked => "message.vault_locked",
            Self::GrantInvalid => "message.grant_invalid",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<VerifyError> for MessageError {
    fn from(e: VerifyError) -> Self {
        match e {
            VerifyError::InvalidAddress => Self::InvalidAddress,
            VerifyError::AddressNoKey => Self::AddressNoKey,
            VerifyError::MalformedSignature => Self::MalformedSignature,
            VerifyError::PubkeyNotRecovered => Self::PubkeyNotRecovered,
            VerifyError::NotSigned => Self::NotSigned,
        }
    }
}

/// `verifymessage`: `Ok(())` when `signature` (base64, 65-byte compact) is
/// `address`'s signature over `message` (UTF-8). No wallet needed (QT-100).
#[uniffi::export]
pub fn verify_message(
    network: DashNetwork,
    address: String,
    message: String,
    signature: String,
) -> Result<(), MessageError> {
    let network = dw_engine::DashNetwork::from(network).core_network();
    Ok(dw_message::verify_message(
        &address,
        &signature,
        message.as_bytes(),
        network,
    )?)
}

#[uniffi::export]
impl NetworkSession {
    /// `signmessage`: base64 compact signature of `message` by `address`'s
    /// key (QT-099). Needs a `SignMessage` grant.
    pub async fn sign_message(
        &self,
        wallet_id: String,
        address: String,
        message: String,
        grant_id: String,
    ) -> Result<String, MessageError> {
        let _ = (parse_wallet_id(&wallet_id)?, address, message, grant_id);
        not_implemented("NetworkSession.sign_message")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_dashd_signature() {
        // testdata/message_cases.json, produced by dashd 24 signmessagewithprivkey.
        let (address, message, signature) = (
            "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n",
            "Trust no one",
            "IIOzMDkvw3GtLWXkeEYRRRH53MOLHM44sJ428Nu4NNacTPJTGcKesMJ+3s3OadYK34tpSQIhu922EviNNWTsiQg=",
        );
        verify_message(
            DashNetwork::Regtest,
            address.into(),
            message.into(),
            signature.into(),
        )
        .unwrap();
        let r = verify_message(
            DashNetwork::Regtest,
            address.into(),
            "Trust everyone".into(),
            signature.into(),
        );
        assert!(matches!(r, Err(MessageError::NotSigned)), "{r:?}");
    }

    #[test]
    fn verify_maps_errors() {
        let r = verify_message(
            DashNetwork::Mainnet,
            "not-an-address".into(),
            "hello".into(),
            "AAAA".into(),
        );
        assert!(matches!(r, Err(MessageError::InvalidAddress)), "{r:?}");
        let r = verify_message(
            DashNetwork::Mainnet,
            "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg".into(),
            "hello".into(),
            "not base64!".into(),
        );
        assert!(matches!(r, Err(MessageError::MalformedSignature)), "{r:?}");
    }
}
