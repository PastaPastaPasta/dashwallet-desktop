// Engine error codes the view models branch on that `ServiceErrorCode` does
// not name yet (docs/contracts/m1-engine.md §4). Kept in their own namespace
// so a later `ServiceErrorCode` constant with the same name cannot clash.
import WalletRuntime

enum EngineCode {
    static let vaultNotEncrypted = ServiceErrorCode(rawValue: "vault.not_encrypted")
    static let vaultAlreadyEncrypted = ServiceErrorCode(rawValue: "vault.already_encrypted")
    static let vaultGrantPurposeMismatch = ServiceErrorCode(rawValue: "vault.grant_purpose_mismatch")

    static let walletNoVault = ServiceErrorCode(rawValue: "wallet.no_vault")
    static let walletVaultLocked = ServiceErrorCode(rawValue: "wallet.vault_locked")

    static let historyInvalidQuery = ServiceErrorCode(rawValue: "history.invalid_query")

    static let receiveRequestNotFound = ServiceErrorCode(rawValue: "receive.request_not_found")
    static let receiveGapLimit = ServiceErrorCode(rawValue: "receive.gap_limit")

    static let sendInsufficientMixedFunds = ServiceErrorCode(rawValue: "send.insufficient_mixed_funds")
    static let sendOutpointUnavailable = ServiceErrorCode(rawValue: "send.outpoint_unavailable")
    static let sendTxTooLarge = ServiceErrorCode(rawValue: "send.tx_too_large")
    static let sendInvalidChangeAddress = ServiceErrorCode(rawValue: "send.invalid_change_address")
    static let sendWatchOnly = ServiceErrorCode(rawValue: "send.watch_only")
    static let sendVaultLocked = ServiceErrorCode(rawValue: "send.vault_locked")
    static let sendGrantInvalid = ServiceErrorCode(rawValue: "send.grant_invalid")
    static let sendPreparedTxSpent = ServiceErrorCode(rawValue: "send.prepared_tx_spent")
    static let sendNoPeers = ServiceErrorCode(rawValue: "send.no_peers")

    static let labelsInvalidAddress = ServiceErrorCode(rawValue: "labels.invalid_address")
    static let labelsOwnAddress = ServiceErrorCode(rawValue: "labels.own_address")
    static let labelsEntryNotFound = ServiceErrorCode(rawValue: "labels.entry_not_found")
    static let labelsReceiveEntryNotDeletable = ServiceErrorCode(rawValue: "labels.receive_entry_not_deletable")

    static let messageInvalidAddress = ServiceErrorCode(rawValue: "message.invalid_address")
    static let messageAddressNoKey = ServiceErrorCode(rawValue: "message.address_no_key")
    static let messageMalformedSignature = ServiceErrorCode(rawValue: "message.malformed_signature")
    static let messagePubkeyNotRecovered = ServiceErrorCode(rawValue: "message.pubkey_not_recovered")
    static let messageAddressNotMine = ServiceErrorCode(rawValue: "message.address_not_mine")
    static let messageWatchOnly = ServiceErrorCode(rawValue: "message.watch_only")
    static let messageVaultLocked = ServiceErrorCode(rawValue: "message.vault_locked")
    static let messageGrantInvalid = ServiceErrorCode(rawValue: "message.grant_invalid")

    static let uriInvalidAmount = ServiceErrorCode(rawValue: "uri.invalid_amount")
    static let uriNotDashURI = ServiceErrorCode(rawValue: "uri.not_dash_uri")
    static let uriBip70Unsupported = ServiceErrorCode(rawValue: "uri.bip70_unsupported")
    static let uriTooLongForQR = ServiceErrorCode(rawValue: "uri.too_long_for_qr")
}
