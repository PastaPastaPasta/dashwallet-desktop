// Touch ID store for the vault's slot B wrap key (IOS-011, DESIGN-opus
// §1.8): a data-protection keychain item whose access control requires the
// current biometric set.
#if os(macOS)
import Foundation
import LocalAuthentication
import PlatformServices
import Security

/// `BiometricKeyStoring` over the keychain.
///
/// - The item is a generic password (service `<service>`, account = the
///   network), `kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`, with
///   `SecAccessControl` `.biometryCurrentSet`: it never leaves the device,
///   and adding or removing a fingerprint invalidates it (the user then
///   enrols again with the passphrase).
/// - `retrieve` asks for Touch ID with `LAContext` first, then reads the
///   item with that context, so the key is handed out only after a
///   successful prompt. Cancel → `platform.cancelled`; no biometrics,
///   lockout or a refused match → `platform.denied`; a missing item →
///   `vault.quick_unlock_unavailable`.
/// - The data-protection keychain needs a signed app with a
///   `keychain-access-groups` entitlement; elsewhere (`swift test`) the
///   keychain answers `errSecMissingEntitlement`, reported as
///   `desktop.os_error`.
///
/// Residual risk: the bytes Security returns arrive in an immutable
/// `CFData` that cannot be wiped; they are copied into a zeroing
/// `BiometricKey` at once. On `store`, the key is passed as a no-copy
/// `CFData` over a buffer this type zeroes; Security's internal copies are
/// out of our reach.
public struct MacBiometricKeyStore: BiometricKeyStoring {
    public static let defaultService = "org.dashfoundation.DashWallet.quick-unlock"

    private let service: String

    public init(service: String = MacBiometricKeyStore.defaultService) {
        self.service = service
    }

    public var kind: BiometricKind {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else { return .none }
        return context.biometryType == .touchID ? .touchID : .none
    }

    private func baseQuery(network: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: network,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    public func store(_ key: BiometricKey, network: String) throws(PlatformServiceError) {
        var accessError: Unmanaged<CFError>?
        guard
            let access = SecAccessControlCreateWithFlags(
                nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, .biometryCurrentSet, &accessError)
        else {
            let detail = accessError.map { String(describing: $0.takeRetainedValue()) } ?? "access control"
            throw PlatformServiceError(code: "desktop.os_error", detail: detail)
        }
        try delete(network: network)
        // The key goes in as a CFData over our own buffer, which is zeroed
        // once SecItemAdd returned (Security keeps its own copy).
        let bytes = UnsafeMutableRawBufferPointer.allocate(byteCount: max(key.count, 1), alignment: 1)
        defer {
            if let base = bytes.baseAddress { _ = memset_s(base, bytes.count, 0, bytes.count) }
            bytes.deallocate()
        }
        key.withUnsafeBytes { source in
            if let from = source.baseAddress { bytes.baseAddress!.copyMemory(from: from, byteCount: source.count) }
        }
        let status: OSStatus = {
            guard
                let data = CFDataCreateWithBytesNoCopy(
                    nil, bytes.baseAddress!.assumingMemoryBound(to: UInt8.self), key.count, kCFAllocatorNull)
            else { return errSecAllocate }
            var query = baseQuery(network: network)
            query[kSecAttrAccessControl as String] = access
            query[kSecValueData as String] = data
            return SecItemAdd(query as CFDictionary, nil)
        }()
        guard status == errSecSuccess else { throw Self.error(status, "storing the quick-unlock key") }
    }

    public func retrieve(network: String, reason: String) async throws(PlatformServiceError) -> BiometricKey {
        let context = LAContext()
        do {
            _ = try await context.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, localizedReason: reason)
        } catch let error as LAError {
            switch error.code {
            case .userCancel, .appCancel, .systemCancel, .userFallback:
                throw PlatformServiceError(code: "platform.cancelled", detail: error.localizedDescription)
            default:
                throw PlatformServiceError(code: "platform.denied", detail: error.localizedDescription)
            }
        } catch {
            throw PlatformServiceError(code: "platform.denied", detail: error.localizedDescription)
        }
        var query = baseQuery(network: network)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        query[kSecUseAuthenticationContext as String] = context
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else {
            throw Self.error(status, "reading the quick-unlock key")
        }
        return data.withUnsafeBytes { BiometricKey(copying: $0) }
    }

    public func delete(network: String) throws(PlatformServiceError) {
        let status = SecItemDelete(baseQuery(network: network) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw Self.error(status, "deleting the quick-unlock key")
        }
    }

    private static func error(_ status: OSStatus, _ action: String) -> PlatformServiceError {
        let message = (SecCopyErrorMessageString(status, nil) as String?) ?? "OSStatus \(status)"
        switch status {
        case errSecItemNotFound:
            return PlatformServiceError(code: "vault.quick_unlock_unavailable", detail: "\(action): \(message)")
        case errSecUserCanceled:
            return PlatformServiceError(code: "platform.cancelled", detail: "\(action): \(message)")
        case errSecAuthFailed, errSecInteractionNotAllowed:
            return PlatformServiceError(code: "platform.denied", detail: "\(action): \(message)")
        default:
            return PlatformServiceError(code: "desktop.os_error", detail: "\(action): \(message) (\(status))")
        }
    }
}
#endif
