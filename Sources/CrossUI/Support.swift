// Small helpers shared by the CrossUI screens: bindings onto main-actor view
// models, text for runtime values, and strings the view-model tables do not
// carry yet.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

/// A binding whose getter and setter run on the main actor. SwiftCrossUI
/// calls bindings from the UI thread, which is the main thread on every backend.
@MainActor
func bind<T: Sendable>(_ get: @escaping @MainActor () -> T, _ set: @escaping @MainActor (T) -> Void) -> Binding<T> {
    Binding(
        get: { MainActor.assumeIsolated { get() } },
        set: { value in MainActor.assumeIsolated { set(value) } })
}

/// A page body: a scrollable column with the page title on top. Pages are
/// top-aligned (ADR 0002 noted SwiftCrossUI centres stacks vertically).
struct Page<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Int(DashSpacing.l)) {
                SectionHeader(title, style: .title2)
                content
                Spacer()
            }
            .padding(Int(DashSpacing.xl))
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }
}

enum Format {
    static let dateTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm"
        return formatter
    }()

    static let day: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }()

    static func date(_ date: Date?) -> String {
        date.map { dateTime.string(from: $0) } ?? L10n.Common.unknown
    }

    static func height(_ height: UInt32?) -> String {
        height.map(String.init) ?? L10n.Common.unknown
    }

    static func network(_ network: DashNetwork?) -> String {
        network.map(L10n.Settings.networkName) ?? L10n.Common.unknown
    }

    static func lockState(_ state: VaultLockState?) -> String {
        switch state {
        case nil: L10n.Common.unknown
        case .noVault: CrossStrings.noVault
        case .noKeys: CrossStrings.noKeys
        case .unencrypted: CrossStrings.unencrypted
        case .locked: CrossStrings.locked
        case .unlockedMixingOnly: CrossStrings.unlockedMixingOnly
        case .unlocked: CrossStrings.unlocked
        }
    }

    static func transition(_ transition: LifecycleTransition) -> String {
        switch transition {
        case .idle: ""
        case .starting(let network): "Starting \(L10n.Settings.networkName(network))…"
        case .stopping(let network): "Stopping \(L10n.Settings.networkName(network))…"
        case .switchingNetwork(_, let to): "Switching to \(L10n.Settings.networkName(to))…"
        case .addingWallet: "Adding wallet…"
        case .removingWallet: "Removing wallet…"
        }
    }

    static func direction(_ category: TxCategory, amount: Amount) -> TransactionDirection {
        switch category {
        case .internalTransfer, .coinJoin: .internalTransfer
        default: amount.duffs < 0 ? .outgoing : .incoming
        }
    }
}

/// Copy the CrossUI screens need that `L10n` (WalletFeatures) does not have.
/// English only; the translation pipeline picks these up with the rest.
enum CrossStrings {
    static let addressBook = "Address Book"
    static let signVerify = "Sign / Verify Message"
    static let settings = "Settings"
    static let lockWallet = "Lock Wallet"
    static let tools = "Tools"
    static let wallet = "Wallet"
    static let loading = "Loading wallet…"
    static let close = "Close"
    static let back = "Back"
    static let cancel = "Cancel"
    static let yes = "Yes"
    static let no = "No"
    static let save = "Save"
    static let delete = "Delete"
    static let show = "Show"
    static let continueTitle = "Continue"
    static let exportCSV = "Export…"
    static let exported = "Saved to"
    static let exportCancelled = "Export cancelled or not supported by this desktop."
    static let balances = "Balances"
    static let recentTransactions = "Recent transactions"
    static let noTransactions = "No transactions yet."
    static let hideBalances = "Discreet mode"
    static let peers = "peers"
    static let noVault = "No vault"
    static let noKeys = "No keys"
    static let unencrypted = "Unencrypted"
    static let locked = "Locked"
    static let unlockedMixingOnly = "Unlocked for mixing only"
    static let unlocked = "Unlocked"
    static let demoMode = "Demo mode: sample data, nothing is sent"

    // Send.
    static let payTo = "Pay To:"
    static let payToPlaceholder = "Pay to: Dash address"
    static let amount = "Amount:"
    static let amountPlaceholder = "Amount to send"
    static let label = "Label:"
    static let labelPlaceholder = "Label for the address book"
    static let subtractFee = "Subtract fee from amount"
    static let useMax = "Use available balance"
    static let removeRecipient = "Remove recipient"
    static let addRecipient = "Add Recipient"
    static let clearAll = "Clear All"
    static let pasteURI = "Paste an address or dash: URI"
    static let pasteCaption = "Paste"
    static let feeTarget = "Confirmation time target:"
    static let customFee = "Custom fee (duffs per kB)"
    static let customFeePlaceholder = "Custom fee in duffs per kB"
    static let requestAmountPlaceholder = "Amount to request (optional)"
    static let requestMessagePlaceholder = "Message for the payer (optional)"
    static let applyCustomFee = "Use custom fee"
    static let recommendedFee = "Use recommended fee"
    static let message = "Message:"
    static let passphrase = "Passphrase"
    static let walletPassphrase = "Wallet passphrase"
    static let authorize = "Authorize"
    static let preparing = "Preparing transaction…"
    static let broadcasting = "Sending transaction…"
    static func sent(_ txid: String) -> String { "Transaction sent: \(txid)" }
    static let viewTransaction = "View transaction"
    static let done = "Done"

    // Receive.
    static let newAddress = "New address"
    static let requestPayment = "Request payment"
    static let clear = "Clear"
    static let requests = "Requested payments history"
    static let address = "Address"
    static let uri = "URI"

    // Transactions.
    static let date = "Date"
    static let type = "Type"
    static let search = "Search"
    static let minAmount = "Min amount"
    static let watchOnly = "Watch-only"
    static let from = "From (YYYY-MM-DD)"
    static let until = "To (YYYY-MM-DD)"
    static let applyRange = "Apply range"
    static let loadMore = "Load more"
    static let transactionDetails = "Transaction details"
    static let status = "Status"
    static let transactionID = "Transaction ID"
    static let fee = "Transaction fee"
    static let size = "Size"
    static let block = "Block"
    static let inputs = "Inputs"
    static let outputs = "Outputs"
    static let saveLabel = "Save label"
    static func matching(_ count: Int) -> String { "\(count) transactions" }

    // Address book.
    static let sending = "Sending addresses"
    static let receiving = "Receiving addresses"
    static let noEntries = "No entries."

    // Sign / verify.
    static let signMessage = "Sign Message"
    static let verifyMessage = "Verify Message"
    static let signature = "Signature"
    static let sign = "Sign Message"
    static let verify = "Verify Message"
    static let signingAddress = "Address to sign the message with"
    static let verifyingAddress = "Address the message was signed with"
    static let messageToSign = "Message to sign"
    static let messageToVerify = "Message that was signed"
    static let signatureToVerify = "Signature to verify"

    // Settings.
    static let network = "Network"
    static let display = "Display"
    static let unit = "Unit to show amounts in"
    static let decimalDigits = "Decimal digits"
    static let theme = "Theme"
    static let security = "Security"
    static let vaultStatus = "Wallet encryption"
    static let encryptWallet = "Encrypt Wallet…"
    static let newPassphrase = "New passphrase"
    static let repeatPassphrase = "Repeat new passphrase"
    static let oldPassphrase = "Old passphrase"
    static let changePassphrase = "Change Passphrase…"
    static let showRecoveryPhrase = "Show Recovery Phrase"
    static let hideRecoveryPhrase = "Hide Recovery Phrase"
    static let dataDirectory = "Data directory"

    // Onboarding.
    static let wordCount = "Recovery phrase length"
    static func words(_ count: Int) -> String { "\(count) words" }
    static let wroteItDown = "I wrote it down"
    static func selectWord(_ position: Int) -> String { "Select word #\(position)" }
    static let encryptWithPassphrase = "Encrypt wallet"
    static let skipEncryption = "Continue without encryption"
    static let strength = "Strength:"
    static let recoveryPhrase = "Recovery phrase"
    static let recoveryPhrasePlaceholder = "Recovery phrase words separated by spaces"
    static let bip39Passphrase = "BIP39 passphrase (optional)"
    static let coreCompatible = "Dash Core compatibility"
    static let birthHeight = "Scan from block height (0 = from the start)"
    static let creatingWallet = "Creating your wallet…"
    static let walletReady = "Your wallet is ready."
    static let suggestions = "Suggestions:"

    // Lock.
    static let unlock = "Unlock"
    static let unlockForMixingOnly = "Unlock for mixing only"
}
