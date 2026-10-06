import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt's Masternodes copy (`masternodelist.cpp`, `masternodemodel.cpp`,
    /// `masternodewizard.cpp`, `masternodedialogs.cpp`, `sharedmn*.cpp`;
    /// research 02 §10) and iOS's masternode tools (IOS-080…083).
    public enum Masternodes {
        // Toolbar and list (QT-118…121)
        public static let typeAll = "All"
        public static let typeRegular = "Regular"
        public static let typeEvo = "Evo"
        public static let typeShared = "Shared"
        public static func typeFilter(_ filter: MasternodeTypeFilter) -> String {
            switch filter {
            case .all: typeAll
            case .regular: typeRegular
            case .evo: typeEvo
            case .shared: typeShared
            }
        }
        public static let filterPlaceholder = "Filter by any property (e.g. address or protx hash)"
        public static let owned = "Owned"
        public static let hideBanned = "Hide banned"
        public static let registerButton = "Register Masternode…"
        public static let sharedButton = "Shared Masternode…"
        public static let nodeCount = "Node Count:"
        public static let registerTip = "Register a new masternode or EvoNode using this wallet"
        public static let registerNeedsWallet = "Registering a masternode requires a wallet."
        public static let registerNeedsKeys = "Masternode registration requires a wallet with private keys."
        public static let sharedTip = "Create or continue a multi-party shared masternode registration session"
        public static let sharedNeedsWallet = "Managing shared masternodes requires a wallet."
        public static let sharedNeedsKeys = "Managing shared masternodes requires a wallet with private keys."
        public static let listSyncing = "The masternode list is still syncing; only this wallet's masternodes are shown."
        public static let listUnavailable = "The masternode list is not available yet."

        public static let columnStatus = "Status"
        public static let columnService = "Service"
        public static let columnType = "Type"
        public static let columnPoSe = "PoSe Score"
        public static let columnRegistered = "Registered"
        public static let columnLastPaid = "Last Paid"
        public static let columnNextPayment = "Next Payment"
        public static let columnOperatorReward = "Operator Reward"
        public static let columnProTxHash = "ProTx Hash"
        public static let dash = "—"
        public static let fullNodeOnly = "Requires full-node data source"
        public static let unknown = "UNKNOWN"
        public static let regular = "Regular"
        public static let evo = "Evo"
        public static func sharedHolding(_ held: Int, of total: Int) -> String { "Shared (you hold \(held) of \(total))" }
        public static func activeFor(_ duration: String) -> String { "Active for \(duration)" }
        public static func bannedFor(_ duration: String) -> String { "Banned for \(duration)" }
        public static let active = "Active"
        public static let banned = "Banned"
        public static let retired = "Revoked"
        public static let statusUnknown = "Unknown"
        public static let operatorRewardNone = "NONE"
        public static func operatorReward(_ percent: String, to address: String?) -> String {
            address.map { "\(percent)% to \($0)" } ?? "\(percent)% but not claimed"
        }

        // Context menu (QT-121)
        public static let copyProTxHash = "Copy ProTx Hash"
        public static let copyCollateralOutpoint = "Copy Collateral Outpoint"
        public static let updateService = "Update Service…"
        public static let updateRegistrar = "Update Registrar…"
        public static let changeRewardAddress = "Change Reward Address…"
        public static let rotateKeys = "Rotate Keys…"
        public static let dissolve = "Dissolve…"
        public static let createStandby = "Create Standby Dissolution…"
        public static let revoke = "Revoke…"
        public static let filterBy = "Filter by"
        public static let collateralAddress = "Collateral Address"
        public static let payoutAddress = "Payout Address"
        public static let ownerAddress = "Owner Address"
        public static let votingAddress = "Voting Address"
        public static let showDetails = "Show details"
        public static let track = "Track Masternode"
        public static let needsSigningWallet = "Requires a wallet capable of signing transactions"
        public static let needsOwnerKey = "Requires this masternode's owner key in the wallet"
        public static let needsShareOwnerKey = "Requires one of this masternode's share owner keys in this wallet"
        public static func standbySaved(_ date: String) -> String { "Already saved on this computer on \(date)" }
        public static let standbyNotSaved = "Not created on this computer yet"
        public static let filterOwnerShared = "Filters by one of this masternode's share owner addresses"

        // Details (QT-122, IOS-080)
        public static func detailsTitle(_ hash: String) -> String { "Details for Masternode \(hash)" }
        public static let fieldProTxHash = "ProTx Hash"
        public static let fieldOperatorKey = "Public Key Operator"
        public static let fieldOwner = "Owner Address"
        public static let fieldPayout = "Payout Address"
        public static let fieldVoting = "Voting Address"
        public static let fieldCollateralAddress = "Collateral Address"
        public static let fieldCollateralHash = "Collateral Hash"
        public static let fieldCollateralIndex = "Collateral Index"
        public static let fieldType = "Masternode Type"
        public static let fieldRegistered = "Registered Height"
        public static let fieldLastPaid = "Last Paid Height"
        public static let fieldConsecutivePayments = "Consecutive Payments"
        public static let fieldOperatorReward = "Operator Reward"
        public static let fieldPoSePenalty = "PoSe Penalty"
        public static let fieldPoSeBan = "PoSe Ban Height"
        public static let fieldPoSeRevived = "PoSe Revived Height"
        public static let fieldService = "Network Addresses"
        public static let fieldPlatformP2P = "Platform P2P Addresses"
        public static let fieldPlatformHTTPS = "Platform HTTPS Addresses"
        public static let fieldPlatformNodeID = "Platform Node ID"
        public static let fieldShares = "Shares"
        public static let fieldEarlyPeriod = "Early period ends at block"
        public static let fieldEarlyPenalty = "Early-exit penalty"
        public static let fieldStandby = "Standby dissolution"
        public static let fieldRevocation = "Revocation reason"
        public static let fieldWalletTransactions = "Wallet transactions"
        public static let fieldOwnedBecause = "Yours because"
        public static func share(_ number: Int) -> String { "Share \(number)" }
        public static let yourShare = "Yours"
        public static let standbySaved = "Saved"
        public static let standbyNone = "None"
        public static let platformStatus = "Platform"
        public static let claimableCredits = "Claimable balance"
        public static let epochBlocks = "Blocks proposed this epoch"
        public static let notAvailableYet = "Not available yet"

        public static func roleName(_ role: MasternodeKeyRole) -> String {
            switch role {
            case .owner: "Owner"
            case .voting: "Voting"
            case .operator: "Operator"
            case .platformNode: "Platform node"
            case .ownerPayout: "Owner payout"
            case .operatorPayout: "Operator payout"
            }
        }

        public static func ownedRole(_ role: OwnedRole) -> String {
            switch role {
            case .collateral: "Collateral"
            case .owner: "Owner key"
            case .voting: "Voting key"
            case .operator: "Operator key"
            case .payout: "Payout address"
            case .operatorPayout: "Operator payout address"
            case .platformNode: "Platform node key"
            case .shareOwner: "Share owner key"
            case .shareRefund: "Share refund address"
            case .tracked: "Tracked"
            }
        }

        public static func refusal(_ refusal: CollateralRefusal) -> String {
            switch refusal {
            case .wrongAmount: "The output is not exactly the collateral amount."
            case .unconfirmed: "The output needs at least 1 confirmation."
            case .notP2PKH: "The collateral must be a P2PKH output."
            case .locked: "The output is locked."
            case .alreadyCollateral: "The output is already a masternode collateral."
            case .notFound: "The output was not found."
            }
        }

        public static func revocationReason(_ reason: RevocationReason) -> String {
            switch reason {
            case .notSpecified: "Not specified"
            case .terminationOfService: "Termination of service"
            case .compromisedKeys: "Compromised keys"
            case .changeOfKeys: "Change of keys"
            }
        }

        // Register wizard (QT-123, QT-124)
        public static let registerMasternode = "Register Masternode"
        public static let registerEvoNode = "Register EvoNode"
        public static func step(_ n: Int, of total: Int, _ title: String) -> String { "Step \(n) of \(total) · \(title)" }
        public static let pageType = "Masternode type"
        public static let pageCollateral = "Collateral"
        public static let pageService = "Service addresses"
        public static let pageKeys = "Keys"
        public static let pagePayout = "Payout"
        public static let pagePlatform = "Platform services"
        public static let pageFee = "Fee source"
        public static let pageReview = "Review"
        public static let pageSaveKey = "Save operator key"
        public static let pageSign = "Prove collateral ownership"
        public static let pageComplete = "Complete"
        public static func typeMasternode(_ amount: String) -> String { "Masternode — \(amount) collateral" }
        public static func typeEvoNode(_ amount: String) -> String { "EvoNode — \(amount) collateral" }
        public static let typeMasternodeHint = "Provides Core network services and earns regular masternode rewards."
        public static let typeEvoNodeHint =
            "Additionally hosts Dash Platform, has four times the voting weight and earns a share of Platform fees."
        public static let collateralFund = "Send collateral from this wallet to a new address"
        public static let collateralFundHint = "A single transaction funds the collateral and registers the masternode."
        public static let collateralExisting = "Use an existing collateral output of this wallet"
        public static let collateralExistingHint =
            "An unspent P2PKH output of exactly the collateral amount, confirmed and not already used."
        public static let collateralExternal = "Reference an external collateral (e.g. hardware wallet)"
        public static let collateralExternalHint =
            "After review you will be asked to sign a message with the collateral key outside this wallet."
        public static let collateralTxidPlaceholder = "Collateral transaction id (64 hexadecimal characters)"
        public static func noCollateralOutput(_ amount: String) -> String {
            "This wallet has no unspent output of exactly \(amount) with a confirmation. Send the collateral from this wallet instead."
        }
        public static let serviceHint =
            "Public addresses your masternode will serve the Core P2P network on, separated by commas or spaces."
        public static let serviceOptional =
            "May be left empty; the masternode then stays inactive until you send a service update."
        public static func defaultPort(_ port: UInt16) -> String { "Default port: \(port)" }
        public static let ownerHint = "Controls this masternode (P2PKH): its key signs registrar updates."
        public static let votingHint = "May be delegated (P2PKH). Leave empty to vote with the owner key."
        public static let votingPlaceholder = "Leave empty to use the owner address"
        public static let operatorHint = "The operator runs the masternode server (BLS); only the public key is registered."
        public static let operatorGenerate = "Generate a new operator key"
        public static let operatorExisting = "Use an existing operator public key"
        public static let operatorGenerateHint =
            "A generated secret key is shown and must be confirmed before registering. It is never stored."
        public static let useNewAddress = "Use new address"
        public static let payoutHint = "Receives this masternode's block rewards (P2PKH or P2SH)."
        public static let operatorRewardTitle = "Operator reward"
        public static let operatorRewardHint = "Share of the reward promised to the operator."
        public static let operatorRewardWarning =
            "The operator will permanently receive this share of all rewards of this masternode."
        public static let platformNodeIDHint = "Derived from the Platform P2P public key (40 hexadecimal characters)."
        public static let platformAddressesHint = "ADDR:PORT entries, separated by commas or spaces."
        public static let platformP2P = "Platform P2P"
        public static let platformHTTPS = "Platform HTTPS API"
        public static let feeSourceHint = "The selected address pays the transaction fee."
        public static func feeSourceFundHint(_ amount: String) -> String {
            "The selected address funds the \(amount) collateral plus the transaction fee."
        }
        public static let noFeeSource = "No spendable wallet address has a positive balance available to pay the transaction fee."
        public static let reviewNetworkFee = "Network fee"
        public static let reviewTotal = "Total"
        public static let saveKeyHint =
            "Save this generated key before registering. It is kept nowhere else and cannot be shown again."
        public static let operatorSecretTitle = "Operator secret key"
        public static let saveKeyNote = "Save it now — registration cannot start until you confirm it."
        public static let configLineHint = "Add this line to dash.conf on your masternode server:"
        public static let typeLast4 = "Type the last 4 characters of the secret key to confirm you saved it"
        public static let last4Mismatch = "The characters do not match the end of the secret key."
        public static let signHint =
            "Sign the following message with the collateral key (for example with a hardware wallet), then paste the signature."
        public static let signaturePlaceholder = "Paste the base64 signature here"
        public static let signatureMissing =
            "Paste the base64-encoded signature of the message above, made with the collateral key."
        public static let completeTitle = "Masternode registered"
        public static let proTxHashTitle = "Provider transaction hash"
        public static let nextStepsTitle = "Next steps"
        public static let nextStepsMasternode =
            "Start your masternode server with the operator key. It joins the list once the transaction confirms."
        public static let next = "Next"
        public static let back = "Back"
        public static let cancel = "Cancel"
        public static let continueTitle = "Continue"
        public static let prepare = "Prepare"
        public static let register = "Register"
        public static let submit = "Submit"
        public static let finish = "Finish"
        public static let backUnavailable =
            "Back is unavailable after preparation because changing the answers would need a new transaction."

        // Field validation (`masternodewizard.cpp` `validateCurrentPage`)
        public static let enterTxid = "Enter the collateral transaction id as 64 hexadecimal characters."
        public static let evoNeedsService =
            "EvoNodes need at least one service address to carry the Platform ports before v24 activation."
        public static let enterOwner = "Enter a valid owner address (P2PKH)."
        public static let enterVoting = "Enter a valid voting address (P2PKH), or leave it empty to use the owner address."
        public static let keysDifferFromCollateral = "The owner and voting addresses must differ from the collateral address."
        public static let enterOperatorKey = "Enter a valid operator BLS public key (96 hexadecimal characters, basic scheme)."
        public static let enterPayout = "Enter a valid payout address (P2PKH or P2SH)."
        public static let payoutDiffersFromKeys = "The payout address must differ from the owner and voting addresses."
        public static let payoutDiffersFromCollateral = "The payout address must differ from the collateral address."
        public static let enterNodeID = "Enter the Platform node ID as 40 hexadecimal characters."
        public static let enterPlatformBoth =
            "Enter at least one Platform P2P and one Platform HTTPS address, or clear the service addresses."
        public static let rewardRange = "Enter an operator reward between 0.00 and 100.00 %."
        public static let enterService = "Enter service addresses as IP:port, separated by commas or spaces."

        // Maintenance (QT-125, IOS-081)
        public static let updateServiceTitle = "Update Service"
        public static let updateRegistrarTitle = "Update Registrar"
        public static let revokeTitle = "Revoke Masternode"
        public static let unbanTitle = "Unban Masternode"
        public static let operatorSecretField = "Operator secret key"
        public static let operatorSecretFieldHint =
            "Typed each time; it must match the masternode's registered operator public key. Leave it empty to use a key this wallet holds."
        public static let operatorPayoutField = "Operator payout address"
        public static let feeSourceAutomatic = "Automatic (recommended)"
        public static let revivesBanned = "Sending a service update revives a PoSe-banned masternode."
        public static let bansWarning =
            "Changing the operator key immediately PoSe-bans the masternode until the new operator sends a service update."
        public static let multiplePayoutsNote = "This masternode pays several addresses; use the RPC to change them."
        public static let revokeReason = "Reason"
        public static let revokeWarning =
            "Revoking ends this masternode's operator service. The owner must register a new operator to bring it back."
        public static let reviewTitle = "Review"
        public static func reviewFee(_ fee: String) -> String { "Network fee: \(fee)" }
        public static func reviewPenalty(_ penalty: String) -> String { "Early-exit penalty: \(penalty)" }
        public static func sent(_ txid: String) -> String { "Transaction sent: \(txid)" }
        public static let unbanPending =
            "Service update sent. The masternode stays banned until the transaction confirms."
        public static let noChanges = "Nothing changed."
        public static let send = "Send"

        // Shared masternodes (QT-126, QT-127)
        public static let sharedTitle = "Shared Masternode"
        public static let sharedCreate = "Create a shared masternode"
        public static let sharedJoin = "Paste a message from the coordinator"
        public static let sharedSessions = "Open sessions"
        public static func sessionCode(_ code: String) -> String { "Session \(code)" }
        public static func fingerprint(_ fingerprint: String) -> String { "Fingerprint \(fingerprint)" }
        public static let fingerprintHint =
            "Compare the fingerprint with the other participants over a separate channel before you continue."
        public static func stage(_ stage: SharedStage) -> String {
            switch stage {
            case .invitation: "Invitation"
            case .details: "Details"
            case .lockedTerms: "Locked terms"
            case .approvals: "Approvals"
            case .signingRequest: "Signing request"
            case .signedContributions: "Signed contributions"
            case .broadcast: "Broadcast"
            case .completed: "Completed"
            case .abandoned: "Abandoned"
            }
        }
        public static func purpose(_ purpose: SharedSessionPurpose) -> String {
            switch purpose {
            case .register: "Registration"
            case .rotateKeys: "Key rotation"
            case .dissolveTogether: "Dissolve together"
            }
        }
        public static let roleCoordinator = "Coordinator"
        public static let roleParticipant = "Participant"
        public static let copyMessage = "Copy message"
        public static let saveMessage = "Save message…"
        public static let pasteMessage = "Paste"
        public static let messageCopied = "The message was copied. Send it to the other participants."
        public static let sharesTitle = "Shares"
        public static let sharesRule = "2–8 shares of at least 100 DASH each, adding up to exactly 1000 DASH."
        public static let sharesTotalWrong = "The shares must add up to exactly 1000 DASH."
        public static let shareTooSmall = "Each share must be at least 100 DASH."
        public static let shareCountWrong = "A shared masternode has 2 to 8 shares."
        public static let earlyPeriod = "Early period (blocks)"
        public static let earlyPeriodTooLong = "The early period is at most 420480 blocks."
        public static let penalty = "Early-exit penalty"
        public static let penaltyTooLarge = "The penalty must be smaller than the smallest share."
        public static let contribute = "Reserve coins"
        public static let approve = "Approve"
        public static let sign = "Sign"
        public static let broadcastShared = "Broadcast"
        public static let closeProtectionTitle = "Leave this session?"
        public static let closeProtectionMessage =
            "Coins are reserved for this session. Save the session to continue later, or release the coins."
        public static let keepSession = "Save and close"
        public static let releaseCoins = "Release coins"
        public static let reservedCoinSpent = "A coin reserved for this session was spent elsewhere. Release and start again."
        public static let dissolveNow = "Dissolve now"
        public static let dissolveNowHint = "Unilateral: returns your share now."
        public static let dissolveTogether = "Dissolve together"
        public static let dissolveTogetherHint = "Unanimous: returns every share's principal."
        public static let acceptPenalty = "I accept the early-exit penalty"
        public static let standbyHint =
            "Creates two raw transactions you can save and broadcast later, for example if this computer is lost."
        public static let standbyBroadcastQuestion = "Broadcast this standby dissolution?"
        public static func standbySent(_ txids: [String]) -> String { "Standby dissolution sent: \(txids.joined(separator: ", "))" }
        public static let pasteRoutedToSession = "The message was added to its session."
        public static let pasteIsStandby = "This text is a standby dissolution."

        // Keychain (IOS-083)
        public static let keychainTitle = "Masternode Keys"
        public static func keyRole(_ role: MasternodeKeyRole) -> String {
            switch role {
            case .owner: "Owner keys"
            case .voting: "Voting keys"
            case .operator: "Operator keys (BLS)"
            case .platformNode: "Platform node keys (Ed25519)"
            case .ownerPayout: "Owner payout addresses"
            case .operatorPayout: "Operator payout addresses"
            }
        }
        public static func keyIndex(_ index: UInt32) -> String { "Key \(index)" }
        public static let derivationPath = "Derivation path"
        public static let address = "Address"
        public static let publicKey = "Public key"
        public static let legacyPublicKey = "Public key (legacy scheme)"
        public static let platformNodeID = "Platform node ID"
        public static let privateKey = "Private key"
        public static let wif = "WIF"
        public static let tenderdashKey = "Tenderdash private key"
        public static let reveal = "Reveal private key"
        public static let hide = "Hide"
        public static let unused = "Unused"
        public static func usedBy(_ hash: String, service: String?) -> String {
            service.map { "Used by \(hash) (\($0))" } ?? "Used by \(hash)"
        }
        public static let revokedSuffix = " — revoked"
        public static let loadMore = "Show more keys"

        // Tracked masternodes (IOS-082)
        public static let trackedTitle = "Tracked Masternodes"
        public static let locatePlaceholder = "IP, IP:port, ProTx hash, address or operator key"
        public static let locate = "Find"
        public static let noMatches = "No masternode matches."
        public static let untrack = "Stop tracking"
        public static let label = "Label"
        public static let attachKey = "Attach key…"
        public static let detachKey = "Remove key"
        public static let attachKeyHint = "WIF or hex (secp256k1), hex (BLS), hex or base64 (Ed25519). Stored in the vault."
        public static let capabilityWithdraw = "Withdraw credits"
        public static let capabilityUpdateService = "Update service / unban"
        public static let capabilityUpdateRegistrar = "Update registrar"
        public static let capabilityVote = "Vote"

        // Evonode (IOS-081)
        public static let withdrawTitle = "Withdraw credits"
        public static let withdrawToPayout = "To the payout address"
        public static let withdrawToAddress = "To another address (owner key only)"
        public static func withdrawn(_ id: String) -> String { "Withdrawal submitted: \(id)" }
    }
}
