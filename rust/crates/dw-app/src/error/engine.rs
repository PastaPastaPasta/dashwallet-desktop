//! The M1–M3 engine errors (m1–m3 contracts): `EngineError`'s own codes and
//! the vault's. The nested failure domains (`send.*`, `labels.*`, …) map in
//! `nested.rs`, with the strings dw-ffi gives them, so the frozen Swift
//! shells and this binding name each failure the same.

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
        StorageTooNew = "storage_too_new",
        StorageRefused = "storage_refused",
        StorageNotPrivate = "storage_not_private",
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
        RescanInterrupted = "rescan_interrupted",
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
        StuckFunding = "stuck_funding",
        ShieldedSpendUnresolved = "shielded_spend_unresolved",
        LastStartupWallet = "last_startup_wallet",
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
        WalletSecretExists = "vault.wallet_secret_exists",
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
                failed_attempts,
                retry_after_secs,
            } => (
                VaultCode::WrongPassphrase.into(),
                Some(ErrorData::WrongPassphrase {
                    failed_attempts,
                    retry_after_secs,
                }),
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
            V::WalletSecretExists => (VaultCode::WalletSecretExists.into(), None),
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
            E::StorageInUse { retryable, .. } => (
                EngineCode::StorageInUse.into(),
                Some(ErrorData::StorageInUse { retryable }),
            ),
            E::Storage(_) => (EngineCode::Storage.into(), None),
            E::StorageTooNew(_) => (EngineCode::StorageTooNew.into(), None),
            E::StoreRefused(r) => {
                let folder = r.folder.display().to_string();
                (
                    EngineCode::StorageRefused.into(),
                    Some(ErrorData::StoreRefused {
                        screen: Box::new(super::StoreScreen::new(&r.screen, &folder)),
                        folder,
                        cause: r.cause.into(),
                    }),
                )
            }
            E::DataFolderNotPrivate { folder, reason, .. } => (
                EngineCode::StorageNotPrivate.into(),
                Some(ErrorData::NotPrivate {
                    folder: folder.display().to_string(),
                    reason: reason.into(),
                }),
            ),
            E::RetryableIo(_) => (
                EngineCode::Io.into(),
                Some(ErrorData::Io { retryable: true }),
            ),
            E::WalletNotFound(_) => (EngineCode::WalletNotFound.into(), None),
            E::InvalidMnemonic(_) => (EngineCode::InvalidMnemonic.into(), None),
            E::WalletAlreadyExists(_) => (EngineCode::WalletAlreadyExists.into(), None),
            E::Wallet(_) => (EngineCode::Wallet.into(), None),
            E::Sdk(_) => (EngineCode::Sdk.into(), None),
            E::InsufficientCredits {
                identity_id,
                needed,
                available,
            } => (
                EngineCode::InsufficientCredits.into(),
                Some(ErrorData::IdentityCredits {
                    identity: identity_id,
                    needed,
                    available,
                }),
            ),
            E::Spv(_) => (EngineCode::Spv.into(), None),
            E::Io(_) => (EngineCode::Io.into(), None),
            E::NotImplemented(call) => (
                EngineCode::NotImplemented.into(),
                Some(ErrorData::Call { call }),
            ),
            E::SpvNotRunning => (EngineCode::SpvNotRunning.into(), None),
            E::ShieldedSpendUnresolved => (EngineCode::ShieldedSpendUnresolved.into(), None),
            E::LastStartupWallet => (EngineCode::LastStartupWallet.into(), None),
            E::SpvRunning => (EngineCode::SpvRunning.into(), None),
            E::RescanInProgress => (EngineCode::RescanInProgress.into(), None),
            E::RescanInterrupted => (EngineCode::RescanInterrupted.into(), None),
            E::InvalidXpub(_) => (EngineCode::InvalidXpub.into(), None),
            E::TxActionRefused(refusal) => (
                EngineCode::TxActionRefused.into(),
                Some(ErrorData::TxAction {
                    refusal: refusal.into(),
                    wallet_id: match refusal {
                        dw_engine::TxActionRefusal::RecordUnreadable(w) => Some(w.to_string()),
                        _ => None,
                    },
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
            E::OutpointNotFound(o) => (
                EngineCode::OutpointNotFound.into(),
                Some(ErrorData::Outpoint {
                    txid: o.txid.to_string(),
                    vout: o.vout,
                }),
            ),
            E::Signer(f) => return f.into(),
            E::Send(f) => return f.into(),
            E::Labels(f) => return f.into(),
            E::Compat(f) => return f.into(),
            E::Backup(f) => return f.into(),
            E::Psbt(f) => return f.into(),
            E::CoinJoin(f) => return f.into(),
            E::Masternode(f) => return f.into(),
            E::Contact(e) => return e.into(),
            E::StuckFunding(r) => (
                EngineCode::StuckFunding.into(),
                Some(ErrorData::StuckFunding {
                    retry_later: r.retry_later(),
                    refusal: r.into(),
                }),
            ),
        };
        AppError {
            code,
            message,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::{
        BackupFailure, CoinJoinFailure, CompatFailure, LabelsFailure, MasternodeFailure,
        PsbtFailure, SendFailure, TxActionRefusal,
    };
    use dw_vault::SignerError;

    use super::*;

    fn refusal() -> dw_engine::store_upgrade::StoreRefusal {
        dw_engine::store_upgrade::StoreRefusal {
            screen: dw_engine::store_upgrade::copy::cant_open(
                dw_engine::store_upgrade::copy::CantOpenCause::RetiredPoolWithoutP9a,
                true,
            ),
            folder: std::path::PathBuf::from("/data/regtest"),
            cause: "RetiredPoolWithoutAdopter",
            stage: dw_engine::store_upgrade::StoreUpgradeStage::Prepare,
            detail: "1 retired contact-pool rows and this build has no P9a".into(),
        }
    }

    /// A store refusal rides in data as the store upgrade's own screen,
    /// `{folder}` filled in, with the folder and the cause's name.
    #[test]
    fn a_store_refusal_rides_in_data_as_its_screen() {
        let r = refusal();
        let screen = r.screen.clone();
        let e =
            serde_json::to_value(AppError::from(EngineError::StoreRefused(Box::new(r)))).unwrap();
        assert_eq!(e["code"], "storage_refused");
        assert_eq!(
            e["data"],
            serde_json::json!({
                "kind": "storeRefused",
                "screen": {
                    "titleKey": "storeUpgrade.cantOpen.title",
                    "title": screen.title,
                    "textKey": "storeUpgrade.cantOpen.text",
                    "text": screen.text.replace("{folder}", "/data/regtest"),
                    "buttons": [{
                        "key": "storeUpgrade.cantOpen.quit",
                        "label": "Quit",
                        "destructive": false,
                        "defaultFocus": false,
                    }],
                    "hint": null,
                },
                "folder": "/data/regtest",
                "cause": "RetiredPoolWithoutAdopter",
            })
        );
        assert!(
            !e["data"]["screen"]["text"]
                .as_str()
                .unwrap()
                .contains("{folder}")
        );
    }

    /// A data root others can change rides in data as the folder and why,
    /// each reason by its camelCase name.
    #[test]
    fn a_data_folder_not_private_rides_in_data() {
        use dw_fs::InsecureReason as R;
        for (reason, name) in [
            (R::GroupWritable, "groupWritable"),
            (R::OtherWritable, "otherWritable"),
            (R::OtherOwner, "otherOwner"),
        ] {
            let e = serde_json::to_value(AppError::from(EngineError::DataFolderNotPrivate {
                folder: "/home/u/dash".into(),
                reason,
                detail: "x".into(),
            }))
            .unwrap();
            assert_eq!(e["code"], "storage_not_private");
            assert_eq!(
                e["data"],
                serde_json::json!({ "kind": "notPrivate", "folder": "/home/u/dash", "reason": name })
            );
        }
    }

    /// `storage_in_use` carries the engine's own `retryable`: true for a
    /// store a clean close releases, false for one held until a restart;
    /// the store upgrade's machine failures are `io`, retryable, never a
    /// refusal.
    #[test]
    fn storage_in_use_and_retryable_io_ride_in_data() {
        for retryable in [true, false] {
            let e = serde_json::to_value(AppError::from(EngineError::StorageInUse {
                detail: "x".into(),
                retryable,
            }))
            .unwrap();
            assert_eq!(e["code"], "storage_in_use");
            assert_eq!(
                e["data"],
                serde_json::json!({ "kind": "storageInUse", "retryable": retryable })
            );
        }
        let e = serde_json::to_value(AppError::from(EngineError::RetryableIo("disk full".into())))
            .unwrap();
        assert_eq!(e["code"], "io");
        assert_eq!(
            e["data"],
            serde_json::json!({ "kind": "io", "retryable": true })
        );
    }

    /// Every `EngineError` variant maps to the code string its `code()`
    /// names; a nested domain (`vault`, `signer`, `send`, …) maps to a code
    /// inside that domain's namespace.
    #[test]
    fn engine_error_codes_match_the_engine() {
        use EngineError as E;
        let s = || String::from("x");
        let own = [
            E::InvalidConfig(s()),
            E::InvalidArgument(s()),
            E::NetworkNotOpen(s()),
            E::StorageInUse {
                detail: s(),
                retryable: true,
            },
            E::Storage(s()),
            E::StorageTooNew(s()),
            E::StoreRefused(Box::new(refusal())),
            E::DataFolderNotPrivate {
                folder: "/data".into(),
                reason: dw_fs::InsecureReason::GroupWritable,
                detail: s(),
            },
            E::RetryableIo(s()),
            E::WalletNotFound(s()),
            E::InvalidMnemonic(s()),
            E::WalletAlreadyExists(s()),
            E::Wallet(s()),
            E::Sdk(s()),
            E::InsufficientCredits {
                identity_id: s(),
                needed: 2,
                available: 1,
            },
            E::Spv(s()),
            E::Io(s()),
            E::NotImplemented(s()),
            E::SpvNotRunning,
            E::SpvRunning,
            E::RescanInProgress,
            E::RescanInterrupted,
            E::InvalidXpub(s()),
            E::TxActionRefused(TxActionRefusal::Confirmed),
            E::NoPeers,
            E::HeightOutOfRange(7),
            E::InvalidQuery(s()),
            E::StaleCursor,
            E::TxNotFound(s()),
            E::GapLimit,
            E::RequestNotFound(3),
            E::NameRejected(s()),
            E::InvalidAddress(s()),
            E::AddressNoKey(s()),
            E::AddressNotMine(s()),
            E::Internal(s()),
            E::OutpointNotFound(dashcore::OutPoint::null()),
            E::StuckFunding(dw_engine::platform::lease::StuckFundingRefusal::NoMarker),
            E::ShieldedSpendUnresolved,
            E::LastStartupWallet,
        ];
        // Every code of `EngineCode` is one of these (the one-to-one check).
        let mut seen: Vec<&str> = Vec::new();
        for e in own {
            let expected = e.code();
            assert_eq!(AppError::from(e).code_str(), expected);
            seen.push(expected);
        }
        for code in EngineCode::ALL {
            assert!(
                seen.contains(&code.as_str()),
                "{} has no EngineError variant",
                code.as_str()
            );
        }

        let nested = [
            E::Vault(VaultError::NoVault),
            E::Signer(SignerError::Locked),
            E::Send(SendFailure::NoRecipients),
            E::Labels(LabelsFailure::OwnAddress),
            E::Compat(CompatFailure::NoHdChain),
            E::Backup(BackupFailure::VaultLocked),
            E::Psbt(PsbtFailure::NotComplete),
            E::CoinJoin(CoinJoinFailure::Disabled),
            E::Masternode(MasternodeFailure::WatchOnly),
        ];
        for e in nested {
            let domain = e.code();
            let code = AppError::from(e).code_str();
            assert!(
                code.starts_with(&format!("{domain}.")),
                "{code} is outside the `{domain}` namespace"
            );
        }
        let e = VaultError::WalletSecretExists;
        assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        // A contact payment's refusal keeps its DashPay code (DP3-01 D16).
        use dw_engine::platform::{ContactError, PlatformError};
        assert_eq!(
            AppError::from(E::Contact(ContactError::IsSelf)).code_str(),
            AppError::from(ContactError::IsSelf).code_str()
        );
        assert_eq!(
            AppError::from(E::Contact(ContactError::Platform(
                PlatformError::Unavailable
            )))
            .code_str(),
            "platform.unavailable"
        );
    }

    #[test]
    fn an_outpoint_rides_in_data() {
        let e = AppError::from(EngineError::OutpointNotFound(dashcore::OutPoint::null()));
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({ "kind": "outpoint", "txid": "0".repeat(64), "vout": u32::MAX })
        );
    }

    /// A refusal rides in data; the one a restart and a rescan clear says
    /// so in its message, the others name the refusal.
    #[test]
    fn a_tx_action_refusal_rides_in_data_with_its_message() {
        let e = |r| serde_json::to_value(AppError::from(EngineError::TxActionRefused(r))).unwrap();
        let rescan = e(TxActionRefusal::RescanFirst);
        assert_eq!(
            rescan["data"],
            serde_json::json!({ "kind": "txAction", "refusal": "rescanFirst", "walletId": null })
        );
        let wallet = dw_engine::WalletId([7; 32]);
        assert_eq!(
            e(TxActionRefusal::RecordUnreadable(wallet))["data"],
            serde_json::json!({
                "kind": "txAction",
                "refusal": "recordUnreadable",
                "walletId": wallet.to_string(),
            })
        );
        assert_eq!(
            rescan["message"],
            "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again."
        );
        assert_eq!(
            e(TxActionRefusal::InstantLocked)["message"],
            "transaction action refused: InstantLocked"
        );
    }

    /// Repair's refusal rides in data with its txid or status, and whether
    /// the same Repair may succeed later (E0-04 P2b).
    #[test]
    fn a_stuck_funding_refusal_rides_in_data() {
        use dw_engine::platform::lease::StuckFundingRefusal as R;
        let data = |r: R| {
            serde_json::to_value(AppError::from(EngineError::StuckFunding(r))).unwrap()["data"]
                .clone()
        };
        let txid = dashcore::OutPoint::null().txid;
        assert_eq!(
            data(R::SpendUnsettled(txid)),
            serde_json::json!({
                "kind": "stuckFunding",
                "refusal": { "kind": "spendUnsettled", "txid": "0".repeat(64) },
                "retryLater": true,
            })
        );
        assert_eq!(
            data(R::NotStuck(Some(dw_engine::platform::DispatchState::Sent))),
            serde_json::json!({
                "kind": "stuckFunding",
                "refusal": { "kind": "notStuck", "status": "sent" },
                "retryLater": false,
            })
        );
    }
}
