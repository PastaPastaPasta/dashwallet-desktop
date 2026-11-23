import Foundation

extension L10n {
    /// dash-qt `ReceiveCoinsDialog` / `ReceiveRequestDialog` copy (QT-081…084).
    public enum Receive {
        public static let formHeader = "Use this form to request payments. All fields are optional."
        public static let labelPlaceholder = "Enter a label to associate with the new receiving address"
        public static let uriTooLong = "Resulting URI too long, try to reduce the text for label / message."
        public static let couldNotGenerate = "Could not generate new address"
        public static let couldNotUnlock = "Could not unlock wallet."
        public static let noLabel = "(no label)"
        public static let noMessage = "(no message)"
        public static let noAmount = "(no amount requested)"
        public static let invalidAmount = "The amount is not valid."

        public static func requestTitle(_ labelOrAddress: String) -> String { "Request payment to \(labelOrAddress)" }
    }
}
