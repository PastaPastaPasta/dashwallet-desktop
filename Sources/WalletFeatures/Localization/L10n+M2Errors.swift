import Foundation
import WalletRuntime

extension L10n {
    /// Copy for the M2 error codes (m2-engine.md §4). View models choose it by
    /// code, never from `ServiceError.detail`.
    public enum M2Errors {
        public static let txNotFound = "The transaction could not be found."
        public static let txRefused = "This action is not available for this transaction."
        public static let spvNotRunning = "Sync is not running. Start it and try again."
        public static let noPeers = "No peers are connected. Try again when the wallet is connected to the network."
        public static let fileUnreadable = "The file could not be read."
        public static let unsupportedFormat = "This file format is not supported."
        public static let corrupt = "The file is damaged and cannot be used."
        public static let filePassphraseRequired = "This wallet file is encrypted. Enter its passphrase."
        public static let noHDChain =
            "This file has no recovery phrase, HD seed or extended private key to rebuild the wallet from."
        public static let networkMismatch = "This file belongs to another network."
        public static let invalidKeyMaterial = "The key material is not valid."
        public static let alreadyExists = "This wallet already exists."
        public static let noVault = "Create or restore a wallet first."
        public static let grantInvalid = "The authorization expired. Please try again."
        public static let watchOnly = "Watch-only wallets have no private keys."
        public static let destinationUnwritable = "There was an error trying to save the data to this location."
        public static let backupPassphraseRequired = "Enter a passphrase for the backup."
        public static let backupWrongPassphrase = "The backup passphrase is incorrect."
        public static let backupUnsupportedVersion = "This backup was made by a newer version of Dash Wallet."
        public static let psbtInvalid = "Unable to decode PSBT"
        public static let psbtTooLarge = "PSBT file must be smaller than 100 MiB"
        public static let psbtNotComplete = "Transaction is not fully signed yet."
        public static let psbtFeeRateTooHigh = "The fee rate is above the broadcast limit of 0.1 DASH/kB."
        public static let psbtCannotSign = "This wallet cannot sign transactions."
        public static let lockedSign = "Cannot sign inputs while wallet is locked."
        public static let grantExceeded = "The amount is more than was authorized. Please try again."
        public static let broadcastRejected = "The network rejected the transaction."
        public static let broadcastUnknown =
            "The transaction was handed to the network, but whether it was accepted is unknown. Check the transaction list before trying again."
        public static let unsupported = "This is not supported on this system."
        public static let osError = "The operating system reported an error."
        public static let noQRCode = "No QR code was found in the image."
        public static let imageUnreadable = "The image could not be read."
        public static let quickUnlockLimitExceeded = "This payment is above the quick-unlock spending limit. Enter your passphrase."
        public static let passphraseStale =
            "Your passphrase was last entered more than 7 days ago. Enter your passphrase."
        public static let quickUnlockUnavailable = "Quick unlock is not available on this computer."
        public static let vaultNotEmpty = "Remove every wallet before deleting the wallet data."
        public static let recoveryMismatch = "This recovery phrase does not belong to the selected wallet."
        public static let invalidXpub = "This is not a valid extended public key for this network."
        public static let nameRejected = "Wallet names must be between 1 and 64 characters."
        public static let spvRunning = "Stop syncing first."
        public static let rescanInProgress = "Wallet is currently rescanning. Abort existing rescan or wait."
        public static let peerNotFound = "The peer is no longer connected."
        public static let heightOutOfRange = "The height is above the current chain tip."
        public static let denied = "Permission was denied."
        public static let cancelled = "Cancelled."
        public static let authTimedOut = "Authorization timed out. Please try again."
        public static let invalidArgument = "The value entered is not valid."
    }
}

extension ErrorText {
    /// Copy for any code an M2 call throws; falls back to `common`.
    public static func m2(_ code: ServiceErrorCode) -> String {
        typealias E = L10n.M2Errors
        switch code {
        case .txActionTxNotFound, .historyTxNotFound: return E.txNotFound
        case .txActionRefused: return E.txRefused
        case .txActionSpvNotRunning, .syncSpvNotRunning: return E.spvNotRunning
        case .txActionNoPeers, .psbtNoPeers, .sendNoPeers: return E.noPeers
        case .compatFileUnreadable: return E.fileUnreadable
        case .compatUnsupportedFormat: return E.unsupportedFormat
        case .compatCorrupt, .backupCorrupt: return E.corrupt
        case .compatPassphraseRequired: return E.filePassphraseRequired
        case .compatWrongPassphrase: return L10n.Common.wrongPassphrase
        case .compatNoHDChain: return E.noHDChain
        case .compatNetworkMismatch, .backupNetworkMismatch, .psbtNetworkMismatch: return E.networkMismatch
        case .compatInvalidKeyMaterial: return E.invalidKeyMaterial
        case .compatAlreadyExists, .backupAlreadyExists, .walletAlreadyExists: return E.alreadyExists
        case .compatNoVault, .walletNoVault, .vaultNoVault: return E.noVault
        case .compatVaultLocked, .backupVaultLocked: return L10n.Common.vaultLocked
        case .compatGrantInvalid, .psbtGrantInvalid, .vaultGrantInvalid, .walletGrantInvalid: return E.grantInvalid
        case .compatWatchOnly: return E.watchOnly
        case .compatDestinationUnwritable, .backupDestinationUnwritable: return E.destinationUnwritable
        case .backupPassphraseRequired: return E.backupPassphraseRequired
        case .backupWrongPassphrase: return E.backupWrongPassphrase
        case .backupUnsupportedVersion: return E.backupUnsupportedVersion
        case .psbtInvalid: return E.psbtInvalid
        case .psbtTooLarge: return E.psbtTooLarge
        case .psbtNotComplete: return E.psbtNotComplete
        case .psbtFeeRateTooHigh: return E.psbtFeeRateTooHigh
        case .psbtWatchOnly: return E.psbtCannotSign
        case .psbtVaultLocked: return E.lockedSign
        case .psbtGrantExceeded: return E.grantExceeded
        case .psbtBroadcastRejected, .sendBroadcastRejected: return E.broadcastRejected
        case .psbtBroadcastUnknown, .sendBroadcastUnknown: return E.broadcastUnknown
        case .desktopUnsupported: return E.unsupported
        case .desktopOSError: return E.osError
        case .desktopNoQRCode: return E.noQRCode
        case .desktopImageUnreadable: return E.imageUnreadable
        case .vaultQuickUnlockLimitExceeded: return E.quickUnlockLimitExceeded
        case .vaultPassphraseStale: return E.passphraseStale
        case .vaultQuickUnlockUnavailable: return E.quickUnlockUnavailable
        case .vaultNotEmpty: return E.vaultNotEmpty
        case .vaultRecoveryMismatch: return E.recoveryMismatch
        case .vaultPassphraseRejected: return L10n.Onboarding.passphraseRejected
        case .walletInvalidXpub: return E.invalidXpub
        case .walletNameRejected: return E.nameRejected
        case .syncSpvRunning: return E.spvRunning
        case .syncRescanInProgress: return E.rescanInProgress
        case .syncPeerNotFound: return E.peerNotFound
        case .syncHeightOutOfRange: return E.heightOutOfRange
        case .platformDenied: return E.denied
        case .platformCancelled: return E.cancelled
        case .settingsWriteFailed: return L10n.Settings.settingsNotSaved
        case .authTimedOut: return E.authTimedOut
        case .invalidArgument: return E.invalidArgument
        default: return common(code)
        }
    }
}
