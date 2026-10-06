// English copy for macOS-only chrome: menus, buttons, window titles and
// status-bar tooltips. Feature copy lives in WalletFeatures `L10n`.
#if os(macOS)
import Foundation

enum MacStrings {
    enum App {
        static let demoBadge = "Demo"
        static let demoHelp = "Demo mode: sample data, no network, nothing is sent."
        static let runtimeUnavailableTitle = "The wallet could not start"
        static func runtimeFailed(_ code: String) -> String { "The wallet engine could not be opened (\(code))." }
        static func launchFailed(_ code: String) -> String { "The network could not be opened (\(code))." }
        static let retry = "Try Again"
        static let aboutTitle = "About Dash Wallet"
        static let aboutBody =
            "Dash Wallet for desktop. Released under the MIT licence. Built on dashpay/platform's platform-wallet."
        static let openDemo = "Relaunch with --demo to try the app with sample data."
        static let dataFolder = "Data folder"
        static let unknown = "Unknown"
    }

    enum Menu {
        static let openURI = "Open URI…"
        static let openURIPrompt = "Open payment request from URI"
        static let backupWallet = "Backup Wallet…"
        static let backupUnavailable = "Backup is not available yet"
        static let signMessage = "Sign Message…"
        static let verifyMessage = "Verify Message…"
        static let wallet = "Wallet"
        static let encryptWallet = "Encrypt Wallet…"
        static let changePassphrase = "Change Passphrase…"
        static let showRecoveryPhrase = "Show Recovery Phrase…"
        static let lockWallet = "Lock Wallet"
        static let discreetMode = "Discreet Mode"
        static let sendingAddresses = "Sending Addresses"
        static let receivingAddresses = "Receiving Addresses"
        static let about = "About Dash Wallet"
        static let help = "Dash Wallet Help"
        static let openWallet = "Open Dash Wallet"
        static let quit = "Quit Dash Wallet"
    }

    enum Toolbar {
        static let wallet = "Wallet"
        static let showBalances = "Show balances"
        static let hideBalances = "Hide balances (discreet mode)"
        static let lock = "Lock wallet"
    }

    enum Status {
        static let unitHelp = "Unit to show amounts in. Click to change."
        static let hdEnabled = "HD key generation is enabled"
        static let unlocked = "Wallet is encrypted and currently unlocked"
        static let mixingOnly = "Wallet is encrypted and currently unlocked for mixing only"
        static let locked = "Wallet is encrypted and currently locked"
        static let synced = "Up to date"
        static let syncing = "Synchronizing with network…"
        static func connections(_ count: UInt32) -> String {
            count == 1 ? "1 active connection to Dash network" : "\(count) active connections to Dash network"
        }
        static func behind(_ text: String) -> String { "\(text) behind" }
    }

    enum SyncOverlay {
        static let title = "Recent transactions may not yet be visible"
        static let body =
            "The wallet is still synchronizing with the Dash network, so its balance may be incorrect. Spending coins from transactions not shown yet will not be accepted by the network."
        static let status = "Status"
        static let blocksLeft = "Number of blocks left"
        static let lastBlockTime = "Last block time"
        static let progress = "Progress"
        static let progressPerHour = "Progress increase per hour"
        static let timeLeft = "Estimated time left until synced"
        static let hide = "Hide"
        static let show = "Show synchronization details"
    }

    enum Peers {
        static let title = "Peers"
        static let show = "Show Peers…"
        static let address = "Address"
        static let userAgent = "User Agent"
        static let height = "Height"
        static let ping = "Ping"
        static let direction = "Direction"
        static let inbound = "Inbound"
        static let outbound = "Outbound"
        static let none = "Not connected to any peer."
        static let changePeers = "Change Peers"
        static let changePeersHelp = "Disconnect from the current peers and connect to others."
    }

    enum Transition {
        static func starting(_ network: String) -> String { "Opening \(network)…" }
        static func stopping(_ network: String) -> String { "Closing \(network)…" }
        static func switching(_ network: String) -> String { "Switching to \(network)…" }
        static let addingWallet = "Adding wallet…"
        static let removingWallet = "Removing wallet…"
    }

    enum Common {
        static let ok = "OK"
        static let cancel = "Cancel"
        static let close = "Close"
        static let copy = "Copy"
        static let copyAddress = "Copy Address"
        static let copyURI = "Copy URI"
        static let paste = "Paste"
        static let back = "Back"
        static let `continue` = "Continue"
        static let done = "Done"
        static let save = "Save"
        static let delete = "Delete"
        static let edit = "Edit"
        static let export = "Export…"
        static let yes = "Yes"
        static let clear = "Clear"
        static let error = "Error"
        static let passphrase = "Passphrase"
        static let newPassphrase = "New passphrase"
        static let repeatPassphrase = "Repeat new passphrase"
        static let oldPassphrase = "Old passphrase"
        static let exportSaved = "Saved."
    }

    enum Onboarding {
        static let subtitle = "A wallet for Dash on your desktop."
        static let network = "Network"
        static let wordCount = "Recovery phrase length"
        static func words(_ count: Int) -> String { "\(count) words" }
        static let phraseTitle = "Your recovery phrase"
        static let writtenDown = "I wrote it down"
        static let verifyTitle = "Verify your recovery phrase"
        static func wordNumber(_ position: Int) -> String { "Word #\(position + 1)" }
        static let passphraseTitle = "Encrypt your wallet"
        static let passphraseBody = "Choose a passphrase. You need it to send coins and to see the recovery phrase."
        static let encrypt = "Encrypt and finish"
        static let skipEncryption = "Continue without encryption"
        static let skipHelp = "Advanced: the wallet keys are stored on this Mac without a passphrase. Anyone with access to your user account can spend the coins."
        static let restoreTitle = "Restore from recovery phrase"
        static let restoreBody = "Enter your 12, 15, 18, 21 or 24 words, separated by spaces."
        static let suggestions = "Suggestions"
        static let optionsTitle = "Restore options"
        static let bip39Passphrase = "BIP39 passphrase (optional)"
        static let coreCompatible = "Dash Core compatibility"
        static let coreCompatibleHelp = "Accept phrases created by Dash Core with its legacy checksum (QT-104)."
        static let knowDate = "I know when the wallet was created"
        static let createdOn = "Created on"
        static let scanFromGenesis = "Scan from the first block"
        static func birthHeight(_ height: UInt32) -> String { "Scanning starts at block \(height)." }
        static let restore = "Restore"
        static let working = "Setting up your wallet…"
        static let failedTitle = "The wallet could not be set up"
        static let captureProtected = "Screenshots and screen recordings of this window are blocked while the phrase is shown."
    }

    enum Lock {
        static let unlock = "Unlock"
        static let mixingOnly = "For mixing only"
        static let quickReceive = "Quick Receive"
    }

    enum Overview {
        static let balance = "Balance"
        static let balances = "Balances"
        static let recent = "Recent transactions"
        static let noTransactions = "No transactions yet."
        static let hiddenRecent = "Recent transactions are hidden in discreet mode."
    }

    enum Send {
        static let payTo = "Pay To"
        static let outcomeUnknownTitle = "The transaction may have been sent"
        static let outcomeUnknownText =
            "The wallet could not confirm whether the network received the transaction. Its coins stay reserved. Check Transactions before you send again."
        static let showTransaction = "Show Transaction"
        static let payToPlaceholder = "Enter a Dash address (e.g. yXdY…) or paste a dash: URI"
        static let chooseAddress = "Choose from address book"
        static let pasteAddress = "Paste address from clipboard"
        static let label = "Label"
        static let labelPlaceholder = "Enter a label for this address to add it to the list of used addresses"
        static let amount = "Amount"
        static let subtractFee = "Subtract fee from amount"
        static let message = "Message"
        static let removeRecipient = "Remove this recipient"
        static let addRecipient = "Add Recipient"
        static let clearAll = "Clear All"
        static let coinControl = "Coin Control Features…"
        static let coinControlUnavailable = "Coin control is not available yet."
        static let transactionFee = "Transaction Fee"
        static let recommended = "Recommended"
        static let custom = "Custom"
        static let confirmationTime = "Confirmation time target"
        static let perKilobyte = "per kilobyte"
        static let estimate = "Estimated fee"
        static let preparing = "Preparing transaction…"
        static let broadcasting = "Sending…"
        static let authorizeTitle = "Unlock wallet"
        static let authorizePrompt = "Enter your wallet passphrase to authorize this payment."
        static let sent = "Transaction sent"
        static let review = "Send"
        static func recipient(_ number: Int) -> String { "Recipient \(number)" }
    }

    enum Receive {
        static let title = "Receive"
        static let yourAddress = "Your Dash address"
        static let newAddress = "New Address"
        static let requestPayment = "Request payment"
        static let amount = "Amount"
        static let label = "Label"
        static let message = "Message"
        static let createRequest = "Create new receiving address"
        static let clear = "Clear"
        static let requests = "Requested payments history"
        static let show = "Show"
        static let remove = "Remove"
        static let date = "Date"
        static let qrLabel = "QR code of the payment URI"
        static let noRequests = "No payment requests yet."
        static let backToAddress = "Show my address"
    }

    enum Transactions {
        static let date = "Date"
        static let type = "Type"
        static let addressLabel = "Address / Label"
        static let amount = "Amount"
        static let status = "Status"
        static let empty = "No transactions match the filter."
        static let loadMore = "Load more"
        static let from = "From"
        static let to = "To"
        static let watchOnly = "Watch-only"
        static let watchOnlyAll = "All"
        static let watchOnlyYes = "Yes"
        static let watchOnlyNo = "No"
        static let details = "Transaction details"
        static let editLabel = "Edit Label…"
        static let copyTxid = "Copy Transaction ID"
        static let copyAddress = "Copy Address"
        static let showDetails = "Show Transaction Details"
        static let exportName = "transactions.csv"
        static let txid = "Transaction ID"
        static let block = "Block"
        static let fee = "Fee"
        static let size = "Size"
        static let inputs = "Inputs"
        static let outputs = "Outputs"
        static let label = "Label"
        static let rawHex = "Raw transaction"
        static let mine = "mine"
        static let change = "change"
        static func bytes(_ count: UInt32) -> String { "\(count) bytes" }
        static func matching(_ count: Int) -> String { count == 1 ? "1 transaction" : "\(count) transactions" }
    }

    enum AddressBook {
        static let sending = "Sending"
        static let receiving = "Receiving"
        static let label = "Label"
        static let address = "Address"
        static let new = "New"
        static let choose = "Choose"
        static let exportName = "addresses.csv"
        static let windowTitle = "Address Book"
        static let empty = "No addresses."
    }

    enum SignVerify {
        static let signTab = "Sign Message"
        static let verifyTab = "Verify Message"
        static let signIntro =
            "You can sign messages/agreements with your addresses to prove you can receive Dash sent to them. Be careful not to sign anything vague or random, as phishing attacks may try to trick you into signing your identity over to them. Only sign fully-detailed statements you agree to."
        static let verifyIntro =
            "Enter the receiver's address, message (ensure you copy line breaks, spaces, tabs, etc. exactly) and signature below to verify the message. Be careful not to read more into the signature than what is in the signed message itself, to avoid being tricked by a man-in-the-middle attack. Note that this only proves the signing party receives with the address, it cannot prove sendership of any transaction!"
        static let address = "Address"
        static let message = "Message"
        static let signature = "Signature"
        static let sign = "Sign Message"
        static let verify = "Verify Message"
        static let clearAll = "Clear All"
        static let copySignature = "Copy the current signature to the system clipboard"
    }

    enum Settings {
        static let general = "General"
        static let display = "Display"
        static let security = "Security"
        static let network = "Network"
        static let networkHelp = "Each network has its own wallets. Switching closes the current network."
        static let unit = "Unit to show amounts in"
        static let decimalDigits = "Decimal digits"
        static let discreet = "Discreet mode"
        static let discreetHelp = "Mask the Overview balances and hide recent transactions."
        static let theme = "Theme"
        static let language = "User interface language"
        static let menuBar = "Show Dash Wallet in the menu bar"
        static let vault = "Wallet encryption"
        static let encrypted = "Encrypted"
        static let unencrypted = "Not encrypted"
        static let noVault = "No wallet keys yet"
        static let phraseTitle = "Recovery phrase"
        static let phraseWarning = "Anyone with these words can take your funds. Do not share them."
        static let bip39 = "BIP39 passphrase"
        static let dataFolder = "Open data folder"
        static let english = "English"
    }

    enum MenuBar {
        static let receive = "Receive"
        static let noAddress = "No receiving address yet"
    }
}
#endif
