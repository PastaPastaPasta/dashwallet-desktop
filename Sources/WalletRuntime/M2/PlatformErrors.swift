// OS-service errors as service errors (m2-swift.md §2.7): the code is kept.
import Foundation
import PlatformServices

extension ServiceError {
    /// The `PlatformServiceError` with its code (`desktop.*`,
    /// `platform.denied`, `platform.cancelled`) and detail.
    public init(_ error: PlatformServiceError) {
        self.init(code: ServiceErrorCode(rawValue: error.code), detail: error.detail)
    }
}

/// Runs an OS-service call and maps its error.
func platformCall<T>(_ body: () throws(PlatformServiceError) -> T) throws(ServiceError) -> T {
    do {
        return try body()
    } catch {
        throw ServiceError(error)
    }
}

/// Async variant of `platformCall`.
func platformCall<T>(
    isolation: isolated (any Actor)? = #isolation,
    _ body: () async throws(PlatformServiceError) -> T
) async throws(ServiceError) -> T {
    do {
        return try await body()
    } catch {
        throw ServiceError(error)
    }
}
