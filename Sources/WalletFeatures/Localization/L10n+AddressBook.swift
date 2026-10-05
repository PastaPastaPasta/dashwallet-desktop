import Foundation

extension L10n {
    /// dash-qt `AddressBookPage` / `EditAddressDialog` copy (QT-095…098).
    public enum AddressBook {
        public static let sendingHeader =
            "These are your Dash addresses for sending payments. Always check the amount and the receiving address before sending coins."
        public static let receivingHeader =
            "These are your Dash addresses for receiving payments. Use the 'Create new receiving address' button in the receive tab to create new addresses."
        public static let searchPlaceholder = "Enter address or label to search"
        public static let noLabel = "(no label)"
        public static let newSendingAddress = "New sending address"
        public static let editSendingAddress = "Edit sending address"
        public static let editReceivingAddress = "Edit receiving address"
        public static let receivingNotDeletable = "Receiving addresses cannot be deleted."
        public static let entryNotFound = "This address is not in the address book."

        public static func invalidAddress(_ address: String) -> String {
            "The entered address \"\(address)\" is not a valid Dash address."
        }

        public static func existsAsReceiving(_ address: String, label: String) -> String {
            "Address \"\(address)\" already exists as a receiving address with label \"\(label)\" and so cannot be added as a sending address."
        }

        public static func alreadyInBook(_ address: String, label: String) -> String {
            "The entered address \"\(address)\" is already in the address book with label \"\(label)\"."
        }

        public static func sendingWindowTitle(_ wallet: String) -> String { "Sending addresses - \(wallet)" }
        public static func receivingWindowTitle(_ wallet: String) -> String { "Receiving addresses - \(wallet)" }
    }
}
