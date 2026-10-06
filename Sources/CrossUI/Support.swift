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
    static let syncDetails = "Sync details"
    static let showQR = "Show QR"
    static let hideQR = "Hide QR"
    static let purpose = "Address list"
    static let noVault = "No vault"
    static let noKeys = "No keys"
    static let unencrypted = "Unencrypted"
    static let locked = "Locked"
    static let unlockedMixingOnly = "Unlocked for mixing only"
    static let unlocked = "Unlocked"

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
    static let done = "Done"
    static let broadcastAgain = "Broadcast again"
    static func broadcastUnknown(_ txid: String) -> String {
        "It is not known whether the network received transaction \(txid). Its coins stay reserved until they show as spent or the wallet is reopened; check Transactions before sending again."
    }

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
    static let unlockVaultPrompt = "This computer already has a wallet vault. Enter its passphrase to add the wallet to it."

    // M2 shell and common actions.
    static let toolsWindow = "Tools Window"
    static let walletsPage = "Wallets"
    static let optionsPage = "Options"
    static let securityPage = "Security"
    static let aboutPage = "About"
    static let yes = "Yes"
    static let ok = "OK"
    static let dismiss = "Dismiss"
    static let hd = "HD"
    static let working = "Working…"
    static let open = "Open"
    static let load = "Load"
    static let add = "Add"
    static let remove = "Remove"
    static let rename = "Rename"
    static let use = "Use"
    static let browse = "Browse…"
    static let choose = "Choose"
    static let quit = "Quit"
    static let reset = "Reset"
    static let abort = "Abort"
    static let copyMenu = "Copy"
    static let copyAddress = "Copy address"
    static let addressWord = "Address"
    static let labelWord = "Label"
    static let amountWord = "Amount"
    static let outpointWord = "Transaction ID and output index"
    static let rawTransactionWord = "Raw transaction"
    static let detailsWord = "Transaction details"
    static let psbtWord = "PSBT"
    static func copied(_ what: String) -> String { "\(what) copied to the clipboard." }
    static func nothingToCopy(_ what: String) -> String { "There is no \(what.lowercased()) to copy." }
    static func noClipboard(_ what: String, _ text: String) -> String {
        "No clipboard tool (wl-copy or xclip) is installed, so \(what.lowercased()) was not copied. Select it here: \(text)"
    }
    static let noClipboardTool =
        "No clipboard tool (wl-copy or xclip) is installed: paste the PSBT as base64 below instead."

    // Tools window.
    static let fontSmaller = "A−"
    static let fontBigger = "A+"
    static let clearConsole = "Clear console"
    static let consoleCommand = "Console command"
    static let consolePlaceholder = "Enter a command, for example help"
    static let consoleNeedsPassphrase = "This command needs your wallet passphrase."
    static let run = "Run"
    static let historyUp = "Previous command"
    static let historyDown = "Next command"
    static let complete = "Complete"
    static let banDuration = "Ban duration"
    static let ban = "Ban"
    static let bannedPeers = "Banned peers"
    static let rescanning = "Rescanning…"
    static let blockHeight = "Block height"

    // Options.
    static let optionsSaved = "Options saved."
    static let noTray =
        "This desktop has no tray icon support in Dash Wallet yet; the tray options are stored but have no effect."
    static let thirdPartyPlaceholder = "https://example.com/tx/%s|https://other.example/tx/%s"
    static let currencySearch = "Search currencies"
    static let currencyListOnly = "Exchange rates arrive in a later release; the currency is stored for then."
    static let resetDone = "The settings were reset. The previous files were saved as:"

    // Coin selection.
    static let coinMode = "Coin selection mode"
    static let noCoins = "No coins."
    static let sortBy = "Sort by:"
    static let lockedCoin = "(locked)"
    static let summaryUnavailable = "The selection summary is not available yet."
    static let customChangeUnavailable =
        "A custom change address cannot be used yet: the Send page does not pass one to the transaction."
    static func selectCoin(_ amount: String, _ address: String) -> String { "Select \(amount) at \(address)" }
    static func coinGroup(address: String, label: String, count: Int, total: String) -> String {
        "\(address)  \(label)  (\(count == 1 ? "1 coin" : "\(count) coins"), \(total))"
    }

    // Send / PSBT.
    static let createUnsignedUnavailable =
        "Create Unsigned is not available yet: the Send page cannot hand its prepared transaction to the PSBT dialog."
    static let loadPSBT = "Load a partially signed transaction"
    static let psbtBase64 = "PSBT (base64)"
    static let psbtBase64Placeholder = "Paste a base64 PSBT"

    // Transactions.
    static let groupByDay = "Group by day"
    static func mixingCount(_ count: Int) -> String { count == 1 ? "1 transaction" : "\(count) transactions" }

    // Wallets.
    static let copyXpub = "Copy"
    static let derivationPath = "Derivation path"
    static let filePassphrase = "File passphrase"
    static func filePassphrasePrompt(_ name: String) -> String { "\(name) is encrypted. Enter its passphrase." }
    static let typeSentence = "Type the sentence above"
    static let closeWallet = "Close"
    static let openWallet = "Open"
    static let xpub = "Extended public key"
    static func loadOnStartup(_ name: String) -> String { "Open \(name) on startup" }
    static let walletName = "Wallet name"
    static func walletState(loaded: Bool, watchOnly: Bool) -> String {
        (loaded ? "Open" : "Closed") + (watchOnly ? ", watch-only" : "")
    }
    static let loadStatesUnavailable = "Closed wallets cannot be listed yet; the list shows the open wallets."
    static let importSection = "Import"
    static let importFileHelp =
        "A Dash Core dumpwallet file, a wallet.dat (SQLite) or a Dash Wallet backup (.dwbackup)."
    static let importFile = "Import File…"
    static let importTitle = "Import"
    static let keyMaterial = "Key material"
    static let keyMaterialKind = "Kind"
    static func keyMaterialName(_ kind: KeyMaterialKind) -> String {
        switch kind {
        case .hdSeed: "HD seed (hex)"
        case .xprv: "Extended private key"
        case .descriptors: "Descriptors (JSON)"
        }
    }
    static let watchOnlyWallet = "Watch-only wallet"
    static let xpubField = "Account xpub"
    static let birthHeightField = "Birth height"
    static let backupPassphrase = "Backup passphrase"
    static let automaticBackups = "Automatic backups"
    static let exportForCore = "Export for Dash Core"
    static let exportFormat = "Format"
    static func exportFormatName(_ format: CoreExportFormat) -> String {
        switch format {
        case .dumpWallet: "dumpwallet file"
        case .importDescriptorsJSON: "importdescriptors JSON"
        }
    }
    static let exportTitle = "Export…"
    static let uriPlaceholder = "dash:… payment URI"
    static func walletOperation(_ operation: WalletOperation) -> String {
        switch operation {
        case .opening: "Opening wallet…"
        case .closing: "Closing wallet…"
        case .renaming: "Renaming wallet…"
        case .removing: "Removing wallet…"
        case .importing: "Importing…"
        case .restoring: "Restoring backup…"
        case .backingUp: "Backing up…"
        case .exporting: "Exporting…"
        case .deleting: "Deleting all wallet data…"
        }
    }

    // Security.
    static let quickUnlock = "Quick unlock"
    static let quickUnlockUnavailable =
        "This computer has no biometric unlock that Dash Wallet supports; unlock with your passphrase."
    static let quickUnlockNeedsEncryption = "Encrypt the wallet first to use quick unlock."

    // Startup.
    static let customDirectory = "Custom data directory"

    // About.
    static let versionTitle = "Version"
    static let documentation = "Documentation"
    static let appOptions = "Options of this app"
}
