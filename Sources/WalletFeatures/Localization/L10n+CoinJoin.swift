import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt's CoinJoin copy (`overviewpage.cpp`, `optionsdialog.ui`,
    /// `utilitydialog.cpp`, Core `coinjoin/*.cpp`; research 02 §9) and iOS's
    /// "move mixed coins" copy (IOS-057).
    public enum CoinJoin {
        public static let name = "CoinJoin"

        // Overview panel (QT-041…044)
        public static let panelTitle = "CoinJoin"
        public static let status = "Status"
        public static let completion = "Completion"
        public static let balance = "CoinJoin Balance"
        public static let amountAndRounds = "Amount and Rounds"
        public static let submittedDenominations = "Submitted Denom"
        public static let notApplicable = "n/a"
        public static let enabled = "Enabled"
        public static let disabled = "Disabled"
        public static let start = "Start CoinJoin"
        public static let stop = "Stop CoinJoin"
        public static let disabledButton = "(Disabled)"
        public static let outOfSync = "(out of sync)"
        public static func keysLeft(_ count: Int) -> String { "keys left: \(count)" }
        public static func rounds(_ n: Int) -> String { n == 1 ? "\(n) Round" : "\(n) Rounds" }
        public static let noInputsDetected = "No inputs detected"
        public static func enoughInputs(_ amount: String) -> String { "Found enough compatible inputs to mix \(amount)" }
        public static func notEnoughInputs(target: String, instead: String) -> String {
            "Not enough compatible inputs to mix \(target), will mix \(instead) instead"
        }
        public static let overallProgress = "Overall progress"
        public static let denominated = "Denominated"
        public static let partiallyMixed = "Partially mixed"
        public static let mixed = "Mixed"
        public static func averageRounds(_ average: String, of rounds: Int) -> String {
            "Denominated inputs have \(average) of \(rounds) rounds on average"
        }
        public static let mostCommonHint =
            "If you don't want to see internal CoinJoin fees/transactions select \"Most Common\" as Type on the \"Transactions\" tab."
        public static func minimumBalance(_ amount: String) -> String { "CoinJoin requires at least \(amount) to use." }
        public static let unlockForMixingTitle = "Unlock wallet for mixing only"
        public static let unlockForMixingMessage =
            "This operation needs your wallet passphrase to unlock the wallet for mixing only. Sending stays locked."
        public static let declinedToUnlock = "Wallet is locked and user declined to unlock. Disabling CoinJoin."
        public static let mixingStoppedLocked = "Mixing stopped because the wallet was locked."
        public static let watchOnlyTooltip = "Watch-only wallets cannot mix."
        public static let disabledTooltip = "CoinJoin is disabled in Options ▸ Wallet."

        // Session status (QT-050, Core `strAutoDenomResult` / `GetStatus`)
        public static let sessionStatus = "Mixing status"
        public static let queueSize = "Queue size"
        public static let sessions = "Sessions"
        public static func status(_ code: CoinJoinStatusCode) -> String {
            switch code {
            case .idle: "CoinJoin is idle."
            case .syncInProgress: "Can't mix while sync in progress."
            case .walletLocked: "Wallet is locked."
            case .mixingInProgress: "Mixing in progress…"
            case .noMasternodes: "No Masternodes detected."
            case .notEnoughFunds: "Not enough funds to mix."
            case .unconfirmedDenominated:
                "Found unconfirmed denominated outputs, will wait till they confirm to continue."
            case .noCompatibleMasternode: "No compatible Masternode found."
            case .noCompatibleInputs: "Can't mix: no compatible inputs found!"
            case .tryingToConnect: "Trying to connect…"
            case .noQueueToJoin: "Failed to find mixing queue to join"
            case .noRandomMasternode: "Can't find random Masternode."
            case .failedToStartQueue: "Failed to start a new mixing queue"
            case .waitingInQueue: "Submitted to masternode, waiting in queue ."
            case .signing: "Found enough users, signing…"
            case .masternode(let message): "Masternode: \(poolMessage(message))"
            }
        }

        /// Core `CoinJoin::GetMessageByID`.
        public static func poolMessage(_ message: CoinJoinPoolMessage) -> String {
            switch message {
            case .alreadyHave: "Already have that input."
            case .denom: "No matching denominations found for mixing."
            case .entriesFull: "Entries are full."
            case .existingTx: "Not compatible with existing transactions."
            case .fees: "Transaction fees are too high."
            case .invalidCollateral: "Collateral not valid."
            case .invalidInput: "Input is not valid."
            case .invalidScript: "Invalid script detected."
            case .invalidTx: "Transaction not valid."
            case .maximum: "Entry exceeds maximum size."
            case .mnList: "Not in the Masternode list."
            case .mode: "Incompatible mode."
            case .queueFull: "Masternode queue is full."
            case .recent: "Last queue was created too recently."
            case .session: "Session not complete!"
            case .missingTx: "Missing input transaction information."
            case .version: "Incompatible version."
            case .noError: "No errors detected."
            case .success: "Transaction created successfully."
            case .entriesAdded: "Your entries added successfully."
            case .sizeMismatch: "Inputs vs outputs size mismatch."
            case .nonStandardPubkey, .notAMasternode: "Unknown response."
            }
        }

        public static func poolState(_ state: CoinJoinPoolState) -> String {
            switch state {
            case .idle: "Idle"
            case .queue: "Queued"
            case .acceptingEntries: "Accepting entries"
            case .signing: "Signing"
            case .error: "Error"
            }
        }

        // Options ▸ CoinJoin (QT-046, QT-047; `optionsdialog.ui`)
        public static let optionsTab = "CoinJoin"
        public static let enableFeatures = "Enable CoinJoin features"
        public static let enableFeaturesTip =
            "Show mixing interface on Overview screen and reveal an additional screen which allows to spend fully mixed coins only."
        public static let advancedInterface = "Enable advanced interface"
        public static let showPopups = "Show popups for mixing transactions"
        public static let showPopupsTip = "Show system popups for mixing transactions just like for all other transaction types."
        public static let lowKeysWarning = "Warn if the wallet is running out of keys"
        public static let lowKeysWarningTip = "Show warning dialog when the wallet has very low number of keys left."
        public static let lowKeysNotApplicable = "HD wallets do not run out of keys; this warning never shows."
        public static let multiSession = "Enable multi-session"
        public static let multiSessionTip =
            "Whether to use experimental mode with multiple mixing sessions per block. Note: You must use this feature carefully."
        public static let parallelSessions = "Parallel sessions"
        public static let parallelSessionsTip = "Use this many separate masternodes in parallel to mix funds."
        public static let mixingRounds = "Mixing rounds"
        public static let mixingRoundsTip =
            "This setting determines the amount of individual masternodes that an input will be mixed through. More rounds of mixing gives a higher degree of privacy, but also costs more in fees."
        public static let targetBalance = "Target balance"
        public static let targetBalanceTip = "This amount acts as a threshold to turn off mixing once it's reached."
        public static let inputsPerDenomination = "Inputs per denomination"
        public static let inputsPerDenominationTip =
            "How many inputs of each denominated amount are created. Lower these numbers if you want fewer smaller denominations."
        public static let denomsTarget = "Target"
        public static let denomsTargetTip = "Try to create at least this many inputs for each denominated amount."
        public static let denomsMaximum = "Maximum"
        public static let denomsMaximumTip = "Create up to this many inputs for each denominated amount."
        public static let invalidSetting = "One of the CoinJoin values is outside its allowed range."
        public static let saltTitle = "CoinJoin salt"
        public static let saltInvalid = "The salt must be 64 lowercase hexadecimal characters."
        public static let saltWhileMixing = "Stop mixing before changing the salt."

        // Send page (QT-051)
        public static let mixedBalance = "CoinJoin Balance"
        public static let sendMixedFunds = "Send mixed funds"

        // Help ▸ CoinJoin information (QT-153, `utilitydialog.cpp`, adapted:
        // HD wallets need no keypool backups, DESIGN-opus §7.5)
        public static let informationTitle = "CoinJoin information"
        public static let informationSections: [(title: String, body: String)] = [
            ("CoinJoin Basics",
             "CoinJoin gives you true financial privacy by obscuring the origins of your funds. All the Dash in your wallet is comprised of different \"inputs\" which you can think of as separate, discrete coins. CoinJoin uses an innovative process to mix your inputs with the inputs of two or more other people, without having your coins ever leave your wallet. You retain control of your money at all times."),
            ("The CoinJoin process works like this:",
             """
             1. CoinJoin begins by breaking your transaction inputs down into standard denominations. These denominations are 0.001 DASH, 0.01 DASH, 0.1 DASH, 1 DASH and 10 DASH -- sort of like the paper money you use every day.
             2. Your wallet then sends requests to specially configured software nodes on the network, called "masternodes." These masternodes are informed then that you are interested in mixing a certain denomination. No identifiable information is sent to the masternodes, so they never know "who" you are.
             3. When two or more other people send similar messages, indicating that they wish to mix the same denomination, a mixing session begins. The masternode mixes up the inputs and instructs all three users' wallets to pay the now-transformed input back to themselves. Your wallet pays that denomination directly to itself, but in a different address (called a change address).
             4. In order to fully obscure your funds, your wallet must repeat this process a number of times with each denomination. Each time the process is completed, it's called a "round." Each round of CoinJoin makes it exponentially more difficult to determine where your funds originated.
             5. This mixing process happens in the background without any intervention on your part. When you wish to make a transaction, your funds will already be mixed. No additional waiting is required.
             """),
            ("Your recovery phrase covers mixed coins",
             "This wallet derives its mixing addresses from your recovery phrase, so mixing never runs out of addresses and needs no extra backups. Restoring the phrase restores your mixed coins."),
        ]
        public static let documentationTitle = "CoinJoin documentation"
        public static let documentationURL = URL(
            string: "https://docs.dash.org/en/stable/wallets/dashcore/coinjoin-instantsend.html")!

        // Move mixed coins (IOS-057)
        public static let recoveryScan = "Scan for CoinJoin funds"
        public static let recoveryScanInfo =
            "Rescans the CoinJoin account and the regular account with a wide address window to find mixed coins another wallet created."
        public static let recoveryScanning = "Scanning for CoinJoin funds…"
        public static func recoveryResult(addresses: Int, balance: String, transactions: Int) -> String {
            "Scanned \(addresses) addresses: \(transactions) new transaction\(transactions == 1 ? "" : "s"), CoinJoin balance \(balance)."
        }
        public static let moveTitle = "Move mixed coins"
        public static let moveInfo =
            "Moves your fully mixed coins to a regular address of this wallet, so they can be spent like any other Dash."
        public static let destinationWallet = "Dash Wallet"
        public static let destinationShielded = "Shielded"
        public static func movePlan(total: String, transactions: Int, fee: String) -> String {
            "\(total) in \(transactions) transaction\(transactions == 1 ? "" : "s"), fee \(fee)"
        }
        public static let moveButton = "Move"
        public static let later = "Later"
        public static let moving = "Moving mixed coins…"
        public static func moved(_ amount: String) -> String { "Moved \(amount) to your wallet." }
        public static func movedPartially(moved: String, remaining: String, reason: String) -> String {
            "Moved \(moved); \(remaining) could not be moved yet: \(reason) You can try again later."
        }
        public static let withdrawalsTitle = "CoinJoin Withdrawals"
        public static let withdrawalsInfo = "Your mixed Dash was moved to your spendable balance using these transactions."
        public static func withdrawalsSummary(transactions: Int, inputs: Int) -> String {
            "\(transactions) transaction\(transactions == 1 ? "" : "s") · \(inputs) UTXO\(inputs == 1 ? "" : "s")"
        }
        public static func moveBanner(_ amount: String) -> String { "You have \(amount) of mixed coins you can move." }
    }
}
