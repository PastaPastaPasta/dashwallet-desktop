//! The failure domains nested inside `EngineError` (`Signer`, `Send`,
//! `Labels`, `Compat`, `Backup`, `Psbt`, `CoinJoin`, `Masternode`).
//!
//! The strings are the ones dw-ffi gives the same failures
//! (`send.dust_amount`, `compat.corrupt`, `masternode.vault_locked`, …), so
//! the frozen Swift shells and this binding name each failure alike. A
//! domain's `From` is one exhaustive `match`; `EngineError`'s `From`
//! delegates to it.
//!
//! dw-ffi also has per-call spellings that no failure variant produces: it
//! maps `VaultError` to `compat.vault_locked`, `psbt.vault_locked`, … for
//! the calls that need them. A blanket `From` cannot know the call, so here
//! a failure keeps the code of the variant the engine returned (D13): a
//! vault refusal is `vault.*`, and `signPsbt`'s locked vault or bad grant,
//! which the engine reports as `SendFailure`, is `send.vault_locked` /
//! `send.grant_invalid`.

use dw_console::ConsoleFailure;
use dw_engine::{
    BackupFailure, CoinJoinFailure, CompatFailure, LabelsFailure, MasternodeFailure, PsbtFailure,
    SendFailure,
};
use dw_vault::SignerError;

use super::{AppError, ErrorCode, ErrorData, codes};

codes! {
    /// `SignerError`: the vault signer refused or failed. dw-ffi folds these
    /// into other domains' codes (`message.vault_locked`, `wallet`), so the
    /// strings are the engine's own: the domain `signer` plus the variant.
    SignerCode {
        Locked = "signer.locked",
        PathNotAllowed = "signer.path_not_allowed",
        KeyMismatch = "signer.key_mismatch",
        DecryptFailed = "signer.decrypt_failed",
        NoSecret = "signer.no_secret",
        DerivationFailed = "signer.derivation_failed",
    }
}

codes! {
    /// `SendFailure`: a payment could not be drafted, prepared or sent.
    SendCode {
        NoRecipients = "send.no_recipients",
        InvalidAddress = "send.invalid_address",
        PlatformAddress = "send.platform_address",
        InvalidAmount = "send.invalid_amount",
        DustAmount = "send.dust_amount",
        DuplicateAddress = "send.duplicate_address",
        AmountExceedsBalance = "send.amount_exceeds_balance",
        AmountWithFeeExceedsBalance = "send.amount_with_fee_exceeds_balance",
        AmountTooSmallAfterFee = "send.amount_too_small_after_fee",
        InsufficientMixedFunds = "send.insufficient_mixed_funds",
        OutpointUnavailable = "send.outpoint_unavailable",
        AbsurdFee = "send.absurd_fee",
        TxTooLarge = "send.tx_too_large",
        InvalidChangeAddress = "send.invalid_change_address",
        WatchOnly = "send.watch_only",
        VaultLocked = "send.vault_locked",
        GrantInvalid = "send.grant_invalid",
        GrantExceeded = "send.grant_exceeded",
        PreparedTxSpent = "send.prepared_tx_spent",
        NoPeers = "send.no_peers",
        BroadcastRejected = "send.broadcast_rejected",
        BroadcastUnknown = "send.broadcast_unknown",
        /// A lease ended before the hand-off; nothing was sent (dw-ffi
        /// folds it into `send.prepared_tx_spent`).
        Cancelled = "send.cancelled",
    }
}

codes! {
    /// `LabelsFailure`: an address-book rule was violated.
    LabelsCode {
        InvalidAddress = "labels.invalid_address",
        DuplicateAddress = "labels.duplicate_address",
        OwnAddress = "labels.own_address",
        EntryNotFound = "labels.entry_not_found",
        ReceiveEntryNotDeletable = "labels.receive_entry_not_deletable",
    }
}

codes! {
    /// `CompatFailure`: a Dash Core file or export failed.
    CompatCode {
        FileUnreadable = "compat.file_unreadable",
        UnsupportedFormat = "compat.unsupported_format",
        Corrupt = "compat.corrupt",
        PassphraseRequired = "compat.passphrase_required",
        WrongPassphrase = "compat.wrong_passphrase",
        NoHdChain = "compat.no_hd_chain",
        NetworkMismatch = "compat.network_mismatch",
        InvalidKeyMaterial = "compat.invalid_key_material",
        WatchOnly = "compat.watch_only",
        DestinationUnwritable = "compat.destination_unwritable",
    }
}

codes! {
    /// `BackupFailure`: a backup could not be written or restored.
    BackupCode {
        VaultLocked = "backup.vault_locked",
        PassphraseRequired = "backup.passphrase_required",
        WrongPassphrase = "backup.wrong_passphrase",
        Corrupt = "backup.corrupt",
        UnsupportedVersion = "backup.unsupported_version",
        NetworkMismatch = "backup.network_mismatch",
        AlreadyExists = "backup.already_exists",
        DestinationUnwritable = "backup.destination_unwritable",
    }
}

codes! {
    /// `PsbtFailure`: a PSBT could not be read, signed or sent.
    PsbtCode {
        Invalid = "psbt.invalid",
        TooLarge = "psbt.too_large",
        NotComplete = "psbt.not_complete",
        FeeRateTooHigh = "psbt.fee_rate_too_high",
        AbsurdFee = "psbt.absurd_fee",
        FeeUnknown = "psbt.fee_unknown",
        WatchOnly = "psbt.watch_only",
        GrantExceeded = "psbt.grant_exceeded",
        NoPeers = "psbt.no_peers",
        BroadcastRejected = "psbt.broadcast_rejected",
        BroadcastUnknown = "psbt.broadcast_unknown",
    }
}

codes! {
    /// `CoinJoinFailure`: a CoinJoin call failed.
    CoinJoinCode {
        Disabled = "coinjoin.disabled",
        WatchOnly = "coinjoin.watch_only",
        InsufficientFunds = "coinjoin.insufficient_funds",
        VaultLocked = "coinjoin.vault_locked",
        GrantInvalid = "coinjoin.grant_invalid",
        NothingToMove = "coinjoin.nothing_to_move",
        SpvNotRunning = "coinjoin.spv_not_running",
        NoPeers = "coinjoin.no_peers",
        BroadcastRejected = "coinjoin.broadcast_rejected",
    }
}

codes! {
    /// `MasternodeFailure`: a masternode keychain call failed.
    MasternodeCode {
        WatchOnly = "masternode.watch_only",
        VaultLocked = "masternode.vault_locked",
        GrantInvalid = "masternode.grant_invalid",
    }
}

codes! {
    /// A `dash:` URI or QR helper refused its input. (A URI that does not
    /// parse is data, not an error: `parsePaymentUri` returns the reason.)
    UriCode {
        /// The text exceeds a QR code's 255 characters (QT-084); dash-qt
        /// asks to shorten the label or message.
        TooLongForQr = "uri.too_long_for_qr",
    }
}

codes! {
    /// `ConsoleFailure`: a console line did not run (dw-ffi's strings).
    ConsoleCode {
        /// dash-qt "Error: Invalid command line".
        ParseError = "console.parse_error",
        /// A Core RPC error (`data`: its code and message).
        RpcError = "console.rpc_error",
        /// The command needs a full node: "Not available in SPV mode."
        NotAvailable = "console.not_available",
        /// The command spends, signs or reveals (`data`: the grant to ask for).
        AuthorizationRequired = "console.authorization_required",
        /// A wallet command with no wallet selected.
        WalletRequired = "console.wallet_required",
    }
}

type Mapped = (ErrorCode, Option<ErrorData>);

fn build(message: String, (code, data): Mapped) -> AppError {
    AppError {
        code,
        message,
        data,
    }
}

impl From<SignerError> for AppError {
    fn from(e: SignerError) -> Self {
        use SignerError as S;
        let message = e.to_string();
        let mapped: Mapped = match e {
            S::Locked => (SignerCode::Locked.into(), None),
            S::PathNotAllowed(_) => (SignerCode::PathNotAllowed.into(), None),
            S::KeyMismatch(_) => (SignerCode::KeyMismatch.into(), None),
            S::Decrypt(_) => (SignerCode::DecryptFailed.into(), None),
            S::NoSecret => (SignerCode::NoSecret.into(), None),
            S::Derivation(_) => (SignerCode::DerivationFailed.into(), None),
            S::Vault(v) => return v.into(),
        };
        build(message, mapped)
    }
}

impl From<ConsoleFailure> for AppError {
    fn from(f: ConsoleFailure) -> Self {
        use ConsoleFailure as F;
        let message = f.to_string();
        let mapped: Mapped = match f {
            F::Parse(_) => (ConsoleCode::ParseError.into(), None),
            F::Rpc { code, message } => (
                ConsoleCode::RpcError.into(),
                Some(ErrorData::Rpc {
                    rpc_code: code,
                    rpc_message: message,
                }),
            ),
            F::NotAvailable(_) => (ConsoleCode::NotAvailable.into(), None),
            F::AuthorizationRequired { purpose, wallet } => (
                ConsoleCode::AuthorizationRequired.into(),
                Some(ErrorData::Authorize {
                    purpose: purpose.into(),
                    wallet_id: wallet.map(|w| w.to_string()),
                }),
            ),
            F::WalletRequired => (ConsoleCode::WalletRequired.into(), None),
            F::Engine(e) => return e.into(),
        };
        build(message, mapped)
    }
}

impl From<SendFailure> for AppError {
    fn from(f: SendFailure) -> Self {
        use SendFailure as F;
        let message = f.to_string();
        let recipient = |index| Some(ErrorData::Recipient { index });
        let mapped: Mapped = match f {
            F::NoRecipients => (SendCode::NoRecipients.into(), None),
            F::InvalidAddress { index } => (SendCode::InvalidAddress.into(), recipient(index)),
            F::PlatformAddress { index } => (SendCode::PlatformAddress.into(), recipient(index)),
            F::InvalidAmount { index } => (SendCode::InvalidAmount.into(), recipient(index)),
            F::DustAmount { index } => (SendCode::DustAmount.into(), recipient(index)),
            F::DuplicateAddress { index } => (SendCode::DuplicateAddress.into(), recipient(index)),
            F::AmountExceedsBalance { available } => (
                SendCode::AmountExceedsBalance.into(),
                Some(ErrorData::Duffs {
                    needed: None,
                    available,
                }),
            ),
            F::AmountWithFeeExceedsBalance { fee, available } => (
                SendCode::AmountWithFeeExceedsBalance.into(),
                Some(ErrorData::Fee {
                    fee,
                    available: Some(available),
                }),
            ),
            F::AmountTooSmallAfterFee { index } => {
                (SendCode::AmountTooSmallAfterFee.into(), recipient(index))
            }
            F::InsufficientMixedFunds { available } => (
                SendCode::InsufficientMixedFunds.into(),
                Some(ErrorData::Duffs {
                    needed: None,
                    available,
                }),
            ),
            F::OutpointUnavailable(outpoint) => (
                SendCode::OutpointUnavailable.into(),
                Some(ErrorData::Outpoint {
                    txid: outpoint.txid.to_string(),
                    vout: outpoint.vout,
                }),
            ),
            F::AbsurdFee { fee } => (
                SendCode::AbsurdFee.into(),
                Some(ErrorData::Fee {
                    fee,
                    available: None,
                }),
            ),
            F::TxTooLarge => (SendCode::TxTooLarge.into(), None),
            F::InvalidChangeAddress => (SendCode::InvalidChangeAddress.into(), None),
            F::WatchOnly => (SendCode::WatchOnly.into(), None),
            F::VaultLocked => (SendCode::VaultLocked.into(), None),
            F::GrantInvalid => (SendCode::GrantInvalid.into(), None),
            F::GrantExceeded { max_duffs, outflow } => (
                SendCode::GrantExceeded.into(),
                Some(ErrorData::SpendCap { max_duffs, outflow }),
            ),
            F::PreparedTxSpent => (SendCode::PreparedTxSpent.into(), None),
            F::NoPeers => (SendCode::NoPeers.into(), None),
            F::BroadcastRejected { reason } => (
                SendCode::BroadcastRejected.into(),
                Some(ErrorData::Reason { reason }),
            ),
            F::BroadcastUnknown { reason } => (
                SendCode::BroadcastUnknown.into(),
                Some(ErrorData::Reason { reason }),
            ),
            F::Cancelled => (SendCode::Cancelled.into(), None),
        };
        build(message, mapped)
    }
}

impl From<LabelsFailure> for AppError {
    fn from(f: LabelsFailure) -> Self {
        use LabelsFailure as F;
        let message = f.to_string();
        let code: ErrorCode = match f {
            F::InvalidAddress => LabelsCode::InvalidAddress.into(),
            F::DuplicateAddress => LabelsCode::DuplicateAddress.into(),
            F::OwnAddress => LabelsCode::OwnAddress.into(),
            F::EntryNotFound => LabelsCode::EntryNotFound.into(),
            F::ReceiveEntryNotDeletable => LabelsCode::ReceiveEntryNotDeletable.into(),
        };
        build(message, (code, None))
    }
}

impl From<CompatFailure> for AppError {
    fn from(f: CompatFailure) -> Self {
        use CompatFailure as F;
        let message = f.to_string();
        let mapped: Mapped = match f {
            F::FileUnreadable(_) => (CompatCode::FileUnreadable.into(), None),
            F::UnsupportedFormat(_) => (CompatCode::UnsupportedFormat.into(), None),
            F::Corrupt(_) => (CompatCode::Corrupt.into(), None),
            F::PassphraseRequired => (CompatCode::PassphraseRequired.into(), None),
            F::WrongPassphrase => (CompatCode::WrongPassphrase.into(), None),
            F::NoHdChain => (CompatCode::NoHdChain.into(), None),
            F::NetworkMismatch { mainnet } => (
                CompatCode::NetworkMismatch.into(),
                Some(ErrorData::KeysNetwork { mainnet }),
            ),
            F::InvalidKeyMaterial(_) => (CompatCode::InvalidKeyMaterial.into(), None),
            F::WatchOnly => (CompatCode::WatchOnly.into(), None),
            F::DestinationUnwritable(_) => (CompatCode::DestinationUnwritable.into(), None),
        };
        build(message, mapped)
    }
}

impl From<BackupFailure> for AppError {
    fn from(f: BackupFailure) -> Self {
        use BackupFailure as F;
        let message = f.to_string();
        let mapped: Mapped = match f {
            F::VaultLocked => (BackupCode::VaultLocked.into(), None),
            F::PassphraseRequired => (BackupCode::PassphraseRequired.into(), None),
            F::WrongPassphrase => (BackupCode::WrongPassphrase.into(), None),
            F::Corrupt(_) => (BackupCode::Corrupt.into(), None),
            F::UnsupportedVersion(version) => (
                BackupCode::UnsupportedVersion.into(),
                Some(ErrorData::Number {
                    value: version.into(),
                }),
            ),
            F::NetworkMismatch => (BackupCode::NetworkMismatch.into(), None),
            F::AlreadyExists(id) => (
                BackupCode::AlreadyExists.into(),
                Some(ErrorData::Wallet {
                    wallet_id: id.to_string(),
                }),
            ),
            F::DestinationUnwritable(_) => (BackupCode::DestinationUnwritable.into(), None),
        };
        build(message, mapped)
    }
}

impl From<PsbtFailure> for AppError {
    fn from(f: PsbtFailure) -> Self {
        use PsbtFailure as F;
        let message = f.to_string();
        let mapped: Mapped = match f {
            F::Invalid(_) => (PsbtCode::Invalid.into(), None),
            F::TooLarge(bytes) => (
                PsbtCode::TooLarge.into(),
                Some(ErrorData::Number { value: bytes }),
            ),
            F::NotComplete => (PsbtCode::NotComplete.into(), None),
            F::FeeRateTooHigh { duffs_per_kb } => (
                PsbtCode::FeeRateTooHigh.into(),
                Some(ErrorData::FeeRate { duffs_per_kb }),
            ),
            F::AbsurdFee { fee } => (
                PsbtCode::AbsurdFee.into(),
                Some(ErrorData::Fee {
                    fee,
                    available: None,
                }),
            ),
            F::FeeUnknown => (PsbtCode::FeeUnknown.into(), None),
            F::WatchOnly => (PsbtCode::WatchOnly.into(), None),
            F::GrantExceeded { max_duffs } => (
                PsbtCode::GrantExceeded.into(),
                Some(ErrorData::Limit { duffs: max_duffs }),
            ),
            F::NoPeers => (PsbtCode::NoPeers.into(), None),
            F::BroadcastRejected { reason } => (
                PsbtCode::BroadcastRejected.into(),
                Some(ErrorData::Reason { reason }),
            ),
            F::BroadcastUnknown { reason } => (
                PsbtCode::BroadcastUnknown.into(),
                Some(ErrorData::Reason { reason }),
            ),
        };
        build(message, mapped)
    }
}

impl From<CoinJoinFailure> for AppError {
    fn from(f: CoinJoinFailure) -> Self {
        use CoinJoinFailure as F;
        let message = f.to_string();
        let mapped: Mapped = match f {
            F::Disabled => (CoinJoinCode::Disabled.into(), None),
            F::WatchOnly => (CoinJoinCode::WatchOnly.into(), None),
            F::InsufficientFunds { min_duffs } => (
                CoinJoinCode::InsufficientFunds.into(),
                Some(ErrorData::Limit { duffs: min_duffs }),
            ),
            F::VaultLocked => (CoinJoinCode::VaultLocked.into(), None),
            F::GrantInvalid => (CoinJoinCode::GrantInvalid.into(), None),
            F::NothingToMove => (CoinJoinCode::NothingToMove.into(), None),
            F::SpvNotRunning => (CoinJoinCode::SpvNotRunning.into(), None),
            F::NoPeers => (CoinJoinCode::NoPeers.into(), None),
            F::BroadcastRejected(reason) => (
                CoinJoinCode::BroadcastRejected.into(),
                Some(ErrorData::Reason { reason }),
            ),
        };
        build(message, mapped)
    }
}

impl From<MasternodeFailure> for AppError {
    fn from(f: MasternodeFailure) -> Self {
        use MasternodeFailure as F;
        let message = f.to_string();
        let code: ErrorCode = match f {
            F::WatchOnly => MasternodeCode::WatchOnly.into(),
            F::VaultLocked => MasternodeCode::VaultLocked.into(),
            F::GrantInvalid => MasternodeCode::GrantInvalid.into(),
        };
        build(message, (code, None))
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::WalletId;

    use super::*;

    fn code(e: impl Into<AppError>) -> &'static str {
        e.into().code_str()
    }

    /// Every variant's code equals dw-ffi's `code_str` for the same failure
    /// (the strings are copies of dw-ffi's; `send.cancelled` is the contract's,
    /// which dw-ffi folds into `send.prepared_tx_spent`).
    #[test]
    fn codes_match_the_engine() {
        let s = || String::from("x");
        let outpoint = dashcore::OutPoint::null();

        let send = [
            (SendFailure::NoRecipients, "send.no_recipients"),
            (
                SendFailure::InvalidAddress { index: 1 },
                "send.invalid_address",
            ),
            (
                SendFailure::PlatformAddress { index: 1 },
                "send.platform_address",
            ),
            (
                SendFailure::InvalidAmount { index: 1 },
                "send.invalid_amount",
            ),
            (SendFailure::DustAmount { index: 1 }, "send.dust_amount"),
            (
                SendFailure::DuplicateAddress { index: 1 },
                "send.duplicate_address",
            ),
            (
                SendFailure::AmountExceedsBalance { available: 1 },
                "send.amount_exceeds_balance",
            ),
            (
                SendFailure::AmountWithFeeExceedsBalance {
                    fee: 2,
                    available: 1,
                },
                "send.amount_with_fee_exceeds_balance",
            ),
            (
                SendFailure::AmountTooSmallAfterFee { index: 1 },
                "send.amount_too_small_after_fee",
            ),
            (
                SendFailure::InsufficientMixedFunds { available: 1 },
                "send.insufficient_mixed_funds",
            ),
            (
                SendFailure::OutpointUnavailable(outpoint),
                "send.outpoint_unavailable",
            ),
            (SendFailure::AbsurdFee { fee: 9 }, "send.absurd_fee"),
            (SendFailure::TxTooLarge, "send.tx_too_large"),
            (
                SendFailure::InvalidChangeAddress,
                "send.invalid_change_address",
            ),
            (SendFailure::WatchOnly, "send.watch_only"),
            (SendFailure::VaultLocked, "send.vault_locked"),
            (SendFailure::GrantInvalid, "send.grant_invalid"),
            (
                SendFailure::GrantExceeded {
                    max_duffs: 1,
                    outflow: 2,
                },
                "send.grant_exceeded",
            ),
            (SendFailure::PreparedTxSpent, "send.prepared_tx_spent"),
            (SendFailure::NoPeers, "send.no_peers"),
            (
                SendFailure::BroadcastRejected { reason: s() },
                "send.broadcast_rejected",
            ),
            (
                SendFailure::BroadcastUnknown { reason: s() },
                "send.broadcast_unknown",
            ),
            (SendFailure::Cancelled, "send.cancelled"),
        ];
        assert_eq!(send.len(), SendCode::ALL.len());
        for (f, expected) in send {
            assert_eq!(code(f), expected);
        }

        let signer = [
            (SignerError::Locked, "signer.locked"),
            (SignerError::PathNotAllowed(s()), "signer.path_not_allowed"),
            (SignerError::KeyMismatch(s()), "signer.key_mismatch"),
            (SignerError::Decrypt(s()), "signer.decrypt_failed"),
            (SignerError::NoSecret, "signer.no_secret"),
            (SignerError::Derivation(s()), "signer.derivation_failed"),
            // The nested vault failure keeps the vault's own code.
            (
                SignerError::Vault(dw_vault::VaultError::NoVault),
                "vault.no_vault",
            ),
        ];
        // Every signer code but the delegating `Vault` variant.
        assert_eq!(signer.len() - 1, SignerCode::ALL.len());
        for (f, expected) in signer {
            assert_eq!(code(f), expected);
        }

        let labels = [
            (LabelsFailure::InvalidAddress, "labels.invalid_address"),
            (LabelsFailure::DuplicateAddress, "labels.duplicate_address"),
            (LabelsFailure::OwnAddress, "labels.own_address"),
            (LabelsFailure::EntryNotFound, "labels.entry_not_found"),
            (
                LabelsFailure::ReceiveEntryNotDeletable,
                "labels.receive_entry_not_deletable",
            ),
        ];
        assert_eq!(labels.len(), LabelsCode::ALL.len());
        for (f, expected) in labels {
            assert_eq!(code(f), expected);
        }

        let compat = [
            (CompatFailure::FileUnreadable(s()), "compat.file_unreadable"),
            (
                CompatFailure::UnsupportedFormat(s()),
                "compat.unsupported_format",
            ),
            (CompatFailure::Corrupt(s()), "compat.corrupt"),
            (
                CompatFailure::PassphraseRequired,
                "compat.passphrase_required",
            ),
            (CompatFailure::WrongPassphrase, "compat.wrong_passphrase"),
            (CompatFailure::NoHdChain, "compat.no_hd_chain"),
            (
                CompatFailure::NetworkMismatch { mainnet: true },
                "compat.network_mismatch",
            ),
            (
                CompatFailure::InvalidKeyMaterial(s()),
                "compat.invalid_key_material",
            ),
            (CompatFailure::WatchOnly, "compat.watch_only"),
            (
                CompatFailure::DestinationUnwritable(s()),
                "compat.destination_unwritable",
            ),
        ];
        assert_eq!(compat.len(), CompatCode::ALL.len());
        for (f, expected) in compat {
            assert_eq!(code(f), expected);
        }

        let backup = [
            (BackupFailure::VaultLocked, "backup.vault_locked"),
            (
                BackupFailure::PassphraseRequired,
                "backup.passphrase_required",
            ),
            (BackupFailure::WrongPassphrase, "backup.wrong_passphrase"),
            (BackupFailure::Corrupt(s()), "backup.corrupt"),
            (
                BackupFailure::UnsupportedVersion(9),
                "backup.unsupported_version",
            ),
            (BackupFailure::NetworkMismatch, "backup.network_mismatch"),
            (
                BackupFailure::AlreadyExists(WalletId([7; 32])),
                "backup.already_exists",
            ),
            (
                BackupFailure::DestinationUnwritable(s()),
                "backup.destination_unwritable",
            ),
        ];
        assert_eq!(backup.len(), BackupCode::ALL.len());
        for (f, expected) in backup {
            assert_eq!(code(f), expected);
        }

        let psbt = [
            (PsbtFailure::Invalid(s()), "psbt.invalid"),
            (PsbtFailure::TooLarge(9), "psbt.too_large"),
            (PsbtFailure::NotComplete, "psbt.not_complete"),
            (
                PsbtFailure::FeeRateTooHigh { duffs_per_kb: 9 },
                "psbt.fee_rate_too_high",
            ),
            (PsbtFailure::AbsurdFee { fee: 9 }, "psbt.absurd_fee"),
            (PsbtFailure::FeeUnknown, "psbt.fee_unknown"),
            (PsbtFailure::WatchOnly, "psbt.watch_only"),
            (
                PsbtFailure::GrantExceeded { max_duffs: 9 },
                "psbt.grant_exceeded",
            ),
            (PsbtFailure::NoPeers, "psbt.no_peers"),
            (
                PsbtFailure::BroadcastRejected { reason: s() },
                "psbt.broadcast_rejected",
            ),
            (
                PsbtFailure::BroadcastUnknown { reason: s() },
                "psbt.broadcast_unknown",
            ),
        ];
        assert_eq!(psbt.len(), PsbtCode::ALL.len());
        for (f, expected) in psbt {
            assert_eq!(code(f), expected);
        }

        let coinjoin = [
            (CoinJoinFailure::Disabled, "coinjoin.disabled"),
            (CoinJoinFailure::WatchOnly, "coinjoin.watch_only"),
            (
                CoinJoinFailure::InsufficientFunds { min_duffs: 9 },
                "coinjoin.insufficient_funds",
            ),
            (CoinJoinFailure::VaultLocked, "coinjoin.vault_locked"),
            (CoinJoinFailure::GrantInvalid, "coinjoin.grant_invalid"),
            (CoinJoinFailure::NothingToMove, "coinjoin.nothing_to_move"),
            (CoinJoinFailure::SpvNotRunning, "coinjoin.spv_not_running"),
            (CoinJoinFailure::NoPeers, "coinjoin.no_peers"),
            (
                CoinJoinFailure::BroadcastRejected(s()),
                "coinjoin.broadcast_rejected",
            ),
        ];
        assert_eq!(coinjoin.len(), CoinJoinCode::ALL.len());
        for (f, expected) in coinjoin {
            assert_eq!(code(f), expected);
        }

        let masternode = [
            (MasternodeFailure::WatchOnly, "masternode.watch_only"),
            (MasternodeFailure::VaultLocked, "masternode.vault_locked"),
            (MasternodeFailure::GrantInvalid, "masternode.grant_invalid"),
        ];
        assert_eq!(masternode.len(), MasternodeCode::ALL.len());
        for (f, expected) in masternode {
            assert_eq!(code(f), expected);
        }
    }

    #[test]
    fn engine_error_delegates_to_the_nested_domains() {
        use dw_engine::EngineError as E;
        assert_eq!(
            AppError::from(E::Send(SendFailure::DustAmount { index: 2 })).code_str(),
            "send.dust_amount"
        );
        assert_eq!(
            AppError::from(E::Labels(LabelsFailure::OwnAddress)).code_str(),
            "labels.own_address"
        );
        assert_eq!(
            AppError::from(E::Compat(CompatFailure::NoHdChain)).code_str(),
            "compat.no_hd_chain"
        );
        assert_eq!(
            AppError::from(E::Backup(BackupFailure::VaultLocked)).code_str(),
            "backup.vault_locked"
        );
        assert_eq!(
            AppError::from(E::Psbt(PsbtFailure::NotComplete)).code_str(),
            "psbt.not_complete"
        );
        assert_eq!(
            AppError::from(E::CoinJoin(CoinJoinFailure::Disabled)).code_str(),
            "coinjoin.disabled"
        );
        assert_eq!(
            AppError::from(E::Masternode(MasternodeFailure::WatchOnly)).code_str(),
            "masternode.watch_only"
        );
        assert_eq!(
            AppError::from(E::Signer(SignerError::NoSecret)).code_str(),
            "signer.no_secret"
        );
    }

    #[test]
    fn fields_ride_in_data() {
        let data = |e: AppError| serde_json::to_value(&e).unwrap()["data"].clone();
        assert_eq!(
            data(SendFailure::DustAmount { index: 3 }.into()),
            serde_json::json!({ "kind": "recipient", "index": 3 })
        );
        assert_eq!(
            data(
                SendFailure::GrantExceeded {
                    max_duffs: 5,
                    outflow: 9
                }
                .into()
            ),
            serde_json::json!({ "kind": "spendCap", "maxDuffs": 5, "outflow": 9 })
        );
        assert_eq!(
            data(
                SendFailure::AmountWithFeeExceedsBalance {
                    fee: 2,
                    available: 1
                }
                .into()
            ),
            serde_json::json!({ "kind": "fee", "fee": 2, "available": 1 })
        );
        assert_eq!(
            data(SendFailure::OutpointUnavailable(dashcore::OutPoint::null()).into()),
            serde_json::json!({
                "kind": "outpoint",
                "txid": "0".repeat(64),
                "vout": u32::MAX
            })
        );
        assert_eq!(
            data(CompatFailure::NetworkMismatch { mainnet: true }.into()),
            serde_json::json!({ "kind": "keysNetwork", "mainnet": true })
        );
        assert_eq!(
            data(BackupFailure::AlreadyExists(WalletId([0xab; 32])).into()),
            serde_json::json!({ "kind": "wallet", "walletId": "ab".repeat(32) })
        );
        assert_eq!(
            data(PsbtFailure::FeeRateTooHigh { duffs_per_kb: 7 }.into()),
            serde_json::json!({ "kind": "feeRate", "duffsPerKb": 7 })
        );
        assert_eq!(
            data(CoinJoinFailure::InsufficientFunds { min_duffs: 140_001 }.into()),
            serde_json::json!({ "kind": "limit", "duffs": 140_001 })
        );
    }
}
