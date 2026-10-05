import Foundation

extension L10n {
    /// dash-qt `SignVerifyMessageDialog` copy (QT-099/100).
    public enum SignVerify {
        public static let windowTitle = "Signatures - Sign / Verify a Message"
        public static let invalidAddress = "The entered address is invalid. Please check the address and try again."
        public static let addressNoKey =
            "The entered address does not refer to a key. Please check the address and try again."
        public static let privateKeyUnavailable = "Private key for the entered address is not available."
        public static let signingFailed = "Message signing failed."
        public static let signed = "Message signed."
        public static let malformedSignature =
            "The signature could not be decoded. Please check the signature and try again."
        public static let digestMismatch =
            "The signature did not match the message digest. Please check the signature and try again."
        public static let verificationFailed = "Message verification failed."
        public static let verified = "Message verified."
    }
}
