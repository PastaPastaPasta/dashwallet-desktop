// Fallback user copy for error codes every domain shares. Feature view
// models map their own domain codes first and fall back to this.
import WalletRuntime

public enum ErrorText {
    public static func common(_ code: ServiceErrorCode) -> String {
        switch code {
        case .notImplemented: L10n.Common.notAvailableYet
        case .networkNotOpen: L10n.Common.networkNotOpen
        case .walletNotFound: L10n.Common.walletNotFound
        case .storage: L10n.Common.storage
        case .vaultWrongPassphrase: L10n.Common.wrongPassphrase
        case .vaultLocked, EngineCode.walletVaultLocked, EngineCode.sendVaultLocked, EngineCode.messageVaultLocked:
            L10n.Common.vaultLocked
        case .vaultMixingOnly: L10n.Common.mixingOnly
        case .vaultThrottled: L10n.Common.throttled
        default: L10n.Common.unexpected
        }
    }
}
