//! The M1–M3 engine errors (m1–m3 contracts): `EngineError`'s own codes and
//! the vault's. The nested failure domains (`send.*`, `labels.*`, …) keep
//! the strings dw-ffi gives them, so the frozen Swift shells and this
//! binding name each failure the same.

use dw_engine::EngineError;
use dw_vault::VaultError;

use super::{AppError, ErrorData, codes};

codes! {
    /// `EngineError::code()`, and the common codes other domains share.
    EngineCode {
        InvalidConfig = "invalid_config",
        InvalidArgument = "invalid_argument",
        NetworkNotOpen = "network_not_open",
        StorageInUse = "storage_in_use",
        Storage = "storage",
        WalletNotFound = "wallet_not_found",
        InvalidMnemonic = "invalid_mnemonic",
        WalletAlreadyExists = "wallet_already_exists",
        Wallet = "wallet",
        Sdk = "sdk",
        InsufficientCredits = "insufficient_credits",
        Spv = "spv",
        Io = "io",
        NotImplemented = "not_implemented",
        SpvNotRunning = "spv_not_running",
        SpvRunning = "spv_running",
        RescanInProgress = "rescan_in_progress",
        InvalidXpub = "invalid_xpub",
        TxActionRefused = "tx_action_refused",
        NoPeers = "no_peers",
        HeightOutOfRange = "height_out_of_range",
        InvalidQuery = "invalid_query",
        StaleCursor = "stale_cursor",
        TxNotFound = "tx_not_found",
        GapLimit = "gap_limit",
        RequestNotFound = "request_not_found",
        NameRejected = "name_rejected",
        InvalidAddress = "invalid_address",
        AddressNoKey = "address_no_key",
        AddressNotMine = "address_not_mine",
        Internal = "internal",
        OutpointNotFound = "outpoint_not_found",
    }
}

codes! {
    VaultCode {
        NoVault = "vault.no_vault",
        AlreadyExists = "vault.already_exists",
        Locked = "vault.locked",
        WrongPassphrase = "vault.wrong_passphrase",
        Throttled = "vault.throttled",
        PassphraseRejected = "vault.passphrase_rejected",
        NotEncrypted = "vault.not_encrypted",
        AlreadyEncrypted = "vault.already_encrypted",
        GrantInvalid = "vault.grant_invalid",
        GrantPurposeMismatch = "vault.grant_purpose_mismatch",
        CredentialRequired = "vault.credential_required",
        MixingOnly = "vault.mixing_only",
        NoSecret = "vault.no_secret",
        QuickUnlockUnavailable = "vault.quick_unlock_unavailable",
        QuickUnlockLimitExceeded = "vault.quick_unlock_limit_exceeded",
        PassphraseStale = "vault.passphrase_stale",
        NotEmpty = "vault.not_empty",
        RecoveryMismatch = "vault.recovery_mismatch",
        OsStoreUnavailable = "vault.os_store_unavailable",
        Corrupt = "vault.corrupt",
    }
}

impl From<VaultError> for AppError {
    fn from(e: VaultError) -> Self {
        use VaultError as V;
        let message = e.to_string();
        let (code, data): (super::ErrorCode, _) = match e {
            V::NoVault => (VaultCode::NoVault.into(), None),
            V::AlreadyExists => (VaultCode::AlreadyExists.into(), None),
            V::Locked => (VaultCode::Locked.into(), None),
            V::WrongPassphrase {
                retry_after_secs, ..
            } => (
                VaultCode::WrongPassphrase.into(),
                retry_after_secs.map(|secs| ErrorData::RetryAfter { secs }),
            ),
            V::Throttled { retry_after_secs } => (
                VaultCode::Throttled.into(),
                Some(ErrorData::RetryAfter {
                    secs: retry_after_secs,
                }),
            ),
            V::PassphraseRejected(_) => (VaultCode::PassphraseRejected.into(), None),
            V::NotEncrypted => (VaultCode::NotEncrypted.into(), None),
            V::AlreadyEncrypted => (VaultCode::AlreadyEncrypted.into(), None),
            V::GrantInvalid => (VaultCode::GrantInvalid.into(), None),
            V::GrantPurposeMismatch => (VaultCode::GrantPurposeMismatch.into(), None),
            V::CredentialRequired => (VaultCode::CredentialRequired.into(), None),
            V::MixingOnly => (VaultCode::MixingOnly.into(), None),
            V::NoSecret => (VaultCode::NoSecret.into(), None),
            V::QuickUnlockUnavailable => (VaultCode::QuickUnlockUnavailable.into(), None),
            V::QuickUnlockLimitExceeded { limit_duffs } => (
                VaultCode::QuickUnlockLimitExceeded.into(),
                Some(ErrorData::Limit { duffs: limit_duffs }),
            ),
            V::PassphraseStale => (VaultCode::PassphraseStale.into(), None),
            V::NotEmpty => (VaultCode::NotEmpty.into(), None),
            V::RecoveryMismatch => (VaultCode::RecoveryMismatch.into(), None),
            V::OsStoreUnavailable(_) => (VaultCode::OsStoreUnavailable.into(), None),
            V::Corrupt(_) => (VaultCode::Corrupt.into(), None),
            V::InvalidArgument(_) => (EngineCode::InvalidArgument.into(), None),
            V::Storage(_) => (EngineCode::Storage.into(), None),
            V::NotImplemented(_) => (EngineCode::NotImplemented.into(), None),
            V::Internal(_) => (EngineCode::Internal.into(), None),
        };
        AppError {
            code,
            message,
            data,
        }
    }
}

impl From<EngineError> for AppError {
    fn from(e: EngineError) -> Self {
        use EngineError as E;
        let message = e.to_string();
        let (code, data): (super::ErrorCode, _) = match e {
            E::InvalidConfig(_) => (EngineCode::InvalidConfig.into(), None),
            E::InvalidArgument(_) => (EngineCode::InvalidArgument.into(), None),
            E::NetworkNotOpen(_) => (EngineCode::NetworkNotOpen.into(), None),
            E::StorageInUse(_) => (EngineCode::StorageInUse.into(), None),
            E::Storage(_) => (EngineCode::Storage.into(), None),
            E::WalletNotFound(_) => (EngineCode::WalletNotFound.into(), None),
            E::InvalidMnemonic(_) => (EngineCode::InvalidMnemonic.into(), None),
            E::WalletAlreadyExists(_) => (EngineCode::WalletAlreadyExists.into(), None),
            E::Wallet(_) => (EngineCode::Wallet.into(), None),
            E::Sdk(_) => (EngineCode::Sdk.into(), None),
            E::InsufficientCredits {
                needed, available, ..
            } => (
                EngineCode::InsufficientCredits.into(),
                Some(ErrorData::Credits { needed, available }),
            ),
            E::Spv(_) => (EngineCode::Spv.into(), None),
            E::Io(_) => (EngineCode::Io.into(), None),
            E::NotImplemented(call) => (
                EngineCode::NotImplemented.into(),
                Some(ErrorData::Call { call }),
            ),
            E::SpvNotRunning => (EngineCode::SpvNotRunning.into(), None),
            E::SpvRunning => (EngineCode::SpvRunning.into(), None),
            E::RescanInProgress => (EngineCode::RescanInProgress.into(), None),
            E::InvalidXpub(_) => (EngineCode::InvalidXpub.into(), None),
            E::TxActionRefused(refusal) => (
                EngineCode::TxActionRefused.into(),
                Some(ErrorData::TxAction {
                    refusal: refusal.into(),
                }),
            ),
            E::NoPeers => (EngineCode::NoPeers.into(), None),
            E::HeightOutOfRange(h) => (
                EngineCode::HeightOutOfRange.into(),
                Some(ErrorData::Number { value: h.into() }),
            ),
            E::InvalidQuery(_) => (EngineCode::InvalidQuery.into(), None),
            E::StaleCursor => (EngineCode::StaleCursor.into(), None),
            E::TxNotFound(_) => (EngineCode::TxNotFound.into(), None),
            E::GapLimit => (EngineCode::GapLimit.into(), None),
            E::RequestNotFound(id) => (
                EngineCode::RequestNotFound.into(),
                Some(ErrorData::Number { value: id }),
            ),
            E::NameRejected(_) => (EngineCode::NameRejected.into(), None),
            E::Vault(v) => return v.into(),
            E::InvalidAddress(_) => (EngineCode::InvalidAddress.into(), None),
            E::AddressNoKey(_) => (EngineCode::AddressNoKey.into(), None),
            E::AddressNotMine(_) => (EngineCode::AddressNotMine.into(), None),
            E::Internal(_) => (EngineCode::Internal.into(), None),
            E::OutpointNotFound(_) => (EngineCode::OutpointNotFound.into(), None),
            // TODO(E0-13 errors): the nested domains below get their own codes.
            E::Signer(_)
            | E::Send(_)
            | E::Labels(_)
            | E::Compat(_)
            | E::Backup(_)
            | E::Psbt(_)
            | E::CoinJoin(_)
            | E::Masternode(_) => (EngineCode::Internal.into(), None),
        };
        AppError {
            code,
            message,
            data,
        }
    }
}
